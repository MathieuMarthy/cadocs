# ADR 0002: SQLite by default, PostgreSQL behind traits in V2

Status: accepted

## Context

"One binary and one data folder" rules out a mandatory database server. Some operators will still want PostgreSQL for existing backups and tooling. Supporting two engines costs more than dialect differences: search (FTS5 or tsvector), the job queue and migrations all exist twice.

## Decision

- SQLite in WAL mode, with `synchronous=NORMAL`, foreign keys on and `cache_size` limited per connection. Vacuum, WAL size and planner statistics are kept up by ADR 0019.
- One writer connection and a few reader connections.
- The schema uses a portable SQL subset ([DATA_MODEL.md](../DATA_MODEL.md)), and data access sits behind traits.
- Only the SQLite implementation exists until V2. PostgreSQL is then added with its own migrations and search implementation, and runs against the same test suite.
- Access goes through sqlx, as in uptime-monitoring. Queries are plain SQL with no ORM.

## Consequences

- Backups are one file: `VACUUM INTO`, or a snapshot.
- Write throughput is limited to one writer. Batched flushes (ADR 0003) keep this far from being a bottleneck at the target scale.
- The data folder must be on a local filesystem.
- Without a second implementation in CI, portability can drift. The schema document and a review rule mitigate this, and V2 pays what remains.

## Rejected alternatives

- **Both engines from the MVP.** Avoids drift, but every feature costs noticeably more to ship.
- **SQLite only.** Simplest, but closes an option some operators need.
- **rusqlite.** Slightly lighter than sqlx, but SQLite-only and synchronous, so V2 would rewrite the store layer. To revisit if measurements show sqlx is expensive.
- **sled, redb or another embedded KV store.** No SQL, no FTS, and relational data would be reinvented by hand.
- **Diesel or SeaORM.** Heavier compile-time or runtime machinery, with no benefit for a small hand-written query set.
