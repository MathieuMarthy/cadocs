// Comment bodies: plain text, never HTML nor Markdown, where `<user:ID>` mentions a member and
// `<page:ID>` a page. The server reads the same grammar (`cadocs-content`'s `comment.rs`) to
// record whom a comment mentions. A body holds ids only: names and titles are read when shown,
// with the reader's rights.

export type Piece =
  | { kind: "text"; text: string }
  | { kind: "user"; id: string }
  | { kind: "page"; id: string };

const TOKEN = /<(user|page):([A-Za-z0-9_-]{1,64})>/g;
/** An id a token can hold. */
const ID = /^[A-Za-z0-9_-]{1,64}$/;

/** The body cut into text and mentions, in order. */
export function pieces(body: string): Piece[] {
  const found: Piece[] = [];
  let at = 0;
  for (const match of body.matchAll(TOKEN)) {
    if (match.index > at) found.push({ kind: "text", text: body.slice(at, match.index) });
    found.push({ kind: match[1] as "user" | "page", id: match[2] });
    at = match.index + match[0].length;
  }
  if (at < body.length) found.push({ kind: "text", text: body.slice(at) });
  return found;
}

/** A node as TipTap's `getJSON` gives it, and as `setContent` takes it. */
export interface Json {
  type: string;
  text?: string;
  attrs?: Record<string, unknown>;
  content?: Json[];
}

/** The composer's document for a body: a paragraph per line. */
export function contentOf(body: string): Json {
  return {
    type: "doc",
    content: body.split("\n").map((line) => {
      const content: Json[] = pieces(line).map((piece) =>
        piece.kind === "text"
          ? { type: "text", text: piece.text }
          : { type: piece.kind === "user" ? "mention" : "pageLink", attrs: { id: piece.id } },
      );
      return content.length > 0 ? { type: "paragraph", content } : { type: "paragraph" };
    }),
  };
}

/** The body of the composer's document: a line per paragraph, a token per mention. */
export function bodyOf(doc: Json): string {
  const inline = (node: Json): string => {
    // A pasted node may hold any id: one the grammar would not read back is dropped.
    const id = typeof node.attrs?.id === "string" && ID.test(node.attrs.id) ? node.attrs.id : "";
    switch (node.type) {
      case "text":
        return node.text ?? "";
      case "hardBreak":
        return "\n";
      // Tokens of the body's syntax, never shown as they are.
      case "mention":
        return id ? `<user:${id}>` : ""; // i18n-ignore
      case "pageLink":
        return id ? `<page:${id}>` : ""; // i18n-ignore
      default:
        return (node.content ?? []).map(inline).join("");
    }
  };
  return (doc.content ?? []).map(inline).join("\n");
}
