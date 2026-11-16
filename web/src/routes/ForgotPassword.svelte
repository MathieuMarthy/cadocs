<script lang="ts">
  import Field from "../components/Field.svelte";
  import Solo from "../components/Solo.svelte";
  import { api } from "../lib/api";
  import { Form } from "../lib/form.svelte";
  import { mailFeatures } from "../lib/session.svelte";
  import Message from "./Message.svelte";
  import { t } from "../lib/i18n";

  let email = $state("");
  let sent = $state(false);
  const form = new Form();

  /** With mail off this screen does not exist (ADR 0026). Unknown at first. */
  let mail = $state<boolean | null>(null);
  mailFeatures().then((features) => (mail = features.mail));

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    // The server answers the same for every address (ADR 0026), and so does this screen.
    sent = await form.run(() => api("POST", "/auth/forgot-password", { email }));
  }
</script>

{#if mail === false}
  <Message title={t("app.notFound")} />
{:else if mail}
<Solo title={t("forgot.title")}>
  {#if sent}
    <p class="notice" role="status">
      {t("forgot.sent")}
    </p>
    <a class="button" href="/login">{t("forgot.back")}</a>
  {:else}
    <form onsubmit={submit}>
      <Field
        label={t("form.email")}
        type="email"
        bind:value={email}
        autocomplete="email"
        required
        problem={form.field("email")}
      />
      {#if form.problem}<p class="problem" role="alert">{form.problem}</p>{/if}
      <button class="button" disabled={form.busy}>{t("forgot.send")}</button>
    </form>
    <p class="meta" style="margin-top: var(--space-6)"><a href="/login">{t("forgot.back")}</a></p>
  {/if}
</Solo>
{/if}
