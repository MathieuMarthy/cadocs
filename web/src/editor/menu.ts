// The command menu of the design system, drawn without a framework like the presence:
// the slash menu and the page picker show their suggestions in it.

import type { SuggestionProps } from "@tiptap/suggestion";

/** What a menu row shows: a label, and a hint on the right. */
export interface MenuItem {
  label: string;
  hint?: string;
}

let menus = 0;

/**
 * The open menu: a list box the editor drives, since focus stays in the text.
 * `name` labels the list and its header, as in `commands · 4`.
 */
export class CommandMenu<T extends MenuItem> {
  readonly element = document.createElement("div");
  private readonly header = document.createElement("div");
  private readonly list = document.createElement("div");
  private readonly id = `menu-${++menus}`;
  private props: SuggestionProps<T, T>;
  private active = 0;

  constructor(
    props: SuggestionProps<T, T>,
    private readonly name: string,
  ) {
    this.props = props;
    this.element.className = "command-menu";
    this.header.className = "command-menu-header";
    this.list.setAttribute("role", "listbox");
    this.list.setAttribute("aria-label", name);
    this.element.append(this.header, this.list);
    // A press on the menu must not take focus from the text, or the suggestion would end.
    this.element.addEventListener("mousedown", (event) => event.preventDefault());
    this.render();
  }

  update(props: SuggestionProps<T, T>) {
    this.props = props;
    this.active = 0;
    this.render();
  }

  key(event: KeyboardEvent) {
    const count = this.props.items.length;
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      // Nothing to pick: the arrows move the caret, as they would without the menu.
      if (count === 0) return false;
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
    this.header.textContent = `${this.name} · ${items.length}`;
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
