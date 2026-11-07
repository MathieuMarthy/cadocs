<script lang="ts">
  import Message from "./routes/Message.svelte";
  import Account from "./routes/Account.svelte";
  import Admin from "./routes/Admin.svelte";
  import Invite from "./routes/Invite.svelte";
  import Login from "./routes/Login.svelte";
  import ResetPassword from "./routes/ResetPassword.svelte";
  import Setup from "./routes/Setup.svelte";
  import Shell from "./components/Shell.svelte";
  import Workspace from "./routes/Workspace.svelte";
  import type { Workspace as WorkspaceRow } from "./lib/api";
  import { navigate, route, safeNext } from "./lib/router.svelte";
  import { app, boot } from "./lib/session.svelte";

  type View =
    | { name: "redirect"; to: string }
    | { name: "setup" | "login" | "account" | "admin" | "home" | "not-found" }
    | { name: "invite" | "reset"; token: string }
    | { name: "workspace"; workspace: WorkspaceRow }
    | { name: "page"; id: string };

  function resolve(path: string, search: string): View {
    const [first, second, ...rest] = path.split("/").filter(Boolean);
    const query = new URLSearchParams(search);

    // A link in a message works whatever state this browser is in.
    if (first === "invite" && second && rest.length === 0) return { name: "invite", token: second };
    if (first === "reset-password" && second && rest.length === 0) {
      return { name: "reset", token: second };
    }

    if (app.setupRequired) {
      return first === "setup" ? { name: "setup" } : { name: "redirect", to: "/setup" };
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
      if (page) return { name: "page", id: page };
      const workspace = session.workspaces[0];
      return workspace ? { name: "redirect", to: `/w/${workspace.slug}` } : { name: "home" };
    }
    if (first === "account" && !second) return { name: "account" };
    if (first === "admin" && !second && session.user.is_instance_admin) return { name: "admin" };
    if (first === "w" && second && rest.length === 0) {
      const workspace = session.workspaces.find((item) => item.slug === second);
      if (workspace) return { name: "workspace", workspace };
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
  <Setup />
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
  <Workspace workspace={view.workspace} />
{:else if view.name === "page"}
  {#await import("./routes/Page.svelte") then { default: Page }}
    {#key view.id}<Page id={view.id} />{/key}
  {:catch}
    <Message title="page failed to load">
      <button class="button" type="button" onclick={() => location.reload()}>reload</button>
    </Message>
  {/await}
{:else if view.name === "home"}
  <Shell title="no workspace">
    <h1>no workspace</h1>
    <p class="notice">ask a workspace admin for an invitation.</p>
  </Shell>
{:else if view.name === "not-found"}
  <Message title="not found" />
{/if}
