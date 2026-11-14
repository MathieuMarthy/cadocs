<script lang="ts">
  import Message from "./routes/Message.svelte";
  import Account from "./routes/Account.svelte";
  import History from "./routes/History.svelte";
  import Admin from "./routes/Admin.svelte";
  import Invite from "./routes/Invite.svelte";
  import Login from "./routes/Login.svelte";
  import ResetPassword from "./routes/ResetPassword.svelte";
  import Search from "./routes/Search.svelte";
  import Setup from "./routes/Setup.svelte";
  import NewWorkspace from "./components/NewWorkspace.svelte";
  import Notice from "./components/Notice.svelte";
  import Shell from "./components/Shell.svelte";
  import SpaceSettings from "./routes/SpaceSettings.svelte";
  import Trash from "./routes/Trash.svelte";
  import Workspace from "./routes/Workspace.svelte";
  import WorkspaceSettings from "./routes/WorkspaceSettings.svelte";
  import type { Workspace as WorkspaceRow } from "./lib/api";
  import { navigate, reopened, route, safeNext } from "./lib/router.svelte";
  import { app, boot } from "./lib/session.svelte";

  type View =
    | { name: "redirect"; to: string }
    | { name: "login" | "account" | "admin" | "home" | "not-found" }
    | { name: "setup"; code: string }
    | { name: "invite" | "reset"; token: string }
    | { name: "workspace" | "settings" | "search"; workspace: WorkspaceRow }
    | { name: "space-settings"; workspace: WorkspaceRow; space: string }
    | { name: "trash"; workspace: WorkspaceRow; space: string }
    | { name: "page"; id: string }
    | { name: "history"; id: string; revision: number | null };

  function resolve(path: string, search: string): View {
    const [first, second, ...rest] = path.split("/").filter(Boolean);
    const query = new URLSearchParams(search);

    // A link in a message works whatever state this browser is in.
    if (first === "invite" && second && rest.length === 0) return { name: "invite", token: second };
    if (first === "reset-password" && second && rest.length === 0) {
      return { name: "reset", token: second };
    }

    if (app.setupRequired) {
      // The link in the server's log carries the code (ADR 0021). Without it, the page asks.
      if (first === "setup" && rest.length === 0) return { name: "setup", code: second ?? "" };
      return { name: "redirect", to: "/setup" };
    }

    const session = app.session;
    if (!session) {
      if (first === "login") return { name: "login" };
      const here = path + search;
      const next = here === "/" || first === "setup" ? "" : `?next=${encodeURIComponent(here)}`;
      return { name: "redirect", to: `/login${next}` };
    }

    if (first === "login" || first === "setup") {
      return { name: "redirect", to: safeNext(query.get("next")) };
    }
    if (!first) {
      const page = query.get("page");
      if (page && query.has("history")) {
        const revision = Number(query.get("history"));
        return { name: "history", id: page, revision: Number.isInteger(revision) && revision > 0 ? revision : null };
      }
      if (page) return { name: "page", id: page };
      const workspace = session.workspaces[0];
      return workspace ? { name: "redirect", to: `/w/${workspace.slug}` } : { name: "home" };
    }
    if (first === "account" && !second) return { name: "account" };
    if (first === "admin" && !second && session.user.is_instance_admin) return { name: "admin" };
    if (first === "w" && second) {
      const workspace = session.workspaces.find((item) => item.slug === second);
      if (workspace && rest.length === 0) return { name: "workspace", workspace };
      if (workspace && rest[0] === "trash" && rest[1] && rest.length === 2) {
        return { name: "trash", workspace, space: rest[1] };
      }
      if (workspace && rest[0] === "search" && rest.length === 1) {
        return { name: "search", workspace };
      }
      if (workspace && rest[0] === "settings" && rest.length === 1) {
        return { name: "settings", workspace };
      }
      if (workspace && rest[0] === "settings" && rest[1] && rest.length === 2) {
        return { name: "space-settings", workspace, space: rest[1] };
      }
    }
    return { name: "not-found" };
  }

  const view = $derived(resolve(route.path, route.search));

  $effect(() => {
    if (app.phase === "ready" && view.name === "redirect") navigate(view.to, { replace: true });
  });
</script>

{#if app.phase === "loading"}
  <p class="meta" role="status" style="padding: var(--space-4)">loading…</p>
{:else if app.phase === "failed"}
  <Message title="server unreachable">
    <button class="button" type="button" onclick={boot}>retry</button>
  </Message>
{:else if view.name === "setup"}
  <Setup code={view.code} />
{:else if view.name === "login"}
  <Login />
{:else if view.name === "invite"}
  {#key view.token}<Invite token={view.token} />{/key}
{:else if view.name === "reset"}
  {#key view.token}<ResetPassword token={view.token} />{/key}
{:else if view.name === "account"}
  <Account />
{:else if view.name === "admin"}
  <Admin />
{:else if view.name === "workspace"}
  {#key view.workspace.id}<Workspace workspace={view.workspace} />{/key}
{:else if view.name === "trash"}
  {#key `${view.workspace.id}/${view.space}`}
    <Trash workspace={view.workspace} slug={view.space} />
  {/key}
{:else if view.name === "search"}
  {#key view.workspace.id}<Search workspace={view.workspace} />{/key}
{:else if view.name === "settings"}
  {#key view.workspace.id}<WorkspaceSettings workspace={view.workspace} />{/key}
{:else if view.name === "space-settings"}
  {#key `${view.workspace.id}/${view.space}`}
    <SpaceSettings workspace={view.workspace} slug={view.space} />
  {/key}
{:else if view.name === "page"}
  {#await import("./routes/Page.svelte") then { default: Page }}
    {#key `${view.id}/${reopened.count}`}<Page id={view.id} />{/key}
  {:catch}
    <Message title="page failed to load">
      <button class="button" type="button" onclick={() => location.reload()}>reload</button>
    </Message>
  {/await}
{:else if view.name === "history"}
  {#key view.id}<History id={view.id} selected={view.revision} />{/key}
{:else if view.name === "home"}
  <Shell title="no workspace">
    <h1>no workspace</h1>
    {#if app.session?.can_create_workspace}
      <p class="notice">create one, or ask a workspace admin for an invitation.</p>
      <NewWorkspace />
    {:else}
      <p class="notice">ask a workspace admin for an invitation.</p>
    {/if}
  </Shell>
{:else if view.name === "not-found"}
  <Message title="not found" />
{/if}

<!-- Outside every screen, so a screen that changes under it keeps the notice. -->
<Notice />
