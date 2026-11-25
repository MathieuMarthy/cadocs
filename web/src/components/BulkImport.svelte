<script lang="ts">
  import Select from "./Select.svelte";
  import { bulkImport } from "../lib/api";
  import { describe, Form } from "../lib/form.svelte";
  import { t } from "../lib/i18n";
  import { collect, Level, type Nav, type PageRow, type Space } from "../lib/nav.svelte";
  import { notify } from "../lib/notice.svelte";

  /** A zip of Markdown files and folders, imported as pages at the root of a space or under one. */
  let { nav, space, onclose }: { nav: Nav; space: Space; onclose: () => void } = $props();

  /** As many pages as one level of the picker lists. */
  const MAX_LISTED = 1000;

  /** The pages of each select, the root's first, and the one picked in each, `""` for none. */
  let levels = $state<PageRow[][]>([]);
  let picked = $state<string[]>([]);
  let files = $state<FileList>();
  /** What the job said when it failed. */
  let failure = $state<string | null>(null);
  let listing = $state<string | null>(null);
  const importing = new Form();

  const name = (page: PageRow) => page.title || t("page.untitled");
  const find = (depth: number, id: string) => levels[depth]?.find((page) => page.id === id);
  const parent = $derived(picked.findLast((id) => id !== "") ?? null);

  async function list(depth: number, under: string | null) {
    try {
      const { items } = await collect<PageRow>(new Level(space.id, under).path, MAX_LISTED);
      // Another page may have been picked above meanwhile.
      if (under !== null && picked[depth - 1] !== under) return;
      levels[depth] = items;
      picked[depth] = "";
    } catch (error) {
      listing = describe(error);
    }
  }

  function pick(depth: number) {
    levels.length = depth + 1;
    picked.length = depth + 1;
    if (find(depth, picked[depth])?.has_children) void list(depth + 1, picked[depth]);
  }

  const options = (depth: number) => [
    {
      value: "",
      label:
        depth === 0
          ? t("import.root", { space: space.name })
          : t("import.itself", { page: name(find(depth - 1, picked[depth - 1])!) }),
    },
    ...levels[depth].map((page) => ({ value: page.id, label: name(page) })),
  ];

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    const file = files?.[0];
    if (!file) return;
    const under = parent;
    failure = null;
    await importing.run(async () => {
      const job = await bulkImport(space.id, under, file);
      if (job.status === "failed") {
        failure = job.message ? t("import.failedAt", { reason: job.message }) : t("import.failed");
        return;
      }
      notify(t("import.done", { count: job.pages ?? 0 }));
      // Unfold down to where the pages went, so they show.
      for (const id of [space.id, ...picked.filter((id) => id !== "")]) {
        if (!nav.open.has(id)) nav.toggle(id, space.id);
      }
      onclose();
    });
  }

  function escape(event: KeyboardEvent) {
    if (event.key !== "Escape" || importing.busy) return;
    event.preventDefault();
    onclose();
  }

  const focus = (element: HTMLElement) => element.querySelector("input")?.focus();

  void list(0, null);
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions (Escape closes from any field) -->
<form class="new-space" onsubmit={submit} onkeydown={escape} {@attach focus}>
  <label class="field">
    <span class="label">{t("import.file")}</span>
    <input type="file" accept=".zip,application/zip" required bind:files disabled={importing.busy} />
    {#if importing.field("file")}
      <span class="problem" role="alert">{importing.field("file")}</span>
    {/if}
  </label>
  {#each levels as _, depth (depth)}
    <Select
      label={depth === 0 ? t("import.under") : t("import.subpage")}
      bind:value={picked[depth]}
      options={options(depth)}
      problem={depth === 0 ? importing.field("parent_id") : null}
      onchange={() => pick(depth)}
      disabled={importing.busy}
    />
  {/each}
  {#if importing.busy}<p class="meta" role="status">{t("import.running")}</p>{/if}
  {#each [listing, importing.problem, failure] as problem}
    {#if problem}<p class="problem" role="alert">{problem}</p>{/if}
  {/each}
  <div class="actions">
    <button class="button" type="submit" disabled={importing.busy || !files?.length}>
      {t("import.submit")}
    </button>
    <button class="button secondary" type="button" disabled={importing.busy} onclick={onclose}>
      {t("common.cancel")}
    </button>
  </div>
</form>
