# Architecture

cadocs is a self-hosted collaborative wiki. It targets the main feature set of Docmost in a fraction of the memory: 10 to 30 MB idle and under 100 MB for a few dozen active users, on a small VPS or a Raspberry Pi. It ships as one binary and one data folder, with no mandatory external service.

Decisions and their rejected alternatives live in [`adr/`](adr/). The schema is in [DATA_MODEL.md](DATA_MODEL.md), the routes and the wire protocol in [API.md](API.md), the crates in [CARGO_WORKSPACE.md](CARGO_WORKSPACE.md).

## Overview

```
 browser (Svelte + TipTap + Yjs, static build)
    │ HTTPS: REST, assets            │ WebSocket: y-sync, one socket per open page
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
| Job worker | One task polling the `jobs` table: indexing, revisions, cleanup |
| File store | Local disk by default, S3 as an option (ADR 0005) |
| Content | Walks a yrs `XmlFragment` into an AST, renders Markdown, HTML or text (ADR 0008) |

## Flows

### HTTP

1. The request passes the connection limits and the session middleware (cookie, token hashed in the database).
2. The handler loads the membership rows it needs and asks `cadocs-core` for a decision.
3. Reads go through a small pool of read connections. Writes go through a single writer connection, as SQLite allows one writer at a time.

Page metadata (title, tree position, permissions) lives in SQL tables. Page content lives only in Yjs state. REST never edits content directly: an import, for example, produces a Yjs update applied through the room.

### WebSocket

1. `GET /ws/pages/{id}` carries the session cookie. The server checks `Origin`, resolves the session, and computes the role **before** the upgrade. A failure is a plain HTTP 401 or 403.
2. The registry returns the room, loading it if needed: latest snapshot, then every later row of `doc_updates`.
3. Both sides run the y-sync handshake (SyncStep1, SyncStep2), then exchange updates and awareness.
4. An update from a client with write access is applied to the `Doc`, broadcast to the other clients and queued for persistence. An update from a read-only client is dropped and answered with an auth message.
5. When the role changes or the page is deleted, the HTTP layer tells the registry, which closes the affected sockets.

Awareness (cursors, names, colours) is never persisted. When a socket closes, the server removes the client ids it announced and broadcasts the removal, so a crashed tab does not leave a ghost cursor.

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
- Durability window: an update lives only in memory until its flush. If the server dies while a client stays connected, that client resends what the server lacks when it reconnects, through the state vector exchange. If the client has also gone, up to one flush interval of edits is lost. This is the accepted trade-off, and the interval is configurable.
- A graceful shutdown flushes every room before exiting.

### Room lifecycle

```
          first client                    last client leaves
absent ───────────────► loading ─► live ─────────────────────► idle
   ▲                                ▲                            │
   │                                └── client returns ──────────┤
   └──────────── flush, compact, drop ◄── idle timeout ──────────┘
```

Load and unload are serialized per document. A client that arrives during an unload waits for it to finish and then loads the persisted state. It must never receive a second copy of a document that is still being written.

### Jobs

The `jobs` table is the queue: `INSERT` to enqueue, one worker task to claim with a conditional `UPDATE`, retries with backoff. An in-memory `Notify` wakes the worker immediately, and a slow poll covers jobs left over from a restart. Jobs are idempotent, because a crash may run one twice.

Job types: reindex a page for search, create a revision, purge expired sessions and old revisions, delete orphaned files.

### Search

A room flush marks the page dirty, and a debounced job extracts the text from the Yjs state and updates the FTS5 index (ADR 0007). Results are filtered by the spaces the user can read, inside the SQL query.

## RAM budget

These figures are design estimates. The prototype's measurements are in [Measured results](#measured-results), and they take precedence.

| Component | Estimate | How the budget is held |
| --- | --- | --- |
| Code and static data actually touched | 4 to 8 MB | File-backed and reclaimable. The embedded front end costs nothing until served, and the kernel can evict it |
| tokio runtime, 2 workers | 1 to 2 MB | `worker_threads = 2`, small `max_blocking_threads`, explicit thread stack size |
| SQLite | 2 to 4 MB | Few connections, `cache_size` limited to 512 KiB each. The OS page cache does the real caching and is not counted against the process |
| Per WebSocket | tens of KB | Bounded send queue. A slow client is disconnected, never buffered without limit |
| Per loaded document | a few times its encoded size | Unload after inactivity, compact on unload |
| Peaks: password hashing | 19 MB per hash at OWASP settings | Semaphore allowing one or two concurrent hashes |
| Peaks: uploads and exports | bounded | Stream to disk, never buffer a whole file. One export job at a time |

Other strategies:

- **Allocator.** Freed memory does not always return to the OS. The benchmark compares glibc, `MALLOC_ARENA_MAX=2` and mimalloc, including after unload.
- **Dependencies.** Each crate must justify its cost ([CARGO_WORKSPACE.md](CARGO_WORKSPACE.md)).
- **No in-memory caches of database rows.** SQLite and the OS page cache are already fast enough at this scale.
- **Backpressure everywhere.** Every queue is bounded, so load shows up as latency or a refused connection instead of memory growth.

## What is measured, and how

`bench/rss.sh` reports four numbers from `/proc/<pid>/smaps_rollup` and `/proc/<pid>/status`:

- **RSS**, the figure usually quoted, which includes reclaimable file-backed pages;
- **PSS**, the same with shared pages divided among the processes sharing them;
- **Anonymous**, the heap and stacks, which is what really competes for RAM on a small machine;
- **Peak** (`VmHWM`), the highest RSS reached.

## Measured results

Not measured yet. The prototype benchmark will fill in this section.

## Limits accepted by design

- One process on one machine: no horizontal scaling or active-active high availability (ADR 0006).
- The database is a file, so backups are `VACUUM INTO` or a filesystem snapshot, and it must live on a local disk. WAL does not work on network filesystems.
- Yjs history grows with edits. Compaction limits overhead, but deleted content keeps small tombstones.
