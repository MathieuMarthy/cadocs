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

Purpose: a credible replacement for Docmost. Done in steps 33 to 56: upgrade tests and the size checks of CI (33), the string catalogue and French (34, 54), groups (35, 36), comments, mentions, backlinks and notifications in the app and by mail (37 to 42), math and Mermaid (43), revision diff (44), public shares (45, 46), TOTP and OIDC (47 to 49), duplication and templates (50, 51), exports and bulk import (52, 53), the exit review (55), and stale clients (56). What each step taught is in `docs/notes/`, its tests in [COVERAGE.md](COVERAGE.md), its measures in [ARCHITECTURE.md](ARCHITECTURE.md#v1-results).

Scope: comments with threads, resolution and mentions of users and pages, notifications in the app and by mail through the relay of ADR 0026, public sharing with server-rendered pages and optional indexing, groups and group grants, OIDC login and TOTP (a code by mail came first, ADR 0026), bulk Markdown import and export and space export, backlinks, page duplication and copy to another space, math and Mermaid loaded on demand in the browser, revision diff, page templates, the interface in English and French.

Decided when the stage started: math and Mermaid render in the browser only, so public pages and exports show their source as a code block. OIDC is tested against a fake provider in Rust, and checked by hand against real providers.

Rules later stages keep: every new screen takes its strings from the catalogue of step 34, in English and French. Branches worked side by side renumber their migrations at merge (`crates/cadocs-store/migrations/sqlite/`) and keep the upgrade test of step 33 green.

- [x] Step 56: stale clients, a page socket carries the fingerprint of the editor schema and another one closes with `4426` (ADR 0016).

Exit criteria met: public pages need no JavaScript, and a test shows a script in content cannot run; the binary stays under 30 MB, and a default build contains no S3 code; the idle RSS and active-user thresholds of the MVP still hold; upgrade from every earlier release is tested through migrations; a tab opened before an upgrade cannot remove content its schema does not know (step 56).

Exit criteria open:

- [ ] OIDC works with at least two providers (for example Authentik and Keycloak). The manual check of [OPERATIONS.md](OPERATIONS.md#single-sign-on-oidc) has not been run.

## V1.1

Purpose: close the gaps V1 left open, before new features.

Scope: what `docs/notes/` lists as left open and V1 users would meet: sessions on account changes, shares of people who left, room panics, prefix search, live mentions, the language of mails, links in copies and exports, two screen faults.

Steps. The rules of the MVP steps apply. Each step removes the items it fixes from the "Left open" lines of `docs/notes/`.

- [ ] Step 57: sessions and account changes. Turning on a second factor (authenticator app or code by mail) ends the person's other sessions, a password change or reset voids the pending codes by mail, and an address change mails the old address. Done when a test covers each.
- [ ] Step 58: shares follow their creator. A share stops answering once its creator can no longer edit its page or has left the workspace, recorded in an ADR as a refinement of ADR 0009. An expired share leaves the share dialog's list. Done when tests show the share of a removed member and of a demoted one answer `404`.
- [ ] Step 59: room panics. A room task that panics frees its `Unloading` slot and its memory estimate, so callers no longer get `ShuttingDown` until a restart. Done when a test with an injected panic shows the budget back where it was and the page opening again.
- [ ] Step 60: prefix search. A prefix typed past the end of its stem finds the word: `deploym` finds `deployment`. Done when search tests cover it in English and French, whole words stem as before, and the index size is measured before and after.
- [ ] Step 61: live body mentions. A mention the indexing job finds in a page body publishes the `notification` event, as a mention in a comment does. Done when a test sees it on the events socket.
- [ ] Step 62: language of mails. The server knows a person's language with no choice made: their setting, else the last browser language it saw (`Accept-Language`, or what the front end reports), else the workspace's language, else English. An invitation takes the workspace's. Every mail (notifications, codes, reset links, invitations) comes from a server-side catalogue in English and French. No IP geolocation: a GeoIP database costs RAM and binary size, and guesses wrong in countries with several languages. Done when a person with no setting and a French browser gets French mail, and a test fails on a mail string left untranslated.
- [ ] Step 63: links in copies. Links between pages of a duplicated subtree point at the copies, not the sources. Done when a test duplicates a tree whose pages link to each other, into the same space and another.
- [ ] Step 64: links in exports. A space export writes links between exported pages as relative file links, in Markdown and HTML, so the zip reads offline. Done when a test follows every page link of an exported zip to a file inside it.
- [ ] Step 65: screen faults. A wrong second-factor code says so, not `wrong email or password`, and a page link clicked inside a comment composer keeps the draft. Done when end-to-end tests cover both.
- [ ] Step 66: V1.1 exit. Benchmark, binary size, `COVERAGE.md`, and a review of what stays left open in `docs/notes/`.

Steps 57 to 65 are independent and can run in any order. 57 and 62 both touch mail, and 63 and 64 both rewrite page links, so rebase the second of each pair on the first.

Set aside, not in this stage:

- Accessibility: no keyboard way to move a page in the tree.
- OIDC: only RS256; an account made by a provider has no password, so the routes that need the current one are closed to it.
- Authenticator app: the per-account limit counts right codes too, so someone who holds the password can keep the owner at `429`; no screen shows how many recovery codes are left.

Exit criteria:
- [ ] Every step's fix has a test listed in `COVERAGE.md`, and its item is gone from `docs/notes/`.
- [ ] Binary size stays under 30 MB, and the RSS thresholds of the MVP still hold.

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
