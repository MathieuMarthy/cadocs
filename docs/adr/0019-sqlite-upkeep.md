# ADR 0019: SQLite upkeep: incremental vacuum, WAL limit, planner statistics, no mmap

Status: accepted

## Context

ADR 0002 sets WAL mode, `synchronous=NORMAL` and a small `cache_size`, but says nothing about how the file behaves over months. cadocs deletes a lot: compaction removes `doc_updates` rows, the trash purge removes pages, revision thinning removes revisions, and the index replaces each page's rows. Without upkeep:

- SQLite never gives deleted pages back to the disk. The file only grows, and the free pages stay inside it.
- The WAL grows to fit the largest transaction since startup, such as an import or a reindex of every page, and keeps that size even when empty.
- The query planner uses the statistics of `sqlite_stat1`, which nothing refreshes. A plan chosen on 10 pages can be wrong on 10,000.

`auto_vacuum` is written in the database header, and only takes effect on a file with no table yet. Changing it later costs a full `VACUUM`, which rewrites the file and needs twice its size on disk. This had to be settled before the first release.

## Decision

- **`auto_vacuum = INCREMENTAL`** on every new database. A database created before this keeps `NONE`, so `SqliteStore::open` converts it once at startup with a `VACUUM`, while `serve` holds the data folder alone.
- **`journal_size_limit` = 64 MiB**: after a checkpoint, SQLite cuts the WAL back to that size.
- **A `maintain_database` job**, recurring with the other cleanups (at start, then every hour), after the purges:
  - `PRAGMA incremental_vacuum(1024)` in a loop until no free page is left, so each step holds the writer for 4 MiB at most and a flush can pass between two steps;
  - `PRAGMA analysis_limit = 400; PRAGMA optimize = 0x10002`. The `0x10000` bit checks every table, not only those queried on the writer connection, since reads run on the reader pool. `optimize` only analyses a table whose size changed a lot, so a run that finds nothing to do is cheap.
- **No `mmap_size`.** Mapped pages count in the process RSS, which the RAM targets measure. Without it, the OS page cache gives the same reads, and they are not counted against the process.
- **Checkpoints stay automatic** (every 1,000 WAL pages). No `wal_checkpoint(TRUNCATE)`: `VACUUM INTO` reads a consistent snapshot that includes the WAL, so a backup does not need one. A forced truncate would also fight a WAL-shipping tool such as Litestream, which must control checkpoints.

## Consequences

- The file shrinks within an hour of a large deletion, and the WAL stays at most 64 MiB on disk after a checkpoint.
- `incremental_vacuum` moves pages to the end of the file before cutting it, so the file grows a little more fragmented than with a full `VACUUM`. That costs little on an SSD, and a `VACUUM` by hand remains possible on a stopped server.
- A database opened by `open_existing` (`backup`, `reset-password`) is never converted. Maintenance on it frees nothing and stops at once.
- The first start on a database from before this ADR runs one `VACUUM`. Its length grows with the file, and it needs free disk space equal to the file's size.
- The PostgreSQL store of V2 needs none of this: autovacuum and `ANALYZE` are the server's work.

## Rejected alternatives

- **`auto_vacuum = FULL`.** Frees pages at every commit, which moves pages during the room flushes and slows each write.
- **A periodic full `VACUUM`.** Rewrites the whole file, blocks writes for its duration and needs twice the space.
- **`mmap_size`.** Slightly faster reads, but RSS would grow with the database and hide what the benchmark measures.
- **Forced `wal_checkpoint(TRUNCATE)` when idle.** `journal_size_limit` already bounds the file, and a truncate would conflict with Litestream.
