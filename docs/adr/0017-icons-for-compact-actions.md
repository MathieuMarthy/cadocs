# ADR 0017: Icons for compact actions, lowercase labels

Status: accepted

## Context

Brut banned icons wherever a character would do, and wrote every action as a lowercase verb. A review of the step 12 interface showed the cost:

- Tree rows carry `+`, `rm` and `trash` as text. Hidden with `opacity: 0`, they still take their width, so `engineering handbook` shows as `engineering h...` in a sidebar with room to spare.
- Verbs drift: `rm` on a page, `trash` on a space, `trashed` and `purge` in the trash.
- Admin and trash rows line up four text buttons each, which reads as a wall of grey words.
- Section labels in spaced capitals (`SPACES`, `EMAIL`) are the only uppercase in an otherwise lowercase interface.

## Decision

- Compact actions (tree rows, list rows, top bar, sidebar footer) use icon buttons, each with an `aria-label` and a `title` holding its verb. Secondary actions go into an `ellipsis` action menu, where the verb shows in text next to its icon.
- Form buttons keep their verb in text. Structure keeps its characters: `├─`, `▸ ▾`, `/`, `[x]`.
- One set, Lucide 0.460.0 (ISC), with a 1.5 stroke, square caps and miter joins to match the 1px rules and square corners. Only the icons in use are copied as inline SVG: no icon font, no CDN, no npm dependency, a few hundred bytes each in the binary and nothing on the server's heap.
- Labels are lowercase. A `danger` color marks final destructive actions.

[DESIGN_SYSTEM.md](../DESIGN_SYSTEM.md#iconography) holds the rules and the icon list. The Brut artifact carries the same change.

## Consequences

- Steps 12a to 12c bring the interface in line, before step 14, so the editor's toolbar and menus start from these components.
- An icon-only button is harder to read than a word. The `title` and the menu's visible verbs make up for it, and forms keep their words.
- Adding an icon means taking it from `lucide-static` at the same version and applying the three stroke settings.
