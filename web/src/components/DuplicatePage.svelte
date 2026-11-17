<script lang="ts">
  import Select from "./Select.svelte";
  import { api } from "../lib/api";
  import { Form } from "../lib/form.svelte";
  import { t } from "../lib/i18n";
  import { canEdit, type Nav } from "../lib/nav.svelte";
  import { navigate } from "../lib/router.svelte";

  /** Copies a page with its subpages and attachments into a space the user writes in. */
  let {
    nav,
    page,
    onclose,
  }: {
    nav: Nav;
    page: { id: string; space_id: string; parent_id: string | null };
    onclose: () => void;
  } = $props();

  const options = $derived(
    nav.spaces
      .filter((space) => canEdit(space.role))
      .map((space) => ({ value: space.id, label: space.name })),
  );
  // Its own space first when the user writes there: the copy then sits right after the page.
  let space = $state(firstTarget());

  function firstTarget() {
    const own = nav.space(page.space_id);
    if (own && canEdit(own.role)) return own.id;
    return nav.spaces.find((item) => canEdit(item.role))?.id ?? "";
  }

  const copying = new Form();

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    const body =
      space === page.space_id ? { parent_id: page.parent_id, after: page.id } : { space_id: space };
    await copying.run(async () => {
      const copy = await api<{ id: string }>("POST", `/pages/${page.id}/duplicate`, body);
      onclose();
      navigate(`/?page=${copy.id}`);
    });
  }

  const focus = (element: HTMLElement) => element.querySelector("select")?.focus();
</script>

<form class="banner duplicate" onsubmit={submit} {@attach focus}>
  <Select
    label={t("page.duplicateInto")}
    bind:value={space}
    {options}
    problem={copying.field("space_id")}
    onkeydown={(event) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      onclose();
    }}
  />
  <p class="meta">{t("page.duplicateHint")}</p>
  {#if copying.problem}<p class="problem" role="alert">{copying.problem}</p>{/if}
  <div class="actions">
    <button class="button" type="submit" disabled={copying.busy || !space}>{t("page.duplicate")}</button>
    <button class="button secondary" type="button" onclick={onclose}>{t("common.cancel")}</button>
  </div>
</form>
