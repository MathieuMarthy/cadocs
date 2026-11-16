<script lang="ts">
  import Field from "./Field.svelte";
  import { api, type Workspace } from "../lib/api";
  import { Form } from "../lib/form.svelte";
  import { t } from "../lib/i18n";
  import { navigate } from "../lib/router.svelte";
  import { refreshSession } from "../lib/session.svelte";

  /**
   * The name of a new workspace, which opens once created. With `onclose`, Escape and `cancel`
   * give up; without it, the form stays, as on the screen of a user with no workspace.
   */
  let { onclose }: { onclose?: () => void } = $props();

  const creating = new Form();
  let name = $state("");

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    await creating.run(async () => {
      const workspace = await api<Workspace>("POST", "/workspaces", { name });
      await refreshSession();
      navigate(`/w/${workspace.slug}`);
    });
  }

  const focus = (element: HTMLElement) => {
    if (onclose) element.querySelector("input")?.focus();
  };
</script>

<form class="new-workspace" onsubmit={submit} {@attach focus}>
  <Field
    label={t("workspace.name")}
    bind:value={name}
    problem={creating.field("name")}
    required
    autocomplete="off"
    onkeydown={(event) => {
      if (event.key !== "Escape" || !onclose) return;
      event.preventDefault();
      onclose();
    }}
  />
  {#if creating.problem}<p class="problem" role="alert">{creating.problem}</p>{/if}
  <div class="actions">
    <button class="button" type="submit" disabled={creating.busy}>{t("workspace.create")}</button>
    {#if onclose}
      <button class="button secondary" type="button" onclick={onclose}>{t("common.cancel")}</button>
    {/if}
  </div>
</form>
