// Math and diagrams. A formula is LaTeX, inline as the `inlineMath` node, or a code block in
// `math`; a diagram is a code block in `mermaid`. Their source is what is stored, and every other
// renderer shows it (`cadocs-content`). KaTeX and Mermaid are large, so each loads on the first
// formula or diagram shown, never with the editor.

import { Extension, InputRule, mergeAttributes, Node } from "@tiptap/core";
import type { Node as PmNode } from "@tiptap/pm/model";
import { NodeSelection, Plugin, PluginKey, TextSelection } from "@tiptap/pm/state";
import { Decoration, DecorationSet } from "@tiptap/pm/view";

import { t } from "../lib/i18n";

type Katex = typeof import("katex").default;
type Mermaid = typeof import("mermaid").default;

let katexLoad: Promise<Katex> | undefined;
let mermaidLoad: Promise<Mermaid> | undefined;
let diagrams = 0;
/** How each formula shown opens its field, for Enter. */
const openers = new WeakMap<object, () => void>();

const katex = () =>
  (katexLoad ??= Promise.all([import("katex"), import("katex/dist/katex.min.css")]).then(
    ([module]) => module.default,
  ));

const dark = () => {
  const chosen = document.documentElement.dataset.theme;
  return chosen ? chosen === "dark" : matchMedia("(prefers-color-scheme: dark)").matches; // i18n-ignore
};

// shortcut: the diagram theme is read once, so a theme switch shows on the next page load.
const mermaid = () =>
  (mermaidLoad ??= import("mermaid").then(({ default: module }) => {
    module.initialize({
      startOnLoad: false,
      securityLevel: "strict",
      suppressErrorRendering: true,
      theme: dark() ? "dark" : "neutral",
    });
    return module;
  }));

/** Draws `latex` into `into`, which keeps the source as text until KaTeX has loaded. */
function renderMath(latex: string, into: HTMLElement, displayMode: boolean) {
  into.textContent = latex;
  katex().then(
    (module) => {
      // Unless a field for the source took its place meanwhile.
      if (latex && into.textContent === latex) module.render(latex, into, { displayMode, throwOnError: false });
    },
    () => {},
  );
}

/** How long a diagram waits for typing to pause: each edit replaces the box, which then drops out. */
const DIAGRAM_PAUSE_MS = 300;

async function renderDiagram(source: string, into: HTMLElement) {
  if (!source.trim()) return;
  await new Promise((done) => setTimeout(done, DIAGRAM_PAUSE_MS));
  if (!into.isConnected) return;
  try {
    const { svg } = await (await mermaid()).render(`diagram-${++diagrams}`, source);
    into.innerHTML = svg;
  } catch {
    // Readers then see the source as well (`page.css`).
    into.classList.add("error");
    into.textContent = t("editor.diagramError");
  }
}

/** A formula in the text, edited in a field of its own once clicked. */
export const InlineMath = Node.create({
  name: "inlineMath",
  group: "inline",
  inline: true,
  atom: true,
  selectable: true,

  addAttributes() {
    return {
      latex: {
        default: "",
        parseHTML: (element) => element.dataset.latex ?? "",
        renderHTML: (attributes) => ({ "data-latex": attributes.latex }),
      },
    };
  },

  parseHTML() {
    return [{ tag: 'span[data-type="inlineMath"]' }];
  },

  renderHTML({ HTMLAttributes, node }) {
    return ["span", mergeAttributes(HTMLAttributes, { "data-type": "inlineMath" }), node.attrs.latex];
  },

  renderText({ node }) {
    return node.attrs.latex;
  },

  /** `$x^2$` becomes a formula, as Markdown reads it: no space inside the dollars. */
  addInputRules() {
    return [
      new InputRule({
        find: /(?<![$\\])\$([^\s$](?:[^$]*[^\s$\\])?)\$$/,
        handler: ({ state, range, match }) => {
          state.tr.replaceWith(range.from, range.to, this.type.create({ latex: match[1] }));
        },
      }),
    ];
  },

  addKeyboardShortcuts() {
    return {
      Enter: ({ editor }) => {
        const { selection } = editor.state;
        if (!(selection instanceof NodeSelection) || selection.node.type !== this.type) return false;
        const dom = editor.view.nodeDOM(selection.from);
        if (dom) openers.get(dom)?.();
        return true;
      },
    };
  },

  addNodeView() {
    return ({ node, editor, getPos }) => {
      const dom = document.createElement("span");
      dom.dataset.type = "inlineMath";
      renderMath(node.attrs.latex, dom, false);
      let field: HTMLInputElement | null = null;

      /**
       * Closes the field, storing `latex` unless it is `null`, and puts the caret after: back in
       * the editor if `refocus`, else where the focus went. An empty formula is removed.
       */
      const close = (latex: string | null, refocus: boolean) => {
        // A view rebuilt meanwhile blurs its removed field, during ProseMirror's own update.
        if (!field || !dom.isConnected) return;
        field = null;
        const pos = getPos();
        if (pos === undefined) return;
        const value = latex ?? node.attrs.latex;
        const { tr } = editor.state;
        if (value.trim()) tr.setNodeMarkup(pos, undefined, { latex: value });
        else tr.delete(pos, pos + 1);
        tr.setSelection(TextSelection.create(tr.doc, value.trim() ? pos + 1 : pos));
        editor.view.dispatch(tr);
        // At once: TipTap's `focus()` waits a frame, and keys pressed meanwhile would be lost.
        if (refocus) editor.view.focus();
      };

      const open = () => {
        if (!editor.isEditable || field) return;
        const input = document.createElement("input");
        input.value = node.attrs.latex;
        input.ariaLabel = t("editor.formula");
        input.spellcheck = false;
        input.addEventListener("keydown", (event) => {
          if (event.key === "Enter") close(input.value, true);
          else if (event.key === "Escape") close(null, true);
          else return;
          event.preventDefault();
        });
        input.addEventListener("blur", () => close(input.value, false));
        field = input;
        dom.replaceChildren(input);
        input.focus();
      };
      // A click opens the field itself: ProseMirror leaves the pointer on a formula alone.
      dom.addEventListener("click", open);
      openers.set(dom, open);

      return {
        dom,
        ignoreMutation: () => true,
        stopEvent: (event) => event.target === field || (editor.isEditable && event.type.startsWith("mouse")),
        // The arrows select a formula on their way past it, and Enter opens it: only an empty
        // one, just inserted by the slash command, opens its field at once.
        selectNode: () => {
          dom.classList.add("ProseMirror-selectednode");
          if (!node.attrs.latex) open();
        },
        deselectNode: () => dom.classList.remove("ProseMirror-selectednode"),
        destroy: () => {
          field = null;
        },
      };
    };
  },
});

/** The language of a code block that renders below its source. */
const RENDERED: Record<string, (source: string, into: HTMLElement) => void> = {
  math: (source, into) => renderMath(source, into, true),
  mermaid: (source, into) => void renderDiagram(source, into),
};

function previews(doc: PmNode) {
  const widgets: Decoration[] = [];
  doc.descendants((node, pos) => {
    if (node.isTextblock) {
      const render = node.type.name === "codeBlock" && RENDERED[node.attrs.language];
      if (render) {
        const source = node.textContent;
        widgets.push(
          Decoration.widget(
            pos + node.nodeSize,
            () => {
              const box = document.createElement("div");
              box.className = "code-preview";
              box.contentEditable = "false";
              render(source, box);
              return box;
            },
            // The same source keeps its drawing: only an edit draws again.
            { key: `${node.attrs.language}:${source}`, side: -1, ignoreSelection: true },
          ),
        );
      }
      return false;
    }
    return true;
  });
  return DecorationSet.create(doc, widgets);
}

/** Draws each `math` and `mermaid` code block below its source. */
export const CodePreview = Extension.create({
  name: "codePreview",

  addProseMirrorPlugins() {
    return [
      new Plugin<DecorationSet>({
        key: new PluginKey("codePreview"),
        state: {
          init: (_, state) => previews(state.doc),
          apply: (tr, set) => (tr.docChanged ? previews(tr.doc) : set),
        },
        props: {
          decorations(state) {
            return this.getState(state);
          },
        },
      }),
    ];
  },
});
