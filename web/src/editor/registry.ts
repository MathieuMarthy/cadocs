// What the editor knows: its blocks, the slash commands that make them, and their shortcuts.
// The editor is built from this list alone, so a block is added here and nowhere else (ADR 0016).
// Node and mark names are TipTap's defaults. `cadocs-content` holds the same schema in Rust, and
// `npm run schema` writes this one where its test compares them (ADR 0008).

import { type AnyExtension, type ChainedCommands, type Editor, InputRule } from "@tiptap/core";
import { NodeSelection } from "@tiptap/pm/state";
import Blockquote from "@tiptap/extension-blockquote";
import CodeBlockLowlight from "@tiptap/extension-code-block-lowlight";
import Heading from "@tiptap/extension-heading";
import HorizontalRule from "@tiptap/extension-horizontal-rule";
import Link, { isAllowedUri } from "@tiptap/extension-link";
import { DetailsContent, DetailsSummary } from "@tiptap/extension-details";
import { BulletList, ListItem, OrderedList, TaskItem, TaskList } from "@tiptap/extension-list";
import { TableKit } from "@tiptap/extension-table";
import StarterKit from "@tiptap/starter-kit";

import { FileBlock, ImageBlock } from "./attachments";
import { Callout, PluginBlock, TableOfContents, Toggle } from "./blocks";
import { lowlight } from "./code";
import { CodePreview, InlineMath } from "./math";
import { Mention } from "./mention";
import { PageLink } from "./pagelink";
import { TableMenuOnRightClick, TableWithControls } from "./table";
import { MarkdownSource, quietInSource } from "./source";
import { type Key, t } from "../lib/i18n";

export interface SlashCommand {
  /** What the menu shows, in lowercase interface words: a getter, so it reads the language. */
  label: string;
  /** The Markdown shortcut that makes the same block, shown on the right. */
  hint?: string;
  /** Other words that find it. */
  keywords: string[];
  /** Applied once the typed `/query` is removed. */
  apply: (chain: ChainedCommands) => ChainedCommands;
  /** Where the command applies, if not everywhere: the menu leaves it out elsewhere. */
  when?: (editor: Editor) => boolean;
}

export interface Entry {
  name: string;
  extensions: AnyExtension[];
  commands?: SlashCommand[];
}

/** `[text](url)` becomes a link, if the address is one a link may hold. */
const markdownLink = Link.extend({
  addInputRules() {
    return [
      new InputRule({
        find: /\[([^\]\n]+)\]\((\S+)\)$/,
        handler: ({ state, range, match }) => {
          const [, text, href] = match;
          if (!text || !href || !isAllowedUri(href, this.options.protocols)) return null;
          const mark = this.type.create({ href });
          // The text typed next is not part of the link.
          state.tr.replaceWith(range.from, range.to, state.schema.text(text, [mark]));
          state.tr.removeStoredMark(this.type);
        },
      }),
    ];
  },
});

const inTable = (editor: Editor) => editor.isActive("table");

/** A command that changes the table under the caret. */
const tableCommand = (
  key: Key,
  keywords: string[],
  apply: SlashCommand["apply"],
): SlashCommand => ({
  get label() {
    return t(key);
  },
  keywords,
  apply,
  when: inTable,
});

const heading = (level: 1 | 2 | 3): SlashCommand => ({
  get label() {
    return t("slash.heading", { level });
  },
  hint: "#".repeat(level),
  keywords: [`h${level}`, "title"],
  apply: (chain) => chain.clearNodes().setHeading({ level }),
});

export const entries: Entry[] = [
  {
    // Paragraphs, text, marks, cursors and history. The blocks below are their own entries.
    name: "core",
    extensions: [
      StarterKit.configure({
        undoRedo: false,
        heading: false,
        blockquote: false,
        bulletList: false,
        orderedList: false,
        listItem: false,
        codeBlock: false,
        horizontalRule: false,
        link: false,
      }),
    ],
    commands: [
      {
        get label() {
          return t("slash.text");
        },
        keywords: ["paragraph", "plain"],
        apply: (chain) => chain.clearNodes().setParagraph(),
      },
    ],
  },
  {
    name: "heading",
    extensions: [quietInSource(Heading).configure({ levels: [1, 2, 3] })],
    commands: [heading(1), heading(2), heading(3)],
  },
  {
    name: "lists",
    extensions: [quietInSource(BulletList), quietInSource(OrderedList), ListItem],
    commands: [
      {
        get label() {
          return t("slash.bulletList");
        },
        hint: "-",
        keywords: ["unordered", "ul"],
        apply: (chain) => chain.clearNodes().toggleBulletList(),
      },
      {
        get label() {
          return t("slash.numberedList");
        },
        hint: "1.",
        keywords: ["ordered", "ol"],
        apply: (chain) => chain.clearNodes().toggleOrderedList(),
      },
    ],
  },
  {
    name: "tasks",
    extensions: [
      TaskList,
      quietInSource(TaskItem).configure({
        nested: true,
        a11y: {
          checkboxLabel: (node) => t("editor.taskCheckbox", { text: node.textContent || t("editor.taskEmpty") }),
        },
      }),
    ],
    commands: [
      {
        get label() {
          return t("slash.taskList");
        },
        hint: "[ ]",
        keywords: ["todo", "checkbox", "checklist"],
        apply: (chain) => chain.clearNodes().toggleTaskList(),
      },
    ],
  },
  {
    name: "code",
    extensions: [quietInSource(CodeBlockLowlight).configure({ lowlight, defaultLanguage: null })],
    commands: [
      {
        get label() {
          return t("slash.code");
        },
        hint: "```",
        keywords: ["snippet", "pre"],
        apply: (chain) => chain.clearNodes().setCodeBlock(),
      },
    ],
  },
  {
    // Formulas and diagrams: code blocks in `math` and `mermaid`, drawn below their source, and
    // the `inlineMath` node. Each library loads when the first one shows (`math.ts`).
    name: "math",
    extensions: [quietInSource(InlineMath), CodePreview],
    commands: [
      {
        get label() {
          return t("slash.mathBlock");
        },
        hint: "```math", // i18n-ignore
        keywords: ["latex", "katex", "equation", "formula", "display"],
        apply: (chain) => chain.clearNodes().setCodeBlock({ language: "math" }),
      },
      {
        get label() {
          return t("slash.inlineMath");
        },
        hint: "$",
        keywords: ["latex", "katex", "equation", "formula"],
        // The empty formula is selected, which opens its field.
        apply: (chain) =>
          chain.insertContent({ type: "inlineMath" }).command(({ tr }) => {
            const pos = tr.selection.from - 1;
            if (tr.doc.nodeAt(pos)?.type.name === "inlineMath") {
              tr.setSelection(NodeSelection.create(tr.doc, pos));
            }
            return true;
          }),
      },
      {
        get label() {
          return t("slash.diagram");
        },
        hint: "```mermaid", // i18n-ignore
        keywords: ["mermaid", "chart", "flowchart", "graph", "sequence"],
        apply: (chain) => chain.clearNodes().setCodeBlock({ language: "mermaid" }),
      },
    ],
  },
  {
    name: "quote",
    extensions: [quietInSource(Blockquote)],
    commands: [
      {
        get label() {
          return t("slash.quote");
        },
        hint: ">",
        keywords: ["blockquote", "citation"],
        apply: (chain) => chain.clearNodes().toggleBlockquote(),
      },
    ],
  },
  {
    name: "divider",
    extensions: [quietInSource(HorizontalRule)],
    commands: [
      {
        get label() {
          return t("slash.divider");
        },
        hint: "---",
        keywords: ["rule", "hr", "separator"],
        apply: (chain) => chain.setHorizontalRule(),
      },
    ],
  },
  {
    name: "callout",
    extensions: [Callout],
    commands: [
      {
        get label() {
          return t("slash.callout");
        },
        keywords: ["note", "warning", "danger", "admonition", "alert"],
        apply: (chain) => chain.clearNodes().wrapIn("callout"),
      },
    ],
  },
  {
    // Open or closed in each tab alone: `persist` is off, so opening writes nothing.
    name: "toggle",
    extensions: [Toggle, DetailsSummary, DetailsContent],
    commands: [
      {
        get label() {
          return t("slash.toggle");
        },
        keywords: ["details", "collapse", "fold"],
        apply: (chain) => chain.clearNodes().setDetails(),
      },
    ],
  },
  {
    // Writers drag a column's edge, stored as the cells' `colwidth`; they also get `+` buttons on
    // the edges and a menu on a cell's right click (`table.ts`). 80 is the cells' `min-width`.
    name: "table",
    extensions: [
      TableKit.configure({ table: { resizable: true, cellMinWidth: 80, View: TableWithControls } }),
      TableMenuOnRightClick,
    ],
    commands: [
      {
        get label() {
          return t("slash.table");
        },
        keywords: ["grid"],
        apply: (chain) => chain.insertTable({ rows: 3, cols: 3, withHeaderRow: true }),
      },
      tableCommand("table.addRowBelow", ["insert"], (chain) => chain.addRowAfter()),
      tableCommand("table.addRowAbove", ["insert"], (chain) => chain.addRowBefore()),
      tableCommand("table.addColumnRight", ["insert"], (chain) => chain.addColumnAfter()),
      tableCommand("table.addColumnLeft", ["insert"], (chain) => chain.addColumnBefore()),
      tableCommand("table.deleteRow", ["remove"], (chain) => chain.deleteRow()),
      tableCommand("table.deleteColumn", ["remove"], (chain) => chain.deleteColumn()),
      tableCommand("table.deleteTable", ["remove"], (chain) => chain.deleteTable()),
    ],
  },
  {
    // The picker that `[[` opens is added by `collab.ts`, which knows the workspace.
    name: "pageLink",
    extensions: [PageLink],
    commands: [
      {
        get label() {
          return t("slash.pageLink");
        },
        hint: "[[",
        keywords: ["page", "wiki"],
        apply: (chain) => chain.insertContent("[["),
      },
    ],
  },
  {
    // The picker that `@` opens is added by `collab.ts` and the comment composer, which know the
    // workspace.
    name: "mention",
    extensions: [Mention],
    commands: [
      {
        get label() {
          return t("slash.mention");
        },
        hint: "@",
        keywords: ["person", "user", "mention"],
        apply: (chain) => chain.insertContent("@"),
      },
    ],
  },
  {
    name: "tableOfContents",
    extensions: [TableOfContents],
    commands: [
      {
        get label() {
          return t("slash.contents");
        },
        keywords: ["toc", "contents", "outline"],
        apply: (chain) => chain.insertContent({ type: "tableOfContents" }),
      },
    ],
  },
  {
    // Uploads need the page, so `collab.ts` adds them with `Uploads`, as it adds `PagePicker`.
    name: "image",
    extensions: [ImageBlock],
    commands: [
      {
        get label() {
          return t("slash.image");
        },
        keywords: ["picture", "photo", "upload"],
        apply: (chain) => chain.pickAttachment("image"),
      },
    ],
  },
  {
    name: "file",
    extensions: [FileBlock],
    commands: [
      {
        get label() {
          return t("slash.file");
        },
        keywords: ["attachment", "upload", "pdf", "document"],
        apply: (chain) => chain.pickAttachment("file"),
      },
    ],
  },
  {
    // Blocks of content plugins, shown by their fallback text until plugins ship (ADR 0016).
    name: "plugin",
    extensions: [PluginBlock],
  },
  {
    name: "link",
    extensions: [
      markdownLink.configure({
        autolink: true,
        defaultProtocol: "https",
        HTMLAttributes: { rel: "noopener noreferrer nofollow", target: "_blank" },
      }),
    ],
  },
  {
    // The source of the block under the caret, edited as text (ADR 0018). Its block shortcuts
    // stay silent there, which is why the blocks above come through `quietInSource`.
    name: "source",
    extensions: [MarkdownSource],
  },
];

export const extensionsOf = (list: Entry[]) => list.flatMap((entry) => entry.extensions);

export const commandsOf = (list: Entry[]) => list.flatMap((entry) => entry.commands ?? []);

/** The commands whose label, one of its words or a keyword starts with the query, in registry order. */
export function findCommands(list: SlashCommand[], query: string) {
  const wanted = query.toLowerCase();
  return list.filter((command) =>
    [command.label, ...command.label.split(" "), ...command.keywords].some((word) =>
      word.startsWith(wanted),
    ),
  );
}
