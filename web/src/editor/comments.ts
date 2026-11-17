// Comment highlights (ADR 0012): anchors are Yjs relative positions, resolved against the shared
// document and drawn as decorations, never written into it. An anchor that no longer spans any
// text is lost, and the panel shows its quote alone.

import { Extension } from "@tiptap/core";
import { type EditorState, Plugin, PluginKey } from "@tiptap/pm/state";
import { Decoration, DecorationSet, type EditorView } from "@tiptap/pm/view";
import {
  absolutePositionToRelativePosition,
  relativePositionToAbsolutePosition,
  ySyncPluginKey,
} from "@tiptap/y-tiptap";
import * as Y from "yjs";

import { type Anchor, fromBase64, type Highlight, quote, toBase64 } from "../lib/comments.svelte";

interface Shown {
  highlights: Highlight[];
  active: string | null;
}

interface State extends Shown {
  decorations: DecorationSet;
  /** Threads whose anchor spans text. */
  found: ReadonlySet<string>;
  /** The decorations were mapped through a local edit, and wait to be resolved again. */
  mapped: boolean;
}

interface Options {
  /** The set of threads whose anchor spans text changed. */
  onFound: (found: ReadonlySet<string>) => void;
  /** A highlight was clicked. */
  onActivate: (id: string) => void;
  /** Whether the selection holds text to comment on changed. */
  onSelection: (hasText: boolean) => void;
}

const key = new PluginKey<State>("comments");
/** A local edit maps the highlights at once, and resolves them again after this pause. */
const RESOLVE_AFTER_MS = 200;

type Sync = NonNullable<ReturnType<typeof ySyncPluginKey.getState>>;

function position(sync: Sync, encoded: string, size: number) {
  try {
    const relative = Y.decodeRelativePosition(fromBase64(encoded));
    const at = relativePositionToAbsolutePosition(sync.doc, sync.type, relative, sync.binding.mapping);
    return at !== null && at >= 0 && at <= size ? at : null;
  } catch {
    // Not a position this client can read: lost, like deleted text.
    return null;
  }
}

/** The text of a range, so a range over nothing but block boundaries counts as lost. */
const spansText = (state: EditorState, from: number, to: number) =>
  from < to && state.doc.textBetween(from, to, " ", " ").trim() !== "";

function resolve(state: EditorState, shown: Shown): Omit<State, "mapped"> {
  const sync = ySyncPluginKey.getState(state);
  const found = new Set<string>();
  const decorations: Decoration[] = [];
  if (sync?.binding) {
    const size = state.doc.content.size;
    for (const highlight of shown.highlights) {
      const from = position(sync, highlight.anchor.start, size);
      const to = position(sync, highlight.anchor.end, size);
      if (from === null || to === null || !spansText(state, from, to)) continue;
      found.add(highlight.id);
      if (!highlight.shown) continue;
      decorations.push(...draw(highlight, from, to, highlight.id === shown.active));
    }
  }
  return { ...shown, decorations: DecorationSet.create(state.doc, decorations), found };
}

function draw(highlight: Highlight, from: number, to: number, active: boolean) {
  const attributes: Record<string, string> = { class: "comment-mark", "data-comment": highlight.id };
  if (active) attributes["data-active"] = "true";
  const mark = Decoration.inline(from, to, attributes, { id: highlight.id });
  if (highlight.count === 0) return [mark];
  // An empty element whose `[n]` comes from CSS, so it never joins the text of the page.
  const counter = Decoration.widget(
    to,
    () => {
      const element = document.createElement("span");
      element.className = "comment-count";
      element.dataset.comment = highlight.id;
      element.dataset.count = String(highlight.count);
      element.contentEditable = "false";
      return element;
    },
    { id: highlight.id, side: -1, ignoreSelection: true, key: `${highlight.id}:${highlight.count}` },
  );
  return [mark, counter];
}

/** The ids of the highlights left after mapping, which a local deletion can empty. */
function foundAfterMapping(state: State, decorations: DecorationSet) {
  const left = new Set<string>();
  for (const decoration of decorations.find()) {
    if (decoration.from < decoration.to) left.add((decoration.spec as { id: string }).id);
  }
  // Resolved threads have no decoration: they keep their state until resolved again.
  for (const highlight of state.highlights) {
    if (!highlight.shown && state.found.has(highlight.id)) left.add(highlight.id);
  }
  return left;
}

const sameSet = (left: ReadonlySet<string>, right: ReadonlySet<string>) =>
  left.size === right.size && [...left].every((id) => right.has(id));

/** The selection as an anchor, or `null` when it holds no text. */
export function anchorOf(state: EditorState): Anchor | null {
  const { from, to, empty } = state.selection;
  if (empty) return null;
  const text = quote(state.doc.textBetween(from, to, " ", " "));
  const sync = ySyncPluginKey.getState(state);
  if (!text || !sync?.binding) return null;
  const encode = (at: number) =>
    toBase64(Y.encodeRelativePosition(absolutePositionToRelativePosition(at, sync.type, sync.binding.mapping)));
  return { start: encode(from), end: encode(to), text };
}

/** Shows `highlights` in the editor, `active` drawn apart, and answers those whose anchor spans text. */
export function showHighlights(view: EditorView, highlights: Highlight[], active: string | null) {
  view.dispatch(view.state.tr.setMeta(key, { highlights, active }));
  return key.getState(view.state)?.found ?? new Set<string>();
}

/** Scrolls the highlight of a thread into view. */
export function revealHighlight(view: EditorView, id: string) {
  const element = view.dom.querySelector(`.comment-mark[data-comment="${CSS.escape(id)}"]`);
  element?.scrollIntoView({ block: "center" });
}

export const CommentHighlights = Extension.create<Options>({
  name: "commentHighlights",

  addOptions() {
    return { onFound: () => {}, onActivate: () => {}, onSelection: () => {} };
  },

  addProseMirrorPlugins() {
    const options = this.options;
    return [
      new Plugin<State>({
        key,
        state: {
          init: () => ({
            highlights: [],
            active: null,
            decorations: DecorationSet.empty,
            found: new Set(),
            mapped: false,
          }),
          apply(tr, state, _old, next) {
            const shown = tr.getMeta(key) as Shown | undefined;
            if (shown) return { ...resolve(next, shown), mapped: false };
            if (!tr.docChanged) return state;
            // A change from Yjs, remote or undone, has updated the binding's mapping: resolve again.
            // A local edit reaches Yjs only after this, so its highlights are mapped for now.
            if (tr.getMeta(ySyncPluginKey)) return { ...resolve(next, state), mapped: false };
            const decorations = state.decorations.map(tr.mapping, tr.doc);
            return { ...state, decorations, found: foundAfterMapping(state, decorations), mapped: true };
          },
        },
        props: {
          decorations: (state) => key.getState(state)?.decorations,
          handleClick(_view, _pos, event) {
            const target = event.target instanceof Element ? event.target.closest("[data-comment]") : null;
            const id = target?.getAttribute("data-comment");
            if (id) options.onActivate(id);
            return false;
          },
        },
        view() {
          let found: ReadonlySet<string> = new Set();
          let hasText = false;
          let timer: ReturnType<typeof setTimeout> | null = null;
          return {
            update(view) {
              const state = key.getState(view.state)!;
              if (!sameSet(found, state.found)) {
                found = state.found;
                options.onFound(found);
              }
              const { from, to } = view.state.selection;
              const now = spansText(view.state, from, to);
              if (now !== hasText) {
                hasText = now;
                options.onSelection(now);
              }
              if (state.mapped && !timer) {
                timer = setTimeout(() => {
                  timer = null;
                  const current = key.getState(view.state);
                  if (current?.mapped && !view.isDestroyed) {
                    showHighlights(view, current.highlights, current.active);
                  }
                }, RESOLVE_AFTER_MS);
              }
            },
            destroy() {
              if (timer) clearTimeout(timer);
            },
          };
        },
      }),
    ];
  },
});
