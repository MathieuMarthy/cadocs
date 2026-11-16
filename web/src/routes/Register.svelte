<script lang="ts">
  import Field from "../components/Field.svelte";
  import Solo from "../components/Solo.svelte";
  import { api, type Session } from "../lib/api";
  import { Form } from "../lib/form.svelte";
  import { mailFeatures, signIn } from "../lib/session.svelte";
  import Message from "./Message.svelte";

  let email = $state("");
  let code = $state("");
  let name = $state("");
  let password = $state("");
  /** Once a code was asked for, the screen takes it. */
  let sent = $state(false);
  const request = new Form();
  const confirmation = new Form();

  /** With mail off or registration closed this screen does not exist (ADR 0026). */
  let open = $state<boolean | null>(null);
  mailFeatures().then((features) => (open = features.registration));

  async function send(event: SubmitEvent) {
    event.preventDefault();
    // The server answers the same for every address (ADR 0026), and so does this screen: an
    // address with an account gets a message that says so instead of a code.
    sent = await request.run(() => api("POST", "/auth/register", { email }));
    code = "";
  }

  async function confirm(event: SubmitEvent) {
    event.preventDefault();
    await confirmation.run(async () => {
      const body = { email, code, name, password };
      signIn(await api<Session>("POST", "/auth/register/confirm", body));
    });
  }
</script>

{#if open === false}
  <Message title="not found" />
{:else if open}
<Solo title="create an account">
  {#if sent}
    <p class="notice" role="status">
      if {email} can make an account, a code is on its way. it works for 10 minutes.
    </p>
    <form onsubmit={confirm}>
      <Field
        label="code"
        bind:value={code}
        inputmode="numeric"
        autocomplete="one-time-code"
        spellcheck="false"
        required
        problem={confirmation.field("code")}
      />
      <Field label="name" bind:value={name} autocomplete="name" required problem={confirmation.field("name")} />
      <Field
        label="password"
        type="password"
        bind:value={password}
        autocomplete="new-password"
        required
        problem={confirmation.field("password")}
      />
      {#if confirmation.field("email")}
        <p class="problem" role="alert">this address has an account now. log in instead.</p>
      {/if}
      {#if confirmation.problem}<p class="problem" role="alert">{confirmation.problem}</p>{/if}
      <button class="button" disabled={confirmation.busy}>create account</button>
    </form>
    <p class="meta" style="margin-top: var(--space-6)">
      no code?
      <button class="button secondary" type="button" onclick={() => (sent = false)}>
        send a new code
      </button>
    </p>
  {:else}
    <p class="meta">a code sent to your address proves it is yours.</p>
    <form onsubmit={send}>
      <Field
        label="email"
        type="email"
        bind:value={email}
        autocomplete="email"
        required
        problem={request.field("email")}
      />
      {#if request.problem}<p class="problem" role="alert">{request.problem}</p>{/if}
      <button class="button" disabled={request.busy}>send code</button>
    </form>
    <p class="meta" style="margin-top: var(--space-6)">
      already an account? <a href="/login">log in</a>
    </p>
  {/if}
</Solo>
{/if}
