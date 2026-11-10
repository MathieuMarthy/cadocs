// Mouse ways to change a table, for writers: `+` buttons on its bottom and right edges, which add
// a row or a column at the end, and a menu on a cell's right click. The slash commands remain.

import { Extension } from "@tiptap/core";
import { Plugin, PluginKey, TextSelection } from "@tiptap/pm/state";
import {
  addColumnAfter,
  addColumnBefore,
  addRowAfter,
  addRowBefore,
  CellSelection,
  deleteColumn,
  deleteRow,
  deleteTable,
} from "@tiptap/pm/tables";
import type { EditorState, Transaction } from "@tiptap/pm/state";
import type { EditorView, ViewMutationRecord } from "@tiptap/pm/view";
import type { Node as PMNode } from "@tiptap/pm/model";
import { TableView } from "@tiptap/extension-table";

import { type IconName, iconMarkup } from "../components/icons";

type Command = (state: EditorState, dispatch?: (tr: Transaction) => void) => boolean;

/** Puts the caret in `cell`, then runs `command` there. */
function runIn(view: EditorView, cell: Element, command: Command) {
  const pos = view.posAtDOM(cell, 0);
  view.dispatch(view.state.tr.setSelection(TextSelection.near(view.state.doc.resolve(pos))));
  command(view.state, view.dispatch);
}

/** Puts the caret in `cell` and gives the editor focus. */
function caretIn(view: EditorView, cell: Element | null | undefined) {
  if (cell) {
    const pos = view.posAtDOM(cell, 0);
    view.dispatch(view.state.tr.setSelection(TextSelection.near(view.state.doc.resolve(pos))));
  }
  view.focus();
}

const cellsOf = (row: Element | null | undefined) => [...(row?.querySelectorAll(":scope > th, :scope > td") ?? [])];

/** The table's view, with its two edge buttons. They show on hover, to writers only (`page.css`). */
export class TableWithControls extends TableView {
  private readonly controls: HTMLElement;

  constructor(node: PMNode, cellMinWidth: number, view: EditorView, attributes?: Record<string, unknown>) {
    super(node, cellMinWidth, view, attributes);
    const body = this.contentDOM;
    const button = (label: string, className: string, act: () => void) => {
      const element = document.createElement("button");
      element.type = "button";
      element.className = `table-edge ${className}`;
      element.setAttribute("aria-label", label);
      element.title = label;
      element.tabIndex = -1;
      element.innerHTML = iconMarkup("plus");
      // A press must not move the caret first, or the cell it lands in would be the one used.
      element.addEventListener("mousedown", (event) => event.preventDefault());
      element.addEventListener("click", () => {
        if (view.editable) act();
      });
      return element;
    };
    const lastRow = () => body.lastElementChild;
    const addRow = button("add row", "table-edge-row", () => {
      const last = cellsOf(lastRow()).at(0);
      if (!last) return;
      runIn(view, last, addRowAfter);
      caretIn(view, cellsOf(lastRow()).at(0));
    });
    const addColumn = button("add column", "table-edge-column", () => {
      const last = cellsOf(body.firstElementChild).at(-1);
      if (!last) return;
      runIn(view, last, addColumnAfter);
      caretIn(view, cellsOf(body.firstElementChild).at(-1));
    });
    this.controls = document.createElement("div");
    this.controls.className = "table-edges";
    this.controls.contentEditable = "false";
    this.controls.append(addRow, addColumn);
    // The wrapper scrolls sideways: the buttons sit in a frame as wide as the table, so they
    // follow its edges instead of the wrapper's visible box.
    const frame = document.createElement("div");
    frame.className = "table-frame";
    frame.append(this.table, this.controls);
    this.dom.append(frame);
  }

  stopEvent(event: Event) {
    return this.controls.contains(event.target as Node);
  }

  ignoreMutation(mutation: ViewMutationRecord) {
    if (this.controls.contains(mutation.target)) return true;
    return super.ignoreMutation(mutation);
  }
}

interface Action {
  label: string;
  icon: IconName;
  command: Command;
  /** Below a rule. */
  rule?: boolean;
  danger?: boolean;
}

const ACTIONS: Action[] = [
  { label: "add row above", icon: "plus", command: addRowBefore },
  { label: "add row below", icon: "plus", command: addRowAfter },
  { label: "add column left", icon: "plus", command: addColumnBefore },
  { label: "add column right", icon: "plus", command: addColumnAfter },
  { label: "delete row", icon: "x", command: deleteRow, rule: true },
  { label: "delete column", icon: "x", command: deleteColumn },
  { label: "delete table", icon: "x", command: deleteTable, danger: true },
];

/**
 * The action menu of the design system, built in plain DOM as the editor's other menus are.
 * It opens at the pointer, holds focus, and takes the arrows, Home, End, Enter and Escape.
 */
class TableMenu {
  private readonly element = document.createElement("div");
  private readonly items: HTMLButtonElement[];

  constructor(
    private readonly view: EditorView,
    x: number,
    y: number,
    private readonly onClose: () => void,
  ) {
    const menu = this.element;
    menu.className = "menu";
    menu.setAttribute("role", "menu");
    menu.setAttribute("aria-label", "table");
    menu.tabIndex = -1;
    menu.popover = "manual";
    this.items = ACTIONS.map((action) => {
      if (action.rule) {
        const rule = document.createElement("div");
        rule.className = "menu-rule";
        rule.setAttribute("role", "separator");
        menu.append(rule);
      }
      const item = document.createElement("button");
      item.type = "button";
      item.className = action.danger ? "menu-item danger" : "menu-item";
      item.setAttribute("role", "menuitem");
      item.tabIndex = -1;
      item.innerHTML = iconMarkup(action.icon);
      item.append(action.label);
      item.addEventListener("click", () => this.choose(action));
      item.addEventListener("pointermove", () => item.focus());
      menu.append(item);
      return item;
    });
    menu.addEventListener("keydown", (event) => this.key(event));
    document.body.append(menu);
    menu.showPopover();
    // At the pointer, kept inside the window.
    const left = Math.min(x, window.innerWidth - menu.offsetWidth);
    const top = y + menu.offsetHeight <= window.innerHeight ? y : Math.max(0, y - menu.offsetHeight);
    menu.style.left = `${Math.max(0, left)}px`;
    menu.style.top = `${top}px`;
    menu.focus();
    addEventListener("pointerdown", this.outside, true);
    addEventListener("scroll", this.outside, true);
    addEventListener("resize", this.outside);
  }

  private choose(action: Action) {
    this.close();
    if (this.view.editable) action.command(this.view.state, this.view.dispatch);
    this.view.focus();
  }

  private key(event: KeyboardEvent) {
    const at = this.items.indexOf(document.activeElement as HTMLButtonElement);
    const move = (index: number) => this.items[(index + this.items.length) % this.items.length]?.focus();
    switch (event.key) {
      case "ArrowDown":
        move(at + 1);
        break;
      case "ArrowUp":
        move(at < 0 ? -1 : at - 1);
        break;
      case "Home":
        move(0);
        break;
      case "End":
        move(-1);
        break;
      case "Escape":
      case "Tab":
        this.close();
        this.view.focus();
        break;
      default:
        return;
    }
    event.preventDefault();
  }

  /** A press outside, or the page scrolling under it, closes the menu. */
  private readonly outside = (event: Event) => {
    if (event.target instanceof Node && this.element.contains(event.target)) return;
    this.close();
  };

  close() {
    if (!this.element.isConnected) return;
    removeEventListener("pointerdown", this.outside, true);
    removeEventListener("scroll", this.outside, true);
    removeEventListener("resize", this.outside);
    this.element.remove();
    this.onClose();
  }
}

/** A right click on a cell of a writer's table opens the table menu instead of the browser's. */
export const TableMenuOnRightClick = Extension.create({
  name: "tableMenu",

  addProseMirrorPlugins() {
    let open: TableMenu | null = null;
    return [
      new Plugin({
        key: new PluginKey("tableMenu"),
        props: {
          handleDOMEvents: {
            contextmenu: (view, event) => {
              if (!view.editable || !(event.target instanceof Element)) return false;
              const cell = event.target.closest("td, th");
              if (!cell || !view.dom.contains(cell)) return false;
              event.preventDefault();
              // A selection of several cells stays, so a command applies to all of them.
              const inSelection =
                view.state.selection instanceof CellSelection &&
                cell.classList.contains("selectedCell");
              if (!inSelection) runIn(view, cell, () => true);
              open?.close();
              open = new TableMenu(view, event.clientX, event.clientY, () => (open = null));
              return true;
            },
          },
        },
        view: () => ({
          destroy: () => open?.close(),
        }),
      }),
    ];
  },
});
