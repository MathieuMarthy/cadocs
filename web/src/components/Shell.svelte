<script lang="ts">
  import type { Snippet } from "svelte";

  import { api, type Workspace } from "../lib/api";
  import { Form } from "../lib/form.svelte";
  import { acquireNav, type Nav } from "../lib/nav.svelte";
  import { route } from "../lib/router.svelte";
  import { app, signOut } from "../lib/session.svelte";
  import Source from "./Source.svelte";
  import Tree from "./Tree.svelte";

  let {
    title,
    workspace = null,
    crumbs = [],
    end,
    bare = false,
    guess = true,
    children,
  }: {
    title: string;
    workspace?: Workspace | null;
    /** Breadcrumb segments between the workspace and the title. */
    crumbs?: string[];
    /** What sits at the right of the top bar. */
    end?: Snippet;
    /** The content brings its own layout, as the page view does. */
    bare?: boolean;
    /** Without a `workspace`, show the first one. A page whose workspace is unknown shows none. */
    guess?: boolean;
    children: Snippet;
  } = $props();

  const session = $derived(app.session!);
  const current = $derived(workspace ?? (guess ? (session.workspaces[0] ?? null) : null));

  let nav = $state<Nav | null>(null);

  $effect(() => {
    if (!current) {
      nav = null;
      return;
    }
    const held = acquireNav(session.user.id, current);
    nav = held.nav;
    return () => held.release();
  });

  const leaving = new Form();

  async function logOut() {
    if (await leaving.run(() => api("POST", "/auth/logout"))) signOut();
  }
</script>

<svelte:head><title>{title} · cadocs</title></svelte:head>

<div class="shell">
  <aside class="sidebar">
    <details class="selector">
      <summary>
        <span class="logo" aria-hidden="true"></span>
        <span class="selector-name">cadocs</span>
        <span class="selector-current">{current?.name.toLowerCase() ?? "no workspace"} ▾</span>
      </summary>
      <ul class="rows">
        {#each session.workspaces as item (item.id)}
          <li>
            <a
              class="row"
              href="/w/{item.slug}"
              aria-current={route.path === `/w/${item.slug}` ? "page" : undefined}
            >
              {item.name.toLowerCase()}
            </a>
          </li>
        {/each}
      </ul>
    </details>

    {#if current}
      <ul class="rows">
        <li>
          <a
            class="row"
            href="/w/{current.slug}"
            aria-current={route.path === `/w/${current.slug}` ? "page" : undefined}
          >
            home
          </a>
        </li>
      </ul>
    {/if}

    {#if nav}<Tree {nav} />{/if}

    <nav class="sidebar-group" aria-label="settings">
      <div class="label">settings</div>
      <ul class="rows">
        <li>
          <a
            class="row"
            href="/account"
            aria-current={route.path === "/account" ? "page" : undefined}
          >
            account
          </a>
        </li>
        {#if session.user.is_instance_admin}
          <li>
            <a
              class="row"
              href="/admin"
              aria-current={route.path === "/admin" ? "page" : undefined}
            >
              admin
            </a>
          </li>
        {/if}
      </ul>
    </nav>

    <div class="sidebar-end">
      <div class="user-name">{session.user.name}</div>
      {#if leaving.problem}<p class="problem" role="alert">{leaving.problem}</p>{/if}
      <div><button class="button secondary" type="button" onclick={logOut}>log out</button></div>
      <Source />
    </div>
  </aside>

  <div class="column">
    <header class="topbar">
      <nav class="breadcrumb" aria-label="location">
        {#if current}<span>{current.name.toLowerCase()}</span><span class="sep">/</span>{/if}
        {#each crumbs as crumb, index (index)}<span>{crumb}</span><span class="sep">/</span>{/each}
        <span class="current">{title}</span>
      </nav>
      {#if end}<div class="topbar-end">{@render end()}</div>{/if}
    </header>
    {#if bare}
      {@render children()}
    {:else}
      <main class="content">
        {@render children()}
      </main>
    {/if}
  </div>
</div>
