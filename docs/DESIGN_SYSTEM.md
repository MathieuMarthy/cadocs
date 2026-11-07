# Design system: Brut

The visual system of cadocs. A self-hosted wiki built for small servers should look like it: the interface shows that it is light instead of saying so.

Tokens live in [`web/src/tokens.css`](../web/src/tokens.css). This document says what they are for.

## Principles

1. **Plain text, on purpose.** The interface (sidebar, bars, menus, metadata) is all monospace. Terminal and Markdown conventions stay visible: trees drawn with `├─ │ └─`, `[x]` and `[ ]` boxes, displayed shortcuts, `/code` commands.
2. **Content stays comfortable.** Page text is IBM Plex Sans 17/1.7 in a 700px column. Inside content, monospace is only for the title, section headings and code.
3. **No decoration.** No rounded corners, gradients, blurred shadows, decorative icons or emoji. 1px rules, flat fills, one accent color.
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
| Lede | `lede` 19/1.6, `ink-2` | sans |
| Body | `body` 17/1.7 | sans |
| Interface | `ui` 13/1.4 | mono |
| Metadata | `meta` 12/1.4, `muted` | mono |
| Section labels | `label` 11/1.4, capitals, spacing 0.1em | mono |
| Code | `code` 14/1.75 on `panel` | mono |

**Numbered headings.** Every H2 starts with its two-digit number (`01`, `02`), on the baseline, 14px from the heading. There is no visible `##`. A CSS counter draws the number, so it is not part of the document.

## Layout

- Sidebar on the left (`sidebar-width`, `panel` background, `line` rule on its right). Main column on the right: top bar (`topbar-height`, bottom rule), then the editing area centered on `measure`.
- Under about 840px, the sidebar moves above the content (flex-wrap), never to a fixed position.
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

## Components

The shell uses the buttons, fields, breadcrumb and workspace selector. The others are the reference for the next steps.

### Page content

- `measure` column, centered, `space-14` top margin.
- Metadata in `meta`, title in `page-title`, lede in `lede`.
- H2 as described above, with a `space-10` top margin.
- Checklist: `[x]` in `accent-text` with a struck, `muted` label when done, `[ ]` in `muted` otherwise.
- Inline code and blocks on `panel`. Blocks have a `line` border, and the `$` prompt is `muted`.

### Presence

- Avatar: 26px square, mono 12px 600 initial, `presence-n` background, `presence-n-ink` text, 3px between avatars, a `title` with the name.
- Remote caret: 2px vertical line in `presence-n`, with a label stuck above it (mono 11px, lowercase name, padding `0 6px`).
- Colors follow order of arrival and loop after three. There is no central order, so each client takes the color the others use least when it joins.

### Button

- Two square variants in mono 13px, `control-height` tall, with lowercase labels: `share`, `history`, `+ new page`.
- Primary: 1px `ink` border, `ink` text, weight 500, 14px horizontal padding.
- Secondary: no border, `muted` text, 6px horizontal padding.
- Never an `accent` background.
- Focus: 2px `accent` outline, offset by 2px.

### Field

- `label` above a `control-height` input in mono 13px, with a `line` border on `bg` and `space-2` horizontal padding.
- A refusal shows below it in `accent-text`, mono 12px, and turns the border `accent-text`.
- Focus as for buttons.

### Tree item

- A sidebar row for a space or a page, `row-height` tall, `space-2` horizontal padding.
- Prefixes in `muted` with `white-space: pre`: `├─ ` for a child, `└─ ` for the last one, `│  ` for each open parent level.
- Folders end with `/`, as in `runbooks/`.
- Selected: full inversion, label at 500, prefix inverted too.
- Spaces use the same component, with a number (`01`, `02`) in place of the prefix.

### Workspace selector

A 22px `accent` square, the name at 600, and the current workspace in `muted` followed by `▾`. It opens in place, as rows below it: only the command menu floats.

### Search field

A button with a `line` border on `bg`, `search…` in `muted`, and a bordered `Ctrl K` key.

### Breadcrumb

Lowercase segments separated by `/`, with the current segment in `ink` at 500.

### Command menu

The floating surface of the slash menu and the `Ctrl K` palette.

- `bg` surface, 1px `ink` border, `shadow-offset`. No other surface has a shadow.
- Header in `label` style, as in `COMMANDS · 4`, with a `line` rule below.
- Items at `control-height`, command on the left, description or shortcut on the right in `muted`.
- Active item: full inversion, description in `selection-ink`.
- The palette is the same surface, 560px wide, with an input at the top.

### Comments

Passage highlighted in `mark`, with a `[2]` counter in `accent-text` in the right margin.

## Tone

Lowercase in the interface (`share`, `history`, `+ new page`), short phrases, plain terminal vocabulary (`synced · 3 online`). Page content keeps normal capitalization.

## Do not

- Round a corner, or add a blurred shadow or a gradient.
- Use `accent` for text on a light background.
- Tint the grays towards yellow or cream.
- Set body text in monospace.
- Add an icon where a character does the job (`▾`, `/`, `+`, `[x]`).
