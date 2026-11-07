<script lang="ts">
  import Field from "../components/Field.svelte";
  import Solo from "../components/Solo.svelte";
  import { api, ApiError } from "../lib/api";
  import { describe, Form } from "../lib/form.svelte";
  import { app, refreshSession, signOut } from "../lib/session.svelte";

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
    else signOut("password changed, log in");
  }
</script>

<Solo title="new password">
  {#if changed}
    <p class="notice" role="status">password changed</p>
    <a class="button" href="/">go to cadocs</a>
  {:else if expired}
    <p class="notice">this link is no longer valid.</p>
    <a class="button" href="/">go to cadocs</a>
  {:else}
    <form onsubmit={submit}>
      <Field
        label="new password"
        type="password"
        bind:value={password}
        autocomplete="new-password"
        required
        problem={form.field("password")}
      />
      {#if form.problem}<p class="problem" role="alert">{form.problem}</p>{/if}
      <button class="button" disabled={form.busy}>set password</button>
    </form>
  {/if}
</Solo>
