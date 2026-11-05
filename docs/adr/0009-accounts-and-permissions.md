# ADR 0009: Global accounts, per-space roles, page restrictions in V2

Status: accepted

## Context

The Brut design system includes a workspace selector, so one person may belong to several workspaces. Permissions must cover workspaces, spaces and eventually pages, and they affect REST, WebSockets, search and public links.

## Decision

**Accounts.** A user is global to the instance. `workspace_members` links users to workspaces. Every workspace-owned table carries `workspace_id`.

**Roles.**

| Level | Roles |
| --- | --- |
| Workspace | `owner`, `admin`, `member` |
| Space | `admin`, `editor`, `commenter`, `viewer` |

**Resolution**, implemented as pure functions in `cadocs-core`:

1. A non-member of the workspace has no access at all.
2. A workspace `owner` or `admin` is implicitly `admin` of every space.
3. Otherwise, the effective space role is the highest of the direct grant and the grants through groups (V1).
4. With no grant, a space's `default_role` applies if set. Otherwise there is no access.
5. The action is allowed if the role includes it.

Default is deny, and an unknown case resolves to no access. Requests for a resource the user cannot read return 404, so existence does not leak.

**Pages (V2).** `page_restrictions` is in the schema from the start. A restriction can only *narrow* access: a user needs access under the space and under every restricted ancestor. Restrictions never grant more than the space role. Search and tree endpoints must apply the same rule, which is the main cost and the reason it ships later.

**Public sharing.** A share is a read-only capability with an unguessable token. It bypasses roles, for the shared page and optionally its descendants, and exposes only rendered content, never the Yjs state or the WebSocket.

**WebSocket.** The role is computed at connection. When permissions change, the server closes affected sockets, so a revoked user cannot keep receiving updates.

## Consequences

- `workspace_id` is present in almost every query. Store methods take it as a required parameter, so it cannot be forgotten.
- Permission logic can be tested exhaustively, without a database.

## Rejected alternatives

- **One workspace per instance.** Simpler, but contradicts the workspace selector, and moving away later is a heavy migration.
- **Per-workspace accounts.** Multiple logins for the same person.
- **Page restrictions in the MVP.** Complicates every read path before the product exists.
- **Page ACLs that can widen access.** Harder to reason about: a space's role would no longer bound what a member can see.
- **A generic policy engine (Casbin, Cedar).** A dependency and a language for a rule set that fits in one small module.
