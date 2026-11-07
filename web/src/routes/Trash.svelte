<script lang="ts">
  import { onDestroy } from "svelte";

  import Confirm from "../components/Confirm.svelte";
  import IconButton from "../components/IconButton.svelte";
  import Shell from "../components/Shell.svelte";
  import { api, type Page, type Workspace } from "../lib/api";
  import { describe } from "../lib/form.svelte";
  import { acquireNav, canEdit, type PageRow } from "../lib/nav.svelte";
  import { app } from "../lib/session.svelte";

  let { workspace, slug }: { workspace: Workspace; slug: string } = $props();

  // App keys this screen by workspace and space, so the props never change under it.
  // svelte-ignore state_referenced_locally
  const held = acquireNav(app.session!.user.id, workspace);
  const nav = held.nav;
  onDestroy(held.release);

  const space = $derived(nav.spaces.find((item) => item.slug === slug) ?? null);
  const title = $derived(space ? `trash of ${space.name}` : "trash");

  let items = $state<PageRow[] | null>(null);
  let next = $state<string | null>(null);
  let problem = $state<string | null>(null);
  let asked = 0;

  async function load(id: string, cursor: string | null) {
    const mine = ++asked;
    const query = cursor ? `?cursor=${encodeURIComponent(cursor)}` : "";
    try {
      const page = await api<Page<PageRow>>("GET", `/spaces/${id}/trash${query}`);
      if (mine !== asked) return;
      items = cursor ? [...(items ?? []), ...page.items] : page.items;
      next = page.next_cursor;
      problem = null;
    } catch (error) {
      if (mine === asked) problem = describe(error);
    }
  }

  /** A string, so that a new list of spaces does not refetch on its own. */
  const spaceId = $derived(space?.id ?? null);

  $effect(() => {
    if (!spaceId) return;
    // Any change to the pages of this space may change its trash.
    void nav.versions.get(spaceId);
    void load(spaceId, null);
  });

  async function act(action: () => Promise<unknown>) {
    problem = null;
    try {
      await action();
    } catch (error) {
      problem = describe(error);
    }
    if (space) nav.spaceChanged(space.id);
  }

  const restore = (page: PageRow) => act(() => api("POST", `/pages/${page.id}/restore`));
  const purge = (page: PageRow) => act(() => api("DELETE", `/pages/${page.id}/purge`));
</script>

<Shell {title} {workspace}>
  <h1>{title}</h1>
  {#if !nav.spacesLoaded}
    <p class="meta" role="status">loading…</p>
  {:else if !space}
    <p class="notice">no such space</p>
  {:else}
    <p class="meta">restoring a page also restores its subpages.</p>
    {#if problem}<p class="problem" role="alert">{problem}</p>{/if}
    {#if items === null}
      {#if !problem}<p class="meta" role="status">loading…</p>{/if}
    {:else if items.length === 0}
      <p class="notice">the trash is empty</p>
    {:else}
      <ul class="records">
        {#each items as page (page.id)}
          <li class="record">
            <span class="record-who">
              {page.title || "untitled"}
              <span>moved to trash {new Date(page.deleted_at ?? page.updated_at).toLocaleString()}</span>
            </span>
            {#if canEdit(page.role)}
              <IconButton icon="rotate-ccw" verb="restore" onclick={() => restore(page)} />
            {/if}
            {#if page.role === "admin"}
              <Confirm
                label="delete forever"
                confirm="delete forever"
                icon="x"
                danger
                onconfirm={() => purge(page)}
              />
            {/if}
          </li>
        {/each}
      </ul>
      {#if next}
        <button class="button secondary" type="button" onclick={() => load(space.id, next)}>
          more
        </button>
      {/if}
    {/if}
  {/if}
</Shell>
