# ADR 0026: Mail through the operator's relay opens registration, recovery and a second factor

Status: accepted. Amends ADR 0013, which stays the rule of an instance with no relay.

## Context

ADR 0013 closed registration and made every account flow work without mail: a workspace invitation is a link, a password reset is a link an instance admin makes. It left mail to V1, as an option.

Daily use asks for more than notifications from it. An instance open to people its admins do not know needs them to register alone, to recover a password without an admin, and to protect an account with more than a password. All three need a proof that someone reads an address, and mail is the only channel that gives one.

"No mandatory external service" still holds: an instance must run with no relay, and behave as ADR 0013 says.

## Decision

**One switch.** Mail is on when `CADOCS_SMTP_HOST` names a relay, and off otherwise. cadocs is an SMTP client only: it hands each message to the operator's relay (Postfix, a provider) and neither queues to disk, signs, nor receives. With mail off, none of the routes below exist for a client, the interface shows none of their screens, and accounts come from first-run setup and invitations, as before.

**What mail turns on:**

- **Registration.** Anyone gives an address and gets a 6-digit code. The code, a name and a password make the account. The user belongs to no workspace: they create one (ADR 0024) or get invited. `CADOCS_REGISTRATION=closed` keeps mail and closes this, for a private wiki that only wants recovery. The default is `open`.
- **Forgot password.** A public form mails the reset link of ADR 0013. It answers the same way, in the same time, whether or not the account exists.
- **A second factor by mail.** A user turns it on in their account. A login then needs a 6-digit code mailed to them, after the password.
- **Proof of a new address.** Changing the address of an account needs a code sent to the new one, since recovery and the second factor now trust it.

**Codes.** 6 digits from the system's random source, valid 10 minutes, stored hashed, single use, and dead after 5 wrong tries. Asking again replaces the code. Every route that sends mail is rate limited per client and per address, so the instance cannot be used to flood a mailbox.

**Sending.** Messages are plain text, in English, one connection per message with no pool, so nothing stays open or allocated at idle. A handler never waits for the relay: the answer must not tell whether a message went out. A message the relay cannot take is logged and lost: the user asks again.

**Transport.** `lettre` over rustls, in every build. `CADOCS_SMTP_TLS` is `starttls` by default, which is never skipped, `tls`, or `none` for a relay on the same host or network. A password is refused with `none`.

**Mail turned off later.** The second factor needs a relay. Without one it is not asked for, and the server says so at start. The operator who removes the relay is the one who could reset any password from the command line, so this gives them nothing new.

## Consequences

- An address is verified when the account was registered or the address changed with mail on. Accounts from invitations, and accounts older than this, hold an address nobody proved: a `users.email_verified_at` column tells them apart, and the first code they use proves it.
- An instance with mail on and registration open is open to the internet's sign-ups: `CADOCS_WORKSPACE_CREATION=admins` and `CADOCS_WORKSPACE_QUOTA_MB` bound what a stranger can store.
- TLS code enters the default build. Step 28 measures what it costs.
- TOTP stays planned for V1. A code by mail is the weaker factor, and the one that needs no app.
- The last admin still recovers through `cadocs reset-password`, with or without mail.

## Rejected alternatives

- **A `mail` build feature, as S3 has.** A third and fourth image flavor, for a setting most public instances want. The measured cost decides: see step 28 in the benchmark notes.
- **A hand-written SMTP client.** Small for the plain case, but STARTTLS, authentication and header encoding are where mail clients get security wrong.
- **Messages in the job queue.** A stored job would hold a code in clear text, and a retry hours later delivers a dead code.
- **A magic link for registration.** Mail scanners open links, and a code can be typed on another device.
- **Locking out users with a second factor when mail goes off.** It turns an operator's setting into lost accounts.
