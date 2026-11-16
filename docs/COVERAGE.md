# Coverage

Where the MVP and V1 scopes of [ROADMAP.md](ROADMAP.md#mvp) and the matrix of [PERMISSIONS.md](PERMISSIONS.md) are tested. A new feature adds its line here with the tests that cover it.

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
| Workspace actions | `workspace_matrix`, `owner_rows_are_needed_to_touch_an_owner`, `a_workspace_keeps_one_owner` | `server/workspaces.rs` (update, delete, members, invitations, last owner), `server/matrix.rs` (reads, search, events socket, space creation, `group_actions_follow_the_matrix_over_rest`) |
| Instance actions | `instance_matrix`, `an_instance_keeps_one_enabled_admin` | `server/admin.rs`, `server/matrix.rs` `workspace_creation_follows_the_setting_over_rest`, `server/workspaces.rs` `the_creation_rule_is_checked_when_a_workspace_is_written`, `e2e/shell` (from the switcher, from the screen of a user with no workspace, and with `admins`), `server/auth.rs` (own account) |
| Reacting to changes, page sockets | `recompute_keeps_or_closes_the_socket` and the two after it | `server/socket_closes.rs`, one test per trigger and outcome, group membership and group deletion included |
| Reacting to changes, events sockets | | `server/events.rs` (`access.changed`, from grants and groups, `4401`, `4403`, `4404`, trash leaves it open) |

Comments, public shares and plugins have no route before V1 or V2: their rows are tested in the rules only, and their REST rows come with their routes.

## MVP scope

| Item | Tests |
| --- | --- |
| Accounts, sessions, invitation and reset links | `server/auth.rs` (with the setup code of ADR 0021), `server/admin.rs` (reset links), `server/workspaces.rs` (invitations), `server/cli.rs` (`reset-password`), `session.rs` unit tests (the `__Host-` cookie), `e2e/shell` (setup with and without the link, login, invitation, reset) |
| Forgot password by mail | `mail/outbox.rs` (no wait, bounded, stale messages, reserved places), `server/mail.rs` (link arrives and works, same answer for any address, limits, full outbox, `404` with mail off), `rate_limit.rs` unit tests, `e2e/mail` |
| Registration by mail | `server/register.rs` (verified account with no workspace, keyed hash, 5 wrong codes, expiry, replacement, same answer for any address, `409` for an address taken meanwhile, limits, `404` closed or with mail off), `session.rs` and `config.rs` unit tests, `e2e/mail` |
| Second factor by mail | `server/second_factor.rs` (no session before the code, single use, wrong password mails nothing, 5 wrong codes, purposes not interchangeable, off needs the password, ignored and `404` with mail off), `server/cli.rs` (notice at start), `e2e/mail` |
| Proof of a new address | `server/email_change.rs` (changes only after the code, taken address gets a notice, code bound to address and account, 5 wrong codes, limits, reset link voided; without a relay changes at once, unproven, factor off), `server/auth.rs`, `e2e/mail` |
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
| `OPERATIONS.md` complete | A document, checked by reading |

## V1 scope

| Item | Tests |
| --- | --- |
| Upgrade from every earlier release | `server/upgrade.rs`: the data folder each release left (`tests/upgrade/<tag>`, from `scripts/make-upgrade-fixture.sh`), restored, migrated and served: logins, roles, grants, tree, content, attachment, search, revisions, trash, a write |
| Interface in English, language per user (step 34) | `scripts/strings.ts`, run by `npm run build`, fails on words outside the catalogue, and `scripts/strings.test.ts` plants each kind of leftover; `scripts/i18n.test.ts` (the choice, then the browser, then English); `server/auth.rs` `the_interface_language_is_set_and_cleared_without_the_password`, `only_a_language_of_the_interface_is_accepted`; the unchanged end-to-end suite and `e2e/screens` for the English look |
| Groups and group grants, server (step 35) | `server/groups.rs` (create, rename, delete, members, names, pages, other workspaces, a group grant over REST), `server/matrix.rs` `group_actions_follow_the_matrix_over_rest`, `server/socket_closes.rs` and `server/events.rs` (joining, leaving, deleting), `server/search.rs` `results_only_come_from_readable_spaces` |
| Groups and group grants, screens (step 36) | `e2e/settings` `an admin makes a group, fills it, renames it and deletes it, and a member only reads it` (create, `taken`, add, rename, a member's read-only view, remove, a member who leaves the workspace leaves the count, delete), `a group opened while another loads shows its own members, and a group made meanwhile shows`, `a group does not say everyone is in it while the workspace's members are unknown`, `a group whose members fail to load says so, until another group opens`, `a group grant raises its members above their own grant, and the open page follows`; `server/socket_closes.rs` `losing_a_group_grant_above_ones_own_closes_with_4409` |
| Binary under 30 MB, no S3 code in a default build | `scripts/check-build.sh`: the dependency graph of each flavor in CI's `rust` job on every pull request; the size and S3 code of the release binary in the `e2e` job after each push to main, and of each binary in `release.yml` |

Not automated: the RAM targets (`bench/rss.sh`), except that a hash's memory goes back with no trim (`server/hash_memory.rs`, ADR 0023), and the release workflow as a whole, which runs on a tag.
