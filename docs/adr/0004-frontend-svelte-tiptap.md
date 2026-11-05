# ADR 0004: Svelte 5 and TipTap, embedded in the binary

Status: accepted

## Context

The editor should feel like Notion, with Markdown shortcuts and real-time collaboration. The front end does not affect server RAM much, since it is a static build served from the binary. It does affect download size and maintenance.

## Decision

- **TipTap** with its open source extensions, Yjs and `y-websocket`. No TipTap Pro extensions.
- **Svelte 5** for the application shell. TipTap's core is framework-agnostic, so the editor is used directly, with a small adapter for node views.
- **Vite** builds static files, and **rust-embed** puts them in the binary.
- The **Brut** design system provides tokens and components. Its fonts (IBM Plex, OFL licence) are **embedded in the binary** instead of loaded from Google Fonts as Brut's README suggests, because a self-hosted app must not depend on or leak to a third party.

## Consequences

- One artifact to deploy, and the server needs no Node at run time. Node is only a build dependency.
- Embedded assets are file-backed pages: they increase binary size but not anonymous memory.
- There is no official Svelte binding for TipTap, so we maintain the adapter.
- Since the design system is plain tokens and HTML, nothing ties it to a framework.

## Rejected alternatives

- **React.** Official TipTap bindings and a larger ecosystem, but a heavier runtime in the browser, for a shell that remains small.
- **Server-rendered templates (Askama).** Fine for uptime-monitoring, but an editor application is mostly client-side state.
- **Lexical, Slate or BlockNote.** Weaker Yjs and Markdown stories, or tied to React.
- **Assets on disk beside the binary.** Breaks the single binary goal for no memory gain.
