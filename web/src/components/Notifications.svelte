<script lang="ts">
  import { api } from "../lib/api";
  import { t, type Key } from "../lib/i18n";
  import type { Nav } from "../lib/nav.svelte";

  /** The sidebar's notifications: what mentions, replies and comments tell the user. */
  let { nav }: { nav: Nav } = $props();

  interface Notification {
    id: string;
    kind: "mention" | "reply" | "comment";
    page: { id: string; title: string };
    actor: { name: string } | null;
    read: boolean;
  }

  let items = $state<Notification[]>([]);
  let unread = $state(0);

  const KEYS: Record<Notification["kind"], Key> = {
    mention: "notifications.mention",
    reply: "notifications.reply",
    comment: "notifications.comment",
  };

  async function load() {
    const body = await api<{ items: Notification[]; unread: number }>("GET", "/me/notifications");
    items = body.items;
    unread = body.unread;
  }

  // A failed read keeps what is shown: notifications are never a reason for a banner. It runs
  // again for each `notification` event, and for a reload the events socket asks for.
  $effect(() => {
    void nav.notifications;
    load().catch(() => {});
  });

  async function read(ids?: string[]) {
    try {
      await api("POST", "/me/notifications/read", ids ? { ids } : {});
      await load();
    } catch {
      // The list stays as it was, and the next event reads it again.
    }
  }
</script>

{#if items.length > 0}
  <nav class="sidebar-group" aria-label={t("notifications.title")}>
    <div class="label">{t("notifications.title")}{unread > 0 ? ` (${unread})` : ""}</div>
    <ul class="rows">
      {#each items as item (item.id)}
        {@const text = t(KEYS[item.kind], {
          name: item.actor?.name ?? t("editor.mentionMissing"),
          title: item.page.title || t("page.untitled"),
        })}
        <li>
          <a class="row" href="/?page={item.page.id}" onclick={() => !item.read && read([item.id])}>
            {#if item.read}{text}{:else}<strong>{text}</strong>{/if}
          </a>
        </li>
      {/each}
      {#if unread > 0}
        <li>
          <button type="button" class="row" onclick={() => read()}>{t("notifications.markAll")}</button>
        </li>
      {/if}
    </ul>
  </nav>
{/if}
