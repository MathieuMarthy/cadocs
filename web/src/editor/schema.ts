// The editor's schema as plain data, which `cadocs-content` compares with its own (ADR 0008),
// and the Yjs document the editor's binding writes for a given content, which it reads back.
// Not part of the app: `scripts/schema.ts` loads it.

import { getSchema } from "@tiptap/core";
import { prosemirrorJSONToYXmlFragment } from "@tiptap/y-tiptap";
import * as Y from "yjs";

import { entries, extensionsOf } from "./registry";

type Attrs = Record<string, { default?: unknown }> | undefined;

/** Sorted by name, so the file this is written to changes only when the schema does. */
const byName = <T>(types: Record<string, T>) =>
  Object.entries(types).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));

const defaults = (attrs: Attrs) =>
  Object.fromEntries(byName(attrs ?? {}).map(([name, spec]) => [name, spec.default ?? null]));

const schema = () => getSchema(extensionsOf(entries));

export function schemaJson() {
  const { nodes, marks } = schema();
  return {
    nodes: Object.fromEntries(
      byName(nodes).map(([name, type]) => [
        name,
        {
          content: type.spec.content ?? "",
          group: type.spec.group ?? "",
          inline: type.isInline,
          atom: type.isAtom,
          attrs: defaults(type.spec.attrs),
        },
      ]),
    ),
    marks: Object.fromEntries(
      byName(marks).map(([name, type]) => [name, { attrs: defaults(type.spec.attrs) }]),
    ),
  };
}

/** The Yjs update the editor writes for ProseMirror JSON, checked against the schema first. */
export function editorUpdate(json: unknown) {
  const current = schema();
  current.nodeFromJSON(json).check();
  const doc = new Y.Doc();
  // A fixed client, so the same content gives the same bytes.
  doc.clientID = 1;
  prosemirrorJSONToYXmlFragment(current, json, doc.getXmlFragment("default"));
  return Y.encodeStateAsUpdate(doc);
}
