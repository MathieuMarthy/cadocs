<script lang="ts">
  import { onMount, tick } from "svelte";

  import Shell from "../components/Shell.svelte";
  import { openPage } from "../editor/collab";
  import { api, ApiError } from "../lib/api";
  import { acquireNav, type Nav, type PageRow } from "../lib/nav.svelte";
  import { app, sessionEnded } from "../lib/session.svelte";
  import Message from "./Message.svelte";

  let { id }: { id: string } = $props();

  /** Why this page cannot show. A failed session goes to login instead. */
  let gone = $state<string | null>(null);

  let editor = $state<HTMLElement>()!;
  let title = $state<HTMLInputElement>()!;
  let status = $state<HTMLElement>()!;
  let presence = $state<HTMLElement>()!;
  let name = $state("");
  /** What REST said about the page, which places it in the shell. It may come late. */
  let row = $state<PageRow | null>(null);
  /** REST did not refuse the page, so the editor shows, even if REST failed for a moment. */
  let showing = $state(false);

  const workspace = $derived(
    app.session?.workspaces.find((item) => item.id === row?.workspace_id) ?? null,
  );

  /** The navigation the shell keeps for this workspace, which names the space. */
  let nav = $state<Nav | null>(null);
  $effect(() => {
    if (!workspace || !app.session) return;
    const held = acquireNav(app.session.user.id, workspace);
    nav = held.nav;
    return () => held.release();
  });
  const spaceName = $derived((row && nav?.space(row.space_id)?.name.toLowerCase()) ?? "");

  /** A string, so that reading the page again does not ask for it again. */
  const spaceId = $derived(row?.space_id ?? null);
  $effect(() => {
    if (!spaceId || !nav) return;
    // A change to the pages of its space may have moved this one: the breadcrumb reads it again.
    if (!nav.versions.get(spaceId)) return;
    void check();
  });

  /** A socket cannot read the status of a failed upgrade, so REST says what went wrong. */
  async function check() {
    try {
      row = await api<PageRow>("GET", `/pages/${id}`);
    } catch (error) {
      if (error instanceof ApiError && error.status === 401) return false;
      if (error instanceof ApiError && error.status === 404) {
        gone = "page not found";
        return false;
      }
      // Anything else is the network or a passing failure, which the socket reports and outlives.
    }
    return true;
  }

  /** The open editor and its socket. */
  let page: ReturnType<typeof openPage> | null = null;

  // A page found gone by any read closes its socket, whether or not the server closed it first.
  $effect(() => {
    if (!gone) return;
    page?.destroy();
    page = null;
  });

  onMount(() => {
    let left = false;
    let retry: ReturnType<typeof setTimeout> | null = null;

    /** Until a read answers, the shell does not know the workspace: read again. */
    function place() {
      retry = setTimeout(() => {
        retry = null;
        if (left || row || gone) return;
        void check().then((readable) => {
          if (!readable) {
            page?.destroy();
            page = null;
          } else place();
        });
      }, 2000);
    }

    void check().then(async (readable) => {
      if (!readable || left) return;
      // The editor's elements render once the page is known to be readable.
      showing = true;
      if (!row) place();
      await tick();
      if (left) return;
      page = openPage(id, { editor, title, status, presence }, {
        onName: (text) => (name = text),
        onFinalClose(code) {
          page?.destroy();
          page = null;
          if (code === 4401) sessionEnded();
          else gone = code === 4403 ? "access revoked" : "page deleted";
        },
        onRepeatedFailure() {
          void check().then((readable) => {
            if (readable) return;
            page?.destroy();
            page = null;
          });
        },
      });
    });
    return () => {
      left = true;
      if (retry) clearTimeout(retry);
      page?.destroy();
    };
  });
</script>

{#if gone}
  <Message title={gone} />
{:else if !showing}
  <p class="meta" role="status" style="padding: var(--space-4)">loading…</p>
{:else}
  <Shell
    title={name || row?.title || "untitled"}
    {workspace}
    guess={false}
    crumbs={spaceName ? [spaceName] : []}
    bare
  >
    {#snippet end()}
      <span bind:this={status} class="status" role="status"></span>
      <div bind:this={presence} class="presence"></div>
    {/snippet}
    <main class="page">
      <input
        bind:this={title}
        class="title"
        type="text"
        placeholder="untitled"
        aria-label="page title"
        autocomplete="off"
      />
      <div bind:this={editor}></div>
    </main>
  </Shell>
{/if}
