<script lang="ts">
  import { untrack } from "svelte";

  import { api, type Workspace } from "../lib/api";
  import { t } from "../lib/i18n";
  import { describe, Form } from "../lib/form.svelte";
  import { collect } from "../lib/nav.svelte";
  import { type Group, MAX_MEMBERS, type Member } from "../lib/roles";
  import Confirm from "./Confirm.svelte";
  import Field from "./Field.svelte";
  import IconButton from "./IconButton.svelte";
  import Select from "./Select.svelte";

  /** The groups of a workspace: every member reads them, its admins change them. */
  let {
    workspace,
    manages,
    members,
    changed,
  }: {
    workspace: Workspace;
    manages: boolean;
    /** The workspace's members, those a group can take. */
    members: Member[] | null;
    /** Bumped when the workspace loses a member, who leaves its groups too. */
    changed: number;
  } = $props();

  let groups = $state<Group[] | null>(null);
  const list = new Form();

  // The group whose members show, with what its admin types.
  let open = $state<string | null>(null);
  let inside = $state<Member[] | null>(null);
  let renamed = $state("");
  let picked = $state("");
  const editing = new Form();

  let name = $state("");
  const creating = new Form();

  // Reads overlap, and only the last one started may write, so an older answer cannot win.
  let groupReads = 0;
  let insideReads = 0;

  async function fetchGroups() {
    const read = ++groupReads;
    const rows = (await collect<Group>(`/workspaces/${workspace.id}/groups`, MAX_MEMBERS)).items;
    if (read === groupReads) groups = rows;
  }

  /** Only the group still open writes its members, so a slow answer cannot show another's. */
  async function fetchInside(id: string) {
    const read = ++insideReads;
    const rows = (await collect<Member>(`/groups/${id}/members`, MAX_MEMBERS)).items;
    if (read === insideReads && open === id) inside = rows;
  }

  async function refetch() {
    await fetchGroups();
    if (open) await fetchInside(open);
  }

  $effect(() => {
    if (changed === 0) untrack(() => list.run(fetchGroups));
    // Outside the form, which an action of the admin may hold.
    else untrack(() => void refetch().catch(() => {}));
  });

  const candidates = $derived(
    (members ?? []).filter((member) => !(inside ?? []).some((item) => item.id === member.id)),
  );

  $effect(() => {
    // The first one left, so that a pick always names someone.
    if (!candidates.some((member) => member.id === picked)) picked = candidates[0]?.id ?? "";
  });

  async function toggle(group: Group) {
    if (open === group.id) {
      open = null;
      return;
    }
    open = group.id;
    inside = null;
    renamed = group.name;
    editing.problem = null;
    editing.fields = {};
    // Outside the form, which another read may hold.
    const id = group.id;
    list.problem = null;
    await fetchInside(id).catch((error) => {
      if (open === id) list.problem = describe(error);
    });
  }

  async function create(event: SubmitEvent) {
    event.preventDefault();
    const done = await creating.run(async () => {
      await api("POST", `/workspaces/${workspace.id}/groups`, { name });
      name = "";
    });
    if (!done) return;
    list.problem = null;
    await fetchGroups().catch((error) => (list.problem = describe(error)));
  }

  /** Runs a change to the open group, then reads it again, refused or not. */
  async function change(action: () => Promise<unknown>) {
    await editing.run(action);
    await refetch().catch(() => {});
  }

  function rename(event: SubmitEvent, group: Group) {
    event.preventDefault();
    return change(() => api("PATCH", `/groups/${group.id}`, { name: renamed }));
  }

  const add = (group: Group) =>
    change(() => api("PUT", `/groups/${group.id}/members/${picked}`));

  const removeFrom = (group: Group, member: Member) =>
    change(() => api("DELETE", `/groups/${group.id}/members/${member.id}`));

  async function remove(group: Group) {
    const done = await editing.run(() => api("DELETE", `/groups/${group.id}`));
    if (done) open = null;
    await fetchGroups().catch(() => {});
  }
</script>

<section aria-labelledby="groups-heading">
  <h2 id="groups-heading">{t("groups.title")}</h2>
  <p class="meta">{t("groups.hint")}</p>
  {#if manages}
    <form onsubmit={create}>
      <Field
        label={t("groups.name")}
        bind:value={name}
        required
        maxlength={100}
        autocomplete="off"
        problem={creating.field("name")}
      />
      {#if creating.problem}<p class="problem" role="alert">{creating.problem}</p>{/if}
      <button class="button" disabled={creating.busy}>{t("groups.create")}</button>
    </form>
  {/if}
  {#if list.problem}<p class="problem" role="alert">{list.problem}</p>{/if}
  {#if groups === null}
    {#if !list.problem}<p class="meta" role="status">{t("common.loading")}</p>{/if}
  {:else if groups.length === 0}
    <p class="meta">{t("groups.none")}</p>
  {:else}
    <ul class="records">
      {#each groups as group (group.id)}
        <li class="record">
          <div class="record-who">
            {group.name}
            <span>{t("groups.count", { count: group.member_count })}</span>
          </div>
          <button
            class="button secondary"
            type="button"
            aria-expanded={open === group.id}
            aria-label={t("groups.membersOf", { name: group.name })}
            onclick={() => void toggle(group)}
          >
            {t("workspace.members")}
          </button>
          {#if open === group.id}
            <div class="group">
              {#if inside === null}
                {#if !list.problem}<p class="meta" role="status">{t("common.loading")}</p>{/if}
              {:else if inside.length === 0}
                <p class="meta">{t("groups.empty")}</p>
              {:else}
                <ul class="records">
                  {#each inside as member (member.id)}
                    <li class="record">
                      <div class="record-who">
                        {member.name}
                        <span>{member.email}</span>
                      </div>
                      {#if manages}
                        <IconButton
                          icon="x"
                          verb={t("workspace.remove")}
                          label={t("groups.removeFrom", { member: member.name, group: group.name })}
                          disabled={editing.busy}
                          onclick={() => void removeFrom(group, member)}
                        />
                      {/if}
                    </li>
                  {/each}
                </ul>
              {/if}
              {#if manages && inside !== null}
                {#if editing.problem}<p class="problem" role="alert">{editing.problem}</p>{/if}
                {#if members === null}
                  <!-- Who could join is unknown: the members section says why. -->
                {:else if candidates.length > 0}
                  <div class="actions">
                    <Select
                      label={t("groups.addTo", { name: group.name })}
                      bind:value={picked}
                      options={candidates.map((member) => ({ value: member.id, label: member.name }))}
                    />
                    <button
                      class="button"
                      type="button"
                      disabled={editing.busy || !picked}
                      onclick={() => void add(group)}
                    >
                      {t("groups.add")}
                    </button>
                  </div>
                {:else}
                  <p class="meta">{t("groups.everyone")}</p>
                {/if}
                <form class="actions" onsubmit={(event) => void rename(event, group)}>
                  <Field
                    label={t("groups.nameOf", { name: group.name })}
                    bind:value={renamed}
                    required
                    maxlength={100}
                    autocomplete="off"
                    problem={editing.field("name")}
                  />
                  <button class="button" disabled={editing.busy}>{t("groups.rename")}</button>
                </form>
                <div class="actions">
                  <Confirm
                    label={t("groups.delete")}
                    confirm={t("admin.deleteConfirm", { name: group.name })}
                    danger
                    disabled={editing.busy}
                    onconfirm={() => void remove(group)}
                  />
                </div>
              {/if}
            </div>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
</section>

<style>
  .group {
    flex: 1 1 100%;
    padding-left: var(--space-4);
  }

  .group .actions {
    align-items: flex-end;
    margin-bottom: var(--space-4);
  }

  .group .actions :global(.field) {
    margin-bottom: 0;
  }
</style>
