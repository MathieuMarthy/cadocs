// The slash menu: `/` opens the registry's commands, typing filters them, arrows and Enter pick.
// It is the command menu of the design system, drawn without a framework like the presence.

import { Extension, type Range } from "@tiptap/core";
import { PluginKey } from "@tiptap/pm/state";
import Suggestion, { type SuggestionProps } from "@tiptap/suggestion";

import { CommandMenu } from "./menu";
import { findCommands, type SlashCommand } from "./registry";

type Props = SuggestionProps<SlashCommand, SlashCommand>;

export const SlashMenu = Extension.create<{ commands: SlashCommand[] }>({
  name: "slashMenu",

  addOptions() {
    return { commands: [] };
  },

  addProseMirrorPlugins() {
    const { commands } = this.options;
    // Some commands only apply where the caret is, as those of a table.
    const find = (editor: Props["editor"], query: string) =>
      findCommands(
        commands.filter((command) => !command.when || command.when(editor)),
        query,
      );
    return [
      Suggestion<SlashCommand, SlashCommand>({
        editor: this.editor,
        pluginKey: new PluginKey("slashMenu"),
        char: "/",
        // Labels have spaces (`add row below`), but a space that matches nothing ends the menu,
        // so a slash in a sentence does not keep it open.
        allowSpaces: true,
        floatingUi: { strategy: "fixed" },
        // Not in code, where a slash is text, nor for a reader.
        allow: ({ editor, state, range }) => {
          if (!editor.isEditable || state.doc.resolve(range.from).parent.type.spec.code) return false;
          const query = state.doc.textBetween(range.from + 1, range.to);
          return !query.includes(" ") || find(editor, query).length > 0;
        },
        items: ({ query, editor }) => find(editor, query),
        command: ({ editor, range, props }: { editor: Props["editor"]; range: Range; props: SlashCommand }) => {
          // Two transactions: prosemirror-tables commands read positions from the document as it
          // was before the chain, so after the query is removed in the same chain they land in
          // the wrong rows (a column added twice). The undo history groups the two.
          editor.chain().focus().deleteRange(range).run();
          props.apply(editor.chain().focus()).run();
        },
        render: () => {
          let menu: CommandMenu<SlashCommand> | null = null;
          let unmount: (() => void) | null = null;
          return {
            onStart(props) {
              menu = new CommandMenu(props, "commands");
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
