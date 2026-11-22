<script lang="ts">
  import Confirm from "../components/Confirm.svelte";
  import Field from "../components/Field.svelte";
  import Shell from "../components/Shell.svelte";
  import { copy } from "../lib/actions";
  import { api, type User } from "../lib/api";
  import { Form } from "../lib/form.svelte";
  import { app, mailFeatures, signOut, updateUser } from "../lib/session.svelte";
  import { t } from "../lib/i18n";
  import { route } from "../lib/router.svelte";

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
  mailFeatures().then((read) => {
    mail = read.mail;
    providers = read.oidc;
  });

  // Single sign-on (ADR 0028): the providers the instance has, and the ones tied to this account.
  let providers = $state<string[]>([]);
  let linked = $state<string[]>([]);
  const sso = new Form();
  const ssoOutcome = new URLSearchParams(route.search).get("oidc");
  /** Whether linking asks for the password: a stolen session must not tie its own provider. */
  let ssoPassword = $state(true);
  let ssoCurrentPassword = $state("");
  api<{ providers: string[]; password: boolean }>("GET", "/me/identities").then((read) => {
    linked = read.providers;
    ssoPassword = read.password;
  });

  async function link(provider: string) {
    const body = ssoPassword ? { current_password: ssoCurrentPassword } : {};
    await sso.run(async () => {
      const started = await api<{ url: string }>("POST", `/me/identities/${provider}`, body);
      location.assign(started.url);
    });
  }

  async function unlink(provider: string) {
    const done = await sso.run(() => api("DELETE", `/me/identities/${provider}`));
    if (done) linked = linked.filter((id) => id !== provider);
  }
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

  // Notifications by mail (step 42), offered only with a relay.
  const notifying = new Form();

  async function chooseNotifications(enabled: boolean) {
    const done = await notifying.run(() => api("PUT", "/me/email-notifications", { enabled }));
    if (done) updateUser({ ...app.session!.user, email_notifications: enabled });
  }

  // The authenticator app (RFC 6238), which needs no relay.
  interface TotpSetup {
    secret: string;
    uri: string;
  }
  interface Recovery {
    recovery_codes: string[];
  }
  let totpPassword = $state("");
  let totpSetup = $state<TotpSetup | null>(null);
  let totpCode = $state("");
  /** The QR code of the setup, once its module loaded. */
  let qr = $state<{ size: number; path: string } | null>(null);
  /** Recovery codes just made, shown this once. */
  let recovery = $state<string[]>([]);
  const totp = new Form();

  /** The dark modules as one SVG path, drawn with no `{@html}`. Loaded only for a setup. */
  async function drawQr(uri: string) {
    const { encode } = await import("uqr");
    const { data, size } = encode(uri, { ecc: "M", border: 2 });
    let path = "";
    data.forEach((row, y) =>
      row.forEach((dark, x) => {
        if (dark) path += `M${x},${y}h1v1h-1z`;
      }),
    );
    qr = { size, path };
  }

  /** The key in groups of four, as apps print it. */
  const grouped = (secret: string) => secret.match(/.{1,4}/g)?.join(" ") ?? secret;

  async function startTotp(event: SubmitEvent) {
    event.preventDefault();
    const body = totpPassword ? { current_password: totpPassword } : {};
    let started: TotpSetup | null = null;
    const done = await totp.run(async () => {
      started = await api<TotpSetup>("POST", "/me/totp", body);
    });
    if (done && started) {
      const setup: TotpSetup = started;
      totpSetup = setup;
      totpPassword = "";
      totpCode = "";
      qr = null;
      // Without the QR code, the key still works typed.
      drawQr(setup.uri).catch(() => (qr = null));
    }
  }

  async function confirmTotp(event: SubmitEvent) {
    event.preventDefault();
    const done = await totp.run(async () => {
      const answer = await api<Recovery>("POST", "/me/totp/confirm", { code: totpCode });
      recovery = answer.recovery_codes;
    });
    if (done) {
      updateUser({ ...app.session!.user, totp: true });
      totpSetup = null;
      qr = null;
    }
  }

  function cancelTotp() {
    totpSetup = null;
    qr = null;
    totp.problem = null;
    totp.fields = {};
  }

  /**
   * New recovery codes or the app off: both need the password. Buttons, not submits, so Enter in
   * the password field does neither.
   */
  async function changeTotp(action: "codes" | "off") {
    const body = totpPassword ? { current_password: totpPassword } : {};
    const done =
      action === "codes"
        ? await totp.run(async () => {
            const answer = await api<Recovery>("POST", "/me/totp/recovery-codes", body);
            recovery = answer.recovery_codes;
          })
        : await totp.run(() => api("DELETE", "/me/totp", body));
    if (!done) return;
    totpPassword = "";
    if (action !== "codes") updateUser({ ...app.session!.user, totp: false });
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
        {#if app.session!.user.totp}<p class="meta">{t("account.factorAfterApp")}</p>{/if}
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

  {#if mail}
    <section aria-labelledby="notifications-heading">
      <h2 id="notifications-heading">{t("account.notifications")}</h2>
      <p class="meta">
        {t(app.session!.user.email_notifications ? "account.notificationsOn" : "account.notificationsOff")}
      </p>
      {#if notifying.problem}<p class="problem" role="alert">{notifying.problem}</p>{/if}
      <div class="actions">
        <button
          class="button"
          class:secondary={app.session!.user.email_notifications}
          type="button"
          disabled={notifying.busy}
          onclick={() => chooseNotifications(!app.session!.user.email_notifications)}
        >
          {t(app.session!.user.email_notifications ? "account.turnOff" : "account.turnOn")}
        </button>
      </div>
    </section>
  {/if}

  {#if providers.length > 0 || linked.length > 0}
    <section aria-labelledby="sso-heading">
      <h2 id="sso-heading">{t("account.sso")}</h2>
      {#if ssoOutcome === "linked"}
        <p class="notice" role="status">{t("account.ssoLinked")}</p>
      {:else if ssoOutcome === "taken"}
        <p class="problem" role="alert">{t("account.ssoTaken")}</p>
      {:else if ssoOutcome}
        <p class="problem" role="alert">{t("account.ssoFailed")}</p>
      {/if}
      {#if sso.problem}<p class="problem" role="alert">{sso.problem}</p>{/if}
      {#if ssoPassword && providers.some((id) => !linked.includes(id))}
        <Field
          label={t("form.currentPassword")}
          type="password"
          bind:value={ssoCurrentPassword}
          autocomplete="current-password"
          problem={sso.field("current_password")}
        />
      {/if}
      <ul class="providers">
        {#each new Set([...providers, ...linked]) as provider (provider)}
          <li>
            <span>{provider}: {t(linked.includes(provider) ? "account.ssoOn" : "account.ssoOff")}</span>
            {#if linked.includes(provider)}
              <button class="button secondary" type="button" disabled={sso.busy} onclick={() => unlink(provider)}>
                {t("account.ssoUnlink")}
              </button>
            {:else if providers.includes(provider)}
              <button class="button secondary" type="button" disabled={sso.busy} onclick={() => link(provider)}>
                {t("account.ssoLink")}
              </button>
            {/if}
          </li>
        {/each}
      </ul>
      {#if user.totp && linked.length > 0}<p class="meta">{t("account.ssoNoFactor")}</p>{/if}
    </section>
  {/if}

  <section aria-labelledby="totp-heading">
    <h2 id="totp-heading">{t("account.totp")}</h2>
    {#if recovery.length > 0}
      <p class="notice" role="status">{t("account.recoveryHint")}</p>
      <ol class="recovery" aria-label={t("account.recoveryCodes")}>
        {#each recovery as code (code)}<li>{code}</li>{/each}
      </ol>
      <div class="actions">
        <button
          class="button secondary"
          type="button"
          onclick={() => copy(recovery.join("\n"), t("account.recoveryCopied"), t("account.recoveryCopyFailed"))}
        >
          {t("account.recoveryCopy")}
        </button>
        <button class="button" type="button" onclick={() => (recovery = [])}>{t("account.recoveryDone")}</button>
      </div>
    {:else if app.session!.user.totp}
      <p class="meta">{t("account.totpOn")}</p>
      <form aria-labelledby="totp-heading" onsubmit={(event) => event.preventDefault()}>
        <Field
          label={t("form.currentPassword")}
          type="password"
          bind:value={totpPassword}
          autocomplete="current-password"
          required
          problem={totp.field("current_password")}
        />
        {#if totp.problem}<p class="problem" role="alert">{totp.problem}</p>{/if}
        <div class="actions">
          <button class="button secondary" type="button" disabled={totp.busy} onclick={() => changeTotp("codes")}>
            {t("account.recoveryNew")}
          </button>
          <button class="button secondary" type="button" disabled={totp.busy} onclick={() => changeTotp("off")}>
            {t("account.turnOff")}
          </button>
        </div>
      </form>
    {:else if totpSetup}
      <p class="meta">{t("account.totpScan")}</p>
      <div class="qr">
        {#if qr}
          <svg viewBox="0 0 {qr.size} {qr.size}" role="img" aria-label={t("account.totpQr")}>
            <path d={qr.path} />
          </svg>
        {/if}
      </div>
      <p class="meta">{t("account.totpKey")} <code class="secret">{grouped(totpSetup.secret)}</code></p>
      <form aria-labelledby="totp-heading" onsubmit={confirmTotp}>
        <Field
          label={t("account.totpCode")}
          bind:value={totpCode}
          inputmode="numeric"
          autocomplete="one-time-code"
          spellcheck="false"
          required
          problem={totp.field("code")}
        />
        {#if totp.problem}<p class="problem" role="alert">{totp.problem}</p>{/if}
        <div class="actions">
          <button class="button" disabled={totp.busy}>{t("account.turnOn")}</button>
          <button class="button secondary" type="button" disabled={totp.busy} onclick={cancelTotp}>
            {t("common.cancel")}
          </button>
        </div>
      </form>
    {:else}
      <p class="meta">{t("account.totpOff")}</p>
      <form aria-labelledby="totp-heading" onsubmit={startTotp}>
        <Field
          label={t("form.currentPassword")}
          type="password"
          bind:value={totpPassword}
          autocomplete="current-password"
          required
          problem={totp.field("current_password")}
        />
        {#if totp.problem}<p class="problem" role="alert">{totp.problem}</p>{/if}
        <button class="button" disabled={totp.busy}>{t("account.totpSetUp")}</button>
      </form>
    {/if}
  </section>

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

<style>
  .providers {
    list-style: none;
    padding: 0;
    display: grid;
    gap: var(--space-3);
  }
  .providers li {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-4);
  }
  /* Dark on light in both themes: not every app reads a QR code drawn the other way round. */
  .qr {
    width: 200px;
    height: 200px;
    margin: var(--space-4) 0;
    border: 1px solid var(--line);
    background: #f5f5f4;
  }

  .qr svg {
    display: block;
    width: 100%;
    height: 100%;
    fill: #141414;
    shape-rendering: crispEdges;
  }

  .secret {
    font-family: var(--font-mono);
    color: var(--ink);
    user-select: all;
  }

  .recovery {
    columns: 2;
    margin: var(--space-4) 0;
    padding-left: var(--space-6);
    font-family: var(--font-mono);
    font-size: 13px;
  }
</style>
