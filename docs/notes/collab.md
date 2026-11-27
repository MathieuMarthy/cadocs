# Collab

What a change to rooms, sync or Yjs must know.

## Rooms

- `DocRegistry::connect(doc, mode, user, session)` returns a `Connection`, and the server only moves bytes: `receive(frame)` (an `Err(Close(code))` means close with it) and `next().await` (cancel safe, `Frame` or `Close`). Hide `yrs` behind such methods: `cadocs-server` has it as a dev dependency only.
- Server writes go through `enter`, under the registry lock, and `Room::enqueue`, so they are stored and broadcast like an edit: `set_title` and `rewrite(doc, user, bytes, action)`, which runs `action` on the room's `TransactionMut` as one edit by `user`. A panic in `action` answers `CorruptDocument`, and what it wrote still goes out. The server never names a yrs type: `cadocs-jobs` passes `cadocs_content::yjs::replace_page`.
- `PATCH` checks the role again once the room is live, since a room write cannot sit in a database transaction; `append_updates` ignores `deleted_at`. `set_title` refuses a title root holding more than text, which only a crafted client writes: `PATCH` then answers `500` for that page, and nothing resets it.
- A flush that gets `PageNotFound` calls `Room::abandon`, closing the sockets with `4404`, as a last resort after the recheck.
- The room keeps one author per queue, the last: a batch of two users credits one.

## Reader

- `DocRegistry::read(doc, action)` hands `action` a private `yrs::Doc` under a turn of `doc_reads` (`CADOCS_DOC_READS`). `action` is synchronous on an async thread: take what is needed and return, heavy work after. `bounded(action)` runs work as heavy as a read under the same turn.
- A document with no page reads as empty: the caller checks the page first. What waits for a dependency is left out, and a stored row that cannot be applied is skipped, as in a room load.
- A restore reads the AST and encodes the full state inside `rewrite`, under the room and registry locks: about twice the pause of a plain rewrite, with the old state held beside the new content briefly.

## Room memory budget

- `CollabConfig::room_memory` caps the sum of `Room::estimate`: `ROOM_BYTES_PER_ENCODED_BYTE` (12) times the encoded size, at least `ROOM_MIN_BYTES`. It is the only bound on the number of rooms (ADR 0022). The factor comes from 37 to 56 KB documents.
- The estimate is set from the stored bytes at load (an uncompacted log counts high), reset at each compaction, grown by each queued update, and measured again from the encoded document past twice `RoomState::measured`.
- A room on its way out still counts, by its estimate when its unload began. Over the budget, `attempt` waits for a room already leaving before it evicts another, so a burst evicts one at a time. `RoomState::joined` keeps a room from eviction until whoever loaded it entered it, or two connections at the cap evict each other's room.
- A loading room counts for nothing, and a load always waits for the one in flight (`Attempt::WaitTurn`): a slow load delays opening any other page.
- A room task that panics (step 59) is dropped by tokio without unloading: `Drop for RoomTask` sees `finished` unset and calls `Inner::forget`, which frees the slot (`Live` of that room, or `Unloading`) and so its estimate, closes its sockets with `1001`, clears its `failing` count, and answers a pending unload with `CorruptDocument`, so waiters try again instead of getting `ShuttingDown`. Unflushed edits are lost on the server; connected clients resend them on reconnect. A runtime dropped with rooms still loaded (a test ending without `shutdown`) goes through `forget` too, which only logs a warning. Tests inject the panic with `MemStore::panic_compact`. Since `Drop` locks the rooms, a `RoomTask` is never spawned or dropped with that lock held: a closing runtime drops a spawned task inline. The runtime-shutdown race this guards against has no test.

## yrs

- yrs 0.28: `yrs::sync` has `Awareness`, `Message`, `SyncMessage`, `MessageReader`, `DefaultProtocol` and the `Protocol` trait. Filter around `Protocol::handle_message` to reject `SyncStep2` and `Update` in read-only mode.
- yrs can panic and segfault on crafted updates, in release too; `catch_unwind` only covers panics. Write access means trust (ADR 0014). Keep unwinding on. `mutated_frames_do_not_break_the_room` pins client ids so it does not crash at random.
- A dependency that never arrives blocks compaction until the log reaches 4 times `compact_updates`; a stored row that cannot be applied (`skipped_row`) blocks it for good.
- Awareness ownership is per user, since a reconnecting client reuses its id, so two tabs of one user can overwrite each other. Only `cursor` is validated, not `user`.
- y-websocket appends `/<room>` to the URL and stops for good on close codes 4400 to 4499: `web/src/editor/collab.ts` passes `shouldReconnect`, and any other client must too.
- Stale clients (step 56): a page socket must carry `?schema=` with `cadocs_content::schema::editor_fingerprint()`, FNV-1a of `tests/tiptap-schema.json`, or `ws::upgrade` opens it only to close it with `4426`, before the session's role or room are touched. Any client outside the browser (tests, `cadocs-bench`, `web/e2e/server.ts` `webSocketClass`) adds it. A new block changes the fingerprint by itself, through `npm run schema`. Left open: a tab of a release from before step 56 does not know `4426`, so it reconnects with y-websocket's backoff (up to 2.5 s, one upgrade each) and shows `offline` instead of the reload notice, until it is reloaded or closed. The hash covers `editor.schema`, the live editor's, while the build check covers the registry's: an extension added in `collab.ts` alone that brings a node, mark or global attribute would refuse every tab, and only the end-to-end tests would show it.
- Layout: root types `title` (`Y.Text`) and `default` (`XmlFragment`). See [DATA_MODEL.md](../DATA_MODEL.md).

## Socket RAM

- tungstenite 0.29 formats each frame whole into `out_buffer` and keeps its capacity, so a page socket holds about one SyncStep2 of its document for life: 62 KiB on the 20 KB benchmark document. axum exposes neither the buffer nor fragmented sends.
- Not scheduled: the way out is to send less. The browser would fetch the state over HTTP before joining, so its SyncStep1 carries a state vector and the SyncStep2 only the difference. That needs a route and front end work.
