# ADR 0027: An authenticator app replaces the mailed code, needs no relay, and has a command-line way back

Status: accepted. Builds on ADR 0026, whose mailed code stays the factor of an account with no app.

## Context

Step 47 adds TOTP (RFC 6238) beside the second factor by mail of ADR 0026. Three questions had no answer in the design: what a login asks when both factors are on, whether the app depends on a relay as the mailed code does, and how an account comes back when its owner loses both the app and the recovery codes.

## Decision

**The app replaces the mailed code.** With the app on, a login asks for a code from the app, or a recovery code, in a `totp` field, and sends no mail, whatever the mail setting says. The mail factor stays stored and comes back once the app is turned off. Asking for both would add a step without adding a proof: the app is the stronger factor, and a mailbox is often reached with the same password.

**No relay needed.** The app works on an instance with mail off. Its routes and its screen exist on every instance, unlike those of ADR 0026, since nothing in it sends a message.

**The secret is stored readable.** The server computes the expected code from the shared secret, so it cannot keep only a hash, as it does for passwords and recovery codes. Anyone who reads the data folder or a backup can compute an account's codes. That person can already read every page, so the factor protects against a stolen password, not a stolen disk. The Security section of `OPERATIONS.md` says so.

**`cadocs reset-totp <email>` is the way back.** A user who lost the app and the recovery codes cannot log in, and no screen can turn the app off for them: neither a password reset link nor an instance admin can, or either would undo the factor for whoever holds a stolen password or an admin account. The operator, who can already reset any password from the command line, turns it off with this command, after checking who is asking. It turns off the app and deletes its recovery codes.

## Consequences

- A user with both factors on never receives a code by mail at login. The account screen says so in the mail section.
- An instance with no relay gets a second factor, which ADR 0026 left to mail.
- Backups hold the TOTP secrets in clear, as they hold the pages.
- Recovering an account with no app and no codes needs the operator.

## Rejected alternatives

- **Both factors at each login.** Two codes, one of them weaker, for no added proof.
- **Encrypting the secret with a key kept beside the data.** The key would sit in the same folder or environment as the data, so it hides nothing from whoever reads them, and it adds a key to lose.
- **Letting an instance admin turn off a user's app.** An admin account becomes enough to strip the factor from any user, which is what the factor guards against.
- **Recovery through the mailed code.** It makes the mailbox enough to bypass the app, so the app would be no stronger than the mail factor.
