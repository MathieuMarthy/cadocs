<script lang="ts">
  import Confirm from "../components/Confirm.svelte";
  import Field from "../components/Field.svelte";
  import Shell from "../components/Shell.svelte";
  import { api, type User } from "../lib/api";
  import { Form } from "../lib/form.svelte";
  import { app, mailFeatures, signOut, updateUser } from "../lib/session.svelte";
  import { t } from "../lib/i18n";

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
  let loginSaved = $state("");
  const login = new Form();
  // With a relay, a new email waits for the code mailed to it (ADR 0026).
  let pending = $state("");
  let proofCode = $state("");
  const proof = new Form();

  async function saveLogin(event: SubmitEvent) {
    event.preventDefault();
    loginSaved = "";
    const changes: Record<string, string> = {};
    if (email !== app.session!.user.email) changes.email = email;
    if (password) changes.password = password;
    if (Object.keys(changes).length === 0) {
      login.problem = t("account.nothing");
      return;
    }
    if (currentPassword) changes.current_password = currentPassword;
    // Lowercased as the server stores it: a different answer means the new email waits for a code.
    const asked = changes.email?.trim().toLowerCase();
    const done = await login.run(async () => {
      const saved = await api<User>("PATCH", "/me", changes);
      updateUser(saved);
      // A save without an email leaves a code that waits.
      if (asked !== undefined) {
        pending = asked !== saved.email ? asked : "";
        proofCode = "";
        proof.problem = null;
      }
      email = saved.email;
      password = "";
      currentPassword = "";
    });
    if (done) {
      const waits = asked !== undefined && pending;
      loginSaved = waits ? t("login.codeSent", { email: pending }) : t("account.saved");
    }
  }

  async function proveEmail(event: SubmitEvent) {
    event.preventDefault();
    const done = await proof.run(async () => {
      const saved = await api<User>("POST", "/me/email/confirm", { email: pending, code: proofCode });
      updateUser(saved);
      email = saved.email;
    });
    if (done) {
      pending = "";
      loginSaved = t("account.emailChanged");
    }
  }

  // The second factor by mail (ADR 0026), offered only with a relay.
  let mail = $state(false);
  mailFeatures().then((read) => (mail = read.mail));
  let factorCode = $state("");
  let factorSent = $state(false);
  let factorPassword = $state("");
  const factor = new Form();

  async function sendFactorCode() {
    factorSent = await factor.run(() => api("POST", "/me/email-2fa"));
    factorCode = "";
  }

  async function turnFactorOn(event: SubmitEvent) {
    event.preventDefault();
    const done = await factor.run(() => api("POST", "/me/email-2fa/confirm", { code: factorCode }));
    if (done) {
      updateUser({ ...app.session!.user, email_2fa: true });
      factorSent = false;
    }
  }

  async function turnFactorOff(event: SubmitEvent) {
    event.preventDefault();
    const body = factorPassword ? { current_password: factorPassword } : {};
    const done = await factor.run(() => api("DELETE", "/me/email-2fa", body));
    if (done) {
      updateUser({ ...app.session!.user, email_2fa: false });
      factorPassword = "";
    }
  }

  const removal = new Form();

  async function remove() {
    const done = await removal.run(() => api("DELETE", "/me"));
    if (done) signOut();
  }
</script>

<Shell title={t("shell.account")} crumbWorkspace={false}>
  <h1>{t("shell.account")}</h1>

  <section aria-labelledby="profile-heading">
    <h2 id="profile-heading">{t("account.profile")}</h2>
    <form aria-labelledby="profile-heading" onsubmit={saveProfile}>
      <Field
        label={t("form.name")}
        bind:value={name}
        autocomplete="name"
        required
        problem={profile.field("name")}
        oninput={() => (profileSaved = false)}
      />
      {#if profile.problem}<p class="problem" role="alert">{profile.problem}</p>{/if}
      {#if profileSaved}<p class="notice" role="status">{t("account.saved")}</p>{/if}
      <button class="button" disabled={profile.busy}>{t("account.save")}</button>
    </form>
  </section>

  <section aria-labelledby="login-heading">
    <h2 id="login-heading">{t("account.login")}</h2>
    <form aria-labelledby="login-heading" onsubmit={saveLogin}>
      <Field
        label={t("form.email")}
        type="email"
        bind:value={email}
        autocomplete="email"
        required
        problem={login.field("email")}
        oninput={() => (loginSaved = "")}
      />
      <Field
        label={t("form.newPassword")}
        type="password"
        bind:value={password}
        autocomplete="new-password"
        problem={login.field("password")}
        oninput={() => (loginSaved = "")}
      />
      <Field
        label={t("form.currentPassword")}
        type="password"
        bind:value={currentPassword}
        autocomplete="current-password"
        problem={login.field("current_password")}
      />
      <p class="meta">{t("account.needsCurrent")}</p>
      {#if login.problem}<p class="problem" role="alert">{login.problem}</p>{/if}
      {#if loginSaved}<p class="notice" role="status">{loginSaved}</p>{/if}
      <button class="button" disabled={login.busy}>{t("account.save")}</button>
    </form>
    {#if pending}
      <form aria-label={t("account.newEmail")} onsubmit={proveEmail}>
        <Field
          label={t("form.code")}
          bind:value={proofCode}
          inputmode="numeric"
          autocomplete="one-time-code"
          spellcheck="false"
          required
          problem={proof.field("code")}
        />
        {#if proof.problem}<p class="problem" role="alert">{proof.problem}</p>{/if}
        {#if proof.field("email")}<p class="problem" role="alert">{t("account.emailTaken")}</p>{/if}
        <button class="button" disabled={proof.busy}>{t("account.changeEmail")}</button>
      </form>
    {/if}
  </section>

  {#if mail}
    <section aria-labelledby="factor-heading">
      <h2 id="factor-heading">{t("account.factor")}</h2>
      {#if app.session!.user.email_2fa}
        <p class="meta">{t("account.factorOn", { email: app.session!.user.email })}</p>
        <form aria-labelledby="factor-heading" onsubmit={turnFactorOff}>
          <Field
            label={t("form.currentPassword")}
            type="password"
            bind:value={factorPassword}
            autocomplete="current-password"
            required
            problem={factor.field("current_password")}
          />
          {#if factor.problem}<p class="problem" role="alert">{factor.problem}</p>{/if}
          <button class="button secondary" disabled={factor.busy}>{t("account.turnOff")}</button>
        </form>
      {:else if factorSent}
        <p class="notice" role="status">
          {t("login.codeSent", { email: app.session!.user.email })}
        </p>
        <form aria-labelledby="factor-heading" onsubmit={turnFactorOn}>
          <Field
            label={t("form.code")}
            bind:value={factorCode}
            inputmode="numeric"
            autocomplete="one-time-code"
            spellcheck="false"
            required
            problem={factor.field("code")}
          />
          {#if factor.problem}<p class="problem" role="alert">{factor.problem}</p>{/if}
          <div class="actions">
            <button class="button" disabled={factor.busy}>{t("account.turnOn")}</button>
            <button class="button secondary" type="button" disabled={factor.busy} onclick={sendFactorCode}>
              {t("login.resend")}
            </button>
          </div>
        </form>
      {:else}
        <p class="meta">{t("account.factorOff")}</p>
        {#if factor.problem}<p class="problem" role="alert">{factor.problem}</p>{/if}
        <div class="actions">
          <button class="button" type="button" disabled={factor.busy} onclick={sendFactorCode}>
            {t("account.turnOn")}
          </button>
        </div>
      {/if}
    </section>
  {/if}

  <section aria-labelledby="delete-heading">
    <h2 id="delete-heading">{t("account.delete")}</h2>
    <p class="meta">{t("account.deleteHint")}</p>
    {#if removal.problem}<p class="problem" role="alert">{removal.problem}</p>{/if}
    <div class="actions">
      <Confirm
        label={t("account.delete")}
        confirm={t("account.delete")}
        danger
        disabled={removal.busy}
        onconfirm={remove}
      />
    </div>
  </section>
</Shell>
