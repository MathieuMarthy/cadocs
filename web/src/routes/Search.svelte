<script lang="ts">
  import { onDestroy } from "svelte";

  import Shell from "../components/Shell.svelte";
  import { api, type Workspace } from "../lib/api";
  import { describe } from "../lib/form.svelte";
  import { acquireNav } from "../lib/nav.svelte";
  import { navigate, route } from "../lib/router.svelte";
  import { app } from "../lib/session.svelte";

  interface Piece {
    text: string;
    match: boolean;
  }

  interface Hit {
    id: string;
    space_id: string;
    title: string;
    excerpt: Piece[];
  }

  /** Results follow the typing, once it pauses this long. */
  const PAUSE_MS = 200;

  let { workspace }: { workspace: Workspace } = $props();

  // App keys this screen by workspace, so the prop never changes under it.
  // svelte-ignore state_referenced_locally
  const held = acquireNav(app.session!.user.id, workspace);
  const nav = held.nav;
  onDestroy(held.release);

  // The address holds the query, so a search can be shared, reloaded and gone back to.
  const params = $derived(new URLSearchParams(route.search));
  const q = $derived(params.get("q") ?? "");
  const space = $derived(params.get("space") ?? "");

  let typed = $state(new URLSearchParams(location.search).get("q") ?? "");
  let hits = $state<Hit[] | null>(null);
  let problem = $state<string | null>(null);
  let asked = 0;

  const spaceName = (id: string) => nav.spaces.find((item) => item.id === id)?.name ?? "";

  function show(next: { q?: string; space?: string }) {
    const query = new URLSearchParams();
    const text = next.q ?? q;
    const where = next.space ?? space;
    if (text) query.set("q", text);
    if (where) query.set("space", where);
    const search = query.size ? `?${query}` : "";
    navigate(`/w/${workspace.slug}/search${search}`, { replace: true });
  }

  $effect(() => {
    // Only the start is trimmed: a space after the last word says that it is whole.
    const text = typed.trimStart();
    if (text === q) return;
    const timer = setTimeout(() => show({ q: text }), PAUSE_MS);
    return () => clearTimeout(timer);
  });

  $effect(() => {
    const text = q;
    const where = space;
    const mine = ++asked;
    if (!text.trim()) {
      hits = null;
      problem = null;
      return;
    }
    const query = new URLSearchParams({ q: text });
    if (where) query.set("space", where);
    api<{ items: Hit[] }>("GET", `/workspaces/${workspace.id}/search?${query}`).then(
      (reply) => {
        if (mine !== asked) return;
        hits = reply.items;
        problem = null;
      },
      (error) => {
        if (mine === asked) problem = describe(error);
      },
    );
  });

  /** Focuses the field when this screen opens, and again on `Ctrl K` from the shell. */
  function focusOnOpen(input: HTMLInputElement) {
    input.focus();
    input.select();
  }
</script>

<Shell title="search" {workspace}>
  <h1>search</h1>
  <form class="search-form" role="search" onsubmit={(event) => {
    event.preventDefault();
    show({ q: typed.trimStart() });
  }}>
    <label class="field search-query">
      <span class="label">words</span>
      <input
        type="search"
        name="q"
        maxlength="200"
        autocomplete="off"
        enterkeyhint="search"
        bind:value={typed}
        {@attach focusOnOpen}
      />
    </label>
    <label class="field search-space">
      <span class="label">space</span>
      <select value={space} onchange={(event) => show({ space: event.currentTarget.value })}>
        <option value="">all spaces</option>
        {#each nav.spaces as item (item.id)}
          <option value={item.id}>{item.name}</option>
        {/each}
      </select>
    </label>
  </form>

  {#if problem}<p class="problem" role="alert">{problem}</p>{/if}
  {#if !q.trim()}
    <p class="meta">words are matched with their other forms: deploy finds deployment.</p>
  {:else if hits === null}
    {#if !problem}<p class="meta" role="status">searching…</p>{/if}
  {:else if hits.length === 0}
    <p class="notice" role="status">no page matches</p>
  {:else}
    <ul class="records search-results" aria-label="results">
      {#each hits as hit (hit.id)}
        <li class="record">
          <span class="record-who">
            <a href="/?page={hit.id}">{hit.title || "untitled"}</a>
            <span>{spaceName(hit.space_id)}</span>
          </span>
          {#if hit.excerpt.length}
            <p class="excerpt">
              {#each hit.excerpt as piece, index (index)}
                {#if piece.match}<mark>{piece.text}</mark>{:else}{piece.text}{/if}
              {/each}
            </p>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
</Shell>
