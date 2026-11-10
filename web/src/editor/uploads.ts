// Uploads from the slash menu, a paste or a drop, which become the blocks of `attachments.ts`.

import { Extension } from "@tiptap/core";
import { NodeSelection, Plugin, PluginKey, TextSelection } from "@tiptap/pm/state";
import type { EditorView } from "@tiptap/pm/view";

import { uploadAttachment } from "../lib/api";
import { describe } from "../lib/form.svelte";
import { notify } from "../lib/notice.svelte";
import { IMAGE_TYPES } from "./attachments";

interface UploadOptions {
  /** The page that uploads belong to. */
  page: string;
}

/** Uploads one file after the other, inserting each block where the selection is then. */
async function insertUploads(view: EditorView, page: string, files: File[]) {
  for (const file of files) {
    let uploaded;
    try {
      notify(`uploading ${file.name || "file"}…`);
      uploaded = await uploadAttachment(page, file);
    } catch (error) {
      notify(`${file.name || "file"}: ${describe(error)}`);
      continue;
    }
    if (view.isDestroyed || !view.editable) return;
    const { schema, tr } = view.state;
    const node = IMAGE_TYPES.includes(uploaded.content_type)
      ? schema.nodes.image.create({ id: uploaded.id, alt: null })
      : schema.nodes.file.create({ id: uploaded.id, name: uploaded.file_name });
    // After an inserted block the selection is that block: the next one goes after it, and is
    // selected in turn, instead of replacing it.
    const { selection } = view.state;
    if (selection instanceof NodeSelection) {
      tr.insert(selection.to, node);
      tr.setSelection(NodeSelection.create(tr.doc, selection.to));
    } else {
      tr.replaceSelectionWith(node);
    }
    view.dispatch(tr.scrollIntoView());
    notify(`uploaded ${uploaded.file_name}`);
  }
}

/** Uploads, configured with the page by `collab.ts`, as `PagePicker` is with the workspace. */
export const Uploads = Extension.create<UploadOptions>({
  name: "uploads",

  addOptions() {
    return { page: "" };
  },

  addCommands() {
    return {
      pickAttachment:
        (kind) =>
        ({ editor }) => {
          if (!editor.isEditable) return false;
          const input = document.createElement("input");
          input.type = "file";
          input.multiple = true;
          if (kind === "image") input.accept = IMAGE_TYPES.join(",");
          input.addEventListener("change", () => {
            void insertUploads(editor.view, this.options.page, [...(input.files ?? [])]);
          });
          input.click();
          return true;
        },
    };
  },

  addProseMirrorPlugins() {
    const page = () => this.options.page;
    const take = (view: EditorView, files: FileList | null | undefined) => {
      if (!view.editable || !files || files.length === 0) return false;
      void insertUploads(view, page(), [...files]);
      return true;
    };
    return [
      new Plugin({
        key: new PluginKey("uploads"),
        props: {
          handlePaste: (view, event) => take(view, event.clipboardData?.files),
          handleDrop: (view, event, _slice, moved) => {
            if (moved) return false;
            const files = event.dataTransfer?.files;
            if (!view.editable || !files?.length) return false;
            // The blocks go where the files were dropped.
            const at = view.posAtCoords({ left: event.clientX, top: event.clientY });
            if (at) {
              const { doc, tr } = view.state;
              view.dispatch(tr.setSelection(TextSelection.near(doc.resolve(at.pos))));
            }
            return take(view, files);
          },
        },
      }),
    ];
  },
});

