# API

All routes are under `/api/v1`, and bodies are JSON unless stated otherwise. Today `/ws/pages/{page_id}`, and these REST routes, exist: `/auth/setup`, `/auth/login`, `/auth/logout`, `/auth/password-reset/{token}`, `/session`, `/me`, `/admin/users`, and those of workspaces, members, invitations, spaces, grants, pages and attachments, apart from `/duplicate`, the imports, the exports and the revisions. Any other path under `/api/v1` answers `404` in the error shape. The rest is the target design.

## Conventions

- **Authentication**: an `HttpOnly`, `Secure`, `SameSite=Lax` session cookie.
- **CSRF**: mutating requests with a session require the `X-CSRF-Token` header, whose value comes from `GET /session`. It is derived from the session token, so it needs no storage and lasts as long as the session. Routes that run before a session exists (`/auth/setup`, `/auth/login`, `/auth/password-reset/{token}`, and `/invitations/{token}/accept` for a new account) have no token to check: they rely on the `Origin` check and on requiring a JSON content type, which a cross-site form cannot send.
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
| GET | `/auth/setup` | none | Whether first-run setup is pending |
| POST | `/auth/setup` | none, first run only | Create the first user and workspace |
| POST | `/auth/login` | none, rate limited | Email and password login |
| POST | `/auth/logout` | user | End the session, close its sockets |
| POST | `/auth/password-reset/{token}` | none, rate limited | Set a new password from a reset link, revoke all sessions (ADR 0013) |
| POST | `/auth/forgot-password` | none, rate limited | Send a reset link by email, only with SMTP (V1) |
| GET | `/auth/oidc/{provider}` | none | Start an OIDC login (V1) |
| GET | `/auth/oidc/{provider}/callback` | none | Finish an OIDC login (V1) |
| GET | `/session` | user | Current user, workspaces, CSRF token |
| PATCH | `/me` | user | Update name, email or password. Email and password need the current password, and a new password revokes other sessions |
| DELETE | `/me` | user | Delete the account, refused with `409` for the last enabled owner of a workspace or the last enabled instance admin |
| POST, DELETE | `/me/totp` | user | Enable or disable MFA (V1) |

Bodies and answers:

- `GET /auth/setup` answers `{ required }`, `true` while no account exists.
- `POST /auth/setup` takes `{ email, name, password, workspace_name }`, and answers `201` with the session body and the cookie. Once an account exists it answers `409`.
- `POST /auth/login` takes `{ email, password }`, and answers `200` with the session body and the cookie. An unknown email, a wrong password and a disabled account all answer the same `401`, in the same time.
- `GET /session` answers `{ user: { id, email, name, is_instance_admin }, workspaces: [{ id, slug, name, role }], csrf_token }`.
- `POST /auth/logout` and `DELETE /me` answer `204` and clear the cookie.
- `PATCH /me` takes `{ name?, email?, password?, current_password? }` and answers the `user` object. A wrong or missing `current_password` is a `400` that names that field, an email in use is a `409` that names `email`, and a `401` means the session or the password changed while the request waited.

An email is at most 254 bytes, a name or workspace name 1 to 100 characters, a password 8 to 1024 characters. These routes refuse bodies over 16 KiB.

- `POST /auth/password-reset/{token}` takes `{ password }` and answers `204`, without opening a session. An unknown, used, replaced or expired token is a `404`. A refused password leaves the link usable.

`/auth/setup`, `/auth/login`, `/auth/password-reset/{token}`, `/invitations/{token}/accept` and a `PATCH /me` that checks the current password share one limit per client: a burst of 10, then one every 15 seconds. A hash that waits over 10 seconds for its turn answers `503`.

### Instance administration

| Method | Route | Role | Purpose |
| --- | --- | --- | --- |
| GET | `/admin/users` | instance admin | List users |
| PATCH, DELETE | `/admin/users/{user}` | instance admin | Disable, enable, grant instance admin, or delete |
| POST | `/admin/users/{user}/password-reset` | instance admin | Generate a one-time reset link (ADR 0013) |

- A user here is `{ id, email, name, is_instance_admin, disabled_at, created_at }`, with `disabled_at` `null` for an enabled account. `GET /admin/users` is paginated, oldest first, 50 per page by default.
- `PATCH /admin/users/{user}` takes `{ disabled?, is_instance_admin? }` and answers the user. Disabling ends the user's sessions and voids their reset link, and enabling brings back neither. A change that leaves no enabled instance admin is a `409`.
- `DELETE /admin/users/{user}` answers `204`, or `409` for the last enabled instance admin, or for an account that leaves a workspace with no enabled owner.
- `POST /admin/users/{user}/password-reset` answers `201` with `{ link, expires_at }`, once, as `<CADOCS_PUBLIC_URL>/reset-password/<token>`. A user has one link at a time: a new one replaces the last, and a password change voids it. A disabled user is a `409`. Disabling, demoting or deleting an admin voids the links they made, and links from the command line stay.
- Every answer under `/admin` has `Cache-Control: no-store`.

### Workspaces and members

| Method | Route | Role | Purpose |
| --- | --- | --- | --- |
| GET | `/workspaces` | user | List the caller's workspaces |
| POST | `/workspaces` | instance admin | Create a workspace |
| GET, PATCH | `/workspaces/{ws}` | member, admin to edit | Read or update |
| DELETE | `/workspaces/{ws}` | owner | Delete, with the workspace name as confirmation |
| GET | `/workspaces/{ws}/members` | member | List members |
| PATCH, DELETE | `/workspaces/{ws}/members/{user}` | admin, owner for owners | Change a role or remove a member. A member can remove themself |
| GET, POST | `/workspaces/{ws}/invitations` | admin | List pending invitations, or create one. Creating returns the link once |
| DELETE | `/invitations/{id}` | admin | Revoke an invitation |
| GET | `/invitations/{token}` | none | Workspace name and email, for the acceptance page |
| POST | `/invitations/{token}/accept` | none | Accept: a new user sends a name and password, a logged-in user with the invited email just joins |
| GET, POST | `/workspaces/{ws}/groups` | member, admin to create | Groups (V1) |
| PATCH, DELETE | `/groups/{group}` | admin | Rename or delete a group (V1) |
| PUT, DELETE | `/groups/{group}/members/{user}` | admin | Add or remove a member (V1) |
| GET | `/workspaces/{ws}/recent` | member | Recently updated pages in readable spaces |

- A workspace in a list is `{ id, slug, name, role }`, the caller's role included, as in `GET /session`. `GET /workspaces` is paginated, oldest first. Alone it also has `{ language, created_at, updated_at }`.
- `POST /workspaces` takes `{ name }` and answers `201` with the workspace, owned by the caller. The slug comes from the name, gets a number if it is taken, and never changes.
- `PATCH /workspaces/{ws}` takes `{ name?, language? }` and answers the workspace. `language` is one of `ar`, `da`, `de`, `el`, `en`, `es`, `fi`, `fr`, `hu`, `it`, `nl`, `no`, `pt`, `ro`, `ru`, `sv`, `ta`, `tr` (ADR 0007).
- `DELETE /workspaces/{ws}` takes `{ name }`, which must match exactly, or it is a `400` with `name: mismatch`. It answers `204`.
- A member is `{ id, email, name, role, disabled, created_at }`, where `id` is the user's. `GET /workspaces/{ws}/members` is paginated, by account age.
- `PATCH /workspaces/{ws}/members/{user}` takes `{ role }` and answers the member. A change or a removal that leaves no enabled owner is a `409`, by the rule of [PERMISSIONS.md](PERMISSIONS.md#instance-actions). `DELETE` answers `204`.
- An invitation is `{ id, email, role, invited_by, expires_at, created_at }`, where `role` is `admin` or `member`. A link ends with the right to make it: when its inviter drops below `admin`, leaves, is removed, disabled or deleted. `GET /workspaces/{ws}/invitations` is paginated, oldest first, and leaves out expired ones.
- `POST /workspaces/{ws}/invitations` takes `{ email, role }` and answers `201` with the invitation and `link`, once, as `<CADOCS_PUBLIC_URL>/invite/<token>`, valid 7 days (ADR 0013). An email has one link per workspace: a new one replaces the last. The email of a member is a `409`, with `email: member`.
- `DELETE /invitations/{id}` answers `204`, and `404` to anyone who is not an admin of its workspace.
- `GET /invitations/{token}` answers `{ workspace_id, workspace_name, email, role, expires_at, has_account }`. `has_account` tells the page to ask for a login rather than a password.
- `POST /invitations/{token}/accept` always takes a JSON body. `{ name, password }` makes the account of the invited email, and answers `201` with the session body and the cookie, or `409` with `email: taken` if that email has an account. `{}` joins with the current session, needs the CSRF token, and answers `200` with the session body: `401` with no session, `403` for an account with another email. A member keeps their role. A refused password leaves the link usable.
- A link that is unknown, used, replaced, revoked or expired, or whose workspace is gone, is a `404`. Answers that hold a link or a session have `Cache-Control: no-store`.
| GET | `/jobs/{id}` | the user who started it | Status of an import or export |

### Spaces

| Method | Route | Role | Purpose |
| --- | --- | --- | --- |
| GET, POST | `/workspaces/{ws}/spaces` | member, admin to create | List readable spaces, or create one |
| GET | `/workspaces/{ws}/spaces/trash` | member | List the trashed spaces the caller can restore |
| GET | `/spaces/{space}` | viewer | Read |
| PATCH, DELETE | `/spaces/{space}` | space admin | Update or trash |
| POST | `/spaces/{space}/restore` | space admin | Restore from trash |
| GET | `/spaces/{space}/trash` | editor | List trashed pages |
| GET | `/spaces/{space}/export?format=md\|html` | viewer | Export the space as a zip, as a job (V1) |
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
| POST | `/pages/{page}/duplicate` | editor | Copy a page, optionally to another space (V1) |
| DELETE | `/pages/{page}` | editor | Move to trash |
| POST | `/pages/{page}/restore` | editor | Restore from trash |
| DELETE | `/pages/{page}/purge` | space admin | Delete a trashed page for good |
| GET | `/pages/{page}/export?format=md\|html&attachments=0\|1` | viewer | Export. Without attachments, one file with absolute links. With them, a zip |
| POST | `/spaces/{space}/import` | editor | Import Markdown or a zip, as a job |
| GET | `/pages/{page}/revisions` | viewer | List revisions |
| GET | `/pages/{page}/revisions/{n}` | viewer | Rendered content of a revision |
| POST | `/pages/{page}/revisions/{n}/restore` | editor | Apply a revision as a new edit |

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
| GET | `/me/mentions` | user | Unread mentions |
| POST | `/me/mentions/{id}/read` | user | Mark a mention as read |

### Attachments

| Method | Route | Role | Purpose |
| --- | --- | --- | --- |
| POST | `/pages/{page}/attachments?name=` | editor | Streaming upload |
| GET | `/attachments/{id}` | viewer of a referencing page | Download |
| DELETE | `/attachments/{id}` | editor of a referencing page | Delete, for every page that references it |

- An upload is the file itself as the body, not multipart: it streams to disk with no parser in between, and a browser sends a `File` as is. `name` is the file name, percent-encoded, `+` for a space, 1 to 255 characters with no control character, else a `400` naming `name`. `Content-Type` is the file's type: its parameters are dropped, and a missing or malformed one is stored as `application/octet-stream`. It answers `201` with `{ id, workspace_id, file_name, content_type, size, uploaded_by, created_at }`.
- A file over `CADOCS_MAX_UPLOAD_MB`, or over what the workspace quota leaves, is a `413`: at once when `Content-Length` announces it, else as soon as the stream passes the limit. A full quota refuses even an empty file. A storage failure, a full disk among them, is a `500`, and nothing is recorded.
- A download sends the stored type, `Content-Length`, an `ETag` of the SHA-256 and `Cache-Control: private, no-cache`, so a reader revalidates and gets `304` while the file is unchanged. `Content-Disposition` is `inline` for PNG, JPEG, GIF, WebP and AVIF, `attachment` for anything else, SVG included, with the name in `filename*`. Its `Content-Security-Policy` is `default-src 'none'; sandbox`, so a file opened by itself runs nothing.
- `DELETE` answers `204`. A reader who is not an editor of any referencing page gets `403`, and someone who reads none of them `404`.

### Search

| Method | Route | Role | Purpose |
| --- | --- | --- | --- |
| GET | `/workspaces/{ws}/search?q=&space=` | member | Results limited to readable spaces |

### Sharing

| Method | Route | Role | Purpose |
| --- | --- | --- | --- |
| GET, POST | `/pages/{page}/shares` | editor | List or create public links |
| DELETE | `/shares/{id}` | editor | Revoke |
| GET | `/s/{token}` (outside `/api`) | none | Public page, rendered as HTML on the server |
| GET | `/s/{token}/p/{slug}` (outside `/api`) | none | A descendant, if the share includes children |
| GET | `/s/{token}/files/{id}` (outside `/api`) | none | An attachment referenced by a page the share covers |

Public routes are rate limited, send an `ETag` based on `updated_at`, and render through the bounded reader described in [ARCHITECTURE.md](ARCHITECTURE.md#reading-a-document-outside-a-room).

### Operations

| Method | Route | Purpose |
| --- | --- | --- |
| GET | `/health` (outside `/api`) | Liveness, with a database check. `503` while flushes are failing |

## Page WebSocket protocol

### Connection

```
GET /ws/pages/{page_id}
Upgrade: websocket
Cookie: session=...
Origin: https://wiki.example.org
```

The URL matches what `y-websocket` builds: the server URL followed by `/` and the room name. Checks happen before the upgrade:

1. `Origin` must match the configured public URL. Cookies are sent on cross-site WebSocket requests, so this check is what prevents cross-site WebSocket hijacking.
2. The session must be valid, otherwise `401`.
3. The page must exist and be readable, otherwise `404`.
4. The resulting role sets the connection mode: read-write for `editor` and above, read-only otherwise. A `commenter` is read-only on the wire: comments go through REST.

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

By default, `y-websocket` reconnects after any close code except 4400 to 4499, where it stops for good. That range includes `4408` and `4409`, so the front end passes its own `shouldReconnect`, which stops only on `4401`, `4403` and `4404`. On `4409` it updates the editor's mode and lets the provider reconnect.

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
| `notification` | `id` | A new notification (V1) |
| `resync` | none | Events were dropped because the client lagged, reload everything shown |
