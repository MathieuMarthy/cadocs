// Mentions of users: an inline node that holds a user id and shows that member's name, and `@`
// that picks a member of the page's workspace by name.

import { type Editor, Extension, mergeAttributes, Node, type Range } from "@tiptap/core";
import { PluginKey } from "@tiptap/pm/state";
import Suggestion, { type SuggestionProps } from "@tiptap/suggestion";

import { t } from "../lib/i18n";
import { nameOf, people } from "../lib/people";
import { CommandMenu, type MenuItem } from "./menu";

/** The picker shows this many matches at most. */
const SHOWN = 8;
/** How often a mention shown before the workspace is known looks for it again. */
const WAIT_MS = 250;

type Workspace = () => string | null;

/** The workspace the picker of this editor knows, which names come from. */
const workspaceOf = (editor: Editor): Workspace =>
  (editor.storage as unknown as Record<string, { workspace?: Workspace } | undefined>).mentionPicker
    ?.workspace ?? (() => null);

/** The workspace, once known, unless `live` turns false first. */
async function known(workspace: Workspace, live: () => boolean) {
  for (;;) {
    const id = workspace();
    if (id || !live()) return id;
    await new Promise((done) => setTimeout(done, WAIT_MS));
  }
}

/**
 * A user named in the content. Only the id is stored: the name is read from the workspace's
 * members when shown, so it follows renames, and someone who left shows as nobody in particular.
 */
export const Mention = Node.create({
  name: "mention",
  group: "inline",
  inline: true,
  atom: true,
  selectable: true,

  addAttributes() {
    return {
      id: {
        default: null,
        parseHTML: (element) => element.dataset.id ?? null,
        renderHTML: (attributes) => ({ "data-id": attributes.id }),
      },
    };
  },

  parseHTML() {
    return [{ tag: 'span[data-type="mention"][data-id]' }];
  },

  renderHTML({ HTMLAttributes }) {
    return ["span", mergeAttributes(HTMLAttributes, { "data-type": "mention", class: "mention" }), "@"];
  },

  renderText() {
    return "@";
  },

  addNodeView() {
    return ({ node, editor }) => {
      const span = document.createElement("span");
      const id = String(node.attrs.id ?? "");
      span.className = "mention";
      span.dataset.type = "mention";
      span.textContent = "@…";
      let live = true;
      void known(workspaceOf(editor), () => live)
        .then((workspace) => (workspace ? nameOf(workspace, id) : null))
        .then(
          (name) => {
            if (!live) return;
            span.textContent = `@${name ?? t("editor.mentionMissing")}`;
            span.classList.toggle("mention-missing", name === null);
          },
          () => {
            if (live) span.textContent = `@${t("editor.mentionUnread")}`;
          },
        );
      return {
        dom: span,
        ignoreMutation: () => true,
        destroy: () => {
          live = false;
        },
      };
    };
  },
});

interface Candidate extends MenuItem {
  id: string;
}

/**
 * `@` opens a list of the workspace's members, filtered by name. Added beside the node by those
 * who know the workspace: the page editor and the comment composer.
 */
export const MentionPicker = Extension.create<{ workspace: Workspace }, { workspace: Workspace }>({
  name: "mentionPicker",

  addOptions() {
    return { workspace: () => null };
  },

  addStorage() {
    return { workspace: this.options.workspace };
  },

  addProseMirrorPlugins() {
    return [
      Suggestion<Candidate, Candidate>({
        editor: this.editor,
        pluginKey: new PluginKey("mentionPicker"),
        char: "@",
        allowSpaces: true,
        floatingUi: { strategy: "fixed" },
        allow: ({ editor, state, range }) =>
          editor.isEditable && !state.doc.resolve(range.from).parent.type.spec.code,
        items: async ({ query }) => {
          const workspace = this.options.workspace();
          if (!workspace) return [];
          const wanted = query.trim().toLowerCase();
          const rows = await people(workspace).catch(() => []);
          return rows
            .filter((row) => row.name.toLowerCase().includes(wanted))
            .slice(0, SHOWN)
            .map((row) => ({ id: row.id, label: row.name }));
        },
        command: ({ editor, range, props }: { editor: SuggestionProps<Candidate>["editor"]; range: Range; props: Candidate }) => {
          editor
            .chain()
            .focus()
            .insertContentAt(range, [
              { type: "mention", attrs: { id: props.id } },
              { type: "text", text: " " },
            ])
            .run();
        },
        render: () => {
          let menu: CommandMenu<Candidate> | null = null;
          let unmount: (() => void) | null = null;
          return {
            onStart(props) {
              menu = new CommandMenu(props, t("editor.people"));
              unmount = props.mount(menu.element);
            },
            onUpdate(props) {
              menu?.update(props);
            },
            onKeyDown({ event }) {
              return menu?.key(event) ?? false;
            },
            onExit() {
              unmount?.();
              unmount = null;
              menu = null;
            },
          };
        },
      }),
    ];
  },
});
