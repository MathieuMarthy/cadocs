# ADR 0003: Real-time collaboration with yrs and y-sync

Status: accepted

## Context

Editing must be simultaneous, with shared cursors, and must work with TipTap in the browser. TipTap's collaboration extension is built on Yjs. The server therefore needs to speak the Yjs wire protocol and persist Yjs state.

## Decision

- Use **yrs** on the server, and implement the WebSocket layer directly on `yrs::sync` (protocol messages and awareness).
- Stay wire-compatible with `y-websocket`, so the standard client works: message types sync, awareness, auth and query-awareness, one socket per open page.
- One room per loaded document, held by a `DocRegistry`. Rooms are unloaded after inactivity.
- Persist as an append-only log of updates, flushed in batches of about 500 ms, then compacted into snapshots past a threshold and on unload.
- Awareness is memory-only.
- Auth happens before the WebSocket upgrade. Writes are also filtered per message: read-only clients get sync and awareness, and their updates are dropped.

## Consequences

- No Node process, and documents are merged in-process.
- Yjs documents can be read on the server, which makes text extraction and export possible (ADR 0008).
- We own a few hundred lines of protocol code, and wire compatibility must be tested against the real JavaScript client.
- Protocol changes in yrs or y-protocols must be tracked by hand.
- Up to one flush interval of edits may be lost if server and client both disappear. See [ARCHITECTURE.md](../ARCHITECTURE.md).

## Rejected alternatives

- **Hocuspocus in a sidecar.** Proven, but brings back the Node.js runtime this project exists to avoid.
- **`yrs-axum` and `y-sync`.** `y-sync`'s protocol now lives in `yrs::sync`. A wrapper would also get in the way of auth, read-only filtering, persistence hooks and unloading, which are the parts that matter here.
- **Automerge.** Good CRDT, but no ProseMirror integration as mature as y-prosemirror.
- **Operational transform (prosemirror-collab).** Light on the server, but requires a central authority per step and handles offline editing poorly.
- **Keeping documents in memory forever.** Simplest, but memory would grow with the number of pages ever opened.
