<script lang="ts" module>
  import type { PageRow } from "../lib/nav.svelte";

  export type Zone = "before" | "inside" | "after";

  /** The type a drag of the tree carries, so a file or text dragged in is never taken for it. */
  export const DRAG_TYPE = "application/x-cadocs-page";

  export const fromTree = (event: DragEvent) =>
    event.dataTransfer?.types.includes(DRAG_TYPE) ?? false;

  /** The page being dragged, and the row the pointer is over. */
  export interface Drag {
    page: PageRow | null;
    /** The dragged page's item, which holds its subtree. */
    element: HTMLElement | null;
    over: string | null;
    zone: Zone | null;
  }
</script>

<script lang="ts">
  import IconButton from "./IconButton.svelte";
  import Menu, { type MenuItem } from "./Menu.svelte";
  import Self from "./TreeLevel.svelte";
  import { unfold } from "../lib/motion";
  import { canEdit, type Level, type Nav, type Place } from "../lib/nav.svelte";
  import { copy, pageLink } from "../lib/actions";
  import { shownPage } from "../lib/router.svelte";

  let {
    nav,
    level,
    lines,
    drag,
    oncreate,
    onmove,
    ontrash,
  }: {
    nav: Nav;
    level: Level;
    /** What the levels above draw in front of each row of this one. */
    lines: string;
    drag: Drag;
    oncreate: (space: string, parent: string) => void;
    onmove: (page: PageRow, place: Place) => void;
    ontrash: (page: PageRow) => void;
  } = $props();

  const name = (page: PageRow) => page.title || "untitled";

  /** The row of the page on screen scrolls into the tree when it appears. */
  const reveal = (row: HTMLElement) => row.scrollIntoView({ block: "nearest" });

  /** A row that appeared in an unfolding level was scrolled to while short of its place. */
  const unfolded = (event: Event) => {
    const row = (event.currentTarget as HTMLElement).querySelector<HTMLElement>(".tree-row.selected");
    if (row) reveal(row);
  };

  /** The page the address shows, which its row marks. */
  const shown = $derived(shownPage());

  function actions(page: PageRow): MenuItem[] {
    const items: MenuItem[] = [{ icon: "link", label: "copy link", select: () => void copy(pageLink(page.id)) }];
    if (canEdit(page.role)) {
      items.push({
        icon: "trash-2",
        label: "move to trash",
        destructive: true,
        select: () => ontrash(page),
      });
    }
    return items;
  }

  function zoneOf(event: DragEvent): Zone {
    const box = (event.currentTarget as HTMLElement).getBoundingClientRect();
    const ratio = (event.clientY - box.top) / box.height;
    return ratio < 0.25 ? "before" : ratio > 0.75 ? "after" : "inside";
  }

  /** Where the dragged page goes when dropped on `target` in `zone`, or null if nowhere. */
  function placeFor(target: PageRow, index: number, zone: Zone): Place | null {
    const dragged = drag.page;
    if (!dragged || !canEdit(target.role) || target.id === dragged.id) return null;
    const row = document.getElementById(`tree-${target.id}`);
    // A page cannot go under itself, which the server would refuse anyway.
    if (row && drag.element?.contains(row)) return null;
    const space_id = target.space_id;
    if (zone === "inside") return { space_id, parent_id: target.id };
    if (zone === "after") return { space_id, parent_id: target.parent_id, after: target.id };
    const previous = level.items[index - 1] ?? null;
    // Already right before it: nothing would change.
    if (previous?.id === dragged.id) return null;
    return { space_id, parent_id: target.parent_id, after: previous?.id ?? null };
  }

  function over(event: DragEvent, target: PageRow, index: number) {
    if (!fromTree(event)) return;
    const zone = zoneOf(event);
    if (!placeFor(target, index, zone)) {
      if (drag.over === target.id) drag.over = null;
      return;
    }
    event.preventDefault();
    drag.over = target.id;
    drag.zone = zone;
  }

  function drop(event: DragEvent, target: PageRow, index: number) {
    const dragged = drag.page;
    const place = fromTree(event) ? placeFor(target, index, zoneOf(event)) : null;
    drag.over = null;
    if (!dragged || !place) return;
    event.preventDefault();
    onmove(dragged, place);
  }

  function start(event: DragEvent, page: PageRow) {
    drag.page = page;
    drag.element = (event.currentTarget as HTMLElement).closest("li");
    event.dataTransfer?.setData(DRAG_TYPE, page.id);
    event.dataTransfer?.setData("text/plain", page.title);
    if (event.dataTransfer) event.dataTransfer.effectAllowed = "move";
  }

  function end() {
    drag.page = null;
    drag.element = null;
    drag.over = null;
  }
</script>

<!-- Local: only the level that opens or closes moves, not the ones it holds. -->
<ul class="rows" transition:unfold onintroend={unfolded}>
  {#each level.items as page, index (page.id)}
    {@const last = index === level.items.length - 1 && !level.more}
    {@const open = page.has_children && nav.open.has(page.id)}
    {@const children = nav.loaded(page.space_id, page.id)}
    <li>
      <div
        id="tree-{page.id}"
        class="tree-row"
        class:selected={shown === page.id}
        role="group"
        data-drop={drag.over === page.id ? drag.zone : undefined}
        ondragover={(event) => over(event, page, index)}
        ondragleave={() => {
          if (drag.over === page.id) drag.over = null;
        }}
        ondrop={(event) => drop(event, page, index)}
        {@attach shown === page.id ? reveal : null}
      >
        <span class="tree-prefix">{lines}{last ? "└─ " : "├─ "}</span>
        {#if page.has_children}
          <button
            class="tree-toggle"
            type="button"
            aria-expanded={open}
            aria-label="{open ? 'collapse' : 'expand'} {name(page)}"
            onclick={() => nav.toggle(page.id, page.space_id)}>{open ? "▾" : "▸"}</button
          >
        {/if}
        <a
          class="tree-label"
          href="/?page={page.id}"
          aria-current={shown === page.id ? "page" : undefined}
          draggable={canEdit(page.role)}
          ondragstart={(event) => start(event, page)}
          ondragend={end}>{name(page)}{page.has_children ? "/" : ""}</a
        >
        <span class="tree-actions">
          {#if canEdit(page.role)}
            <IconButton
              class="tree-plus"
              icon="plus"
              verb="new page"
              label="new page in {name(page)}"
              onclick={() => oncreate(page.space_id, page.id)}
            />
          {/if}
          <Menu label="more actions for {name(page)}" items={actions(page)} />
        </span>
      </div>
      <!-- Once loaded, so the level unfolds to the height of its rows. -->
      {#if open && children?.loaded}
        <Self
          {nav}
          level={children}
          lines={lines + (last ? "   " : "│  ")}
          {drag}
          {oncreate}
          {onmove}
          {ontrash}
        />
      {/if}
    </li>
  {/each}
  {#if level.more}
    <li class="tree-row">
      <span class="tree-prefix">{lines}└─ </span>
      <button class="tree-more" type="button" onclick={() => nav.loadLevel(level, true)}>
        more…
      </button>
    </li>
  {/if}
  {#if level.problem}
    <li class="tree-row"><span class="tree-prefix">{lines}</span><span class="problem">{level.problem}</span></li>
  {/if}
</ul>
