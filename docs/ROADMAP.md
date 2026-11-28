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

Purpose, reached: close the gaps V1 left open. Done in steps 57 to 66: sessions on account changes (57), shares that follow their creator (58, ADR 0030), room panics (59), prefix search (60), live body mentions (61), the language of mails (62), links in copies and exports (63, 64), two screen faults (65), the exit review (66). Lessons in `docs/notes/`, tests in [COVERAGE.md](COVERAGE.md), measures in [ARCHITECTURE.md](ARCHITECTURE.md#v11-results).

Exit criteria met: each fix has its test in `COVERAGE.md` and left `docs/notes/`; the binary is 20.4 MB and the MVP RSS thresholds hold.

Still left open, each in its note: keyboard moves in the tree, OIDC beyond RS256 and for accounts with no password, the authenticator's per-account limit and recovery code count, and smaller screens and edges (step 66 review: none loses data or reaches past a permission).

## V1.2

Purpose: answer what the first users reported after V1.1, the most pressing first: a confirmation before an account goes, then the faults they met, then the features they asked for.

Decided when the stage started: the invitation form suggests accounts of the whole instance, but only once 3 characters of a name or address are typed, an accepted risk that step 73 writes into [OPERATIONS.md](OPERATIONS.md#security). Tables resize their columns only: rows take the height of their content, and `prosemirror-tables` has no handle for them. The MCP server asked for goes to V2, on the API tokens planned there.

Steps. The rules of the earlier stages apply. Each step removes the items it fixes from the "Left open" lines of `docs/notes/`.

- [x] Step 67: confirm before deleting an account. `DELETE /me` takes `{ current_password }`: missing or wrong is a `400` that names it, and it shares the limit of password guesses. An account with no password (made by a provider) needs a session opened in the last 10 minutes instead, and the screen offers to log in again otherwise. The confirmation on the account screen asks for the password. Done when tests show a session without the password cannot delete its account, and an end-to-end test deletes one with it.
- [x] Step 68: screen faults. A setup code refused shows its field and says `wrong`: the code of the link stays hidden today, since `Form.run` calls `explain` only when no field is named, so `askCode` never turns on. The admin screen keeps the workspace the sidebar showed, not the first one (`guess` in `Shell.svelte`). Saving a space's settings goes back to the page the person came from, or to the workspace home when there was none. Every screen that says a mail was sent (registration, forgotten password, address change, codes by mail) adds that it may be in the spam folder. Done when end-to-end tests cover each.
- [x] Step 69: CI review. `ci.yml` and `security.yml` run on each pull request, then again on the push to `main` that merges it. For each job, decide what the second run catches that the first did not (a `main` that moved since the pull request was tested), and keep only that: branches required to be up to date, a merge queue, or a lighter run on `main`. Done when `docs/notes/ci.md` says what runs on a pull request, on `main`, on a tag and on schedule, and why, with the minutes per merged pull request before and after.
- [x] Step 70: six heading levels. Headings 4 to 6 in the editor (`levels` in `editor/registry.ts`), the slash menu, the Markdown shortcuts and the source under the caret (ADR 0018). Markdown import keeps them instead of folding them into level 3, the renderers and exports write them, and their numbers and the table of contents follow. Their type sizes go into `DESIGN_SYSTEM.md` and `tokens.css`. If the fingerprint of the editor schema changes (ADR 0016), the notes say that tabs of the last release reload. Done when a test takes `#` to `######` through import and export, and an end-to-end test makes a level 6 heading by typing.
- [x] Step 71: code blocks. Tab indents the selected lines of a code block and Shift-Tab outdents them, instead of leaving the editor; keyboard users keep a way out, which the shortcuts screen of step 80 names. While the caret is in a code block, its fence (` ```rust `, or ` ``` ` with no language) shows as a hint, as heading markers do, so people see where the language changes, and a click on it writes the fence out (ADR 0018). Done when end-to-end tests cover Tab, Shift-Tab, the way out, and a language changed from the hint.
- [x] Step 72: callout and toggle. The callout is redrawn: Brut has no such component, so the step proposes one from its tokens and shows it to the maintainer in light and dark before it goes into `DESIGN_SYSTEM.md`, and the styles of revisions, shares and exports follow (`base.css`). A toggle clicked several times in a row disappears: find why, and fix that. Done when the maintainer accepts the callout, and an end-to-end test clicks a toggle's button five times fast and still finds it with its content.
- [x] Step 73: invitations, server. A route for workspace admins finds accounts of the instance by name or address once 3 characters are typed: at most 10, members and disabled accounts left out, rate limited. With a relay, a new invitation mails its link, in the workspace's language (step 62). An account that holds the invited address lists its pending invitations (`GET /me/invitations`), and accepts or declines one by its id, without the link. The events sockets it has open carry an `invitation` event. Done when tests cover the search's permissions and limits, the mail, accepting and declining by id, and the event.
- [ ] Step 74: invitations, screens. The invitation form suggests people after 3 characters, and still takes any address. Pending invitations show in the workspace selector, with join and decline, at load and live from the event. Done when end-to-end tests invite an existing account, which joins from the selector without the link, and a new address, which gets the mail.
- [x] Step 75: loading skeletons. A page that loads shows the shape of its title and lines, not an empty page, and the tree and the search results do the same. It shows only after a short delay, so a fast load does not flash, and has no motion under `prefers-reduced-motion`. Brut has no skeleton either: its style comes from the tokens. Done when an end-to-end test with a slowed route sees the skeleton, then the page.
- [x] Step 76: table column widths. Writers drag a column's edge (`resizable` on in `editor/registry.ts`), and the width is stored in the document as `colwidth`, which the content schema and AST already carry. The HTML renderer writes the widths, so revisions, shares and HTML exports show them, and Markdown leaves them out. Readers cannot drag. Done when an end-to-end test drags a column and the second browser sees the width, and a test finds it in the HTML.
- [ ] Step 77: passkeys, server. An ADR on WebAuthn: what is written by hand with the `ring` already linked (ES256 and RS256, the browser's `getPublicKey()` and `getAuthenticatorData()` so that registration reads no CBOR), the relying party (the host of `CADOCS_PUBLIC_URL`), and whether a passkey with user verification stands for the second factor. A migration, routes to register a passkey (with the current password, or the fresh session of step 67 for an account with none), to list, rename and remove them, and a login by a discoverable credential that shares the limit of password guesses. Done when Rust tests sign with a software authenticator and cover a wrong origin, a wrong or reused challenge, and a removed passkey.
- [ ] Step 78: passkeys, screens. A passkey button on the login screen and the browser's autofill of passkeys, and a section of the account screen to add, name and remove them. Done when an end-to-end test with a virtual authenticator (through Chromium's DevTools protocol, which Helium has) adds a passkey and logs in with it.
- [ ] Step 79: settings in one place. The account screen becomes the person's settings, in sections: profile, login and security (password, second factors, passkeys, providers), preferences (language, theme), notifications, and deleting the account. Each section has its own address, with the list of sections beside it on wide screens and above it on narrow ones. The user menu leads there, and `/account` and the providers' `/account?oidc=` keep working. Done when end-to-end tests reach each section from the menu and by its address, and the existing account tests pass on the new screen.
- [ ] Step 80: keyboard shortcuts. One list in `web/src/lib/` feeds the key handlers and a shortcuts screen, opened with `?` outside text and from the user menu. It shows those that exist (search, the editor's marks and blocks, the slash menu, the code blocks of step 71) and adds a few, proposed to the maintainer first, such as a new page or the sidebar. A shortcut never fires while typing in a field, and shows `⌘` on a Mac. Done when an end-to-end test opens the screen with `?` and checks each new shortcut.
- [x] Step 81: what's new. After an upgrade, the first visit of each browser shows a small dialog of what changed in this release, once. A browser that never saw an earlier release shows nothing, nor does a release with no entries. Each release's entries are written in the catalogue, in English and French. Done when an end-to-end test sees it once after the version changes, and not again once closed.
- [ ] Step 82: V1.2 exit. Benchmark, binary size, a security review of account deletion, the people search, invitations by id and passkeys, `COVERAGE.md` and the permission matrix for the new routes, and a review of what stays left open in `docs/notes/`.

Order: step 67 first, as the most pressing. Steps 68 to 72, 75 and 76 are independent. 73 comes before 74, 77 before 78, and 79 after 67 and 78, so the settings screen is laid out once with every section. 80 comes after 71 and 79, 82 after all. 70, 71, 72 and 76 all change `editor/registry.ts` or `page.css`: rebase each on the last one merged.

Exit criteria:
- [ ] Every step's change has its tests listed in `COVERAGE.md`.
- [ ] No route deletes an account or adds a way to log in on a session alone, without the password or a fresh login.
- [ ] The binary stays under 30 MB, and the RSS thresholds of the MVP still hold.

## V2

Purpose: larger deployments and finer control.

Scope:

- Page-level restrictions (ADR 0009)
- PostgreSQL store (ADR 0002)
- Audit log
- Public API tokens and webhooks
- An MCP server on those tokens, so that an assistant such as Claude reads, searches and writes pages with a person's permissions, where the team sees and edits them (asked after V1.1). cadocs runs no model itself, so AI features stay out of scope
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
