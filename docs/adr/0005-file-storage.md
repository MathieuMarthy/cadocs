# ADR 0005: Local disk for files, S3 as an option

Status: accepted

## Context

Pages embed images and attachments. A Raspberry Pi has a local disk. A VPS may prefer object storage.

## Decision

- A `FileStore` trait with streaming `put`, `get` and `delete`.
- Local implementation by default, under `data/files/`.
- Files are stored by opaque id with a two-level directory fan-out. Original names and types live in the `attachments` table, so user input never reaches a filesystem path.
- Uploads stream to a temporary file, are hashed while streaming, and move into place when complete. No file is ever held whole in memory.
- Size limits are enforced during the stream.
- Downloads check page permissions, and are sent with `Content-Disposition: attachment` unless the type is on a safe image list. User-uploaded HTML or SVG must never run in the application's origin.
- S3 comes behind a Cargo feature, so that its dependencies stay out of default builds. ADR 0020 brought it into the MVP and settles its details.

## Consequences

- The default deployment remains one folder to back up.
- Image thumbnails are out of scope at first: decoding images costs memory peaks. If added, they run as a job limited to one at a time.

## Rejected alternatives

- **BLOBs in SQLite.** One-file backups, but the database grows large, and backups and WAL suffer.
- **Content-addressed deduplication.** Saves little on a wiki, and complicates deletion.
- **The official AWS SDK.** Its dependency tree is heavy. A small S3 client is preferred.
