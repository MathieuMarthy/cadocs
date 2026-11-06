# ADR 0015: Return freed memory after an unload

Status: accepted

## Context

The first run of the benchmark gave a no-go. Three targets held, but after unloading 100 documents RSS stayed at 72 MB with glibc, against a target of about 17 MB, and no allocator came close:

| Allocator | Idle | 100 documents | After unload |
| --- | --- | --- | --- |
| glibc | 7.4 MB | 74.1 MB | 72.4 MB |
| glibc, `MALLOC_ARENA_MAX=2` | 7.5 MB | 72.7 MB | 72.5 MB |
| mimalloc | 15.7 MB | 95.7 MB | 103.5 MB |

This was not a leak. After the unload, glibc reported 2.2 MB in use and 67.7 MB free inside its arenas. It only returns the top of a heap by itself, and a few live blocks above the free ones keep everything below them. Calling `malloc_trim(0)` by hand, which also releases whole free pages inside a heap, brought RSS from 76.9 MB to 12.9 MB. Four rounds of load and unload on one server left in-use bytes flat (2.33 MB to 2.36 MB) and RSS after unload at about 14 MB, so nothing accumulates.

The rule in [ARCHITECTURE.md](../ARCHITECTURE.md#benchmark-scenario) asks for a new ADR before any MVP work. The rooms, the store and the protocol are not at fault, so the rework is limited to this.

## Decision

Keep glibc with its default arenas, and call `malloc_trim(0)` 5 seconds after a room unloads. `DocRegistry::room_unloaded` announces the unload, and the binary owns the call, because the allocator is its concern. The delay lets rooms that went idle together share one trim, and the call runs on a blocking thread because it walks each heap with that arena locked.

mimalloc is dropped. It used about twice the idle RSS, was worse at every measure, and RSS rose after unload.

The benchmark scenario did not change. The whole run was repeated on the new server, for every allocator. See [Measured results](../ARCHITECTURE.md#measured-results).

## Consequences

- cadocs depends on glibc, which [OPERATIONS.md](../OPERATIONS.md) already required. A `musl` target needs its own answer to this problem.
- While rooms unload, a trim briefly holds each arena's lock. A busy server pays this at most once per 5 seconds.
- Memory freed by anything other than an unload is only returned at the next unload. MVP peaks such as password hashing and exports must check their own return to the idle target: the exit criterion on login peaks measures this.
- Password hashing needed it. After the first hash, glibc raises its mmap threshold, so later 19 MB blocks stay in a heap: RSS went from 8 MB to 28 MB after 8 logins and stayed there. `Passwords::hash_finished` now triggers the same delayed trim, which brings RSS back to 8.6 MB.

## Rejected alternatives

- **`MALLOC_ARENA_MAX=2`.** No measurable effect here, with two worker threads.
- **mimalloc.** See above.
- **A periodic trim.** It wakes an idle server for nothing, and an unload is the event that frees memory.
- **jemalloc.** Not measured: a trim solves this with no new dependency and no larger binary.
