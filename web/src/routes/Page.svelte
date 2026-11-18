<script lang="ts" module>
  /** The comments panel stays open from one page to the next, as the person left it. */
  let panelWasOpen = false;
</script>

<script lang="ts">
  import { onMount, tick, untrack } from "svelte";

  import Comments from "../components/Comments.svelte";
  import DuplicatePage from "../components/DuplicatePage.svelte";
  import Icon from "../components/Icon.svelte";
  import Menu, { type MenuItem } from "../components/Menu.svelte";
  import Share from "../components/Share.svelte";
  import Shell from "../components/Shell.svelte";
  import { openPage } from "../editor/collab";
  import { copy, pageLink, showPage, trashPage } from "../lib/actions";
  import { api, ApiError, exportUrl } from "../lib/api";
  import { PageComments } from "../lib/comments.svelte";
  import { describe } from "../lib/form.svelte";
  import { acquireNav, canEdit, lastWorkspace, type Nav, type PageView } from "../lib/nav.svelte";
  import { notify } from "../lib/notice.svelte";
  import { app, sessionEnded } from "../lib/session.svelte";
  import Message from "./Message.svelte";
  import { type Key, t } from "../lib/i18n";

  let { id }: { id: string } = $props();

  /** Why this page cannot show. A failed session goes to login instead. */
  /** Why the page went, as a key of the catalogue. */
  let gone = $state<Key | null>(null);

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
      label: crumb.title || t("page.untitled"),
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

  const shownTitle = $derived(name || row?.title || t("page.untitled"));

  // A trash from the tree must know whether it takes this page away, through an ancestor too.
  const path = $derived([...(row?.ancestors ?? []).map((crumb) => crumb.id), id]);
  $effect(() => showPage(path));

  /** Where to go from a page that went: its parent, or its workspace for a page at the root. */
  const wayOut = $derived.by(() => {
    if (!row) return null;
    if (row.parent_id) return { href: `/?page=${row.parent_id}`, label: t("page.openParent") };
    const home = known(row.workspace_id);
    return home ? { href: `/w/${home.slug}`, label: t("page.openWorkspace") } : null;
  });

  /** The share dialog shows, for those who may write (PERMISSIONS.md). */
  let sharing = $state(false);
  // A role that drops below editor closes it, and a role given back does not open it again. The
  // open dialog goes without `close()`, so focus would fall to the body: it goes back to the menu.
  $effect(() => {
    if (editable || !untrack(() => sharing)) return;
    sharing = false;
    void tick().then(() => document.querySelector<HTMLElement>(".page-actions")?.focus());
  });

  /** The form that copies the page shows above it. */
  let duplicating = $state(false);

  /**
   * The top bar's menu: copy link, history, export and print for anyone, duplicate for those who
   * write in some space, public links and move to trash for those who may write here.
   */
  const actions = $derived.by<MenuItem[]>(() => {
    const items: MenuItem[] = [
      { icon: "link", label: t("page.copyLink"), select: () => void copy(pageLink(id)) },
      ...(editable ? [{ icon: "globe" as const, label: t("page.share"), select: () => (sharing = true) }] : []),
      { icon: "history", label: t("history.title"), href: `/?page=${id}&history` },
      { icon: "download", label: t("page.export"), href: exportUrl(id, false) },
      { icon: "download", label: t("page.exportAll"), href: exportUrl(id, true) },
      { icon: "printer", label: t("page.print"), select: () => print() },
    ];
    const page = row;
    const held = nav;
    if (page && held?.spaces.some((space) => canEdit(space.role))) {
      items.splice(2, 0, {
        icon: "copy",
        label: t("page.duplicate"),
        select: () => (duplicating = true),
      });
    }
    if (page && held && canEdit(page.role)) {
      items.push({
        icon: "trash-2",
        label: t("page.trash"),
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
    if (!first) void check({ socketTells: true });
  });

  // A role change that keeps the socket's mode, as from viewer to commenter, closes nothing:
  // the space list says it, and the page reads its role again for the comments panel.
  const spaceRole = $derived((spaceId && nav?.space(spaceId)?.role) || null);
  $effect(() => {
    const role = spaceRole;
    const read = untrack(() => row);
    if (role && read && read.role !== role) void check({ socketTells: true });
  });

  /**
   * A socket cannot read the status of a failed upgrade, so REST says what went wrong.
   * Reads can overlap, and an older one answering last would bring back an older role.
   *
   * REST answers `404` alike for a page in the trash and one the reader lost, while the socket
   * closes with `4404` or `4403`. A read made while the socket is open (`socketTells`) leaves a
   * `404` to it, and no read replaces what the socket said.
   */
  let reads = 0;
  async function check({ socketTells = false } = {}) {
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
        if (gone) return false;
        // An editor stopped on content it cannot show has no socket left to tell.
        if (socketTells && page && !stale) return true;
        gone = "page.notFound";
        return false;
      }
      // Anything else is the network or a passing failure, which the socket reports and outlives.
    }
    return true;
  }

  /** The open editor and its socket. */
  let page: ReturnType<typeof openPage> | null = null;

  /** The threads of this page, drawn by the editor and listed by the panel. */
  const comments = new PageComments(untrack(() => id));
  let panelOpen = $state(panelWasOpen);
  $effect(() => {
    panelWasOpen = panelOpen;
  });
  const panelId = $props.id();

  /** The last comment change heard, so that only a new one reads the threads again. */
  let heard: { nav: Nav; version: number; reloads: number } | null = null;
  $effect(() => {
    if (!nav) return;
    const now = { nav, version: nav.comments.get(id) ?? 0, reloads: nav.reloads };
    const before = heard;
    heard = now;
    if (before?.nav === nav && (before.version !== now.version || before.reloads !== now.reloads)) {
      comments.reload();
    }
  });

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
        // A `4409` closed the socket: no close code will follow, so a `404` here is the answer.
        onRoleChanged: () => void check(),
        onContentError: () => (stale = true),
        onSynced: () => comments.editorSynced(),
        onCommentsFound: (found) => comments.foundChanged(found),
        onCommentActivated(thread) {
          comments.activate(thread);
          panelOpen = true;
        },
        onSelection: (hasText) => (comments.hasSelection = hasText),
        onFinalClose(code) {
          page?.destroy();
          page = null;
          if (code === 4401) sessionEnded();
          else gone = code === 4403 ? "page.revoked" : "page.deleted";
        },
        onRepeatedFailure() {
          void check().then((readable) => {
            if (readable) return;
            page?.destroy();
            page = null;
          });
        },
      });
      comments.attach(page.comments);
      void comments.load();
    });
    return () => {
      left = true;
      if (retry) clearTimeout(retry);
      comments.stop();
      page?.destroy();
    };
  });
</script>

{#if gone}
  {#if wayOut && gone !== "page.revoked"}
    <Message title={t(gone)}><a class="button" href={wayOut.href}>{wayOut.label}</a></Message>
  {:else}
    <Message title={t(gone)} />
  {/if}
{:else if !showing}
  <Shell title={t("common.loading")} {workspace} guess={false} crumbWorkspace={false}>
    <p class="meta" role="status">{t("common.loading")}</p>
  </Shell>
{:else}
  <Shell title={shownTitle} {workspace} guess={false} {crumbs} bare>
    {#snippet end()}
      <span bind:this={status} class="status" role="status"></span>
      <div bind:this={presence} class="presence"></div>
      <button
        type="button"
        class="icon-button comments-toggle"
        aria-label={t("comments.toggle")}
        title={comments.open.length > 0
          ? t("comments.toggleOpen", { count: comments.open.length })
          : t("comments.toggle")}
        aria-expanded={panelOpen}
        aria-controls={panelId}
        onclick={() => (panelOpen = !panelOpen)}
      >
        <Icon name="message-square" />
        {#if comments.open.length > 0}<span class="comments-count" aria-hidden="true">{comments.open.length}</span>{/if}
      </button>
      <Menu label={t("menu.moreFor", { name: shownTitle })} items={actions} class="page-actions" />
    {/snippet}
    <div class="page-layout">
      <main class="page">
        {#if duplicating && row && nav}
          <DuplicatePage {nav} page={row} onclose={() => (duplicating = false)} />
        {/if}
        {#if stale}
          <div class="banner" role="alert">
            <span>{t("page.stale")}</span>
            <button type="button" class="button" onclick={() => location.reload()}>{t("app.reload")}</button>
          </div>
        {/if}
        <textarea
          bind:this={title}
          class="title"
          rows="1"
          placeholder={t("page.untitled")}
          aria-label={t("page.titleField")}
          autocomplete="off"
          spellcheck="false"
        ></textarea>
        <div bind:this={editor}></div>
      </main>
      {#if panelOpen && app.session}
        <Comments
          id={panelId}
          {comments}
          role={row?.role ?? "viewer"}
          user={app.session.user.id}
          workspace={row?.workspace_id ?? null}
          onclose={() => {
            panelOpen = false;
            document.querySelector<HTMLElement>(".comments-toggle")?.focus();
          }}
        />
      {/if}
    </div>
    {#if sharing && editable}
      <Share page={id} onclose={() => (sharing = false)} />
    {/if}
  </Shell>
{/if}
