# ADR 0013: Account flows work without email

Status: accepted

## Context

"No mandatory external service" rules out requiring SMTP, which only arrives as an option in V1. Invitations and password recovery still have to work on an MVP instance with no mail server.

## Decision

- **Registration is closed.** An account comes from first-run setup, an invitation, or OIDC in V1.
- **Invitations are links.** Creating one returns a single-use link, valid 7 days, that the admin passes on by any channel. Only the token hash is stored.
- **Password resets are links.** An instance admin generates one, valid 1 hour, or an operator runs `cadocs reset-password <email>` on the server. Using it revokes all the user's sessions.
- **With SMTP configured (V1)**, the same tokens are also sent by email, and a public "forgot password" form appears. It always answers the same way, whether or not the account exists.

## Consequences

- The MVP has no mail dependency.
- The last admin can always recover through the CLI, which requires access to the server.
- An email address is unverified until SMTP exists. It is an identifier only, and nothing is sent to it.

## Rejected alternatives

- **SMTP in the MVP.** A working invitation would depend on an external service.
- **Open registration.** Unsuitable as a default for a private wiki.
- **Security questions or recovery codes.** More surface to build, for a flow an admin handles in seconds.
