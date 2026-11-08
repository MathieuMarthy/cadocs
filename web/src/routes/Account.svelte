<script lang="ts">
  import Confirm from "../components/Confirm.svelte";
  import Field from "../components/Field.svelte";
  import Shell from "../components/Shell.svelte";
  import { api, type User } from "../lib/api";
  import { Form } from "../lib/form.svelte";
  import { app, signOut, updateUser } from "../lib/session.svelte";

  const user = app.session!.user;

  // The profile: what others see, changed without a password.
  let name = $state(user.name);
  let profileSaved = $state(false);
  const profile = new Form();

  async function saveProfile(event: SubmitEvent) {
    event.preventDefault();
    profileSaved = false;
    if (name === app.session!.user.name) {
      profileSaved = true;
      return;
    }
    profileSaved = await profile.run(async () => {
      updateUser(await api<User>("PATCH", "/me", { name }));
    });
  }

  // How one logs in: the server asks the current password for either.
  let email = $state(user.email);
  let password = $state("");
  let currentPassword = $state("");
  let loginSaved = $state(false);
  const login = new Form();

  async function saveLogin(event: SubmitEvent) {
    event.preventDefault();
    loginSaved = false;
    const changes: Record<string, string> = {};
    if (email !== app.session!.user.email) changes.email = email;
    if (password) changes.password = password;
    if (Object.keys(changes).length === 0) {
      login.problem = "nothing to save: type a new email or password";
      return;
    }
    if (currentPassword) changes.current_password = currentPassword;
    loginSaved = await login.run(async () => {
      updateUser(await api<User>("PATCH", "/me", changes));
      password = "";
      currentPassword = "";
    });
  }

  const removal = new Form();

  async function remove() {
    const done = await removal.run(() => api("DELETE", "/me"));
    if (done) signOut();
  }
</script>

<Shell title="account" crumbWorkspace={false}>
  <h1>account</h1>

  <section aria-labelledby="profile-heading">
    <h2 id="profile-heading">profile</h2>
    <form aria-labelledby="profile-heading" onsubmit={saveProfile}>
      <Field
        label="name"
        bind:value={name}
        autocomplete="name"
        required
        problem={profile.field("name")}
        oninput={() => (profileSaved = false)}
      />
      {#if profile.problem}<p class="problem" role="alert">{profile.problem}</p>{/if}
      {#if profileSaved}<p class="notice" role="status">saved</p>{/if}
      <button class="button" disabled={profile.busy}>save</button>
    </form>
  </section>

  <section aria-labelledby="login-heading">
    <h2 id="login-heading">email and password</h2>
    <form aria-labelledby="login-heading" onsubmit={saveLogin}>
      <Field
        label="email"
        type="email"
        bind:value={email}
        autocomplete="email"
        required
        problem={login.field("email")}
        oninput={() => (loginSaved = false)}
      />
      <Field
        label="new password"
        type="password"
        bind:value={password}
        autocomplete="new-password"
        problem={login.field("password")}
        oninput={() => (loginSaved = false)}
      />
      <Field
        label="current password"
        type="password"
        bind:value={currentPassword}
        autocomplete="current-password"
        problem={login.field("current_password")}
      />
      <p class="meta">a new email or password needs the current one</p>
      {#if login.problem}<p class="problem" role="alert">{login.problem}</p>{/if}
      {#if loginSaved}<p class="notice" role="status">saved</p>{/if}
      <button class="button" disabled={login.busy}>save</button>
    </form>
  </section>

  <section aria-labelledby="delete-heading">
    <h2 id="delete-heading">delete account</h2>
    <p class="meta">this removes the account for good</p>
    {#if removal.problem}<p class="problem" role="alert">{removal.problem}</p>{/if}
    <div class="actions">
      <Confirm
        label="delete account"
        confirm="delete account"
        danger
        disabled={removal.busy}
        onconfirm={remove}
      />
    </div>
  </section>
</Shell>
