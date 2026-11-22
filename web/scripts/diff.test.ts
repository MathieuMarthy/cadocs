import assert from "node:assert/strict";
import { test } from "node:test";

import { diffBlocks } from "../src/lib/diff.ts";

test("equal lists are all kept", () => {
  assert.deepEqual(diffBlocks(["a", "b"], ["a", "b"]), [
    { kind: "same", block: "a" },
    { kind: "same", block: "b" },
  ]);
});

test("a changed block is a removal then an addition, the rest kept", () => {
  assert.deepEqual(diffBlocks(["a", "b", "c"], ["a", "x", "c"]), [
    { kind: "same", block: "a" },
    { kind: "del", block: "b" },
    { kind: "add", block: "x" },
    { kind: "same", block: "c" },
  ]);
});

test("an insertion and a deletion at the ends", () => {
  assert.deepEqual(diffBlocks(["a"], ["n", "a"]), [
    { kind: "add", block: "n" },
    { kind: "same", block: "a" },
  ]);
  assert.deepEqual(diffBlocks(["a", "z"], ["a"]), [
    { kind: "same", block: "a" },
    { kind: "del", block: "z" },
  ]);
});

test("a very large change falls back to removed then added", () => {
  const before = Array.from({ length: 3000 }, (_, i) => `a${i}`);
  const after = Array.from({ length: 3000 }, (_, i) => `b${i}`);
  const parts = diffBlocks(before, after);
  assert.equal(parts.length, 6000);
  assert.ok(parts.slice(0, 3000).every((part) => part.kind === "del"));
});
