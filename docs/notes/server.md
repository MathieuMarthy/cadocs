# Server

What a change to HTTP, sockets, sessions, administration or mail must know.

## Shape

- `cadocs-server` is a library plus the `cadocs` binary. `serve::<S, A, E>` takes the store, the authorizer and the embedded assets as type parameters, so tests pass a failing store (`FlakyStore`) and fixture assets. `start_jobs` starts the worker beside `serve`, not inside it, so `serve` needs no `JobStore` and `FlakyStore` stays small.
- `AppState` fields that `main` sets after `AppState::new` (`workspace_creation`, `mail`, `registration`) keep other callers, such as the benchmark, on the defaults; `tests/common` sets them too.
- `privileges::drop_root` is the first thing `main` does, for every command, before any thread or file (ADR 0025). Code before it runs as root in the Docker image. It only acts for root with `CADOCS_RUN_AS` set, so only `scripts/check-image.sh` covers the real drop.
- Every command reads the full `Config`, so `CADOCS_PUBLIC_URL` is needed even by `backup`. `Config::secret` reads a secret or its `_FILE`, and strips all trailing whitespace.

## HTTP

- `api.rs` holds `ApiError` (build from `ErrorCode`, return with `?`) and `api::router`: add REST routes before its `finish` call, which only reaches existing routes. `/api/v1/` needs its own line, since a nest cannot reach it. A storage error becomes `503` through `From<StoreError>`.
- Read bodies with `api::JsonBody`, path parameters with `api::PathParam`, pages of a list with `api::PageQuery` and `api::Page`, so refusals keep the error shape. `axum` has no `query` feature, on purpose: `q` and `space` of search are read by hand.
- `api::rate_limit` is a route layer over `(Arc<RateLimiter>, Arc<HttpConfig>)`, `api::count_request` the same from a handler. Test a limit over a real socket (`tests/auth.rs`), since only `serve` gives the real `ConnectInfo`. `RateLimiter` counts an IPv6 /48 as one client and tracks 4096 at most; past that a newcomer waits, but a client is never reset.
- `security::add_headers` covers every response and leaves alone what a handler set. `PublicUrl::allows` accepts a missing `Origin`, so REST must not rely on it alone.

## Sessions

- A route that needs a user goes in `authenticated` in `api/auth.rs`, or gets `session::require`, which checks the session and CSRF; handlers read `Extension<CurrentSession>`. `session::require_in_handler` does it for a route open to everyone.
- What depends on a password check must hold when the write happens, since a hash can wait 10 seconds: `create_session`, `UpdateBasis` and `use_password_reset` re-check inside the write. So does the choice of a second factor: `create_session` refuses an account whose app, or whose code by mail with a relay (`mail`), went on during the hash, and `create_session_with_code` one whose app did; the login then answers `401` and the next try asks for the factor (`cadocs-store/tests/accounts.rs`).
- `POST /auth/setup` needs `AppState::setup_code`, new at each start and logged by `main` while no account exists (ADR 0021). `tests/common` sends `Api::setup_code`; the end-to-end `TestServer::setupCode` reads it from the log.
- Over HTTPS the cookie is `__Host-session` (`session::cookie_name`): tests on `common::ORIGIN` use `common::COOKIE`.
- A failed session touch retries on each request, up to 1 second each, with no back-off.
- An admin who deletes or disables themself through `/admin/users` gets no removal cookie, unlike `DELETE /me`: the next request is a `401`. The reset links an admin made go when they are disabled, demoted or deleted.

## Authenticator app (step 47)

- `totp.rs` holds RFC 6238 (HMAC-SHA-1, 6 digits, 30 s, a window of one step each way, checked against the RFC's vectors) and the recovery codes. The routes are in `api/auth.rs`, outside the mail blocks: they need no relay.
- At login the app comes first: `Credentials::totp_secret` set means `totp` is asked and nothing is mailed, whatever `email_2fa` says. Its refusals name the field `totp`, so the mail flow and its `code` are untouched.
- `AppState::totp_guesses` (`AccountLimiter`, by user id) bounds guesses per account, counted after the password matched and before the code is looked at, so an email alone locks nobody out. A test that logs in more than 5 times with the app in a minute hits it; `forget_step` in `tests/totp.rs` resets the replay guard instead of waiting.
- Replay: the step a login uses is stored by `create_session_with_totp` with `last_step < step`, in the session's transaction. A confirmed setup stores its step too, so the next login needs a later code (e2e uses step + 1).
- `cadocs reset-totp <email>` is the way back for someone who lost the app and the codes. A reset link leaves the app on, on purpose.
- Left open: turning the app on ends no other session, as for the code by mail; no screen shows how many recovery codes are left; a setup abandoned without `DELETE` keeps its pending row until the next setup or `DELETE`; the per-account limit counts right codes too, so someone who holds the password can keep the owner at `429` (they could log in anyway).

## Sockets

- Page and events sockets sit behind `api::check_origin` then `session::require` as route layers, so refusals are the JSON ones of REST and the upgrade touches the session.
- One task per socket does everything (`ws.rs`), since `Connection::next` takes `&mut self`: a blocked send stops that socket's reads, up to `CADOCS_PING_TIMEOUT_SECS`.
- The `tungstenite` version must stay axum's, or the downcast behind `1009` stops matching (a test covers it). A frame far over the limit can lose its `1009` to a TCP reset.
- `serve` flushes rooms before it waits on HTTP, and bounds that wait, or a stalled client holds up shutdown.

## Events

- `cadocs-collab::events` holds the hubs, reached through `DocRegistry` (`publish`, `publish_after_recheck`, `join_events`). A handler that changes the tree or a space publishes once it is stored; a rename's event comes from the flush, never from `PATCH`. A new space has no reader yet, so `space.created` goes through `publish_after_recheck`.
- A comment's change publishes `Event::comment` (`api/comments.rs`), to the readers of its space like a page event. Its body limit of 64 KiB assumes UTF-8 sent as is, as `JSON.stringify` does: a body of 10,000 characters written as `\u` escapes can pass it.
- Code that changes pages outside a handler must publish the same events, as `Cleanup::purge_trash` does for `purge_page`.
- `tests/events.rs` forces `resync` with a single-threaded runtime, since a real TCP client takes far too long to fall behind.

## Jobs

- `ServerJobs` runs `Index` for `reindex_page`, `Cleanup` for the rest. A new kind goes in a handler's `run`. One worker runs one job at a time, so a slow job holds up the next.
- Jobs are queued by the store inside the write that needs them, and `JobStore::jobs_added` wakes the worker. `JobStore` runs on the writer, reads too, so the poll keeps no reader open at idle.
- A store failure while recording the end of a job leaves it `running` until the next start, which runs it again.
- `tests/common` sets `with_reindex_delay(100 ms)`: search tests wait for results with `found`.

## Attachments and S3

- `FileStore` is object safe: `AppState` keeps an `Arc<dyn FileStore>`, and tests swap in a failing one (`FullDisk`) or one that links (`Redirecting`). `main::open_files` picks it from `Config::files`. `LocalFileStore::open` empties `data/tmp`, so it runs after the lock.
- An upload is the raw body, with no `DefaultBodyLimit`: `put` enforces the limit while streaming, and its temporary file goes on drop. A refusal that leaves a body unread can reach a test client as a reset, so the `announced` and `chunked` tests send nothing past what the server reads.
- A store with `download_url` gets a `302` after the access check, never cached, for `INLINE_TYPES` only: other files stream with the sandbox CSP and `nosniff`, which S3 cannot send. The router adds the store's origin to `img-src`.
- `S3FileStore` (`cadocs-files/src/s3.rs`, `s3` feature): every request is a presigned `rusty-s3` URL sent with `reqwest`, which follows no redirect. Keys are the disk paths under the prefix, and `list` skips other keys, so a shared prefix is safe. `valid_prefix` and `new` refuse a prefix or endpoint that could leave the bucket. Neither `Debug` nor errors show the secret or a presigned URL, which works for 5 minutes.
- `tests/s3.rs` runs one contract on an in-memory S3 that ignores signatures, and on a real one with `CADOCS_TEST_S3_ENDPOINT`, `_BUCKET`, `_ACCESS_KEY_ID`, `_SECRET_ACCESS_KEY`. MinIO no longer publishes community images: use `bitnamilegacy/minio:2025.4.22`.
- `rusty-s3` 0.12 parses listings with `instant-xml`, which needs the S3 namespace on `ListBucketResult` (real servers send it, a fake must too). It addresses the bucket as `/bucket/` and writes spaces in the query as `+`, which MinIO accepts.
- Left open: the startup check needs the bucket to answer `HEAD` (`s3:ListBucket`).

## Revisions, export, import, backup

- A restore (`cadocs_jobs::restore`) checks, decodes under a turn of `CADOCS_DOC_READS`, opens the room, checks again, keeps the current content, then `rewrite`s, so edits typed since are kept as one more revision. `tests/revisions.rs` sets `CADOCS_REVISION_PAUSE_SECS=0`.
- Export (`api/markdown.rs`) reads through `DocRegistry::read`, and a spawned task writes the zip into a `tokio::io::duplex`; a failure drops the connection. The test client's `raw` reads chunked answers.
- Import checks `editor` before reading the body, bounds the zip directory by hand (`check_directory`), stores each linked entry with its declared size as its limit, then writes through `registry.rewrite`. A failure once the page exists trashes it, in a spawned task so a client leaving midway changes nothing. `pages.title` follows at the next flush. async_zip 0.0.19 needs `#![allow(deprecated)]`.
- Left open: a panic inside `fill` leaves the imported page live; an import keeps links to attachments it does not bring.
- `backup` and `restore` (`backup.rs`): the database through `write_backup` (`VACUUM INTO`, works beside a server) and `check_backup` (refuses a newer schema). Files are copied before and after the database; a grace period on orphan rows in `purge_orphan_files` would close the last window. Copies go through `.partial` names, renamed once synced. `restore` takes the lock and refuses a folder with a `cadocs.db`, `-wal` or `-shm`, since a stale WAL would replay into it.

## Tests

- `tests/common` holds the REST test client (`outside` for paths beyond `/api/v1`, such as `/s/`), and `tests/common/mail.rs` the relay helpers (`with_mail`, `with_relay_and`, `before_setup`, `messages`, `settle`, `body`).
- `tests/matrix.rs` tries every space action with each role and gathers the wrong cells before failing. A new space route adds its row there and its line in [COVERAGE.md](../COVERAGE.md). A denial is `404` when the caller cannot read the target, `403` when they can.

## Public shares (step 45)

- `public.rs` serves `/s/{token}`, `/s/{token}/p/{slug}` and `/s/{token}/files/{id}` outside `/api`, with no session and no origin check, behind their own limiter (`public_requests`, 120 at once then one per 100 ms per client). The asset fallback and the front end's router leave `/s/` to the server (`assets.rs`, `SERVER_PATHS`).
- Order per request: limiter, `shared_view` (two to four queries), `registry.read`, render, then the `ETag` and `304`. A bad token never takes a turn of `CADOCS_DOC_READS`; a revalidation does.
- The `ETag` is the SHA-256 of the body served (and `indexable`). An `ETag` from `pages.updated_at` was tried first and gave `304` for edits still in the room, since `registry.read` sees them and a flush has not moved the date yet: a `304` could keep a paragraph an editor had removed. The cost is a read and a render per revalidation; the `304` only saves the body.
- Files open only when `page_attachments` lists them for a covered page (see [store notes](store.md#shares)), so a file removed from the content closes a few seconds later, once the indexing job has run (`REINDEX_DELAY` after the flush).
- The public policy is `default-src 'none'; script-src 'none'` with the one inline style sheet allowed by its SHA-256 (`STYLE`): a change to `STYLE` changes the hash by itself, and `a_script_in_content_cannot_run_on_a_public_page` checks the two match. `img-src` adds the file store's origin, since `api::attachments::send`, which public files share with downloads, redirects safe images there.
- A share reaches no socket, so its routes call no `access_changed()`: each public request checks the share again, and a revoke closes it at once.
- Left open: a share stays open when its creator loses the editor role or leaves (ADR 0009 makes it a capability), and an expired share stays listed until revoked. Public answers are HTML for `404` only: a `429` or `503` keeps the JSON error shape.

## Duplication (step 50)

- `api/copies.rs`: `subtree_to_copy` (breadth first, at most `MAX_COPY_PAGES` + 1 rows, so a huge subtree costs one row over the limit), fresh ids and slugs, `create_copies` (all rows, one transaction), one `page.created`, then `fill` in a spawned task like an import: per page, `page_to_read` on the source (unreadable, trashed or moved to another space fails the copy), `registry.read` with `read_page` and `state_len`, each file through `copy_file` once (`files` maps source id to copy, `None` to leave it), `map_attachments`, then `open`, `page_to_edit`, `rewrite`. A failure, or a panic of that task, which a second task awaits, trashes the copy's own pages, by the ids made for it, through `trash_failed_copy`, which checks no role: `move_to_trash` would refuse a caller who lost edit on the target and leave a half copy live. `tests/server.rs` `copy_while` uses `hold_load` to change rights between the first source's check and the next.
- RAM: one document and its AST at a time, plus the ids of at most 500 pages and the map of at most 5000 files. Every copy loads a room, which counts against `CADOCS_ROOM_MEMORY_MB` and unloads like any idle room, so a large copy evicts idle rooms rather than growing past the budget; with every room live it fails with `503` and goes to the trash.
- Step 51 (templates) can reuse `fill` and `copy_file`: a template page copied into a new page is the same work with one pair.
- Left open: links between pages of the copied subtree still point at the sources; the copy keeps the title as is (no "copy of"), which the client could change; a panic inside `fill` leaves the copy live, as for an import; a copy of a page with 500 descendants answers only once all are written, with no progress shown.

## Mail (ADR 0026)

- `AppState::mail` is `Option<Outbox>`: with `None`, the routes of the Mail stage are not registered, so they answer `404` for any method. A new mail route does the same.
- A handler never awaits a send: `Outbox::queue` never waits and refuses past 64 waiting messages; `Outbox::reserve` takes a place before replacing an earlier link or code, so a full outbox leaves it alone. Messages older than two minutes are dropped.
- A route that mails about an address answers at once, the same for every address, and does everything that depends on the account in a spawned task (`mail_reset`, `mail_registration`, `mail_new_address`). Keep it so.
- Limits: `mail_requests` per client (`429`), `mail_addresses` per address, silent past it (a keyed hash per address, 4096 at most).
- Codes (`email_codes`, one per purpose and address): add a `CodePurpose` and reuse `use_code`, which deletes a right code, counts a wrong one and kills it at the 5th; commit before returning `WrongCode`. Codes are hashed with `AppState::code_key`, drawn at each start. `mail_code` sends to an account's own address.
- Nobody registers before first-run setup: a route that makes an account must refuse then, as `register_user` does.
- `Credentials::needs_code(mail)` is the one rule for the second factor, which counts only with a relay. `main` warns at start when it is off and accounts have the factor.
- `SmtpMailer` builds its transport for each message. Keep message lines under 76 ASCII characters, or lettre sends quoted-printable and cuts links with `=`. A code sits alone on a line indented 4 spaces. Keep `normalize_email` before `Mail::to`, which accepts a quoted local part.
- `FakeRelay` (`cadocs_mail::fake`, dev only) is a real plain text SMTP server.
- Left open: no `CADOCS_SMTP_CA_FILE`; asking for a reset link voids earlier ones, up to the per-address limit; the per-address table refuses newcomers silently past 4096; a registration code for an address taken meanwhile answers `409 email: taken`; a login started just before the factor went on ends without a code; turning the factor on ends no other session; the old address is not told when an account moves; a pending code survives a password change; two asks at once can store one code while the other's message goes out last.
