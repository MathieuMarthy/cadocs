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
- What depends on a password check must hold when the write happens, since a hash can wait 10 seconds: `create_session`, `UpdateBasis` and `use_password_reset` re-check inside the write.
- `POST /auth/setup` needs `AppState::setup_code`, new at each start and logged by `main` while no account exists (ADR 0021). `tests/common` sends `Api::setup_code`; the end-to-end `TestServer::setupCode` reads it from the log.
- Over HTTPS the cookie is `__Host-session` (`session::cookie_name`): tests on `common::ORIGIN` use `common::COOKIE`.
- A failed session touch retries on each request, up to 1 second each, with no back-off.
- An admin who deletes or disables themself through `/admin/users` gets no removal cookie, unlike `DELETE /me`: the next request is a `401`. The reset links an admin made go when they are disabled, demoted or deleted.

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
- `rusty-s3` 0.11 parses listings with `instant-xml`, which needs the S3 namespace on `ListBucketResult` (real servers send it, a fake must too). It addresses the bucket as `/bucket/` and writes spaces in the query as `+`, which MinIO accepts.
- Left open: the startup check needs the bucket to answer `HEAD` (`s3:ListBucket`).

## Revisions, export, import, backup

- A restore (`cadocs_jobs::restore`) checks, decodes under a turn of `CADOCS_DOC_READS`, opens the room, checks again, keeps the current content, then `rewrite`s, so edits typed since are kept as one more revision. `tests/revisions.rs` sets `CADOCS_REVISION_PAUSE_SECS=0`.
- Export (`api/markdown.rs`) reads through `DocRegistry::read`, and a spawned task writes the zip into a `tokio::io::duplex`; a failure drops the connection. The test client's `raw` reads chunked answers.
- Import checks `editor` before reading the body, bounds the zip directory by hand (`check_directory`), stores each linked entry with its declared size as its limit, then writes through `registry.rewrite`. A failure once the page exists trashes it, in a spawned task so a client leaving midway changes nothing. `pages.title` follows at the next flush. async_zip 0.0.19 needs `#![allow(deprecated)]`.
- Left open: a panic inside `fill` leaves the imported page live; an import keeps links to attachments it does not bring.
- `backup` and `restore` (`backup.rs`): the database through `write_backup` (`VACUUM INTO`, works beside a server) and `check_backup` (refuses a newer schema). Files are copied before and after the database; a grace period on orphan rows in `purge_orphan_files` would close the last window. Copies go through `.partial` names, renamed once synced. `restore` takes the lock and refuses a folder with a `cadocs.db`, `-wal` or `-shm`, since a stale WAL would replay into it.

## Tests

- `tests/common` holds the REST test client, and `tests/common/mail.rs` the relay helpers (`with_mail`, `with_relay_and`, `before_setup`, `messages`, `settle`, `body`).
- `tests/matrix.rs` tries every space action with each role and gathers the wrong cells before failing. A new space route adds its row there and its line in [COVERAGE.md](../COVERAGE.md). A denial is `404` when the caller cannot read the target, `403` when they can.

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
