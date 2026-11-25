# ADR 0029: imports read deflated zips

Status: accepted.

## Context

Imports read only stored entries, to keep a decompressor out of the binary (step 20). That suits cadocs's own exports. Zips made by file managers and the default of `zip` are deflated, so a bulk import (step 53) refused almost every zip a person makes.

## Decision

Imports read stored and deflated entries, through async_zip's `deflate` feature. Other methods are refused (`file: compression`). Exports still store entries: attachments are mostly compressed already.

What a zip unpacks to is no longer bounded by its size, so:

- The sizes the directory declares, all together, stay under `CADOCS_MAX_IMPORT_MB`, else `413`.
- A page is read to at most 2 MiB, and an attachment to at most its declared size, which is at most `CADOCS_MAX_UPLOAD_MB`. A file that unpacks to less or more is refused.
- The packed sizes, all together, stay under the size of the zip, so entries cannot share bytes.

## Consequences

- New crates, all MIT or Apache-2.0 compatible: `async-compression`, `compression-codecs`, `compression-core`, `flate2`, `miniz_oxide` (Rust, no C).
- Binary: 20,087,144 bytes before step 53, 20,274,664 after (+0.19 MB), the new import code and screen included.
- RAM: an inflate state and its buffers while one entry is read, nothing at idle. Not measured; step 55 runs the benchmark.
