# Store and permissions

What a change to the stores, roles or the workspace, space and page routes must know.

## Stores

- **One store per area, beside each other:** `DocStore`, `AccountStore`, `WorkspaceStore`, `GroupStore`, `SpaceStore`, `PageStore`, `AttachmentStore`, `SearchStore`, `RevisionStore`, `JobStore`, all on `SqliteStore`. `DocStore` uses native `impl Future + Send` methods, so collab is generic over it instead of `dyn`. `StoreError` wraps `sqlx::Error`, so a failing test store can return `sqlx::Error::PoolTimedOut`. `SqliteStore::open` needs the parent folder to exist; `open_existing` neither creates nor migrates, for commands beside a live server.
- **Roles are checked in the store.** A method takes the actor and checks the role itself, inside the write transaction (`authorize` in `sqlite/workspaces.rs`, `spaces.rs`, `pages.rs`, resolving through `SELECT_SPACE` or `SELECT_PAGE` and `cadocs-core`), so a route cannot forget it. A refusal becomes `SpaceNotFound`, `PageNotFound` or `NotAllowed`. Methods only jobs call (`index_target`, `index_page`, `latest_revision`, `add_revision`) check no role.
- **Last owner, last admin:** counted on the enabled ones who would stay. `would_remove_last_instance_admin` is called by `delete_user` and `administer_user`.

## Second factors

- The app (`user_totp`, `user_recovery_codes`, step 47) sits beside `users.email_2fa`. `User::totp` comes from an `EXISTS` on an enabled row in every query that feeds `user(&row)`: a new one must select it too. `check_basis` is the password and session check of `update_user` for the other writes that follow a password check (`start_totp`, `replace_recovery_codes`). Turning the app off is `UserChanges::totp_off` through `update_user`, or `turn_totp_off` for the command line.

## Identities (step 48)

- `user_identities` maps `(provider, subject)` to an account. `identity_user` reads it like `credentials`, through `SELECT_CREDENTIALS`; `create_identity_session` opens a session with no password for an enabled user; `register_identity` makes the account, its identity and its session in one transaction, refuses before setup (`NotAllowed`) and on an address in use (`EmailTaken`), never linking to that account.

## Documents

- The room assigns `seq` from `StoredDoc::last_seq() + 1`, and must flush its queue before it compacts. `compact` refuses an older snapshot (`StaleSnapshot`). `append_updates` can be replayed after a lost commit, and answers `SeqConflict` for a `seq` the snapshot covers or that holds other data. It takes a `DocEdit` (title and author for the `pages` row), and answers `PageNotFound` without that row: a test on `SqliteStore` inserts one.
- Snapshots are zstd level 3 with `codec = 1`. `load` decodes by `codec`, so raw rows stay readable and shrink at their next compaction. A state that does not decompress is `CorruptSnapshot`.
- `load` holds a document's whole update log: 3.0 MB more for 100 documents of 2,000 rows, so it stays.

## SQLite upkeep (ADR 0019)

- New databases use `auto_vacuum = INCREMENTAL`; `open` converts an older one with a `VACUUM`. `maintain_database` runs `incremental_vacuum` by steps of 1,024 pages until a step frees nothing, then `optimize`; it is the last of `Cleanup::RECURRING`. Do not add `mmap_size` or a forced `wal_checkpoint(TRUNCATE)`. `VACUUM INTO` (backup) reads the WAL. A PostgreSQL store implements `maintain_database` as nothing.
- RUSTSEC-2026-0459 is ignored in `deny.toml`: sqlx 0.9.0 caps `libsqlite3-sys` below 0.38, the fix is in 0.39. Take the sqlx release that lifts the cap, then remove the ignore.

## Permissions

- `cadocs-core::permissions` holds the matrix, the special cases and the recompute of page sockets. `Authorizer` and `PageAuthorizer` are in `cadocs-collab`.
- **A handler whose change can alter what a user reaches calls `state.access_changed()` once it is stored.** That wakes `watch_access` in `lib.rs`, which runs `DocRegistry::recheck` over every page and events socket. It does not wait, so a test waits for the close frame. The `CADOCS_SOCKET_RECHECK_SECS` timer covers expired sessions, commands beside the server and a failed pass. `upgrade` rechecks once after joining, which closes the race with a change landing before the join. A pass costs two queries per socket, no dedupe.
- A socket never extends its session: the cookie only renews on a REST response. Left open: no screen renews it on a timer, so an editor making no REST call gets `4401` at the end of the lifetime.
- Tests about something else use `Fixed` in `tests/server.rs`.

## Workspaces, spaces, pages

- Changing a workspace's `language` queues a reindex of every page (`queue_reindex` in `sqlite/search.rs`). `purge_expired` deletes expired invitations. `GET /invitations/{token}` has no rate limit, kept by the security review ([OPERATIONS.md](../OPERATIONS.md#security)).
- Groups (step 35) are a `GroupStore` beside `WorkspaceStore`. `authorize_group` in `sqlite/groups.rs` finds the workspace from the group row, then answers `GroupNotFound` to a stranger. Grants to groups resolve in `space_access!` only, which every read path shares. New tests make groups over REST with `new_group` and `join_group` in `tests/common`; `group_grant` in `tests/spaces.rs` still inserts one with SQL. The `purge_trash` job deletes a trashed space; no route does.
- `POST /spaces/{space}/pages` takes no title: the Yjs document holds it. `page_to_edit` is the check of a change through a room. The store publishes no event: handlers do (see [server notes](server.md)).
- `position` is computed from `after` and only grows: alternating moves inflate it, and it is also the tree cursor. No cap, since appending grows a key by a digit every 30 pages or so, and writers are trusted (ADR 0014).
- Copies (step 50): `subtree_to_copy` reads with `ReadPage` and stops its recursive walk at `limit` rows (`ORDER BY` depth and `LIMIT` inside the CTE); `create_copies` checks `duplicate_allowed` in the write transaction, the depth from the copies' own levels, and inserts the copies below the top one with their source's `position`. The subtree is read before the transaction; `page_to_read` checks each source again as it is filled. `trash_failed_copy` is the one page write with no role check, for a copy that failed: it takes the ids the copy made and trashes only those. A live page of someone else moved under one of them meanwhile goes up to the first page above that is not the copy's (the place of the copy, for a copy left where it was made), last among its new siblings, so a live page never sits under a trashed one, which share coverage relies on.
- `tree` resolves the role per row. A tree has at most `MAX_PAGE_DEPTH` levels, from SQLite's limit on nested triggers.

## Attachments

- The quota is checked again inside `create_attachment`, so two uploads at once cannot both pass. `DELETE` removes the row with every reference, then the handler deletes the file, so it needs `editor` on every referencing page, trashed ones included (`delete_allowed`).
- The indexing job adds references with one `INSERT … SELECT` per id, from the page's workspace only: an orphan deleted first gives nothing, one referenced first is kept by `forget_orphan`.

## Comments

- `CommentStore` (step 37) checks on the page row: `authorize_comment` in `sqlite/comments.rs` finds the page from the comment, picks `EditOwnComment` or `EditOtherComment` by author, and turns `PageNotFound` into `CommentNotFound`. Resolving is `WriteComment` on any thread. Every write answers a `CommentChange` with the `PageSite`, so the handler publishes `Event::comment`.
- The body and the anchor are stored as given: nothing parses the Yjs positions. The handler reads the users a body mentions (`cadocs_content::comment::mentioned_users`) and passes them in `NewComment::mentions` or to `edit_comment`.
- Notifications (step 41): `mentions::replace` inserts a `mention` notification for each user it is about to insert a row for (same filter, author apart, and only those `mentions::readers` finds able to read the page, step 55) and deletes those of users no longer named, and `create_comment` adds `reply` then `comment` rows guarded by one row per (comment, user), for readers only. A thread takes `MAX_REPLIES` (100), checked in the write transaction, so a list of 100 threads holds at most 10,100 comments. `mentions::replace` answers the `mention` rows it inserted (`RETURNING`), so `index_page` hands them back in `Indexed`, with the page's `PageSite`, for the job's `notification` events (step 61); `CommentChange::notified` carries a comment's back for its handler's events. The inbox is the newest 100 rows (`rowid`) joined to their pages and filtered by `scope(row).allows(ReadPage)` in Rust, as `backlinks` does, so `unread` counts that window. Left open: a page's creator who left the workspace still gets `comment` rows that the filter hides.
- Mentions (step 39): `sqlite/mentions.rs` `replace` makes the rows of one place (a body, `comment_id IS NULL`, or a comment) those of a list, in the caller's transaction: a delete of the users gone, then one `INSERT … SELECT` from `workspace_members`, so a stranger or an unknown id is skipped without an error. Ids are `lower(hex(randomblob(16)))`, since the job has no UUID generator and nothing sorts by them. `index_page` calls it with the page's `updated_by` as author. Its delete also drops users no longer in `workspace_members`, and `remove_member` deletes the mentions of the one who leaves on the pages of that workspace (`forget_member`), so a row always names a member. Comment mentions go with their comment by the foreign key. `member_names` (on `RevisionStore`, beside `readable_titles`) answers names for renderers, only to a member of the page's workspace.
## Shares

- `ShareStore` (`sqlite/shares.rs`): create, list and revoke check `ManageShares` on the page; a revoke that cannot read the page is `ShareNotFound`. The public lookups take a token and check only the creator's role: `open_share` runs `authorize` with `ManageShares` for `created_by`, at each request (ADR 0030), and they answer `None` alike for every closed case. A creator who gets the role back reopens their shares, so nothing deletes shares on role changes. `list_shares` leaves expired shares out, closed ones in. Tokens are stored as is, since editors list them again.
- `attachment_refs` only grows: a revision may still show a file, downloads by readers and the orphan cleanup rely on that, so it cannot say what a page shows now. `page_attachments` does: `index_page` replaces a page's rows with what its content holds, beside the references, and only public files read it. An upload adds a reference, not a row there, so a file uploaded to a shared page and never placed stays closed. Migration 0016 queues a reindex of every page to fill it.
- Coverage climbs `parent_id` from a page until it meets the shared page (`COVERED_BY_CLIMB`), which needs no trash check: a live page never sits under a trashed one. Code that breaks that rule (a restore that keeps a trashed parent) must keep the climb right.

## Search

- `search` takes an FTS5 expression, which only `cadocs-search` builds, and filters with `HAS_ROLE`, live pages and live spaces. `with_reindex_delay` exists for tests; production waits 5 seconds (`REINDEX_DELAY`).
- Left open: a prefix typed past the end of a stem finds nothing until the word is whole (`deploym` misses `deployment`, stemmed `deploy`).

## Revisions

- `append_updates` records the flush's author in `revision_authors` and moves the `create_revision` job (`note_edit`) with an upsert on the partial index `jobs_dedup`, so every flush wakes the worker. `readable_titles` answers the titles a renderer may show. `with_revision_timing` takes `CADOCS_REVISION_PAUSE_SECS` and `_MAX_SECS`; tests that wait for jobs must not count `create_revision`, which waits 120 seconds by default (`run` in `tests/jobs.rs`).
- Left open, all only noise in the history: a restore and a revision job at the same moment can store the same content twice; a job queued before a page went to the trash still takes its revision; a job between a restore's rewrite and its last `keep` numbers the replaced content after the restored one.
- Templates (step 51): `templates` is space-scoped and checked with `spaces::authorize` (`ReadPage` to list and use, `EditPage` to save and delete), so a template of an unreadable space is `TemplateNotFound`. `state` is packed by `cadocs_jobs::pack` and read by `decode_bounded`, like a revision. `template_to_use` checks the target space and the workspace in one read; the server then creates an empty page and fills it as a duplicate does (`copies::copy_file`, `discard`).
