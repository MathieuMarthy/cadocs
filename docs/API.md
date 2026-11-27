# API

All routes are under `/api/v1`, and bodies are JSON unless stated otherwise. Today `/ws/pages/{page_id}`, and these REST routes, exist: `/auth/setup`, `/auth/login`, `/auth/logout`, `/auth/password-reset/{token}`, `/auth/forgot-password`, `/auth/register` with its confirmation and `/me/email-2fa` with its confirmation and `/me/email/confirm` when mail is on, `/session`, `/me`, `/me/totp` with its confirmation and recovery codes, `/me/identities`, `/admin/users`, and those of workspaces, members, invitations, groups, spaces, grants, pages, attachments, search, backlinks, revisions and comments, with the export, import and duplication of one page, apart from the space exports and bulk imports. Any other path under `/api/v1` answers `404` in the error shape. The rest is the target design.

## Conventions

- **Authentication**: an `HttpOnly`, `Secure`, `SameSite=Lax` session cookie, named `__Host-session` over HTTPS so that no other host can set it, and `session` over plain HTTP, where `Secure` and the prefix are impossible.
- **CSRF**: mutating requests with a session require the `X-CSRF-Token` header, whose value comes from `GET /session`. It is derived from the session token, so it needs no storage and lasts as long as the session. Routes that run before a session exists (`/auth/setup`, `/auth/login`, `/auth/password-reset/{token}`, `/auth/forgot-password`, `/auth/register` and its confirmation, and `/invitations/{token}/accept` for a new account) have no token to check: they rely on the `Origin` check and on requiring a JSON content type, which a cross-site form cannot send.
- **Ids**: UUIDv7 strings.
- **Pagination**: `?limit=` (maximum 100) and an opaque `?cursor=`. Responses have the form `{ "items": [...], "next_cursor": "..." | null }`.
- **Origin**: a request that sends an `Origin` other than the one of `CADOCS_PUBLIC_URL` gets `403` `forbidden`. Clients outside a browser send none.
- **Hidden resources**: a resource the caller cannot read returns `404`, not `403`, so that its existence does not leak. `403` means the caller can read it but cannot do this action.

Errors have one shape:

```json
{ "error": { "code": "validation_failed", "message": "Title is too long", "fields": { "title": "too_long" } } }
```

| Status | Codes |
| --- | --- |
| 400 | `invalid_request`, `validation_failed` |
| 401 | `unauthenticated` |
| 403 | `forbidden`, `csrf_failed` |
| 404 | `not_found` |
| 405 | `method_not_allowed` |
| 409 | `conflict` |
| 413 | `too_large` |
| 429 | `rate_limited`, with a `Retry-After` header in seconds |
| 500 | `internal` (details go to the log only) |
| 503 | `unavailable` (a limit is reached, or storage is failing) |

## REST routes

Minimum role per route: `member` is a workspace role. Space roles are `viewer` < `commenter` < `editor` < `admin` (ADR 0009). [PERMISSIONS.md](PERMISSIONS.md) is the reference when a route needs more detail.

### Auth and session

| Method | Route | Role | Purpose |
| --- | --- | --- | --- |
| GET | `/auth/setup` | none | Whether first-run setup is pending, and whether the instance sends mail |
| POST | `/auth/setup` | none, first run only | Create the first user and workspace |
| POST | `/auth/login` | none, rate limited | Email and password login |
| POST | `/auth/logout` | user | End the session, close its sockets |
| POST | `/auth/password-reset/{token}` | none, rate limited | Set a new password from a reset link, revoke all sessions, void the codes mailed for the account and untie every OIDC provider (ADR 0013, ADR 0028) |
| POST | `/auth/forgot-password` | none, rate limited | Mail the reset link of ADR 0013 to an address. Only with a mail relay (ADR 0026) |
| POST | `/auth/register` | none, rate limited | Mail a code that makes an account. Only with a mail relay and open registration (ADR 0026) |
| POST | `/auth/register/confirm` | none, rate limited | Make the account with the code, a name and a password |
| GET | `/auth/oidc/{provider}` | none, rate limited | Start an OIDC login: a redirect to the provider (ADR 0028) |
| GET | `/auth/oidc/{provider}/callback` | none, rate limited | Finish it: a redirect to `/` with the session cookie, or to the login screen |
| GET | `/me/identities` | user | The providers tied to the account, `{ providers, password }` (ADR 0028) |
| POST | `/me/identities/{provider}` | user, rate limited | Start linking a provider, with `current_password`, or a fresh session for an account with none: `{ url }` to send the browser to, and the flow cookie |
| DELETE | `/me/identities/{provider}` | user | Unlink it: `204`, `404` if not tied, `409` if it is the only way into the account |
| GET | `/session` | user | Current user, workspaces, CSRF token |
| PATCH | `/me` | user | Update name, email or password. Email and password need the current password, and a new password revokes other sessions and voids the codes mailed for the account |
| DELETE | `/me` | user, rate limited | Delete the account, with the current password, or a fresh session for an account with none. Refused with `409` for the last enabled owner of a workspace or the last enabled instance admin |
| POST | `/me/email/confirm` | user, rate limited | Change the address to the one a code went to. Only with a mail relay (ADR 0026) |
| POST | `/me/email-2fa` | user | Mail a code that turns on the second factor by mail. Only with a mail relay (ADR 0026) |
| POST | `/me/email-2fa/confirm` | user, rate limited | Turn the second factor on with the code |
| DELETE | `/me/email-2fa` | user, rate limited | Turn the second factor off, with the current password |
| POST | `/me/totp` | user, rate limited | Start the setup of an authenticator app, with the current password |
| POST | `/me/totp/confirm` | user, rate limited | Turn the app on with a code from it, and get recovery codes |
| POST | `/me/totp/recovery-codes` | user, rate limited | Replace the recovery codes, with the current password |
| DELETE | `/me/totp` | user, rate limited | Turn the app off, with the current password |

Bodies and answers:

- `GET /auth/setup` answers `{ required }`, `true` while no account exists. An instance with a mail relay adds `mail: true`, which tells the login screen to offer what needs mail, and `registration: true` unless `CADOCS_REGISTRATION=closed`. A field that would be `false` is left out.
- `POST /auth/setup` takes `{ email, name, password, workspace_name, code }`, and answers `201` with the session body and the cookie. `code` is the one of the link the server logs at each start while setup is pending (ADR 0021): missing or empty is `400` with `code: required`, any other value `400` with `code: wrong`. Once an account exists it answers `409`, whatever the code.
- `POST /auth/login` takes `{ email, password, code? }`, and answers `200` with the session body and the cookie. An unknown email, a wrong password and a disabled account all answer the same `401`, in the same time, and the code is then not looked at. An account with the second factor on, on an instance with a relay, also needs `code` (ADR 0026): without it the answer is a `400` with `code: required`, and a 6-digit code valid 10 minutes goes to the account's address, in place of any it had. That sending shares both limits of `/auth/forgot-password`: past the client's the answer is `429`, past the address's it stays the `400` and nothing is sent. A code that is not the one mailed is a `400` with `code: wrong`, which counts a try, and the 5th kills the code. No session exists before the right code, which also proves the address. Without a relay the factor is not asked for, and the server logs at start how many accounts have it on. An account with an authenticator app on needs `totp` instead, with or without a relay, and gets no mail: a code from the app or a recovery code. Without it the answer is a `400` with `totp: required`; a code that is not the app's for the step of now, the one before or the one after, a step at or before the last one used, or a recovery code the account does not hold is a `400` with `totp: wrong`. Each account gets 5 guesses at once, then one a minute, from any client: past it the answer is `429` with `Retry-After`. Only a right password reaches these checks. A recovery code works once. The `user` object of every answer carries `email_2fa` and `totp`.
- `GET /session` answers `{ user: { id, email, name, is_instance_admin, email_2fa, totp, language }, workspaces: [{ id, slug, name, role }], can_create_workspace, csrf_token }`, where `can_create_workspace` says whether `POST /workspaces` would let the user through. The other routes that answer the session body carry it too.
- `POST /auth/logout` and `DELETE /me` answer `204` and clear the cookie.
- `DELETE /me` takes `{ current_password }`, so a stolen session cannot delete the account: missing or wrong is a `400` that names `current_password`. An account with no password, made by a provider, sends `{}` and needs a session opened by a login in the last 10 minutes instead: an older one is a `400` with `session: too_old`, and the client offers to log in again. A `401` means the session or the password changed while the request waited.
- `PATCH /me` takes `{ name?, email?, password?, current_password?, language? }` and answers the `user` object. `language` is the interface's, one of `en` and `fr`, or `null` to follow the browser's; any other is a `400` with `language: invalid`, and it needs no password. A wrong or missing `current_password` is a `400` that names that field, and a `401` means the session or the password changed while the request waited. Without a relay a new email changes at once, an email in use is a `409` that names `email`, and nobody proved the new address: it is no longer verified, the second factor goes off, and a reset link the account had stops working. With a relay a new email does not change here (ADR 0026): the other fields do, and the answer still carries the current address, the same, in the same time, whether the new one is free. A free address gets a 6-digit code, valid 10 minutes, which replaces any it had; one with an enabled account gets a message that says so, with no code; a disabled one gets nothing. That sending shares both limits of `/auth/forgot-password`: past the client's the answer is `429` and nothing changes, past the address's nothing is sent. An email equal to the current one sends nothing.
- `POST /me/email/confirm` takes `{ email, code }` and answers the `user` object with the new address, verified, once the code mailed to it by `PATCH /me` is right. A missing code is a `400` with `code: required`, an invalid address a `400` that names `email`. A code that is missing, used, replaced, expired, another account's or for the current address is a `400` with `code: wrong`; one of this account's counts a try, and the 5th kills it. An address another account took after the code was sent is a `409` with `email: taken`, and the code stays. A reset link the account had stops working, since it may have gone to the old address, and the old address gets a message that names the new one. The second factor stays as it was, and its next codes go to the new address. It answers `404` without a relay.

An email is at most 254 bytes, a name or workspace name 1 to 100 characters, a password 8 to 1024 characters. These routes refuse bodies over 16 KiB.

Mail is written in English or French (step 62): the person's `language`, else the last browser language the server saw, else the language of the first workspace they joined that mail is written in, else English. A request with a session that carries `Accept-Language` sets the browser language at once when none is stored, and changes it only when the session is due for its extension (every 5 minutes), the first of `en` and `fr` by weight, and one that names neither leaves it as it was; the front end reports nothing else. A message to an address with no account, the code of `/auth/register`, follows the `Accept-Language` of that request. An account made from an invitation has that workspace as its first, so its mail takes the workspace's language until a browser is seen. A language that cannot be read gives English. There is no IP geolocation.

- `POST /auth/forgot-password` takes `{ email }` and answers `204` with no body, for every address: one with an account, one with none, one whose account is disabled. The lookup, the link and the message come after the answer, so its time tells nothing either. The account gets a message with a reset link valid one hour, which replaces any link it had. An address that is not one is a `400` that names `email`. Each client can ask 5 times at once, then once a minute, a budget every route that sends mail shares: past it the answer is `429`. Each address gets 3 messages at once, then one per 10 minutes, whoever asks: past it the answer is still `204`, and nothing is sent. Without a relay the route does not exist: a `404`, whatever the method or the body.
- `POST /auth/register` takes `{ email }` and answers `204` with no body, for every address, in the same time, as `/auth/forgot-password` does, and shares both of its limits. An address with no account gets a 6-digit code, valid 10 minutes, which replaces any code it had. An address with an account gets a message that says so, with no code, and a disabled one gets nothing. With mail off or registration closed the route does not exist: a `404`, whatever the method or the body. Before first-run setup it sends nothing, so the first account stays the operator's (ADR 0021).
- `POST /auth/register/confirm` takes `{ email, code, name, password }` and answers `201` with the session body and the cookie, as setup does: the account has no workspace and no instance role, and its address is verified. A missing, used, replaced, expired or other code is a `400` with `code: wrong`, and so is any code for an address that has an account. A wrong code counts a try, and the 5th kills the code. A refused field counts nothing. An address another account took after the code was sent is a `409` with `email: taken`, which only the holder of the code can see. It shares the limit of password guesses, since it hashes one. It exists only when `/auth/register` does.
- `POST /me/email-2fa` takes no body, or `{}`, and answers `204`: it mails a 6-digit code, valid 10 minutes, to the account's address, with the limits of `/auth/forgot-password`. `POST /me/email-2fa/confirm` takes `{ code }` and answers `204` once the factor is on, and ends every other session of the account: the code proves the address, so no login depends on one that does not work. A missing code is a `400` with `code: required`, any other a `400` with `code: wrong` that counts a try, as at login. A login code does not turn the factor on, nor does this code log in. `DELETE /me/email-2fa` takes `{ current_password }` and answers `204` once the factor is off: missing or wrong is a `400` that names `current_password`, so a stolen session cannot take the factor off. All three answer `404` without a relay. Confirming a code and checking the password share the limit of password guesses.
- `POST /me/totp` takes `{ current_password }` and answers `{ secret, uri }`: a new 160-bit secret in base32 and its `otpauth://totp/` URI, whose issuer is the host of `CADOCS_PUBLIC_URL`. Codes are RFC 6238 with SHA-1, 6 digits and 30 seconds. The secret waits for its first code, in place of any setup not finished, and changes nothing at login until then. A missing or wrong password is a `400` that names `current_password`, so a stolen session cannot tie the account to its own app; an app already on is a `409`. `POST /me/totp/confirm` takes `{ code }`, a code from the app, and answers `{ recovery_codes }`, ten codes of 80 random bits written `xxxx-xxxx-xxxx-xxxx`, shown only then: the app is on, that code's step is used, and every other session of the account ends. A missing code is a `400` with `code: required`; a wrong one, or no setup waiting, a `400` with `code: wrong`. `POST /me/totp/recovery-codes` takes `{ current_password }` and answers ten new codes in place of the old ones, or `409` while the app is off. `DELETE /me/totp` takes `{ current_password }` and answers `204` once the app, a setup waiting and the recovery codes are gone. None needs a relay, and the code by mail stays as it was, asked again once the app is off. Answers that carry a secret or codes are `Cache-Control: no-store`.
- `GET /auth/oidc/{provider}` and its callback are browser navigations, not calls from the front end, and answer `303`, except `404` for a provider `CADOCS_OIDC_PROVIDERS` does not name. The start sends the browser to the provider's authorization endpoint with `response_type=code`, `scope=openid email profile`, a `state`, a `nonce` and an S256 `code_challenge`, and sets a cookie that holds them for 10 minutes (`__Host-oidc` over HTTPS, `oidc` otherwise). The callback takes the provider's `code` and `state`, and needs that cookie, for that provider, with that state: anything else sends no request to the provider. It trades the code with the PKCE verifier, checks the ID token (RS256 by a key of the provider, issuer, audience, `azp`, expiry, nonce), then opens a session for the account linked to the token's `sub`, or makes one. It answers a redirect to `/` with the session cookie, or to `/login?oidc=` with `failed` (the provider refused or failed, the callback is not this browser's, the account is disabled), `taken` (no account is linked to this subject and one holds its address, which is never linked by itself) or `closed` (no account is linked, and registration is closed or setup pending). Either way the cookie of the login goes. Both share the limit of password guesses, since each makes requests to the provider.
- `GET /auth/setup` lists the ids of the OIDC providers as `oidc`, left out when there are none. `POST /me/identities/{provider}` is a `POST` with the CSRF token, not a navigation, so a page of another site cannot tie its own identity to an account, and it needs the current password, or for an account with none a fresh session, as `DELETE /me` does, so a stolen session cannot either: it answers `{ url }` and sets the flow cookie, whose fifth part is the user the link is for. The callback of such a flow trades the code as for a login, then ties the subject only if the browser's session is that user, and redirects to `/account?oidc=` with `linked`, `taken` (the subject, or a subject of this provider, is tied to an account already) or `failed`. A provider the user has can be tied once, and linking logs nobody in. An unknown provider is a `404`; one that cannot be reached, a `503`.
- `POST /auth/password-reset/{token}` takes `{ password }` and answers `204`, without opening a session. An unknown, used, replaced or expired token is a `404`. A refused password leaves the link usable.

`/auth/setup`, `/auth/login`, `/auth/password-reset/{token}`, `/auth/register/confirm`, `/invitations/{token}/accept`, `/me/email-2fa/confirm`, `/me/email/confirm`, `DELETE /me/email-2fa`, the four `/me/totp` routes and a `DELETE /me` or `PATCH /me` that checks the current password share one limit per client: a burst of 10, then one every 15 seconds. A hash that waits over 10 seconds for its turn answers `503`.

### Instance administration

| Method | Route | Role | Purpose |
| --- | --- | --- | --- |
| GET | `/admin/users` | instance admin | List users |
| PATCH, DELETE | `/admin/users/{user}` | instance admin | Disable, enable, grant instance admin, or delete |
| POST | `/admin/users/{user}/password-reset` | instance admin | Generate a one-time reset link (ADR 0013) |

- A user here is `{ id, email, name, is_instance_admin, disabled_at, created_at }`, with `disabled_at` `null` for an enabled account. `GET /admin/users` is paginated, oldest first, 50 per page by default.
- `PATCH /admin/users/{user}` takes `{ disabled?, is_instance_admin? }` and answers the user. Disabling ends the user's sessions and voids their reset link, and enabling brings back neither. A change that leaves no enabled instance admin is a `409`.
- `DELETE /admin/users/{user}` answers `204`, or `409` for the last enabled instance admin, or for an account that leaves a workspace with no enabled owner. The admin's own account is a `400`: it goes through `DELETE /me`, which asks for the password.
- `POST /admin/users/{user}/password-reset` answers `201` with `{ link, expires_at }`, once, as `<CADOCS_PUBLIC_URL>/reset-password/<token>`. A user has one link at a time: a new one replaces the last, and a password change voids it. A disabled user is a `409`. Disabling, demoting or deleting an admin voids the links they made, and links from the command line stay.
- Every answer under `/admin` has `Cache-Control: no-store`.

### Workspaces and members

| Method | Route | Role | Purpose |
| --- | --- | --- | --- |
| GET | `/workspaces` | user | List the caller's workspaces |
| POST | `/workspaces` | user, instance admin with `CADOCS_WORKSPACE_CREATION=admins` | Create a workspace (ADR 0024) |
| GET, PATCH | `/workspaces/{ws}` | member, admin to edit | Read or update |
| DELETE | `/workspaces/{ws}` | owner | Delete, with the workspace name as confirmation |
| GET | `/workspaces/{ws}/members` | member | List members |
| PATCH, DELETE | `/workspaces/{ws}/members/{user}` | admin, owner for owners | Change a role or remove a member. A member can remove themself |
| GET, POST | `/workspaces/{ws}/invitations` | admin | List pending invitations, or create one. Creating returns the link once |
| DELETE | `/invitations/{id}` | admin | Revoke an invitation |
| GET | `/invitations/{token}` | none | Workspace name and email, for the acceptance page |
| POST | `/invitations/{token}/accept` | none | Accept: a new user sends a name and password, a logged-in user with the invited email just joins |
| GET | `/workspaces/{ws}/invitees?q=` | admin, rate limited | Accounts of the instance to invite, by name or address (step 73) |
| GET | `/me/invitations` | user | The caller's pending invitations (step 73) |
| POST | `/me/invitations/{id}/accept` | user | Join with one of them, without the link |
| DELETE | `/me/invitations/{id}` | user | Decline one of them |
| GET, POST | `/workspaces/{ws}/groups` | member, admin to create | Groups (V1) |
| PATCH, DELETE | `/groups/{group}` | admin | Rename or delete a group (V1) |
| GET | `/groups/{group}/members` | member | Members of a group (V1) |
| PUT, DELETE | `/groups/{group}/members/{user}` | admin | Add or remove a member (V1) |
| GET | `/workspaces/{ws}/recent` | member | Recently updated pages in readable spaces |

- A workspace in a list is `{ id, slug, name, role }`, the caller's role included, as in `GET /session`. `GET /workspaces` is paginated, oldest first. Alone it also has `{ language, created_at, updated_at }`.
- `POST /workspaces` takes `{ name }` and answers `201` with the workspace, owned by the caller, or `403` when the setting keeps creation to instance admins. The slug comes from the name, gets a number if it is taken, and never changes.
- `PATCH /workspaces/{ws}` takes `{ name?, language? }` and answers the workspace. `language` is one of `ar`, `da`, `de`, `el`, `en`, `es`, `fi`, `fr`, `hu`, `it`, `nl`, `no`, `pt`, `ro`, `ru`, `sv`, `ta`, `tr` (ADR 0007).
- `DELETE /workspaces/{ws}` takes `{ name }`, which must match exactly, or it is a `400` with `name: mismatch`. It answers `204`.
- A member is `{ id, email, name, role, disabled, created_at }`, where `id` is the user's. `GET /workspaces/{ws}/members` is paginated, by account age.
- `PATCH /workspaces/{ws}/members/{user}` takes `{ role }` and answers the member. A change or a removal that leaves no enabled owner is a `409`, by the rule of [PERMISSIONS.md](PERMISSIONS.md#instance-actions). `DELETE` answers `204`.
- An invitation is `{ id, email, role, invited_by, expires_at, created_at }`, where `role` is `admin` or `member`. A link ends with the right to make it: when its inviter drops below `admin`, leaves, is removed, disabled or deleted. `GET /workspaces/{ws}/invitations` is paginated, oldest first, and leaves out expired ones.
- `POST /workspaces/{ws}/invitations` takes `{ email, role }` and answers `201` with the invitation and `link`, once, as `<CADOCS_PUBLIC_URL>/invite/<token>`, valid 7 days (ADR 0013). An email has one link per workspace: a new one replaces the last. The email of a member is a `409`, with `email: member`. With a relay the link is also mailed to the address, in the workspace's language when mail is written in it, else in English, with the limit of an address of `/auth/forgot-password` (3 messages at once, then one per 10 minutes) and not the limit of a client: past it the answer is the same, with no mail. The invitation is made for the account that holds the address then, if any, whose events sockets get an `invitation` event.
- `GET /workspaces/{ws}/invitees?q=` answers `{ items: [{ id, name, email }] }`: at most 10 enabled accounts of the whole instance that are not members, by name, whose address or a word of whose name starts with `q`, whatever the ASCII case. `q` needs 3 characters once trimmed, or it is a `400` with `q: too_short`, and at most 200. An admin gets 30 searches at once, then one a second: past it, `429`. A member gets `403`, anyone else `404`. Suggesting accounts of the whole instance is an accepted risk ([OPERATIONS.md](OPERATIONS.md#security)).
- `GET /me/invitations` lists the invitations made for the caller while their account held the address it still holds, usable, paginated, oldest first, as `{ id, workspace_id, workspace_name, role, invited_by_name, expires_at, created_at }`. An address taken after the invitation was made, by registration or by a change, does not list it: that needs the link, so an account cannot collect the invitations of an address by moving to it. Invitations made before step 73 have no account and need the link too.
- `POST /me/invitations/{id}/accept` takes no body and answers `200` with the session body, as accepting by link does, and `DELETE /me/invitations/{id}` answers `204`, the link going with it. An id that is not one of `GET /me/invitations` is a `404` for both.
- `DELETE /invitations/{id}` answers `204`, and `404` to anyone who is not an admin of its workspace.
- `GET /invitations/{token}` answers `{ workspace_id, workspace_name, email, role, expires_at, has_account }`. `has_account` tells the page to ask for a login rather than a password.
- `POST /invitations/{token}/accept` always takes a JSON body. `{ name, password }` makes the account of the invited email, and answers `201` with the session body and the cookie, or `409` with `email: taken` if that email has an account. `{}` joins with the current session, needs the CSRF token, and answers `200` with the session body: `401` with no session, `403` for an account with another email. A member keeps their role. A refused password leaves the link usable.
- A link that is unknown, used, replaced, revoked or expired, or whose workspace is gone, is a `404`. Answers that hold a link or a session have `Cache-Control: no-store`.
- A group is `{ id, workspace_id, name, member_count, created_at }`. `GET /workspaces/{ws}/groups` is paginated, oldest first. `POST` takes `{ name }`, at most 100 characters once trimmed, and answers `201` with the group; `PATCH /groups/{group}` takes the same and answers the group. A name another group of the workspace has is a `409` with `name: taken`. `DELETE /groups/{group}` answers `204`, and takes the group's memberships and grants with it.
- `GET /groups/{group}/members` lists members as `GET /workspaces/{ws}/members` does, paginated by account age. `PUT /groups/{group}/members/{user}` answers `204`, also for a user already in the group, and `404` for a user who is not a member of the workspace. `DELETE` answers `204`, or `404` for a user who is not in the group. A member who leaves the workspace leaves its groups.
- A group route answers `404` to whoever is not a member of the group's workspace, and `403` to a member who may not change it. A change of members and a deleted group recheck open sockets ([PERMISSIONS.md](PERMISSIONS.md#reacting-to-changes)).
| GET | `/jobs/{id}` | the user who started it | Status of a space export or a bulk import |
| GET | `/jobs/{id}/file` | the user who started it | The zip of a finished export |

### Spaces

| Method | Route | Role | Purpose |
| --- | --- | --- | --- |
| GET, POST | `/workspaces/{ws}/spaces` | member, admin to create | List readable spaces, or create one |
| GET | `/workspaces/{ws}/spaces/trash` | member | List the trashed spaces the caller can restore |
| GET | `/spaces/{space}` | viewer | Read |
| PATCH, DELETE | `/spaces/{space}` | space admin | Update or trash |
| POST | `/spaces/{space}/restore` | space admin | Restore from trash |
| GET | `/spaces/{space}/trash` | editor | List trashed pages |
| POST | `/spaces/{space}/export?format=md\|html` | viewer | Start the export of the space as a zip, as a job |
| GET, PUT | `/spaces/{space}/grants` | viewer, space admin to edit | Roles of users and groups |
| GET | `/spaces/{space}/tree?parent={id}` | viewer | One level of the tree, loaded lazily |

- A space is `{ id, workspace_id, slug, name, description, default_role, role, deleted_at, created_at, updated_at }`, where `role` is the caller's, after resolution. Both lists are paginated, oldest first.
- `POST /workspaces/{ws}/spaces` takes `{ name, description?, default_role? }` and answers `201` with the space. `default_role` is `viewer`, `commenter`, `editor` or `null`, the default, which leaves the space to those with a grant. A description is at most 1000 characters. The slug comes from the name, gets a number if it is taken in the workspace, and never changes.
- `PATCH /spaces/{space}` takes the same fields, all optional, and answers the space. `"default_role": null` clears it. Any other field that is missing or `null` keeps its value, as does a missing `default_role`.
- `DELETE /spaces/{space}` answers `204`. A trashed space answers `404` to everyone, its admins too, everywhere but `POST /spaces/{space}/restore` and the trash list. That route answers the space, or `403` for one that is not in the trash.
- A grant is `{ id, user_id, group_id, role, created_at }`, with one of `user_id` and `group_id` set. `GET /spaces/{space}/grants` is paginated, oldest first.
- `PUT /spaces/{space}/grants` takes `{ grants: [{ user_id | group_id, role }] }`, at most 500, and makes these the grants of the space: a target that already has one keeps its id and date, and the others lose theirs. It answers `204`. `grants` in `fields` says what is wrong: `too_many`, `invalid` (not exactly one target), `duplicate`, or `unknown_target` for a user or group outside the workspace. An unknown role is `role: invalid`. There is no version check: of two admins who edit at once, the last `PUT` wins.

### Pages

| Method | Route | Role | Purpose |
| --- | --- | --- | --- |
| POST | `/spaces/{space}/pages` | editor | Create a page |
| GET | `/pages/{page}` | viewer | Metadata, including the caller's effective `role` |
| GET | `/workspaces/{ws}/pages/by-slug/{slug}` | viewer | Same, from a front-end URL |
| PATCH | `/pages/{page}` | editor | Title |
| POST | `/pages/{page}/move` | editor | New parent and place among its siblings, possibly in another space, where `editor` is also required |
| POST | `/pages/{page}/template` | editor | Keep the page as a template of its space |
| GET | `/spaces/{space}/templates` | viewer | List the templates of a space |
| DELETE | `/templates/{id}` | editor | Delete a template |
| POST | `/templates/{id}/pages` | viewer of the template's space, editor of the target | Start a page from a template |
| POST | `/pages/{page}/duplicate` | viewer, and editor of the target | Copy a page and its descendants, possibly to another space |
| DELETE | `/pages/{page}` | editor | Move to trash |
| POST | `/pages/{page}/restore` | editor | Restore from trash |
| DELETE | `/pages/{page}/purge` | space admin | Delete a trashed page for good |
| GET | `/pages/{page}/export?format=md&attachments=0\|1` | viewer | Export. Without attachments, one file with absolute links. With them, a zip. `format=html` writes `<title>.html`, a standalone page with no script or style |
| POST | `/spaces/{space}/import?name=&parent_id=` | editor | Import Markdown or a zip as a new page |
| POST | `/spaces/{space}/bulk-import?parent_id=` | editor | Import a zip of Markdown files and folders as a tree of pages, as a job |
| GET | `/pages/{page}/revisions` | viewer | List revisions |
| GET | `/pages/{page}/revisions/{n}` | viewer | Rendered content of a revision |
| POST | `/pages/{page}/revisions/{n}/restore` | editor | Apply a revision as a new edit |

- An export writes the title as a first `#` heading, then the content, by the conventions of `cadocs-content`'s `markdown.rs`. Without attachments it is `<title>.md`, `text/markdown`, whose links to pages and attachments start with `CADOCS_PUBLIC_URL`. With `attachments=1` it is `<title>.zip`: `<title>.md` at its root, and each attachment the caller can download as `attachments/<id>/<file name>`, which the Markdown links to; an attachment they cannot download keeps its absolute link and stays out. A title or file name loses `/ \ : * ? " < > |` and control characters. Zip entries are stored, not compressed, and the zip is written as it is sent: a failure midway drops the connection rather than end a truncated file. A missing `format` is `md`, anything else `400` with `format: invalid`, and an `attachments` other than `0` or `1` is `attachments: invalid`.
- A space export answers `202` with `{ id, kind: "space_export", status: "running" }`; `format` is `md` (default) or `html`, anything else `400` with `format: invalid`. `GET /jobs/{id}` answers the same with `status` `running`, `done` (with the zip's `size`) or `failed`, and `GET /jobs/{id}/file` the zip as `<space name>.zip` (`409` until it is done). A job is the starter's alone, and ends for them once they lose read access to the space: any other caller gets `404`. The zip holds each page as `<title>.md` (or `.html`), with the children of a page in a folder of the same name beside it, a title already taken in its folder becoming `<title> (2)`, and each attachment the starter can download once, as `attachments/<id>/<file name>` at the root, which pages link with `../` for each folder. Trashed pages are left out. Jobs live in memory: one runs at a time, the others wait, at most 8 are running or hold a file, 2 per user (`429` past that), an ended one stays an hour (8 per user for those with no file, the oldest going first), and a restart drops them all. A page that cannot be read is left out, and a space of over 10,000 pages fails the job.
- An import is the raw file as the body, like an upload, and answers `201` with the new page, last at the root of the space or under `parent_id`. `Content-Type: application/zip` (or `application/x-zip-compressed`) reads a zip, anything else Markdown, in UTF-8. The title is a first `#` heading, which is taken out of the content, else `name` without `.md`, `.markdown` or `.zip`. Links to the server's own pages and attachments, relative or under `CADOCS_PUBLIC_URL`, read as page links, images and files; in a zip, a link to one of its entries becomes a copy of that entry, as a new attachment of the page, named after the entry and typed by its extension. Synchronous, not a job: one page is quick, and its body goes to `data/tmp`, which a restart empties. Limits: a body, or what a zip unpacks to, over `CADOCS_MAX_IMPORT_MB` or Markdown over 2 MiB is `413`, as is an entry over `CADOCS_MAX_UPLOAD_MB` or the quota. `file` in `fields` says what else is wrong: `not_utf8`, `invalid` (not a zip, zip64, or a directory over 1 MiB), `too_many_entries` (over 1000), `compression` (entries are stored or deflated, ADR 0029), `no_markdown` or `several_markdown` (one `.md` file at the root of the zip). A failure once the page exists moves it to the trash.
- A bulk import is a zip as the body, whatever its type, and answers `202` with `{ id, kind: "space_import", status: "running" }`. It reads what a space export writes: each `.md` or `.markdown` entry is a page, titled as for an import of one page, and what the folder of the same name holds goes under it. A folder with no such file is an empty page named after it. Files in `attachments/<uuid>/` at the root, where an export puts attachments, hidden entries and `__MACOSX/` are never pages. Pages are added under `parent_id`, or at the root, siblings in the order of their names. Links are relative to the folder of their file: one to a page of the zip becomes a page link, one to another entry an attachment, stored once. A job is `done` with `pages`, their number, or `failed` with a `message` that names the entry; a failed one moves all its pages to the trash. It shares the slots and the single turn of space exports, and `GET /jobs/{id}/file` is `404` for it. A restart drops it, and keeps the pages made so far. Refused at once: `parent_id: unknown`, a body, or what it unpacks to, over `CADOCS_MAX_IMPORT_MB`, or a page over 2 MiB (`413`), and `file` in `fields` as `invalid`, `compression`, `too_many_entries` (over 5000), `too_many_pages` (over 1000), `too_deep` (over 100 levels) or `no_markdown`. Found by the job: text that is not UTF-8, an attachment over `CADOCS_MAX_UPLOAD_MB` or the quota, a tree too deep under `parent_id`.
- `POST /pages/{page}/duplicate` takes `{ space_id?, parent_id?, after? }`, placed as a move places a page, and answers `201` with the top copy. The caller reads the page and is an `editor` of the target space, in the same workspace (`space_id: other_workspace`); a parent and a sibling are refused as for a move, and the copy under the page itself is allowed, since it takes what sat under the page when it started. The copy takes the live descendants, in their order, each with its title and content, written through its room by the caller, and `created_by` the caller. Each attachment the content shows, and the caller can download, gets a copy, stored in the target page's workspace and shared by the copies that show it; links to the source's pages are left as they are. One `page.created` event, for the top copy, announces the whole copy. Limits: at most 500 pages and 5000 distinct files, else `413`, as is a copy over the workspace quota; the depth of the tree as for a move (`parent_id: too_deep`). Synchronous, like an import. Each source is checked again before it is read, so a caller who loses read access, or a source trashed or moved to another space meanwhile, stops the copy with `404`. A failure once the rows exist moves the pages of the copy to the trash, even when the caller can no longer edit the target; a page someone else moved under the copy meanwhile stays live, moved up to where the copy sat.
- Templates belong to a space. `POST /pages/{page}/template` takes no body, keeps the title and content of the page as they are now, flushed or not, and answers `201` with `{ id, space_id, title, created_by, created_at }`. A space holds at most 200, else `409`. `GET /spaces/{space}/templates` is paginated, oldest first. `POST /templates/{id}/pages` takes `{ space_id?, parent_id?, after? }`, placed as a move places a page, in the template's space when `space_id` is missing, and answers `201` with the new page, which has the template's title and content, written through its room by the caller. Each attachment the content shows, and the caller can download, gets a copy, as for a duplicate; one they cannot, or whose file is gone, keeps its id and shows broken. A template holds a snapshot, so its source page can change or go. The target must be in the template's workspace (`space_id: other_workspace`). A failure once the page exists moves it to the trash. A template of a space the caller cannot read is `404`.
- A revision summary is `{ number, title, authors: [{ id, name }], created_at }`, the most recent first, paginated with the number as the cursor. `authors` are who wrote since the revision before, minus deleted accounts. `GET /pages/{page}/revisions/{n}` adds `html`, rendered by the server with the titles of the linked pages the caller can open (`page not found` for the others). A number that is not a positive integer is `400` with `n: invalid`, an unknown one `404`.
- A restore answers `204`. It first keeps the current content as a revision when it differs from the latest one, then replaces the title and content through the room in one transaction: open clients receive it as an edit, and the next revision job records it with the restorer as its author.

- A page is `{ id, workspace_id, space_id, parent_id, position, slug, title, role, has_children, created_by, updated_by, deleted_at, deleted_by, created_at, updated_at }`. `GET /pages/{page}` also answers `ancestors`, the pages above it as `[{ id, title }]`, the root first, for the breadcrumb and the tree. They are live and in its space, so the caller's role covers them. `role` is the caller's on the space, `has_children` says whether a live page sits under it, and `position` is an opaque key that sorts among its siblings. A trashed page answers `404` everywhere but the trash list, `restore` and `purge`.
- `POST /spaces/{space}/pages` takes `{ parent_id?, after? }` and answers `201` with the page. A new page has an empty title, which its client writes through the page socket. `POST /pages/{page}/move` takes `{ space_id?, parent_id?, after? }` and answers the page. A missing or `null` `parent_id` is the root of the space, a missing `space_id` the current one.
- `after` is the id of the live sibling to go after, `null` to go first, and when it is missing the page goes last. The server computes the key, so two concurrent moves to one place cannot collide.
- A refusal names its field: `parent_id` is `unknown` (not a live page of the target space), `cycle` (the page itself or a descendant) or `too_deep`, `after` is `unknown`, `space_id` is `other_workspace`. A tree has at most 100 levels, as SQLite refuses to cascade a delete through more than 1000.
- `GET /spaces/{space}/tree` is paginated, in tree order, without trashed pages. A missing `parent` is the root, and a `parent` that is not a live page of the space is a `404`.
- `GET /spaces/{space}/trash` holds every trashed page of the space, its descendants too, the last trashed first. `GET /workspaces/{ws}/recent` holds the pages of readable spaces, the last changed first: an edit and a move count as a change, a trash or a restore does not. Both are paginated, and a cursor the server did not give is a `400` with `cursor: invalid`.
- `DELETE /pages/{page}` answers `204`. `POST /pages/{page}/restore` answers the page, or `403` for one that is not in the trash. `DELETE /pages/{page}/purge` answers `204`, or `403` for one that is not in the trash, and takes everything under the page, including what was trashed separately before it.

The Yjs document holds the title, and the server copies it to `pages.title` when flushing. `PATCH` does not write that column: it applies a Yjs update through the room, like any other edit, so a later flush cannot overwrite it.

`PATCH /pages/{page}` takes `{ title }`, at most 500 characters once trimmed, and answers the page with that title. An empty title is allowed. The other fields, and what `GET` answers, follow at the next flush. A `503` means the room cannot take the edit: it is at capacity, or its flushes are failing. A client can write a longer title through the socket: the document keeps it, and `pages.title` holds its first 500 characters.

The role in `GET /pages/{page}` tells the front end whether to make the editor read-only.

### Comments

| Method | Route | Role | Purpose |
| --- | --- | --- | --- |
| GET | `/pages/{page}/comments` | viewer | List threads |
| POST | `/pages/{page}/comments` | commenter | Create a comment or a reply, with an optional anchor (ADR 0012) |
| PATCH, DELETE | `/comments/{id}` | author, or space admin | Edit or delete |
| POST | `/comments/{id}/resolve` | commenter | Resolve or reopen |
| GET | `/me/notifications` | user | Notifications: mentions, replies and comments on one's pages |
| POST | `/me/notifications/read` | user | Mark some or all as read |
| PUT | `/me/email-notifications` | user, relay | Turn the mail of notifications on or off |

Bodies and answers:
- A comment is `{ id, page_id, parent_id, author, body, anchor, resolved_at, resolved_by, created_at, updated_at }`. `author` and `resolved_by` are `{ id, name }`, or `null` once the account is deleted. `parent_id` is `null` for a thread. `anchor` is `{ start, end, text }` or `null` for a reply and a comment on the whole page: `start` and `end` are base64 of encoded Yjs `RelativePosition` values, stored as sent (ADR 0012), and `text` is the quoted text.
- `GET /pages/{page}/comments` is paginated over threads, oldest first, each with a `replies` array holding all its replies, oldest first. Comments of a trashed page answer `404`, as the page does.
- `POST /pages/{page}/comments` takes `{ body, parent_id?, anchor? }` and answers `201` with the comment. `body` is at most 10,000 characters once trimmed, and not empty. A position is at most 1024 bytes decoded and not empty (`anchor: invalid`), and `text` at most 1000 characters (`anchor: too_long`). A reply's `parent_id` names a thread of the same page (`parent_id: unknown`), not a reply (`parent_id: nested`), and a reply takes no anchor (`anchor: reply`). A thread takes at most 100 replies: one more is a `409`.
- `PATCH /comments/{id}` takes `{ body }` and answers the comment with a new `updated_at`. `DELETE` answers `204`, and deleting a thread deletes its replies. Each needs `commenter` for one's own comment, a space `admin` for another's.
- `POST /comments/{id}/resolve` takes `{ resolved }` and answers the comment. Resolving a resolved thread keeps who resolved it first, and neither leaves `updated_at` changed. A reply cannot be resolved: `409`.
- Each account may create or edit 30 comments at once, then one every 2 seconds; past it the answer is `429` with `Retry-After`.
- Every change sends the `comment` event to the readers of the space.
- A body is plain text, stored and answered as sent, which a client shows as text, never as HTML or Markdown. In it, `<user:ID>` mentions a user and `<page:ID>` a page, where `ID` is 1 to 64 ASCII letters, digits, `-` or `_`; anything else, a malformed token included, is text. A client shows a user mention as `@` and the member's name, and a page mention as a link with the page's title, each as the reader may see it (`GET /workspaces/{ws}/members`, `GET /pages/{page}`), and someone else as `@someone` or `page not found`. Creating or editing a comment records a mention of each member of the page's workspace its body names, and drops those it no longer names. Each user newly named, the author apart, gets a notification (below).

Notifications (step 41):
- `GET /me/notifications` answers `{ items, unread }`: the newest 100 notifications of the caller, newest first, among those whose page the caller can still read and that is not in the trash. `unread` counts those items, not older ones. An item is `{ id, kind, page: { id, space_id, title }, comment_id, actor, read, created_at }`. `kind` is `mention` (the caller is named in a comment, or in a page body as the last index found it, `comment_id` then `null`), `reply` (someone answered the caller's thread) or `comment` (someone commented on the caller's page: the one they created). A user gets one notification per comment, the first of those in that order, and never one for their own action, nor one for a page they cannot read when it happens: regaining access later shows nothing of it. `actor` is `{ id, name }`, `null` once the account is deleted.
- `POST /me/notifications/read` takes `{ ids? }` and answers `204`: it marks those of the caller that are unread as read, or all of theirs without `ids`. An id of someone else is ignored. There is no way to mark one unread again.
- Mail (step 42, with a relay only: without one `PUT /me/email-notifications` is a `404` like the other mail routes). It takes `{ enabled }` and answers `204`; the choice is `email_notifications` in the `user` object, off by default. For a user who chose it, a notification nobody read starts a job one minute later (`CADOCS_MAIL_DELAY` is not a setting: a store option for tests), and the notifications that arrive meanwhile share it. The job mails one plain text message with those still unread and not yet mailed, each as "who did what on which page" and a link, at most 20 lines, and marks them as mailed. A notification read, or on a page the user can no longer read, before the job runs is not mailed, and neither is one while the choice is off.
- A notification goes when its comment or page does, and a mention notification when an edit stops naming its user. A comment's `notification` event follows its `comment` event; a body mention's is sent when the indexing job stores it, a few seconds after the edit's flush.

### Attachments

| Method | Route | Role | Purpose |
| --- | --- | --- | --- |
| POST | `/pages/{page}/attachments?name=` | editor | Streaming upload |
| GET | `/attachments/{id}` | viewer of a referencing page | Download |
| DELETE | `/attachments/{id}` | editor of every referencing page | Delete, for every page that references it |

- An upload is the file itself as the body, not multipart: it streams to disk with no parser in between, and a browser sends a `File` as is. `name` is the file name, percent-encoded, `+` for a space, 1 to 255 characters with no control character, else a `400` naming `name`. `Content-Type` is the file's type: its parameters are dropped, and a missing or malformed one is stored as `application/octet-stream`. It answers `201` with `{ id, workspace_id, file_name, content_type, size, uploaded_by, created_at }`.
- A file over `CADOCS_MAX_UPLOAD_MB`, or over what the workspace quota leaves, is a `413`: at once when `Content-Length` announces it, else as soon as the stream passes the limit. A full quota refuses even an empty file. A storage failure, a full disk among them, is a `500`, and nothing is recorded.
- A download sends the stored type, `Content-Length`, an `ETag` of the SHA-256 and `Cache-Control: private, no-cache`, so a reader revalidates and gets `304` while the file is unchanged. `Content-Disposition` is `inline` for PNG, JPEG, GIF, WebP and AVIF, `attachment` for anything else, SVG included, with the name in `filename*`. Its `Content-Security-Policy` is `default-src 'none'; sandbox`, so a file opened by itself runs nothing.
- `DELETE` answers `204`. It needs `editor` on every page that references the file, a page in the trash included, since it removes the file from all of them. Someone who reads a referencing page but misses that role on one of them gets `403`, and someone who reads none of them `404`.

### Search

| Method | Route | Role | Purpose |
| --- | --- | --- | --- |
| GET | `/workspaces/{ws}/search?q=&space=` | member | Results limited to readable spaces |

- `q` is percent-encoded, `+` for a space, 1 to 200 characters once trimmed, with no control character, else a `400` naming `q` (`required`, `too_long` or `invalid`). It is cut into words (runs of letters and digits), each lowercased, folded to its letters without diacritics and stemmed in the workspace's language (ADR 0007), and a page must hold every one. The last word is a prefix while the query ends right after it, so results follow typing. A query with no word answers no result. Nothing in `q` is read as FTS5 syntax.
- `space` keeps the results of one space. A space the caller cannot read gives no result, not an error.
- It answers `200` with `{ items: [{ id, space_id, title, excerpt }] }`: at most 20 live pages of live spaces the caller can read, the best first, a word in the title counting five times one in the body. There is no further page. `excerpt` is a list of `{ text, match }` pieces, about 200 characters of the body around its first match, or from its start, with `…` where it was cut. A client shows the `match` pieces marked, and never reads a piece as HTML.
| GET | `/pages/{page}/backlinks` | reader of the page | Pages that link to it |

- `GET /pages/{page}/backlinks` answers `200` with `{ items: [{ id, space_id, title }] }`: the live pages of live spaces the caller can read whose last index found a link to the page, by title. `404` for a page the caller cannot read. Same delay as search.
- A page is indexed a few seconds after an edit (see [Search](DATA_MODEL.md#search)), so a page just written may not show yet.

### Sharing

| Method | Route | Role | Purpose |
| --- | --- | --- | --- |
| GET, POST | `/pages/{page}/shares` | editor | List or create public links |
| DELETE | `/shares/{id}` | editor | Revoke |
| GET | `/s/{token}` (outside `/api`) | none | Public page, rendered as HTML on the server |
| GET | `/s/{token}/p/{slug}` (outside `/api`) | none | A descendant, if the share includes children |
| GET | `/s/{token}/files/{id}` (outside `/api`) | none | An attachment referenced by a page the share covers |

Public routes are rate limited, send an `ETag` of what they serve, and render through the bounded reader described in [ARCHITECTURE.md](ARCHITECTURE.md#reading-a-document-outside-a-room).

- A share is `{ id, page_id, token, include_children, indexable, created_by, expires_at, created_at }`. `token` is 128 random bits in hex (32 characters), and the public address is `/s/{token}`. Editors see the tokens of the page's shares again, since the share dialog lists them.
- `POST /pages/{page}/shares` takes `{ include_children?, indexable?, expires_at? }`, both booleans `false` by default, `expires_at` in milliseconds or `null` for none, and answers `201` with the share. An `expires_at` not in the future is a `400` with `expires_at: past`, and an unknown field a `400`. A page can have several shares, each with its own token.
- `GET /pages/{page}/shares` answers `{ items, next_cursor }`, oldest first, expired shares left out. `DELETE /shares/{id}` answers `204`, and the address answers `404` at once. Each needs `editor` on the page: a reader gets `403`, anyone else `404`.
- `GET /s/{token}` answers the shared page as HTML; with `include_children`, `GET /s/{token}/p/{slug}` answers a live page under it by its `slug`, and the shared page by its own. Links in content to a covered page point under the share, with its title; links to any other page show `page not found` and no title. A page with children lists up to 200 of them below its content, and one below the shared page leads back through its ancestors.
- `GET /s/{token}/files/{id}` answers an attachment that the content of a live page the share covers shows, as a download does, with `Cache-Control: no-cache` and `X-Robots-Tag: noindex`.
- An unknown token, a revoked or expired share, a share whose creator can no longer manage the shares of its page (left the workspace, lost `editor`, account disabled or deleted; ADR 0030), a page or space in the trash, a page or file the share does not cover: each is the same `404` page.
- Pages answer `Cache-Control: no-cache`, an `ETag` that hashes the body and `304` while it matches, edits not yet flushed included, `X-Robots-Tag: noindex` unless the share is `indexable`, and `Content-Security-Policy: default-src 'none'; script-src 'none'; img-src 'self'; style-src 'sha256-…'; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'`, the one inline style sheet allowed by its hash. With `CADOCS_S3_DOWNLOADS=redirect`, `img-src` adds the endpoint's origin. Each client may make 120 requests to `/s/` at once, then one every 100 ms; past it the answer is `429` with `Retry-After`. At most 16 pages render or send at once, 4 for one client, and an answer not sent within 60 seconds ends its connection, since a client that does not read holds its turn: past it a page answers `503` with `Retry-After: 1`.

### Operations

| Method | Route | Purpose |
| --- | --- | --- |
| GET | `/health` (outside `/api`) | Liveness, with a database check. `503` while flushes are failing |

## Page WebSocket protocol

### Connection

```
GET /ws/pages/{page_id}?schema={fingerprint}
Upgrade: websocket
Cookie: __Host-session=...
Origin: https://wiki.example.org
```

The URL matches what `y-websocket` builds: the server URL followed by `/` and the room name, then its `params`. Checks happen before the upgrade:

1. `Origin` must match the configured public URL. Cookies are sent on cross-site WebSocket requests, so this check is what prevents cross-site WebSocket hijacking.
2. The session must be valid, otherwise `401`.
3. `schema` must be the fingerprint of the editor schema this server ships: FNV-1a (32 bits, 8 lowercase hex digits) of `crates/cadocs-content/tests/tiptap-schema.json`, which the editor computes from its own schema (`web/src/editor/fingerprint.ts`). Otherwise the socket opens and closes at once with `4426`, before any frame is read, since a client of another schema would delete the nodes and marks it lacks (ADR 0016). What such a tab typed while the server was down is dropped with its frames, and lost on reload.
4. The page must exist and be readable, otherwise `404`.
5. The resulting role sets the connection mode: read-write for `editor` and above, read-only otherwise. A `commenter` is read-only on the wire: comments go through REST.

A client outside a browser sends no `Origin`, and is accepted. At capacity, during shutdown or when the database does not answer, the answer is `503`. An upgrade extends the session like any request, so the `101` may carry a `Set-Cookie`.

Since browsers cannot read the status of a failed upgrade, the client confirms its session through REST after repeated failures.

### Frames

Binary frames only, using the y-protocols encoding. Each frame starts with a varuint message type.

| Type | Name | Payload |
| --- | --- | --- |
| 0 | Sync | Sub-type, then content (below) |
| 1 | Awareness | Length-prefixed awareness update |
| 2 | Auth | `0` then a reason string: permission denied |
| 3 | QueryAwareness | Empty: asks for all awareness states |

| Sync sub-type | Name | Content |
| --- | --- | --- |
| 0 | SyncStep1 | The sender's state vector |
| 1 | SyncStep2 | The update the receiver is missing |
| 2 | Update | An incremental update |

### Sequence

```
client                                  server
  │ ───────── HTTP upgrade ──────────────► │  auth, load room
  │ ◄──────── SyncStep1 ───────────────── │
  │ ◄──────── Awareness (all states) ──── │
  │ ───────── SyncStep1 ─────────────────► │
  │ ◄──────── SyncStep2 ───────────────── │
  │ ───────── SyncStep2 ─────────────────► │  applied if writable
  │ ───────── Update ────────────────────► │  applied, broadcast, queued
  │ ◄──────── Update (from others) ─────── │
  │ ───────── Awareness ─────────────────► │  broadcast
  │ ◄──────── Awareness (echo) ────────── │
```

### Rules

- **Read-only.** `SyncStep2` and `Update` frames from a read-only client are dropped, and the server sends one Auth frame per socket with the reason `read-only`. `y-websocket` answers every SyncStep1 with a SyncStep2, so a read-only client gets this frame at each connection, and it only logs a warning. Awareness is still accepted, so a reader's presence shows.
- **Awareness echo.** An awareness change goes back to its sender too, as with `y-websocket`'s server. That client reconnects after 30 seconds without a message and cannot see WebSocket pings, so a client alone in a room lives on the echo of its 15-second heartbeat.
- **Awareness ownership.** The server tracks the client ids each socket announced, and removes them on close with a null state at the same clock, as `y-websocket`'s server does. A client that reconnects with an unchanged clock therefore shows again at its next change, at most one heartbeat later. An entry about an id held by another user's socket is dropped and the socket stays, because `y-websocket` echoes what it hears and removes a peer it has not heard from for 30 seconds. A socket of the same user takes the id over, which is what a reconnect does while its old socket is still open.
- **Awareness limits.** A socket may introduce 16 client ids over its life, a state must be valid JSON of at most 64 KiB, and a frame may hold 272 entries. Otherwise the socket closes with `1002`. Removing an unknown id is ignored. A room holds at most 256 states and 1 MiB of them: past that a new or larger state is dropped and the socket stays, so that person shows no cursor until there is space, and their heartbeat tries again. A room also remembers 4096 ids at most, gone ones included: past that it forgets the clocks of those that left.
- **Cursor shape.** The `cursor` field of a state is `null` or absent, or holds `anchor` and `head`, each a `Y.RelativePosition` as JSON: at least one of `item`, `type` and `tname`, where `item` and `type` hold integer `client` and `clock`, `tname` is a string and `assoc` an integer. Anything else closes the socket with `1002`, because the cursor plugin of every other client would throw on it.
- **Limits.** A frame over the maximum size closes the socket, and so does a frame declaring more entries than its bytes can hold (yrs would reserve memory for that count first). Each client has a send queue bounded in frames (`CADOCS_SEND_QUEUE`) and in bytes (`CADOCS_MAX_FRAME_BYTES`). A client that stops reading is closed rather than buffered.
- **Keep-alive.** The server pings periodically, and closes a socket with `4408` when it stays silent past the timeout, or when one send takes longer than that.
- **Malformed frames** close the socket with `1002`, without affecting the room. An Auth frame, an unknown message type or a text frame from a client counts as malformed.
- **One message per frame.** `y-websocket` reads a single message from a frame, so the server never packs several. It reads the first message of a client frame and ignores any bytes after it, as `y-websocket`'s own server does.

### Close codes

| Code | Meaning | Client behaviour |
| --- | --- | --- |
| 1000 | Normal | None |
| 1001 | Server shutting down | Reconnect with backoff |
| 1002 | Malformed frame | Report an error |
| 1009 | Frame too large | Report an error |
| 1013 | Storage is failing | Reconnect with backoff, keep local edits |
| 4401 | Session ended | Stop, go to login |
| 4403 | Access revoked | Stop, leave the page |
| 4404 | Page deleted | Stop, leave the page |
| 4408 | Client too slow, or idle timeout | Reconnect with backoff |
| 4409 | Role changed, page still readable | Refetch the role, reconnect in the new mode |
| 4426 | `schema` is not the server's: the client runs another release | Stop, ask for a reload |

By default, `y-websocket` reconnects after any close code except 4400 to 4499, where it stops for good. That range includes `4408` and `4409`, so the front end passes its own `shouldReconnect`, which stops only on `4401`, `4403`, `4404` and `4426`. On `4409` it updates the editor's mode and lets the provider reconnect.

## Events WebSocket

```
GET /ws/events?workspace={workspace_id}      MVP
```

The checks before the upgrade are the same as for a page socket, with workspace membership in place of page access. Frames are server to client, as JSON text (ADR 0011). A client frame other than a pong closes the socket with `1002`. Close codes `1001`, `4401`, `4403`, `4404` (the workspace was deleted) and `4408` apply. A missing `workspace` parameter answers `404`, like a workspace the user is not a member of.

Events are hints. The client refetches through REST, and refetches the tree after every reconnect.

```json
{ "type": "page.moved", "space": "<id>", "page": "<id>" }
```

| Type | Fields | Meaning |
| --- | --- | --- |
| `page.created`, `page.moved`, `page.renamed`, `page.trashed`, `page.restored`, `page.purged` | `space`, `page` | The tree or the trash changed. A move across spaces is sent to both |
| `space.created`, `space.updated`, `space.trashed`, `space.restored`, `space.purged` | `space` | The space list changed. A reader of a new space may also get `access.changed` |
| `access.changed` | none | The user's permissions changed, reload the space list |
| `comment` | `space`, `page` | A comment of the page was written, edited, resolved, reopened or deleted |
| `notification` | `id` | A new notification, sent to its user alone (a socket of the workspace of the page it is about) |
| `invitation` | `id` | A new invitation for the user, sent to their sockets of every workspace: reload `GET /me/invitations` |
| `resync` | none | Events were dropped because the client lagged, reload everything shown |
