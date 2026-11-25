<script lang="ts">
  import Field from "../components/Field.svelte";
  import Solo from "../components/Solo.svelte";
  import { api, ApiError, type Session } from "../lib/api";
  import { describe, Form } from "../lib/form.svelte";
  import { navigate } from "../lib/router.svelte";
  import { app, signIn, signOut } from "../lib/session.svelte";
  import { t } from "../lib/i18n";
  import { roleName } from "../lib/roles";

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
      await signIn(session);
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
  <Solo title={t("invite.title")}><p class="meta" role="status">{t("common.loading")}</p></Solo>
{:then invitation}
  <Solo title={t("invite.join", { workspace: invitation.workspace_name })}>
    <p class="meta">
      {t("invite.for", { email: invitation.email, role: roleName(invitation.role) })}
    </p>
    {#if form.problem}<p class="problem" role="alert">{form.problem}</p>{/if}

    {#if app.session && sameEmail(invitation)}
      <button
        class="button"
        disabled={form.busy}
        onclick={() => accept({}, invitation.workspace_id)}
      >
        {t("invite.join", { workspace: invitation.workspace_name })}
      </button>
    {:else if app.session}
      <p class="notice">
        {t("invite.otherAccount", { email: app.session.user.email })}
      </p>
      <button class="button" type="button" disabled={form.busy} onclick={logOut}>{t("shell.logOut")}</button>
    {:else if invitation.has_account}
      <p class="notice">{t("invite.hasAccount")}</p>
      <a class="button" href="/login?next={encodeURIComponent(`/invite/${token}`)}">{t("login.title")}</a>
    {:else}
      <form
        onsubmit={(event) => {
          event.preventDefault();
          void accept({ name, password }, invitation.workspace_id);
        }}
      >
        <Field
          label={t("form.name")}
          bind:value={name}
          autocomplete="name"
          required
          problem={form.field("name")}
        />
        <Field
          label={t("form.password")}
          type="password"
          bind:value={password}
          autocomplete="new-password"
          required
          problem={form.field("password")}
        />
        {#if form.field("email")}
          <p class="problem" role="alert">{t("invite.emailProblem", { problem: form.field("email") ?? "" })}</p>
        {/if}
        <button class="button" disabled={form.busy}>
          {t("invite.join", { workspace: invitation.workspace_name })}
        </button>
      </form>
    {/if}
  </Solo>
{:catch error}
  <Solo title={t("invite.title")}>
    <p class="notice">
      {error instanceof ApiError && error.status === 404
        ? t("reset.expired")
        : describe(error)}
    </p>
    <a class="button" href="/">{t("app.home")}</a>
  </Solo>
{/await}
