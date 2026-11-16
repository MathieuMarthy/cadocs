# Roadmap

Each stage ends when all of its exit criteria hold. RAM figures are measured with `bench/rss.sh` on the release binary, and use RSS unless stated otherwise.

A step to do says what it covers and when it is done, one session each. Once checked, it shrinks to one line: its lessons go to `docs/notes/`, its reasons to an ADR.

## Stage 0: prototype (done)

Checked the RAM hypothesis before building the product: collaboration server, SQLite persistence, a minimal TipTap page, a benchmark. Verdict **go**, on glibc with `malloc_trim` after unload (ADR 0015). Results in [ARCHITECTURE.md](ARCHITECTURE.md#measured-results).

- [x] Step 1: workspace, `cadocs-core` permissions, `cadocs-store`.
- [x] Step 2: `cadocs-collab` rooms, sync, flush, compaction, unload.
- [x] Step 3: `cadocs-server`, `web/` with TipTap and Brut tokens, `DESIGN_SYSTEM.md`.
- [x] Step 4: compatibility test with real `yjs` and `y-websocket`, Playwright with two browsers.
- [x] Step 5: `cadocs-bench`, `bench/rss.sh`, allocator comparison, go or no-go.

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
- Image and file attachments on local disk or S3, chosen by the operator, with size limits (ADR 0020)
- Full-text search (FTS5, stemming)
- Revisions: list, view, restore, retention
- Brut interface in Svelte, light and dark themes, usable on a phone browser
- A link to the source code in the interface (ADR 0010)
- Docker image, `aarch64` and `x86_64` binaries, backup and restore commands, [OPERATIONS.md](OPERATIONS.md) complete

Rules every step followed, and later stages keep: each step adds the migration for the tables it needs, its REST routes come with tests of the permissions they use, and a step that adds a dependency or a background task reports the idle RSS.

Steps, all done. What each one taught is in `docs/notes/`, its tests in [COVERAGE.md](COVERAGE.md).

- [x] Step 1: HTTP foundation (`/api/v1`, error shape, `Origin` check, trusted proxies, rate limiter, security headers, pools, data folder lock).
- [x] Step 2: accounts and sessions, CSRF, Argon2 behind a semaphore.
- [x] Step 3: instance administration, reset links, `cadocs reset-password`.
- [x] Step 4: workspaces, members, invitations.
- [x] Step 5: spaces, grants, space trash, role resolution.
- [x] Step 6: pages, fractional index, tree, trash, recent pages.
- [x] Step 7: page socket `/ws/pages/{id}` behind sessions, real `Authorizer`.
- [x] Step 8: socket closes on access changes, awareness hardening.
- [x] Step 9: events socket `/ws/events`, `resync`.
- [x] Step 10: `cadocs-jobs`, purges, reader outside a room (`CADOCS_DOC_READS`).
- [x] Step 11: Svelte shell, setup, login, account, administration, themes.
- [x] Step 12: navigation, lazy tree with drag and drop, live updates.
- [x] Step 12a: icons and action menus (ADR 0017).
- [x] Step 12b: shell layout, drawer under 840px, user menu, breadcrumb.
- [x] Step 12c: home, page, trash, account and administration screens.
- [x] Step 13: workspace and space settings, members, invitations, grants.
- [x] Step 14: editor, first blocks, block registry, Markdown source under the caret (ADR 0016, 0018).
- [x] Step 15: callouts, toggles, tables, links to pages, table of contents.
- [x] Step 16: `cadocs-content`, Yjs ↔ AST ↔ text, HTML, Markdown, schema comparison (ADR 0008).
- [x] Step 17: attachments, `cadocs-files`, quota, orphan cleanup.
- [x] Step 18: search, `cadocs-search`, FTS5, stemming, reindex job.
- [x] Step 19: revisions, thinning, restore.
- [x] Step 19a: SQLite upkeep (ADR 0019).
- [x] Step 20b: S3 file store (ADR 0020).
- [x] Step 20c: zstd snapshots.
- [x] Step 20d: room memory budget, `CADOCS_ROOM_MEMORY_MB`.
- [x] Step 20: Markdown import and export of one page, print stylesheet.
- [x] Step 21: `cadocs backup` and `restore`, Docker image, release binaries.
- [x] Step 22: full permission matrix over REST and sockets, [COVERAGE.md](COVERAGE.md).
- [x] Step 23: MVP benchmark ([MVP results](ARCHITECTURE.md#mvp-results)).
- [x] Step 24: security review of auth, uploads and rendered HTML.
- [x] Step 23b: `CADOCS_MAX_ROOMS` removed (ADR 0022).
- [x] Step 23a: fixed mmap threshold for password hashes (ADR 0023).
- [x] Step 25: leaving a trashed page for its parent.
- [x] Step 26: any user creates a workspace (ADR 0024).
- [x] Step 27: motion tokens and transitions.

Exit criteria:

- [x] All of the scope is covered by automated tests, including the permission matrix over REST and WebSocket ([COVERAGE.md](COVERAGE.md)).
- [x] Revoking a role closes the user's open sockets, and this is tested.
- [x] Idle RSS under 30 MB.
- [x] RSS under 100 MB with 30 simulated active users on 30 pages, each with one page socket and one events socket.
- [x] After login peaks, RSS returns to the idle target plus at most 10 MB. Missed in step 23, then 12.2 MB against an idle of 10.8 after step 23a (ADR 0023).
- [ ] 10,000 pages load and search with p95 latency under 100 ms on a Raspberry Pi 4 or an equivalent 1 vCPU VPS. Step 23 measured 16 ms on one desktop core: run `bench/rss.sh mvp` on such a machine to check it.
- [x] A backup and restore round trip is tested, on a running server.
- [x] A security review of auth, uploads and rendered HTML is done.
- [ ] One team uses it daily for two weeks with no data loss.

## Mail

Purpose: an instance with a mail relay lets people register, recover a password and add a second factor by themselves, and an instance without one works as before (ADR 0026).

Steps, all done. The rules of the MVP steps applied.

- [x] Step 28: `cadocs-mail`, `SmtpMailer` on `lettre`, `CADOCS_SMTP_*`, `cadocs test-mail`.
- [x] Step 29: forgot password, the outbox.
- [x] Step 30: registration by mailed code, `CADOCS_REGISTRATION`.
- [x] Step 31: second factor by mail.
- [x] Step 32: proof of a new address.

Exit criteria:

- [x] With `CADOCS_SMTP_HOST` unset, no route or screen of this stage answers, and the MVP tests pass unchanged.
- [ ] No route tells whether an address has an account. Open: with registration open and `CADOCS_WORKSPACE_CREATION=all`, the defaults, anyone registers, creates a workspace, invites an address and reads `has_account` from `GET /invitations/{token}`. Without a relay, `PATCH /me` still answers `409` for a taken address, as in the MVP.
- [x] Idle RSS and active-user thresholds of the MVP still hold.

## V1

Purpose: a credible replacement for Docmost.

Scope:

- Comments, threads, resolution, mentions of users and pages, in-app notifications
- Public sharing with server-rendered pages, and optional indexing by search engines
- Groups and group grants
- OIDC login, TOTP multi-factor authentication (a code by mail came first, ADR 0026)
- Bulk Markdown import and export, space export, Docmost and Notion import through their Markdown exports (what these exports lose is checked when this stage starts)
- Backlinks, page duplication and copy to another space
- Math (KaTeX) and Mermaid blocks, loaded on demand in the browser
- Diff between revisions
- Email notifications through the relay of ADR 0026
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
- Plugins from an operator library (ADR 0016): content plugins offered per workspace, user plugins such as themes and client-side exports enabled per user, built-in plugins embedded in the binary
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
