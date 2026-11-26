import assert from "node:assert/strict";
import { test } from "node:test";

import { fingerprint } from "../src/editor/fingerprint.ts";

test("the fingerprint is FNV-1a of the UTF-8 bytes, as the server's", () => {
  // The same vectors as `fingerprint_is_fnv_1a` in `cadocs-content`.
  assert.equal(fingerprint(""), "811c9dc5");
  assert.equal(fingerprint("a"), "e40c292c");
  assert.equal(fingerprint("é"), "1e9de8c1");
});
