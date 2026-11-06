import "./fonts";
import "./tokens.css";
import "./style.css";

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

function element<T extends HTMLElement>(id: string) {
  return document.getElementById(id) as T;
}

/** The prototype has no page list: `?page=<id>` picks the page, and login happens through REST. */
const page = new URLSearchParams(location.search).get("page");
if (!page) {
  element("status").textContent = "no page: add ?page=<id> to the address";
  throw new Error("no page to open");
}
element("doc-name").textContent = page;

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
  element: element("editor"),
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

const title = element<HTMLInputElement>("title");
bindTitle(title, ydoc.getText("title"), (text) => {
  document.title = text ? `${text} · cadocs` : "cadocs";
});
title.addEventListener("keydown", (event) => {
  if (event.key === "Enter") {
    event.preventDefault();
    editor.commands.focus("start");
  }
});

const status = element("status");
let problem: string | null = null;

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
});

provider.on("sync", (synced) => {
  if (synced) {
    problem = null;
    editor.commands.updateUser({ ...user, slot: freeSlot(awareness) });
  }
  renderStatus();
});

const presence = element("presence");
awareness.on("change", () => {
  renderAvatars(presence, awareness);
  renderStatus();
});
renderStatus();
