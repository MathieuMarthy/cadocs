# cadocs

Self-hosted collaborative wiki, a lightweight Docmost alternative. Top priority: minimal server RAM (10 to 30 MB idle, under 100 MB for a few dozen active users). One binary plus one data folder, no mandatory external service.

Design lives in `docs/`: read only what the current task needs. `ARCHITECTURE.md` for the flows, `API.md` for the wire protocol, `DATA_MODEL.md` for the schema, `PERMISSIONS.md` for the access matrix, `OPERATIONS.md` for configuration and deployment, `CARGO_WORKSPACE.md` for crates, `ROADMAP.md` for stages and progress, `adr/` for the reasons behind a decision. Do not reopen a decision recorded in an ADR unless a measurement contradicts it. If it does, write a new ADR.

## Commands

```bash
cargo test --workspace
cargo clippy --all-targets -- -D warnings
cargo build --release
```

## Workflow

**One session per step.** Work is cut into steps small enough for one session, so state lives in the code and `docs/`, never in a conversation.

Starting a session:
1. Find the first unchecked step of the current stage in `docs/ROADMAP.md`.
2. Read only the docs and code that step needs. Do not re-explore the whole repository.
3. Do that step and nothing beyond it.

Ending a session:
1. Tests and clippy pass, or the failure is reported as is.
2. Get an independent review of the step's code, from a reviewer that did not write it and starts without the session's context (the `code-review` skill or a fresh subagent). Fix what it confirms, with a failing test first, and report what is left open.
3. Check the step in `docs/ROADMAP.md`. Anything learned that changes the design goes into `docs/` or an ADR. Anything later steps must know goes into Notes below.
4. Summarize what was done and what comes next, then stop.

Other rules:
- **Tests first.** Write the failing test, watch it fail, implement, watch it pass.
- **Commit only when asked.** Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/): `feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `chore:`, with an optional crate as scope, as in `feat(collab): add room unloading`.
- **Stay lean on tokens.** Redirect long command output to a file and read its tail. Do not repeat content already shown.
- **Report results as they are.** If a RAM target is missed, say so with the analysis. Never adjust a benchmark scenario to make a number pass.

## Conventions

- **Language:** everything in the repository is in English: docs, code comments, commit messages and UI strings.
- **Errors:** `thiserror` in library crates, `anyhow` in binaries.
- **Dependencies:** each one must justify its RAM and binary cost, and be compatible with AGPL-3.0 (ADR 0010). Update `docs/CARGO_WORKSPACE.md` when adding one.
- **Reference project:** https://github.com/MathieuMarthy/uptime-monitoring uses the same stack (SQLite setup in `src/db.rs`, embedded assets in `src/assets.rs`, release profile).
- **Design:** the UI follows the Brut design system, documented in `docs/DESIGN_SYSTEM.md` from step 3 on. IBM Plex is embedded in the binary, never loaded from Google Fonts (ADR 0004). Brut's examples are in French, and the UI translates them to English.

## Notes

- **Prototype limits:** no real auth, REST API, search or Markdown conversion. `Authorizer` is a trait with a permissive implementation, and the server binds to `127.0.0.1`. Do not expose it.
- **Browser tests:** Playwright reads the browser path from `CHROMIUM_PATH` through `executablePath`, and uses its own Chromium when unset. Any Chromium-based browser works.
- **yrs 0.28:** `yrs::sync` has `Awareness`, `Message`, `SyncMessage`, `MessageReader`, `DefaultProtocol` and the `Protocol` trait. Filter around `Protocol::handle_message` to reject `SyncStep2` and `Update` in read-only mode. `Doc` is `Send + Sync`. Enable the `sync` feature if the async or observer APIs require it.
- **y-websocket** appends `/<room>` to the server URL, and reconnects whatever the close code.
- **Yjs document layout:** root types `title` (`Y.Text`) and `default` (`XmlFragment`, TipTap's default field). See `docs/DATA_MODEL.md`.
- **Store:** `DocStore` (`cadocs-store`) uses native `impl Future + Send` methods, so collab is generic over it instead of using `dyn`. The room assigns `seq`, starting from `StoredDoc::last_seq() + 1`. `compact` refuses a snapshot older than the stored one with `StoreError::StaleSnapshot`. `append_updates` can be replayed after a lost commit, and returns `StoreError::SeqConflict` for a `seq` the snapshot covers or that holds other data. A room must flush its queue before it compacts. `SqliteStore::open` takes the database file path, and its parent folder must exist. There is no in-memory store or `close()` yet. `StoreError` wraps `sqlx::Error`, so a failure-injecting test store can return `sqlx::Error::PoolTimedOut`.
- **Permissions:** `cadocs-core::permissions` covers the matrix, the special cases and the recompute of page sockets. The `4401` closes and the recompute of events sockets are not there yet: they come with sessions in the MVP. `would_remove_last_instance_admin` is a separate check, so MVP handlers must call it before disabling, deleting or demoting a user, or fold it into the actions as `last_owner` is. `Authorizer` and its `Permissive` implementation are in `cadocs-collab`.
- **Collab:** `DocRegistry::connect(doc, mode)` returns a `Connection`, and the server only moves bytes: `receive(frame)` (an `Err(Close(code))` means close with it) and `next().await` (cancel safe, `Frame` or `Close`). Map `CollabError::AtCapacity` to `503` before the upgrade. Call `shutdown()` before exiting and report `healthy()` in `/health`. The server still owns pings, `CollabConfig` from the environment, and the text-frame rejection. The `max_frame_bytes` check also belongs in axum's limit, or a large frame is buffered first. Closing sockets on a permission change is not there yet: it needs sessions.
- **yrs hardening:** yrs 0.28 can panic and even segfault on crafted updates, in release too, and `catch_unwind` only covers panics. Write access means trust, and this risk is accepted (ADR 0014). Keep unwinding on. `mutated_frames_do_not_break_the_room` pins client ids so it does not crash at random. A dependency that never arrives blocks that document's compaction for good: it needs a way out in the MVP. Awareness has per-socket limits but no per-room cap, and ownership is not checked yet: it needs sessions, because a reconnecting client reuses its id.
- **Store load:** `load` holds a document's whole update log in memory. Step 5 measures this peak before anything changes (streaming would alter the `DocStore` trait).
- **Benchmark:** the scenario and go or no-go rule in `docs/ARCHITECTURE.md` are fixed. Do not change them to make a number pass.
