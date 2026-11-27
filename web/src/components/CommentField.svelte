<script lang="ts">
  import { Editor } from "@tiptap/core";
  import StarterKit from "@tiptap/starter-kit";
  import { onMount } from "svelte";

  import { Mention, MentionPicker } from "../editor/mention";
  import { PageLink, PagePicker } from "../editor/pagelink";
  import { bodyOf, contentOf, type Json } from "../lib/mentions";

  /**
   * The text of a comment: a small editor of plain lines, where `@` mentions a member of the
   * workspace and `[[` a page, kept as `value` in the body's syntax (`lib/mentions.ts`).
   */
  let {
    value = $bindable(""),
    label,
    workspace,
    short = false,
  }: {
    value?: string;
    /** The field's accessible name. */
    label: string;
    /** The page's workspace, whose members and pages the pickers offer. */
    workspace: string | null;
    /** Two lines high instead of three, for a reply. */
    short?: boolean;
  } = $props();

  let element: HTMLDivElement;

  onMount(() => {
    const editor = new Editor({
      element,
      // Plain lines: no marks, no blocks, no rule that turns `**` or `#` into either.
      extensions: [
        StarterKit.configure({
          blockquote: false,
          bold: false,
          bulletList: false,
          code: false,
          codeBlock: false,
          dropcursor: false,
          gapcursor: false,
          heading: false,
          horizontalRule: false,
          italic: false,
          link: false,
          listItem: false,
          listKeymap: false,
          orderedList: false,
          strike: false,
          trailingNode: false,
          underline: false,
        }),
        Mention,
        PageLink.configure({ newTab: true }),
        MentionPicker.configure({ workspace: () => workspace }),
        PagePicker.configure({ workspace: () => workspace }),
      ],
      content: contentOf(value),
      editorProps: {
        attributes: {
          role: "textbox",
          "aria-multiline": "true",
          "aria-label": label,
          class: short ? "comment-field short" : "comment-field", // i18n-ignore
        },
      },
      onUpdate: ({ editor }) => {
        value = bodyOf(editor.getJSON() as Json);
      },
    });
    return () => editor.destroy();
  });
</script>

<div bind:this={element} class="comment-editor"></div>
