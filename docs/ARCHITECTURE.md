# Architecture

cadocs is a self-hosted collaborative wiki. It targets the main feature set of Docmost in a fraction of the memory: 10 to 30 MB idle and under 100 MB for a few dozen active users, on a small VPS or a Raspberry Pi. It ships as one binary and one data folder, with no mandatory external service.

Decisions and their rejected alternatives live in [`adr/`](adr/). The schema is in [DATA_MODEL.md](DATA_MODEL.md), the routes and the wire protocol in [API.md](API.md), the access rules in [PERMISSIONS.md](PERMISSIONS.md), the crates in [CARGO_WORKSPACE.md](CARGO_WORKSPACE.md), and configuration and deployment in [OPERATIONS.md](OPERATIONS.md).

## Overview

```
 browser (Svelte + TipTap + Yjs, static build)
    │ HTTPS: REST, assets            │ WebSocket: y-sync per open page, events per tab
    ▼                                ▼
┌──────────────────────────── cadocs (one process) ────────────────────────────┐
│ axum router                                                                  │
│  ├─ static files (rust-embed)                                                │
│  ├─ REST handlers ── permissions (cadocs-core, pure)                         │
│  └─ /ws ──────────── DocRegistry ── Room per loaded page (yrs Doc, awareness)│
│                         │  batched flush, compaction, unload                 │
│ job worker (one task) ──┤                                                    │
│                         ▼                                                    │
│                 store traits ─── SQLite (WAL, FTS5)    file store            │
└─────────────────────────┼───────────────────────────────────┼────────────────┘
                          ▼                                   ▼
                  data/cadocs.db                        data/files/ (or S3)
```

Everything runs in one process, on purpose (ADR 0006). Channels between components are in-memory tokio channels. There is no Redis, no separate worker and no Node runtime.

## Components

| Component | Role |
| --- | --- |
| HTTP layer | axum routes, session cookie, CSRF, rate limits, embedded front end |
| Permissions | Pure functions in `cadocs-core`: role and action in, decision out. No I/O, exhaustively tested |
| Store | Traits over the database. SQLite is the only implementation until V2 (ADR 0002) |
| DocRegistry | Maps a page id to its loaded room, loads on demand, unloads when idle |
| Room | One yrs `Doc` and `Awareness`, its connected clients, its pending updates |
| Event hub | One bounded in-memory broadcast per workspace with open event sockets (ADR 0011) |
| Job worker | One task polling the `jobs` table: indexing, revisions, cleanup |
| File store | Local disk by default, S3 chosen at runtime in builds with the `s3` feature (ADR 0005, ADR 0020) |
| Content | Walks a yrs `XmlFragment` into an AST, renders Markdown, HTML or text (ADR 0008) |

## Flows

### HTTP

1. The request passes the connection limits and the session middleware (cookie, token hashed in the database).
2. The handler loads the membership rows it needs and asks `cadocs-core` for a decision.
3. Reads go through a small pool of read connections. Writes go through a single writer connection, as SQLite allows one writer at a time.

Page metadata (title, tree position, permissions) lives in SQL tables. Page content lives only in Yjs state. REST never edits content directly: an import, for example, produces a Yjs update applied through the room.

### WebSocket

1. `GET /ws/pages/{id}` carries the session cookie. The server checks `Origin`, resolves the session, and computes the role **before** the upgrade. A failure is a plain HTTP 401, 403 or 404.
2. The registry returns the room, loading it if needed: latest snapshot, then every later row of `doc_updates`.
3. Both sides run the y-sync handshake (SyncStep1, SyncStep2), then exchange updates and awareness.
4. An update from a client with write access is applied to the `Doc`, broadcast to the other clients and queued for persistence. An update from a read-only client is dropped and answered with an auth message.
5. When access changes or the page is deleted, the HTTP layer wakes a task that rechecks every open socket against the database and closes those that lost something. A timer wakes it too. [PERMISSIONS.md](PERMISSIONS.md#reacting-to-changes) lists the triggers and close codes.

Awareness (cursors, names, colours) is never persisted. When a socket closes, the server removes the client ids it announced and broadcasts the removal, so a crashed tab does not leave a ghost cursor.

### Events

A page socket only covers its page. Changes to the tree and to titles reach other users through one events socket per tab (ADR 0011). A REST handler or a flush that changes the tree publishes to the workspace hub, and each socket forwards the events for spaces its user can read. Events carry ids only, and the client refetches through REST. The hubs live in `DocRegistry`, beside the rooms, because a flush publishes `page.renamed`, the socket count is shared, and one recheck pass covers both kinds of socket.

### Persistence and compaction

```
update ─► Room queue ─► flush every ~500 ms ─► INSERT INTO doc_updates (one transaction)
                                                   │
       more than N updates, or room unloading ─────┤
                                                   ▼
      one transaction: write doc_snapshots, delete the updates it covers
```

- A batch of updates is flushed as one transaction, so typing does not cost one fsync per keystroke.
- Compaction encodes the whole document as a single update, which also drops the per-keystroke structure. It needs the document loaded, and the room is loaded at both trigger points.
- One task per room does all its flushes and compactions, so they never overlap. A snapshot's `upto_seq` is the last stored `seq`, even if the state already holds edits still queued. Those keep a higher `seq`, so their flush is accepted, and replaying them changes nothing.
- The log stores what an update really changed, so a repeated update costs nothing. The exception is an update whose dependency is missing: yrs holds it outside the document, so the frame itself is stored too, or a restart would lose it. This covers blocks and deletes of blocks not seen yet.
- A document with an update still waiting does not compact. yrs merges waiting data into the encoded state, and that merge was seen to lose a delete. The log stays correct, so compaction resumes once nothing waits. That wait has an end: past 4 times `CADOCS_COMPACT_UPDATES` rows, the dependency counts as lost, and the snapshot holds what is applied, without what waits. Real clients send in order, and a connected writer resends what it sent after reconnecting, so what is lost for good is a crafted update, or one whose writer left and whose dependency never came.
- An update that cannot be applied at load is skipped with a warning, since it can turn out invalid only once its dependency arrives, and that document stops compacting so the row is never deleted. An update that cannot be decoded, or a snapshot that cannot be applied, makes the load fail.
- Durability window: an update lives only in memory until its flush. If the process dies while a client stays connected, that client resends what the server lacks when it reconnects, through the state vector exchange. If the client has also gone, up to one flush interval of edits is lost. This is the accepted trade-off, and the interval is configurable.
- Power loss or an OS crash can lose more. With `synchronous=NORMAL` (ADR 0002), WAL commits reach the disk at the next checkpoint, so the most recent transactions may roll back. The database stays consistent, and connected clients resend their edits as above. Edits from clients that have gone since the last checkpoint are lost.
- A graceful shutdown flushes every room before exiting, retrying a failed flush three times before giving up.
- If a flush fails, updates stay queued and the flush retries with backoff. When the room queue is full while a flush is failing, a frame with new blocks makes the room close its write sockets with `1013` instead of dropping edits. On working storage the limit is one frame higher, since the next flush drains the queue, so a large offline edit closes nobody. Clients keep them and resend after reconnecting ([OPERATIONS.md](OPERATIONS.md#failure-behaviour)).
- A flush that finds the page gone drops what the room holds and closes its sockets with `4404`. Retrying could not help, and the page's document went with it.

### Room lifecycle

```
          first client                    last client leaves
absent ───────────────► loading ─► live ─────────────────────► idle
   ▲                                ▲                            │
   │                                └── client returns ──────────┤
   └──────────── flush, compact, drop ◄── idle timeout ──────────┘
```

An unload whose flush fails puts the room back as live: its edits exist nowhere else. It retries once a flush succeeds.

Load and unload are serialized per document. A client that arrives during an unload waits for it to finish and then loads the persisted state. It must never receive a second copy of a document that is still being written.

Loaded rooms are bounded by the room memory budget, `CADOCS_ROOM_MEMORY_MB`, which counts each room from its encoded size (ADR 0022). Past it, the oldest idle room unloads first, skipping rooms whose flush is failing. If every room is live, the new connection gets a `503` before the upgrade.

### Reading a document outside a room

Indexing, exports, revisions and public pages need a document's content with no client connected. They go through one reader instead of creating a room:

- If the room is loaded, the reader encodes its `Doc` under the room lock, which includes unflushed edits, and works on a private copy, so a slow reader holds up no edit. A room that is unloading is waited for, because it still holds edits the store lacks.
- Otherwise it builds a temporary `Doc` from the snapshot and updates, and drops it after use. It writes nothing and is never registered.
- A semaphore (`CADOCS_DOC_READS`) bounds how many of these exist at once, so a crawler on public pages waits instead of raising RAM.

Anything that writes to a document (imports, title changes through REST, revision restores) goes through a real room.

### Jobs

The `jobs` table is the queue: `INSERT` to enqueue, one worker task to claim with a conditional `UPDATE`, retries with backoff. An in-memory `Notify` wakes the worker immediately, and a slow poll covers jobs left over from a restart. Jobs are idempotent, because a crash may run one twice.

Job types: reindex a page for search, create a revision, purge expired sessions and tokens, thin old revisions, purge the trash, delete orphaned files.

Recurring jobs are queued at start and then every hour, with their kind as their key. The last cleanup, `maintain_database`, gives free pages back to the disk and refreshes the planner statistics (ADR 0019). A job that fails is retried after 30 seconds, then twice as long each time, up to five runs. A claim counts as a run, so a job that kills the process also ends as `failed`. A panic is a failure, and costs the worker nothing.

### Search

A room flush marks the page dirty, and a debounced job extracts the text from the Yjs state and updates the FTS5 index (ADR 0007). Results are filtered by the spaces the user can read, inside the SQL query.

The flush queues the job in its own transaction, `REINDEX_DELAY` (5 seconds) ahead, with `reindex:<page id>` as its key: the edits of those seconds share one run, and a crash between the flush and the job loses nothing. The store's `jobs_added` wakes the worker when a write adds a job, so the worker does not wait for its poll. The job reads the document through the reader outside a room, renders it to plain text with `cadocs-content`, stems it with `cadocs-search`, and replaces the page's index in one transaction, with the attachment references it finds. A query is stemmed the same way, and its excerpts are made in Rust from the stored plain text.

## RAM budget

These figures are design estimates. The prototype's measurements are in [Measured results](#measured-results), and they take precedence.

| Component | Estimate | How the budget is held |
| --- | --- | --- |
| Code and static data actually touched | 4 to 8 MB | File-backed and reclaimable. The embedded front end costs nothing until served, and the kernel can evict it |
| tokio runtime, 2 workers | 1 to 2 MB | `worker_threads = 2`, small `max_blocking_threads`, explicit thread stack size |
| SQLite | 2 to 4 MB | Few connections, `cache_size` limited to 512 KiB each, and no `mmap_size` (ADR 0019). The OS page cache does the real caching and is not counted against the process |
| Per WebSocket | tens of KB | Bounded send queue, capped socket count. A slow client is disconnected, never buffered without limit |
| Per loaded document | a few times its encoded size | Unload after inactivity, compact on unload, capped room count |
| Peaks: password hashing | 19 MB per hash at OWASP settings | A semaphore allows one hash at a time, and a fixed mmap threshold unmaps its block when it ends (ADR 0023) |
| Peaks: uploads, imports and exports | bounded | Stream to disk, never buffer a whole file. A page export streams its zip through a 64 KiB pipe, and its content read is bounded by `CADOCS_DOC_READS`. One space export job at a time (V1) |

Other strategies:

- **Allocator.** Freed memory does not always return to the OS. The server stays on glibc and calls `malloc_trim` after a room unloads (ADR 0015). It fixes the mmap threshold at 4 MiB at start, so a larger block goes back as soon as it is freed (ADR 0023).
- **Dependencies.** Each crate must justify its cost ([CARGO_WORKSPACE.md](CARGO_WORKSPACE.md)).
- **No in-memory caches of database rows.** SQLite and the OS page cache are already fast enough at this scale.
- **Backpressure everywhere.** Every queue is bounded, so load shows up as latency or a refused connection instead of memory growth.

## What is measured, and how

`bench/rss.sh` reports four numbers from `/proc/<pid>/smaps_rollup` and `/proc/<pid>/status`:

- **RSS**, the figure usually quoted, which includes reclaimable file-backed pages;
- **PSS**, the same with shared pages divided among the processes sharing them;
- **Anonymous**, the heap and stacks, which is what really competes for RAM on a small machine;
- **Peak** (`VmHWM`), the highest RSS reached.

### Benchmark scenario

Fixed before the first measurement. A change after that needs a new ADR, and old results are measured again.

- **Build:** release binary, `x86_64-unknown-linux-gnu`, default settings from [OPERATIONS.md](OPERATIONS.md).
- **Document:** about 20 KB of text, about 3,000 words, built from 2,000 small edits. Generated by `cadocs-bench` from a fixed seed.
- **Load:** each open document has 2 clients. Each client sends 2 updates per second and 1 awareness update per second, for 60 seconds. RSS is read 30 seconds after the load stops, with clients still connected.

| Measure | Setup | Target |
| --- | --- | --- |
| Idle | Started on a database holding 100 documents, none open, no client ever connected | RSS 30 MB or less |
| 10 documents | 10 documents, 20 clients | RSS under 50 MB |
| 100 documents | 100 documents, 200 clients | RSS under 100 MB |
| After unload | All clients gone, `CADOCS_ROOM_IDLE_SECS` plus 10 seconds later | RSS at most idle plus 10 MB |

**Go or no-go.** Go if every target holds with at least one allocator. Go with conditions if a target is missed by under 20% and the cause is identified, with the fix recorded as an MVP task. Otherwise no-go, and a new ADR reworks the architecture before any MVP work.

### MVP scenario

Fixed in step 23, before its first measurement, for the exit criteria of the MVP in [ROADMAP.md](ROADMAP.md#mvp). `bench/rss.sh mvp` runs it. The same rule applies: a change needs a new ADR, and old results are measured again.

- **Build:** as above, glibc only. Every server is pinned with `taskset` to the first CPU the shell may use, for the 1 vCPU machine of the criteria, and sets `CADOCS_TRUSTED_PROXIES=127.0.0.1`, as behind the proxy of [OPERATIONS.md](OPERATIONS.md#deployment). The load tool is not pinned, so the scheduler may also put it on that CPU.
- **Database:** 10,000 pages in one space, each holding the benchmark document of the seed `1 + i`, in a tree where each page has 20 children, indexed for search as the reindex job would, with no job left queued and no revision. 30 users: the owner and 29 members. A second database holds 256 empty pages.
- **Idle:** each phase starts a fresh server on a copy of the database, and reads idle 45 seconds after start, once the jobs that run at start are done.

| Phase | Setup | Target |
| --- | --- | --- |
| Idle | No client ever connected | RSS under 30 MB |
| 30 active users | User `i` on page `i`, each with one page socket and one events socket, at the load of the benchmark scenario (2 updates and 1 awareness update per second for 60 seconds). Read 30 seconds after the load, clients still connected | RSS under 100 MB |
| After unload | All clients gone, `CADOCS_ROOM_IDLE_SECS` plus 10 seconds later | Recorded, no target |
| Login peaks | 3 peaks 20 seconds apart, each the 30 users logging in at once from 30 addresses. Read 10 seconds after the last, past the trim | RSS at most the measured idle plus 10 MB |
| Latency | 200 samples of each request, one at a time: a page's REST read, opening a page not loaded yet up to its SyncStep2, the root of the tree, the children of a page, a search matching every page, a search matching one title | p95 under 100 ms for each |
| 256 rooms | Second database, 256 clients on the 256 empty pages, no edits, held 5 seconds | RSS under 100 MB. Step 23 sized `CADOCS_MAX_ROOMS` with it, which ADR 0022 then removed |

The machine is faster than a Raspberry Pi 4, so a latency close to the target there is not a pass on a Pi.

## Measured results

**Verdict: go.** Every target holds with glibc, on the server of ADR 0015. The first run was a no-go: without the trim, RSS after unloading 100 documents stayed at 72 MB. The scenario did not change between the two runs.

Run on 2026-11-06 with `bench/rss.sh run`, on an Intel Core Ultra 5 236V, Linux 7.3.1, transparent huge pages set to `always`. RSS in MB (10^6 bytes). Each document count starts a fresh server, and the idle row shows the one before the 10 documents. The mimalloc column comes from the previous run, on a build removed after it, so `bench/rss.sh` no longer produces it.

| Measure | Target | glibc | glibc, `MALLOC_ARENA_MAX=2` | mimalloc |
| --- | --- | --- | --- | --- |
| Idle | 30 or less | 7.5 | 7.5 | 17.8 |
| 10 documents | under 50 | 16.3 | 15.8 | 28.0 |
| After unload of 10 | idle plus 10 | 11.5 | 11.5 | 28.4 (missed) |
| 100 documents | under 100 | 74.5 | 72.7 | 88.1 |
| After unload of 100 | idle plus 10 | 14.1 | 14.3 | 95.1 (missed) |

glibc with 100 documents, in MB: PSS 71.8, anonymous 67.2, peak 74.5. After unload: PSS 11.5, anonymous 6.9. Idle: PSS 4.8, anonymous 0.7, so most of the idle RSS is the file-backed binary.

The server keeps glibc with its default settings: `MALLOC_ARENA_MAX=2` changes nothing beyond noise. The release binary is 6.1 MB.

Costs, from `bench/rss.sh extras` on glibc, outside the scenario:

| Cost | Measured | Estimate in the budget |
| --- | --- | --- |
| Per open socket on the benchmark document | 62 KiB | tens of KB |
| Per loaded document, after the load | about 530 KiB, 10 to 15 times its encoded size of 37 to 56 KB | a few times its encoded size |
| Opening 100 documents with a 2,000-row log instead of a compacted one | 3.0 MB more in total | not estimated |

- A socket's cost is mostly the write buffer, which keeps the capacity of the SyncStep2 it sent. It grows with the document, as a 4 MiB document showed in step 3.
- A loaded document costs far more than estimated: each `yrs` block is a heap structure, and 2,000 edits make thousands of them. The cap of `CADOCS_MAX_ROOMS` (256) would therefore reach about 140 MB with this document, which is above the MVP's 100 MB. The MVP's scenario has 30 pages, so it holds, but a larger active set needs a lower cap.
- `load` holding the whole log is not a problem at this size, so `DocStore` stays as it is.

Not measured: documents much larger than 20 KB, peaks on `aarch64`, and variance beyond the three runs of this step. glibc agreed within 0.4 MB on every measure the trim does not touch, while mimalloc moved by up to 8 MB over two runs.

### MVP results

Step 23, on 2026-11-12, with `bench/rss.sh mvp`, one run, on the machine above (Linux 7.3.3), each server on one core. The seed took 190 seconds and its database 1,092 MB. RSS in MB (10^6 bytes).

| Phase | Target | RSS | Anonymous | Verdict |
| --- | --- | --- | --- | --- |
| Idle, 10,000 pages | under 30 | 11.0 | 2.0 | holds |
| 30 active users, 30 page and 30 events sockets | under 100 | 32.3 | 22.1 | holds |
| After unload of the 30 | none | 19.9 | 9.7 | idle plus 8.9 |
| After 3 login peaks of 30 | idle plus 10, so 20.9 | 50.7 | 41.7 | missed, then 12.2 after step 23a |
| 256 rooms on empty pages | under 100 | 19.8 | 10.3 | holds |
| After the latency phase, 200 rooms opened | none | 69.0 | 58.5 | the room budget, as designed |

| Request, 200 samples one at a time | p50 ms | p95 ms | max ms |
| --- | --- | --- | --- |
| `GET /pages/{id}` | 0.4 | 1.4 | 7.6 |
| Opening a page not loaded, up to SyncStep2 | 4.4 | 8.5 | 40.0 |
| Tree root (20 pages) | 0.2 | 0.3 | 1.1 |
| Children of a page (20 pages) | 0.2 | 0.7 | 1.4 |
| Search matching all 10,000 pages | 15.4 | 15.8 | 704.1 |
| Search matching one title | 1.3 | 1.8 | 25.5 |

A login of the peak takes 0.4 seconds at p50 and 0.8 at p95, since 30 hashes run one at a time. None waited past 10 seconds.

- **Login peaks miss the target by 30 MB.** Each hash runs on a blocking thread, and glibc gives such a thread its own arena. After the first hash the mmap threshold has grown past 19 MB, so the next hashes take their block from those arenas, and `malloc_trim` does not give it back: 41 MB of anonymous memory stay, 40 seconds later and after the threads ended. Measured outside the scenario, on 30 users and no page: `MALLOC_ARENA_MAX=1` returns to idle plus 0.9 MB after the trim, and `GLIBC_TUNABLES=glibc.malloc.mmap_threshold=4194304`, which keeps the threshold fixed, never holds the block at all (idle plus 1.0 MB, at most 1.5 MB during the peaks). ADR 0015's trim covers rooms, not hashes. Step 23a fixed the threshold at 4 MiB with `mallopt` (ADR 0023): the same phase then ended at 12.2 MB, idle plus 1.4, with a peak of 31.4 MB.
- **Latency holds on one core of this machine, with a margin of 6 at the slowest p95.** The pinned core matters: step 23a found the shell limited to the slower efficiency cores (4 to 7), where a search matching every page took 30 ms at p95 on both sides of its change. A Raspberry Pi 4 core is several times slower, and that was not measured, so the criterion is not checked yet.
- **`CADOCS_MAX_ROOMS` no longer binds, and step 23b removed it (ADR 0022).** 256 small rooms with their sockets take 9 MB over idle, about 36 KiB each, under the 64 KiB the room budget counts at least per room. So the budget alone caps rooms at 1,152, and `CADOCS_MAX_SOCKETS` (512) caps the live ones at 512, about 18 MB over idle for such rooms. Larger documents reach the budget first, at about 110 benchmark documents. Not measured: an idle room with no socket, waiting to unload, which should cost less than one with its socket.
- **A reindex of every page slows search.** A first run seeded the pages with their 20,000 reindex and revision jobs still queued, which each server then ran on its one core. Meanwhile a search matching every page took 3.6 to 4.5 seconds, against 16 ms once the reindexes were done. A change of the workspace language queues the same reindex of every page.

## Limits accepted by design

- One process on one machine: no horizontal scaling or active-active high availability (ADR 0006).
- The database is a file, so backups are `VACUUM INTO` or a filesystem snapshot, and it must live on a local disk. WAL does not work on network filesystems.
- A power cut can lose recent edits that no connected client still holds (see [Persistence and compaction](#persistence-and-compaction)).
- Files removed from a page stay on disk until the page is purged or the file is deleted explicitly ([DATA_MODEL.md](DATA_MODEL.md#attachments)).
- **yrs 0.28 is not memory safe on crafted updates.** A fuzz of mutated frames gave a `SIGSEGV` inside `apply_update`, in dev and release builds, with no panic first, so `catch_unwind` cannot stop it. Anyone who can write to a page can therefore crash the server, which loses unflushed edits. Read-only sockets never reach that code. Write access therefore means trust, and the server must not be open to untrusted writers (ADR 0014).
- The risks the security review of step 24 accepted are listed in [OPERATIONS.md](OPERATIONS.md#security).
- yrs does not always converge when every update arrives shuffled: about one order in 2,000 in a test with no cadocs code. Real clients send in order over one socket and resync through the handshake.
- Yjs history grows with edits. Compaction limits overhead, but deleted content keeps small tombstones.
