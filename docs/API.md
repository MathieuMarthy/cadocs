# API

All routes are under `/api/v1`, and bodies are JSON unless stated otherwise. Only the WebSocket route marked "prototype" exists today. The rest is the target design.

## Conventions

- **Authentication**: an `HttpOnly`, `Secure`, `SameSite=Lax` session cookie.
- **CSRF**: mutating requests require the `X-CSRF-Token` header, whose value comes from `GET /session`.
- **Ids**: UUIDv7 strings.
- **Pagination**: `?limit=` (maximum 100) and an opaque `?cursor=`. Responses have the form `{ "items": [...], "next_cursor": "..." | null }`.
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
| 409 | `conflict` |
| 413 | `too_large` |
| 429 | `rate_limited` |
| 500 | `internal` (details go to the log only) |

## REST routes

Minimum role per route: `member` is a workspace role. Space roles are `viewer` < `commenter` < `editor` < `admin` (ADR 0009).

### Auth and session

| Method | Route | Role | Purpose |
| --- | --- | --- | --- |
| POST | `/auth/setup` | none, first run only | Create the first user and workspace |
| POST | `/auth/login` | none, rate limited | Email and password login |
| POST | `/auth/logout` | user | End the session |
| GET | `/auth/oidc/{provider}` | none | Start an OIDC login (V1) |
| GET | `/auth/oidc/{provider}/callback` | none | Finish an OIDC login (V1) |
| GET | `/session` | user | Current user, workspaces, CSRF token |
| PATCH | `/me` | user | Update name or password |

### Workspaces and members

| Method | Route | Role | Purpose |
| --- | --- | --- | --- |
| GET | `/workspaces` | user | List the caller's workspaces |
| POST | `/workspaces` | user | Create a workspace |
| GET, PATCH | `/workspaces/{ws}` | member, admin to edit | Read or update |
| GET | `/workspaces/{ws}/members` | member | List members |
| PATCH, DELETE | `/workspaces/{ws}/members/{user}` | admin | Change a role or remove a member |
| POST | `/workspaces/{ws}/invitations` | admin | Invite by email |
| POST | `/invitations/{token}/accept` | none | Accept an invitation |
| GET, POST | `/workspaces/{ws}/groups` | member, admin to create | Groups (V1) |

### Spaces

| Method | Route | Role | Purpose |
| --- | --- | --- | --- |
| GET, POST | `/workspaces/{ws}/spaces` | member, admin to create | List readable spaces, or create one |
| GET | `/spaces/{space}` | viewer | Read |
| PATCH, DELETE | `/spaces/{space}` | space admin | Update or trash |
| GET, PUT | `/spaces/{space}/grants` | viewer, space admin to edit | Roles of users and groups |
| GET | `/spaces/{space}/tree?parent={id}` | viewer | One level of the tree, loaded lazily |

### Pages

| Method | Route | Role | Purpose |
| --- | --- | --- | --- |
| POST | `/spaces/{space}/pages` | editor | Create a page |
| GET | `/pages/{page}` | viewer | Metadata |
| PATCH | `/pages/{page}` | editor | Title and icon |
| POST | `/pages/{page}/move` | editor | New `parent_id` and `position`, possibly in another space |
| DELETE | `/pages/{page}` | editor | Move to trash |
| POST | `/pages/{page}/restore` | editor | Restore from trash |
| GET | `/pages/{page}/export?format=md\|html` | viewer | Export |
| POST | `/spaces/{space}/import` | editor | Import Markdown or a zip, as a job |
| GET | `/pages/{page}/revisions` | viewer | List revisions |
| GET | `/pages/{page}/revisions/{n}` | viewer | Rendered content of a revision |
| POST | `/pages/{page}/revisions/{n}/restore` | editor | Apply a revision as a new edit |

The title is also edited live in the document. The server copies it to `pages.title` when flushing. `PATCH` covers API clients.

### Comments

| Method | Route | Role | Purpose |
| --- | --- | --- | --- |
| GET | `/pages/{page}/comments` | viewer | List threads |
| POST | `/pages/{page}/comments` | commenter | Create a comment or a reply |
| PATCH, DELETE | `/comments/{id}` | author, or space admin | Edit or delete |
| POST | `/comments/{id}/resolve` | commenter | Resolve or reopen |
| GET | `/me/mentions` | user | Unread mentions |

### Attachments

| Method | Route | Role | Purpose |
| --- | --- | --- | --- |
| POST | `/pages/{page}/attachments` | editor | Streaming multipart upload |
| GET | `/attachments/{id}` | viewer of the page | Download |
| DELETE | `/attachments/{id}` | editor | Delete |

### Search

| Method | Route | Role | Purpose |
| --- | --- | --- | --- |
| GET | `/workspaces/{ws}/search?q=&space=` | member | Results limited to readable spaces |

### Sharing

| Method | Route | Role | Purpose |
| --- | --- | --- | --- |
| GET, POST | `/pages/{page}/shares` | viewer, editor to create | List or create public links |
| DELETE | `/shares/{id}` | editor | Revoke |
| GET | `/s/{token}` (outside `/api`) | none | Public page, rendered as HTML on the server |

### Operations

| Method | Route | Purpose |
| --- | --- | --- |
| GET | `/health` (outside `/api`) | Liveness, with a database check |

## WebSocket protocol

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
```

### Rules

- **Read-only.** `SyncStep2` and `Update` frames from a read-only client are dropped, and the server sends one Auth frame with the reason `read-only`. Awareness is still accepted, so a reader's presence shows.
- **Awareness ownership.** The server tracks the client ids each socket announced, and removes them on close.
- **Limits.** A frame over the maximum size closes the socket. Each client has a bounded send queue. A client that stops reading is closed rather than buffered.
- **Keep-alive.** The server pings periodically, and closes a socket that stays silent past the timeout.
- **Malformed frames** close the socket, without affecting the room.

### Close codes

| Code | Meaning | Client behaviour |
| --- | --- | --- |
| 1000 | Normal | None |
| 1001 | Server shutting down | Reconnect with backoff |
| 1009 | Frame too large | Report an error |
| 4401 | Session ended | Stop, go to login |
| 4403 | Access revoked | Stop, leave the page |
| 4404 | Page deleted | Stop, leave the page |
| 4408 | Client too slow, or idle timeout | Reconnect with backoff |

`y-websocket` reconnects automatically, whatever the close code. The front end must listen for `4401`, `4403` and `4404`, and call `provider.disconnect()`.
