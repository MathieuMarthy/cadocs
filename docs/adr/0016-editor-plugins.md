# ADR 0016: Plugins from an operator library, with one envelope node for content

Status: accepted

## Context

Plugins would let people add blocks, themes, buttons, panels or client-side exports. Two risks shape the design.

- **Trust.** A plugin's JavaScript runs in the page with the user's session, so it can do anything that user can do.
- **Content loss.** `@tiptap/y-tiptap` 3.2.0 deletes a Yjs node or text whose node or mark type is missing from the schema (`deleteYType` in `createNodeFromYElement` and `createTextNodesFromYText`). The deletion then syncs to every client. A missing plugin would therefore erase its blocks. A tab left open across a server upgrade has the same problem with any block added since, plugin or not.

A ProseMirror schema has no catch-all node type: the binding calls `schema.node(name)`, which throws before any fallback node could apply. Keeping unknown names would require a fork of the binding.

## Decision

**Safety net, from the MVP.** The editor sets `enableContentCheck: true`. Content that does not fit the schema is then never deleted. The editor gets a `contentError` event, disables collaboration in that tab, and shows a banner asking the user to reload.

**Installation.** The operator installs plugin code on the server, in `data/plugins/<id>/<version>/`, with a manifest. This is the only way code enters cadocs. The admin interface lists installed plugins but never accepts uploaded code. cadocs may also ship built-in plugins embedded in the binary, such as alternative themes. They are file-backed like the rest of the front end, so they cost no RAM until served.

**Two families.** The manifest declares one of them.

| | `content` | `user` |
| --- | --- | --- |
| Examples | New blocks | Themes, client-side exports, panels, buttons, shortcuts |
| Writes to documents | Yes, through the envelope node only | Never |
| Offered by | A workspace admin, per workspace | The operator, for the whole instance |
| Enabled by | Each member, for inserting blocks | Each user, for the whole instance |
| Loaded for | Every reader of a page that contains its blocks | Only the users who enabled it |

The operator can mark a plugin as enabled by default. Each user can still disable it.

**Content plugins.** They never add node or mark types. They all use one `plugin` block node, part of the core schema from step 16, with three attributes:

| Attribute | Content |
| --- | --- |
| `kind` | The plugin and block, such as `kanban/board` |
| `data` | JSON owned by the plugin |
| `text` | Plain text written by the plugin, used when the plugin cannot run |

The node is an atom: the plugin renders it through a node view. An inline variant is added only when a plugin needs one. If the plugin is not offered any more, its blocks show a fallback with `text` and stay intact.

Rendering is not tied to the member's choice. A member's choice only controls what they can insert. Otherwise, readers who did not enable a plugin would see the fallback instead of their colleagues' content.

**User plugins.** They run only in the browser of the user who enabled them, and they follow that user across workspaces. A theme can be CSS alone, overriding the Brut tokens, with no JavaScript. A client-side export reads the document already loaded in the browser, so it sees only what the user can read and costs the server nothing.

**Server side.** The Rust AST has a `Plugin { kind, data, text }` node, known to the schema like any other. Text, HTML, Markdown and search use `text`. HTML escapes it like any other text, and public pages never run plugin code.

**Loading.** Plugin files are served from the instance origin, so the CSP stays `'self'`. The editor builds its blocks, slash menu and shortcuts from an internal registry from step 14 on. That registry is not a public API until plugins ship.

**Stage.** Plugins ship in V2. The safety net, the registry and the envelope node come in the MVP, because adding them later would require migrating content.

## Consequences

- No plugin, removed or missing, can delete content. A missing user plugin only removes a theme or a button.
- The schema comparison test of ADR 0008 is unchanged, since plugins add no node types.
- Every plugin is as trusted as its code: installing one is an operator decision, like installing any software on the server. A CSS-only theme still goes through the operator.
- Public pages, exports and search only show a content block's `text`. A plugin that writes poor fallback text degrades those paths.
- A content plugin owns the evolution of its `data`, and must read older versions of it.
- A plugin cannot add marks. Inline formatting stays in the core schema.

## Rejected alternatives

- **A fallback node for unknown names.** Not possible without forking the binding.
- **Each plugin declaring its own node types.** Gives plugins full control of the schema, but any missing plugin then means deleted content.
- **Plugins uploaded by users.** Code from any user would run in other users' sessions on the same origin.
- **Plugin code uploaded through the admin interface.** A stolen admin session would then be enough to run code for every user. Server access is a higher bar.
- **Workspace control of user plugins.** They touch no shared content, and a theme belongs to a person rather than to a workspace.
- **Sandboxed iframes for every plugin.** Safe for untrusted code, but much more work, and the trust model above does not need it.
