# cadocs

Self-hosted collaborative wiki, a lightweight Docmost alternative. Top priority: minimal server RAM (10 to 30 MB idle, under 100 MB for a few dozen active users). One binary plus one data folder, no mandatory external service.

Design lives in `docs/`: read only what the current task needs. `ARCHITECTURE.md` for the flows, `API.md` for the wire protocol, `DATA_MODEL.md` for the schema, `PERMISSIONS.md` for the access matrix, `OPERATIONS.md` for configuration and deployment, `CARGO_WORKSPACE.md` for crates, `ROADMAP.md` for stages and progress, `adr/` for the reasons behind a decision, `notes/` for what past steps learned. Do not reopen a decision recorded in an ADR unless a measurement contradicts it. If it does, write a new ADR.

## Commands

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
npm --prefix web ci && npm --prefix web run build   # before a release build: the binary embeds web/dist
cargo build --release
npm --prefix web run test:e2e                       # after the two builds: it runs target/release/cadocs, whose web/dist is embedded
bench/rss.sh build && bench/rss.sh run              # RAM benchmark, about 12 minutes, results in target/bench
```

Run it with `CADOCS_PUBLIC_URL=http://127.0.0.1:3000 target/release/cadocs`. The page needs a session and a page id, and nothing offers them before step 11: `POST /api/v1/auth/setup` from the browser's console, create a space and a page over REST, then open `http://127.0.0.1:3000/?page=<id>`. For front end work, set `CADOCS_PUBLIC_URL` to the Vite address (`http://localhost:5173`), run the server, then `npm --prefix web run dev`, which proxies `/ws` and `/api` to it.

## Workflow

**One session per step.** Work is cut into steps small enough for one session, so state lives in the code and `docs/`, never in a conversation.

Starting a session:
1. Find the first unchecked step of the current stage in `docs/ROADMAP.md`.
2. Read only the docs and code that step needs. Do not re-explore the whole repository.
3. Do that step and nothing beyond it.

Ending a session:
1. Tests and clippy pass, or the failure is reported as is.
2. Get an independent review of the step's code, from a reviewer that did not write it and starts without the session's context (the `code-review` skill or a fresh subagent). Fix what it confirms, with a failing test first, and report what is left open. Follow **Review cost** below.
3. Check the step in `docs/ROADMAP.md`. Anything learned that changes the design goes into `docs/` or an ADR. Anything later steps must know goes into `docs/notes/`.
4. Summarize what was done and what comes next, then stop.

Other rules:
- **Tests first.** Write the failing test, watch it fail, implement, watch it pass.
- **Commit only when asked.** Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/): `feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `chore:`, with an optional crate as scope, as in `feat(collab): add room unloading`.
- **Stay lean on tokens.** Redirect long command output to a file and read its tail. Do not repeat content already shown.
- **Review cost.** The reviewer stays on the strongest model, and reads less. Write the production diff to a scratch file, with new files but no tests or docs (`git add -N .`, then `git diff -- 'crates/*/src/*' 'crates/*/migrations/*' 'web/src/*'`), and give it that path. Paste the passages of the spec that apply into its prompt, never the session's reasoning. Ask it to read by line ranges, in at most 8 tool calls, and to open a test only when in doubt. It does not judge test coverage then, so check that yourself. After a fix, continue the same reviewer with `SendMessage` instead of starting a new one.
- **Report results as they are.** If a RAM target is missed, say so with the analysis. Never adjust a benchmark scenario to make a number pass.

## Conventions

- **Language:** everything in the repository is in English: docs, code comments, commit messages and UI strings.
- **Errors:** `thiserror` in library crates, `anyhow` in binaries.
- **Dependencies:** each one must justify its RAM and binary cost, and be compatible with AGPL-3.0 (ADR 0010). Update `docs/CARGO_WORKSPACE.md` when adding one.
- **Reference project:** https://github.com/MathieuMarthy/uptime-monitoring uses the same stack (SQLite setup in `src/db.rs`, embedded assets in `src/assets.rs`, release profile).
- **Design:** the UI follows the Brut design system, documented in `docs/DESIGN_SYSTEM.md`, with its tokens in `web/src/tokens.css`. IBM Plex is embedded in the binary, never loaded from Google Fonts (ADR 0004). Brut's examples are in French, and the UI translates them to English.

## Notes

What later steps must know, by topic. Read the file of the code you touch, and add to it. Keep this section short: every session and subagent pays for it.

- **Prototype limits:** there is no search or Markdown conversion. The server binds to `127.0.0.1`. Do not expose it.
- **Access changes:** a handler whose change can alter what a user reaches calls `state.access_changed()` once it is stored ([store notes](docs/notes/store.md)).
- [docs/notes/collab.md](docs/notes/collab.md): rooms, yrs, y-websocket, Yjs layout, socket RAM.
- [docs/notes/server.md](docs/notes/server.md): server, page socket, HTTP foundation, sessions, administration.
- [docs/notes/store.md](docs/notes/store.md): stores, permissions, workspaces, spaces, pages.
- [docs/notes/web.md](docs/notes/web.md): front end, browser and end-to-end tests.
- [docs/notes/benchmark.md](docs/notes/benchmark.md): benchmark rules and measures.
