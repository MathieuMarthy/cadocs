# API

All routes are under `/api/v1`, and bodies are JSON unless stated otherwise. Today the WebSocket route marked "prototype", and these REST routes, exist: `/auth/setup`, `/auth/login`, `/auth/logout`, `/session` and `/me`. Any other path under `/api/v1` answers `404` in the error shape. The rest is the target design.

## Conventions

- **Authentication**: an `HttpOnly`, `Secure`, `SameSite=Lax` session cookie.
- **CSRF**: mutating requests with a session require the `X-CSRF-Token` header, whose value comes from `GET /session`. It is derived from the session token, so it needs no storage and lasts as long as the session. Routes that run before a session exists (`/auth/setup`, `/auth/login`) have no token to check: they rely on the `Origin` check and on requiring a JSON content type, which a cross-site form cannot send.
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

- `POST /auth/setup` takes `{ email, name, password, workspace_name }`, and answers `201` with the session body and the cookie. Once an account exists it answers `409`.
- `POST /auth/login` takes `{ email, password }`, and answers `200` with the session body and the cookie. An unknown email, a wrong password and a disabled account all answer the same `401`, in the same time.
- `GET /session` answers `{ user: { id, email, name, is_instance_admin }, workspaces: [{ id, slug, name, role }], csrf_token }`.
- `POST /auth/logout` and `DELETE /me` answer `204` and clear the cookie.
- `PATCH /me` takes `{ name?, email?, password?, current_password? }` and answers the `user` object. A wrong or missing `current_password` is a `400` that names that field, an email in use is a `409` that names `email`, and a `401` means the session or the password changed while the request waited.

An email is at most 254 bytes, a name or workspace name 1 to 100 characters, a password 8 to 1024 characters. These routes refuse bodies over 16 KiB.

`/auth/setup`, `/auth/login` and a `PATCH /me` that checks the current password share one limit per client: a burst of 10, then one every 15 seconds. A hash that waits over 10 seconds for its turn answers `503`.

### Instance administration

| Method | Route | Role | Purpose |
| --- | --- | --- | --- |
| GET | `/admin/users` | instance admin | List users |
| PATCH, DELETE | `/admin/users/{user}` | instance admin | Disable, enable, grant instance admin, or delete |
| POST | `/admin/users/{user}/password-reset` | instance admin | Generate a one-time reset link (ADR 0013) |

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
| GET | `/jobs/{id}` | the user who started it | Status of an import or export |

### Spaces

| Method | Route | Role | Purpose |
| --- | --- | --- | --- |
| GET, POST | `/workspaces/{ws}/spaces` | member, admin to create | List readable spaces, or create one |
| GET | `/spaces/{space}` | viewer | Read |
| PATCH, DELETE | `/spaces/{space}` | space admin | Update or trash |
| POST | `/spaces/{space}/restore` | space admin | Restore from trash |
| GET | `/spaces/{space}/trash` | editor | List trashed pages |
| GET | `/spaces/{space}/export?format=md\|html` | viewer | Export the space as a zip, as a job (V1) |
| GET, PUT | `/spaces/{space}/grants` | viewer, space admin to edit | Roles of users and groups |
| GET | `/spaces/{space}/tree?parent={id}` | viewer | One level of the tree, loaded lazily |

### Pages

| Method | Route | Role | Purpose |
| --- | --- | --- | --- |
| POST | `/spaces/{space}/pages` | editor | Create a page |
| GET | `/pages/{page}` | viewer | Metadata, including the caller's effective `role` |
| GET | `/workspaces/{ws}/pages/by-slug/{slug}` | viewer | Same, from a front-end URL |
| PATCH | `/pages/{page}` | editor | Title |
| POST | `/pages/{page}/move` | editor | New `parent_id` and `position`, possibly in another space, where `editor` is also required |
| POST | `/pages/{page}/duplicate` | editor | Copy a page, optionally to another space (V1) |
| DELETE | `/pages/{page}` | editor | Move to trash |
| POST | `/pages/{page}/restore` | editor | Restore from trash |
| DELETE | `/pages/{page}/purge` | space admin | Delete a trashed page for good |
| GET | `/pages/{page}/export?format=md\|html&attachments=0\|1` | viewer | Export. Without attachments, one file with absolute links. With them, a zip |
| POST | `/spaces/{space}/import` | editor | Import Markdown or a zip, as a job |
| GET | `/pages/{page}/revisions` | viewer | List revisions |
| GET | `/pages/{page}/revisions/{n}` | viewer | Rendered content of a revision |
| POST | `/pages/{page}/revisions/{n}/restore` | editor | Apply a revision as a new edit |

The Yjs document holds the title, and the server copies it to `pages.title` when flushing. `PATCH` does not write that column: it applies a Yjs update through the room, like any other edit, so a later flush cannot overwrite it.

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
| POST | `/pages/{page}/attachments` | editor | Streaming multipart upload |
| GET | `/attachments/{id}` | viewer of a referencing page | Download |
| DELETE | `/attachments/{id}` | editor of a referencing page | Delete |

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
GET /ws/pages/{page_id}      target
GET /ws/{doc_id}             prototype
Upgrade: websocket
Cookie: session=...
Origin: https://wiki.example.org
```

The URL matches what `y-websocket` builds: the server URL followed by `/` and the room name. Checks happen before the upgrade:

1. `Origin` must match the configured public URL. Cookies are sent on cross-site WebSocket requests, so this check is what prevents cross-site WebSocket hijacking.
2. The session must be valid, otherwise `401`.
3. The page must exist and be readable, otherwise `404`.
4. The resulting role sets the connection mode: read-write for `editor` and above, read-only otherwise. A `commenter` is read-only on the wire: comments go through REST.

The prototype has no session. A browser's `Origin` must be the one of `CADOCS_PUBLIC_URL`, otherwise the answer is `403`. Clients that send no `Origin` are accepted. A prototype `doc_id` is 1 to 64 letters, digits, `-` or `_`, otherwise `404`. At capacity or during shutdown, the answer is `503`.

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
- **Awareness ownership.** The server tracks the client ids each socket announced, and removes them on close with a null state at the same clock, as `y-websocket`'s server does. A client that reconnects with an unchanged clock therefore shows again at its next change, at most one heartbeat later.
- **Awareness limits.** A socket may introduce 16 client ids over its life, and a state must be valid JSON of at most 64 KiB. Otherwise the socket closes with `1002`. Removing an unknown id is ignored. Ownership of an id is not checked yet, so a socket can overwrite another cursor.
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

The checks before the upgrade are the same as for a page socket, with workspace membership in place of page access. Frames are server to client, as JSON text (ADR 0011). A client frame closes the socket. Close codes `1001`, `4401`, `4403` and `4408` apply.

Events are hints. The client refetches through REST, and refetches the tree after every reconnect.

```json
{ "type": "page.moved", "space": "<id>", "page": "<id>" }
```

| Type | Fields | Meaning |
| --- | --- | --- |
| `page.created`, `page.moved`, `page.renamed`, `page.trashed`, `page.restored` | `space`, `page` | The tree changed. A move across spaces is sent to both |
| `space.created`, `space.updated`, `space.trashed`, `space.restored` | `space` | The space list changed |
| `access.changed` | none | The user's permissions changed, reload the space list |
| `notification` | `id` | A new notification (V1) |
| `resync` | none | Events were dropped because the client lagged, reload everything shown |
