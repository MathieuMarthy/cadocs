<script lang="ts">
  import { api } from "../lib/api";
  import { t } from "../lib/i18n";

  let { page }: { page: string } = $props();

  interface Backlink {
    id: string;
    title: string;
  }

  let items = $state<Backlink[]>([]);

  // A failed read leaves the list empty: backlinks are a convenience, never a reason for a banner.
  $effect(() => {
    const id = page;
    items = [];
    api<{ items: Backlink[] }>("GET", `/pages/${encodeURIComponent(id)}/backlinks`)
      .then((body) => {
        if (id === page) items = body.items;
      })
      .catch(() => {});
  });
</script>

{#if items.length > 0}
  <nav class="backlinks" aria-label={t("page.backlinks")}>
    <h2>{t("page.backlinks")}</h2>
    <ul>
      {#each items as item (item.id)}
        <li><a href="/?page={item.id}">{item.title || t("page.untitled")}</a></li>
      {/each}
    </ul>
  </nav>
{/if}
