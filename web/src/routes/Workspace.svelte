<script lang="ts">
  import Shell from "../components/Shell.svelte";
  import { api, type Page, type Workspace } from "../lib/api";
  import { describe } from "../lib/form.svelte";

  let { workspace }: { workspace: Workspace } = $props();
</script>

<Shell title={workspace.name} {workspace}>
  <h1>{workspace.name}</h1>
  <p class="meta">your role: {workspace.role}</p>
  {#key workspace.id}
    {#await api<Page<{ id: string; name: string }>>("GET", `/workspaces/${workspace.id}/spaces`)}
      <p class="meta" role="status">loading…</p>
    {:then spaces}
      {#if spaces.items.length === 0}
        <p class="notice">no spaces yet</p>
      {:else}
        <ul class="records">
          {#each spaces.items as space (space.id)}
            <li class="record">{space.name}</li>
          {/each}
        </ul>
      {/if}
    {:catch error}
      <p class="problem" role="alert">{describe(error)}</p>
    {/await}
  {/key}
</Shell>
