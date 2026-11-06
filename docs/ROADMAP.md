# Roadmap

Each stage ends when all of its exit criteria hold. RAM figures are measured with `bench/rss.sh` on the release binary, and use RSS unless stated otherwise.

## Stage 0: prototype

Purpose: check the RAM hypothesis before building the product.

Scope: collaboration server, SQLite persistence, a minimal TipTap page, a benchmark.

Steps, one session each:

- [ ] **Step 1:** Cargo workspace, `cadocs-core` (permissions), `cadocs-store` (migrations, updates, snapshots). Done when the matrix of [PERMISSIONS.md](PERMISSIONS.md) and the store tests pass.
- [ ] **Step 2:** `cadocs-collab` (rooms, y-sync, batched flush, compaction, unload, limits from [OPERATIONS.md](OPERATIONS.md)). Done when replay, compaction equivalence, unload and reload, load/unload concurrency, read-only, awareness cleanup, room cap and flush failure tests pass.
- [ ] **Step 3:** `cadocs-server` (`/ws/{doc}`, `/health`, embedded front end), `web/` (TipTap, Brut tokens), `docs/DESIGN_SYSTEM.md`. Done when the page edits and persists when opened by hand.
- [ ] **Step 4:** Node test with the real `yjs` and `y-websocket`, Playwright test with two browsers. Done when both pass, including after a server restart.
- [ ] **Step 5:** `cadocs-bench`, `bench/rss.sh`, allocator comparison, following the [benchmark scenario](ARCHITECTURE.md#benchmark-scenario). Done when the results are in `ARCHITECTURE.md` and the go or no-go is recorded.

Exit criteria:

- [ ] Two browsers edit one document at the same time, with visible cursors.
- [ ] Content survives a server restart.
- [ ] Compaction and unloading keep the document identical (automated tests).
- [ ] The real `yjs` and `y-websocket` clients pass the compatibility test.
- [ ] RSS is measured idle, with 10 and 100 open documents, and after unload.
- [ ] A go or no-go on the architecture is recorded, using the rule in the benchmark scenario.

## MVP

Purpose: a small team can use cadocs as its only wiki.

Scope:

- Email and password accounts, sessions, invitation and reset links with no mail server (ADR 0013)
- Instance administration: users, disabling, instance admins
- Workspaces, spaces, page tree, trash, recent pages
- Per-space roles
- Live updates of the tree and titles (ADR 0011)
- Collaborative editor with a slash menu and these blocks: headings, lists, tasks, code with syntax highlighting, quotes, callouts, toggles, dividers, tables, links, links to pages, images, file attachments
- Table of contents, generated in the browser
- Markdown shortcuts, Markdown import and export for one page, with or without attachments
- Printing to PDF through a print stylesheet
- Image and file attachments on local disk, with size limits
- Full-text search (FTS5, stemming)
- Revisions: list, view, restore, retention
- Brut interface in Svelte, light and dark themes, usable on a phone browser
- A link to the source code in the interface (ADR 0010)
- Docker image, `aarch64` and `x86_64` binaries, backup and restore commands, [OPERATIONS.md](OPERATIONS.md) complete

Exit criteria:

- [ ] All of the scope is covered by automated tests, including the permission matrix over REST and WebSocket.
- [ ] Revoking a role closes the user's open sockets, and this is tested.
- [ ] Idle RSS under 30 MB.
- [ ] RSS under 100 MB with 30 simulated active users on 30 pages, each with one page socket and one events socket.
- [ ] After login peaks, RSS returns to the idle target plus at most 10 MB.
- [ ] 10,000 pages load and search with p95 latency under 100 ms on a Raspberry Pi 4 or an equivalent 1 vCPU VPS.
- [ ] A backup and restore round trip is tested, on a running server.
- [ ] A security review of auth, uploads and rendered HTML is done.
- [ ] One team uses it daily for two weeks with no data loss.

## V1

Purpose: a credible replacement for Docmost.

Scope:

- Comments, threads, resolution, mentions of users and pages, in-app notifications
- Public sharing with server-rendered pages, and optional indexing by search engines
- Groups and group grants
- OIDC login, TOTP multi-factor authentication
- Bulk Markdown import and export, space export, Docmost and Notion import through their Markdown exports (what these exports lose is checked when this stage starts)
- Backlinks, page duplication and copy to another space
- Math (KaTeX) and Mermaid blocks, loaded on demand in the browser
- Diff between revisions
- S3 file storage, behind a Cargo feature
- Email notifications through optional SMTP
- Internationalized interface
- Page templates

Exit criteria:

- [ ] Public pages contain no JavaScript requirement, and a test confirms a script in content cannot execute.
- [ ] OIDC works with at least two providers (for example Authentik and Keycloak).
- [ ] A real Docmost export imports with its tree and attachments intact.
- [ ] Binary size stays under 30 MB, and a default build contains no S3 code.
- [ ] Idle RSS and active-user thresholds of the MVP still hold.
- [ ] Upgrade from every earlier release is tested through migrations.

## V2

Purpose: larger deployments and finer control.

Scope:

- Page-level restrictions (ADR 0009)
- PostgreSQL store (ADR 0002)
- Audit log
- Public API tokens and webhooks
- Page labels
- Embeds through iframes, with an allow-list that fits the CSP
- Attachment text extraction for search, if memory allows

Exit criteria:

- [ ] Page restrictions apply to the tree, search, WebSocket, exports and shares, with tests for each.
- [ ] The PostgreSQL store passes the same test suite as SQLite.
- [ ] Migration from SQLite to PostgreSQL is documented and tested.
- [ ] RSS thresholds still hold on the SQLite build.

## Out of scope for now

- Horizontal scaling (ADR 0006)
- Native mobile applications
- Offline-first editing beyond what Yjs provides during a short disconnection
- AI features
- Databases and Kanban views (Docmost's Bases)
- Synced blocks and presentation mode
- LDAP, SAML and SCIM. OIDC covers single sign-on
- Draw.io and Excalidraw: the first depends on a third-party service, the second is heavy in the browser
- Page icons and emoji: the Brut design system rules them out
