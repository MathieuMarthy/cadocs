<script lang="ts">
  import { onDestroy, untrack } from "svelte";

  import Field from "../components/Field.svelte";
  import Select from "../components/Select.svelte";
  import Shell from "../components/Shell.svelte";
  import { api, type Workspace } from "../lib/api";
  import { Form } from "../lib/form.svelte";
  import { acquireNav, collect, type Space } from "../lib/nav.svelte";
  import { notify } from "../lib/notice.svelte";
  import {
    DEFAULT_ROLES,
    managesWorkspace,
    MAX_MEMBERS,
    type Member,
    SPACE_ROLES,
  } from "../lib/roles";
  import { navigate } from "../lib/router.svelte";
  import { app } from "../lib/session.svelte";

  /** A row of `GET /spaces/{space}/grants`: groups come in V1, and are kept as they are. */
  interface Grant {
    id: string;
    user_id: string | null;
    group_id: string | null;
    role: string;
    created_at: string;
  }

  let { workspace, slug }: { workspace: Workspace; slug: string } = $props();

  // App keys this screen by workspace and space, so the props never change under it.
  // svelte-ignore state_referenced_locally
  const held = acquireNav(app.session!.user.id, workspace);
  const nav = held.nav;
  onDestroy(held.release);

  const space = $derived(nav.spaces.find((item) => item.slug === slug) ?? null);
  const spaceId = $derived(space?.id ?? null);
  /** A string, so that a new list of spaces with the same role reads nothing again. */
  const spaceRole = $derived(space?.role ?? null);
  const admin = $derived(spaceRole === "admin");
  const crumbs = $derived(space ? [{ label: space.name }] : []);

  // General settings, filled once from the space, so that an event does not undo typing.
  let name = $state("");
  let description = $state("");
  let defaultRole = $state("");
  let filled: string | null = null;
  const general = new Form();

  $effect(() => {
    if (!space || filled === space.id) return;
    filled = space.id;
    name = space.name;
    description = space.description ?? "";
    defaultRole = space.default_role ?? "";
  });

  async function save(event: SubmitEvent) {
    event.preventDefault();
    if (!space) return;
    const id = space.id;
    const body = { name, description, default_role: defaultRole || null };
    const done = await general.run(() => api<Space>("PATCH", `/spaces/${id}`, body));
    if (!done) return;
    await nav.loadSpaces();
    notify("saved");
  }

  const roleName = (role: string | null) => role ?? "no access";

  // Access: every member, with their grant, and the workspace admins who need none.
  let members = $state<Member[] | null>(null);
  let grants = $state<Grant[]>([]);
  const access = new Form();

  async function fetchAccess(id: string) {
    const [people, rows] = await Promise.all([
      collect<Member>(`/workspaces/${workspace.id}/members`, MAX_MEMBERS),
      collect<Grant>(`/spaces/${id}/grants`, MAX_MEMBERS),
    ]);
    members = people.items;
    grants = rows.items;
  }

  async function loadAccess(id: string) {
    await access.run(() => fetchAccess(id));
  }

  $effect(() => {
    if (!spaceId) return;
    // A new role for this user, after `access.changed`, may change who they can edit.
    void spaceRole;
    const id = spaceId;
    untrack(() => loadAccess(id));
  });

  const grantOf = (member: Member) =>
    grants.find((grant) => grant.user_id === member.id)?.role ?? "";

  const accessOptions = $derived([
    { value: "", label: `default: ${roleName(space?.default_role ?? null)}` },
    ...SPACE_ROLES.map((role) => ({ value: role, label: role })),
  ]);

  /** `PUT` replaces every grant: the others go back as they were, this one changes. */
  async function setGrant(member: Member, select: HTMLSelectElement) {
    if (!space) return;
    const role = select.value;
    const before = grantOf(member);
    const id = space.id;
    const known = new Set((members ?? []).map((item) => item.id));
    const next = [
      ...grants
        .filter((grant) => grant.group_id || (grant.user_id && grant.user_id !== member.id))
        // A grant of someone who left the workspace would be refused as `unknown_target`.
        .filter((grant) => grant.group_id || known.has(grant.user_id!))
        .map((grant) =>
          grant.group_id
            ? { group_id: grant.group_id, role: grant.role }
            : { user_id: grant.user_id, role: grant.role },
        ),
      ...(role ? [{ user_id: member.id, role }] : []),
    ];
    const done = await access.run(() => api("PUT", `/spaces/${id}/grants`, { grants: next }));
    // The select keeps what was picked, and Svelte does not write back an unchanged role.
    if (!done) select.value = before;
    // Outside the form, so its refusal stays on screen.
    await fetchAccess(id).catch(() => {});
    if (!done) return;
    notify("access changed");
    // This user's own role may be the one that changed.
    if (member.id === app.session?.user.id) await nav.loadSpaces();
  }

  // Trash.
  const trashing = new Form();

  async function trash() {
    if (!space) return;
    const moved = space;
    const done = await trashing.run(() => api("DELETE", `/spaces/${moved.id}`));
    if (!done) return;
    await nav.loadSpaces();
    navigate(`/w/${workspace.slug}`);
    notify("moved to trash", async () => {
      await api("POST", `/spaces/${moved.id}/restore`);
      await nav.loadSpaces();
    });
  }
</script>

<Shell title="settings" {workspace} {crumbs}>
  {#if !nav.spacesLoaded}
    <h1>settings</h1>
    <p class="meta" role="status">loading…</p>
  {:else if !space}
    <h1>settings</h1>
    <p class="notice">no such space</p>
  {:else}
    <h1>{space.name} settings</h1>

    <section aria-labelledby="space-heading">
      <h2 id="space-heading">space</h2>
      {#if admin}
        <form onsubmit={save}>
          <Field label="name" bind:value={name} required problem={general.field("name")} />
          <Field
            label="description"
            bind:value={description}
            maxlength={1000}
            autocomplete="off"
            problem={general.field("description")}
          />
          <Select
            label="default role"
            bind:value={defaultRole}
            options={DEFAULT_ROLES}
            problem={general.field("default_role")}
          />
          <p class="meta">what a workspace member without a grant gets.</p>
          {#if general.problem}<p class="problem" role="alert">{general.problem}</p>{/if}
          <button class="button" disabled={general.busy}>save</button>
        </form>
      {:else}
        {#if space.description}<p class="notice">{space.description}</p>{/if}
        <p class="notice">default role: {roleName(space.default_role)}</p>
        <p class="meta">only space admins change these settings.</p>
      {/if}
    </section>

    <section aria-labelledby="access-heading">
      <h2 id="access-heading">access</h2>
      <p class="meta">a grant replaces the default role, even when it is lower.</p>
      {#if access.problem}<p class="problem" role="alert">{access.problem}</p>{/if}
      {#each ["grants", "role"] as field (field)}
        {#if access.field(field)}
          <p class="problem" role="alert">{field}: {access.field(field)}</p>
        {/if}
      {/each}
      {#if members === null}
        {#if !access.problem}<p class="meta" role="status">loading…</p>{/if}
      {:else}
        <ul class="records">
          {#each members as member (member.id)}
            <li class="record">
              <div class="record-who">
                {member.name}{#if member.id === app.session?.user.id}&nbsp;<span class="you">(you)</span
                  >{/if}
                <span>{member.email}</span>
              </div>
              {#if managesWorkspace(member.role)}
                <span class="badges"><span class="badge">workspace admin</span></span>
              {:else if admin}
                <select
                  class="select"
                  aria-label="access of {member.name}"
                  value={grantOf(member)}
                  disabled={access.busy}
                  onchange={(event) => void setGrant(member, event.currentTarget)}
                >
                  {#each accessOptions as option (option.value)}
                    <option value={option.value}>{option.label}</option>
                  {/each}
                </select>
              {:else}
                <span class="badges">
                  <span class="badge">{grantOf(member) || `default: ${roleName(space.default_role)}`}</span>
                </span>
              {/if}
            </li>
          {/each}
        </ul>
      {/if}
    </section>

    {#if admin}
      <section aria-labelledby="trash-heading">
        <h2 id="trash-heading">trash</h2>
        <p class="meta">its pages leave the tree for everyone until a space admin restores it.</p>
        {#if trashing.problem}<p class="problem" role="alert">{trashing.problem}</p>{/if}
        <button class="button" type="button" disabled={trashing.busy} onclick={() => void trash()}>
          move to trash
        </button>
      </section>
    {/if}
  {/if}
</Shell>
