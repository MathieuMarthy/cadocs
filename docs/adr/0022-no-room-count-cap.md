# ADR 0022: The room budget alone bounds loaded rooms

Status: accepted

## Context

Step 2 capped loaded rooms with `CADOCS_MAX_ROOMS` (256), before any measure of what a room costs. Step 20d added the room memory budget, `CADOCS_ROOM_MEMORY_MB`, which counts each room from its encoded size and at least 64 KiB, and evicts or refuses the same way. Two bounds then did one job.

Step 23 measured 256 rooms of empty pages, each with its socket, at 9 MB over idle, about 36 KiB each ([MVP results](../ARCHITECTURE.md#mvp-results)). The budget counts such a room at 64 KiB, so its default of 72 MiB caps them at 1,152, about 40 MB over idle. `CADOCS_MAX_SOCKETS` (512) caps the live ones at 512, about 18 MB. Larger documents reach the budget long before 256 rooms: about 110 of the benchmark's. So the count cap only refused pages the memory could hold.

## Decision

`CADOCS_MAX_ROOMS` is removed. The budget alone decides when idle rooms unload and when a connection gets `503`, with the same order as before: the oldest idle room first, skipping a failing room and one nobody has joined yet.

The count cap also bounded rooms evicted but still unloading, which the budget did not count. A room on its way out now counts on the budget, by its estimate when its unload began, until it is gone. Otherwise each connection could evict a room and the next take its place before it left memory.

A server that still gets `CADOCS_MAX_ROOMS` starts, logs at `warn` that it is ignored, and does not parse it, so an old deployment keeps working.

## Consequences

- One setting less to size. An operator who wants fewer rooms lowers `CADOCS_ROOM_MEMORY_MB`.
- Many idle rooms of small documents may stay loaded where 256 was the limit before, up to the budget. An idle room with no socket was not measured, but should cost less than the 36 KiB of one with its socket, so the budget still overcounts it.
- The benchmark keeps its phase of 256 rooms on empty pages, so its results compare with step 23's.
