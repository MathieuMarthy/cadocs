// What the editor knows: its blocks, the slash commands that make them, and their shortcuts.
// The editor is built from this list alone, so a block is added here and nowhere else (ADR 0016).
// Node and mark names are TipTap's defaults. `cadocs-content` holds the same schema in Rust, and
// `npm run schema` writes this one where its test compares them (ADR 0008).

import { type AnyExtension, type ChainedCommands, type Editor, InputRule } from "@tiptap/core";
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
import { PageLink } from "./pagelink";
import { TableMenuOnRightClick, TableWithControls } from "./table";
import { MarkdownSource, quietInSource } from "./source";

export interface SlashCommand {
  /** What the menu shows, in lowercase interface words. */
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
  label: string,
  keywords: string[],
  apply: SlashCommand["apply"],
): SlashCommand => ({ label, keywords, apply, when: inTable });

const heading = (level: 1 | 2 | 3): SlashCommand => ({
  label: `heading ${level}`,
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
        label: "text",
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
        label: "bullet list",
        hint: "-",
        keywords: ["unordered", "ul"],
        apply: (chain) => chain.clearNodes().toggleBulletList(),
      },
      {
        label: "numbered list",
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
        a11y: { checkboxLabel: (node) => `task: ${node.textContent || "empty"}` },
      }),
    ],
    commands: [
      {
        label: "task list",
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
        label: "code",
        hint: "```",
        keywords: ["snippet", "pre"],
        apply: (chain) => chain.clearNodes().setCodeBlock(),
      },
    ],
  },
  {
    name: "quote",
    extensions: [quietInSource(Blockquote)],
    commands: [
      {
        label: "quote",
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
        label: "divider",
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
        label: "callout",
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
        label: "toggle",
        keywords: ["details", "collapse", "fold"],
        apply: (chain) => chain.clearNodes().setDetails(),
      },
    ],
  },
  {
    // Column widths are not stored: `resizable` would write them into the document. Writers also
    // get `+` buttons on the edges and a menu on a cell's right click (`table.ts`).
    name: "table",
    extensions: [
      TableKit.configure({ table: { resizable: false, View: TableWithControls } }),
      TableMenuOnRightClick,
    ],
    commands: [
      {
        label: "table",
        keywords: ["grid"],
        apply: (chain) => chain.insertTable({ rows: 3, cols: 3, withHeaderRow: true }),
      },
      tableCommand("add row below", ["insert"], (chain) => chain.addRowAfter()),
      tableCommand("add row above", ["insert"], (chain) => chain.addRowBefore()),
      tableCommand("add column right", ["insert"], (chain) => chain.addColumnAfter()),
      tableCommand("add column left", ["insert"], (chain) => chain.addColumnBefore()),
      tableCommand("delete row", ["remove"], (chain) => chain.deleteRow()),
      tableCommand("delete column", ["remove"], (chain) => chain.deleteColumn()),
      tableCommand("delete table", ["remove"], (chain) => chain.deleteTable()),
    ],
  },
  {
    // The picker that `[[` opens is added by `collab.ts`, which knows the workspace.
    name: "pageLink",
    extensions: [PageLink],
    commands: [
      {
        label: "link to page",
        hint: "[[",
        keywords: ["page", "mention", "wiki"],
        apply: (chain) => chain.insertContent("[["),
      },
    ],
  },
  {
    name: "tableOfContents",
    extensions: [TableOfContents],
    commands: [
      {
        label: "table of contents",
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
        label: "image",
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
        label: "file",
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
