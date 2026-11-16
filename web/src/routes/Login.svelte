<script lang="ts">
  import Field from "../components/Field.svelte";
  import Solo from "../components/Solo.svelte";
  import { api, ApiError, type Session } from "../lib/api";
  import { describe, Form } from "../lib/form.svelte";
  import { app, mailFeatures, signIn, type MailFeatures } from "../lib/session.svelte";

  let email = $state("");
  let password = $state("");
  /** The second factor (ADR 0026): once the server mailed a code, the form takes it. */
  let code = $state("");
  let codeAsked = $state(false);
  const form = new Form();

  /** Unknown at first, and nothing shows until then. */
  let features = $state<MailFeatures | null>(null);
  mailFeatures().then((read) => (features = read));

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    await logIn(codeAsked ? code : "");
  }

  /** Without a code, an account with the second factor gets one by mail. */
  async function logIn(withCode: string) {
    const body = withCode ? { email, password, code: withCode } : { email, password };
    await form.run(
      async () => signIn(await api<Session>("POST", "/auth/login", body)),
      (error) =>
        error instanceof ApiError && error.status === 401
          ? "wrong email or password"
          : describe(error),
    );
    if (form.fields.code === "required") {
      form.fields = {};
      codeAsked = true;
      code = "";
    }
  }

  function startOver() {
    codeAsked = false;
    code = "";
    password = "";
    form.fields = {};
    form.problem = null;
  }
</script>

<Solo title="log in">
  {#if app.notice}<p class="notice" role="status">{app.notice}</p>{/if}
  {#if codeAsked}
    <p class="notice" role="status">a code is on its way to {email}. it works for 10 minutes.</p>
    <form onsubmit={submit}>
      <Field
        label="code"
        bind:value={code}
        inputmode="numeric"
        autocomplete="one-time-code"
        spellcheck="false"
        required
        problem={form.field("code")}
      />
      {#if form.problem}<p class="problem" role="alert">{form.problem}</p>{/if}
      <button class="button" disabled={form.busy}>log in</button>
    </form>
    <p class="meta" style="margin-top: var(--space-6)">
      no code?
      <button class="button secondary" type="button" disabled={form.busy} onclick={() => logIn("")}>
        send a new code
      </button>
    </p>
    <p class="meta">
      <button class="button secondary" type="button" onclick={startOver}>use another account</button>
    </p>
  {:else}
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
    {#if features?.mail}
      <p class="meta" style="margin-top: var(--space-6)">
        <a href="/forgot-password">forgot your password?</a>
      </p>
    {:else if features}
      <p class="meta" style="margin-top: var(--space-6)">
        forgot your password? ask an administrator for a reset link.
      </p>
    {/if}
    {#if features?.registration}
      <p class="meta">no account? <a href="/register">create an account</a></p>
    {/if}
  {/if}
</Solo>
