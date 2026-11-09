import "../page.css";

import { Editor } from "@tiptap/core";
import Collaboration from "@tiptap/extension-collaboration";
import CollaborationCaret from "@tiptap/extension-collaboration-caret";
import { WebsocketProvider } from "y-websocket";
import * as Y from "yjs";

import { freeSlot, renderAvatars, renderCaret, renderSelection, type User } from "./presence";
import { commandsOf, entries, extensionsOf } from "./registry";
import { SlashMenu } from "./slash";
import { bindTitle } from "./title";

const CLOSE_MALFORMED = 1002;
const CLOSE_TOO_LARGE = 1009;
const CLOSE_STORAGE_FAILING = 1013;
/** The closes where reconnecting cannot help. See `Close codes` in `docs/API.md`. */
const CLOSES_FINAL = [4401, 4403, 4404];
const CLOSE_ROLE_CHANGED = 4409;
/** Failed connections in a row before asking REST what is wrong. */
const FAILURES_BEFORE_CHECK = 3;

interface Elements {
  editor: HTMLElement;
  title: HTMLTextAreaElement;
  status: HTMLElement;
  presence: HTMLElement;
}

interface Options {
  /** Who is writing, as the others see them. */
  user: Omit<User, "slot">;
  /** Whether the role allows writing, until `setEditable` says otherwise. */
  editable: boolean;
}

interface Callbacks {
  onName: (title: string) => void;
  onFinalClose: (code: number) => void;
  onRepeatedFailure: () => void;
  /** The server closed with `4409`: the role must be read again. */
  onRoleChanged: () => void;
  /** The page holds content this schema cannot show, and this tab stopped syncing. */
  onContentError: () => void;
}

export function openPage(page: string, elements: Elements, options: Options, callbacks: Callbacks) {
  const { title, status, presence } = elements;
  const ydoc = new Y.Doc();
  const scheme = location.protocol === "https:" ? "wss" : "ws";
  // Tabs go through the server like any other client, instead of a BroadcastChannel shortcut.
  const provider = new WebsocketProvider(`${scheme}://${location.host}/ws/pages`, page, ydoc, {
    disableBc: true,
    // By default it gives up on all of 4400 to 4499, which includes 4408 and 4409.
    shouldReconnect: (event) => !CLOSES_FINAL.includes(event.code),
  });
  const { awareness } = provider;

  const user: User = { ...options.user, slot: 1 };
  let editable = options.editable;
  /** Set once content did not fit: this tab then neither syncs nor writes again. */
  let stopped = false;
  let problem: string | null = null;
  let failures = 0;

  const editor = new Editor({
    element: elements.editor,
    editable,
    // Content this schema lacks pauses the editor instead of being deleted for everyone (ADR 0016).
    enableContentCheck: true,
    onContentError: ({ disableCollaboration }) => {
      if (stopped) return;
      stopped = true;
      problem = "stopped";
      provider.destroy();
      disableCollaboration();
      applyMode();
      renderStatus();
      callbacks.onContentError();
    },
    extensions: [
      ...extensionsOf(entries),
      SlashMenu.configure({ commands: commandsOf(entries) }),
      Collaboration.configure({ document: ydoc }),
      CollaborationCaret.configure({
        provider,
        user: { ...user },
        render: renderCaret,
        selectionRender: renderSelection,
      }),
    ],
    editorProps: { attributes: { "aria-label": "page content" } },
  });

  const unbindTitle = bindTitle(title, ydoc.getText("title"), (text) => {
    document.title = text ? `${text} · cadocs` : "cadocs";
    callbacks.onName(text);
  });
  function applyMode() {
    const writable = editable && !stopped;
    title.readOnly = !writable;
    if (editor.isEditable !== writable) editor.setEditable(writable);
  }
  applyMode();

  title.addEventListener("keydown", (event) => {
    if (event.key === "Enter") {
      event.preventDefault();
      editor.commands.focus("start");
    }
  });

  function renderStatus() {
    if (problem) {
      status.dataset.state = "error";
      status.textContent = problem;
    } else if (provider.synced) {
      status.dataset.state = "synced";
      const mode = editable ? "" : " · read only";
      status.textContent = `synced · ${awareness.getStates().size} online${mode}`;
    } else {
      status.dataset.state = "pending";
      status.textContent = provider.wsconnected ? "syncing…" : "connecting…";
    }
  }

  provider.on("connection-close", (event) => {
    if (event && CLOSES_FINAL.includes(event.code)) {
      callbacks.onFinalClose(event.code);
      return;
    }
    if (stopped) return;
    if (event?.code === CLOSE_ROLE_CHANGED) {
      // Read-only at once, so nothing typed before the role is read stays in this tab alone.
      // The provider reconnects in the new mode, and the role read turns writing back on.
      editable = false;
      applyMode();
      problem = "role changed, reconnecting";
      renderStatus();
      callbacks.onRoleChanged();
      return;
    }
    switch (event?.code) {
      case CLOSE_MALFORMED:
      case CLOSE_TOO_LARGE:
        problem = "rejected by the server";
        break;
      case CLOSE_STORAGE_FAILING:
        problem = "storage failing, retrying";
        break;
      default:
        problem = "offline, retrying";
    }
    renderStatus();
    failures += 1;
    if (failures % FAILURES_BEFORE_CHECK === 0) callbacks.onRepeatedFailure();
  });

  provider.on("sync", (synced) => {
    if (synced) {
      problem = null;
      failures = 0;
      editor.commands.updateUser({ ...user, slot: freeSlot(awareness) });
    }
    renderStatus();
  });

  awareness.on("change", () => {
    renderAvatars(presence, awareness);
    renderStatus();
  });
  renderStatus();

  // A block left written as its source would stay so for everyone: read it before leaving.
  const readSource = () => {
    if (editor.isEditable) editor.commands.readSource();
  };
  addEventListener("pagehide", readSource);

  return {
    setEditable(value: boolean) {
      editable = value;
      applyMode();
      renderStatus();
    },
    destroy() {
      readSource();
      removeEventListener("pagehide", readSource);
      unbindTitle();
      provider.destroy();
      editor.destroy();
      ydoc.destroy();
    },
  };
}
