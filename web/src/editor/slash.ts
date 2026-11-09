// The slash menu: `/` opens the registry's commands, typing filters them, arrows and Enter pick.
// It is the command menu of the design system, drawn without a framework like the presence.

import { Extension, type Range } from "@tiptap/core";
import { PluginKey } from "@tiptap/pm/state";
import Suggestion, { type SuggestionProps } from "@tiptap/suggestion";

import { findCommands, type SlashCommand } from "./registry";

type Props = SuggestionProps<SlashCommand, SlashCommand>;

let menus = 0;

/** The open menu: a list box the editor drives, since focus stays in the text. */
class CommandMenu {
  readonly element = document.createElement("div");
  private readonly header = document.createElement("div");
  private readonly list = document.createElement("div");
  private readonly id = `commands-${++menus}`;
  private props: Props;
  private active = 0;

  constructor(props: Props) {
    this.props = props;
    this.element.className = "command-menu";
    this.header.className = "command-menu-header";
    this.list.setAttribute("role", "listbox");
    this.list.setAttribute("aria-label", "commands");
    this.element.append(this.header, this.list);
    // A press on the menu must not take focus from the text, or the suggestion would end.
    this.element.addEventListener("mousedown", (event) => event.preventDefault());
    this.render();
  }

  update(props: Props) {
    this.props = props;
    this.active = 0;
    this.render();
  }

  key(event: KeyboardEvent) {
    const count = this.props.items.length;
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      if (count === 0) return true;
      const step = event.key === "ArrowDown" ? 1 : count - 1;
      this.active = (this.active + step) % count;
      this.render();
      return true;
    }
    if (event.key === "Enter" || event.key === "Tab") {
      const item = this.props.items[this.active];
      if (!item) return false;
      this.props.command(item);
      return true;
    }
    return false;
  }

  private render() {
    const { items } = this.props;
    this.header.textContent = `commands · ${items.length}`;
    if (items.length === 0) {
      const empty = document.createElement("div");
      empty.className = "command-menu-empty";
      empty.textContent = "no match";
      this.list.replaceChildren(empty);
      return;
    }
    this.list.replaceChildren(
      ...items.map((item, index) => {
        const option = document.createElement("div");
        option.className = "command-menu-item";
        option.id = `${this.id}-${index}`;
        option.setAttribute("role", "option");
        option.setAttribute("aria-selected", String(index === this.active));
        const label = document.createElement("span");
        label.textContent = item.label;
        option.append(label);
        if (item.hint) {
          const hint = document.createElement("span");
          hint.className = "command-menu-hint";
          hint.textContent = item.hint;
          option.append(hint);
        }
        option.addEventListener("click", () => this.props.command(item));
        option.addEventListener("mousemove", () => {
          if (this.active === index) return;
          this.active = index;
          this.render();
        });
        return option;
      }),
    );
    this.list.querySelector('[aria-selected="true"]')?.scrollIntoView({ block: "nearest" });
  }
}

export const SlashMenu = Extension.create<{ commands: SlashCommand[] }>({
  name: "slashMenu",

  addOptions() {
    return { commands: [] };
  },

  addProseMirrorPlugins() {
    const { commands } = this.options;
    return [
      Suggestion<SlashCommand, SlashCommand>({
        editor: this.editor,
        pluginKey: new PluginKey("slashMenu"),
        char: "/",
        floatingUi: { strategy: "fixed" },
        // Not in code, where a slash is text, nor for a reader.
        allow: ({ editor, state, range }) =>
          editor.isEditable && !state.doc.resolve(range.from).parent.type.spec.code,
        items: ({ query }) => findCommands(commands, query),
        command: ({ editor, range, props }: { editor: Props["editor"]; range: Range; props: SlashCommand }) => {
          props.apply(editor.chain().focus().deleteRange(range)).run();
        },
        render: () => {
          let menu: CommandMenu | null = null;
          let unmount: (() => void) | null = null;
          return {
            onStart(props) {
              menu = new CommandMenu(props);
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
