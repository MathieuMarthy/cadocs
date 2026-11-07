<script lang="ts">
  import Field from "../components/Field.svelte";
  import Solo from "../components/Solo.svelte";
  import { api, ApiError, type Session } from "../lib/api";
  import { describe, Form } from "../lib/form.svelte";
  import { navigate } from "../lib/router.svelte";
  import { app, signIn, signOut } from "../lib/session.svelte";

  interface Invitation {
    workspace_id: string;
    workspace_name: string;
    email: string;
    role: string;
    has_account: boolean;
  }

  let { token }: { token: string } = $props();

  const path = $derived(`/invitations/${encodeURIComponent(token)}`);
  const form = new Form();
  let name = $state("");
  let password = $state("");

  async function accept(body: object, workspaceId: string) {
    await form.run(async () => {
      const session = await api<Session>("POST", `${path}/accept`, body);
      signIn(session);
      const joined = session.workspaces.find((item) => item.id === workspaceId);
      navigate(joined ? `/w/${joined.slug}` : "/", { replace: true });
    });
  }

  async function logOut() {
    const back = `/login?next=${encodeURIComponent(`/invite/${token}`)}`;
    if (await form.run(() => api("POST", "/auth/logout"))) signOut(null, back);
  }

  const sameEmail = (invitation: Invitation) =>
    app.session?.user.email.toLowerCase() === invitation.email.toLowerCase();
</script>

{#await api<Invitation>("GET", path)}
  <Solo title="invitation"><p class="meta" role="status">loading…</p></Solo>
{:then invitation}
  <Solo title="join {invitation.workspace_name.toLowerCase()}">
    <p class="meta">
      invitation for {invitation.email}, as {invitation.role}.
    </p>
    {#if form.problem}<p class="problem" role="alert">{form.problem}</p>{/if}

    {#if app.session && sameEmail(invitation)}
      <button
        class="button"
        disabled={form.busy}
        onclick={() => accept({}, invitation.workspace_id)}
      >
        join {invitation.workspace_name.toLowerCase()}
      </button>
    {:else if app.session}
      <p class="notice">
        logged in as {app.session.user.email}. this invitation is for another account.
      </p>
      <button class="button" type="button" disabled={form.busy} onclick={logOut}>log out</button>
    {:else if invitation.has_account}
      <p class="notice">this email has an account.</p>
      <a class="button" href="/login?next={encodeURIComponent(`/invite/${token}`)}">log in</a>
    {:else}
      <form
        onsubmit={(event) => {
          event.preventDefault();
          void accept({ name, password }, invitation.workspace_id);
        }}
      >
        <Field
          label="name"
          bind:value={name}
          autocomplete="name"
          required
          problem={form.field("name")}
        />
        <Field
          label="password"
          type="password"
          bind:value={password}
          autocomplete="new-password"
          required
          problem={form.field("password")}
        />
        {#if form.field("email")}
          <p class="problem" role="alert">email {form.field("email")}</p>
        {/if}
        <button class="button" disabled={form.busy}>
          join {invitation.workspace_name.toLowerCase()}
        </button>
      </form>
    {/if}
  </Solo>
{:catch error}
  <Solo title="invitation">
    <p class="notice">
      {error instanceof ApiError && error.status === 404
        ? "this link is no longer valid."
        : describe(error)}
    </p>
    <a class="button" href="/">go to cadocs</a>
  </Solo>
{/await}
