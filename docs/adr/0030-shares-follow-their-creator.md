# ADR 0030: shares follow their creator

Status: accepted. Refines the public sharing of ADR 0009.

## Context

ADR 0009 makes a share a capability: the token opens the page, whoever holds it, until it expires or an editor revokes it. A share therefore outlived the right that made it. A member removed from the workspace, or an editor demoted to viewer, kept every link they had made, and nobody else learned about it unless they opened the share dialog of each page. Removing a person from a wiki should close what they opened.

## Decision

A share answers only while its creator may still manage shares of its page (`ManageShares`, so `editor` or above), as resolved at each public request. A creator who left the workspace, lost the role, has a disabled account or no account at all (`created_by` is `NULL` once the account is deleted) closes the share, which answers the same `404` as an unknown token.

The check is made when a share opens (`open_share`), not when a role changes. Nothing is deleted: a creator given the role back reopens their shares, and an editor can still list and revoke a closed one.

The share dialog lists only the shares not expired: an expired share can never answer again, so it is noise there. A closed share stays listed, since it can reopen and an editor may want to revoke it.

## Consequences

- One more page lookup with role resolution per public request, on the reader pool. No new table, no write.
- No code on the paths that change roles (members, grants, groups, default roles, account state) has to remember shares, and none can forget them.
- A share's lifetime now depends on a person. A link meant to outlive its creator must be made again by someone who stays.

## Rejected alternatives

- **Delete the creator's shares when a role changes.** Every path that can change a role would have to find the shares that change affects, through groups and default roles too, and a mistake leaves a share open.
- **Shares owned by the space, not a person.** A share would still outlive the editor who made it, which is the problem.
- **List closed shares apart in the dialog.** It needs the creator's role for every listed share; the dialog stays as it is until someone asks.
