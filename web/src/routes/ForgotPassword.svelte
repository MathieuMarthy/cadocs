<script lang="ts">
  import Field from "../components/Field.svelte";
  import Solo from "../components/Solo.svelte";
  import { api } from "../lib/api";
  import { Form } from "../lib/form.svelte";
  import { sendsMail } from "../lib/session.svelte";
  import Message from "./Message.svelte";

  let email = $state("");
  let sent = $state(false);
  const form = new Form();

  /** With mail off this screen does not exist (ADR 0026). Unknown at first. */
  let mail = $state<boolean | null>(null);
  sendsMail().then((sends) => (mail = sends));

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    // The server answers the same for every address (ADR 0026), and so does this screen.
    sent = await form.run(() => api("POST", "/auth/forgot-password", { email }));
  }
</script>

{#if mail === false}
  <Message title="not found" />
{:else if mail}
<Solo title="forgot password">
  {#if sent}
    <p class="notice" role="status">
      if an account uses this address, a reset link is on its way. it works for one hour.
    </p>
    <a class="button" href="/login">back to log in</a>
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
      {#if form.problem}<p class="problem" role="alert">{form.problem}</p>{/if}
      <button class="button" disabled={form.busy}>send reset link</button>
    </form>
    <p class="meta" style="margin-top: var(--space-6)"><a href="/login">back to log in</a></p>
  {/if}
</Solo>
{/if}
