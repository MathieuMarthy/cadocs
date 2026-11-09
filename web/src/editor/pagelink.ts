// Links to pages: an inline node that holds a page id and shows that page's title as the reader
// may see it, and `[[` that picks a page by its title.

import { Extension, mergeAttributes, Node, type Range } from "@tiptap/core";
import { PluginKey } from "@tiptap/pm/state";
import Suggestion, { type SuggestionProps } from "@tiptap/suggestion";

import { api, ApiError, type Page } from "../lib/api";
import { CommandMenu, type MenuItem } from "./menu";

/** The title a link shows, `null` for a page this reader cannot open. Kept while a page is open. */
const titles = new Map<string, Promise<string | null>>();

/** Forgets the titles, so the next page shows renames made meanwhile. */
export function forgetTitles() {
  titles.clear();
}

function titleOf(id: string) {
  let title = titles.get(id);
  if (!title) {
    title = api<{ title: string }>("GET", `/pages/${encodeURIComponent(id)}`).then(
      (page) => page.title || "untitled",
      (error) => {
        // Unreadable and missing look the same, as they do over REST: nothing is told of it.
        if (error instanceof ApiError && (error.status === 404 || error.status === 403)) return null;
        // A passing failure: the next view asks again, unless the map was cleared meanwhile.
        if (titles.get(id) === title) titles.delete(id);
        throw error;
      },
    );
    titles.set(id, title);
  }
  return title;
}

/**
 * A link to another page of the instance. Only the id is stored: the title is read when shown,
 * so it follows renames and is never shown to someone who cannot read the page.
 */
export const PageLink = Node.create({
  name: "pageLink",
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
    return [{ tag: 'a[data-type="pageLink"][data-id]' }];
  },

  renderHTML({ node, HTMLAttributes }) {
    const href = `/?page=${encodeURIComponent(node.attrs.id ?? "")}`;
    return ["a", mergeAttributes(HTMLAttributes, { "data-type": "pageLink", href })];
  },

  addNodeView() {
    return ({ node }) => {
      const link = document.createElement("a");
      const id = String(node.attrs.id ?? "");
      link.className = "page-link";
      link.dataset.type = "pageLink";
      link.href = `/?page=${encodeURIComponent(id)}`;
      link.textContent = "…";
      let live = true;
      titleOf(id).then(
        (title) => {
          if (!live) return;
          link.textContent = title ?? "page not found";
          link.classList.toggle("page-link-missing", title === null);
        },
        () => {
          if (live) link.textContent = "page";
        },
      );
      return {
        dom: link,
        // The router opens it in this tab. The press is kept from ProseMirror, whose
        // `handleClick` runs on mouseup, or the link mark's handler would open a new tab as well.
        stopEvent: (event) => ["mousedown", "mouseup", "click"].includes(event.type),
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

/** Candidates are read again after this long, so a page made meanwhile shows. */
const CANDIDATES_MS = 10_000;
/** The picker shows this many matches at most. */
const SHOWN = 8;
/** Pages read for the picker, until search (step 18) answers instead. */
const READ = 1000;

interface PageRow {
  id: string;
  title: string;
}

/**
 * `[[` opens a list of the workspace's pages that the reader can open, filtered by title,
 * the last changed first. It reads the recent pages, which are every live page of the readable
 * spaces, up to `READ`.
 */
export const PagePicker = Extension.create<{ workspace: () => string | null }>({
  name: "pagePicker",

  addOptions() {
    return { workspace: () => null };
  },

  addProseMirrorPlugins() {
    let cached: { workspace: string; at: number; rows: Promise<PageRow[]> } | null = null;
    const candidates = (workspace: string) => {
      if (!cached || cached.workspace !== workspace || Date.now() - cached.at > CANDIDATES_MS) {
        const entry = { workspace, at: Date.now(), rows: Promise.resolve<PageRow[]>([]) };
        entry.rows = readPages(workspace).catch(() => {
          if (cached === entry) cached = null;
          return [];
        });
        cached = entry;
      }
      return cached.rows;
    };

    return [
      Suggestion<Candidate, Candidate>({
        editor: this.editor,
        pluginKey: new PluginKey("pagePicker"),
        char: "[[",
        allowSpaces: true,
        floatingUi: { strategy: "fixed" },
        allow: ({ editor, state, range }) =>
          editor.isEditable && !state.doc.resolve(range.from).parent.type.spec.code,
        items: async ({ query }) => {
          const workspace = this.options.workspace();
          if (!workspace) return [];
          const wanted = query.trim().toLowerCase();
          const rows = await candidates(workspace);
          return rows
            .filter((row) => (row.title || "untitled").toLowerCase().includes(wanted))
            .slice(0, SHOWN)
            .map((row) => ({ id: row.id, label: row.title || "untitled" }));
        },
        command: ({ editor, range, props }: { editor: SuggestionProps<Candidate>["editor"]; range: Range; props: Candidate }) => {
          editor
            .chain()
            .focus()
            .insertContentAt(range, [
              { type: "pageLink", attrs: { id: props.id } },
              { type: "text", text: " " },
            ])
            .run();
        },
        render: () => {
          let menu: CommandMenu<Candidate> | null = null;
          let unmount: (() => void) | null = null;
          return {
            onStart(props) {
              menu = new CommandMenu(props, "pages");
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

async function readPages(workspace: string) {
  const rows: PageRow[] = [];
  let cursor: string | null = null;
  do {
    const query = new URLSearchParams({ limit: "100" });
    if (cursor) query.set("cursor", cursor);
    const page: Page<PageRow> = await api("GET", `/workspaces/${workspace}/recent?${query}`);
    rows.push(...page.items);
    cursor = page.next_cursor;
  } while (cursor && rows.length < READ);
  return rows;
}
