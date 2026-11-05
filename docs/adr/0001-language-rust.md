# ADR 0001: Rust, axum and tokio in one binary

Status: accepted

## Context

The priority is server memory. Docmost's 450 to 600 MB comes mostly from the Node.js runtime, its dependency tree, and the Redis and PostgreSQL services around it. A garbage-collected runtime also holds on to headroom it does not currently need.

## Decision

Write the server in Rust with axum on tokio, built as a single binary. The tokio runtime is configured for small machines: two worker threads and a small blocking pool.

The author's uptime-monitoring project already validates this stack on a similar goal, and cadocs reuses its conventions (SQLite setup, embedded assets, release profile).

## Consequences

- Memory use is predictable, with no GC and no JIT.
- yrs, the Rust port of Yjs, runs in-process (ADR 0003).
- Compile times are long, and the contributor pool is smaller than for TypeScript.
- Server-side document conversion has to be written in Rust, because the TipTap libraries are not available (ADR 0008).

## Rejected alternatives

- **Node.js or Bun with Hocuspocus.** Mature collaboration stack, but the runtime alone exceeds the idle budget.
- **Go.** Small and simple, but it has no maintained Yjs implementation, and the GC trades memory for latency.
- **Zig or C.** Lean, but too little ecosystem and too much unsafe code for an internet-facing service.
