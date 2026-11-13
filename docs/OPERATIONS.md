# Operations

How cadocs is configured and run. Default values are starting points, and the benchmark may change them.

## Configuration

Environment variables only, so no config file format has to be parsed or documented.

### Server

| Variable | Default | Meaning |
| --- | --- | --- |
| `CADOCS_DATA_DIR` | `./data` | Data folder, on a local disk |
| `CADOCS_BIND` | `127.0.0.1:3000` | Listen address |
| `CADOCS_PUBLIC_URL` | none, required | Public URL: a scheme, a host and an optional port, with no path, as cadocs lives at the root of its host. Sets the allowed `Origin`, and the `Secure` cookie flag when it is `https`. `cadocs` refuses to start without it |
| `CADOCS_TRUSTED_PROXIES` | empty | Proxies allowed to set `X-Forwarded-For`, comma-separated: addresses or blocks such as `172.18.0.0/16`. The rightmost address no listed proxy covers is the client |
| `CADOCS_LOG` | `info` | Log filter |

### Collaboration

| Variable | Default | Meaning |
| --- | --- | --- |
| `CADOCS_FLUSH_INTERVAL_MS` | `500` | Interval between flushes of a room |
| `CADOCS_COMPACT_UPDATES` | `500` | Stored updates of a page that trigger compaction |
| `CADOCS_ROOM_IDLE_SECS` | `60` | Delay before an empty room unloads |
| `CADOCS_ROOM_QUEUE_BYTES` | `4194304` | Unflushed updates a room may hold |
| `CADOCS_ROOM_MEMORY_MB` | `72` | What loaded rooms may take together, 1 to 1048576, counted as 12 times each document's encoded size and at least 64 KiB per room. It is the only bound on loaded rooms (ADR 0022). Past it, idle rooms unload first, then new connections get `503`. Rooms load one at a time, so the room that crosses it is the only one over |
| `CADOCS_MAX_SOCKETS` | `512` | Open WebSockets of any kind. Past this, `503`. `DocRegistry` holds the count, for page and events sockets alike |
| `CADOCS_MAX_FRAME_BYTES` | `8388608` | Largest incoming frame |
| `CADOCS_SEND_QUEUE` | `256` | Queued messages per socket. Also the events a workspace hub keeps, up to 4096, which an events socket may fall behind by before it gets `resync` |
| `CADOCS_PING_SECS` | `30` | Ping interval |
| `CADOCS_SOCKET_RECHECK_SECS` | `60` | Time between two checks of every open socket: the longest a socket outlives its session, or a change made by a command beside the server. A change made through the API closes sockets at once |
| `CADOCS_PING_TIMEOUT_SECS` | `90` | Silence before a socket closes, and the longest a single send may take. Must be above `CADOCS_PING_SECS` |
| `CADOCS_DOC_READS` | `2` | Concurrent document reads outside a room, at least 1 ([ARCHITECTURE.md](ARCHITECTURE.md#reading-a-document-outside-a-room)) |

`CADOCS_MAX_ROOMS` is no longer read: the room budget bounds loaded rooms (ADR 0022). A server that still gets it starts and logs at `warn` that it is ignored. To keep fewer rooms, lower `CADOCS_ROOM_MEMORY_MB`.

### Content

| Variable | Default | Meaning |
| --- | --- | --- |
| `CADOCS_SESSION_DAYS` | `30` | Session lifetime, 1 to 3650, extended by use |
| `CADOCS_MAX_UPLOAD_MB` | `25` | Largest attachment, 1 to 1048576 |
| `CADOCS_WORKSPACE_QUOTA_MB` | `0` | Attachment storage per workspace, `0` for no limit, at most 1048576 |
| `CADOCS_MAX_IMPORT_MB` | `100` | Largest import body, a page zipped with its attachments, 1 to 1048576. Written to `data/tmp` as it arrives |
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

Every command reads the same settings as the server, including `CADOCS_PUBLIC_URL`, so it runs with the server's environment: `docker exec` in a container, or the service's environment file. `reset-password` refuses a disabled account. It never creates or migrates the database, and refuses one of another version, so after an upgrade the server must have started once. `reset-password` and `backup` can run beside a live server, because they only open the database as an ordinary SQLite client. The exclusive lock of ADR 0006 is taken by `serve` and `restore`. [Backup and restore](#backup-and-restore) describes the last two.

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

- **Platforms:** Linux `x86_64` and `aarch64`, on the `-gnu` targets. The server calls glibc's `malloc_trim` and `mallopt` (ADR 0015, ADR 0023), so a `musl` build does not link. musl's allocator is also too slow for this workload.
- **Releases:** each `v*` tag publishes, on the GitHub release, `cadocs-<tag>-<target>.tar.gz` for `x86_64-unknown-linux-gnu` and `aarch64-unknown-linux-gnu`, each with a `.sha256`, and the same with `-s3` for the build with the S3 file store. They need glibc 2.36 or later (Debian 12, Ubuntu 24.04, RHEL 10). The images are `ghcr.io/mathieumarthy/cadocs:<version>` (also `latest`) and `:<version>-s3` (also `s3`), for both architectures. The default build carries no S3 code (ADR 0020).
- **Docker:** the image is the binary on a distroless glibc base, run as the `nonroot` user (uid 65532), with `CADOCS_DATA_DIR=/data`, `CADOCS_BIND=0.0.0.0:3000` and one volume on `/data`. A bind-mounted data folder must be writable by uid 65532. The image has no shell, so no `HEALTHCHECK`: an orchestrator checks `GET /health` from outside. Build it from the repository with `docker build .`, and `--build-arg FEATURES=s3` for S3.
- **From source:** `npm --prefix web ci && npm --prefix web run build`, then `cargo build --release` (with `--features s3` for S3), as [the Dockerfile](../Dockerfile) does. The binary embeds `web/dist`, so the front end is built first.
- **Memory:** cadocs is meant for 10 to 30 MB idle and under 100 MB with a few dozen active users. A container limit of 256 MB leaves room for login peaks: a password hash takes 19 MB while it runs, and one runs at a time.
- **TLS:** handled by a reverse proxy (Caddy, nginx, Traefik). cadocs has no TLS code. Plain HTTP works on `localhost` only, because browsers accept `Secure` cookies there.
- **Reverse proxy:** it must forward WebSocket upgrades and keep them open past the ping interval. Set `CADOCS_TRUSTED_PROXIES`, otherwise rate limits see the proxy as one client.

A Compose file, behind a proxy on the host:

```yaml
services:
  cadocs:
    image: ghcr.io/mathieumarthy/cadocs:latest
    restart: unless-stopped
    environment:
      CADOCS_PUBLIC_URL: https://wiki.example.org
      # The bridge network's gateway, which is how the proxy on the host reaches the container.
      CADOCS_TRUSTED_PROXIES: 172.16.0.0/12
    ports:
      - "127.0.0.1:3000:3000"
    volumes:
      - cadocs-data:/data
    mem_limit: 256m

volumes:
  cadocs-data:
```

And the proxy, here Caddy, which forwards WebSocket upgrades on its own and gets a certificate:

```
wiki.example.org {
	reverse_proxy 127.0.0.1:3000
}
```

With nginx, forward the upgrade (`proxy_http_version 1.1`, `Upgrade` and `Connection` headers), set `proxy_read_timeout` above `CADOCS_PING_TIMEOUT_SECS`, and `client_max_body_size` at least the larger of `CADOCS_MAX_UPLOAD_MB` and `CADOCS_MAX_IMPORT_MB`.

Without Docker, run the binary as its own user with a systemd unit:

```ini
[Unit]
Description=cadocs
After=network.target

[Service]
User=cadocs
EnvironmentFile=/etc/cadocs.env
Environment=CADOCS_DATA_DIR=/var/lib/cadocs
StateDirectory=cadocs
ExecStart=/usr/local/bin/cadocs serve
Restart=on-failure
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true

[Install]
WantedBy=multi-user.target
```

`/etc/cadocs.env` holds `CADOCS_PUBLIC_URL` and the other settings, and the commands read it too: `sudo -u cadocs sh -c 'set -a; . /etc/cadocs.env; CADOCS_DATA_DIR=/var/lib/cadocs cadocs backup /srv/backups/cadocs'`.

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

## Security

- **First run:** until the first account exists, `cadocs serve` logs `first-run setup is pending: open <public url>/setup/<code> to create the first account` at each start. Setup needs that code, so whoever reaches the instance first cannot claim it (ADR 0021). Read it with `docker logs cadocs` or `journalctl -u cadocs`. The code changes at each start, and a log filter above `warn` hides it.
- **Writers are trusted:** anyone with write access to a page can crash the server with a crafted update, which loses unflushed edits (ADR 0014). Give write access to known people only. Public shares stay read-only.
- **TLS:** the session cookie is `__Host-session` over HTTPS, so no other host of the domain can set or replace it. Let the proxy send `Strict-Transport-Security`, which cadocs leaves to it (with Caddy: `header Strict-Transport-Security "max-age=31536000"`).
- **Accepted by the review of step 24:**
  - Password guesses are limited per client (a burst of 10, then one every 15 seconds), not per account, and hashes run one at a time. Many addresses can guess in parallel, and a flood of logins makes others wait, then answer `503`. On an instance open to the internet, a limit at the proxy helps.
  - A session lasts `CADOCS_SESSION_DAYS` after its last use, with no absolute limit. Logout, a password change or reset, disabling and deletion end it.
  - Invitation links can be read without a limit: a token has 256 bits, so this only costs a hash and a read per request.
  - An attachment id works as a key inside its workspace ([PERMISSIONS.md](PERMISSIONS.md#special-cases)).
  - A page's position key grows a little with each move between two close siblings, so an editor scripting moves can make the keys of their own spaces long. Writers are trusted.
  - `style-src` allows inline styles (see below). Scripts only load from the instance itself.

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

`cadocs backup <dir>` runs beside a live server, or a stopped one, and creates `<dir>` if needed. It refuses a folder inside the data folder, or one that holds it.

1. It copies the files, then the database, then the files again. A file the database points to was there for the first pass or is new for the second, and a file newer than the database is at worst an orphan, which the cleanup job removes. Only a file uploaded between the first pass and the database copy, then dropped as an orphan before the second pass reaches it, can be missing from a backup whose database points to it.
2. Each copy goes to a `.partial` file, then takes its name, so a run cut short leaves the previous `cadocs.db` whole.
3. Running it again into the same folder replaces `cadocs.db` and copies only the files the folder lacks, as an attachment never changes once stored. It never deletes a file from the backup folder, so files deleted since the last run stay there, and a restore brings them back as orphans that the cleanup removes. Start a new folder, for example one per day, to keep several backups and to let old files go.

`VACUUM INTO` reads one snapshot and rewrites the database without its free pages, so the copy is often smaller than `cadocs.db`. It holds a read transaction while it runs, during which the WAL can grow past its usual size: the server keeps writing. The command prints the number of files it copied.

`cadocs restore <dir>` runs on a stopped server: it takes the lock `serve` takes, and refuses a data folder that holds a database (`cadocs.db`, or a leftover `-wal` or `-shm`). A folder with no database counts as empty, so a restore cut short can simply be run again. It checks the backup's database with `PRAGMA quick_check` and refuses one from a newer cadocs. An older one is fine: the next start migrates it. It copies the files, then moves the database in last. With S3 it copies the database only, whatever the backup folder holds.

With Docker, the backup folder is a second mount:

```sh
docker exec cadocs cadocs backup /backup                 # with /backup mounted in the container
docker compose stop cadocs
docker compose run --rm -v ./backup:/backup cadocs restore /backup   # into an empty volume
```

The container's user (uid 65532) must be able to write to the backup folder, and to read it for a restore.

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
