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
| `CADOCS_REVISION_PAUSE_SECS` | `120` | Pause in editing that creates a revision |
| `CADOCS_REVISION_MAX_SECS` | `600` | Longest editing time without a revision |
| `CADOCS_REVISION_KEEP_DAYS` | `30` | All revisions are kept this long. After that, one per day remains |

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
├── cadocs.db        with -wal and -shm while running
├── files/           attachments, by opaque id, under two folders named from its last four digits
└── tmp/             uploads and exports in progress, emptied at startup
```

## Deployment

- **Platforms:** Linux `x86_64` and `aarch64`, on the `-gnu` targets. The server calls glibc's `malloc_trim` (ADR 0015), so a `musl` build does not link. musl's allocator is also too slow for this workload.
- **Docker:** a small glibc base image, a non-root user, and one volume on the data folder.
- **TLS:** handled by a reverse proxy (Caddy, nginx, Traefik). cadocs has no TLS code. Plain HTTP works on `localhost` only, because browsers accept `Secure` cookies there.
- **Reverse proxy:** it must forward WebSocket upgrades and keep them open past the ping interval. Set `CADOCS_TRUSTED_PROXIES`, otherwise rate limits see the proxy as one client.

## Security headers

Sent on every response, unless a handler sets its own value.

| Header | Value |
| --- | --- |
| `Content-Security-Policy` | `default-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'` |
| `X-Content-Type-Options` | `nosniff` |
| `Referrer-Policy` | `same-origin` |

`style-src` needs `'unsafe-inline'` because ProseMirror and table column widths use inline styles. Public pages use a stricter policy, with `script-src 'none'`, and `X-Robots-Tag: noindex` unless the share allows indexing.

## Backup and restore

A backup is a folder with `cadocs.db`, from `VACUUM INTO`, and a copy of `files/`.

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
| Job fails | Retried with backoff, then kept as `failed` for inspection |
| Second instance on the same folder | Refuses to start, because of the lock |

While a flush is failing, edits exist only in memory and in the clients. A process crash in that state loses what no client still holds.
