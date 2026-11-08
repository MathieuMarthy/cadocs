<script lang="ts">
  import { onDestroy, tick } from "svelte";

  import NewSpace, { managesSpaces } from "../components/NewSpace.svelte";
  import Shell from "../components/Shell.svelte";
  import Time from "../components/Time.svelte";
  import { api, type Page, type Workspace } from "../lib/api";
  import { describe } from "../lib/form.svelte";
  import { acquireNav, canEdit, type PageRow, type Space } from "../lib/nav.svelte";
  import { navigate } from "../lib/router.svelte";
  import { app } from "../lib/session.svelte";

  let { workspace }: { workspace: Workspace } = $props();

  const RECENT = 20;

  // App keys this screen by workspace, so the props never change under it.
  // svelte-ignore state_referenced_locally
  const held = acquireNav(app.session!.user.id, workspace);
  const nav = held.nav;
  onDestroy(held.release);

  const manages = $derived(managesSpaces(workspace));

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

  const spaceName = (id: string) => nav.space(id)?.name ?? "";

  /** Where the empty home's `new page` goes: the first space this user may write in. */
  const writable = $derived(nav.spaces.find((space) => canEdit(space.role)) ?? null);
  let adding = $state(false);
  let addButton = $state<HTMLButtonElement>();
  let problem = $state<string | null>(null);
  /** A double click makes one page. */
  let creating = $state(false);

  async function firstPage(space: Space) {
    if (creating) return;
    creating = true;
    problem = null;
    try {
      const page = await nav.create(space.id, null);
      navigate(`/?page=${page.id}`);
    } catch (error) {
      problem = describe(error);
    } finally {
      creating = false;
    }
  }

  /** The form gives focus back to the button that opened it. */
  async function closeAdding() {
    adding = false;
    await tick();
    addButton?.focus();
  }
</script>

<Shell title={workspace.name} {workspace} crumbWorkspace={false}>
  <h1>{workspace.name}</h1>

  <section aria-labelledby="recent-heading">
    <h2 id="recent-heading">recent pages</h2>
    {#if recentProblem}
      <p class="problem" role="alert">{recentProblem}</p>
    {:else if !recent || !nav.spacesLoaded}
      <p class="meta" role="status">loading…</p>
    {:else if nav.spaces.length === 0}
      <p class="notice">no spaces yet</p>
      {#if !manages}
        <p class="meta">a workspace admin can add one, or give you access to one.</p>
      {:else if adding}
        <NewSpace {nav} onclose={closeAdding} />
      {:else}
        <div class="actions">
          <button bind:this={addButton} class="button" type="button" onclick={() => (adding = true)}>
            new space
          </button>
        </div>
      {/if}
    {:else if recent.length === 0}
      <p class="notice">no pages yet</p>
      {#if writable}
        {#if problem}<p class="problem" role="alert">{problem}</p>{/if}
        <div class="actions">
          <button
            class="button"
            type="button"
            disabled={creating}
            onclick={() => firstPage(writable)}>new page</button
          >
        </div>
      {:else}
        <p class="meta">pages show here once someone writes one.</p>
      {/if}
    {:else}
      <ul class="records">
        {#each recent as page (page.id)}
          <li class="record">
            <span class="record-who">
              <a href="/?page={page.id}">{page.title || "untitled"}</a>
              <span>{spaceName(page.space_id)} · <Time value={page.updated_at} /></span>
            </span>
          </li>
        {/each}
      </ul>
    {/if}
  </section>
</Shell>
