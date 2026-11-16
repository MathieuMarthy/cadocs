# Coverage

Where the MVP scope of [ROADMAP.md](ROADMAP.md#mvp) and the matrix of [PERMISSIONS.md](PERMISSIONS.md) are tested. Checked item by item in step 22. A new feature adds its line here with the tests that cover it.

Paths are short: `server/auth.rs` is `crates/cadocs-server/tests/auth.rs`, `e2e/shell` is `web/e2e/shell.spec.ts`.

## Permission matrix

| Part | In the rules | Over REST and sockets |
| --- | --- | --- |
| Resolution order | `cadocs-core/tests/permissions.rs`, one test per rule | `server/spaces.rs` (grants, default role, workspace admins), `server/matrix.rs` |
| Space actions, each role | `space_matrix` | `server/matrix.rs` `each_space_action_follows_the_matrix_over_rest`: every MVP row, five roles, success or the `403` and `404` of API.md |
| Page socket mode | `socket_mode_follows_the_role` | `server/matrix.rs` `the_page_socket_mode_follows_the_matrix`, `server/page_socket.rs` |
| Trashed space and page | `trashed_space_denies_everything_but_its_restoration`, `trashed_page_is_absent_…` | `server/matrix.rs` `a_trashed_space_denies_everything_but_its_restoration`, `server/pages.rs` |
| Move across spaces | `move_needs_editor_on_both_spaces_of_one_workspace` | `server/pages.rs`, `server/events.rs` |
| Attachment download | `download_needs_one_readable_referencing_page` | `server/attachments.rs` `a_download_needs_one_readable_page_that_references_the_file` |
| Attachment delete | `delete_needs_an_editor_of_every_referencing_page` | `server/attachments.rs` `a_delete_needs_an_editor_of_every_page_that_references_the_file` |
| Workspace actions | `workspace_matrix`, `owner_rows_are_needed_to_touch_an_owner`, `a_workspace_keeps_one_owner` | `server/workspaces.rs` (update, delete, members, invitations, last owner), `server/matrix.rs` (reads, search, events socket, space creation) |
| Instance actions | `instance_matrix`, `an_instance_keeps_one_enabled_admin` | `server/admin.rs`, `server/matrix.rs` `workspace_creation_follows_the_setting_over_rest`, `server/workspaces.rs` `the_creation_rule_is_checked_when_a_workspace_is_written`, `e2e/shell` (from the switcher, from the screen of a user with no workspace, and with `admins`), `server/auth.rs` (own account) |
| Reacting to changes, page sockets | `recompute_keeps_or_closes_the_socket` and the two after it | `server/socket_closes.rs`, one test per trigger and outcome |
| Reacting to changes, events sockets | | `server/events.rs` (`access.changed`, `4401`, `4403`, `4404`, trash leaves it open) |

Comments, public shares, groups and plugins have no route before V1 or V2: their rows are tested in the rules only, and their REST rows come with their routes.

## MVP scope

| Item | Tests |
| --- | --- |
| Accounts, sessions, invitation and reset links | `server/auth.rs` (with the setup code of ADR 0021), `server/admin.rs` (reset links), `server/workspaces.rs` (invitations), `server/cli.rs` (`reset-password`), `session.rs` unit tests (the `__Host-` cookie), `e2e/shell` (setup with and without the link, login, invitation, reset) |
| Forgot password by mail (Mail stage, step 29) | `mail/outbox.rs` (no wait, bounded, stale messages, reserved places), `server/mail.rs` (the link arrives and works, same answer for an unknown or disabled account, both limits, a full outbox keeps earlier links, `404` with mail off), `rate_limit.rs` unit tests, `e2e/mail` (from the login link to a login with the new password, and neither link nor screen without a relay) |
| Registration by mail (Mail stage, step 30) | `server/register.rs` (a mailed code makes a verified account with no workspace, only a keyed hash stored, 5 wrong codes kill a code and 4 do not, one address per code, expiry, replacement, same answer for an address with an account and a message to its owner, nothing to a disabled one, an account made meanwhile is a `409`, refused fields count no try, both mail limits and the guess limit, `404` closed or with mail off, `GET /auth/setup`), `session.rs` and `config.rs` unit tests, `e2e/mail` (from the login link to the home with no workspace through a wrong code then the right one, a new code from the code form, no link nor screen closed or without a relay) |
| Second factor by mail (Mail stage, step 31) | `server/second_factor.rs` (a login with the factor needs the mailed code and no session exists before it, the code proves the address and is used once, a wrong password never tries a code nor mails one, 5 wrong codes kill it, a disabled account gets nothing, a code is ignored without the factor, both mail limits, the factor ignored and its routes `404` with mail off, turning it on needs a code to the address, a login code and an `email` code are not interchangeable, turning it off needs the current password, a session is needed), `cli.rs` (the notice at start with mail off), `e2e/mail` (on from the account screen with a mailed code, a login that asks for the code then refuses a wrong one, off with the password, no section without a relay) |
| Proof of a new address (Mail stage, step 32) | `server/email_change.rs` (the address changes only after the code mailed to it, other fields change at once, the current password is needed, the same address sends nothing, a taken address gets a notice and no code, an address taken meanwhile is a `409` that keeps the code, a code works for its address and account only, 5 wrong codes kill it, an empty code and a bad address, a factor code and a change code are not interchangeable, a proven address keeps the factor, both mail limits, a moved address voids the reset link; without a relay the address changes at once, unproven, with the factor off and the reset link void, and the confirmation is a `404`), `server/auth.rs` (the MVP behavior without a relay), `e2e/mail` (a new email from the account screen, changed once the mailed code is typed, with the code form kept through another save) |
| Instance administration | `server/admin.rs`, `e2e/shell` `instance administration lists, disables and promotes users`, `e2e/screens` |
| Workspaces, spaces, tree, trash, recent pages | `server/workspaces.rs`, `server/spaces.rs`, `server/pages.rs`, `server/jobs.rs` (trash purge), `e2e/navigation`, `e2e/settings` |
| Per-space roles | Permission matrix above, `e2e/settings` (grants and roles while a page is open), `e2e/editor` (read-only viewer) |
| Live updates of the tree and titles | `server/events.rs`, `e2e/navigation` `two browsers see each other…`, `e2e/browsers` (title) |
| Collaborative editor and its blocks | `e2e/editor` (slash menu, headings, lists, tasks, code, quotes, dividers, links, shortcuts), `e2e/blocks` (callouts, toggles, tables, links to pages), `e2e/attachments` (images, files), `e2e/browsers` (cursors, restart), `content/*.rs`, `collab/*.rs` |
| Table of contents | `e2e/blocks` `the table of contents lists the headings…` |
| Markdown shortcuts, import and export | `e2e/editor` `Markdown shortcuts make the blocks and marks`, `server/markdown.rs`, `content/markdown.rs`, `e2e/transfer` |
| Print stylesheet | `e2e/transfer` `printing shows the page alone` |
| Attachments on disk or S3, limits | `server/attachments.rs`, `files/local.rs`, `files/s3.rs` (fake S3, and MinIO in CI), `e2e/attachments` |
| Full-text search, stemming | `server/search.rs`, `search/analyzer.rs`, `store/search.rs`, `e2e/search` |
| Revisions: list, view, restore, retention | `server/revisions.rs`, `store/revisions.rs` (thinning), `e2e/revisions` |
| Brut interface, themes, phone | `e2e/screens` (each screen in both themes), `e2e/shell` (theme, phone width), `e2e/navigation` (drawer) |
| Source code link | `e2e/shell` `every screen links to the source code` |
| Docker image, binaries | `scripts/check-image.sh`: serve, setup, backup, restore in the image, then a folder of root and a folder of the caller, each served without root (ADR 0025), with `server/privileges.rs`. CI runs it on `amd64` after each push to main, and `release.yml` on each architecture before pushing |
| Backup and restore | `server/backup.rs`, `scripts/check-image.sh` |
| `OPERATIONS.md` complete | A document, checked by reading in step 21 |

Not automated: the RAM targets (step 23, `bench/rss.sh`), except that a hash's memory goes back with no trim (`server/hash_memory.rs`, ADR 0023), and the release workflow as a whole, which runs on a tag.
