<script lang="ts">
  import { onDestroy } from "svelte";

  import Field from "../components/Field.svelte";
  import Shell from "../components/Shell.svelte";
  import { api, type Page, type Workspace } from "../lib/api";
  import { describe, Form } from "../lib/form.svelte";
  import { acquireNav, type PageRow, type Space } from "../lib/nav.svelte";
  import { app } from "../lib/session.svelte";

  let { workspace }: { workspace: Workspace } = $props();

  const RECENT = 20;

  // App keys this screen by workspace, so the props never change under it.
  // svelte-ignore state_referenced_locally
  const held = acquireNav(app.session!.user.id, workspace);
  const nav = held.nav;
  onDestroy(held.release);

  const manages = $derived(workspace.role === "owner" || workspace.role === "admin");

  let recent = $state<PageRow[] | null>(null);
  let recentProblem = $state<string | null>(null);
  let asked = 0;

  $effect(() => {
    // Any change to a page may reorder the list.
    void nav.version;
    const id = workspace.id;
    const mine = ++asked;
    api<Page<PageRow>>("GET", `/workspaces/${id}/recent?limit=${RECENT}`).then(
      (page) => {
        if (mine !== asked) return;
        recent = page.items;
        recentProblem = null;
      },
      (error) => {
        if (mine === asked) recentProblem = describe(error);
      },
    );
  });

  const spaceName = (id: string) => nav.space(id)?.name.toLowerCase() ?? "";

  const creating = new Form();
  let name = $state("");

  async function createSpace(event: SubmitEvent) {
    event.preventDefault();
    const done = await creating.run(() =>
      api<Space>("POST", `/workspaces/${workspace.id}/spaces`, { name }),
    );
    if (!done) return;
    name = "";
    await nav.loadSpaces();
  }
</script>

<Shell title={workspace.name} {workspace}>
  <h1>{workspace.name}</h1>
  <p class="meta">your role: {workspace.role}</p>

  <section aria-labelledby="recent-heading">
    <h2 id="recent-heading">recent pages</h2>
    {#if recentProblem}
      <p class="problem" role="alert">{recentProblem}</p>
    {:else if !recent || !nav.spacesLoaded}
      <p class="meta" role="status">loading…</p>
    {:else if nav.spaces.length === 0}
      <p class="notice">no spaces yet</p>
    {:else if recent.length === 0}
      <p class="notice">no pages yet</p>
    {:else}
      <ul class="records">
        {#each recent as page (page.id)}
          <li class="record">
            <span class="record-who">
              <a href="/?page={page.id}">{page.title || "untitled"}</a>
              <span>{spaceName(page.space_id)} · {new Date(page.updated_at).toLocaleString()}</span>
            </span>
          </li>
        {/each}
      </ul>
    {/if}
  </section>

  {#if manages}
    <section aria-labelledby="space-heading">
      <h2 id="space-heading">new space</h2>
      <form onsubmit={createSpace}>
        <Field
          label="space name"
          bind:value={name}
          problem={creating.field("name")}
          required
          autocomplete="off"
        />
        {#if creating.problem}<p class="problem" role="alert">{creating.problem}</p>{/if}
        <div class="actions">
          <button class="button" type="submit" disabled={creating.busy}>+ new space</button>
        </div>
      </form>
    </section>
  {/if}
</Shell>
