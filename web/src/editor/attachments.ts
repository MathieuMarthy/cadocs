// Images and files: two blocks that hold an attachment's id. The bytes stay on the server
// (ADR 0005): the document only stores the id, and the name or alternative text. The uploads that
// make them are in `uploads.ts`, which needs the page.

import { mergeAttributes, Node } from "@tiptap/core";

import { attachmentUrl } from "../lib/api";

/** The types the server shows in the page. Anything else, SVG included, is a file to download. */
export const IMAGE_TYPES = ["image/avif", "image/gif", "image/jpeg", "image/png", "image/webp"];

const idAttribute = {
  default: null,
  parseHTML: (element: HTMLElement) => element.dataset.id ?? null,
  renderHTML: (attributes: Record<string, unknown>) => ({ "data-id": attributes.id }),
};

export const ImageBlock = Node.create({
  name: "image",
  group: "block",
  atom: true,
  selectable: true,
  draggable: true,

  addAttributes() {
    return {
      id: idAttribute,
      alt: {
        default: null,
        parseHTML: (element) => element.querySelector("img")?.getAttribute("alt") ?? null,
        renderHTML: () => ({}),
      },
    };
  },

  parseHTML() {
    return [{ tag: 'figure[data-type="image"][data-id]' }];
  },

  renderHTML({ node, HTMLAttributes }) {
    return [
      "figure",
      mergeAttributes(HTMLAttributes, { "data-type": "image" }),
      ["img", { src: attachmentUrl(String(node.attrs.id ?? "")), alt: node.attrs.alt ?? "" }],
    ];
  },

  addNodeView() {
    return ({ node }) => {
      const dom = document.createElement("figure");
      dom.dataset.type = "image";
      dom.className = "image-block";
      dom.contentEditable = "false";
      const image = document.createElement("img");
      image.src = attachmentUrl(String(node.attrs.id ?? ""));
      image.alt = String(node.attrs.alt ?? "");
      image.draggable = false;
      // A file deleted since, or one this reader cannot open, shows as a placeholder.
      image.addEventListener("error", () => {
        dom.classList.add("attachment-missing");
        dom.textContent = "image not found";
      });
      dom.append(image);
      return { dom, ignoreMutation: () => true };
    };
  },
});

export const FileBlock = Node.create({
  name: "file",
  group: "block",
  atom: true,
  selectable: true,
  draggable: true,

  addAttributes() {
    return {
      id: idAttribute,
      name: {
        default: null,
        parseHTML: (element) => element.textContent ?? null,
        renderHTML: () => ({}),
      },
    };
  },

  parseHTML() {
    return [{ tag: 'a[data-type="file"][data-id]' }];
  },

  renderHTML({ node, HTMLAttributes }) {
    const href = attachmentUrl(String(node.attrs.id ?? ""));
    return [
      "a",
      mergeAttributes(HTMLAttributes, { "data-type": "file", href, download: "" }),
      String(node.attrs.name ?? "file"),
    ];
  },

  addNodeView() {
    return ({ node }) => {
      const dom = document.createElement("div");
      dom.dataset.type = "file";
      dom.className = "file-block";
      dom.contentEditable = "false";
      const link = document.createElement("a");
      link.href = attachmentUrl(String(node.attrs.id ?? ""));
      link.download = String(node.attrs.name ?? "");
      // A label, not an icon: icons stand for verbs (`docs/DESIGN_SYSTEM.md`).
      const label = document.createElement("span");
      label.className = "file-label";
      label.textContent = "file";
      const name = document.createElement("span");
      name.className = "file-name";
      name.textContent = String(node.attrs.name || "file");
      // Not stored in the document: the server knows it, so it is read with a `HEAD`.
      const size = document.createElement("span");
      size.className = "file-size";
      link.append(label, name, size);
      let live = true;
      void fileSize(link.href).then((bytes) => {
        if (!live) return;
        if (bytes === null) dom.classList.add("attachment-missing");
        size.textContent = bytes === null ? "not found" : formatSize(bytes);
      });
      dom.append(link);
      return {
        dom,
        // The browser downloads it: ProseMirror must not take the press for a selection.
        stopEvent: (event) => link.contains(event.target as globalThis.Node) && event.type !== "dragstart",
        ignoreMutation: () => true,
        destroy: () => {
          live = false;
        },
      };
    };
  },
});

declare module "@tiptap/core" {
  interface Commands<ReturnType> {
    attachments: {
      /** Opens the browser's file picker, and inserts what is picked once it is uploaded. */
      pickAttachment: (kind: "image" | "file") => ReturnType;
    };
  }
}

/** The size the server gives, `null` for a file it does not serve to this reader. */
async function fileSize(href: string): Promise<number | null> {
  try {
    // `no-store`: a cached copy would answer `304`, which carries no length.
    const response = await fetch(href, { method: "HEAD", cache: "no-store" });
    const length = response.headers.get("content-length");
    return response.ok && length !== null ? Number(length) : null;
  } catch {
    return null;
  }
}

/** `13 B`, `4.2 KB`, `12 MB`: one decimal below 10, none above. */
export function formatSize(bytes: number) {
  const units = ["B", "KB", "MB", "GB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  const shown = unit === 0 || value >= 10 ? Math.round(value) : Math.round(value * 10) / 10;
  return `${shown} ${units[unit]}`;
}
