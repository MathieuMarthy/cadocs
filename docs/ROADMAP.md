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

Purpose, reached: a small team can use cadocs as its only wiki, and an instance with a mail relay lets people register, recover a password and add a second factor by themselves (ADR 0026). Done in steps 1 to 32: server and sessions (1 to 10), Svelte interface (11 to 15, 27), content, files, search and revisions (16 to 20d), operations, permission matrix, benchmark and security review (21 to 26), mail (28 to 32). What each step taught is in `docs/notes/`, its tests in [COVERAGE.md](COVERAGE.md), its measures in [ARCHITECTURE.md](ARCHITECTURE.md#mvp-results).

Scope: email and password accounts with links that need no mail server (ADR 0013), mail by an optional relay, instance administration, workspaces, spaces, page tree, trash, per-space roles, live updates (ADR 0011), the collaborative editor and its blocks, Markdown import and export of one page, printing, attachments on disk or S3 (ADR 0020), full-text search, revisions, the Brut interface in light and dark, Docker image, release binaries, backup and restore.

Rules every step followed, and later stages keep: each step adds the migration for the tables it needs, its REST routes come with tests of the permissions they use, and a step that adds a dependency or a background task reports the idle RSS.

Exit criteria, all met: tests cover the scope and the permission matrix over REST and WebSocket; revoking a role closes sockets; idle RSS under 30 MB; under 100 MB with 30 active users; after login peaks, idle plus at most 10 MB (ADR 0023); backup and restore tested on a running server; security review done; without `CADOCS_SMTP_HOST`, nothing of the mail stage answers. Search latency (16 ms p95 on one desktop core) and daily use were accepted by the maintainer without a Raspberry Pi run or a full two weeks.

Accepted risk: a user who can invite learns whether an address has an account ([OPERATIONS.md](OPERATIONS.md#security)).

## V1

Purpose: a credible replacement for Docmost.

Scope:

- Comments, threads, resolution, mentions of users and pages, in-app notifications
- Public sharing with server-rendered pages, and optional indexing by search engines
- Groups and group grants
- OIDC login, TOTP multi-factor authentication (a code by mail came first, ADR 0026)
- Bulk Markdown import and export, space export
- Backlinks, page duplication and copy to another space
- Math (KaTeX) and Mermaid blocks, loaded on demand in the browser
- Diff between revisions
- Email notifications through the relay of ADR 0026
- Internationalized interface, in English and French
- Page templates

Decided when the stage started: math and Mermaid render in the browser only, so public pages and exports show their source as a code block. OIDC is tested against a fake provider in Rust, and checked by hand once with Authentik and Keycloak.

Steps. The rules of the MVP steps apply, and every new screen takes its strings from the catalogue of step 34.

- [x] Step 33: upgrade tests. One fixture database per release (`v0.1.0`, `v0.2.0`), opened by the current build in a test that checks its data. CI fails if the release binary passes 30 MB or a default build links S3 code. Done when both checks run in CI.
- [x] Step 34: i18n foundation. A string catalogue in `web/`, English as the source, a language setting per user that falls back to the browser's, every existing string moved into it. Done when the UI looks the same in English and a test fails on a string left out of the catalogue.
- [x] Step 35: groups, server. REST routes of groups and their members, `access_changed()` on each change, the permission matrix extended. Done when group grants change what a member reaches, over REST and sockets.
- [x] Step 36: groups, screens. Groups in workspace settings, groups in space grants.
- [x] Step 37: comments, server. Migration, REST routes, resolve and reopen, one level of replies, anchors stored as given (ADR 0012), a `comment` event on the events socket, permission tests.
- [x] Step 38: comments, screens. The side panel, anchors from the selection, highlights as decorations, lost anchors shown with their quote, resolve and reopen.
- [x] Step 39: mentions. A user mention node in the editor and in `cadocs-content`, mentions of users and pages in comments, body mentions diffed by the reindex job into `mentions`.
- [x] Step 40: backlinks. `page_links` filled by the reindex job, a route and a panel listing the pages that link here.
- [x] Step 41: in-app notifications. `notifications` for mentions, replies and comments on one's pages, the `notification` event, a list with unread count and read state.
- [x] Step 42: mail notifications. Through the relay only, a setting per user, grouped by a job so one burst sends one message. Done when, with no relay, nothing of it answers.
- [x] Step 43: math and Mermaid. Block and inline math, Mermaid blocks, loaded on demand in the editor. `cadocs-content` writes their source to Markdown and as a code block in HTML.
- [x] Step 44: revision diff. Compare a revision with the current page or another revision, on the screen of revisions.
- [x] Step 45: public shares, server. `page_shares`, its routes, `/s/{token}` rendered by the bounded reader, descendants, files, `ETag`, the strict CSP, `X-Robots-Tag` unless indexable, a rate limit. Done when a test shows a script in content cannot run there.
- [x] Step 46: public shares, screens. A share dialog on the page: children, indexing, expiry, revoke.
- [x] Step 47: TOTP. `user_totp`, recovery codes, login asking for the code, the account screen. Mail codes (ADR 0026) keep working beside it.
- [x] Step 48: OIDC, server. Providers in `CADOCS_OIDC_*`, code flow with PKCE, `user_identities`, how an identity meets an existing account, tests against a fake provider. An ADR for the HTTP client and its RAM and binary cost.
- [x] Step 49: OIDC, screens and providers. Login buttons, linking on the account screen, a manual check with Authentik and Keycloak written into `OPERATIONS.md`.
- [x] Step 50: duplication. `POST /pages/{page}/duplicate`, with descendants and attachments, into the same space or another, through a room.
- [x] Step 51: templates. `templates`, saving a page as one, a new page from one.
- [x] Step 52: exports. `format=html` for one page, the space export as a job behind `/jobs/{id}`, in Markdown or HTML, streamed as a zip.
- [x] Step 53: bulk import. A zip of Markdown files and folders becomes a tree of pages with its attachments, as a job.
- [ ] Step 54: French. Every string of the catalogue translated, the language picker on the account screen.
- [ ] Step 55: V1 exit. Benchmark, binary size, security review of shares, OIDC, TOTP and comments, `COVERAGE.md` and the permission matrix for the new routes.

Parallel work. Steps 38 to 53 fall into independent lanes. Each lane runs in order, and lanes can run side by side in separate branches or worktrees:

| Lane | Steps | Depends on |
| --- | --- | --- |
| Discussion | 38 → 39 → 41 → 42 | 39 needs the comment panel of 38 for mentions in comments; 41 notifies mentions; 42 mails what 41 stores |
| Content | 40, 43, 44, in any order | none |
| Shares | 45 → 46 | none |
| Login | 47 → 48 → 49 | none, but 47 and 48 both change the login flow, so not at once |
| Page copies | 50 → 51 | 51 reuses the copy of 50 to create a page from a template |
| Archives | 52 → 53 | 53 reuses the job and zip handling of 52 |

Step 54 comes after every step with a screen (38, 41, 42, 44, 46, 47, 49, 51, 52, 53), and step 55 after all of them.

Lanes do not depend on each other, but they share files, so merge one branch at a time and rebase the others:

- Migrations are numbered in sequence (`crates/cadocs-store/migrations/sqlite/`): renumber at merge, and keep the upgrade test of step 33 green.
- The string catalogue `web/src/lib/i18n/en.ts` grows in every screen step.
- 39 and 40 both change the reindex job (`crates/cadocs-jobs/src/index.rs`); 39 and 43 both add nodes to the editor schema and to `cadocs-content`.
- 43, 45 and 52 all touch the HTML renderer of `cadocs-content`.
- `ROADMAP.md`, `COVERAGE.md`, `docs/notes/` and the permission matrix change at the end of every step.
- The idle RSS each step reports is measured on its own branch, then again once merged if the step added a dependency or a background task.

Exit criteria:

- [ ] Public pages contain no JavaScript requirement, and a test confirms a script in content cannot execute.
- [ ] OIDC works with at least two providers (for example Authentik and Keycloak).
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
