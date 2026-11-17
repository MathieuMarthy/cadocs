<script lang="ts">
  import { titleOf } from "../editor/pagelink";
  import { t } from "../lib/i18n";
  import { pieces } from "../lib/mentions";
  import { nameOf } from "../lib/people";

  /**
   * A comment's body as text, never HTML: each mention shows the member's name or the page's
   * title as this reader may see them, read when shown. The markup holds no whitespace between
   * pieces, since the body keeps its own (`white-space: pre-wrap`).
   */
  let { body, workspace }: { body: string; workspace: string | null } = $props();

  const name = (id: string) => (workspace ? nameOf(workspace, id) : Promise.resolve(null));
  const href = (id: string) => `/?page=${encodeURIComponent(id)}`;
</script>

<p class="comment-body">{#each pieces(body) as piece, index (index)}{#if piece.kind === "text"}{piece.text}{:else if piece.kind === "user"}{@render user(piece.id)}{:else}{@render page(piece.id)}{/if}{/each}</p>

{#snippet user(id: string)}{#await name(id)}<span class="mention">@…</span>{:then found}<span
      class="mention"
      class:mention-missing={found === null}>@{found ?? t("editor.mentionMissing")}</span
    >{:catch}<span class="mention">@{t("editor.mentionUnread")}</span>{/await}{/snippet}

{#snippet page(id: string)}{#await titleOf(id)}<a class="page-link" href={href(id)}>…</a>{:then title}<a
      class="page-link"
      class:page-link-missing={title === null}
      href={href(id)}>{title ?? t("editor.linkMissing")}</a
    >{:catch}<a class="page-link" href={href(id)}>{t("editor.linkUnread")}</a>{/await}{/snippet}
