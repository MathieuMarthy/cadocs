# Benchmark

What a change to `cadocs-bench` or a RAM measure must know. Scenarios and results are in [ARCHITECTURE.md](../ARCHITECTURE.md#what-is-measured-and-how).

## Rules

- **The scenarios are fixed.** Do not change them to make a number pass: a change needs an ADR, and old results are measured again.
- **Compare side by side.** Build the commit before in a worktree with its own `CARGO_TARGET_DIR`: a shared `target/` keeps its workspace crates, newer than the sources, and the next release build links old code. Run both binaries from fresh copies in the same folder, or the page cache skews them (once 7.7 against 10.6 MB). For a step's idle cost: three runs each, 45 seconds after start, on an empty database.
- **Noise.** One `bench/rss.sh run` moves by 1 to 2 MB. Three runs a side settle gaps under 1 MB.
- **CPU.** `start_server` pins each server to the first CPU the shell may use, which can change during a run (once from 0-7 to 4-7, the slower efficiency cores here). Latency only compares between runs on the same kind of core: a search matching every page took 16 ms at p95 on one, 30 ms on the other. The load tool is not pinned.
- **Probes.** glibc's in-use bytes need a preload probe, since `ptrace_scope` blocks `gdb -p`.

## `cadocs-bench`

- Outside `default-members`: `--workspace` covers it, and building it with the server would change the server's features.
- `seed` needs an empty database, makes the accounts behind `session_token(i)` (all `BENCH_PASSWORD`), and drains the job queue at the end: `append_updates` queues a reindex and a revision per page, which used to run during measures (likely part of the 13 to 15 MB idle of steps 20c and 20d). Seeded pages have no revision, and are indexed only with `--index`.
- The generator panics on some seeds (330 is the first): backspaces can empty a paragraph that later gets a word. Seeds 1 to 100 are safe and keep their exact bytes.
- A search that ends with its last word is a prefix search (`Analyzer::query`), so `latency` sends `q=<word>%20` and checks the hit count.
- Index size (step 60): `dbstat` on the tables `page_fts%` of a database seeded with `--docs 10000 --index true` gives 315,392,000 bytes before the prefix words and 320,851,968 after. The bench vocabulary repeats 37 words, so real text grows more (+21% on the English ADRs, [store notes](store.md#search)).
- `latency` and `logins` speak HTTP by hand (`http.rs`).

## Costs per step

Idle RSS against the commit before, three runs each unless stated. Binary sizes include `web/dist` from step 28.

| Step | Idle RSS | Binary | Where it goes |
| --- | --- | --- | --- |
| 10, jobs | +0.1 MB | +89 KB | The worker |
| 17, attachments | +0.6 MB | +205 KB | Mostly the binary; anonymous +0.2 MB |
| 18, search | +0.4 MB | +499 KB | `cadocs-content` and 18 stemmers enter the binary |
| 19, revisions | +0.16 MB | +1.0 MB | `zstd` bundles its C library |
| 20, export and import | +0.6 MB | +732 KB | `async_zip`, `tokio-util`; an export holds a 64 KiB pipe |
| 20b, S3 feature | +1.4 MB unused, +2.7 MB in use | 8.64 → 11.33 MB | rustls and the system roots; a default build has none of it |
| 20c, 20d | no lasting change | | Compression saves disk (snapshots 3.3 times smaller, `cadocs.db` 30 %), the budget bounds RAM |
| 28, mail relay | +0.5 MB mail off, +1.3 MB with a relay | 10.78 → 12.48 MB | `lettre` and rustls; the start check reads the system roots |
| 29, outbox | none measurable | +65 KB | One task and an empty channel |

## Measures

- **Room cost.** A loaded benchmark document costs about 530 KiB, 10 to 15 times its encoded size (stage 0), and about 640 KiB of anonymous memory with its two sockets (steps 20c and 20d). `Room::estimate` gives about 670 KiB, on the safe side. The default `CADOCS_ROOM_MEMORY_MB` of 72 (the 100 MB target less about 15 MB idle and 12 MB for 200 sockets) caps such rooms at about 110, about 85 MB of RSS. The first default, 64, failed the 100-document row.
- **Login peaks.** The 41 MB left after step 23 were hash blocks in the arenas of blocking threads. ADR 0023 fixed the mmap threshold, with its numbers.
- **After unload.** The stage 0 rows after unload read 20 to 22 MB since the MVP, against 14 in stage 0, from steps before 23a.
- **V1 exit** (step 55, on 2026-11-25, `bench/rss.sh mvp`, RSS in KiB): idle 12892, 30 users 33732, unloaded 20000, after three login peaks 14916 (idle plus 2.1 MB), 256 rooms 22012, after latency 68604. `bench/rss.sh run`: idle 12620, 100 documents 81148. Every MVP target holds. Idle grew 1.6 MB over the mail stage exit, with the V1 code in the binary (12.48 to 20.30 MB with `web/dist`); anonymous memory stayed at 2.7 MB. Latency p95: page 2.2 ms, search over every page 14.9 ms, login 0.56 s. The release binary is 20,295,400 bytes, with no S3 code.
- **Mail stage exit** (`bench/rss.sh mvp`, mail off, RSS in KiB): idle 11276, 30 users 32700, unloaded 19188, after three login peaks 13260, 256 rooms 20232. `bench/rss.sh run`: idle 11516, 100 documents 79756. Every MVP target holds.

## Not measured

- A Raspberry Pi 4 or 1 vCPU VPS, which the MVP latency criterion needs.
- A burst of revision views on a large page (seeded pages have no revision), and the pause of a restore on one.
- The recheck cost of access changes at `CADOCS_MAX_SOCKETS`, and the scan of `pages` when a user is deleted.
- The room factor on documents of other sizes, and a document over 4 MiB with the fixed mmap threshold.
- An idle room with no socket, waiting to unload.
- Memory while mail waits in the outbox (at most 64 messages under 1 KB), a relay answering over TLS, a burst of messages.
- Variance over several runs.
