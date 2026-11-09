// Callouts and the table of contents: block nodes of our own, each drawn by a node view.
// Toggles and tables come from TipTap's packages, set up in the registry.

import { type Editor, mergeAttributes, Node } from "@tiptap/core";
import { Details } from "@tiptap/extension-details";

import { headingNumbers } from "./source";

export const CALLOUT_KINDS = ["note", "warning", "danger"] as const;
export type CalloutKind = (typeof CALLOUT_KINDS)[number];

const isKind = (value: unknown): value is CalloutKind =>
  CALLOUT_KINDS.includes(value as CalloutKind);

/** Calls `apply` with the editor's mode now, and again whenever it may have changed. */
function followMode(editor: Editor, apply: (editable: boolean) => void) {
  const run = () => apply(editor.isEditable);
  run();
  // `setEditable` emits `update`.
  editor.on("update", run);
  return () => editor.off("update", run);
}

/**
 * A framed block of other blocks, with a kind (`note`, `warning`, `danger`) that writers pick
 * from a list at its top. Markdown has no callout: step 20 writes it as a GitHub alert.
 */
export const Callout = Node.create({
  name: "callout",
  group: "block",
  content: "block+",
  defining: true,

  addAttributes() {
    return {
      kind: {
        default: "note",
        parseHTML: (element) => (isKind(element.dataset.kind) ? element.dataset.kind : "note"),
        renderHTML: (attributes) => ({ "data-kind": attributes.kind }),
      },
    };
  },

  parseHTML() {
    return [{ tag: 'div[data-type="callout"]' }];
  },

  renderHTML({ HTMLAttributes }) {
    return ["div", mergeAttributes(HTMLAttributes, { "data-type": "callout" }), 0];
  },

  addNodeView() {
    return ({ node, editor, getPos }) => {
      let current = node;
      const dom = document.createElement("div");
      dom.dataset.type = "callout";
      dom.dataset.kind = current.attrs.kind;

      const head = document.createElement("div");
      head.className = "callout-head";
      head.contentEditable = "false";
      const select = document.createElement("select");
      select.setAttribute("aria-label", "callout kind");
      for (const kind of CALLOUT_KINDS) select.add(new Option(kind, kind));
      select.value = current.attrs.kind;
      head.append(select);

      const content = document.createElement("div");
      content.className = "callout-body";
      dom.append(head, content);

      select.addEventListener("change", () => {
        const pos = getPos();
        if (!editor.isEditable || typeof pos !== "number" || !isKind(select.value)) {
          select.value = current.attrs.kind;
          return;
        }
        const kind = select.value;
        editor
          .chain()
          .command(({ tr }) => {
            tr.setNodeMarkup(pos, undefined, { ...current.attrs, kind });
            return true;
          })
          .run();
      });
      const stop = followMode(editor, (editable) => (select.disabled = !editable));

      return {
        dom,
        contentDOM: content,
        // The list is a control of the view, not content.
        stopEvent: (event) => head.contains(event.target as globalThis.Node),
        ignoreMutation: (mutation) =>
          mutation.type !== "selection" && !content.contains(mutation.target),
        update: (updated) => {
          if (updated.type !== current.type) return false;
          current = updated;
          dom.dataset.kind = updated.attrs.kind;
          select.value = updated.attrs.kind;
          return true;
        },
        destroy: stop,
      };
    };
  },
});

/** A toggle: a summary line over blocks each tab opens or closes for itself, not for the others. */
export const Toggle = Details.configure({
  persist: false,
  renderToggleButton: ({ element, isOpen }) => {
    element.textContent = isOpen ? "▾" : "▸";
    element.className = "toggle-button";
    element.setAttribute("aria-label", isOpen ? "close" : "open");
    element.setAttribute("aria-expanded", String(isOpen));
    element.title = isOpen ? "close" : "open";
  },
});

/**
 * The headings of the page, numbered as they show, listed by the browser from the document:
 * the node holds nothing else, so it never goes stale.
 */
export const TableOfContents = Node.create({
  name: "tableOfContents",
  group: "block",
  atom: true,
  selectable: true,

  parseHTML() {
    return [{ tag: 'div[data-type="tableOfContents"]' }];
  },

  renderHTML({ HTMLAttributes }) {
    return ["div", mergeAttributes(HTMLAttributes, { "data-type": "tableOfContents" })];
  },

  addNodeView() {
    return ({ editor }) => {
      const dom = document.createElement("div");
      dom.dataset.type = "tableOfContents";
      dom.contentEditable = "false";
      const nav = document.createElement("nav");
      nav.setAttribute("aria-label", "table of contents");
      const label = document.createElement("div");
      label.className = "toc-label";
      label.textContent = "contents";
      const list = document.createElement("ol");
      nav.append(label, list);
      dom.append(nav);

      /** What the list shows, so an edit elsewhere does not redraw it. */
      let shown = "";
      const render = () => {
        const headings = headingNumbers(editor.state.doc).map(({ pos, node, label }) => ({
          pos,
          label,
          text: node.textContent,
        }));
        const key = JSON.stringify(headings.map(({ label, text }) => [label, text]));
        if (key === shown) return;
        shown = key;
        if (headings.length === 0) {
          const empty = document.createElement("li");
          empty.className = "toc-empty";
          empty.textContent = "no headings yet";
          list.replaceChildren(empty);
          return;
        }
        list.replaceChildren(
          ...headings.map(({ label, text }, index) => {
            const item = document.createElement("li");
            item.style.setProperty("--depth", String(label.split(".").length - 1));
            const link = document.createElement("a");
            link.href = "#";
            const number = document.createElement("span");
            number.className = "toc-number";
            number.textContent = label;
            link.append(number, " ", text || "untitled");
            link.addEventListener("click", (event) => {
              // Not an address: the router must not follow it.
              event.preventDefault();
              const target = headingNumbers(editor.state.doc)[index];
              if (!target) return;
              const element = editor.view.nodeDOM(target.pos);
              if (element instanceof HTMLElement) element.scrollIntoView({ block: "start" });
            });
            item.append(link);
            return item;
          }),
        );
      };
      render();
      editor.on("transaction", render);

      return {
        dom,
        ignoreMutation: () => true,
        stopEvent: (event) => nav.contains(event.target as globalThis.Node) && event.type !== "dragstart",
        destroy: () => editor.off("transaction", render),
      };
    };
  },
});
