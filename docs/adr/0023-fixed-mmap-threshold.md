# ADR 0023: A fixed mmap threshold gives back password hashes

Status: accepted

## Context

Step 23 missed the login peak criterion: after 3 peaks of 30 logins, RSS stayed at 50.7 MB against a target of idle plus 10 MB ([MVP results](../ARCHITECTURE.md#mvp-results)). An Argon2id hash at OWASP settings takes one block of about 19 MB, and runs on a blocking thread, which glibc gives its own arena. The first hash maps its block with `mmap`, and freeing it raises glibc's dynamic mmap threshold past 19 MB. The next hashes then take their block from the heap of their thread's arena, and `malloc_trim`, which ADR 0015 called after each hash, did not give it back: 41 MB of anonymous memory stayed.

Step 23 measured two fixes outside the scenario, on 30 logins: `MALLOC_ARENA_MAX=1` came back to idle plus 0.9 MB after the trim, and a threshold fixed at 4 MiB with `GLIBC_TUNABLES` never held the block (idle plus 1.0 MB, at most 1.5 MB above idle during the peaks).

## Decision

At start, before anything else, the binary calls `mallopt(M_MMAP_THRESHOLD, 4 MiB)`. A threshold set by hand turns off glibc's dynamic one, so every block of 4 MiB or more is mapped on its own and unmapped as soon as it is freed, whichever thread or arena it came from. The hash no longer triggers a trim: there is nothing left for one to find. The trim after a room unloads stays (ADR 0015).

4 MiB is above what a room usually allocates in one block, and far below a hash. The `mallopt` call lives with `malloc_trim` in `cadocs_server::memory`.

The `hash_memory` test checks it in a process of its own: 5 hashes after a first one leave at most 5 MB of anonymous memory, with no trim. Without the threshold, 19 MB stayed.

## Measured

Step 23a, on 2026-11-13, on the machine of step 23, against the commit before it, the same day. RSS in MB (10^6 bytes).

| Measure | Target | Before | After |
| --- | --- | --- | --- |
| MVP, idle before the login peaks | | 10.9 | 10.8 |
| MVP, after 3 login peaks of 30 | idle plus 10 | 50.6 | 12.2 |
| MVP, peak during the logins | | 51.1 | 31.4 |
| MVP, 30 users after unload | | 19.4 | 18.8 |
| MVP, after the latency phase | | 70.4 | 63.6 |
| MVP, 256 rooms | under 100 | 19.2 | 19.3 |
| Stage 0, 100 documents | under 100 | 79.0 | 79.0 |
| Stage 0, after unload of 100 | idle plus 10 | 22.5 | 20.3 |

Latency, both servers alternated twice on one database and pinned to the same core, as the mean of the two p95: each request moved by 0.5 ms or less, except a search matching every page (30.5 ms against 29.5) and a login during a peak (857 ms against 796). A hash now faults in its 19 MB of fresh pages each time, about 8 % of its time.

## Consequences

- The login peak criterion holds, and the hash no longer depends on a trim.
- Any block of 4 MiB or more costs a `mmap` and a `munmap`, with its pages faulted in again each time. Only hashes and the rare large document reach it.
- The rest of a login's small allocations goes back at the next room unload, as ADR 0015 already accepts.
- A `musl` build still needs its own answer.

## Rejected alternatives

- **A single arena (`mallopt(M_ARENA_MAX, 1)`).** It returned the memory too, but only through the trim, and every thread would then share one lock for each allocation.
- **A hash block mapped by cadocs itself**, passed to Argon2 as its memory. It frees the same block with more code of our own, and leaves other large blocks to the dynamic threshold.
- **`GLIBC_TUNABLES` in the environment.** Measured the same, but a deployment can drop it. The call in the binary always applies.
