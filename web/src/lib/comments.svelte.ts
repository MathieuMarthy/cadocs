// The comments of a page over REST (`docs/API.md`, Comments), and who may do what with them.

import { api } from "./api";
import { describe } from "./form.svelte";
import { collect } from "./nav.svelte";

/** Where a comment points: two encoded Yjs relative positions, in base64, and the quoted text (ADR 0012). */
export interface Anchor {
  start: string;
  end: string;
  text: string;
}

export interface CommentUser {
  id: string;
  name: string;
}

export interface Comment {
  id: string;
  page_id: string;
  parent_id: string | null;
  /** `null` once the account is deleted. */
  author: CommentUser | null;
  body: string;
  anchor: Anchor | null;
  resolved_at: number | null;
  resolved_by: CommentUser | null;
  created_at: number;
  updated_at: number;
}

export interface Thread extends Comment {
  replies: Comment[];
}

/** At most this many threads show, which no page should reach. */
const MAX_THREADS = 1000;

export const listThreads = async (page: string) =>
  (await collect<Thread>(`/pages/${encodeURIComponent(page)}/comments`, MAX_THREADS)).items;

export const createComment = (page: string, body: { body: string; parent_id?: string; anchor?: Anchor }) =>
  api<Comment>("POST", `/pages/${encodeURIComponent(page)}/comments`, body);

export const editComment = (id: string, body: string) =>
  api<Comment>("PATCH", `/comments/${encodeURIComponent(id)}`, { body });

export const deleteComment = (id: string) => api<void>("DELETE", `/comments/${encodeURIComponent(id)}`);

export const resolveThread = (id: string, resolved: boolean) =>
  api<Comment>("POST", `/comments/${encodeURIComponent(id)}/resolve`, { resolved });

/** Writing, replying, resolving and reopening need `commenter` (ADR 0009). */
export const canComment = (role: string) => role !== "viewer";

/** Editing or deleting needs to be the author, or a space admin for another's comment. */
export const canChange = (comment: Comment, role: string, user: string) =>
  role === "admin" || (canComment(role) && comment.author?.id === user);

/** The longest quote the server takes, in characters. */
const MAX_QUOTE = 1000;

/** The selected text as a quote: one line, cut on a whole character to what the server takes. */
export function quote(text: string) {
  const line = text.replace(/\s+/g, " ").trim();
  const characters = [...line];
  if (characters.length <= MAX_QUOTE) return line;
  return `${characters.slice(0, MAX_QUOTE - 1).join("")}…`;
}

export function toBase64(bytes: Uint8Array) {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary);
}

export function fromBase64(text: string) {
  const binary = atob(text);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) bytes[index] = binary.charCodeAt(index);
  return bytes;
}

/** A thread as the editor draws it. */
export interface Highlight {
  id: string;
  anchor: Anchor;
  /** Comments in the thread, shown as `[n]` in the margin, none for the one being written. */
  count: number;
  /** Open threads are highlighted. A resolved one is only resolved, so the panel knows it is found. */
  shown: boolean;
}

/** What the page's editor does for comments (`editor/comments.ts`). */
export interface CommentEditor {
  /** The selection as an anchor, or `null` when it holds no text. */
  anchor(): Anchor | null;
  /** Draws the highlights, and answers the threads whose anchor spans text. */
  showComments(highlights: Highlight[], active: string | null): ReadonlySet<string>;
  revealComment(id: string): void;
}

/** The id of the highlight of the comment being written. */
const DRAFT = "draft";

/** The threads of one page, what the panel shows of them, and what the editor draws. */
export class PageComments {
  threads = $state<Thread[]>([]);
  loaded = $state(false);
  problem = $state<string | null>(null);
  /** The thread picked, in the panel or by a click on its highlight. */
  active = $state<string | null>(null);
  /** The comment being written, on its anchor, or on the whole page for `null`. */
  draft = $state<{ anchor: Anchor | null } | null>(null);
  /** Threads whose anchor spans text, known once the editor holds the real document. */
  found = $state<ReadonlySet<string> | null>(null);
  /** The editor's selection holds text to comment on. */
  hasSelection = $state(false);
  /** Bumped by every read that lands, for what waits on a thread to arrive. */
  version = $state(0);

  readonly open = $derived(this.threads.filter((thread) => thread.resolved_at === null));
  readonly resolved = $derived(this.threads.filter((thread) => thread.resolved_at !== null));

  private editor: CommentEditor | null = null;
  private synced = false;
  /** Only the last read started may write, so a slow answer cannot undo a newer one. */
  private reads = 0;
  private timer: ReturnType<typeof setTimeout> | null = null;

  constructor(readonly page: string) {}

  /** Reads the threads again, once for a burst of events. */
  reload() {
    if (this.timer) return;
    this.timer = setTimeout(() => {
      this.timer = null;
      void this.load();
    }, 50);
  }

  async load() {
    const read = ++this.reads;
    try {
      const threads = await listThreads(this.page);
      if (read !== this.reads) return;
      // A picked thread that someone resolves stops being picked, so its passage goes with it.
      const was = this.threads.find((thread) => thread.id === this.active);
      const now = threads.find((thread) => thread.id === this.active);
      if (was?.resolved_at === null && now && now.resolved_at !== null) this.active = null;
      this.threads = threads;
      this.version += 1;
      this.loaded = true;
      this.problem = null;
      this.draw();
    } catch (error) {
      if (read === this.reads) this.problem = describe(error);
    }
  }

  /** An anchor that spans no text any more: its quote shows alone. */
  lost(thread: Thread) {
    return thread.anchor !== null && this.found !== null && !this.found.has(thread.id);
  }

  attach(editor: CommentEditor | null) {
    this.editor = editor;
    this.synced = false;
    this.found = null;
    this.draw();
  }

  /** The editor synced: until then its document is empty, and every anchor would look lost. */
  editorSynced() {
    this.synced = true;
    this.draw();
  }

  /** The editor resolved the anchors again after an edit. */
  foundChanged(found: ReadonlySet<string>) {
    if (this.synced) this.found = found;
  }

  activate(id: string | null) {
    // The highlight of the comment being written belongs to no thread.
    if (id === DRAFT) return;
    this.active = id;
    this.draw();
  }

  /** Picks a thread from the panel and scrolls the page to its passage. */
  reveal(id: string) {
    this.activate(id);
    this.editor?.revealComment(id);
  }

  /** Starts a comment on the selection, taken now, before the composer takes the focus. */
  start() {
    this.draft = { anchor: this.editor?.anchor() ?? null };
    this.draw();
  }

  cancel() {
    this.draft = null;
    this.draw();
  }

  stop() {
    this.reads += 1;
    if (this.timer) clearTimeout(this.timer);
    this.timer = null;
    this.editor = null;
  }

  private draw() {
    if (!this.editor) return;
    const highlights: Highlight[] = [];
    for (const thread of this.threads) {
      if (!thread.anchor) continue;
      const count = 1 + thread.replies.length;
      // A resolved thread shows its passage only while it is picked, so its quote can show it.
      const shown = thread.resolved_at === null || thread.id === this.active;
      highlights.push({ id: thread.id, anchor: thread.anchor, count, shown });
    }
    if (this.draft?.anchor) highlights.push({ id: DRAFT, anchor: this.draft.anchor, count: 0, shown: true });
    const found = this.editor.showComments(highlights, this.active);
    if (this.synced) this.found = found;
  }
}
