<script lang="ts">
  import { tick, untrack } from "svelte";

  import {
    canChange,
    canComment,
    type Comment,
    createComment,
    deleteComment,
    editComment,
    type PageComments,
    resolveThread,
    type Thread,
  } from "../lib/comments.svelte";
  import { Form } from "../lib/form.svelte";
  import { t } from "../lib/i18n";
  import CommentBody from "./CommentBody.svelte";
  import CommentField from "./CommentField.svelte";
  import IconButton from "./IconButton.svelte";
  import Menu, { type MenuItem } from "./Menu.svelte";
  import Time from "./Time.svelte";

  /** The comments panel of a page (ADR 0012): threads, the composer, replies, resolve and reopen. */
  let {
    id,
    comments,
    role,
    user,
    workspace,
    onclose,
  }: {
    /** The panel's id, which the top bar's button controls. */
    id: string;
    comments: PageComments;
    /** The reader's role on the page, which decides what they may do. */
    role: string;
    /** The reader's id, who may change their own comments. */
    user: string;
    /** The page's workspace once known, whose members and pages a comment mentions. */
    workspace: string | null;
    onclose: () => void;
  } = $props();

  const headingId = $props.id();
  const writable = $derived(canComment(role));
  let showResolved = $state(false);
  const shown = $derived(showResolved ? comments.resolved : comments.open);

  let panel = $state<HTMLElement>();
  let body = $state("");
  const composing = new Form();

  /** The one comment being replied to, edited or deleted: one form at a time keeps it plain. */
  let replying = $state<string | null>(null);
  let editing = $state<string | null>(null);
  let deleting = $state<string | null>(null);
  let text = $state("");
  /** Bumped by each opening, so that a field opened again reads `text` again: it reads it once. */
  let opened = $state(0);
  const acting = new Form();
  /** The thread a resolve or reopen acted on, which shows its refusal. */
  let target = $state<string | null>(null);

  // A comment left unwritten as the panel closes goes, with the highlight of its passage.
  $effect(() => () => {
    if (untrack(() => comments.draft)) comments.cancel();
  });

  const problemOf = (form: Form) => form.problem ?? form.field("body") ?? form.field("anchor");

  async function start() {
    comments.start();
    body = "";
    await tick();
    panel?.querySelector<HTMLElement>(".comment-form .comment-field")?.focus();
  }

  function cancel() {
    comments.cancel();
    composing.problem = null;
    composing.fields = {};
  }

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    const draft = comments.draft;
    if (!draft) return;
    const sent = { body, ...(draft.anchor ? { anchor: draft.anchor } : {}) };
    let created: Comment | null = null;
    const done = await composing.run(async () => {
      created = await createComment(comments.page, sent);
    });
    if (!done || !created) return;
    comments.cancel();
    body = "";
    showResolved = false;
    await comments.load();
    comments.activate((created as Comment).id);
  }

  /** Opens one of the forms of a comment, closing any other. */
  async function open(kind: "reply" | "edit" | "delete", comment: Comment) {
    replying = kind === "reply" ? comment.id : null;
    editing = kind === "edit" ? comment.id : null;
    deleting = kind === "delete" ? comment.id : null;
    text = kind === "edit" ? comment.body : "";
    opened += 1;
    target = null;
    acting.problem = null;
    acting.fields = {};
    await tick();
    panel
      ?.querySelector<HTMLElement>(`[data-form="${CSS.escape(comment.id)}"] :is(.comment-field, button)`)
      ?.focus();
  }

  function close() {
    replying = editing = deleting = null;
  }

  /** Runs one action on a comment of `thread`, then reads the threads again. */
  async function act(action: () => Promise<unknown>, thread: string | null = null) {
    target = thread;
    if (await acting.run(action)) {
      close();
      await comments.load();
    }
  }

  const reply = (event: SubmitEvent, thread: Thread) => {
    event.preventDefault();
    void act(() => createComment(comments.page, { body: text, parent_id: thread.id }));
  };

  const save = (event: SubmitEvent, comment: Comment) => {
    event.preventDefault();
    void act(() => editComment(comment.id, text));
  };

  const authorName = (comment: Comment) => comment.author?.name ?? t("comments.deletedAccount");

  function actions(comment: Comment): MenuItem[] {
    return [
      { icon: "pencil", label: t("comments.edit"), select: () => void open("edit", comment) },
      { icon: "x", label: t("comments.delete"), destructive: true, select: () => void open("delete", comment) },
    ];
  }

  // A thread picked, from its highlight or once written, scrolls into view, as soon as a read
  // brings it: a new comment can arrive with a later read than the one its form waited for.
  // Each pick scrolls once, so reading the threads again leaves the filter alone.
  let scrolled: string | null = null;
  $effect(() => {
    const id = comments.active;
    void comments.version;
    if (!id) {
      scrolled = null;
      return;
    }
    if (id === scrolled) return;
    const thread = untrack(() => comments.threads.find((item) => item.id === id));
    if (!thread) return;
    scrolled = id;
    showResolved = thread.resolved_at !== null;
    void tick().then(() =>
      panel?.querySelector(`[data-thread="${CSS.escape(id)}"]`)?.scrollIntoView({ block: "nearest" }),
    );
  });
</script>

<section bind:this={panel} {id} class="comments-panel" aria-labelledby={headingId}>
  <header class="comments-head">
    <h2 id={headingId} class="label">{t("comments.title")}</h2>
    <IconButton icon="x" verb={t("comments.close")} onclick={onclose} />
  </header>
  <div class="comments-filter">
    <button type="button" class="button secondary" aria-pressed={!showResolved} onclick={() => (showResolved = false)}>
      {t("comments.showOpen", { count: comments.open.length })}
    </button>
    <button type="button" class="button secondary" aria-pressed={showResolved} onclick={() => (showResolved = true)}>
      {t("comments.showResolved", { count: comments.resolved.length })}
    </button>
  </div>

  {#if writable}
    {#if comments.draft}
      <form class="comment-form" onsubmit={submit}>
        {#if comments.draft.anchor}
          <blockquote class="comment-quote">{comments.draft.anchor.text}</blockquote>
        {:else}
          <p class="meta">{t("comments.wholePage")}</p>
        {/if}
        <CommentField label={t("comments.field")} bind:value={body} {workspace} />
        {#if problemOf(composing)}<p class="problem" role="alert">{problemOf(composing)}</p>{/if}
        <div class="comment-actions">
          <button class="button" type="submit" disabled={composing.busy}>{t("comments.submit")}</button>
          <button class="button secondary" type="button" onclick={cancel}>{t("common.cancel")}</button>
        </div>
      </form>
    {:else}
      <button class="button comment-start" type="button" onclick={start}>
        {comments.hasSelection ? t("comments.onSelection") : t("comments.onPage")}
      </button>
    {/if}
  {/if}

  {#if comments.problem}<p class="problem" role="alert">{comments.problem}</p>{/if}

  {#if comments.loaded}
    {#if shown.length === 0}
      <p class="meta">{showResolved ? t("comments.noResolved") : t("comments.noOpen")}</p>
    {/if}
    <div class="threads">
      {#each shown as thread (thread.id)}
        <article
          class="thread"
          data-thread={thread.id}
          aria-current={comments.active === thread.id ? "true" : undefined}
        >
          {#if !thread.anchor}
            <p class="meta">{t("comments.wholePage")}</p>
          {:else if comments.lost(thread)}
            <p class="comment-quote lost">
              <span class="quote-text">{thread.anchor.text}</span>
              <span class="meta">{t("comments.lost")}</span>
            </p>
          {:else}
            <button
              type="button"
              class="comment-quote"
              title={t("comments.reveal")}
              onclick={() => comments.reveal(thread.id)}
            >
              {thread.anchor.text}
            </button>
          {/if}
          {#each [thread, ...thread.replies] as comment (comment.id)}
            {@render item(comment, thread)}
          {/each}
          {#if thread.resolved_at !== null}
            <p class="meta">
              {t("comments.resolvedBy", { name: thread.resolved_by?.name ?? t("comments.deletedAccount") })}
              · <Time value={thread.resolved_at} />
            </p>
          {/if}
          {#if writable && replying === thread.id}
            <form class="comment-form" data-form={thread.id} onsubmit={(event) => reply(event, thread)}>
              {#key opened}
                <CommentField label={t("comments.replyField")} bind:value={text} {workspace} short />
              {/key}
              {#if problemOf(acting)}<p class="problem" role="alert">{problemOf(acting)}</p>{/if}
              <div class="comment-actions">
                <button class="button" type="submit" disabled={acting.busy}>{t("comments.reply")}</button>
                <button class="button secondary" type="button" onclick={close}>{t("common.cancel")}</button>
              </div>
            </form>
          {:else if writable}
            {#if target === thread.id && problemOf(acting)}
              <p class="problem" role="alert">{problemOf(acting)}</p>
            {/if}
            <div class="comment-actions">
              {#if thread.resolved_at === null}
                <button class="button secondary" type="button" onclick={() => open("reply", thread)}>
                  {t("comments.reply")}
                </button>
                <button
                  class="button secondary"
                  type="button"
                  disabled={acting.busy}
                  onclick={() => act(() => resolveThread(thread.id, true), thread.id)}
                >
                  {t("comments.resolve")}
                </button>
              {:else}
                <button
                  class="button secondary"
                  type="button"
                  disabled={acting.busy}
                  onclick={() => act(() => resolveThread(thread.id, false), thread.id)}
                >
                  {t("comments.reopen")}
                </button>
              {/if}
            </div>
          {/if}
        </article>
      {/each}
    </div>
  {/if}
</section>

{#snippet item(comment: Comment, thread: Thread)}
  <div class="comment" class:reply={comment.id !== thread.id}>
    <div class="comment-head">
      <span class="comment-author">{authorName(comment)}</span>
      <span class="meta">
        <Time value={comment.created_at} />{#if comment.updated_at > comment.created_at}
          · {t("comments.edited")}{/if}
      </span>
      {#if canChange(comment, role, user)}
        <Menu label={t("comments.actions", { name: authorName(comment) })} items={actions(comment)} />
      {/if}
    </div>
    {#if editing === comment.id}
      <form class="comment-form" data-form={comment.id} onsubmit={(event) => save(event, comment)}>
        {#key opened}
          <CommentField label={t("comments.editField")} bind:value={text} {workspace} />
        {/key}
        {#if problemOf(acting)}<p class="problem" role="alert">{problemOf(acting)}</p>{/if}
        <div class="comment-actions">
          <button class="button" type="submit" disabled={acting.busy}>{t("comments.save")}</button>
          <button class="button secondary" type="button" onclick={close}>{t("common.cancel")}</button>
        </div>
      </form>
    {:else}
      <CommentBody body={comment.body} {workspace} />
    {/if}
    {#if deleting === comment.id}
      <div class="comment-form" data-form={comment.id}>
        <p class="meta">{comment.id === thread.id ? t("comments.confirmThread") : t("comments.confirmReply")}</p>
        {#if problemOf(acting)}<p class="problem" role="alert">{problemOf(acting)}</p>{/if}
        <div class="comment-actions">
          <button class="button danger" type="button" disabled={acting.busy} onclick={() => act(() => deleteComment(comment.id))}>
            {comment.id === thread.id ? t("comments.deleteThread") : t("comments.deleteReply")}
          </button>
          <button class="button secondary" type="button" onclick={close}>{t("common.cancel")}</button>
        </div>
      </div>
    {/if}
  </div>
{/snippet}
