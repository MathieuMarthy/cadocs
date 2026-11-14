# ADR 0024: Any user creates a workspace, unless the operator keeps it to admins

Status: accepted

## Context

Until step 26, only an instance admin could create a workspace, from the admin screen. Daily use showed what that costs: a member who wants a workspace for a team, or for their own notes, has to ask an instance admin, whose role is otherwise about accounts. A user who belongs to no workspace sees a dead end.

Accounts only come from first-run setup, an invitation or an instance admin, so every user is someone the instance already let in. A new workspace reaches no one else's content: its creator owns it, alone, until they invite people.

What it does cost is storage. `CADOCS_WORKSPACE_QUOTA_MB` bounds each workspace, so a user who creates many workspaces multiplies what they may store.

## Decision

`POST /workspaces` needs a user, enabled, who becomes its owner. `CADOCS_WORKSPACE_CREATION=admins` keeps it to instance admins, as before. The default is `all`, since a small team running cadocs on its own wants the first behavior.

The rule lives in `cadocs-core` as `InstanceAction::CreateWorkspace(WorkspaceCreation)`, and the store checks it in the transaction that writes the workspace, as it did, with the setting the server passes in `NewWorkspace`.

`GET /session` says whether the caller may create one, in `can_create_workspace`, so the front end shows the form only to those who may use it. The form moves from the admin screen to the workspace switcher, and to the screen of a user with no workspace.

## Consequences

- An instance whose users must not create workspaces, or where the storage of each workspace matters, sets `CADOCS_WORKSPACE_CREATION=admins`.
- There is no limit on how many workspaces a user creates. If storage abuse shows up, a per-user count or quota is the next step, not this setting.
- Instance admins keep no implicit access to content: one who creates a workspace owns it as any user would.
