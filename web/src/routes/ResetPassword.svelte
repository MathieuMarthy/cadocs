<script lang="ts">
  import Field from "../components/Field.svelte";
  import Solo from "../components/Solo.svelte";
  import { api, ApiError } from "../lib/api";
  import { describe, Form } from "../lib/form.svelte";
  import { app, refreshSession, signOut } from "../lib/session.svelte";
  import { t } from "../lib/i18n";

  let { token }: { token: string } = $props();

  let password = $state("");
  let expired = $state(false);
  let changed = $state(false);
  const form = new Form();

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    const done = await form.run(
      () => api("POST", `/auth/password-reset/${encodeURIComponent(token)}`, { password }),
      (error) => {
        expired = error instanceof ApiError && error.status === 404;
        return describe(error);
      },
    );
    if (!done) return;
    // The reset ended the sessions of the link's account, which may not be this browser's.
    await refreshSession().catch(() => {});
    if (app.session) changed = true;
    else signOut(t("reset.loggedOut"));
  }
</script>

<Solo title={t("reset.title")}>
  {#if changed}
    <p class="notice" role="status">{t("reset.done")}</p>
    <a class="button" href="/">{t("app.home")}</a>
  {:else if expired}
    <p class="notice">{t("reset.expired")}</p>
    <a class="button" href="/">{t("app.home")}</a>
  {:else}
    <form onsubmit={submit}>
      <Field
        label={t("form.newPassword")}
        type="password"
        bind:value={password}
        autocomplete="new-password"
        required
        problem={form.field("password")}
      />
      {#if form.problem}<p class="problem" role="alert">{form.problem}</p>{/if}
      <button class="button" disabled={form.busy}>{t("reset.submit")}</button>
    </form>
  {/if}
</Solo>
