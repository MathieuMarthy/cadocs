# Store and permissions

What a change to the stores, roles or the workspace, space and page routes must know.

## Stores

- **One store per area, beside each other:** `DocStore`, `AccountStore`, `WorkspaceStore`, `GroupStore`, `SpaceStore`, `PageStore`, `AttachmentStore`, `SearchStore`, `RevisionStore`, `JobStore`, all on `SqliteStore`. `DocStore` uses native `impl Future + Send` methods, so collab is generic over it instead of `dyn`. `StoreError` wraps `sqlx::Error`, so a failing test store can return `sqlx::Error::PoolTimedOut`. `SqliteStore::open` needs the parent folder to exist; `open_existing` neither creates nor migrates, for commands beside a live server.
- **Roles are checked in the store.** A method takes the actor and checks the role itself, inside the write transaction (`authorize` in `sqlite/workspaces.rs`, `spaces.rs`, `pages.rs`, resolving through `SELECT_SPACE` or `SELECT_PAGE` and `cadocs-core`), so a route cannot forget it. A refusal becomes `SpaceNotFound`, `PageNotFound` or `NotAllowed`. Methods only jobs call (`index_target`, `index_page`, `latest_revision`, `add_revision`) check no role.
- **Last owner, last admin:** counted on the enabled ones who would stay. `would_remove_last_instance_admin` is called by `delete_user` and `administer_user`.

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
- `tree` resolves the role per row. A tree has at most `MAX_PAGE_DEPTH` levels, from SQLite's limit on nested triggers.

## Attachments

- The quota is checked again inside `create_attachment`, so two uploads at once cannot both pass. `DELETE` removes the row with every reference, then the handler deletes the file, so it needs `editor` on every referencing page, trashed ones included (`delete_allowed`).
- The indexing job adds references with one `INSERT … SELECT` per id, from the page's workspace only: an orphan deleted first gives nothing, one referenced first is kept by `forget_orphan`.

## Search

- `search` takes an FTS5 expression, which only `cadocs-search` builds, and filters with `HAS_ROLE`, live pages and live spaces. `with_reindex_delay` exists for tests; production waits 5 seconds (`REINDEX_DELAY`).
- Left open: a prefix typed past the end of a stem finds nothing until the word is whole (`deploym` misses `deployment`, stemmed `deploy`).

## Revisions

- `append_updates` records the flush's author in `revision_authors` and moves the `create_revision` job (`note_edit`) with an upsert on the partial index `jobs_dedup`, so every flush wakes the worker. `readable_titles` answers the titles a renderer may show. `with_revision_timing` takes `CADOCS_REVISION_PAUSE_SECS` and `_MAX_SECS`; tests that wait for jobs must not count `create_revision`, which waits 120 seconds by default (`run` in `tests/jobs.rs`).
- Left open, all only noise in the history: a restore and a revision job at the same moment can store the same content twice; a job queued before a page went to the trash still takes its revision; a job between a restore's rewrite and its last `keep` numbers the replaced content after the restored one.
