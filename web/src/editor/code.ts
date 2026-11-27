// Syntax highlighting for code blocks. A short list of languages keeps the editor chunk small:
// a block in another language shows as plain code, and a block with none gets the closest guess.

import { Extension } from "@tiptap/core";
import { Plugin } from "@tiptap/pm/state";
import type { EditorView } from "@tiptap/pm/view";
import bash from "highlight.js/lib/languages/bash";
import c from "highlight.js/lib/languages/c";
import css from "highlight.js/lib/languages/css";
import diff from "highlight.js/lib/languages/diff";
import dockerfile from "highlight.js/lib/languages/dockerfile";
import go from "highlight.js/lib/languages/go";
import ini from "highlight.js/lib/languages/ini";
import java from "highlight.js/lib/languages/java";
import javascript from "highlight.js/lib/languages/javascript";
import json from "highlight.js/lib/languages/json";
import markdown from "highlight.js/lib/languages/markdown";
import python from "highlight.js/lib/languages/python";
import rust from "highlight.js/lib/languages/rust";
import shell from "highlight.js/lib/languages/shell";
import sql from "highlight.js/lib/languages/sql";
import typescript from "highlight.js/lib/languages/typescript";
import xml from "highlight.js/lib/languages/xml";
import yaml from "highlight.js/lib/languages/yaml";
import { createLowlight } from "lowlight";

import { inSource } from "./source";

export const lowlight = createLowlight({
  bash,
  c,
  css,
  diff,
  dockerfile,
  go,
  ini,
  java,
  javascript,
  json,
  markdown,
  python,
  rust,
  shell,
  sql,
  typescript,
  xml,
  yaml,
});

const INDENT = "  ";
const OUTDENT = /^(?:\t| {1,2})/;

/** Whether the whole selection is in one code block of an editable view. */
function inCode(view: EditorView) {
  const { $from, $to } = view.state.selection;
  return view.editable && !!$from.parent.type.spec.code && $from.sameParent($to);
}

/**
 * Tab indents every line the selection touches in a code block, and Shift-Tab outdents them.
 * Escape first lets Tab leave the editor, so the keyboard is never trapped.
 */
export const CodeIndent = Extension.create({
  name: "codeIndent",
  // Before the list keymaps, which would sink an item holding the code, and after the source's.
  priority: 999,

  addProseMirrorPlugins() {
    let leaving = false;
    return [
      new Plugin({
        props: {
          handleDOMEvents: {
            // Seen before every key handler, so that no list or table keymap takes the Tab out.
            keydown(view, event) {
              if (["Shift", "Control", "Alt", "Meta"].includes(event.key)) return false;
              const escaped = leaving;
              leaving = event.key === "Escape" && inCode(view);
              const plain = !event.ctrlKey && !event.metaKey && !event.altKey;
              // True skips ProseMirror's handlers and leaves the key to the browser, which moves focus.
              return escaped && event.key === "Tab" && plain && inCode(view);
            },
            mousedown() {
              leaving = false;
              return false;
            },
          },
          handleKeyDown(view, event) {
            if (event.key !== "Tab" || event.ctrlKey || event.metaKey || event.altKey || !inCode(view)) return false;
            const { state } = view;
            const { $from, from, to, empty } = state.selection;
            const start = $from.start();
            const text = $from.parent.textContent;
            const head = from - start;
            const first = head === 0 ? 0 : text.lastIndexOf("\n", head - 1) + 1;
            const starts = [first];
            // A selection ending at the start of a line leaves that line alone.
            let at = text.indexOf("\n", first);
            while (at !== -1 && at + 1 < to - start) {
              starts.push(at + 1);
              at = text.indexOf("\n", at + 1);
            }
            // A fence written out stays as it is, or the block would no longer read as code.
            const fence = first === 0 && inSource(state, from);
            if (fence) starts.shift();
            const tr = state.tr;
            if (empty && !event.shiftKey) {
              if (!fence) tr.insertText(INDENT);
            } else {
              // The last line first, so that the positions before it hold.
              for (const line of starts.reverse()) {
                if (!event.shiftKey) tr.insertText(INDENT, start + line);
                else {
                  const found = OUTDENT.exec(text.slice(line));
                  if (found) tr.delete(start + line, start + line + found[0].length);
                }
              }
            }
            if (tr.docChanged) view.dispatch(tr.scrollIntoView());
            return true;
          },
        },
      }),
    ];
  },
});
