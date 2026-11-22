<script lang="ts">
  import Select from "./Select.svelte";
  import { api } from "../lib/api";
  import { Form } from "../lib/form.svelte";
  import { t } from "../lib/i18n";
  import { navigate } from "../lib/router.svelte";

  /** Starts a subpage of `page` from one of the templates of its space, or deletes a template. */
  let {
    page,
    onclose,
  }: {
    page: { id: string; space_id: string };
    onclose: () => void;
  } = $props();

  type Template = { id: string; title: string };

  let templates = $state<Template[] | null>(null);
  let chosen = $state("");
  const starting = new Form();

  $effect(() => {
    const space = page.space_id;
    void api<{ items: Template[] }>("GET", `/spaces/${space}/templates`).then((list) => {
      templates = list.items;
      chosen = list.items[0]?.id ?? "";
    });
  });

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    await starting.run(async () => {
      const made = await api<{ id: string }>("POST", `/templates/${chosen}/pages`, { parent_id: page.id });
      onclose();
      navigate(`/?page=${made.id}`);
    });
  }

  async function remove() {
    await starting.run(async () => {
      await api("DELETE", `/templates/${chosen}`);
      templates = (templates ?? []).filter((template) => template.id !== chosen);
      chosen = templates[0]?.id ?? "";
    });
  }

  const focus = (element: HTMLElement) => element.querySelector("select")?.focus();
</script>

<form class="banner duplicate" onsubmit={submit} {@attach focus}>
  {#if templates && templates.length === 0}
    <p class="meta">{t("page.templateNone")}</p>
  {:else if templates}
    <Select
      label={t("page.template")}
      bind:value={chosen}
      options={templates.map((template) => ({ value: template.id, label: template.title || t("page.untitled") }))}
      onkeydown={(event) => {
        if (event.key !== "Escape") return;
        event.preventDefault();
        onclose();
      }}
    />
  {/if}
  {#if starting.problem}<p class="problem" role="alert">{starting.problem}</p>{/if}
  <div class="actions">
    <button class="button" type="submit" disabled={starting.busy || !chosen}>{t("page.templateStart")}</button>
    <button class="button secondary" type="button" disabled={starting.busy || !chosen} onclick={remove}>
      {t("page.templateDelete")}
    </button>
    <button class="button secondary" type="button" onclick={onclose}>{t("common.cancel")}</button>
  </div>
</form>
