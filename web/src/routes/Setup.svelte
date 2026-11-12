<script lang="ts">
  import Field from "../components/Field.svelte";
  import Solo from "../components/Solo.svelte";
  import { api, ApiError, type Session } from "../lib/api";
  import { describe, Form } from "../lib/form.svelte";
  import { boot, signIn } from "../lib/session.svelte";

  let { code: linked }: { code: string } = $props();

  // A code from the link stays out of sight until the server refuses it: it changes at each
  // start of the server.
  // svelte-ignore state_referenced_locally
  let code = $state(linked);
  // svelte-ignore state_referenced_locally
  let askCode = $state(!linked);
  let name = $state("");
  let email = $state("");
  let password = $state("");
  let workspace = $state("");
  const form = new Form();

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    await form.run(
      async () => {
        const body = { name, email, password, workspace_name: workspace, code };
        signIn(await api<Session>("POST", "/auth/setup", body));
      },
      (error) => {
        if (error instanceof ApiError && error.fields.code) askCode = true;
        // Someone else finished setup first.
        if (error instanceof ApiError && error.status === 409) void boot();
        return describe(error);
      },
    );
  }
</script>

<Solo title="first run">
  <p class="meta">create the first account. it owns the first workspace and runs this instance.</p>
  <form onsubmit={submit}>
    {#if askCode}
      <p class="meta">the server log shows a link with the setup code each time the server starts.</p>
      <Field
        label="setup code"
        bind:value={code}
        autocomplete="off"
        spellcheck="false"
        required
        problem={form.field("code")}
      />
    {/if}
    <Field label="name" bind:value={name} autocomplete="name" required problem={form.field("name")} />
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
      autocomplete="new-password"
      required
      problem={form.field("password")}
    />
    <Field
      label="workspace name"
      bind:value={workspace}
      required
      problem={form.field("workspace_name")}
    />
    {#if form.problem}<p class="problem" role="alert">{form.problem}</p>{/if}
    <button class="button" disabled={form.busy}>create workspace</button>
  </form>
</Solo>
