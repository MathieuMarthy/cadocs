<script lang="ts" module>
  import type { Workspace } from "../lib/api";

  /** Only a workspace owner or admin creates spaces. */
  export const managesSpaces = (workspace: Workspace) =>
    workspace.role === "owner" || workspace.role === "admin";
</script>

<script lang="ts">
  import Field from "./Field.svelte";
  import { api } from "../lib/api";
  import { Form } from "../lib/form.svelte";
  import type { Nav, Space } from "../lib/nav.svelte";

  /** The name of a new space: Enter creates it, Escape gives up. */
  let { nav, onclose }: { nav: Nav; onclose: () => void } = $props();

  const creating = new Form();
  let name = $state("");

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    const done = await creating.run(() =>
      api<Space>("POST", `/workspaces/${nav.workspace.id}/spaces`, { name }),
    );
    if (!done) return;
    await nav.loadSpaces();
    onclose();
  }

  const focus = (element: HTMLElement) => element.querySelector("input")?.focus();
</script>

<form class="new-space" onsubmit={submit} {@attach focus}>
  <Field
    label="space name"
    bind:value={name}
    problem={creating.field("name")}
    required
    autocomplete="off"
    onkeydown={(event) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      onclose();
    }}
  />
  {#if creating.problem}<p class="problem" role="alert">{creating.problem}</p>{/if}
  <div class="actions">
    <button class="button" type="submit" disabled={creating.busy}>create space</button>
    <button class="button secondary" type="button" onclick={onclose}>cancel</button>
  </div>
</form>
