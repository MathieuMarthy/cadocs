# ADR 0011: One events WebSocket for changes outside the open page

Status: accepted

## Context

The y-sync socket only covers the open page. When someone creates, moves, renames or trashes a page, other users' sidebars would stay stale until a reload. In-app notifications (V1) have the same need.

## Decision

- `GET /ws/events?workspace={id}`: one WebSocket per tab, authenticated before the upgrade like the page socket.
- Server to client only, with JSON text frames. Anything a client sends other than a pong closes the socket.
- Events are **hints, not data**: they carry ids and a kind, and the client refetches through REST. Permissions therefore stay in one place.
- Each socket keeps the set of spaces its user can read, and only receives events for those. The set is recomputed when permissions change.
- A workspace has one in-memory bounded broadcast channel, created with its first socket and dropped with its last. Nothing is persisted.
- A client that lags receives one `resync` event and reloads what it shows. It is never buffered without limit.

The wire format is in [API.md](../API.md).

## Consequences

- One more socket per tab, at tens of KB each. The MVP RAM exit criterion counts it.
- A client offline during an event misses it, so it refetches the tree on every reconnect.
- V1 notifications reuse the same socket.

## Rejected alternatives

- **Server-sent events.** Slightly simpler, but browsers limit HTTP/1.1 connections per origin, which several tabs reach quickly when no HTTP/2 proxy is in front.
- **Polling the tree.** No socket, but constant load on small machines and visible delay.
- **A shared Yjs document for the tree.** Elegant for ordering, but it makes SQL no longer the source of truth for the tree, and permissions cannot filter part of a Yjs document.
- **Multiplexing page sync and events on one socket.** Fewer sockets, but incompatible with the standard `y-websocket` client (ADR 0003).
