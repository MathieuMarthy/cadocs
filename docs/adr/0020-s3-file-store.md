# ADR 0020: S3 file store in the MVP, chosen at runtime

Status: accepted. Brings forward the S3 part of ADR 0005 and settles its details.

## Context

ADR 0005 planned S3 for V1, behind a Cargo feature. The first deployment shares a self-hosted S3 service (MinIO at first) with other projects: its storage, replication and backups already exist, so attachments cost no disk on the cadocs host, and the bucket can be versioned so that a compromised server cannot destroy files for good. The S3 service is reachable from users' browsers.

Most operators still have no S3, so local disk stays the default, and the choice belongs to the operator.

## Decision

- **Runtime choice.** `CADOCS_FILE_STORE=local|s3`, `local` by default. The S3 code sits behind the `s3` Cargo feature of `cadocs-server` and `cadocs-files`, so a default build carries none of it (the V1 exit criterion still holds). A build without the feature refuses `s3` at startup with a message that names the feature.
- **Settings.** `CADOCS_S3_ENDPOINT`, `CADOCS_S3_BUCKET`, `CADOCS_S3_ACCESS_KEY_ID` and `CADOCS_S3_SECRET_ACCESS_KEY` (or `CADOCS_S3_SECRET_ACCESS_KEY_FILE`, for container secrets) are required. `CADOCS_S3_REGION` defaults to `us-east-1`, which self-hosted servers accept. `CADOCS_S3_PREFIX` lets several applications share a bucket. URLs are path-style (`endpoint/bucket/key`), which every self-hosted server supports. TLS uses the system roots, plus `CADOCS_S3_CA_FILE` for an internal CA. Verification is never turned off.
- **Same layout as the disk.** An object's key is the prefix, then the file's path under `data/files/` (`<last 2>/<previous 2>/<id>`). A local folder therefore moves to S3 with `mc mirror` or `rclone copy`, and back, with no code.
- **Client.** `rusty-s3` signs requests and parses listings, with no I/O of its own, and `reqwest` with rustls sends them. The official AWS SDK stays rejected (ADR 0005).
- **Upload.** Unchanged up to the move into place: the body streams to `data/tmp`, hashed and limited on the way. The temporary file is then sent with one streamed `PUT` of known length, and removed. Since `CADOCS_MAX_UPLOAD_MB` is 25 by default and at most 1 TiB, a single `PUT` (up to 5 GiB on S3) covers the realistic range. Multipart waits for a need.
- **Download.** `CADOCS_S3_DOWNLOADS=proxy|redirect`, `proxy` by default.
  - `proxy`: cadocs streams the object, as from disk. It works when browsers cannot reach S3.
  - `redirect`: after the same permission check, cadocs answers `302` to a presigned `GET` that sets `response-content-type`, `response-content-disposition` and `response-cache-control`. No byte goes through cadocs. The disposition follows the same safe image list, so HTML or SVG is downloaded, never rendered, on the S3 origin either. `img-src` in the CSP gains the S3 origin.
  - A presigned URL is signed at the start of a window of half of `CADOCS_S3_LINK_SECS` (3600 by default), and valid for `CADOCS_S3_LINK_SECS`. Within a window every view gets the same URL, so the browser cache works. The `302` itself is never cached, so each view rechecks the permission.
- **Startup check.** `serve` sends a `HEAD` on the bucket and refuses to start if it fails, so a wrong setting shows at once and not at the first upload.
- **Cleanup and quota.** Unchanged. `purge_orphan_files` lists the prefix through `ListObjectsV2`, and reads each object's `LastModified`.

## Consequences

- A leaked presigned URL keeps working until it expires: at most `CADOCS_S3_LINK_SECS` after it was issued, even if the reader loses access. Revocation is otherwise checked at each view.
- With S3, `cadocs backup` (step 21) copies the database only, and the files are backed up on the S3 side: bucket versioning, replication or a mirror. Restoring an older database beside a newer bucket leaves orphans, which the cleanup removes.
- Uploads still need local disk for one temporary file each. `data/tmp` must hold `CADOCS_MAX_UPLOAD_MB` per concurrent upload.
- The release ships an S3 build beside the default one. Step 21 decides how: separate binaries and images, or one build with the feature.
- The cadocs credentials should only reach their bucket and prefix: `GetObject`, `PutObject`, `DeleteObject`, `ListBucket`. With versioning on, they should lack the right to delete versions. [OPERATIONS.md](../OPERATIONS.md) gives the policy.

## Rejected alternatives

- **Write-behind**: store on disk, then push to S3 with a job. Uploads would not depend on S3, but every read would check two places, and a file not yet pushed would be outside the S3 backups. To revisit if S3 slows uploads in measures.
- **A separate storage service.** Costs a process and network hops for what a trait does in the process.
- **Flat keys (`prefix/id`).** Simpler, but a local folder could no longer be mirrored as it is.
- **Fresh presigned URLs on each view.** Each URL differs, so the browser downloads every image again on every view.
- **Multipart uploads now.** More requests and code for files that fit one `PUT`.
