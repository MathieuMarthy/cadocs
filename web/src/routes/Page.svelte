<script lang="ts">
  import { onMount, tick, untrack } from "svelte";

  import Menu, { type MenuItem } from "../components/Menu.svelte";
  import Shell from "../components/Shell.svelte";
  import { openPage } from "../editor/collab";
  import { copy, pageLink, trashPage } from "../lib/actions";
  import { api, ApiError, exportUrl } from "../lib/api";
  import { describe } from "../lib/form.svelte";
  import { acquireNav, canEdit, lastWorkspace, type Nav, type PageView } from "../lib/nav.svelte";
  import { notify } from "../lib/notice.svelte";
  import { app, sessionEnded } from "../lib/session.svelte";
  import Message from "./Message.svelte";

  let { id }: { id: string } = $props();

  /** Why this page cannot show. A failed session goes to login instead. */
  let gone = $state<string | null>(null);

  let editor = $state<HTMLElement>()!;
  let title = $state<HTMLTextAreaElement>()!;
  let status = $state<HTMLElement>()!;
  let presence = $state<HTMLElement>()!;
  let name = $state("");
  /** What REST said about the page, which places it in the shell. It may come late. */
  let row = $state<PageView | null>(null);
  /** REST did not refuse the page, so the editor shows, even if REST failed for a moment. */
  let showing = $state(false);
  /** The page holds content this version cannot show, so the editor stopped (ADR 0016). */
  let stale = $state(false);
  /** Read-only until a read says the role may write. */
  const editable = $derived(row ? canEdit(row.role) : false);

  const known = (id: string | undefined) =>
    app.session?.workspaces.find((item) => item.id === id) ?? null;
  /** Until the first read answers, the tree that showed stays, so a click in it does not flash. */
  const workspace = $derived(
    row || showing
      ? known(row?.workspace_id)
      : known(app.session ? lastWorkspace(app.session.user.id)?.id : undefined),
  );

  /** The navigation the shell keeps for this workspace, which names the space. */
  let nav = $state<Nav | null>(null);
  $effect(() => {
    if (!workspace || !app.session) return;
    const held = acquireNav(app.session.user.id, workspace);
    nav = held.nav;
    return () => held.release();
  });
  const spaceName = $derived((row && nav?.space(row.space_id)?.name) ?? "");
  const crumbs = $derived([
    ...(spaceName ? [{ label: spaceName }] : []),
    ...(row?.ancestors ?? []).map((crumb) => ({
      label: crumb.title || "untitled",
      href: `/?page=${crumb.id}`,
    })),
  ]);

  /** The tree opens down to this page once, so its row shows, and is left alone after that. */
  let revealed = false;
  $effect(() => {
    if (!row || !nav || revealed || nav.workspace.id !== row.workspace_id) return;
    revealed = true;
    const space = row.space_id;
    nav.expand(space, space);
    for (const crumb of row.ancestors) nav.expand(crumb.id, space);
  });

  const shownTitle = $derived(name || row?.title || "untitled");

  /** The top bar's menu: copy link, history, export and print for anyone, move to trash for those who may write. */
  const actions = $derived.by<MenuItem[]>(() => {
    const items: MenuItem[] = [
      { icon: "link", label: "copy link", select: () => void copy(pageLink(id)) },
      { icon: "history", label: "history", href: `/?page=${id}&history` },
      { icon: "download", label: "export as markdown", href: exportUrl(id, false) },
      { icon: "download", label: "export with attachments", href: exportUrl(id, true) },
      { icon: "printer", label: "print", select: () => print() },
    ];
    const page = row;
    const held = nav;
    if (page && held && canEdit(page.role)) {
      items.push({
        icon: "trash-2",
        label: "move to trash",
        destructive: true,
        select: () => trashPage(held, page).catch((error) => notify(describe(error))),
      });
    }
    return items;
  });

  /** A string, so that reading the page again does not ask for it again. */
  const spaceId = $derived(row?.space_id ?? null);
  /** The last change read for, so an effect that runs for another reason reads nothing. */
  let seen: { nav: Nav; space: string; version: number } | null = null;
  $effect(() => {
    if (!spaceId || !nav) return;
    // A change to the pages of its space may have moved this one: the breadcrumb reads it again.
    const version = nav.versions.get(spaceId);
    if (!version) return;
    if (seen?.nav === nav && seen.space === spaceId && seen.version === version) return;
    const first = seen === null;
    seen = { nav, space: spaceId, version };
    // The first sight of a version only learns it: the page was just read.
    if (!first) void check();
  });

  /** A socket cannot read the status of a failed upgrade, so REST says what went wrong. */
  /** Reads can overlap, and an older one answering last would bring back an older role. */
  let reads = 0;
  async function check() {
    const read = ++reads;
    try {
      const answer = await api<PageView>("GET", `/pages/${id}`);
      if (read !== reads) return true;
      row = answer;
      // Every read sets the mode, even to the one it had: a `4409` made the editor read-only.
      page?.setEditable(canEdit(answer.role));
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
      const session = app.session;
      if (!session) return;
      const user = { id: session.user.id, name: session.user.name };
      const options = {
        user,
        editable: untrack(() => editable),
        workspace: () => row?.workspace_id ?? null,
      };
      page = openPage(id, { editor, title, status, presence }, options, {
        onName: (text) => (name = text),
        onRoleChanged: () => void check(),
        onContentError: () => (stale = true),
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
  <Shell title="loading…" {workspace} guess={false} crumbWorkspace={false}>
    <p class="meta" role="status">loading…</p>
  </Shell>
{:else}
  <Shell title={shownTitle} {workspace} guess={false} {crumbs} bare>
    {#snippet end()}
      <span bind:this={status} class="status" role="status"></span>
      <div bind:this={presence} class="presence"></div>
      <Menu label="more actions for {shownTitle}" items={actions} />
    {/snippet}
    <main class="page">
      {#if stale}
        <div class="banner" role="alert">
          <span>this page holds content this version cannot show. reload to edit it.</span>
          <button type="button" class="button" onclick={() => location.reload()}>reload</button>
        </div>
      {/if}
      <textarea
        bind:this={title}
        class="title"
        rows="1"
        placeholder="untitled"
        aria-label="page title"
        autocomplete="off"
        spellcheck="false"
      ></textarea>
      <div bind:this={editor}></div>
    </main>
  </Shell>
{/if}
