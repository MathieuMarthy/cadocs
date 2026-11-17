// Comment bodies (`src/lib/mentions.ts`): the grammar `cadocs-content`'s `comment.rs` reads, and
// the composer's document written to a body and back.

import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import { test } from "node:test";

import { runnerImport } from "vite";

const { module: mentions } = await runnerImport<typeof import("../src/lib/mentions.ts")>(
  fileURLToPath(new URL("../src/lib/mentions.ts", import.meta.url)),
);

test("a body reads as text and mentions, as the server reads it", () => {
  assert.deepEqual(mentions.pieces("Ask <user:u-1> about <page:p_2>.\n<b>ok</b>"), [
    { kind: "text", text: "Ask " },
    { kind: "user", id: "u-1" },
    { kind: "text", text: " about " },
    { kind: "page", id: "p_2" },
    { kind: "text", text: ".\n<b>ok</b>" },
  ]);
  for (const body of ["<user:>", "<user:a b>", "<user:a", "<team:a>", `<user:${"a".repeat(65)}>`]) {
    assert.deepEqual(mentions.pieces(body), [{ kind: "text", text: body }], body);
  }
  assert.deepEqual(mentions.pieces("<<user:a>>"), [
    { kind: "text", text: "<" },
    { kind: "user", id: "a" },
    { kind: "text", text: ">" },
  ]);
});

test("the composer's document becomes a body and back", () => {
  const body = "Ask <user:u1> about <page:p1>\n\nthen  <user:u2>";
  const doc = mentions.contentOf(body);
  assert.deepEqual(doc, {
    type: "doc",
    content: [
      {
        type: "paragraph",
        content: [
          { type: "text", text: "Ask " },
          { type: "mention", attrs: { id: "u1" } },
          { type: "text", text: " about " },
          { type: "pageLink", attrs: { id: "p1" } },
        ],
      },
      { type: "paragraph" },
      {
        type: "paragraph",
        content: [
          { type: "text", text: "then  " },
          { type: "mention", attrs: { id: "u2" } },
        ],
      },
    ],
  });
  assert.equal(mentions.bodyOf(doc), body);
  assert.equal(mentions.bodyOf(mentions.contentOf("")), "");
});

test("a mention whose id the grammar would not read back is dropped", () => {
  const node = (type: string, id: string) => ({ type, attrs: { id } });
  const doc = {
    type: "doc",
    content: [
      {
        type: "paragraph",
        content: [
          { type: "text", text: "a" },
          node("mention", "x>y"),
          node("pageLink", "p q"),
          node("mention", "a".repeat(65)),
          node("pageLink", ""),
          node("mention", "ok_1-2"),
        ],
      },
    ],
  };
  assert.equal(mentions.bodyOf(doc), "a<user:ok_1-2>");
});

test("a hard break is a new line, and unknown nodes give their text", () => {
  assert.equal(
    mentions.bodyOf({
      type: "doc",
      content: [
        {
          type: "paragraph",
          content: [
            { type: "text", text: "a" },
            { type: "hardBreak" },
            { type: "text", text: "b" },
            { type: "mention", attrs: { id: null } },
          ],
        },
      ],
    }),
    "a\nb",
  );
});
