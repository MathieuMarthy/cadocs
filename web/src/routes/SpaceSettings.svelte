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
    defaultRoles,
    type Group,
    higherRole,
    managesWorkspace,
    MAX_MEMBERS,
    type Member,
    roleName,
    SPACE_ROLES,
  } from "../lib/roles";
  import { navigate } from "../lib/router.svelte";
  import { app } from "../lib/session.svelte";
  import { currentLanguage, t } from "../lib/i18n";

  /** A row of `GET /spaces/{space}/grants`, to a user or to a group. */
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
    notify(t("account.saved"));
  }

  const accessName = (role: string | null) => (role ? roleName(role) : t("space.noAccess"));
  const defaultAccess = (role: string | null) => t("space.default", { role: accessName(role) });
  /** The fields a refusal of the grants can name. */
  const GRANT_FIELDS = { grants: "space.grants", role: "space.role" } as const;

  // Access: every group and member, with their grant, and the workspace admins who need none.
  let members = $state<Member[] | null>(null);
  let groups = $state<Group[]>([]);
  let grants = $state<Grant[]>([]);
  /** The members of each group that has a grant here, by group id. */
  let granted = $state<Map<string, Set<string>>>(new Map());
  const access = new Form();

  async function fetchAccess(id: string) {
    const [people, teams, rows] = await Promise.all([
      collect<Member>(`/workspaces/${workspace.id}/members`, MAX_MEMBERS),
      collect<Group>(`/workspaces/${workspace.id}/groups`, MAX_MEMBERS),
      collect<Grant>(`/spaces/${id}/grants`, MAX_MEMBERS),
    ]);
    const withGrant = rows.items.flatMap((grant) => (grant.group_id ? [grant.group_id] : []));
    const inside = await Promise.all(
      withGrant.map((group) => collect<Member>(`/groups/${group}/members`, MAX_MEMBERS)),
    );
    members = people.items;
    groups = teams.items;
    grants = rows.items;
    granted = new Map(withGrant.map((group, i) => [group, new Set(inside[i].items.map((m) => m.id))]));
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

  /** Who a grant goes to: a user or a group. */
  type Target = { user_id: string } | { group_id: string };

  const targets = (grant: Grant, target: Target) =>
    "user_id" in target ? grant.user_id === target.user_id : grant.group_id === target.group_id;

  const grantOf = (target: Target) => grants.find((grant) => targets(grant, target))?.role ?? "";

  /** The highest role a member gets from the groups granted here, and those groups' names. */
  function throughGroups(member: Member) {
    let role = "";
    let names: string[] = [];
    for (const grant of grants) {
      if (!grant.group_id || !granted.get(grant.group_id)?.has(member.id)) continue;
      const name = groups.find((group) => group.id === grant.group_id)?.name ?? "";
      if (grant.role === role) names.push(name);
      else if (higherRole(grant.role, role) === grant.role) [role, names] = [grant.role, [name]];
    }
    if (!role) return null;
    const list = new Intl.ListFormat(currentLanguage(), { type: "conjunction" });
    return t("space.through", { role: roleName(role), groups: list.format(names) });
  }

  const roleOptions = SPACE_ROLES.map((role) => ({ value: role, label: roleName(role) }));

  /** A member without a grant of their own gets their groups' grants, else the default role. */
  const memberOptions = (member: Member) => [
    {
      value: "",
      label: throughGroups(member) ? t("space.noOwnGrant") : defaultAccess(space?.default_role ?? null),
    },
    ...roleOptions,
  ];

  const groupOptions = $derived([{ value: "", label: t("space.noGrant") }, ...roleOptions]);

  /** `PUT` replaces every grant: the others go back as they were, this one changes. */
  async function setGrant(target: Target, select: HTMLSelectElement) {
    if (!space) return;
    const role = select.value;
    const before = grantOf(target);
    const id = space.id;
    const known = new Set((members ?? []).map((item) => item.id));
    const next = [
      ...grants
        .filter((grant) => !targets(grant, target))
        // A grant of someone who left the workspace would be refused as `unknown_target`.
        .filter((grant) => grant.group_id || known.has(grant.user_id!))
        .map((grant) =>
          grant.group_id
            ? { group_id: grant.group_id, role: grant.role }
            : { user_id: grant.user_id, role: grant.role },
        ),
      ...(role ? [{ ...target, role }] : []),
    ];
    const done = await access.run(() => api("PUT", `/spaces/${id}/grants`, { grants: next }));
    // The select keeps what was picked, and Svelte does not write back an unchanged role.
    if (!done) select.value = before;
    // Outside the form, so its refusal stays on screen.
    await fetchAccess(id).catch(() => {});
    if (!done) return;
    notify(t("space.accessChanged"));
    // This user's own role may be the one that changed, directly or through a group.
    if (!("user_id" in target) || target.user_id === app.session?.user.id) await nav.loadSpaces();
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
    notify(t("space.trashed"), async () => {
      await api("POST", `/spaces/${moved.id}/restore`);
      await nav.loadSpaces();
    });
  }
</script>

<Shell title={t("shell.settings")} {workspace} {crumbs}>
  {#if !nav.spacesLoaded}
    <h1>{t("shell.settings")}</h1>
    <p class="meta" role="status">{t("common.loading")}</p>
  {:else if !space}
    <h1>{t("shell.settings")}</h1>
    <p class="notice">{t("trash.noSpace")}</p>
  {:else}
    <h1>{t("space.settingsOf", { name: space.name })}</h1>

    <section aria-labelledby="space-heading">
      <h2 id="space-heading">{t("space.section")}</h2>
      {#if admin}
        <form onsubmit={save}>
          <Field label={t("form.name")} bind:value={name} required problem={general.field("name")} />
          <Field
            label={t("space.description")}
            bind:value={description}
            maxlength={1000}
            autocomplete="off"
            problem={general.field("description")}
          />
          <Select
            label={t("space.defaultRole")}
            bind:value={defaultRole}
            options={defaultRoles()}
            problem={general.field("default_role")}
          />
          <p class="meta">{t("space.defaultRoleHint")}</p>
          {#if general.problem}<p class="problem" role="alert">{general.problem}</p>{/if}
          <button class="button" disabled={general.busy}>{t("account.save")}</button>
        </form>
      {:else}
        {#if space.description}<p class="notice">{space.description}</p>{/if}
        <p class="notice">{t("space.defaultRoleIs", { role: accessName(space.default_role) })}</p>
        <p class="meta">{t("space.adminsOnly")}</p>
      {/if}
    </section>

    <section aria-labelledby="access-heading">
      <h2 id="access-heading">{t("space.access")}</h2>
      <p class="meta">{t("space.accessHint")}</p>
      {#if access.problem}<p class="problem" role="alert">{access.problem}</p>{/if}
      {#each Object.entries(GRANT_FIELDS) as [field, key] (field)}
        {#if access.field(field)}
          <p class="problem" role="alert">{t("space.fieldProblem", { field: t(key), problem: access.field(field) ?? "" })}</p>
        {/if}
      {/each}
      {#if members === null}
        {#if !access.problem}<p class="meta" role="status">{t("common.loading")}</p>{/if}
      {:else}
        {#if groups.length > 0}
          <ul class="records">
            {#each groups as group (group.id)}
              <li class="record">
                <div class="record-who">
                  {group.name}
                  <span>{t("space.group", { count: group.member_count })}</span>
                </div>
                {#if admin}
                  <select
                    class="select"
                    aria-label={t("space.accessOfGroup", { name: group.name })}
                    value={grantOf({ group_id: group.id })}
                    disabled={access.busy}
                    onchange={(event) => void setGrant({ group_id: group.id }, event.currentTarget)}
                  >
                    {#each groupOptions as option (option.value)}
                      <option value={option.value}>{option.label}</option>
                    {/each}
                  </select>
                {:else}
                  <span class="badges">
                    <span class="badge">
                      {grantOf({ group_id: group.id }) ? roleName(grantOf({ group_id: group.id })) : t("space.noGrant")}
                    </span>
                  </span>
                {/if}
              </li>
            {/each}
          </ul>
        {/if}
        <ul class="records">
          {#each members as member (member.id)}
            <li class="record">
              <div class="record-who">
                {member.name}{#if member.id === app.session?.user.id}&nbsp;<span class="you">{t("admin.you")}</span
                  >{/if}
                <span>{member.email}</span>
                {#if !managesWorkspace(member.role) && throughGroups(member)}<span>{throughGroups(member)}</span>{/if}
              </div>
              {#if managesWorkspace(member.role)}
                <span class="badges"><span class="badge">{t("space.workspaceAdmin")}</span></span>
              {:else if admin}
                <select
                  class="select"
                  aria-label={t("space.accessOf", { name: member.name })}
                  value={grantOf({ user_id: member.id })}
                  disabled={access.busy}
                  onchange={(event) => void setGrant({ user_id: member.id }, event.currentTarget)}
                >
                  {#each memberOptions(member) as option (option.value)}
                    <option value={option.value}>{option.label}</option>
                  {/each}
                </select>
              {:else}
                <span class="badges">
                  <span class="badge">
                    {#if grantOf({ user_id: member.id })}
                      {roleName(grantOf({ user_id: member.id }))}
                    {:else if throughGroups(member)}
                      {t("space.noOwnGrant")}
                    {:else}
                      {defaultAccess(space.default_role)}
                    {/if}
                  </span>
                </span>
              {/if}
            </li>
          {/each}
        </ul>
      {/if}
    </section>

    {#if admin}
      <section aria-labelledby="trash-heading">
        <h2 id="trash-heading">{t("trash.title")}</h2>
        <p class="meta">{t("space.trashHint")}</p>
        {#if trashing.problem}<p class="problem" role="alert">{trashing.problem}</p>{/if}
        <button class="button" type="button" disabled={trashing.busy} onclick={() => void trash()}>
          {t("page.trash")}
        </button>
      </section>
    {/if}
  {/if}
</Shell>
