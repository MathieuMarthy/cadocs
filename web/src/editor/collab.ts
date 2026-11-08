import "../page.css";

import { Editor } from "@tiptap/core";
import Collaboration from "@tiptap/extension-collaboration";
import CollaborationCaret from "@tiptap/extension-collaboration-caret";
import StarterKit from "@tiptap/starter-kit";
import { WebsocketProvider } from "y-websocket";
import * as Y from "yjs";

import { freeSlot, newUser, renderAvatars, renderCaret, renderSelection } from "./presence";
import { bindTitle } from "./title";

const CLOSE_MALFORMED = 1002;
const CLOSE_TOO_LARGE = 1009;
const CLOSE_STORAGE_FAILING = 1013;
/** The closes where reconnecting cannot help. See `Close codes` in `docs/API.md`. */
const CLOSES_FINAL = [4401, 4403, 4404];
/** Failed connections in a row before asking REST what is wrong. */
const FAILURES_BEFORE_CHECK = 3;

interface Elements {
  editor: HTMLElement;
  title: HTMLTextAreaElement;
  status: HTMLElement;
  presence: HTMLElement;
}

interface Callbacks {
  onName: (title: string) => void;
  onFinalClose: (code: number) => void;
  onRepeatedFailure: () => void;
}

export function openPage(page: string, elements: Elements, callbacks: Callbacks) {
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

  const user = newUser();
  const editor = new Editor({
    element: elements.editor,
    extensions: [
      StarterKit.configure({ undoRedo: false }),
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
  title.addEventListener("keydown", (event) => {
    if (event.key === "Enter") {
      event.preventDefault();
      editor.commands.focus("start");
    }
  });

  let problem: string | null = null;
  let failures = 0;

  function renderStatus() {
    if (problem) {
      status.dataset.state = "error";
      status.textContent = problem;
    } else if (provider.synced) {
      status.dataset.state = "synced";
      status.textContent = `synced · ${awareness.getStates().size} online`;
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

  return {
    destroy() {
      unbindTitle();
      provider.destroy();
      editor.destroy();
      ydoc.destroy();
    },
  };
}
