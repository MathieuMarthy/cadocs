# ADR 0012: Comment anchors as Yjs relative positions in SQL

Status: accepted

## Context

The first design stored each comment's anchor as a mark in the document. A `commenter` is read-only on the WebSocket (ADR 0003, ADR 0009), so that user could not write the mark, and a comment would have needed edit rights.

## Decision

- A comment stores two encoded Yjs `RelativePosition` values, for the start and the end of the range, in `comments.anchor_start` and `comments.anchor_end`. It also stores the quoted text in `comments.anchor_text`.
- Both anchor columns are `NULL` for replies and for comments on the whole page.
- The client computes the positions from the selection and sends them through REST. The server stores them without interpreting them.
- The highlight is a ProseMirror decoration computed on the client. It is never part of the document.
- If an anchor no longer resolves to a range, for example because the text was deleted, the comment stays in the panel with its quoted text and no highlight.

## Consequences

- `commenter` stays read-only on the wire, with no exception in the sync filter.
- Comments never change the document, so they create no revision and no CRDT history.
- Relative positions follow concurrent edits, because they point at Yjs item ids rather than offsets.
- Restoring a revision replaces the content, so anchors inside it stop resolving and fall back to the quoted text.
- Exports and public pages do not show comments, which is the desired behaviour.

## Rejected alternatives

- **A mark written by the server.** The commenter sends a range and the room applies the mark. This keeps marks, but every comment becomes a document edit, and deleting one needs a second edit.
- **Letting commenters write to the document.** Would require the server to parse each update and allow marks only, which is fragile.
- **Character offsets.** Wrong after the first concurrent edit.
