<script lang="ts">
  import { onMount, tick } from "svelte";

  import Field from "../components/Field.svelte";
  import IconButton from "../components/IconButton.svelte";
  import Menu, { type MenuItem } from "../components/Menu.svelte";
  import Shell from "../components/Shell.svelte";
  import { copy } from "../lib/actions";
  import { api, type Page } from "../lib/api";
  import { Form } from "../lib/form.svelte";
  import { app, refreshSession } from "../lib/session.svelte";

  interface AdminUser {
    id: string;
    email: string;
    name: string;
    is_instance_admin: boolean;
    disabled_at: number | string | null;
  }

  let users = $state<AdminUser[]>([]);
  let cursor = $state<string | null>(null);
  /** The one reset link on screen, which the server shows once. */
  let reset = $state<{ user: string; link: string } | null>(null);
  const list = new Form();
  const action = new Form();

  async function load(after: string | null) {
    await list.run(async () => {
      const query = after ? `?cursor=${encodeURIComponent(after)}` : "";
      const page = await api<Page<AdminUser>>("GET", `/admin/users${query}`);
      users = after ? [...users, ...page.items] : page.items;
      cursor = page.next_cursor;
    });
  }

  onMount(() => void load(null));

  /** A change to one's own account can end the session or the right to be here. */
  async function afterChange(id: string) {
    if (id === app.session?.user.id) await refreshSession();
  }

  async function change(user: AdminUser, changes: object) {
    await action.run(async () => {
      const updated = await api<AdminUser>("PATCH", `/admin/users/${user.id}`, changes);
      users = users.map((item) => (item.id === updated.id ? updated : item));
      await afterChange(user.id);
    });
  }

  async function remove(user: AdminUser) {
    await action.run(async () => {
      await api("DELETE", `/admin/users/${user.id}`);
      users = users.filter((item) => item.id !== user.id);
      await afterChange(user.id);
    });
  }

  async function resetLink(user: AdminUser) {
    await action.run(async () => {
      const path = `/admin/users/${user.id}/password-reset`;
      const { link } = await api<{ link: string }>("POST", path);
      reset = { user: user.id, link };
    });
  }

  /** The user whose deletion waits for a second press. */
  let deleting = $state<string | null>(null);

  const isSelf = (user: AdminUser) => user.id === app.session?.user.id;

  /** No one demotes, disables or deletes themselves from here: another admin does. */
  function actions(user: AdminUser): MenuItem[] {
    const reset: MenuItem = { icon: "link", label: "new reset link", select: () => void resetLink(user) };
    if (isSelf(user)) return [reset];
    return [
      {
        icon: "settings",
        label: user.is_instance_admin ? "remove admin" : "make admin",
        select: () => void change(user, { is_instance_admin: !user.is_instance_admin }),
      },
      {
        icon: user.disabled_at ? "rotate-ccw" : "log-out",
        label: user.disabled_at ? "enable" : "disable",
        select: () => void change(user, { disabled: !user.disabled_at }),
      },
      reset,
      {
        icon: "trash-2",
        label: "delete",
        destructive: true,
        danger: true,
        select: () => (deleting = user.id),
      },
    ];
  }

  const focus = (element: HTMLElement) => element.focus();

  /** Ends the deletion prompt, giving focus back to the user's menu button. */
  async function stopDeleting(user: AdminUser, refocus: boolean) {
    deleting = null;
    if (!refocus) return;
    await tick();
    document.querySelector<HTMLElement>(`[data-user="${user.id}"] .record-menu`)?.focus();
  }
</script>

<Shell title="admin" crumbWorkspace={false}>
  <h1>admin</h1>

  <h2>users</h2>
  {#if list.problem}<p class="problem" role="alert">{list.problem}</p>{/if}
  {#if action.problem}<p class="problem" role="alert">{action.problem}</p>{/if}
  <ul class="records">
    {#each users as user (user.id)}
      <li class="record" data-user={user.id}>
        <div class="record-who">
          {user.name}{#if isSelf(user)}&nbsp;<span class="you">(you)</span>{/if}
          <span>{user.email}</span>
        </div>
        <span class="badges">
          {#if user.is_instance_admin}<span class="badge">admin</span>{/if}
          {#if user.disabled_at}<span class="badge">disabled</span>{/if}
        </span>
        <Menu label="more actions for {user.name}" items={actions(user)} class="record-menu" />
        {#if deleting === user.id}
          <!-- svelte-ignore a11y_no_static_element_interactions -->
          <div
            class="record-link actions"
            onkeydown={(event) => {
              if (event.key !== "Escape") return;
              event.preventDefault();
              void stopDeleting(user, true);
            }}
          >
            <span class="meta">delete {user.name} for good?</span>
            <button
              class="button danger"
              type="button"
              disabled={action.busy}
              onclick={() => {
                void stopDeleting(user, false);
                void remove(user);
              }}
              {@attach focus}>delete</button
            >
            <button
              class="button secondary"
              type="button"
              onclick={() => void stopDeleting(user, true)}
            >
              cancel
            </button>
          </div>
        {/if}
        {#if reset?.user === user.id}
          <div class="record-link reset-link">
            <Field label="one-time link" value={reset.link} readonly />
            <IconButton
              icon="copy"
              verb="copy"
              label="copy the reset link"
              onclick={() => reset && void copy(reset.link)}
            />
          </div>
        {/if}
      </li>
    {/each}
  </ul>
  {#if cursor}
    <button class="button" type="button" disabled={list.busy} onclick={() => load(cursor)}>
      load more
    </button>
  {/if}
</Shell>
