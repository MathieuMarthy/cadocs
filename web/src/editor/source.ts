// The Markdown source of a block, shown where the caret is and edited as text (ADR 0018).
//
// A heading under the caret shows its hashes beside it, drawn by this tab alone. Arrowing left
// onto them, or Backspace at the start, writes the source into the block as text: `## ` in a
// heading, `[ ] ` in a task, `- ` in a bullet, `> ` in a quote, the fence line in code, `---` for
// a divider. The others see that text while it is edited. Once the caret leaves the block, the
// text is read again and the block becomes what it says.

import { Extension, type AnyExtension, InputRule } from "@tiptap/core";
import type { Node, NodeType, ResolvedPos } from "@tiptap/pm/model";
import {
  type EditorState,
  NodeSelection,
  Plugin,
  PluginKey,
  TextSelection,
  type Transaction,
} from "@tiptap/pm/state";
import { canJoin } from "@tiptap/pm/transform";
import { Decoration, DecorationSet, type EditorView } from "@tiptap/pm/view";
import { ySyncPluginKey } from "@tiptap/y-tiptap";

interface SourceState {
  /** The block written out as its source, which holds the caret. */
  pos: number | null;
  /** The type and text of that block, to find it again after Yjs rebuilt the document. */
  kind: string | null;
  text: string | null;
  /** The block holding the caret, once this tab changed it: only such a block is read unasked. */
  touched: number | null;
  /** Whether the editor has focus, since a hint shows only to the one typing. */
  focused: boolean;
}

const key = new PluginKey<SourceState>("markdownSource");

const HEADING = /^(#{1,3}) /;
const TASK = /^(?:[-*+] )?\[([ xX])\] /;
const BULLET = /^[-*+] /;
const ORDERED = /^(\d{1,9})[.)] /;
const QUOTE = /^> /;
const RULE = /^(?:---|\*\*\*|___)$/;
const FENCE = /^```([^\s`]*)[ \t]*$/;

/** What a textblock is, which says what its source looks like. */
type Role =
  | { kind: "heading"; level: number }
  | { kind: "item"; list: string; number: number; checked: boolean }
  | { kind: "quote" }
  | { kind: "code"; language: string | null }
  | { kind: "plain" };

/** The role of the textblock that `$pos` points into. */
function roleOf($pos: ResolvedPos): Role {
  const block = $pos.parent;
  if (block.type.name === "heading") return { kind: "heading", level: block.attrs.level };
  if (block.type.spec.code) return { kind: "code", language: block.attrs.language ?? null };
  const depth = $pos.depth;
  if (depth >= 2) {
    const container = $pos.node(depth - 1);
    const first = $pos.index(depth - 1) === 0;
    if ((container.type.name === "listItem" || container.type.name === "taskItem") && first) {
      const list = $pos.node(depth - 2);
      const index = $pos.index(depth - 2);
      return {
        kind: "item",
        list: list.type.name,
        number: (list.attrs.start ?? 1) + index,
        checked: !!container.attrs.checked,
      };
    }
    if (container.type.name === "blockquote") return { kind: "quote" };
  }
  return { kind: "plain" };
}

/** The source a block of this role starts with, or `null` if it has none. */
function sourceOf(role: Role) {
  switch (role.kind) {
    case "heading":
      return `${"#".repeat(role.level)} `;
    case "item":
      if (role.list === "taskList") return role.checked ? "[x] " : "[ ] ";
      return role.list === "orderedList" ? `${role.number}. ` : "- ";
    case "quote":
      return "> ";
    case "code":
      return `\`\`\`${role.language ?? ""}\n`;
    case "plain":
      return null;
  }
}

/** Whether a block of this role starts with text that reads as its source. */
function looksWritten(role: Role, text: string) {
  switch (role.kind) {
    case "heading":
      return HEADING.test(text);
    case "item":
      return TASK.test(text) || BULLET.test(text) || ORDERED.test(text);
    case "quote":
      return QUOTE.test(text);
    case "code":
      return FENCE.test(text.split("\n", 1)[0] ?? "");
    case "plain":
      return RULE.test(text);
  }
}

/** Whether a step of `tr` changed the block at `pos` of its new document. */
function changes(tr: Transaction, pos: number) {
  const node = tr.doc.nodeAt(pos);
  if (!node) return false;
  const end = pos + node.nodeSize;
  let hit = false;
  tr.mapping.maps.forEach((map, index) => {
    const rest = tr.mapping.slice(index + 1);
    map.forEach((_oldStart, _oldEnd, newStart, newEnd) => {
      if (rest.map(newStart, -1) <= end && rest.map(newEnd, 1) >= pos) hit = true;
    });
  });
  return hit;
}

/** The position before the textblock the selection is in, if it is in one. */
function blockAt($pos: ResolvedPos) {
  return $pos.parent.isTextblock && $pos.depth > 0 ? $pos.before() : null;
}

function inside(state: EditorState, pos: number) {
  const node = state.doc.nodeAt(pos);
  if (!node) return false;
  const { from, to } = state.selection;
  return from > pos && to < pos + node.nodeSize;
}

/** Whether `pos` is in the block being edited as source, where shortcuts must not fire. */
function inSource(state: EditorState, pos: number) {
  const source = key.getState(state)?.pos;
  if (source == null) return false;
  const node = state.doc.nodeAt(source);
  return !!node && pos > source && pos < source + node.nodeSize;
}

/** Writes the source of the block at `pos` into it, and puts the caret `back` before its end. */
function writeSource(tr: Transaction, pos: number, back: number) {
  const role = roleOf(tr.doc.resolve(pos + 1));
  const source = sourceOf(role);
  if (source === null) return false;
  tr.insert(pos + 1, tr.doc.type.schema.text(source));
  tr.setSelection(TextSelection.create(tr.doc, pos + 1 + source.length - back));
  tr.setMeta(key, { pos, kind: tr.doc.nodeAt(pos)!.type.name });
  return true;
}

/** Writes a divider out as its dashes, with the caret after them. */
function writeRule(tr: Transaction, pos: number) {
  const { schema } = tr.doc.type;
  tr.replaceWith(pos, pos + 1, schema.nodes.paragraph!.create(null, schema.text("---")));
  tr.setSelection(TextSelection.create(tr.doc, pos + 4));
  tr.setMeta(key, { pos, kind: "paragraph" });
}

/** Joins the node at `pos` with the same kind of node on either side, as one list or quote. */
function joinAround(tr: Transaction, pos: number) {
  const node = tr.doc.nodeAt(pos);
  if (!node) return;
  const end = pos + node.nodeSize;
  if (tr.doc.resolve(end).nodeAfter?.type === node.type && canJoin(tr.doc, end)) tr.join(end);
  if (tr.doc.resolve(pos).nodeBefore?.type === node.type && canJoin(tr.doc, pos)) tr.join(pos);
}

/**
 * Splits the container at `pos` so that its child `index` is alone in it, and returns where
 * that container now starts.
 */
function isolate(tr: Transaction, pos: number, index: number) {
  const container = tr.doc.nodeAt(pos)!;
  let offset = pos + 1;
  for (let i = 0; i < index; i++) offset += container.child(i).nodeSize;
  const after = offset + container.child(index).nodeSize;
  if (index < container.childCount - 1) {
    tr.split(after);
    // The items after it keep their numbers.
    if (container.attrs.start != null) {
      tr.setNodeAttribute(after + 1, "start", container.attrs.start + index + 1);
    }
  }
  if (index > 0) {
    tr.split(offset);
    return offset + 1;
  }
  return pos;
}

/** Wraps the block at `pos` as a quote, a list or a task, read from its stripped source. */
function wrap(tr: Transaction, pos: number, wrapper: (block: Node) => Node | null) {
  const block = tr.doc.nodeAt(pos)!;
  const wrapped = wrapper(block);
  if (!wrapped) return;
  tr.replaceWith(pos, pos + block.nodeSize, wrapped);
  joinAround(tr, pos);
}

/** The list a written item marker asks for: its list, its item and their attributes. */
function itemFor(text: string, nodes: Record<string, NodeType | undefined>) {
  let match = TASK.exec(text);
  if (match && nodes.taskList && nodes.taskItem) {
    return {
      length: match[0].length,
      list: nodes.taskList,
      listAttrs: null,
      item: nodes.taskItem,
      itemAttrs: { checked: match[1] !== " " },
    };
  }
  match = ORDERED.exec(text);
  if (match && nodes.orderedList && nodes.listItem) {
    return {
      length: match[0].length,
      list: nodes.orderedList,
      listAttrs: { start: Number(match[1]) },
      item: nodes.listItem,
      itemAttrs: null,
    };
  }
  match = BULLET.exec(text);
  if (match && nodes.bulletList && nodes.listItem) {
    return { length: match[0].length, list: nodes.bulletList, listAttrs: null, item: nodes.listItem, itemAttrs: null };
  }
  return null;
}

/**
 * Reads the block at `pos` again and makes it what its text says. `written` says the block was
 * written out as source, so a source now missing removes what it stood for.
 */
function readSource(tr: Transaction, pos: number, written: boolean) {
  const block = tr.doc.nodeAt(pos);
  if (!block?.isTextblock) return;
  const { schema } = tr.doc.type;
  const nodes = schema.nodes;
  const $pos = tr.doc.resolve(pos + 1);
  const role = roleOf($pos);
  const text = block.textContent;
  const start = pos + 1;

  if (role.kind === "code") {
    const line = text.split("\n", 1)[0] ?? "";
    const fence = FENCE.exec(line);
    if (fence) {
      tr.delete(start, start + line.length + (text.length > line.length ? 1 : 0));
      tr.setNodeAttribute(pos, "language", fence[1] || null);
    } else if (written) {
      // Without its fence, code is plain text, a paragraph per line.
      const lines = text.split("\n").map((each) => nodes.paragraph!.create(null, each ? schema.text(each) : null));
      tr.replaceWith(pos, pos + block.nodeSize, lines);
    }
    return;
  }

  if (role.kind === "item") {
    const depth = $pos.depth;
    const listPos = $pos.before(depth - 2);
    const index = $pos.index(depth - 2);
    const wanted = itemFor(text, nodes);
    if (!wanted) {
      if (!written) return;
      // No marker left: the item leaves its list, as the blocks it holds.
      const alone = isolate(tr, listPos, index);
      const list = tr.doc.nodeAt(alone)!;
      tr.replaceWith(alone, alone + list.nodeSize, list.child(0).content);
      return;
    }
    tr.delete(start, start + wanted.length);
    const list = tr.doc.nodeAt(listPos)!;
    const item = list.child(index);
    if (list.type === wanted.list && item.type === wanted.item) {
      if (wanted.itemAttrs) {
        let itemPos = listPos + 1;
        for (let i = 0; i < index; i++) itemPos += list.child(i).nodeSize;
        tr.setNodeAttribute(itemPos, "checked", wanted.itemAttrs.checked);
      }
      return;
    }
    const alone = isolate(tr, listPos, index);
    const single = tr.doc.nodeAt(alone)!;
    const content = single.child(0).content;
    tr.replaceWith(
      alone,
      alone + single.nodeSize,
      wanted.list.create(wanted.listAttrs, wanted.item.create(wanted.itemAttrs, content)),
    );
    joinAround(tr, alone);
    return;
  }

  if (role.kind === "quote") {
    if (QUOTE.test(text)) {
      tr.delete(start, start + 2);
      return;
    }
    if (!written) return;
    const depth = $pos.depth;
    const alone = isolate(tr, $pos.before(depth - 1), $pos.index(depth - 1));
    const quote = tr.doc.nodeAt(alone)!;
    tr.replaceWith(alone, alone + quote.nodeSize, quote.content);
    return;
  }

  // A heading, or a paragraph outside lists and quotes.
  const heading = HEADING.exec(text);
  if (heading && nodes.heading) {
    tr.delete(start, start + heading[0].length);
    tr.setNodeMarkup(pos, nodes.heading, { level: heading[1]!.length });
    return;
  }
  if (RULE.test(text) && nodes.horizontalRule) {
    tr.replaceWith(pos, pos + block.nodeSize, nodes.horizontalRule.create());
    return;
  }
  const paragraph = () => {
    if (block.type !== nodes.paragraph) tr.setNodeMarkup(pos, nodes.paragraph);
  };
  if (QUOTE.test(text) && nodes.blockquote) {
    tr.delete(start, start + 2);
    paragraph();
    wrap(tr, pos, (inner) => nodes.blockquote!.create(null, inner));
    return;
  }
  // Only a top level paragraph becomes a list: one inside an item would start a nested list.
  const wanted = $pos.depth === 1 ? itemFor(text, nodes) : null;
  if (wanted) {
    tr.delete(start, start + wanted.length);
    paragraph();
    wrap(tr, pos, (inner) => wanted.list.create(wanted.listAttrs, wanted.item.create(wanted.itemAttrs, inner)));
    return;
  }
  if (written && role.kind === "heading") paragraph();
}

/** The label of each heading: `01`, then `01.1` below it, counted from the highest level used. */
function headingNumbers(doc: Node) {
  const headings: { pos: number; node: Node }[] = [];
  doc.descendants((node, pos) => {
    if (node.type.name === "heading") headings.push({ pos, node });
    return !node.isTextblock;
  });
  const levels = [...new Set(headings.map(({ node }) => node.attrs.level as number))].sort();
  const counts: number[] = [];
  return headings.map(({ pos, node }) => {
    const rank = levels.indexOf(node.attrs.level);
    counts[rank] = (counts[rank] ?? 0) + 1;
    counts.length = rank + 1;
    const parts = Array.from({ length: rank + 1 }, (_, i) => counts[i] ?? 0);
    const label = [String(parts[0]).padStart(2, "0"), ...parts.slice(1)].join(".");
    return { pos, node, label };
  });
}

function decorations(state: EditorState, editable: boolean) {
  const { pos: source, focused } = key.getState(state)!;
  const found: Decoration[] = [];
  for (const { pos, node, label } of headingNumbers(state.doc)) {
    found.push(Decoration.node(pos, pos + node.nodeSize, { "data-number": label }));
  }
  state.doc.descendants((node, pos) => {
    if (node.type.spec.code && pos !== source) {
      found.push(Decoration.node(pos, pos + node.nodeSize, { "data-language": node.attrs.language || "auto" }));
    }
    return !node.isTextblock;
  });
  if (source !== null) {
    const $source = state.doc.resolve(source + 1);
    const block = $source.parent;
    found.push(Decoration.node(source, source + block.nodeSize, { class: "md-source" }));
    if (roleOf($source).kind === "item") {
      const item = $source.before($source.depth - 1);
      found.push(Decoration.node(item, item + $source.node($source.depth - 1).nodeSize, { class: "md-source" }));
    }
  } else if (editable && focused) {
    const { $from, empty } = state.selection;
    const block = empty ? blockAt($from) : null;
    const role = roleOf($from);
    if (block !== null && (role.kind === "heading" || role.kind === "quote")) {
      const hint = sourceOf(role)!.trimEnd();
      found.push(Decoration.node(block, block + $from.parent.nodeSize, { class: "md-hinted" }));
      found.push(
        Decoration.widget(block + 1, (view, getPos) => hintElement(view, getPos, hint), {
          side: -1,
          key: `hint-${hint}`,
          ignoreSelection: true,
        }),
      );
    }
  }
  return DecorationSet.create(state.doc, found);
}

/** The source shown beside a block, which a press writes out. */
function hintElement(view: EditorView, getPos: () => number | undefined, text: string) {
  const element = document.createElement("span");
  element.className = "md-hint";
  element.contentEditable = "false";
  element.textContent = `${text} `;
  element.addEventListener("mousedown", (event) => {
    event.preventDefault();
    // A widget is kept while its block moves, so its place is read at the press.
    const at = getPos();
    const tr = view.state.tr;
    if (at !== undefined && writeSource(tr, at - 1, 1)) view.dispatch(tr.scrollIntoView());
    view.focus();
  });
  return element;
}

/**
 * Brings the editor's selection to the one the page shows. A key pressed right after Home, End
 * or a click can come before the editor reads the moved selection, and would then act on the
 * old one. Only a caret or a range between texts is read: anything else is the editor's.
 */
function syncCaret(view: EditorView) {
  const shown = view.dom.ownerDocument.getSelection();
  const { selection, doc } = view.state;
  if (!(selection instanceof TextSelection) || !shown?.anchorNode || !shown.focusNode) return;
  if (!view.dom.contains(shown.anchorNode) || !view.dom.contains(shown.focusNode)) return;
  let anchor: number;
  let head: number;
  try {
    anchor = view.posAtDOM(shown.anchorNode, shown.anchorOffset);
    head = view.posAtDOM(shown.focusNode, shown.focusOffset);
  } catch {
    return;
  }
  if (anchor === selection.anchor && head === selection.head) return;
  const $anchor = doc.resolve(anchor);
  const $head = doc.resolve(head);
  if (!$anchor.parent.isTextblock || !$head.parent.isTextblock) return;
  view.dispatch(view.state.tr.setSelection(TextSelection.between($anchor, $head)));
}

/** A change that went through Yjs, which may rebuild the document: another tab's, or an undo. */
function rebuilt(tr: Transaction) {
  return !!tr.getMeta(ySyncPluginKey)?.isChangeOrigin;
}

/** Another tab's change. An undo goes through Yjs too, but it is this tab's. */
function isRemote(tr: Transaction) {
  const sync = tr.getMeta(ySyncPluginKey);
  return !!sync?.isChangeOrigin && !sync.isUndoRedoOperation;
}

/** Where the block at `pos` is after `tr`, or `null` if it is gone. */
function mapBlock(tr: Transaction, pos: number | null) {
  if (pos === null) return null;
  const mapped = tr.mapping.mapResult(pos, 1);
  return !mapped.deletedAfter && tr.doc.nodeAt(mapped.pos)?.isTextblock ? mapped.pos : null;
}

export const MarkdownSource = Extension.create({
  name: "markdownSource",
  // Before the keymaps, so that Backspace at a block's start edits its source first.
  priority: 1000,

  addCommands() {
    return {
      /** Reads the block being edited as source again, as if the caret had left it. */
      readSource:
        () =>
        ({ state, tr }) => {
          const pos = key.getState(state)?.pos;
          if (pos == null) return false;
          readSource(tr, pos, true);
          tr.setMeta(key, { pos: null });
          return true;
        },
    };
  },

  addProseMirrorPlugins() {
    const editor = this.editor;
    return [
      new Plugin<SourceState>({
        key,
        state: {
          init: () => ({ pos: null, kind: null, text: null, focused: false, touched: null }),
          apply(tr, value, _old, state) {
            const meta = tr.getMeta(key) as Partial<SourceState> | undefined;
            let { pos, touched } = value;
            const caret = blockAt(state.selection.$from);
            if (tr.docChanged && rebuilt(tr)) {
              // Yjs may rebuild the document. The caret never leaves a written block without
              // a read, so the block is the caret's, if it is still of the same type.
              // A neighbour of the same type, or the block after an undo, is told apart by its
              // text: the one this tab last saw, or one that still reads as its source.
              const node = caret === null ? null : state.doc.nodeAt(caret);
              const same =
                node?.type.name === value.kind &&
                (node.textContent === value.text || looksWritten(roleOf(state.selection.$from), node.textContent));
              pos = pos !== null && same ? caret : null;
              if (isRemote(tr)) touched = touched !== null ? caret : null;
              else if (caret !== null && changes(tr, caret)) touched = caret;
            } else if (tr.docChanged) {
              pos = mapBlock(tr, pos);
              touched = mapBlock(tr, touched);
              // Only a change in the caret's block, and not one this plugin made, marks it.
              if (!meta && caret !== null && changes(tr, caret)) touched = caret;
            }
            if (touched !== caret) touched = null;
            const next = { ...value, pos, touched, ...meta };
            const block = next.pos === null ? null : state.doc.nodeAt(next.pos);
            return { ...next, text: block ? block.textContent : null };
          },
        },

        appendTransaction(trs, old, state) {
          if (!editor.isEditable || trs.every(isRemote)) return null;
          const source = key.getState(state)!.pos;
          const tr = state.tr;
          let changed = false;

          // The block the caret left is read again: one written out, or one typed as source.
          if (source !== null && !inside(state, source)) {
            readSource(tr, source, true);
            tr.setMeta(key, { pos: null });
            changed = true;
          } else if (trs.some((each) => each.selectionSet || each.docChanged)) {
            const before = blockAt(old.selection.$from);
            const previous = key.getState(old)!;
            // Only a block this tab changed: another tab may be writing its source out, and
            // a block nobody edited keeps what it holds, such as Markdown pasted in code.
            const own = before !== null && before === previous.touched && before !== previous.pos;
            if (own && !old.doc.nodeAt(before)?.type.spec.code) {
              let pos = before;
              let gone = false;
              for (const each of trs) {
                const result = each.mapping.mapResult(pos, 1);
                gone ||= result.deletedAfter;
                pos = result.pos;
              }
              if (!gone && state.doc.nodeAt(pos)?.isTextblock && !inside(state, pos)) {
                readSource(tr, pos, false);
                changed = tr.docChanged;
              }
            }
          }

          // A divider the caret lands on is written out as its dashes.
          const selection = tr.selection;
          if (selection instanceof NodeSelection && selection.node.type.name === "horizontalRule") {
            writeRule(tr, selection.from);
            changed = true;
          }
          return changed ? tr : null;
        },

        props: {
          decorations: (state) => decorations(state, editor.isEditable),
          handleDOMEvents: {
            focus: (view) => {
              view.dispatch(view.state.tr.setMeta(key, { focused: true }));
              return false;
            },
            blur: (view) => {
              view.dispatch(view.state.tr.setMeta(key, { focused: false }));
              return false;
            },
          },
          handleKeyDown(view, event) {
            // Composition text is not in the document yet: the selection is left alone.
            if (view.composing || event.isComposing || event.keyCode === 229) return false;
            if (editor.isEditable) syncCaret(view);
            if (!editor.isEditable || event.ctrlKey || event.metaKey || event.altKey || event.shiftKey) {
              return false;
            }
            if (event.key !== "ArrowLeft" && event.key !== "Backspace") return false;
            const { state } = view;
            const { $from, empty } = state.selection;
            const block = blockAt($from);
            if (!empty || block === null || $from.parentOffset !== 0) return false;
            if (key.getState(state)!.pos === block) return false;
            const role = roleOf($from);
            const tr = state.tr;

            if (event.key === "ArrowLeft") {
              // Onto the source: before the space or line break that ends it.
              if (!writeSource(tr, block, 1)) return false;
              view.dispatch(tr.scrollIntoView());
              return true;
            }

            // Backspace writes the source out and deletes its last character, as in text.
            if (role.kind === "code") return false;
            if (role.kind === "plain") {
              const index = $from.index($from.depth - 1);
              const before = index > 0 ? $from.node($from.depth - 1).child(index - 1) : null;
              if (before?.type.name !== "horizontalRule") return false;
              const rule = block - before.nodeSize;
              if ($from.parent.content.size === 0) tr.delete(block, block + $from.parent.nodeSize);
              writeRule(tr, rule);
              view.dispatch(tr.scrollIntoView());
              return true;
            }
            if (!writeSource(tr, block, 0)) return false;
            const caret = tr.selection.from;
            tr.delete(caret - 1, caret);
            view.dispatch(tr.scrollIntoView());
            return true;
          },
        },
      }),
    ];
  },
});

declare module "@tiptap/core" {
  interface Commands<ReturnType> {
    markdownSource: {
      readSource: () => ReturnType;
    };
  }
}

/** The extension, with its shortcuts silent in a block being edited as source. */
export function quietInSource<T extends AnyExtension>(extension: T): T {
  return extension.extend({
    addInputRules() {
      const rules: InputRule[] = this.parent?.() ?? [];
      return rules.map(
        (rule) =>
          new InputRule({
            find: rule.find,
            undoable: rule.undoable,
            handler: (props) => (inSource(props.state, props.range.from) ? null : rule.handler(props)),
          }),
      );
    },
  }) as T;
}
