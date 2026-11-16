# Web

What a change to the front end or its tests must know.

## Shell

- Svelte 5 with runes (ADR 0004). `App.svelte` holds every route in `resolve`, which also redirects: setup while no account exists, login with `?next=` without a session, back to `safeNext(next)` once there is one. A screen logs in by calling `signIn`, never by navigating. A logged-in person is sent home from `/login`, `/register` and `/forgot-password`.
- `lib/router.svelte.ts` catches clicks on same-site links. The server answers `index.html` for any path that is not a file (`assets.rs` lists the first segments that are not routes: add `s` with public shares).
- `lib/api.ts` keeps the CSRF token and sends every `unauthenticated` answer to `sessionEnded`. Only a real `POST /auth/logout` or a failed `refreshSession` drops the session: the server decides, not the screen.
- `lib/form.svelte.ts` holds a form's busy state and refusals. A `Form.run` inside an effect goes through `untrack`, since it reads `busy`.
- `Solo.svelte` for screens without a sidebar, `Shell.svelte` for the others. `Source.svelte` (source link, ADR 0010) must stay on every screen; the shell passes `withTheme={false}` since its user menu has the theme.
- The theme is `data-theme` on `<html>` over `light-dark()`, set before the first paint by `public/theme.js`: a file, since the CSP refuses inline scripts.
- Names users typed keep their case; only interface words are lowercase.
- Sidebar: sticky, only `.sidebar-scroll` scrolls, its `scrollTop` kept in a module variable since each screen mounts its own shell. Under 840px it is a drawer with `display: none` until `drawer-open`, so a test sees no `complementary` while it is closed.
- `.icon-button` sets `display` late in `base.css`, so a rule that hides one needs two classes (`.icon-button.drawer-button`).
- Logo: `.logo` caret is `aria-hidden`, so a test reads `cadocs`. `public/favicon.svg` carries its own colors. The README images in `docs/assets/` are captures of the real binary at 1280 by 640, 2x, both themes, seeded through `e2e/server.ts`: retake them when the shell changes visibly.
- Type checks: `tsc` skips `.svelte` files, and `svelte-check` needs TypeScript 5 or 6. Run it by hand after changing a component, from `web/`: `npx -y -p typescript@6 -p svelte-check@4.7.8 -p svelte@5.57.3 svelte-check --workspace . --tsconfig ./tsconfig.json`.

## Navigation

- `lib/nav.svelte.ts` holds a workspace's spaces, tree levels and events socket. `acquireNav` shares one `Nav` between screens and keeps the socket 2 seconds after the last leaves; a screen that calls it is keyed by workspace in `App.svelte`.
- A node is open exactly when its level is in `levels`, so events only refetch what shows. Templates read `loaded`, never `level`, which would write state while rendering. A page event refetches every loaded level of its space, 50 ms after the first of a burst, and bumps `versions` and `version`.
- Spaces load over REST at start, without waiting for a socket a proxy may block. Opening the socket and `resync` reload everything.
- Socket closes: `4401` ends the session, `4403` and `4404` refresh it, anything else retries with backoff, refreshing the session every third failure.
- Tree drags carry `application/x-cadocs-page`. A drop on the top quarter of a row goes before it, the bottom quarter after, in between under it, on a space at the end of its root.
- Tree actions are `display: none` until the row is hovered or focused, so tests point at the row first (`press` in `navigation.spec.ts`) or use `includeHidden`. An open menu keeps them through `:has([aria-expanded="true"])`, or moving focus into the menu would remove it.
- `lib/actions.ts` holds what the tree and the page's top bar share (`copy`, `pageLink`, `trashPage` with its `undo`). `trashPage` leaves a page shown under the trashed one for its parent, or `/w/<slug>`; the page view tells it what shows through `showPage([...ancestors, id])`. `undo` goes back, bumping `reopened` (part of the page view's key) to mount it again.
- Left open: no keyboard way to move a page; no `rename` in the tree menu (`PATCH /pages/{id}` takes a title); trashing an ancestor while on the history route leaves that history on screen; `open the parent page` can land on a trashed page when an ancestor further up was trashed.

## Page view

- `routes/Page.svelte` and `editor/collab.ts` load on demand, so a login does not download TipTap. The page reads `GET /pages/{id}` before opening the socket, and again every 3 failed connections, since a socket cannot read why an upgrade failed. Only `401` and `404` stop it. Another REST failure still shows the editor, in a shell with no workspace, and reads again every 2 seconds.
- `ancestors` from that read fill the breadcrumb and expand the tree once per mount (`revealed`). While the first read is pending, the shell shows `lastWorkspace`, so a click in the tree does not flash. A re-read on a change to its space fires only for a version not yet seen (`seen`), or a revoked page showed `page not found` instead of `access revoked`.
- A page opened by its address has its space open: a test must not click the space after it, which would close it.
- `disableBc` is on, so two tabs of one browser really go through the server. Awareness holds `user: { id, name, slot }`, `slot` 1 to 3 picking the presence color. Avatars show from two people.
- The title is a `textarea` bound to `Y.Text` (`editor/title.ts`), grown to its text and kept on one line. Tests read it with `getByLabel("page title")`.
- A page whose access ends while it reconnects after a `4409` shows `page not found`, as REST answers `404`. Tests that revoke after a role change wait for `synced … read only` first.
- A screen's `status` role is not unique (the tree has one while loading): tests look in `banner` or `main`. The notice's `status` exists only while there is a notice, mounted once in `App.svelte`.
- Left open: presence counts awareness clients, so one person in two tabs shows two avatars; edits typed between a demotion and its `4409` stay in that tab until a reload.

## Editor

- `editor/registry.ts` is the only list of what the editor knows: each entry brings its extensions and slash commands. StarterKit stays as the core with every registry block turned off: add a block as an entry, never through StarterKit. Node names are TipTap's defaults. A new block needs `npm run schema`, or `npm run build` fails ([content notes](content.md)); the registry must stay importable under Node, so no DOM and no `$state` at module level.
- A new block extension with Markdown input rules goes through `quietInSource()`, or its shortcuts fire inside written-out sources.
- `editor/menu.ts` is the command menu, used by the slash menu (`slash.ts`) and the `[[` picker, drawn in plain DOM, focus left in the text: a `listbox` (`commands`, or `pages` for the `[[` picker) with `option`s. Off in code blocks and for readers. A slash opens it only at the start of a block or after a space; a command can take `when`. The command removes the typed query in its own transaction first: `prosemirror-tables` commands in one chain read positions from before the chain.
- The `[[` picker reads up to 1000 recent pages and filters titles in the browser, kept 10 seconds.
- Read-only follows the role of each `GET /pages/{id}` answer. `4409` makes the editor read-only at once, then reads again. `enableContentCheck` is on: a `contentError` destroys the provider and shows the `reload` banner (`editor.spec.ts` writes an unknown `futureBlock` to test it).
- Markdown source (ADR 0018, `editor/source.ts`): a block's source is written into it when the caret reaches its marker, read back when it leaves. A remote change can rebuild the document, so the block is found again from the caret. A block not written out is read on leaving only if this tab changed it (`touched`) and it is not code. A hint is not drawn on a block whose text already reads as its source. `syncCaret` reads the selection before every key.
- Heading numbers and the code language are decoration attributes (`data-number`, `data-language`) shown by `::before`. The language changes only through the fence line.
- Blocks of our own: `callout` (`kind`), `details` (`persist` off, each tab opens it for itself), tables with `resizable` off, `pageLink` (inline atom with `id`; the title is read over REST and cached per open page, so a rename shows on the next page opened), `tableOfContents` (atom, drawn from the headings), `plugin` (ADR 0016, shows its `text` dashed), `image` and `file` (`attachments.ts`).
- Table controls (`editor/table.ts`): two `+` buttons in `.table-frame`, shown only on hover in an editable editor, so tests hover first; a right click opens a `table` menu with the classes of `Menu.svelte`.
- Uploads go one at a time (`uploads.ts`). A paste test builds its `DataTransfer` in the page (`attachments.spec.ts`). A file block shows its size from a `HEAD` on the download, not stored in the document.
- StarterKit's `TrailingNode` adds a paragraph in every tab when a page ends on another block, readers too, so two writers can add two. Not fixed: without it, nothing can be typed below a final block. Tests find it with `:scope > p`.
- Left open: links to pages cannot be dragged from the tree.

## Other screens

- Icons: `components/icons.ts` holds the Lucide shapes of the design system, drawn by `Icon.svelte` inside one `svg`, and `iconMarkup` for plain DOM. Add one by copying its shapes from `lucide-static` 0.460.0, never its attributes. `IconButton` takes the verb as `title` and the full label as `aria-label`.
- `Menu` is a `popover="manual"` in the top layer, placed from its button and closed by a press outside, a scroll or a resize. Opened by keyboard it focuses its first item, by pointer the menu itself.
- `Confirm` takes `icon` and `danger`. The notice focuses its `undo` and restarts its timer.
- Account: two forms named by their headings (`profile`, `email and password`), reached with `getByRole("form", { name })`; the email sits with the password, since the server asks the current password for it. Tests log out through the user menu (`logOut` in `shell.spec.ts`).
- Settings: sections are `section`s named by their `h2`, reached with `getByRole("region", { name })` (a name matches by substring unless `exact`). Row role pickers are plain `select.select` with an `aria-label`, acting on change. A grant change `PUT`s every other grant as it was, groups included. Nobody changes their own workspace role: they `leave`. `access.changed` refreshes the session, whose workspaces carry the role.
- Search (`routes/Search.svelte`): the address holds the query, so a reload or link shows the same results. Excerpts render as text with `<mark>`, never `{@html}`. `Ctrl K` goes there. Left open: the design system's command palette.
- History (`routes/History.svelte`, `/?page=<id>&history`): the server's escaped HTML shows with `{@html}` inside `.tiptap.revision-content`; `base.css` covers the task list and callout markup that differ from TipTap's.
- The print stylesheet is at the end of `base.css`.
- Motion (`lib/motion.ts`): `unfold` and `appear` read `--motion-duration` at each run; it is `0ms` under `prefers-reduced-motion`, and Svelte then animates nothing. A tree level's `{#if}` waits for `loaded`, or `slide` measured 0. During an outro the element stays in the page, `inert`. A test that scrolls the tree waits for `document.getAnimations()` to be empty.
- Mail screens show only when `mailFeatures()` (`GET /auth/setup`, read when a screen mounts) says so; with no relay their paths show `not found`. Forgot password and registration show the same notice for every address. Login keeps the password in memory for the second factor's code form. Account compares the address `PATCH /me` answers with the one typed to know whether a code was mailed.
- Left open: a `401` on the second factor's code form shows `wrong email or password` and keeps the code form.

## Browser tests

- `web/e2e/` runs the real binary on its own port and data folder (`server.ts`; `CADOCS_BIN` picks another binary, and `../target/debug/cadocs` reads `web/dist` at run time for a quick loop). `TestServer` runs first-run setup, `page(name)` creates a page per name, and `call`, `invite`, `addMember` do what a test does not go through the interface for.
- Playwright reads `CHROMIUM_PATH`. Helium in a new process can stall every request 5 seconds or more, then load the page twice: `expect.timeout` is 30 seconds.
- `compat.test.ts` uses `node:test` with the real `yjs` and `y-websocket` (Node 22.18 or later). Node's `WebSocket` sends the cookie through `webSocketClass()`.
- `e2e/editing.ts` holds the two-browser fixture and editor helpers. A paragraph's text holds the label of a remote caret: read it with `bodyLines` or `expectText`. A task's text is in `ul[data-type="taskList"] > li > div`. `newLine` uses the keyboard only. A code block ends on a third Enter.
- Open a second tab of a logged-in browser with `browser.newContext()` and `logIn`.
- A rename reaches `pages.title` at the next flush, about 540 ms later: a test that renames over REST and then reads the tree, recent pages or the picker calls `server.titleStored(id, title)` first.
- An open client resends its whole document when it reconnects, so a test about the disk has no client left when the server comes back.
- Do not `goto("about:blank")` before another `goto`: the second navigation can abort.
- `e2e/relay.ts` is a plain text SMTP relay in Node; `relay.message(n)` waits for the n-th. Codes match `/^ {4}(\d{6})$/m`.
- `e2e/revisions.spec.ts` overrides the `server` fixture with a 1-second revision pause.
- `screens.spec.ts` saves a screenshot of each screen in both themes in `test-results`.
- Known flaky, with Helium, not regressions: `a source written out in one browser is left alone by a caret passing in the other` (`editor.spec.ts`, about one run in four), `a page moves to trash from its menu by keyboard alone…` (`navigation.spec.ts`, 3 to 5 runs in 10). Seen failing once, then passing alone: `two browsers see each other's tree changes`, `a source being edited survives the other browser's typing`, `a link to a page the reader cannot open does not show its title`, `a link to a page is picked by its title…`, `an admin invites a user…`, `a read older than a demotion…` (if it comes back, check first that the held read is the one the rename triggers).
