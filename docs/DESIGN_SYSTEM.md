# Design system: Brut

The visual system of cadocs. A self-hosted wiki built for small servers should look like it: the interface shows that it is light instead of saying so.

Tokens live in [`web/src/tokens.css`](../web/src/tokens.css). This document says what they are for.

## Principles

1. **Plain text, on purpose.** The interface (sidebar, bars, menus, metadata) is all monospace. Terminal and Markdown conventions stay visible: trees drawn with `├─ │ └─`, `[x]` and `[ ]` boxes, displayed shortcuts, `/code` commands.
2. **Content stays comfortable.** Page text is IBM Plex Sans 17/1.7 in a 700px column. Inside content, monospace is only for the title, section headings and code.
3. **No decoration.** No rounded corners, gradients, blurred shadows or emoji. 1px rules, flat fills, one accent color. Icons are tools, never ornament: they stand for a verb in a compact action, and nothing else (see [Iconography](#iconography)).
4. **Neutral gray.** Backgrounds are untinted grays. Never warm them towards cream or yellow.

## Colors

Each color has a light and a dark value, except the presence colors, which are shared.

| Token | Light | Dark | Use |
| --- | --- | --- | --- |
| `bg` | `#f5f5f4` | `#0e0e0e` | Page and editing area |
| `panel` | `#ebebea` | `#151515` | Sidebar, code blocks, inline code |
| `ink` | `#141414` | `#e6e6e4` | Main text, button borders, offset shadow, selected row |
| `ink-2` | `#2d2d2c` | `#c8c8c5` | Lede and long secondary text on `bg` |
| `muted` | `#5a5a58` | `#999996` | Metadata, section labels, tree prefixes, shortcuts. Passes 4.5:1 on `bg` and `panel` |
| `line` | `#cdcdca` | `#2b2b2b` | 1px rules and borders. Never text |
| `accent` | `#ff5a1f` | `#ff6b30` | Signal orange, as a flat fill only: logo, marks, active caret. Fails 4.5:1 as text on light `bg` |
| `accent-text` | `#b3390d` | `#ff8a5c` | Orange that carries text: heading numbers, checked boxes, comment counters, slash commands |
| `selection` | `ink` | `ink` | Selected row in trees and menus, text selection |
| `selection-ink` | `bg` | `bg` | Text on `selection` |
| `mark` | `#ffdccb` | `#3f2516` | Commented passage, with `ink` text on top |
| `ok` | `#2e7d32` | `#6ccb6f` | "synced" dot, always next to a label |
| `danger` | `#b3261e` | `#ff7b72` | Final destructive actions (delete forever, delete an account), as text or icon, never as a fill. Passes 4.5:1 on `bg` and `panel` |
| `presence-1`, `presence-1-ink` | `#f5b83d`, `#1a1200` | same | First collaborator, and its text color |
| `presence-2`, `presence-2-ink` | `#f49ac1`, `#2a0a18` | same | Second collaborator |
| `presence-3`, `presence-3-ink` | `#7cc4f2`, `#04202f` | same | Third collaborator |

- `bg`, `panel`, `ink`, `ink-2`, `muted` and `line` make up 95% of the interface.
- The tokens use `light-dark()`, so the system chooses, unless `data-theme` on `<html>` sets `color-scheme` to one of them.
- Selection is a **full inversion**, never a pale tinted background.
- Past three collaborators, the presence colors loop.

## Typography

Two IBM Plex families, embedded in the binary through `@fontsource` (ADR 0004), never loaded from Google Fonts. The bundled faces are in [`web/src/fonts.ts`](../web/src/fonts.ts): Mono 400, 500 and 600, Sans 400, 400 italic, 500 and 600, in the `latin` and `latin-ext` subsets.

Those subsets have no box-drawing characters, so the tree prefixes of the MVP need a subset or a check that the fallback font lines up.

| Role | Style | Family |
| --- | --- | --- |
| Page title | `page-title` 34/1.2, 600, spacing -0.01em | mono |
| Section heading (H2) | `section-title` 19/1.3, 600, after its `section-number` | mono |
| Section number | `section-number` 13/1.3, 500, `accent-text` | mono |
| Content headings H1 to H6 | `heading-1` 24, `heading-2` 19 (the section heading), `heading-3` 15, `heading-4` 14, `heading-5` 13, all 600 at 1.3; `heading-6` 13, 500, `ink-2`. Tokens in `tokens.css` | mono |
| Lede | `lede` 19/1.6, `ink-2` | sans |
| Body | `body` 17/1.7 | sans |
| Interface | `ui` 13/1.4 | mono |
| Metadata | `meta` 12/1.4, `muted` | mono |
| Section and field labels | `label` 11/1.4, lowercase, `muted` | mono |
| Code | `code` 14/1.75 on `panel` | mono |

**No capitals.** Section and field labels (`spaces`, `settings`, `email`) are lowercase like the rest of the interface: their 11px size and `muted` color set them apart. Nothing in the interface uses spaced capitals.

**Numbered headings.** Every heading starts with its number, on the baseline, 14px from the heading. Numbers count from the highest level used on the page: `01` and `02` at the top, `01.1` below, `01.1.1` under that. The top number has two digits. The editor sets them as an attribute, so they are not part of the document.

**Markdown source.** Under the caret of a writer, a heading shows its `##` in `muted`, in place of its number, and a quote paragraph its `>`. Edited as text, the source is set in the block's own type (ADR 0018). A code block shows its language at the top right, in `meta` `muted`, or `auto` when it is guessed.

## Logo

The name followed by the editor's caret: `cadocs` in mono 600, then a flat `accent` block. The logo is the word being typed, so it has no separate symbol.

- **In the interface:** the `.logo` caret of [`web/src/base.css`](../web/src/base.css) after the name, sized in `em` (0.55 wide, 1.2 high), so it follows the text size.
- **As an icon:** [`web/public/favicon.svg`](../web/public/favicon.svg), a block `c` in `ink` and the caret in `accent` on a 32 grid, with its own dark values.
- **Colors:** the name is `ink` and the caret `accent`, never the reverse: `accent` does not carry text.
- **README image:** `docs/assets/promo-light.png` and `promo-dark.png`, 1280 by 640 at 2x, made from a capture of the running application.

## Iconography

One set: **Lucide** 0.460.0 (ISC license, compatible with AGPL-3.0), redrawn with sharp corners.

- **Stroke:** 1.5 on the 24 grid, which renders as 1px at 16px, like the rules. `stroke-linecap: square`, `stroke-linejoin: miter`, `currentColor`.
- **Size:** `icon-size` (16px), always. An icon never grows to catch the eye.
- **Embedded:** inline SVG, only the icons in use, copied from `lucide-static` with the three settings above. No icon font, no CDN, no npm dependency. The ISC notice ships with the other licenses.
- **Where:** compact actions only, where a verb in text would crowd out the content: tree actions, list rows, the top bar, the sidebar footer. Form buttons (`save`, `log in`, `create workspace`) keep their verb in text.
- **Accessibility:** an icon button always has an `aria-label` and a `title` holding its verb, in the same words as everywhere else.
- **Characters stay:** structure remains plain text. `├─ │ └─` for the tree, `▸ ▾` to expand, `/` for a folder, `[x]` for a box. Icons replace verbs only.

| Icon | Verb |
| --- | --- |
| `plus` | new page, new space |
| `ellipsis` | more actions (opens an action menu) |
| `trash-2` | move to trash, open the trash |
| `rotate-ccw` | restore |
| `history` | history (a page's revisions) |
| `x` | close, delete forever (in `danger`) |
| `pencil` | rename |
| `link`, `copy` | copy link, copy |
| `sun`, `moon`, `sun-moon` | light, dark, automatic theme |
| `log-out` | log out |
| `user`, `settings` | account, settings |
| `menu` | open the sidebar on a phone |
| `download`, `upload` | export, import |
| `printer` | print |
| `message-square` | comments (opens the panel) |
| `globe` | share publicly (opens the share dialog) |

## Layout

- Sidebar on the left (`sidebar-width`, `panel` background, `line` rule on its right). Main column on the right: top bar (`topbar-height`, bottom rule), then the editing area centered on `measure`.
- The sidebar stays on screen (`position: sticky`, the window's height): its tree scrolls on its own and its footer stays visible.
- Under about 840px, the sidebar becomes a drawer: closed by default, opened by the `menu` button of the top bar, over the content, with an `ink` rule on its right and `shadow-offset`. The content comes first on screen.
- Spacing only uses the `space-*` tokens, except for the few fixed offsets given in this document. Sidebar rows use `row-height`, controls `control-height`.

| Token | Value | Use |
| --- | --- | --- |
| `space-1` | 4px | Smallest gap, `kbd` padding |
| `space-2` | 8px | Horizontal padding of sidebar rows |
| `space-3` | 12px | Padding of the sidebar and menu items |
| `space-4` | 16px | Padding of code blocks |
| `space-6` | 24px | Between sidebar groups |
| `space-8` | 32px | Side gutter of the editing area |
| `space-10` | 40px | Above a section heading |
| `space-14` | 56px | Top margin of the edited page |
| `radius-none` | 0 | Everywhere: buttons, fields, avatars and menus are square |
| `shadow-offset` | `5px 5px 0` in `ink` | The only shadow: offset, solid, no blur. Floating surfaces only |
| `measure` | 700px | Content column, about 70 characters |
| `sidebar-width` | 264px | Base sidebar width |
| `topbar-height` | 56px | Top bar |
| `row-height` | 30px | Sidebar and tree rows |
| `control-height` | 34px | Buttons and menu items |
| `icon-size` | 16px | Every icon |

## Motion

Motion says where something went, and nothing more. It is short, linear and plain.

- One token, `motion-duration` (120ms). No other duration, no delay.
- Linear only: no ease, no spring, no bounce, no overshoot. Opacity and height only: nothing slides in from the side, scales or turns.
- Under `prefers-reduced-motion: reduce`, `motion-duration` is 0 and nothing moves: things show and go at once.
- What moves, and how:

| What | In | Out |
| --- | --- | --- |
| A tree level (a page's children, a space's root) | Height, from 0 | Height, to 0 |
| An action menu | Opacity | Opacity |
| The notice | Opacity | Opacity |
| The confirm buttons | Opacity, each branch | None: the branch it replaces goes at once, so both never show side by side |

- Only a level that opens or closes moves, once its rows are there, so it unfolds to their height. The ancestors of the page on screen unfold too when it opens them. A level already loaded when the tree shows, and the levels inside one being closed, do not move.
- Everything else stays still: pages, rows, hover, selection and focus change at once.

## Components

The icons, the action menu, the drawer and the lowercase labels come from ADR 0017. A component not used yet is the reference for the step that needs it.

### Page content

- `measure` column, centered, `space-14` top margin.
- Metadata in `meta`, title in `page-title`, lede in `lede`.
- H1 and H2 with a `space-10` top margin, H3 to H6 with `space-6` above and `space-2` below.
- Checklist: `[x]` in `accent-text` with a struck, `muted` label when done, `[ ]` in `muted` otherwise.
- Inline code and blocks on `panel`. Blocks have a `line` border, and the `$` prompt is `muted`.
- Code highlighting adds no color: comments in `muted`, keywords, types and numbers in `accent-text`, strings in `ok`, titles at 600.
- A page whose content this version cannot show gets a banner above the title: 1px `ink` border, mono 13px, and a `reload` button.
- Callout: `panel` fill, 1px `line` border and a 3px rule on the left in the color of its kind: `ink` for `note`, `accent` for `warning`, `danger` for `danger`. Its kind shows at the top in `label` style, in `muted`, `accent-text` or `danger`, as a list writers can change.
- Toggle: `▸` closed, `▾` open, in `muted`, before the summary line, as in the tree. Each reader opens it for themself.
- Table: 1px `line` rules, the header row on `panel` in mono 14px 600, cells in body text. For writers, hovering it shows a `plus` bar on its bottom edge (`add row`) and its right edge (`add column`), `panel` with a `line` border and an `ink` one under the pointer. A right click on a cell opens the action menu at the pointer: the four `add` verbs with `plus`, then below a rule `delete row`, `delete column` and `delete table` with `x`, the last in `danger`.
- Link to a page: the page's title in `accent-text`, underlined by a dotted rule so it reads apart from a web link. A page the reader cannot open shows `page not found`, struck, in `muted`.
- Table of contents: 1px `line` frame, a `contents` label, then each heading with its number in `accent-text`, indented by `space-4` per level, in `ui` mono.

### Presence

- Avatar: 26px square, mono 12px 600 initial, `presence-n` background, `presence-n-ink` text, 3px between avatars, a `title` with the name.
- Remote caret: 2px vertical line in `presence-n`, with a label stuck above it (mono 11px, the name as its owner typed it, padding `0 6px`).
- Colors follow order of arrival and loop after three. There is no central order, so each client takes the color the others use least when it joins.

### Button

- Three square variants in mono 13px, `control-height` tall, with lowercase labels: `share`, `history`, `new page`.
- Primary: 1px `ink` border, `ink` text, weight 500, 14px horizontal padding.
- Secondary: no border, `muted` text, 6px horizontal padding.
- Icon: a `control-height` square (`row-height` in the tree), an `icon-size` icon in `muted`, `ink` on hover, with a required `aria-label` and `title`. Compact actions only: a form keeps a primary button with its verb.
- A final action is in `danger`, as text or icon, after a confirmation that repeats the verb (`delete forever`).
- Never an `accent` background.
- Focus: 2px `accent` outline, offset by 2px.

### Field

- `label` above a `control-height` input in mono 13px, with a `line` border on `bg` and `space-2` horizontal padding.
- A refusal shows below it in `accent-text`, mono 12px, and turns the border `accent-text`.
- Focus as for buttons.

### Checkbox

A 14px square with a 1px `ink` border on `bg`, filled with `ink` inside a 2px `bg` ring when on, before its label in mono 13px, the row `control-height` tall. The whole row is the label. Focus as for buttons. In forced colors, the system's own box, since the fill would not show.

### Dialog

A modal surface for a task that needs the page to wait, such as the share dialog.

- The command menu's surface: `bg`, 1px `ink` border, `shadow-offset`, centered, at most 560px wide and the window's width less `space-4` on each side.
- Behind it, the page under a 70% wash of `bg`, and inert. The notice sits behind too, so the dialog says what its actions did in its own `status` line at its foot.
- A head with its title in `label` style and an `x` icon button (`close`), above a `line` rule. Escape closes it too, and focus goes back to what opened it.
- Inside, the usual pieces: `meta` text, bordered blocks in `line` for listed items, fields and a primary button for its form below a `line` rule.

### Tree item

- A sidebar row for a space or a page, `row-height` tall, `space-2` horizontal padding.
- Prefixes in `muted` with `white-space: pre`: `├─ ` for a child, `└─ ` for the last one, `│  ` for each open parent level.
- Folders end with `/`, as in `runbooks/`.
- The label takes all the free width and ends with `…` when it overflows.
- `▸` and `▾` expand and collapse, as characters, not icons.
- Actions on hover or focus: two `row-height` icon buttons on the right, `plus` (new page below) and `ellipsis` (action menu: rename, copy link, move to trash). Without hover or focus they are removed (`display: none`) and reserve no width. On a touch screen, `ellipsis` stays visible.
- Selected: full inversion, label at 500, prefix and icons inverted too.
- Spaces use the same component with no prefix and no number. Their `ellipsis` menu leads to the trash and the space settings.
- The page on screen is always visible: its ancestors open when it loads.

### Workspace selector

A 22px `accent` square, the name at 600, and the current workspace in `muted` followed by `▾`. It opens in place, as rows below it: only the command menu floats. A last `new workspace` row, in `muted`, shows to who may create one, and opens the name form in place of itself.

### Search field

A button with a `line` border on `bg`, `search…` in `muted`, and a bordered `Ctrl K` key.

### Breadcrumb

Lowercase segments separated by `/`, with the current segment in `ink` at 500. Parent segments are links. Screens outside a workspace (account, instance administration) show no workspace segment.

### Command menu

The floating surface of the slash menu and the `Ctrl K` palette.

- `bg` surface, 1px `ink` border, `shadow-offset`. No other surface has a shadow.
- Header in `label` style, as in `commands · 4`, with a `line` rule below.
- Items at `control-height`, command on the left, description or shortcut on the right in `muted`.
- Active item: full inversion, description in `selection-ink`.
- The palette is the same surface, 560px wide, with an input at the top.

### Action menu

The menu the `ellipsis` button opens, on a tree row, a list row or a page's top bar.

- The command menu's surface: `bg`, 1px `ink` border, `shadow-offset`, anchored below the button and aligned to its right.
- One action per `control-height` row: a `muted` icon on the left, the verb in text (`rename`, `copy link`, `move to trash`). Here the text stays, and the icon helps the eye.
- Destructive actions last, below a `line` rule. A final one is in `danger` and asks for confirmation.
- Active row (hover or keyboard) in full inversion. Up and down arrows move, Enter picks, Escape closes and gives focus back to the button.

### Notice

The short message after an action, such as `moved to trash` or `link copied`, at the bottom left of the window.

- `bg` surface, 1px `ink` border, no shadow, mono 13px, inside a `status` region that exists only while it shows.
- At most one action, a secondary button (`undo`). When it has one, the notice takes focus, so a keyboard reaches it without hunting.
- It goes after 8 seconds, unless the pointer or focus is on it.

### Comments

Passage highlighted in `mark`, with a `[2]` counter in `accent-text` in the right margin.

- The highlight and its counter are decorations, never part of the page's text. The thread picked shows a 2px `accent` rule under its passage. A resolved thread has no highlight, unless it is picked from its quote; a thread picked when someone resolves it is unpicked.
- The top bar's `message-square` button opens the panel, with the count of open threads as `[2]` in `accent-text`, and in its `title` (`comments · 2 open`) for screen readers.
- The panel sits right of the page, 320px wide, `bg` with a `line` rule on its left, sticky like the sidebar. Without room for both (under about 1180px) it covers the right of the page, with an `ink` rule.
- In it, in mono: the `label` heading, `open · 2` and `resolved · 1` as secondary buttons (the shown one in `ink`), the composer's button (`comment on the selection` or `comment on the page`), then one bordered block per thread, `ink` border for the one picked.
- A thread starts with its quote after a 3px `accent` rule, a button that scrolls to the passage. A passage no longer in the page keeps its quote struck in `muted`, with a `line` rule and `passage removed`. A comment on the whole page says `on the page`.
- Comments: the author at 600, the date in `meta`, the body in sans 15px. Replies sit under a 1px `line` rule on their left. Text verbs below (`reply`, `resolve`, `reopen`); `edit` and `delete` in the `ellipsis` menu of one's own comments.

### Share dialog

The dialog `share publicly` opens from the page's action menu, for editors and admins only.

- One bordered block per public link, oldest first: the address in mono with a `copy` icon button, then its facts in `meta` joined by ` · ` (`with subpages` or `this page only`, `indexed by search engines` or `hidden from search engines`, `no expiry`, `until <date>` or `expired` in `accent-text`, the address then struck in `muted`), and `revoke` in `danger`, confirmed by `revoke the link`.
- Below a `line` rule, `new link`: two checkboxes (`include subpages`, `let search engines index it`), a date field `valid until (optional)` that offers no day before today, and `create link`. A link valid until a day works through that day.

## Tone

Lowercase in the interface (`share`, `history`, `new page`), short phrases, plain terminal vocabulary (`synced · 3 online`). An action keeps one verb everywhere: button, tooltip, menu and message (`move to trash`, then `moved to trash`). Page content keeps normal capitalization.

## Do not

- Animate anything outside [Motion](#motion), or ease, delay or bounce what is in it.
- Round a corner, or add a blurred shadow or a gradient.
- Use `accent` for text on a light background.
- Tint the grays towards yellow or cream.
- Set body text in monospace.
- Put an icon next to a heading, a label or a navigation link, as ornament.
- Ship an icon button without an `aria-label` and a `title`.
- Mix icon sets, or keep Lucide's round caps and joins.
- Write a label in capitals.
