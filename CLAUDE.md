# cadocs

Self-hosted collaborative wiki, a lightweight Docmost alternative. Top priority: minimal server RAM (10 to 30 MB idle, under 100 MB for a few dozen active users). One binary plus one data folder, no mandatory external service.

Design lives in `docs/`: read only what the current task needs. `ARCHITECTURE.md` for the flows, `API.md` for the wire protocol, `DATA_MODEL.md` for the schema, `PERMISSIONS.md` for the access matrix, `OPERATIONS.md` for configuration and deployment, `CARGO_WORKSPACE.md` for crates, `ROADMAP.md` for stages and progress, `adr/` for the reasons behind a decision. Do not reopen a decision recorded in an ADR unless a measurement contradicts it. If it does, write a new ADR.

## Commands

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
npm --prefix web ci && npm --prefix web run build   # before a release build: the binary embeds web/dist
cargo build --release
npm --prefix web run test:e2e                       # after the two builds: it runs target/release/cadocs, whose web/dist is embedded
bench/rss.sh build && bench/rss.sh run              # RAM benchmark, about 12 minutes, results in target/bench
```

Run it with `CADOCS_PUBLIC_URL=http://127.0.0.1:3000 target/release/cadocs` and open `http://127.0.0.1:3000/?doc=<id>`. For front end work, set `CADOCS_PUBLIC_URL` to the Vite address (`http://localhost:5173`), run the server, then `npm --prefix web run dev`, which proxies `/ws` to it.

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
- **Design:** the UI follows the Brut design system, documented in `docs/DESIGN_SYSTEM.md`, with its tokens in `web/src/tokens.css`. IBM Plex is embedded in the binary, never loaded from Google Fonts (ADR 0004). Brut's examples are in French, and the UI translates them to English.

## Notes

- **Prototype limits:** the page socket has no auth yet, and there is no search or Markdown conversion. `Authorizer` is a trait with a permissive implementation, and the server binds to `127.0.0.1`. Do not expose it.
- **Browser tests:** Playwright reads the browser path from `CHROMIUM_PATH` through `executablePath`, and uses its own Chromium when unset. Any Chromium-based browser works.
- **End-to-end tests:** `web/e2e/` runs the real binary, on its own port and temporary data folder (`server.ts`, `CADOCS_BIN` picks another binary). `compat.test.ts` uses `node:test` with the real `yjs` and `y-websocket`, and needs Node 22.18 or later for type stripping and `node:sqlite`. `browsers.spec.ts` uses Playwright, with two real browser processes. An open client resends its whole document when it reconnects, so a test about the disk must have no client left when the server comes back. A paragraph's text holds the label of a remote caret: read it with `bodyLines`. Do not `goto("about:blank")` before another `goto`: the browser sometimes commits it twice, which aborts the next navigation.
- **yrs 0.28:** `yrs::sync` has `Awareness`, `Message`, `SyncMessage`, `MessageReader`, `DefaultProtocol` and the `Protocol` trait. Filter around `Protocol::handle_message` to reject `SyncStep2` and `Update` in read-only mode. `Doc` is `Send + Sync`. Enable the `sync` feature if the async or observer APIs require it.
- **y-websocket** appends `/<room>` to the server URL. By default it stops for good on close codes 4400 to 4499, so `web/src/main.ts` passes `shouldReconnect`, and any other client of this server must too. The MVP must still show `4401`, `4403` and `4404` in the page: today the status stays at "offline, retrying".
- **Yjs document layout:** root types `title` (`Y.Text`) and `default` (`XmlFragment`, TipTap's default field). See `docs/DATA_MODEL.md`.
- **Store:** `DocStore` (`cadocs-store`) uses native `impl Future + Send` methods, so collab is generic over it instead of using `dyn`. The room assigns `seq`, starting from `StoredDoc::last_seq() + 1`. `compact` refuses a snapshot older than the stored one with `StoreError::StaleSnapshot`. `append_updates` can be replayed after a lost commit, and returns `StoreError::SeqConflict` for a `seq` the snapshot covers or that holds other data. A room must flush its queue before it compacts. `SqliteStore::open` takes the database file path, and its parent folder must exist. There is no in-memory store or `close()` yet. `StoreError` wraps `sqlx::Error`, so a failure-injecting test store can return `sqlx::Error::PoolTimedOut`.
- **Permissions:** `cadocs-core::permissions` covers the matrix, the special cases and the recompute of page sockets. The `4401` closes and the recompute of events sockets are not there yet: they come with sessions in the MVP. `would_remove_last_instance_admin` is a separate check, so MVP handlers must call it before disabling, deleting or demoting a user, or fold it into the actions as `last_owner` is. `Authorizer` and its `Permissive` implementation are in `cadocs-collab`.
- **Collab:** `DocRegistry::connect(doc, mode)` returns a `Connection`, and the server only moves bytes: `receive(frame)` (an `Err(Close(code))` means close with it) and `next().await` (cancel safe, `Frame` or `Close`). Closing sockets on a permission change is not there yet: it needs sessions.
- **Server:** `cadocs-server` is a library plus the `cadocs` binary. `serve::<S, A, E>` takes the store, the authorizer and the embedded assets as type parameters, so tests pass a failing store and fixture assets. One task per socket does everything (`ws.rs`), because `Connection::next` takes `&mut self`: a blocked send stops that socket's reads, up to `CADOCS_PING_TIMEOUT_SECS`. The `tungstenite` version must stay the one axum uses, or the downcast behind `1009` stops matching (a test covers it). `serve` flushes rooms before it waits on HTTP, and bounds that wait, because a stalled client would otherwise hold up shutdown. A frame far over the limit can lose its `1009` to a TCP reset: `recv` ends at the capacity error, so the rest is never read.
- **HTTP foundation:** `api.rs` holds `ApiError` (build from `ErrorCode`, return it with `?`) and `api::router`: add REST routes before its `finish` call, which only reaches the routes that already exist. `api::rate_limit` is a route layer over `(Arc<RateLimiter>, Arc<HttpConfig>)`, and `api::count_request` does the same from a handler. Test a limit over a real socket, as `tests/auth.rs` does, since only `serve` provides the real `ConnectInfo`. The burst and refill of each limiter are chosen where it is wired. `RateLimiter` counts an IPv6 /48 as one client and tracks 4096 clients at most. Past that a newcomer waits, so 4096 live addresses or sites can hold up strangers, but never reset a client. `security::add_headers` covers every response and leaves alone what a handler set. `PublicUrl::allows` accepts a missing `Origin`, so REST must not rely on it alone. `/api/v1/` needs its own line in `router`, because a nest cannot reach it, so it answers `404` to any `Origin`.
- **Accounts and sessions:** add a route that needs a user to `authenticated` in `api/auth.rs`, or put `session::require` on it: that layer checks the session and CSRF, and handlers read `Extension<CurrentSession>`. `AccountStore` (`cadocs-store`) sits beside `DocStore`, and the server needs both. Read bodies with `api::JsonBody`, whose refusals are in the error shape. A storage error becomes `503` through `From<StoreError>`. Whatever depends on a password check must hold when the write happens, because a hash can wait 10 seconds: `create_session` and `UpdateBasis` re-check inside the write, and step 3's reset links must do the same. The last owner check in `delete_user` counts enabled owners only, and step 4 must use the same rule. `would_remove_last_instance_admin` is already called in `delete_user`, and step 3 still needs it for disabling and demoting. Nothing tells a client that first-run setup is pending, as `GET /session` answers `401` either way: step 11 needs a public read of `has_users`. A failed session touch retries on each request, each waiting up to 1 second, and has no back-off.
- **Socket RAM:** tungstenite's write buffer keeps the capacity of the largest frame sent, and no option shrinks it, so each socket holds about one SyncStep2 of its document for its whole life. Measured at 62 KiB per socket on the 20 KB benchmark document.
- **Front end:** plain TypeScript and TipTap for now, Svelte comes with the MVP shell (ADR 0004). `?doc=<id>` picks the document, `welcome` by default. `disableBc` is on, so two tabs of one browser really go through the server. Awareness holds `user: { name, slot }`, where `slot` (1 to 3) picks the presence color. The title is a plain input bound to `Y.Text` in `web/src/title.ts`.
- **yrs hardening:** yrs 0.28 can panic and even segfault on crafted updates, in release too, and `catch_unwind` only covers panics. Write access means trust, and this risk is accepted (ADR 0014). Keep unwinding on. `mutated_frames_do_not_break_the_room` pins client ids so it does not crash at random. A dependency that never arrives blocks that document's compaction for good: it needs a way out in the MVP. Awareness has per-socket limits but no per-room cap, and ownership is not checked yet: it needs sessions, because a reconnecting client reuses its id. The server only checks that a state is JSON, and a reader can send this too: `{"cursor":{}}` makes `Y.createRelativePositionFromJSON` throw in node, so it may break the cursor plugin of other clients (not tested in a browser). Validate the cursor shape in the MVP.
- **Store load:** `load` holds a document's whole update log in memory. Measured at 3.0 MB more for 100 documents of 2,000 rows, so it stays.
- **Benchmark:** the scenario and go or no-go rule in `docs/ARCHITECTURE.md` are fixed. Do not change them to make a number pass. Stage 0 ended on a go, after ADR 0015 added `malloc_trim` on unload. A loaded document costs about 530 KiB, 10 to 15 times its encoded size, so 256 rooms would pass 100 MB: the MVP must keep that in mind when it sets `CADOCS_MAX_ROOMS`. `cadocs-bench` is outside `default-members`: `--workspace` covers it, and building it with the server changes the server's features. glibc's in-use bytes need a preload probe, because `ptrace_scope` blocks `gdb -p`.
