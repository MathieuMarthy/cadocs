# Permissions

The rules of [ADR 0009](adr/0009-accounts-and-permissions.md) as a matrix. `cadocs-core` implements this document, and its tests cover every cell. Anything not listed is denied.

## Resolution

Evaluated in this order. The first rule that applies decides.

1. A disabled user, or a session that is not valid, has no access.
2. A non-member of the workspace has no access.
3. A workspace `owner` or `admin` is `admin` of every space.
4. A direct or group grant gives the highest of those grants. An explicit grant **replaces** `default_role`, even when it is lower, so a space admin can lower one member's access.
5. With no grant, the space's `default_role` applies if it is set.
6. Otherwise there is no access.

Instance admins have no implicit access to content. Their role covers accounts and workspace creation only.

## Workspace actions

| Action | `member` | `admin` | `owner` |
| --- | --- | --- | --- |
| Read the workspace, list members, search | yes | yes | yes |
| Leave the workspace | yes | yes | yes, unless last owner |
| Update the workspace | no | yes | yes |
| Create a space | no | yes | yes |
| Create or revoke an invitation | no | yes | yes |
| Change or remove a `member` or `admin` | no | yes | yes |
| Manage groups (V1) | no | yes | yes |
| Choose which installed content plugins the workspace offers (V2) | no | yes | yes |
| Enable or disable an offered content plugin for oneself (V2) | yes | yes | yes |
| Change or remove an `owner`, or grant `owner` | no | no | yes |
| Delete the workspace | no | no | yes |

A workspace always keeps at least one owner. Any change that would remove the last one returns `409 conflict`.

## Space actions

| Action | `viewer` | `commenter` | `editor` | `admin` |
| --- | --- | --- | --- | --- |
| Read the space, tree and grants | yes | yes | yes | yes |
| Read a page, export it, read its revisions, download attachments | yes | yes | yes | yes |
| Read comments | yes | yes | yes | yes |
| Open the page socket, read-only | yes | yes | yes | yes |
| Create a comment or reply, resolve or reopen | no | yes | yes | yes |
| Edit or delete one's own comment | no | yes | yes | yes |
| Edit or delete another user's comment | no | no | no | yes |
| Open the page socket, read-write | no | no | yes | yes |
| Create, update, move, trash or restore a page | no | no | yes | yes |
| List the trash | no | no | yes | yes |
| Restore a revision, import | no | no | yes | yes |
| Upload or delete an attachment | no | no | yes | yes |
| List, create or revoke public shares | no | no | yes | yes |
| Purge a page from the trash | no | no | no | yes |
| Update the space, edit grants | no | no | no | yes |
| Trash or restore the space | no | no | no | yes |

## Special cases

- **Move across spaces:** needs `editor` on both spaces. A move across workspaces is refused.
- **Trashed space:** every action is denied except restoring it, for a space `admin`, who also finds it in the list of trashed spaces of the workspace.
- **Trashed page:** it behaves as absent for all actions except listing the trash, restoring and purging.
- **Attachments:** a download needs read access to at least one page that references the file ([DATA_MODEL.md](DATA_MODEL.md#attachments)).
- **Public shares** bypass these rules for rendered content only (ADR 0009).

## Instance actions

| Action | User | Instance admin |
| --- | --- | --- |
| Read or update one's own account, delete it | yes | yes |
| Create a workspace | no | yes |
| List, disable or delete users, grant instance admin | no | yes |
| Generate a password reset link | no | yes |
| Enable or disable an installed user plugin for oneself (V2) | yes | yes |
| List installed plugins (V2) | no | yes |

Installing plugin code is not an instance action: only the operator does it, on the server (ADR 0016).

An instance always keeps at least one enabled instance admin. A user who is the last owner of a workspace cannot be deleted until ownership moves or the workspace is deleted. Only enabled owners count, as a disabled one cannot act. Disabling the last enabled owner is allowed: it can be undone, and an instance admin must be able to lock out an account that has no other owner beside it.

## Reacting to changes

A change that may lower access runs through the same check for open sockets, both page and events:

| Trigger | Effect on open sockets |
| --- | --- |
| Grant, `default_role`, workspace role or group membership changed | Recompute. See the two outcomes below |
| Page moved to another space | Recompute for sockets on the page and its descendants |
| Member removed, user disabled or deleted | Close with `4403`, or `4401` if the account is gone |
| Logout, session expired, password changed | Close that user's affected sockets with `4401` |
| Page or space trashed, page purged, workspace deleted | Close with `4404` |

Outcome of a recompute on an events socket: the set of spaces and roles it keeps is replaced, and if that changes it gets `access.changed`. It closes with `4403` once its user is no longer an enabled member of the workspace, and with `4404` once the workspace is gone. Trashing a page or a space does not close it, and a trashed space stays in the set, so that its readers hear `space.restored`.

Outcome of a recompute on a page socket: if the page is no longer readable, close with `4403`. If it stays readable but the mode changes, close with `4409`, and the client reconnects in the new mode. A raise closes too, so a viewer who becomes an editor can write without reloading.

Every trigger runs one pass over all open sockets, which looks at each socket's session, then at its role, from the database. So a trigger needs no list of who it affects, and a pass closes in the order of that check: a session that ended or an account that is gone gives `4401`, a disabled account `4403`, a page row that is gone `4404`. A pass also runs every `CADOCS_SOCKET_RECHECK_SECS`, for what no request announces: an expired session, a command run beside the server, and a pass that failed on a storage error, whose sockets stay open until then. A socket that joins runs the same check once, after joining, so a change landing between its first check and its join cannot miss it.
