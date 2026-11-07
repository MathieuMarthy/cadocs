<script lang="ts">
  import TreeLevel, { type Drag, fromTree } from "./TreeLevel.svelte";
  import { describe } from "../lib/form.svelte";
  import { canEdit, type Nav, type PageRow, type Place, type Space } from "../lib/nav.svelte";
  import { navigate } from "../lib/router.svelte";

  let { nav }: { nav: Nav } = $props();

  const drag = $state<Drag>({ page: null, element: null, over: null, zone: null });
  let problem = $state<string | null>(null);

  const label = (space: Space) => space.name.toLowerCase();

  async function attempt(action: () => Promise<unknown>) {
    problem = null;
    try {
      await action();
    } catch (error) {
      problem = describe(error);
    }
  }

  function create(space: string, parent: string | null) {
    void attempt(async () => {
      const page = await nav.create(space, parent);
      navigate(`/?page=${page.id}`);
    });
  }

  const move = (page: PageRow, place: Place) => void attempt(() => nav.move(page, place));
  const trash = (page: PageRow) => void attempt(() => nav.trash(page));

  /** A page dropped on a space goes last at its root. */
  function overSpace(event: DragEvent, space: Space) {
    if (!drag.page || !fromTree(event) || !canEdit(space.role)) return;
    event.preventDefault();
    drag.over = space.id;
    drag.zone = "inside";
  }

  function dropOnSpace(event: DragEvent, space: Space) {
    const page = drag.page;
    drag.over = null;
    if (!page || !fromTree(event) || !canEdit(space.role)) return;
    event.preventDefault();
    move(page, { space_id: space.id, parent_id: null });
  }
</script>

<nav class="tree" aria-label="spaces">
  <div class="label">spaces</div>
  {#if !nav.spacesLoaded}
    <p class="meta" role="status">loading…</p>
  {:else if nav.problem}
    <p class="problem" role="alert">{nav.problem}</p>
  {:else if nav.spaces.length === 0}
    <p class="meta">no spaces</p>
  {/if}
  <ul class="rows">
    {#each nav.spaces as space, index (space.id)}
      {@const open = nav.open.has(space.id)}
      {@const root = nav.loaded(space.id, null)}
      <li>
        <div
          class="tree-row"
          role="group"
          data-drop={drag.over === space.id ? "inside" : undefined}
          ondragover={(event) => overSpace(event, space)}
          ondragleave={() => {
            if (drag.over === space.id) drag.over = null;
          }}
          ondrop={(event) => dropOnSpace(event, space)}
        >
          <span class="tree-prefix">{`${String(index + 1).padStart(2, "0")} `}</span>
          <button
            class="tree-label"
            type="button"
            aria-expanded={open}
            onclick={() => nav.toggle(space.id, space.id)}>{label(space)}/</button
          >
          {#if canEdit(space.role)}
            <span class="tree-actions">
              <button
                class="tree-action"
                type="button"
                aria-label="new page in {label(space)}"
                onclick={() => create(space.id, null)}>+</button
              >
              <a
                class="tree-action"
                href="/w/{nav.workspace.slug}/trash/{space.slug}"
                aria-label="trash of {label(space)}">trash</a
              >
            </span>
          {/if}
        </div>
        {#if open && root}
          {#if root.loaded && root.items.length === 0 && !root.problem}
            <p class="tree-row meta"><span class="tree-prefix">└─ </span>no pages</p>
          {/if}
          <TreeLevel
            {nav}
            level={root}
            lines=""
            {drag}
            oncreate={create}
            onmove={move}
            ontrash={trash}
          />
        {/if}
      </li>
    {/each}
  </ul>
  {#if problem}<p class="problem" role="alert">{problem}</p>{/if}
</nav>
