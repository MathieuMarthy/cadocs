// Writes what `cadocs-content` tests the Rust schema against (ADR 0008): the editor's schema,
// and the Yjs update the editor writes for `every-node.json`. `--check` fails instead when the
// written files are not current, so the build catches a block added without `npm run schema`.

import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

import { runnerImport } from "vite";

const tests = (name: string) =>
  fileURLToPath(new URL(`../../crates/cadocs-content/tests/${name}`, import.meta.url));

const { module } = await runnerImport<{
  schemaJson: () => unknown;
  editorUpdate: (json: unknown) => Uint8Array;
}>(fileURLToPath(new URL("../src/editor/schema.ts", import.meta.url)));

const outputs: [string, Buffer][] = [
  ["tiptap-schema.json", Buffer.from(`${JSON.stringify(module.schemaJson(), null, 2)}\n`)],
  [
    "every-node.bin",
    Buffer.from(module.editorUpdate(JSON.parse(readFileSync(tests("every-node.json"), "utf8")))),
  ],
];

const check = process.argv.includes("--check");
for (const [name, written] of outputs) {
  if (!check) {
    writeFileSync(tests(name), written);
    continue;
  }
  let current = Buffer.alloc(0);
  try {
    current = readFileSync(tests(name));
  } catch {
    // Missing counts as stale.
  }
  if (!current.equals(written)) {
    console.error(`${name} is stale: run \`npm --prefix web run schema\``);
    process.exit(1);
  }
}
