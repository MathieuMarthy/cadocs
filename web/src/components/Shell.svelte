<script lang="ts" module>
  /** A breadcrumb segment above the title: a link when it leads somewhere. */
  export interface Crumb {
    label: string;
    href?: string;
  }

  /** Where the tree was scrolled, kept while screens each mount their own shell. */
  let treeScroll = 0;
</script>

<script lang="ts">
  import { tick, type Snippet } from "svelte";

  import { api, type Workspace } from "../lib/api";
  import { Form } from "../lib/form.svelte";
  import { acquireNav, type Nav } from "../lib/nav.svelte";
  import { navigate, route } from "../lib/router.svelte";
  import { app, signOut } from "../lib/session.svelte";
  import { nextTheme, theme } from "../lib/theme.svelte";
  import type { IconName } from "./icons";
  import IconButton from "./IconButton.svelte";
  import Menu, { type MenuItem } from "./Menu.svelte";
  import NewWorkspace from "./NewWorkspace.svelte";
  import Source from "./Source.svelte";
  import Tree from "./Tree.svelte";

  let {
    title,
    workspace = null,
    crumbs = [],
    crumbWorkspace = true,
    end,
    bare = false,
    guess = true,
    children,
  }: {
    title: string;
    workspace?: Workspace | null;
    /** Breadcrumb segments between the workspace and the title. */
    crumbs?: Crumb[];
    /** The breadcrumb starts with the workspace: not on its home, nor outside it. */
    crumbWorkspace?: boolean;
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
  /** The new workspace form is open in the selector. */
  let addingWorkspace = $state(false);

  async function closeAddingWorkspace() {
    addingWorkspace = false;
    await tick();
    sidebar?.querySelector<HTMLElement>(".new-workspace-row")?.focus();
  }

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

  const themeIcon: Record<string, IconName> = { light: "sun", dark: "moon", auto: "sun-moon" };

  const userItems = $derived<MenuItem[]>([
    { icon: "user", label: "account", href: "/account" },
    { icon: themeIcon[theme.current]!, label: `theme: ${theme.current}`, select: nextTheme },
    { icon: "log-out", label: "log out", select: () => void logOut() },
  ]);

  // The drawer of a narrow window: closed by default, and after every move to another address.
  const drawerId = $props.id();
  let drawer = $state(false);
  let sidebar = $state<HTMLElement>();
  let opener = $state<HTMLButtonElement>();

  $effect(() => {
    void route.path;
    void route.search;
    drawer = false;
  });

  async function openDrawer() {
    drawer = true;
    await tick();
    sidebar?.querySelector(".tree-row.selected")?.scrollIntoView({ block: "nearest" });
    sidebar?.querySelector<HTMLElement>("summary, a, button")?.focus();
  }

  function closeDrawer(refocus: boolean) {
    drawer = false;
    if (refocus) opener?.focus();
  }

  $effect(() => {
    if (!drawer) return;
    const key = (event: KeyboardEvent) => {
      // An open menu takes Escape for itself.
      if (event.key !== "Escape" || event.defaultPrevented) return;
      if (document.querySelector('.menu[role="menu"]')) return;
      closeDrawer(true);
    };
    const press = (event: PointerEvent) => {
      const target = event.target as Node | null;
      if (!target || sidebar?.contains(target) || opener?.contains(target)) return;
      // A menu of the sidebar sits in the top layer, outside it.
      if (target instanceof Element && target.closest(".menu")) return;
      closeDrawer(false);
    };
    window.addEventListener("keydown", key);
    window.addEventListener("pointerdown", press, true);
    return () => {
      window.removeEventListener("keydown", key);
      window.removeEventListener("pointerdown", press, true);
    };
  });

  // `Ctrl K`, or `⌘ K`, opens search from any screen of a workspace, or goes back to its field.
  $effect(() => {
    if (!current) return;
    const slug = current.slug;
    const key = (event: KeyboardEvent) => {
      if (event.key.toLowerCase() !== "k" || !(event.ctrlKey || event.metaKey)) return;
      if (event.altKey || event.shiftKey) return;
      event.preventDefault();
      const field = document.querySelector<HTMLInputElement>(".search-query input");
      if (field) {
        field.focus();
        field.select();
      } else {
        navigate(`/w/${slug}/search`);
      }
    };
    window.addEventListener("keydown", key);
    return () => window.removeEventListener("keydown", key);
  });

  /** The tree keeps its scroll from one screen to the next. */
  function keepScroll(element: HTMLElement) {
    element.scrollTop = treeScroll;
    const save = () => (treeScroll = element.scrollTop);
    element.addEventListener("scroll", save, { passive: true });
    return () => element.removeEventListener("scroll", save);
  }
</script>

<svelte:head><title>{title} · cadocs</title></svelte:head>

<div class="shell" class:drawer-open={drawer}>
  <aside bind:this={sidebar} id={drawerId} class="sidebar">
    <!-- The drawer covers the menu button of the top bar, so it closes from here. -->
    <IconButton class="drawer-close" icon="x" verb="close the sidebar" onclick={() => closeDrawer(true)} />
    <details class="selector">
      <summary>
        <span class="logo" aria-hidden="true"></span>
        <span class="selector-name">cadocs</span>
        <span class="selector-current">
          {current?.name ?? (guess ? "no workspace" : "…")} ▾
        </span>
      </summary>
      <ul class="rows">
        {#each session.workspaces as item (item.id)}
          <li>
            <a
              class="row"
              href="/w/{item.slug}"
              aria-current={route.path === `/w/${item.slug}` ? "page" : undefined}
            >
              {item.name}
            </a>
          </li>
        {/each}
        {#if session.can_create_workspace && !addingWorkspace}
          <li>
            <button class="row new-workspace-row" type="button" onclick={() => (addingWorkspace = true)}>
              new workspace
            </button>
          </li>
        {/if}
      </ul>
      {#if session.can_create_workspace && addingWorkspace}
        <NewWorkspace onclose={closeAddingWorkspace} />
      {/if}
    </details>

    {#if current}
      <a
        class="search-field"
        href="/w/{current.slug}/search"
        aria-label="search"
        aria-keyshortcuts="Control+K"
      >
        <span>search…</span><kbd>Ctrl K</kbd>
      </a>
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

    <div class="sidebar-scroll" {@attach keepScroll}>
      {#if nav}<Tree {nav} />{/if}
    </div>

    {#if current || session.user.is_instance_admin}
      <nav class="sidebar-group" aria-label="settings">
        <div class="label">settings</div>
        <ul class="rows">
          {#if current}
            <li>
              <a
                class="row"
                href="/w/{current.slug}/settings"
                aria-current={route.path === `/w/${current.slug}/settings` ? "page" : undefined}
              >
                workspace
              </a>
            </li>
          {/if}
          {#if session.user.is_instance_admin}<li>
            <a
              class="row"
              href="/admin"
              aria-current={route.path === "/admin" ? "page" : undefined}
            >
              admin
            </a>
          </li>{/if}
        </ul>
      </nav>
    {/if}

    <div class="sidebar-end">
      {#if leaving.problem}<p class="problem" role="alert">{leaving.problem}</p>{/if}
      <Menu label="user menu" text={session.user.name} items={userItems} class="row user-button" />
      <Source withTheme={false} />
    </div>
  </aside>

  <div class="column">
    <header class="topbar">
      <IconButton
        bind:element={opener}
        class="drawer-button"
        icon="menu"
        verb="open the sidebar"
        aria-expanded={drawer}
        aria-controls={drawerId}
        onclick={() => (drawer ? closeDrawer(false) : void openDrawer())}
      />
      <nav class="breadcrumb" aria-label="location">
        {#if current && crumbWorkspace}
          <a href="/w/{current.slug}">{current.name}</a><span class="sep">/</span>
        {/if}
        {#each crumbs as crumb, index (index)}
          {#if crumb.href}<a href={crumb.href}>{crumb.label}</a>{:else}<span>{crumb.label}</span>{/if}
          <span class="sep">/</span>
        {/each}
        <span class="current" aria-current="page">{title}</span>
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
