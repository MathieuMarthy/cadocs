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
├── web/                      Vite, Svelte and TipTap front end
├── bench/                    RSS benchmark
└── docs/
```

`cadocs-core` depends on nothing heavy, so permission tests compile and run quickly. `cadocs-store` uses it for the last owner and last instance admin checks, which must run inside a transaction. `cadocs-collab` knows the store only through a trait, so it can be tested with an in-memory store. Only `cadocs-server` knows about axum.

`cadocs-server` embeds `web/dist`, which is not committed. Build the front end first (`npm --prefix web ci && npm --prefix web run build`), otherwise the binary serves no page and the build prints a warning. Debug builds read the folder at run time.

`cadocs-bench` is outside `default-members`, so a plain `cargo build --release` does not build it. Built together, its dependency features would unify into the server's and change the shipped binary. Use `--workspace` for tests and clippy, and `-p` to build it.

The prototype contains `core`, `store`, `collab`, `server` and `bench`, step 10 of the MVP added `jobs` and step 16 `content`. The others are created when their stage starts, and the split may be reduced if a crate turns out too small to justify itself.

## Selected crates

Costs are qualitative until measured. "Binary" is the effect on the release binary, "RAM" the effect at run time.

### In the prototype

| Crate | Why | RAM | Binary |
| --- | --- | --- | --- |
| `tokio` | Async runtime, required by axum and sqlx. Features limited to what is used | Low, set by worker count | Medium |
| `axum` (`http1`, `json`, `tokio`, `ws`, no default features, `multipart` from the MVP) | Same stack as uptime-monitoring. Thin layer over hyper | Low. Socket buffers are set to 4 KiB each way instead of 128 KiB | Medium |
| `yrs` (`sync` feature, so observers are `Send`) | Yjs port, the core of ADR 0003. Includes sync and awareness | Scales with loaded documents | Medium |
| `sqlx` (`sqlite`, `runtime-tokio`, `migrate`, `macros` for `migrate!`, no default features) | Async SQLite with embedded migrations, path to PostgreSQL | Each connection has a thread and a page cache, bounded by pool size | Medium, bundled SQLite included |
| `rust-embed` (no `mime-guess`: the build produces six file types) | Static front end in the binary, as in uptime-monitoring | None until served (file-backed) | Size of the assets |
| `tracing`, `tracing-subscriber` (`env-filter`) | Structured logs | Low | Low |
| `thiserror` | Typed errors in the library crates | None | None |
| `anyhow` | Error context in the binary and tools | None | Low |
| `serde` (`derive` in the server), `serde_json` | Checking that awareness states are JSON, and the REST bodies. Already pulled in by yrs | None extra | None extra |
| `argon2` (`alloc`, `getrandom`, `password-hash`) | Password hashing at OWASP settings | 19 MB per hash, one at a time, returned by a trim (ADR 0015) | With the next two rows, 0.4 MB in all, routes included |
| `sha2`, `subtle`, `getrandom` | Session tokens, their hash and constant-time CSRF comparison. `sha2` and `getrandom` were already in the tree through `rust-embed` and `tempfile` | None | See `argon2` |
| `uuid` (`std`, `v7`) | Sortable ids | None | See `argon2` |
| `pulldown-cmark` (no default features), MIT | Markdown import in `cadocs-content` (ADR 0008). Its `html` and `getopts` features are off: HTML comes from the AST, and there is no binary | Low: a pull parser, nothing kept after an import | Low. Not linked into the server until a route uses it |
| `tungstenite` (no default features) | Only to recognise the frame-limit error behind axum's socket and answer `1009`. Same version as axum's, so it adds no code | None | None |

### Dev tools only

These never reach the server binary.

| Crate | Why |
| --- | --- |
| `tokio-tungstenite`, `futures-util` | WebSocket client in `cadocs-bench` and in integration tests. Same version as axum's, so it is not compiled twice |
| `tower` (`util`) | Calling the router directly in tests |
| `tempfile` | Temporary databases in tests |

`web/` has two npm dev dependencies for the same purpose: `@playwright/test` (Apache-2.0) for the browser tests, and `@types/node` to type-check them. Neither reaches `web/dist`. `svelte` (MIT) is a runtime dependency of the shell, and `@sveltejs/vite-plugin-svelte` (MIT) a build dependency. The shell is about 73 KB of JavaScript, 26 KB compressed, and TipTap stays in a chunk that only a page loads.

The editor (step 14) adds, all MIT except `highlight.js` (BSD-3-Clause): the TipTap block packages it builds from its registry (`@tiptap/extension-heading`, `-blockquote`, `-horizontal-rule`, `-link`, `-list`, already pulled in by `@tiptap/starter-kit`), `@tiptap/suggestion` for the slash menu (it brings `@floating-ui/dom` to place it), and `@tiptap/extension-code-block-lowlight` with `lowlight` and `highlight.js` for code. Only 18 languages are registered (`web/src/editor/code.ts`) instead of `lowlight`'s 37 common ones. The page chunk went from 521 KB to 632 KB, 165 KB to 203 KB compressed. None of this reaches the server's memory: the binary serves it from the embedded files.

Step 15 adds `@tiptap/extension-table` (it brings nothing new: `prosemirror-tables` already came with `@tiptap/pm`) and `@tiptap/extension-details` for toggles, both MIT. Callouts, links to pages and the table of contents are our own nodes, in `web/src/editor/blocks.ts` and `pagelink.ts`. `@tiptap/extension-table-of-contents` was left out: it writes an id onto every heading, which each tab would write into the shared document. The page chunk went to 706 KB, 226 KB compressed.

Step 16 adds a `plugin` node of our own (`blocks.ts`, ADR 0016) and `web/scripts/schema.ts`, which runs the registry under Node through Vite's `runnerImport` to export the schema: nothing new is installed, and `src/editor/schema.ts` is not part of the app.

### Planned

| Crate | Stage | Why | Cost note |
| --- | --- | --- | --- |
| `tower-http` | MVP | Timeouts, compression, tracing, limited features | Low |
| `rust-stemmers` | MVP | Snowball stemmers (ADR 0007) | Static tables in the binary |
| `zstd` | MVP | Compression of revisions | Bounded by compression level |
| `jiff` | MVP | Dates, as in uptime-monitoring | Low without the bundled tz database |
| `async_zip` | MVP | Streaming zip for exports with attachments and for imports | Low, nothing buffered whole |
| `lettre` | V1 | SMTP, optional feature | Only in builds that enable it |
| `rusty-s3` | V1 | Signs S3 requests, optional feature (ADR 0005) | Only in builds that enable it |
| `totp-lite` | V1 | TOTP codes | Very low |
| `openidconnect` or a smaller client | V1 | OIDC, optional feature. To evaluate against a hand-written flow | Pulls an HTTP client |

Fractional indexing, rate limiting, the data folder lock (`std::fs::File::try_lock`) and parsing `CADOCS_PUBLIC_URL` (`http::Uri`) are small enough to write without a dependency.

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
| `mimalloc`, `jemallocator` | glibc and `malloc_trim` hold the targets (ADR 0015). mimalloc was measured and lost, jemalloc was not needed |

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
