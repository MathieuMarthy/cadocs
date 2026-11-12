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
| Instance actions | `instance_matrix`, `an_instance_keeps_one_enabled_admin` | `server/admin.rs`, `server/workspaces.rs` `only_an_instance_admin_creates_a_workspace_and_they_own_it`, `server/auth.rs` (own account) |
| Reacting to changes, page sockets | `recompute_keeps_or_closes_the_socket` and the two after it | `server/socket_closes.rs`, one test per trigger and outcome |
| Reacting to changes, events sockets | | `server/events.rs` (`access.changed`, `4401`, `4403`, `4404`, trash leaves it open) |

Comments, public shares, groups and plugins have no route before V1 or V2: their rows are tested in the rules only, and their REST rows come with their routes.

## MVP scope

| Item | Tests |
| --- | --- |
| Accounts, sessions, invitation and reset links | `server/auth.rs` (with the setup code of ADR 0021), `server/admin.rs` (reset links), `server/workspaces.rs` (invitations), `server/cli.rs` (`reset-password`), `session.rs` unit tests (the `__Host-` cookie), `e2e/shell` (setup with and without the link, login, invitation, reset) |
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
| Docker image, binaries | `scripts/check-image.sh`: serve, setup, backup, restore in the image. CI runs it on `amd64` after each push to main, and `release.yml` on each architecture before pushing |
| Backup and restore | `server/backup.rs`, `scripts/check-image.sh` |
| `OPERATIONS.md` complete | A document, checked by reading in step 21 |

Not automated: the RAM targets (step 23, `bench/rss.sh`), and the release workflow as a whole, which runs on a tag.
