# ADR 0006: One process, in-memory channels, jobs in SQLite

Status: accepted

## Context

Docmost uses Redis and BullMQ for queues and for sharing state between processes. cadocs targets one small machine, and each extra service costs memory and operations.

## Decision

- cadocs runs as **one process**. Rooms, broadcasts and wake-ups use in-memory tokio channels.
- Background work uses a `jobs` table with one worker task, woken by a `Notify` and backed by a slow poll.
- Jobs are idempotent, retried with backoff, and survive restarts because they live in the database.

## Consequences

- No Redis, no broker and no worker process.
- **Horizontal scaling is impossible.** A document must have exactly one live room, and that is guaranteed by being in one process. Two instances on one database would silently diverge. The server takes an exclusive lock on the data folder at startup to prevent this.
- High availability comes down to a fast restart and backups. Clients reconnect on their own.
- Vertical scaling covers the target: a few dozen active users is far from what one process can handle.

## Rejected alternatives

- **Redis.** Tens of MB for a job the process can do by itself.
- **Multi-instance with a pub/sub layer.** Only useful beyond the target audience. It can be reconsidered for PostgreSQL deployments, and it would then need a routing layer per document.
- **In-memory job queue.** Jobs would be lost on restart.
