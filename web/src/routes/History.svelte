<script lang="ts">
  import { onMount } from "svelte";

  import Confirm from "../components/Confirm.svelte";
  import Shell from "../components/Shell.svelte";
  import Time from "../components/Time.svelte";
  import { api, ApiError, type Page } from "../lib/api";
  import { describe } from "../lib/form.svelte";
  import { acquireNav, canEdit, type Nav, type PageView } from "../lib/nav.svelte";
  import { notify } from "../lib/notice.svelte";
  import { navigate } from "../lib/router.svelte";
  import { app } from "../lib/session.svelte";
  import Message from "./Message.svelte";

  /** `selected` is the revision in the address, or the most recent one when it holds none. */
  let { id, selected = null }: { id: string; selected?: number | null } = $props();

  interface Summary {
    number: number;
    title: string;
    authors: { id: string; name: string }[];
    created_at: number;
  }
  interface Shown extends Summary {
    /** Rendered and escaped by the server. */
    html: string;
  }

  let row = $state<PageView | null>(null);
  let gone = $state<string | null>(null);
  let items = $state<Summary[] | null>(null);
  let next = $state<string | null>(null);
  let problem = $state<string | null>(null);
  let shown = $state<Shown | null>(null);
  let restoring = $state(false);

  const workspace = $derived(app.session?.workspaces.find((item) => item.id === row?.workspace_id) ?? null);
  let nav = $state<Nav | null>(null);
  $effect(() => {
    if (!workspace || !app.session) return;
    const held = acquireNav(app.session.user.id, workspace);
    nav = held.nav;
    return () => held.release();
  });

  const pageTitle = $derived(row?.title || "untitled");
  const spaceName = $derived((row && nav?.space(row.space_id)?.name) ?? "");
  const crumbs = $derived([
    ...(spaceName ? [{ label: spaceName }] : []),
    ...(row?.ancestors ?? []).map((crumb) => ({ label: crumb.title || "untitled", href: `/?page=${crumb.id}` })),
    ...(row ? [{ label: pageTitle, href: `/?page=${id}` }] : []),
  ]);

  const current = $derived(selected ?? items?.[0]?.number ?? null);
  const authors = (revision: Summary) => revision.authors.map((author) => author.name).join(", ");

  async function load(cursor: string | null) {
    const query = cursor ? `?cursor=${encodeURIComponent(cursor)}` : "";
    try {
      const page = await api<Page<Summary>>("GET", `/pages/${id}/revisions${query}`);
      items = cursor ? [...(items ?? []), ...page.items] : page.items;
      next = page.next_cursor;
    } catch (error) {
      problem = describe(error);
    }
  }

  let asked = 0;
  $effect(() => {
    const number = current;
    if (number === null) return;
    const mine = ++asked;
    void api<Shown>("GET", `/pages/${id}/revisions/${number}`).then(
      (revision) => {
        if (mine === asked) shown = revision;
      },
      (error) => {
        if (mine === asked) problem = describe(error);
      },
    );
  });

  onMount(() => {
    void api<PageView>("GET", `/pages/${id}`).then(
      (answer) => (row = answer),
      (error) => {
        if (error instanceof ApiError && error.status === 404) gone = "page not found";
        else problem = describe(error);
      },
    );
    void load(null);
  });

  async function restore(number: number) {
    restoring = true;
    problem = null;
    try {
      await api("POST", `/pages/${id}/revisions/${number}/restore`);
      notify(`revision ${number} restored`);
      navigate(`/?page=${id}`);
    } catch (error) {
      problem = describe(error);
    } finally {
      restoring = false;
    }
  }
</script>

{#if gone}
  <Message title={gone} />
{:else}
  <Shell title="history" {workspace} guess={false} {crumbs}>
    <h1>history</h1>
    <p class="meta">a revision is kept after each pause in editing. restoring one keeps the current content as a revision too.</p>
    {#if problem}<p class="problem" role="alert">{problem}</p>{/if}
    {#if items === null}
      {#if !problem}<p class="meta" role="status">loading…</p>{/if}
    {:else if items.length === 0}
      <p class="notice">no revision yet. one is kept once editing pauses.</p>
    {:else}
      <div class="history">
        <nav class="history-list" aria-label="revisions">
          <ul class="records">
            {#each items as revision (revision.number)}
              <li class="record">
                <span class="record-who">
                  <a
                    href="/?page={id}&history={revision.number}"
                    aria-current={revision.number === current ? "page" : undefined}
                  >
                    #{revision.number} {revision.title || "untitled"}
                  </a>
                  <span><Time value={revision.created_at} />{#if revision.authors.length} · {authors(revision)}{/if}</span>
                </span>
              </li>
            {/each}
          </ul>
          {#if next}
            <button class="button secondary" type="button" onclick={() => load(next)}>more</button>
          {/if}
        </nav>
        {#if shown && shown.number === current}
          <section class="history-view" aria-label="revision {shown.number}">
            <div class="history-bar">
              <span class="meta">revision {shown.number} · <Time value={shown.created_at} /></span>
              {#if row && canEdit(row.role)}
                <Confirm
                  label="restore"
                  confirm="restore revision {shown.number}"
                  icon="rotate-ccw"
                  disabled={restoring}
                  onconfirm={() => restore(shown!.number)}
                />
              {/if}
            </div>
            <h2 class="history-title">{shown.title || "untitled"}</h2>
            <!-- The server escapes every text and attribute of this HTML (cadocs-content). -->
            <div class="tiptap revision-content">{@html shown.html}</div>
          </section>
        {:else}
          <p class="meta" role="status">loading…</p>
        {/if}
      </div>
    {/if}
  </Shell>
{/if}
