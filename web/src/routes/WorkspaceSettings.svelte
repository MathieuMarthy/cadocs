<script lang="ts">
  import { onDestroy, untrack } from "svelte";

  import Confirm from "../components/Confirm.svelte";
  import Field from "../components/Field.svelte";
  import Groups from "../components/Groups.svelte";
  import IconButton from "../components/IconButton.svelte";
  import Select from "../components/Select.svelte";
  import Shell from "../components/Shell.svelte";
  import Time from "../components/Time.svelte";
  import { copy } from "../lib/actions";
  import { api, type Workspace } from "../lib/api";
  import { Form } from "../lib/form.svelte";
  import { acquireNav, collect, type Space } from "../lib/nav.svelte";
  import { notify } from "../lib/notice.svelte";
  import {
    languageName,
    managesWorkspace,
    MAX_MEMBERS,
    type Member,
    roleName,
    stemmingLanguages,
  } from "../lib/roles";
  import { navigate } from "../lib/router.svelte";
  import { app, refreshSession } from "../lib/session.svelte";
  import { t } from "../lib/i18n";

  interface Invitation {
    id: string;
    email: string;
    role: string;
    invited_by: string;
    expires_at: string;
    created_at: string;
  }

  let { workspace }: { workspace: Workspace } = $props();

  // App keys this screen by workspace, so the id never changes under it.
  // svelte-ignore state_referenced_locally
  const held = acquireNav(app.session!.user.id, workspace);
  const nav = held.nav;
  onDestroy(held.release);

  const me = $derived(app.session!.user.id);
  // The session carries the role, and `access.changed` refreshes it.
  const manages = $derived(managesWorkspace(workspace.role));
  const owner = $derived(workspace.role === "owner");

  // General settings.
  let name = $state("");
  let language = $state("en");
  const general = new Form();
  let generalLoaded = $state(false);

  // Filled once: a refreshed session, after `access.changed`, must not undo what is typed.
  // App keys this screen by workspace, so the id never changes under it.
  // svelte-ignore state_referenced_locally
  const generalId = workspace.id;
  $effect(() => {
    const id = generalId;
    // A form's busy state is read by `run`, which must not make this effect run again.
    untrack(() =>
      general.run(async () => {
        const row = await api<{ name: string; language: string }>("GET", `/workspaces/${id}`);
        name = row.name;
        language = row.language;
        generalLoaded = true;
      }),
    );
  });

  async function save(event: SubmitEvent) {
    event.preventDefault();
    const done = await general.run(() =>
      api("PATCH", `/workspaces/${workspace.id}`, { name, language }),
    );
    if (!done) return;
    await refreshSession();
    notify(t("account.saved"));
  }

  // Members.
  let members = $state<Member[] | null>(null);
  const memberForm = new Form();
  /** Members removed from here, which the groups count. */
  let left = $state(0);

  const fetchMembers = async () =>
    (members = (await collect<Member>(`/workspaces/${workspace.id}/members`, MAX_MEMBERS)).items);

  async function loadMembers() {
    await memberForm.run(fetchMembers);
  }

  $effect(() => {
    // A change of role, which `access.changed` reports to this user, may change what shows.
    void workspace.role;
    untrack(loadMembers);
  });

  /** Owners are changed only by owners, and nobody changes themself from here: they leave. */
  const editable = (member: Member) =>
    manages && member.id !== me && (owner || member.role !== "owner");

  const roleOptions = $derived(
    (owner ? ["member", "admin", "owner"] : ["member", "admin"]).map((role) => ({
      value: role,
      label: roleName(role),
    })),
  );

  async function changeRole(member: Member, select: HTMLSelectElement) {
    const role = select.value;
    const done = await memberForm.run(async () => {
      const updated = await api<Member>("PATCH", `/workspaces/${workspace.id}/members/${member.id}`, {
        role,
      });
      members = (members ?? []).map((item) => (item.id === updated.id ? updated : item));
    });
    if (done) {
      notify(t("workspace.roleChanged"));
      return;
    }
    // The select keeps what was picked, and Svelte does not write back an unchanged role.
    select.value = member.role;
    // Outside the form, so its refusal stays on screen.
    await fetchMembers().catch(() => {});
  }

  async function remove(member: Member) {
    const done = await memberForm.run(() =>
      api("DELETE", `/workspaces/${workspace.id}/members/${member.id}`),
    );
    if (!done) return;
    members = (members ?? []).filter((item) => item.id !== member.id);
    // They left the workspace's groups too.
    left += 1;
  }

  // Invitations.
  let invitations = $state<Invitation[] | null>(null);
  let invited = $state<{ email: string; link: string } | null>(null);
  let email = $state("");
  let invitedRole = $state("member");
  const invitationForm = new Form();
  const invitationList = new Form();

  async function loadInvitations() {
    await invitationList.run(async () => {
      const path = `/workspaces/${workspace.id}/invitations`;
      invitations = (await collect<Invitation>(path, MAX_MEMBERS)).items;
    });
  }

  $effect(() => {
    if (manages) untrack(loadInvitations);
  });

  async function invite(event: SubmitEvent) {
    event.preventDefault();
    await invitationForm.run(async () => {
      const created = await api<Invitation & { link: string }>(
        "POST",
        `/workspaces/${workspace.id}/invitations`,
        { email, role: invitedRole },
      );
      invited = { email: created.email, link: created.link };
      email = "";
    });
    await loadInvitations();
  }

  async function revoke(invitation: Invitation) {
    const done = await invitationList.run(() => api("DELETE", `/invitations/${invitation.id}`));
    if (!done) return;
    invitations = (invitations ?? []).filter((item) => item.id !== invitation.id);
    if (invited?.email === invitation.email) invited = null;
  }

  // Trashed spaces, which only their admins see.
  let trashed = $state<Space[]>([]);
  const trashForm = new Form();

  $effect(() => {
    // A space trashed or restored elsewhere changes the live list too.
    void nav.spaces;
    const id = workspace.id;
    untrack(() =>
      trashForm.run(async () => {
        trashed = (await collect<Space>(`/workspaces/${id}/spaces/trash`, MAX_MEMBERS)).items;
      }),
    );
  });

  async function restore(space: Space) {
    const done = await trashForm.run(() => api("POST", `/spaces/${space.id}/restore`));
    if (!done) return;
    trashed = trashed.filter((item) => item.id !== space.id);
    await nav.loadSpaces();
  }

  // Leaving and deleting, after which this screen has nothing left to show.
  const leaving = new Form();

  async function leave() {
    const done = await leaving.run(() =>
      api("DELETE", `/workspaces/${workspace.id}/members/${me}`),
    );
    if (!done) return;
    await refreshSession();
    navigate("/");
  }

  let typed = $state("");
  const deleting = new Form();

  async function deleteWorkspace(event: SubmitEvent) {
    event.preventDefault();
    const done = await deleting.run(() =>
      api("DELETE", `/workspaces/${workspace.id}`, { name: typed }),
    );
    if (!done) return;
    await refreshSession();
    navigate("/");
  }
</script>

<Shell title={t("shell.settings")} {workspace}>
  <h1>{t("space.settingsOf", { name: workspace.name })}</h1>

  <section aria-labelledby="general-heading">
    <h2 id="general-heading">{t("shell.workspace")}</h2>
    {#if !generalLoaded}
      {#if general.problem}
        <p class="problem" role="alert">{general.problem}</p>
      {:else}
        <p class="meta" role="status">{t("common.loading")}</p>
      {/if}
    {:else if manages}
      <form onsubmit={save}>
        <Field label={t("form.name")} bind:value={name} required problem={general.field("name")} />
        <Select
          label={t("workspace.language")}
          bind:value={language}
          options={stemmingLanguages()}
          problem={general.field("language")}
        />
        <p class="meta">{t("workspace.languageHint")}</p>
        {#if general.problem}<p class="problem" role="alert">{general.problem}</p>{/if}
        <button class="button" disabled={general.busy}>{t("account.save")}</button>
      </form>
    {:else}
      <p class="notice">{t("workspace.languageIs", { language: languageName(language) })}</p>
      <p class="meta">{t("workspace.adminsOnly")}</p>
    {/if}
  </section>

  <section aria-labelledby="members-heading">
    <h2 id="members-heading">{t("workspace.members")}</h2>
    {#if memberForm.problem}<p class="problem" role="alert">{memberForm.problem}</p>{/if}
    {#if members === null}
      {#if !memberForm.problem}<p class="meta" role="status">{t("common.loading")}</p>{/if}
    {:else}
      <ul class="records">
        {#each members as member (member.id)}
          <li class="record">
            <div class="record-who">
              {member.name}{#if member.id === me}&nbsp;<span class="you">{t("admin.you")}</span>{/if}
              <span>{member.email}</span>
            </div>
            {#if editable(member)}
              <select
                class="select"
                aria-label={t("workspace.roleOf", { name: member.name })}
                value={member.role}
                disabled={memberForm.busy}
                onchange={(event) => void changeRole(member, event.currentTarget)}
              >
                {#each roleOptions as option (option.value)}
                  <option value={option.value}>{option.label}</option>
                {/each}
              </select>
              {#if member.disabled}<span class="badges"><span class="badge">{t("admin.disabled")}</span></span>{/if}
              <Confirm
                label={t("workspace.remove")}
                confirm={t("workspace.removeConfirm", { name: member.name })}
                disabled={memberForm.busy}
                onconfirm={() => void remove(member)}
              />
            {:else}
              <span class="badges">
                <span class="badge">{roleName(member.role)}</span>
                {#if member.disabled}<span class="badge">{t("admin.disabled")}</span>{/if}
              </span>
            {/if}
          </li>
        {/each}
      </ul>
    {/if}
  </section>

  <Groups {workspace} {manages} {members} changed={left} />

  {#if manages}
    <section aria-labelledby="invitations-heading">
      <h2 id="invitations-heading">{t("workspace.invitations")}</h2>
      <p class="meta">{t("workspace.invitationsHint")}</p>
      <form onsubmit={invite}>
        <Field
          label={t("form.email")}
          type="email"
          bind:value={email}
          required
          autocomplete="off"
          problem={invitationForm.field("email")}
        />
        <Select
          label={t("workspace.role")}
          bind:value={invitedRole}
          options={["member", "admin"].map((value) => ({ value, label: roleName(value) }))}
          problem={invitationForm.field("role")}
        />
        {#if invitationForm.problem}
          <p class="problem" role="alert">{invitationForm.problem}</p>
        {/if}
        <button class="button" disabled={invitationForm.busy}>{t("workspace.invite")}</button>
      </form>
      {#if invited}
        <div class="reset-link">
          <Field label={t("workspace.invitationLink")} value={invited.link} readonly />
          <IconButton
            icon="copy"
            verb={t("admin.copy")}
            label={t("workspace.copyInvitation")}
            onclick={() => invited && void copy(invited.link)}
          />
        </div>
        <p class="meta">{t("workspace.invitedFor", { email: invited.email })}</p>
      {/if}
      {#if invitationList.problem}<p class="problem" role="alert">{invitationList.problem}</p>{/if}
      {#if invitations && invitations.length > 0}
        <ul class="records">
          {#each invitations as invitation (invitation.id)}
            <li class="record">
              <div class="record-who">
                {invitation.email}
                <span>{t("workspace.expires")} <Time value={invitation.expires_at} /></span>
              </div>
              <span class="badges"><span class="badge">{roleName(invitation.role)}</span></span>
              <IconButton
                icon="x"
                verb={t("workspace.revoke")}
                label={t("workspace.revokeOf", { email: invitation.email })}
                disabled={invitationList.busy}
                onclick={() => void revoke(invitation)}
              />
            </li>
          {/each}
        </ul>
      {:else if invitations}
        <p class="meta">{t("workspace.noInvitations")}</p>
      {/if}
    </section>
  {/if}

  {#if trashed.length > 0 || trashForm.problem}
    <section aria-labelledby="trashed-heading">
      <h2 id="trashed-heading">{t("workspace.trashed")}</h2>
      {#if trashForm.problem}<p class="problem" role="alert">{trashForm.problem}</p>{/if}
      <ul class="records">
        {#each trashed as space (space.id)}
          <li class="record">
            <span class="record-who">
              {space.name}
              <span>{t("space.trashed")} <Time value={space.deleted_at ?? space.updated_at} /></span>
            </span>
            <IconButton
              icon="rotate-ccw"
              verb={t("trash.restore")}
              label={t("workspace.restoreSpace", { name: space.name })}
              disabled={trashForm.busy}
              onclick={() => void restore(space)}
            />
          </li>
        {/each}
      </ul>
    </section>
  {/if}

  <section aria-labelledby="leave-heading">
    <h2 id="leave-heading">{t("workspace.leave")}</h2>
    <p class="meta">{t("workspace.leaveHint")}</p>
    {#if leaving.problem}<p class="problem" role="alert">{leaving.problem}</p>{/if}
    <div class="actions">
      <Confirm
        label={t("workspace.leaveLabel")}
        confirm={t("workspace.leave")}
        disabled={leaving.busy}
        onconfirm={() => void leave()}
      />
    </div>
  </section>

  {#if owner}
    <section aria-labelledby="delete-heading">
      <h2 id="delete-heading">{t("admin.delete")}</h2>
      <p class="meta">{t("workspace.deleteHint")}</p>
      <form onsubmit={deleteWorkspace}>
        <Field
          label={t("workspace.deleteField")}
          bind:value={typed}
          autocomplete="off"
          problem={deleting.field("name")}
        />
        {#if deleting.problem}<p class="problem" role="alert">{deleting.problem}</p>{/if}
        <button class="button danger" disabled={deleting.busy || typed !== workspace.name}>
          {t("workspace.delete")}
        </button>
      </form>
    </section>
  {/if}
</Shell>
