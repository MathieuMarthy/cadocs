<script lang="ts">
  import { onMount } from "svelte";

  import Confirm from "../components/Confirm.svelte";
  import Field from "../components/Field.svelte";
  import Shell from "../components/Shell.svelte";
  import { api, type Page, type Workspace } from "../lib/api";
  import { Form } from "../lib/form.svelte";
  import { navigate } from "../lib/router.svelte";
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
  let workspaceName = $state("");
  const list = new Form();
  const action = new Form();
  const creation = new Form();

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

  async function createWorkspace(event: SubmitEvent) {
    event.preventDefault();
    await creation.run(async () => {
      const workspace = await api<Workspace>("POST", "/workspaces", { name: workspaceName });
      await refreshSession();
      navigate(`/w/${workspace.slug}`);
    });
  }
</script>

<Shell title="admin" crumbWorkspace={false}>
  <h1>admin</h1>

  <h2>users</h2>
  {#if list.problem}<p class="problem" role="alert">{list.problem}</p>{/if}
  {#if action.problem}<p class="problem" role="alert">{action.problem}</p>{/if}
  <ul class="records">
    {#each users as user (user.id)}
      <li class="record">
        <div class="record-who">
          {user.name}
          <span>
            {user.email}{#if user.is_instance_admin}&nbsp;· admin{/if}{#if user.disabled_at}&nbsp;· disabled{/if}
          </span>
        </div>
        <div class="actions">
          <button
            class="button secondary"
            type="button"
            disabled={action.busy}
            onclick={() => change(user, { is_instance_admin: !user.is_instance_admin })}
          >
            {user.is_instance_admin ? "remove admin" : "make admin"}
          </button>
          <button
            class="button secondary"
            type="button"
            disabled={action.busy}
            onclick={() => change(user, { disabled: !user.disabled_at })}
          >
            {user.disabled_at ? "enable" : "disable"}
          </button>
          <button
            class="button secondary"
            type="button"
            disabled={action.busy}
            onclick={() => resetLink(user)}
          >
            reset link
          </button>
          <Confirm
            label="delete"
            confirm="yes, delete"
            disabled={action.busy}
            onconfirm={() => remove(user)}
          />
        </div>
        {#if reset?.user === user.id}
          <div class="record-link">
            <Field label="one-time link" value={reset.link} readonly />
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

  <h2>new workspace</h2>
  <form onsubmit={createWorkspace}>
    <Field
      label="workspace name"
      bind:value={workspaceName}
      required
      problem={creation.field("name")}
    />
    {#if creation.problem}<p class="problem" role="alert">{creation.problem}</p>{/if}
    <button class="button" disabled={creation.busy}>create workspace</button>
  </form>
</Shell>
