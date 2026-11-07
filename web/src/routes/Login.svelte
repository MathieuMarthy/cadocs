<script lang="ts">
  import Field from "../components/Field.svelte";
  import Solo from "../components/Solo.svelte";
  import { api, ApiError, type Session } from "../lib/api";
  import { describe, Form } from "../lib/form.svelte";
  import { app, signIn } from "../lib/session.svelte";

  let email = $state("");
  let password = $state("");
  const form = new Form();

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    await form.run(
      async () => signIn(await api<Session>("POST", "/auth/login", { email, password })),
      (error) =>
        error instanceof ApiError && error.status === 401
          ? "wrong email or password"
          : describe(error),
    );
  }
</script>

<Solo title="log in">
  {#if app.notice}<p class="notice" role="status">{app.notice}</p>{/if}
  <form onsubmit={submit}>
    <Field
      label="email"
      type="email"
      bind:value={email}
      autocomplete="email"
      required
      problem={form.field("email")}
    />
    <Field
      label="password"
      type="password"
      bind:value={password}
      autocomplete="current-password"
      required
      problem={form.field("password")}
    />
    {#if form.problem}<p class="problem" role="alert">{form.problem}</p>{/if}
    <button class="button" disabled={form.busy}>log in</button>
  </form>
  <p class="meta" style="margin-top: var(--space-6)">
    forgot your password? ask an administrator for a reset link.
  </p>
</Solo>
