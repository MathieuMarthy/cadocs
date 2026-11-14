<h1 align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/assets/logo-dark.png">
    <img src="docs/assets/logo-light.png" alt="cadocs" width="280">
  </picture>
</h1>

<p align="center">
  <strong>A self-hosted wiki your team edits together, on a server with 11 MB of RAM to spare.</strong>
</p>

<p align="center">
  <a href="https://github.com/MathieuMarthy/cadocs/actions/workflows/ci.yml"><img src="https://github.com/MathieuMarthy/cadocs/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-AGPL--3.0-141414?style=flat-square" alt="License: AGPL-3.0"></a>
  <a href="https://github.com/MathieuMarthy/cadocs/pkgs/container/cadocs"><img src="https://img.shields.io/badge/docker-ghcr.io-ff5a1f?style=flat-square" alt="Docker image on ghcr.io"></a>
  <img src="https://img.shields.io/badge/built_with-Rust_%2B_Svelte-141414?style=flat-square" alt="Built with Rust and Svelte">
</p>

<p align="center">
  <a href="#features">Features</a> ·
  <a href="#run-it">Run it</a> ·
  <a href="docs/OPERATIONS.md">Operations</a> ·
  <a href="#documentation">Documentation</a>
</p>

<br>

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/assets/promo-dark.png">
  <img src="docs/assets/promo-light.png" alt="A page open in the cadocs editor, with its tree of pages, numbered headings, tasks, a note and a second person's caret.">
</picture>

## Features

cadocs is a lightweight alternative to Docmost and Confluence. It ships as one binary and one data folder, with no database server, cache or queue to run beside it. The server idles near 11 MB of RAM and stays under 100 MB with 30 people editing ([measures](docs/ARCHITECTURE.md#mvp-results)).

- **Live editing:** several people on one page, with their cursors, and nothing lost across restarts.
- **A real editor:** numbered headings, tasks, code with highlighting, callouts, toggles, tables, links to pages and a table of contents, from a slash menu or Markdown shortcuts.
- **Organized:** workspaces, spaces and a tree of pages you rearrange by dragging, with a trash.
- **Search:** full text, with stemming in 18 languages.
- **History:** revisions of each page, which you can view and restore.
- **Files:** images and attachments, on disk or in an S3 bucket.
- **Markdown:** import and export of a page, with its attachments.
- **Access:** roles per workspace and per space, invitations by link.
- **Easy to run:** Docker images and binaries for `x86_64` and `aarch64`, `cadocs backup` and `cadocs restore`, light and dark themes.

## Run it

The simplest way is the Docker image, `ghcr.io/mathieumarthy/cadocs`, for `x86_64` and `aarch64`. Save this as `compose.yaml`:

```yaml
services:
  cadocs:
    image: ghcr.io/mathieumarthy/cadocs:latest
    restart: unless-stopped
    environment:
      # The address people open. Behind a TLS proxy, its https:// address.
      CADOCS_PUBLIC_URL: http://127.0.0.1:3000
    ports:
      - "127.0.0.1:3000:3000"
    volumes:
      - cadocs-data:/data

volumes:
  cadocs-data:
```

Then start it and read the setup link it logs:

```bash
docker compose up -d
docker compose logs cadocs | grep setup
```

Open that link to create the first account. To open the wiki to others, put a TLS proxy in front and set `CADOCS_PUBLIC_URL` to its address, as [docs/OPERATIONS.md](docs/OPERATIONS.md#deployment) shows with Caddy. The same page covers configuration, backups and upgrades.

### From source

Build the front end, then the binary, which embeds it:

```bash
npm --prefix web ci && npm --prefix web run build
cargo build --release
CADOCS_PUBLIC_URL=http://127.0.0.1:3000 target/release/cadocs
```

The server logs a setup link at its first start, as above.

## Documentation

- [Architecture](docs/ARCHITECTURE.md) and the [decisions](docs/adr/) behind it
- [API](docs/API.md), [data model](docs/DATA_MODEL.md) and [permissions](docs/PERMISSIONS.md)
- [Roadmap](docs/ROADMAP.md)

## License

[AGPL-3.0](LICENSE)
