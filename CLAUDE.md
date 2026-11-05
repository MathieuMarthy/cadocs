# cadocs

Self-hosted collaborative wiki, a lightweight Docmost alternative. Top priority: minimal server RAM (10 to 30 MB idle, under 100 MB for a few dozen active users). One binary plus one data folder, no mandatory external service.

Design lives in `docs/`: read only what the current task needs. `ARCHITECTURE.md` for the flows, `API.md` for the wire protocol, `DATA_MODEL.md` for the schema, `CARGO_WORKSPACE.md` for crates, `ROADMAP.md` for stages and progress, `adr/` for the reasons behind a decision. Do not reopen a decision recorded in an ADR unless a measurement contradicts it. If it does, write a new ADR.

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
2. Check the step in `docs/ROADMAP.md`. Anything learned that changes the design goes into `docs/` or an ADR. Anything later steps must know goes into Notes below.
3. Summarize what was done and what comes next, then stop.

Other rules:
- **Tests first.** Write the failing test, watch it fail, implement, watch it pass.
- **Commit only when asked.**
- **Stay lean on tokens.** Redirect long command output to a file and read its tail. Do not repeat content already shown.
- **Report results as they are.** If a RAM target is missed, say so with the analysis. Never adjust a benchmark scenario to make a number pass.

## Conventions

- **Language:** everything in the repository is in English: docs, code comments, commit messages and UI strings.
- **Errors:** `thiserror` in library crates, `anyhow` in binaries.
- **Dependencies:** each one must justify its RAM and binary cost. Update `docs/CARGO_WORKSPACE.md` when adding one.
- **Reference project:** https://github.com/MathieuMarthy/uptime-monitoring uses the same stack (SQLite setup in `src/db.rs`, embedded assets in `src/assets.rs`, release profile).
- **Design:** the UI follows the Brut design system, documented in `docs/DESIGN_SYSTEM.md` from step 3 on. IBM Plex is embedded in the binary, never loaded from Google Fonts (ADR 0004).

## Notes

- **Prototype limits:** no real auth, REST API, search or Markdown conversion. `Authorizer` is a trait with a permissive implementation, and the server binds to `127.0.0.1`. Do not expose it.
- **Browser tests:** Playwright reads the browser path from `CHROMIUM_PATH` through `executablePath`, and uses its own Chromium when unset. Any Chromium-based browser works.
- **yrs 0.28:** `yrs::sync` has `Awareness`, `Message`, `SyncMessage`, `MessageReader`, `DefaultProtocol` and the `Protocol` trait. Filter around `Protocol::handle_message` to reject `SyncStep2` and `Update` in read-only mode. `Doc` is `Send + Sync`. Enable the `sync` feature if the async or observer APIs require it.
- **y-websocket** appends `/<room>` to the server URL, and reconnects whatever the close code.
