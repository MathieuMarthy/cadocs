# ADR 0014: Write access means trust

Status: accepted

## Context

A fuzz of mutated frames in step 2 gave a `SIGSEGV` inside `yrs` 0.28 `apply_update`, in dev and release builds, with no panic first. `catch_unwind` cannot stop it, so a crafted frame from a client with write access can stop the whole server. Unflushed edits that no connected client holds are then lost. The database stays consistent. Read-only sockets never reach that code. Whether the bug allows more than a crash has not been studied.

The trigger is not known: shrinking stopped at 1,313 frames, so frames cannot be validated against it yet.

## Decision

Accept the risk. Write access to a page means the user is trusted not to attack the server. cadocs targets small teams whose editors are known, and write access already lets them destroy a page's content.

## Consequences

- An instance must not give write access to untrusted people. Public shares stay read-only (ADR 0009).
- The self-hosting documentation must say so when it is written.
- `panic = "abort"` stays off, because `yrs` also panics on crafted frames.
- The decision is reopened if cadocs ever offers write access to anonymous or untrusted users.

## Rejected alternatives

- **Validating frames before `yrs`.** The trigger is unknown, so a validator would give false confidence.
- **Applying updates in a separate process.** A crash would stay contained, but it costs RAM and goes against ADR 0006.
- **Replacing `yrs`.** It is the core of ADR 0003, and no other Rust CRDT speaks the Yjs protocol.
