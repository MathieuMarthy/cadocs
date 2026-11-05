# ADR 0008: Document conversion in Rust, straight from Yjs

Status: accepted

## Context

The server needs document text for search, and HTML or Markdown for public pages and exports, without Node. Docmost-style stacks convert through ProseMirror JSON using the TipTap JavaScript libraries. The Yjs document is the source of truth here, and y-prosemirror stores it as an `XmlFragment`.

## Decision

- The server walks the yrs `XmlFragment` directly and builds a small internal **AST**, with no ProseMirror JSON in the middle.
- Renderers go from the AST to plain text, HTML and Markdown.
- Import parses Markdown with `pulldown-cmark` into the same AST, and writes it to a Yjs document. HTML import is left to the browser, where paste already goes through TipTap.
- The node and mark schema is defined once in Rust. A test exports the TipTap schema from `web/` and compares it with the Rust definition, so the two cannot drift silently.
- An unknown node keeps its text and logs a warning. Content is never dropped.
- HTML output is built from the AST with escaping at every step, and link and image URLs go through a scheme allow-list. Public pages render content written by other users.

## Consequences

- Public pages render on the server and need no JavaScript.
- Every new editor node needs a Rust counterpart, and the test makes omissions visible.
- Markdown cannot represent everything, so some export paths are lossy (callouts, for example, use a documented convention).

## Rejected alternatives

- **Convert everything in the browser.** Works for a user's own export, but not for search, public pages or bulk operations.
- **Store ProseMirror JSON next to the Yjs state.** Two sources of truth, and it would need a client to keep it updated.
- **Embed a JS engine.** QuickJS could run the TipTap code, but it adds memory and complexity for a job a few hundred lines of Rust can do.
