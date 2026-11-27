<script lang="ts">
  import { SvelteSet } from "svelte/reactivity";

  import BulkImport from "./BulkImport.svelte";
  import IconButton from "./IconButton.svelte";
  import Menu, { type MenuItem } from "./Menu.svelte";
  import NewSpace, { managesSpaces } from "./NewSpace.svelte";
  import Skeleton from "./Skeleton.svelte";
  import TreeLevel, { type Drag, fromTree } from "./TreeLevel.svelte";
  import { describe } from "../lib/form.svelte";
  import { t } from "../lib/i18n";
  import { canEdit, type Nav, type PageRow, type Place, type Space } from "../lib/nav.svelte";
  import { trashPage } from "../lib/actions";
  import { importPage } from "../lib/api";
  import { navigate } from "../lib/router.svelte";

  let { nav }: { nav: Nav } = $props();

  const drag = $state<Drag>({ page: null, element: null, over: null, zone: null });
  let problem = $state<string | null>(null);

  const label = (space: Space) => space.name;

  /** The new space form, which opens under the label and gives focus back to its plus. */
  let adding = $state(false);
  let plus = $state<HTMLButtonElement>();
  function closeAdding() {
    adding = false;
    queueMicrotask(() => plus?.focus());
  }

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
  const trash = (page: PageRow) => void attempt(() => trashPage(nav, page));

  /** The spaces whose zip import form is open, under their row: one each, as each polls its job. */
  const bulk = new SvelteSet<string>();

  /** The file picker of `import markdown`, and the space it imports into. */
  let picker = $state<HTMLInputElement>();
  let importing: Space | null = null;

  function pick(space: Space) {
    importing = space;
    picker?.click();
  }

  function imported() {
    const file = picker?.files?.[0];
    const space = importing;
    if (picker) picker.value = "";
    if (!file || !space) return;
    void attempt(async () => {
      const page = await importPage<PageRow>(space.id, file);
      navigate(`/?page=${page.id}`);
    });
  }

  /** Every reader reaches the settings, which show the grants; editors also import and reach the trash. */
  function spaceItems(space: Space): MenuItem[] {
    const base = `/w/${nav.workspace.slug}`;
    const settings: MenuItem = {
      icon: "settings",
      label: t("tree.spaceSettings"),
      href: `${base}/settings/${space.slug}`,
    };
    if (!canEdit(space.role)) return [settings];
    return [
      { icon: "upload", label: t("tree.import"), select: () => pick(space) },
      { icon: "upload", label: t("tree.importZip"), select: () => void bulk.add(space.id) },
      { icon: "trash-2", label: t("tree.openTrash"), href: `${base}/trash/${space.slug}` },
      settings,
    ];
  }

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

<nav class="tree" aria-label={t("tree.spaces")}>
  <div class="tree-head">
    <span class="label">{t("tree.spaces")}</span>
    {#if managesSpaces(nav.workspace) && !adding}
      <IconButton
        bind:element={plus}
        icon="plus"
        verb={t("space.new")}
        onclick={() => (adding = true)}
      />
    {/if}
  </div>
  {#if adding}<NewSpace {nav} onclose={closeAdding} />{/if}
  {#if !nav.spacesLoaded}
    <Skeleton kind="rows" />
  {:else if nav.problem}
    <p class="problem" role="alert">{nav.problem}</p>
  {:else if nav.spaces.length === 0}
    <p class="meta">{t("tree.noSpaces")}</p>
  {/if}
  <ul class="rows">
    {#each nav.spaces as space (space.id)}
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
          <button
            class="tree-label"
            type="button"
            aria-expanded={open}
            onclick={() => nav.toggle(space.id, space.id)}>{label(space)}/</button
          >
          <span class="tree-actions">
            {#if canEdit(space.role)}
              <IconButton
                class="tree-plus"
                icon="plus"
                verb={t("page.new")}
                label={t("page.newIn", { parent: label(space) })}
                onclick={() => create(space.id, null)}
              />
            {/if}
            <Menu label={t("menu.moreFor", { name: label(space) })} items={spaceItems(space)} />
          </span>
        </div>
        {#if bulk.has(space.id)}
          <BulkImport {nav} {space} onclose={() => bulk.delete(space.id)} />
        {/if}
        <!-- Once loaded, so the level unfolds to the height of its rows. -->
        {#if open && root?.loaded}
          {#if root.items.length === 0 && !root.problem}
            <p class="tree-row meta"><span class="tree-prefix">└─ </span>{t("tree.noPages")}</p>
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
  <input
    bind:this={picker}
    hidden
    type="file"
    accept=".md,.markdown,.zip,text/markdown,application/zip"
    aria-label={t("tree.importFile")}
    tabindex="-1"
    onchange={imported}
  />
</nav>
