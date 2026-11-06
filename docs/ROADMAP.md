# Roadmap

Each stage ends when all of its exit criteria hold. RAM figures are measured with `bench/rss.sh` on the release binary, and use RSS unless stated otherwise.

## Stage 0: prototype

Purpose: check the RAM hypothesis before building the product.

Scope: collaboration server, SQLite persistence, a minimal TipTap page, a benchmark.

Steps, one session each:

- [x] **Step 1:** Cargo workspace, `cadocs-core` (permissions), `cadocs-store` (migrations, updates, snapshots). Done when the matrix of [PERMISSIONS.md](PERMISSIONS.md) and the store tests pass.
- [x] **Step 2:** `cadocs-collab` (rooms, y-sync, batched flush, compaction, unload, limits from [OPERATIONS.md](OPERATIONS.md)). Done when replay, compaction equivalence, unload and reload, load/unload concurrency, read-only, awareness cleanup, room cap and flush failure tests pass.
- [x] **Step 3:** `cadocs-server` (`/ws/{doc}`, `/health`, embedded front end), `web/` (TipTap, Brut tokens), `docs/DESIGN_SYSTEM.md`. Done when the page edits and persists when opened by hand.
- [x] **Step 4:** Node test with the real `yjs` and `y-websocket`, Playwright test with two browsers. Done when both pass, including after a server restart.
- [x] **Step 5:** `cadocs-bench`, `bench/rss.sh`, allocator comparison, following the [benchmark scenario](ARCHITECTURE.md#benchmark-scenario). Done when the results are in `ARCHITECTURE.md` and the go or no-go is recorded.

Exit criteria:

- [x] Two browsers edit one document at the same time, with visible cursors.
- [x] Content survives a server restart.
- [x] Compaction and unloading keep the document identical (automated tests).
- [x] The real `yjs` and `y-websocket` clients pass the compatibility test.
- [x] RSS is measured idle, with 10 and 100 open documents, and after unload.
- [x] A go or no-go on the architecture is recorded, using the rule in the benchmark scenario.

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

Steps, one session each. Each step adds the migration for the tables it needs, and its REST routes come with tests of the permissions they use. A step that adds a dependency or a background task reports the idle RSS, so a drift shows before the final benchmark.

- [x] **Step 1:** HTTP foundation: `/api/v1` router, error shape, `CADOCS_PUBLIC_URL` and the `Origin` check, `CADOCS_TRUSTED_PROXIES`, rate limiter, security headers, read and write pools, data folder lock, `serve` subcommand. Done when each of these has a test and the prototype socket still works.
- [x] **Step 2:** Accounts and sessions: `users`, `sessions`, `workspaces`, `workspace_members`, `/auth/setup`, login, logout, `/session`, `/me`, session middleware, CSRF, Argon2 behind a semaphore. Done when first run, login, expiry, CSRF and rate limits are tested.
- [x] **Step 3:** Instance administration: `/admin/users`, `password_resets`, reset links, `cadocs reset-password`, the last instance admin check. Done when the instance actions of [PERMISSIONS.md](PERMISSIONS.md) pass over REST.
- [x] **Step 4:** Workspaces, members and invitations, with the last owner check. Done when the workspace actions pass over REST and an invited user can join with a link.
- [x] **Step 5:** Spaces and grants: routes, space trash, role resolution from database rows. Done when each rule of the resolution order passes over REST.
- [x] **Step 6:** Pages: `pages`, fractional index, tree, create, read, move, trash, restore, purge, recent pages. Done when the move and trash rules of [DATA_MODEL.md](DATA_MODEL.md#pages) pass over REST. The prototype socket stays as it is: its documents have no `pages` row.
- [x] **Step 7:** Page socket behind sessions: `/ws/pages/{id}` replaces `/ws/{doc}`, a real `Authorizer` replaces `Permissive`, `doc_id` becomes `page_id` with its foreign key, a flush copies the title to `pages`, the server can write to a room, and `PATCH /pages/{page}` uses that. `cadocs-bench`, its seed, `web/e2e/` and the prototype page all use the old route or columns, so they move in this step. Done when the end-to-end tests and a short benchmark run pass on the new route.
- [x] **Step 8:** Socket closes and collab hardening: every trigger of [Reacting to changes](PERMISSIONS.md#reacting-to-changes) on page sockets, awareness ownership, cursor shape validation, a per-room awareness cap, a way out for a dependency that never arrives. Done when revoking a role closes the user's page sockets with the right code, in a test.
- [x] **Step 9:** Events socket: `/ws/events`, workspace hub, `resync`, the same triggers on events sockets, a socket count shared with page sockets. Done when two clients see each other's tree changes and a lagging client gets `resync`.
- [ ] **Step 10:** `cadocs-jobs` and the reader outside a room: `jobs`, worker, retries, purge of sessions, tokens and the trash, `CADOCS_DOC_READS`. Done when a crash during a job, retries and the read bound are tested.
- [ ] **Step 11:** Svelte shell: routing, setup, login, invitation and reset pages, account, instance administration, light and dark themes, phone layout, the source code link, `4401`, `4403` and `4404` shown in the page. Done when Playwright goes from first run to an empty workspace.
- [ ] **Step 12:** Navigation: spaces, lazy tree with drag and drop, trash, recent pages, live updates from the events socket. Done when two browsers see each other's tree changes.
- [ ] **Step 13:** Administration screens: workspace settings, members, invitations, space settings and grants. Comes after step 14, which brings the page view. Done when Playwright invites a user and changes their role, and the open page reacts.
- [ ] **Step 14:** Editor, first blocks: page view in the shell, read-only mode from the role, slash menu, headings, lists, tasks, code with syntax highlighting, quotes, dividers, links, Markdown shortcuts. Done when each block works in Playwright with two browsers.
- [ ] **Step 15:** Editor, other blocks: callouts, toggles, tables, links to pages, table of contents. Done when each block works in Playwright with two browsers.
- [ ] **Step 16:** `cadocs-content`: `XmlFragment` to AST, renderers to text, HTML and Markdown, Markdown to AST to Yjs, and the test that compares the Rust and TipTap schemas (ADR 0008). Done when every node round-trips and HTML escaping and the URL allow-list are tested.
- [ ] **Step 17:** Attachments: `cadocs-files`, streaming upload, download, limits and quota, reference tracking, orphan cleanup, image and file blocks. Done when the attachment rules of [DATA_MODEL.md](DATA_MODEL.md#attachments) pass, including a full disk. The reference added when a page's content holds an attachment belongs to step 18, which owns the indexing job.
- [ ] **Step 18:** Search: `cadocs-search`, stemming, FTS5, the reindex job, the search route and screen. The indexing job also adds the attachment references it finds. Done when results respect readable spaces, stemming is tested in English and French, and an image pasted into another page is readable there.
- [ ] **Step 19:** Revisions: creation job, list, view, restore, thinning, screens. Done when a restore reaches connected clients as a normal edit and keeps the previous content as a revision.
- [ ] **Step 20:** Markdown import and export for one page, with or without attachments, and the print stylesheet. Done when an export imports back with its content and attachments.
- [ ] **Step 21:** Delivery: `cadocs backup` and `cadocs restore`, Docker image, `aarch64` and `x86_64` binaries, [OPERATIONS.md](OPERATIONS.md) complete. Done when a backup and restore round trip passes on a running server.
- [ ] **Step 22:** Coverage: the whole permission matrix over REST and WebSocket, and a check of each scope item against its tests. Done when nothing in the scope lacks an automated test.
- [ ] **Step 23:** MVP benchmark: idle, 30 active users, login peaks, 10,000 pages, and the value of `CADOCS_MAX_ROOMS`. Comes last, after step 24, whose fixes could invalidate the measures. Done when the results are recorded against the exit criteria, as measured.
- [ ] **Step 24:** Security review of auth, uploads and rendered HTML. Done when its findings are fixed or recorded.

The two weeks of daily use start once steps 23 and 24 are both done.

### Order and parallel work

Steps keep their numbers, but this order wins over them: the next step is the first unchecked one in this table, and steps of one wave can run at the same time, each in its own worktree. A session running in parallel is told its step.

| Wave | Steps | Needs |
| --- | --- | --- |
| 1 | 9, 10, 11 | Nothing left |
| 2 | 12, 14 | 12: 9 and 11. 14: 11 |
| 3 | 13, 15 | 13: 14. 15: 14, and 12 for picking a page to link to |
| 4 | 16 | 15, for the schema comparison test |
| 5 | 17, 18, 19 | 10 and 16. 17 and 19 also need 14 for their blocks and screens |
| 6 | 20 | 16 and 17 |
| 7 | 22, 24 | Every feature step |
| 8 | 23 | Everything, alone on a quiet machine |

Step 21 needs nothing left and fits any wave before 7. If it runs before step 17, its round trip test gains attachments afterwards, and `OPERATIONS.md` is finished at the end. Step 16 can start beside 14 and 15 for the AST and renderers, once the node list is fixed, but it only ends after 15.

When steps run in parallel:

- **Migrations:** agree on the numbers before starting, since two branches would otherwise both take the next one. This matters most in wave 5.
- **Steps 9 and 10:** both touch `lib.rs` and `DocRegistry`. When they meet, check that the trash purge job publishes the same events as `purge_page`.
- **Steps 12 and 14:** both touch the Svelte routes.
- **Idle RSS:** a step only measures its own branch, so measure again after each merge to see the combined drift.

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
