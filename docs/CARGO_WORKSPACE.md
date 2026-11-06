# Cargo workspace

## Target structure

```
cadocs/
├── Cargo.toml                workspace, shared dependency versions, release profile
├── crates/
│   ├── cadocs-core/          domain types and permission rules, no I/O
│   ├── cadocs-store/         store traits and the SQLite implementation, migrations
│   ├── cadocs-collab/        DocRegistry, rooms, y-sync, persistence loop
│   ├── cadocs-content/       Yjs ↔ AST ↔ Markdown, HTML, text (MVP)
│   ├── cadocs-search/        stemming and indexing (MVP)
│   ├── cadocs-files/         FileStore trait, local disk, S3 feature (MVP)
│   ├── cadocs-jobs/          worker and job handlers (MVP)
│   ├── cadocs-server/        the binary: config, HTTP, WebSocket, embedded assets
│   └── cadocs-bench/         y-sync load client, dev tool only
├── web/                      Vite and TipTap front end
├── bench/                    RSS benchmark
└── docs/
```

`cadocs-core` depends on nothing heavy, so permission tests compile and run quickly. `cadocs-collab` knows the store only through a trait, so it can be tested with an in-memory store. Only `cadocs-server` knows about axum.

`cadocs-server` embeds `web/dist`, which is not committed. Build the front end first (`npm --prefix web ci && npm --prefix web run build`), otherwise the binary serves no page and the build prints a warning. Debug builds read the folder at run time.

The prototype contains `core`, `store`, `collab`, `server` and `bench`. The others are created when their stage starts, and the split may be reduced if a crate turns out too small to justify itself.

## Selected crates

Costs are qualitative until measured. "Binary" is the effect on the release binary, "RAM" the effect at run time.

### In the prototype

| Crate | Why | RAM | Binary |
| --- | --- | --- | --- |
| `tokio` | Async runtime, required by axum and sqlx. Features limited to what is used | Low, set by worker count | Medium |
| `axum` (`http1`, `tokio`, `ws`, no default features, `multipart` from the MVP) | Same stack as uptime-monitoring. Thin layer over hyper | Low. Socket buffers are set to 4 KiB each way instead of 128 KiB | Medium |
| `yrs` (`sync` feature, so observers are `Send`) | Yjs port, the core of ADR 0003. Includes sync and awareness | Scales with loaded documents | Medium |
| `sqlx` (`sqlite`, `runtime-tokio`, `migrate`, `macros` for `migrate!`, no default features) | Async SQLite with embedded migrations, path to PostgreSQL | Each connection has a thread and a page cache, bounded by pool size | Medium, bundled SQLite included |
| `rust-embed` (no `mime-guess`: the build produces six file types) | Static front end in the binary, as in uptime-monitoring | None until served (file-backed) | Size of the assets |
| `tracing`, `tracing-subscriber` (`env-filter`) | Structured logs | Low | Low |
| `thiserror` | Typed errors in the library crates | None | None |
| `anyhow` | Error context in the binary and tools | None | Low |
| `serde`, `serde_json` | Checking that awareness states are JSON. Already pulled in by yrs | None extra | None extra |
| `tungstenite` (no default features) | Only to recognise the frame-limit error behind axum's socket and answer `1009`. Same version as axum's, so it adds no code | None | None |
| `mimalloc` (optional feature) | Candidate allocator, kept only if the benchmark justifies it | To be measured | Low |

### Dev tools only

These never reach the server binary.

| Crate | Why |
| --- | --- |
| `tokio-tungstenite`, `futures-util` | WebSocket client in `cadocs-bench` and in integration tests. Same version as axum's, so it is not compiled twice |
| `tower` (`util`) | Calling the router directly in tests |
| `tempfile` | Temporary databases in tests |

### Planned

| Crate | Stage | Why | Cost note |
| --- | --- | --- | --- |
| `argon2` | MVP | Password hashing | About 19 MB per hash, capped by a semaphore |
| `uuid` (`v7`) | MVP | Sortable ids | Very low |
| `tower-http` | MVP | Timeouts, compression, tracing, limited features | Low |
| `pulldown-cmark` | MVP | Markdown parsing, no allocation-heavy DOM | Low |
| `rust-stemmers` | MVP | Snowball stemmers (ADR 0007) | Static tables in the binary |
| `zstd` | MVP | Compression of revisions | Bounded by compression level |
| `sha2`, `subtle`, `getrandom` | MVP | Tokens and constant-time comparison, as in uptime-monitoring | Very low |
| `jiff` | MVP | Dates, as in uptime-monitoring | Low without the bundled tz database |
| `async_zip` | MVP | Streaming zip for exports with attachments and for imports | Low, nothing buffered whole |
| `lettre` | V1 | SMTP, optional feature | Only in builds that enable it |
| `rusty-s3` | V1 | Signs S3 requests, optional feature (ADR 0005) | Only in builds that enable it |
| `totp-lite` | V1 | TOTP codes | Very low |
| `openidconnect` or a smaller client | V1 | OIDC, optional feature. To evaluate against a hand-written flow | Pulls an HTTP client |

Fractional indexing, rate limiting and the data folder lock (`std::fs::File::try_lock`) are small enough to write without a dependency.

## Rejected crates

| Crate | Reason |
| --- | --- |
| `yrs-axum`, `y-sync`, `yrs-warp` | Superseded by `yrs::sync`, and they hide the hooks we need (ADR 0003) |
| `tantivy` | Second index and memory footprint, FTS5 is enough (ADR 0007) |
| `diesel`, `sea-orm` | ORM machinery for a small set of hand-written queries |
| `redis`, `apalis` | Replaced by the in-process design (ADR 0006) |
| `aws-sdk-s3` | Very large dependency tree (ADR 0005) |
| `async-graphql` | REST covers the need |
| `actix-web` | Fine, but axum shares the tokio and tower ecosystem already in use |
| `jemallocator` | Good at returning memory, but a slower build and larger binary. Would be compared if mimalloc disappoints |

## Release profile

```toml
[profile.release]
lto = true
codegen-units = 1
strip = true
panic = "abort"
opt-level = 3
```

These are the settings of uptime-monitoring, plus `panic = "abort"`, which removes unwinding tables. The trade-off: a panic in one task stops the process instead of only that task. For a server whose rooms hold unflushed edits this needs care, so the choice is confirmed or reversed once the panic paths have been reviewed. The prototype keeps the default unwinding until then.

Unwinding is load-bearing today. yrs can panic on crafted frames, and `cadocs-collab` catches that with `catch_unwind` to close one socket instead of losing a room. `panic = "abort"` would turn such a frame into a server crash, so it stays off until yrs is hardened or frames are validated first. This does not cover everything: yrs can also segfault on crafted frames ([ARCHITECTURE.md](ARCHITECTURE.md#limits-accepted-by-design)).

`[profile.dev.package.yrs]` turns off debug assertions and overflow checks for yrs alone, so tests exercise what the release binary does with crafted frames.
