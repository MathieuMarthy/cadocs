<script lang="ts">
  import Confirm from "../components/Confirm.svelte";
  import Field from "../components/Field.svelte";
  import Shell from "../components/Shell.svelte";
  import { api, type User } from "../lib/api";
  import { Form } from "../lib/form.svelte";
  import { app, signOut, updateUser } from "../lib/session.svelte";

  const user = app.session!.user;
  let name = $state(user.name);
  let email = $state(user.email);
  let password = $state("");
  let currentPassword = $state("");
  let saved = $state(false);
  const form = new Form();
  const removal = new Form();

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    saved = false;
    const before = app.session!.user;
    const changes: Record<string, string> = {};
    if (name !== before.name) changes.name = name;
    if (email !== before.email) changes.email = email;
    if (password) changes.password = password;
    if (currentPassword) changes.current_password = currentPassword;

    saved = await form.run(async () => {
      updateUser(await api<User>("PATCH", "/me", changes));
      password = "";
      currentPassword = "";
    });
  }

  async function remove() {
    const done = await removal.run(() => api("DELETE", "/me"));
    if (done) signOut();
  }
</script>

<Shell title="account">
  <h1>account</h1>
  <form onsubmit={submit}>
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
      label="new password"
      type="password"
      bind:value={password}
      autocomplete="new-password"
      problem={form.field("password")}
    />
    <Field
      label="current password"
      type="password"
      bind:value={currentPassword}
      autocomplete="current-password"
      problem={form.field("current_password")}
    />
    <p class="meta">a new email or password needs the current password.</p>
    {#if form.problem}<p class="problem" role="alert">{form.problem}</p>{/if}
    {#if saved}<p class="notice" role="status">saved</p>{/if}
    <button class="button" disabled={form.busy}>save</button>
  </form>

  <h2>delete</h2>
  <p class="meta">this removes the account for good.</p>
  {#if removal.problem}<p class="problem" role="alert">{removal.problem}</p>{/if}
  <div class="actions">
    <Confirm
      label="delete account"
      confirm="yes, delete"
      disabled={removal.busy}
      onconfirm={remove}
    />
  </div>
</Shell>
