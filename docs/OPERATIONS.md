# Operations

How cadocs is configured and run. The prototype only reads the settings marked **P**. Default values are starting points, and the benchmark may change them.

## Configuration

Environment variables only, so no config file format has to be parsed or documented.

### Server

| Variable | Default | Meaning |
| --- | --- | --- |
| `CADOCS_DATA_DIR` **P** | `./data` | Data folder, on a local disk |
| `CADOCS_BIND` **P** | `127.0.0.1:3000` | Listen address |
| `CADOCS_PUBLIC_URL` **P** | none, required | Public URL: a scheme, a host and an optional port, with no path, as cadocs lives at the root of its host. Sets the allowed `Origin`, and the `Secure` cookie flag when it is `https`. `cadocs` refuses to start without it |
| `CADOCS_TRUSTED_PROXIES` **P** | empty | Proxies allowed to set `X-Forwarded-For`, comma-separated: addresses or blocks such as `172.18.0.0/16`. The rightmost address no listed proxy covers is the client |
| `CADOCS_LOG` **P** | `info` | Log filter |

### Collaboration

| Variable | Default | Meaning |
| --- | --- | --- |
| `CADOCS_FLUSH_INTERVAL_MS` **P** | `500` | Interval between flushes of a room |
| `CADOCS_COMPACT_UPDATES` **P** | `500` | Stored updates of a page that trigger compaction |
| `CADOCS_ROOM_IDLE_SECS` **P** | `60` | Delay before an empty room unloads |
| `CADOCS_ROOM_QUEUE_BYTES` **P** | `4194304` | Unflushed updates a room may hold |
| `CADOCS_MAX_ROOMS` **P** | `256` | Loaded rooms. Idle rooms are unloaded first. If all are live, new connections get `503` |
| `CADOCS_ROOM_MEMORY_MB` **P** | `64` | What loaded rooms may take together, 1 to 1048576, counted as 12 times each document's encoded size and at least 64 KiB per room. Past it, as past `CADOCS_MAX_ROOMS`: idle rooms unload first, then new connections get `503`. Rooms load one at a time, so the room that crosses it is the only one over |
| `CADOCS_MAX_SOCKETS` **P** | `512` | Open WebSockets of any kind. Past this, `503`. `DocRegistry` holds the count, for page and events sockets alike |
| `CADOCS_MAX_FRAME_BYTES` **P** | `8388608` | Largest incoming frame |
| `CADOCS_SEND_QUEUE` **P** | `256` | Queued messages per socket. Also the events a workspace hub keeps, up to 4096, which an events socket may fall behind by before it gets `resync` |
| `CADOCS_PING_SECS` **P** | `30` | Ping interval |
| `CADOCS_SOCKET_RECHECK_SECS` **P** | `60` | Time between two checks of every open socket: the longest a socket outlives its session, or a change made by a command beside the server. A change made through the API closes sockets at once |
| `CADOCS_PING_TIMEOUT_SECS` **P** | `90` | Silence before a socket closes, and the longest a single send may take. Must be above `CADOCS_PING_SECS` |
| `CADOCS_DOC_READS` | `2` | Concurrent document reads outside a room, at least 1 ([ARCHITECTURE.md](ARCHITECTURE.md#reading-a-document-outside-a-room)) |

### Content

| Variable | Default | Meaning |
| --- | --- | --- |
| `CADOCS_SESSION_DAYS` **P** | `30` | Session lifetime, 1 to 3650, extended by use |
| `CADOCS_MAX_UPLOAD_MB` | `25` | Largest attachment, 1 to 1048576 |
| `CADOCS_WORKSPACE_QUOTA_MB` | `0` | Attachment storage per workspace, `0` for no limit, at most 1048576 |
| `CADOCS_TRASH_DAYS` | `30` | Time in the trash before purging, 1 to 3650. The purge runs at start and then every hour |
| `CADOCS_REVISION_PAUSE_SECS` | `120` | Pause in editing that creates a revision. `0` takes one after each flush |
| `CADOCS_REVISION_MAX_SECS` | `600` | Longest editing time without a revision. At least the pause |
| `CADOCS_REVISION_KEEP_DAYS` | `30` | All revisions are kept this long. After that, one per day remains |

### Files

Attachments go to `data/files/` unless `CADOCS_FILE_STORE` is `s3` (ADR 0020). S3 needs a build with the `s3` feature (`cargo build --release --features s3`): a default build refuses `s3` at startup and carries no S3 code. The server checks the bucket with a `HEAD` at startup and refuses to start if it fails.

| Variable | Default | Meaning |
| --- | --- | --- |
| `CADOCS_FILE_STORE` | `local` | `local` or `s3` |
| `CADOCS_S3_ENDPOINT` | | Required for `s3`. The server's URL, `http` or `https`, with no path or credentials, such as `https://s3.internal.example.org`. Requests are path-style |
| `CADOCS_S3_BUCKET` | | Required for `s3`. The bucket must exist |
| `CADOCS_S3_ACCESS_KEY_ID` | | Required for `s3` |
| `CADOCS_S3_SECRET_ACCESS_KEY` | | Required for `s3`, unless the next one is set. Never both |
| `CADOCS_S3_SECRET_ACCESS_KEY_FILE` | | A file holding the secret, for container secrets. A final newline is ignored |
| `CADOCS_S3_REGION` | `us-east-1` | Used in signatures. Self-hosted servers accept the default |
| `CADOCS_S3_PREFIX` | empty | Put before every key, so applications can share a bucket. Segments such as `apps/cadocs`, with no leading `/`, no empty segment and no `.` or `..`. A `/` is added if missing. Two cadocs instances must never share a bucket and prefix: each one's cleanup would delete the other's files |
| `CADOCS_S3_DOWNLOADS` | `proxy` | `proxy`: the server streams each file. `redirect`: after the access check, the server answers `302` to a presigned link for safe images, and the browser fetches them from S3. Other files are still streamed. Use it only if browsers can reach `CADOCS_S3_ENDPOINT` |
| `CADOCS_S3_LINK_SECS` | `3600` | Lifetime of a presigned link, 2 to 604800. A link is the same for half of it, so browsers keep files in cache, and a leaked link works at most this long |
| `CADOCS_S3_CA_FILE` | | A PEM bundle trusted besides the system roots, for an internal certificate authority |

## Commands

| Command | Purpose |
| --- | --- |
| `cadocs` or `cadocs serve` | Run the server |
| `cadocs reset-password <email>` | Print a one-time reset link (ADR 0013) |
| `cadocs backup <dir>` | Write a consistent backup, on a running or stopped server |
| `cadocs restore <dir>` | Restore a backup into an empty data folder, on a stopped server |

`reset-password` reads the same settings as the server, including `CADOCS_PUBLIC_URL`, and refuses a disabled account. It never creates or migrates the database, and refuses one of another version, so after an upgrade the server must have started once. `reset-password` and `backup` can run beside a live server, because they only open the database as an ordinary SQLite client. The exclusive lock of ADR 0006 is taken by `serve` and `restore`.

## Data folder

```
data/
├── cadocs.lock      exclusive lock, held while serving
├── cadocs.db        with -wal and -shm while running; the WAL is cut back to 64 MiB after a checkpoint
├── files/           attachments, by opaque id, under two folders named from its last four digits (empty with S3)
└── tmp/             uploads and exports in progress, emptied at startup
```

The database frees deleted pages within the hour, through the `maintain_database` job (ADR 0019). The first start on a database created before that ADR runs one `VACUUM`. It needs free disk space of about twice the size of `cadocs.db`, for the rewritten copy and the WAL that receives it. Without that space, `serve` refuses to start until space is freed.

## Deployment

- **Platforms:** Linux `x86_64` and `aarch64`, on the `-gnu` targets. The server calls glibc's `malloc_trim` (ADR 0015), so a `musl` build does not link. musl's allocator is also too slow for this workload.
- **Docker:** a small glibc base image, a non-root user, and one volume on the data folder.
- **TLS:** handled by a reverse proxy (Caddy, nginx, Traefik). cadocs has no TLS code. Plain HTTP works on `localhost` only, because browsers accept `Secure` cookies there.
- **Reverse proxy:** it must forward WebSocket upgrades and keep them open past the ping interval. Set `CADOCS_TRUSTED_PROXIES`, otherwise rate limits see the proxy as one client.

## S3

An object's key is `CADOCS_S3_PREFIX`, then the file's path under `data/files/`: `<prefix>/ab/89/0190a1b2-…-0123456789ab`. A local folder therefore moves to S3 as it is, and back:

1. Stop the server.
2. Copy the files: `mc mirror data/files/ <alias>/<bucket>/<prefix>/`, or `rclone copy data/files/ <remote>:<bucket>/<prefix>/`.
3. Set `CADOCS_FILE_STORE=s3` and the `CADOCS_S3_*` settings, then start the server.

The local files can be deleted once the server serves them from S3. Uploads still go through `data/tmp`, which needs room for `CADOCS_MAX_UPLOAD_MB` per upload in progress.

Give cadocs its own key, limited to its bucket and prefix. With bucket versioning on, a compromised server can delete objects, but not their earlier versions. A policy for MinIO or any S3 server that reads IAM policies:

```json
{
  "Version": "2012-10-17",
  "Statement": [
    {
      "Effect": "Allow",
      "Action": ["s3:GetObject", "s3:PutObject", "s3:DeleteObject"],
      "Resource": ["arn:aws:s3:::wiki/cadocs/*"]
    },
    {
      "Effect": "Allow",
      "Action": ["s3:ListBucket"],
      "Resource": ["arn:aws:s3:::wiki"],
      "Condition": { "StringLike": { "s3:prefix": ["cadocs/*"] } }
    }
  ]
}
```

`HEAD` on the bucket at startup is allowed by `s3:ListBucket`. The orphan cleanup lists the prefix and needs listings in the S3 XML namespace, as AWS, MinIO, Garage and Ceph send them: a gateway that drops it makes that job fail on every run, while uploads and downloads keep working.

In `redirect` mode, only safe images (AVIF, GIF, JPEG, PNG, WebP) are redirected, and S3 serves them with the type, disposition and cache lifetime cadocs signed into the link. Every other file, SVG and HTML included, is still streamed by cadocs, with its sandbox CSP and `nosniff`, because S3 sends neither and its origin may be shared. `img-src` in the CSP adds the endpoint's origin.

## Security headers

Sent on every response, unless a handler sets its own value.

| Header | Value |
| --- | --- |
| `Content-Security-Policy` | `default-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'` |
| `X-Content-Type-Options` | `nosniff` |
| `Referrer-Policy` | `same-origin` |

With `CADOCS_S3_DOWNLOADS=redirect`, `img-src` also lists the origin of `CADOCS_S3_ENDPOINT`. `style-src` needs `'unsafe-inline'` because ProseMirror and table column widths use inline styles. Public pages use a stricter policy, with `script-src 'none'`, and `X-Robots-Tag: noindex` unless the share allows indexing.

## Backup and restore

A backup is a folder with `cadocs.db`, from `VACUUM INTO`, and a copy of `files/`. With S3, the backup holds the database only, and the files are backed up on the S3 side: bucket versioning, replication or a mirror. Restoring an older database beside a newer bucket leaves orphans, which the cleanup removes.

1. Copy the database first, then the files. A file newer than the database is at worst an orphan, which the cleanup job removes. The reverse order could leave a page pointing at a missing file.
2. Copying files never deletes anything from the backup folder, so a failed run can simply be repeated.

Restoring on a stopped server replaces the data folder. Browsers still open on older content push their newer edits back when they reconnect, which is normal CRDT behaviour.

## Upgrades

- Migrations run at startup, inside transactions, and the server refuses to start if the database is newer than the binary.
- There are no down migrations. Take a backup first, and roll back by restoring it.
- Releases follow semantic versioning from V1. Every release can upgrade from any earlier one.

## Failure behaviour

| Failure | Behaviour |
| --- | --- |
| Flush fails (disk full, I/O error) | Updates stay in the room queue and the flush retries with backoff. `/health` returns `503`. See below |
| Room queue full while flushes fail | The room stops accepting updates and closes its write sockets with `1013`. Clients keep their edits and resend them when they reconnect |
| Page deleted under a live room | The next flush drops the room's edits and closes its sockets with `4404`. `/health` stays `200` |
| Upload hits a full disk | The upload fails with `500`, its temporary file is removed, and nothing is recorded |
| S3 unreachable or refusing | At startup, the server does not start. Later, an upload fails with `500` and records nothing, and a download in `proxy` mode fails with `500`. In `redirect` mode the link is still given, and the browser gets the error from S3 |
| Job fails | Retried with backoff, then kept as `failed` for inspection |
| Second instance on the same folder | Refuses to start, because of the lock |

While a flush is failing, edits exist only in memory and in the clients. A process crash in that state loses what no client still holds.
