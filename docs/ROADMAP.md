# Roadmap

Each stage ends when all of its exit criteria hold. RAM figures are measured with `bench/rss.sh` on the release binary, and use RSS unless stated otherwise.

## Stage 0: prototype

Purpose: check the RAM hypothesis before building the product.

Scope: collaboration server, SQLite persistence, a minimal TipTap page, a benchmark.

Steps, one session each:

- [ ] **Step 1:** Cargo workspace, `cadocs-core` (permissions), `cadocs-store` (migrations, updates, snapshots). Done when the permission matrix and store tests pass.
- [ ] **Step 2:** `cadocs-collab` (rooms, y-sync, batched flush, compaction, unload). Done when replay, compaction equivalence, unload and reload, load/unload concurrency, read-only and awareness cleanup tests pass.
- [ ] **Step 3:** `cadocs-server` (`/ws/{doc}`, `/health`, embedded front end), `web/` (TipTap, Brut tokens), `docs/DESIGN_SYSTEM.md`. Done when the page edits and persists when opened by hand.
- [ ] **Step 4:** Node test with the real `yjs` and `y-websocket`, Playwright test with two browsers. Done when both pass, including after a server restart.
- [ ] **Step 5:** `cadocs-bench`, `bench/rss.sh`, allocator comparison. Done when the results are in `ARCHITECTURE.md` and the go or no-go is recorded.

Exit criteria:

- [ ] Two browsers edit one document at the same time, with visible cursors.
- [ ] Content survives a server restart.
- [ ] Compaction and unloading keep the document identical (automated tests).
- [ ] The real `yjs` and `y-websocket` clients pass the compatibility test.
- [ ] RSS is measured idle, with 10 and 100 open documents, and after unload.
- [ ] A go or no-go on the architecture is recorded, based on those measurements.

## MVP

Purpose: a small team can use cadocs as its only wiki.

Scope:

- Email and password accounts, sessions, invitations
- Workspaces, spaces, page tree, trash
- Per-space roles
- Collaborative editor with the core blocks: headings, lists, tasks, code, quotes, tables, links, images
- Markdown shortcuts, Markdown import and export for one page
- Image and file attachments on local disk
- Full-text search (FTS5, stemming)
- Revisions: list, view, restore
- Brut interface in Svelte
- Docker image, `aarch64` and `x86_64` binaries, backup procedure

Exit criteria:

- [ ] All of the scope is covered by automated tests, including the permission matrix over REST and WebSocket.
- [ ] Revoking a role closes the user's open sockets, and this is tested.
- [ ] Idle RSS under 30 MB.
- [ ] RSS under 100 MB with 30 simulated active users on 30 pages.
- [ ] 10,000 pages load and search with p95 latency under 100 ms on a Raspberry Pi 4 or an equivalent 1 vCPU VPS.
- [ ] A backup and restore round trip is tested, on a running server.
- [ ] A security review of auth, uploads and rendered HTML is done.
- [ ] One team uses it daily for two weeks with no data loss.

## V1

Purpose: a credible replacement for Docmost.

Scope:

- Comments, threads, resolution, mentions and in-app notifications
- Public sharing with server-rendered pages
- Groups and group grants
- OIDC login
- Bulk Markdown import and export, Docmost and Notion import
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
- Page diffing between revisions
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
