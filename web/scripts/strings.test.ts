// The check of `strings.ts`: each place a word can hide is planted once, and each place code
// strings go must stay quiet. `npm run build` runs it with the check itself.

import assert from "node:assert/strict";
import { test } from "node:test";

import { leftovers } from "./strings.ts";

const found = (file: string, source: string) => leftovers(file, source).map((item) => item.text);

test("template text, text attributes and shown expressions are found", () => {
  const source = `<script lang="ts">
  let { open }: { open: boolean } = $props();
</script>

<h1>settings</h1>
<input placeholder="untitled" aria-label="page title" />
<Field label="name" />
<IconButton verb="new page" />
<p>{open ? "close" : t("tree.expand")}</p>
<span title="copy {name}"></span>
{#each items as item}{@const shown = item.title || "untitled"}{shown}{/each}
`;
  assert.deepEqual(found("Plant.svelte", source), [
    "settings",
    "untitled",
    "page title",
    "name",
    "new page",
    "close",
    "copy",
    "untitled",
  ]);
});

test("code attributes, keys of the catalogue and the product name are left alone", () => {
  const source = `<script lang="ts">
  import { t } from "../lib/i18n";
</script>

<a class="button secondary" href="/login" rel="noreferrer">{t("login.title")}</a>
<input type="search" name="q" autocomplete="off" enterkeyhint="search" aria-hidden="true" />
<span class="logo">cadocs</span>
{#if mode === "edit"}<Icon icon="x" />{/if}
<!-- a comment is not shown -->
`;
  assert.deepEqual(found("Quiet.svelte", source), []);
});

test("words given to text properties, calls and the DOM are found in code", () => {
  const source = `import { notify } from "./notice";

const items = [{ icon: "link", label: "copy link" }];
element.textContent = "no match";
element.setAttribute("aria-label", "table");
notify(\`uploaded \${name}\`);
let problem = null;
problem = "offline, retrying";
function describe() {
  return "something went wrong";
}
const phrase = cond ? "a whole sentence here" : null;
notify(\`\${file.name || "file"}: \${reason}\`);
const actions = [{ verb: "restore", confirm: "delete forever" }];
`;
  assert.deepEqual(found("plant.ts", source), [
    "copy link",
    "no match",
    "table",
    "uploaded …",
    "offline, retrying",
    "something went wrong",
    "a whole sentence here",
    "file",
    "restore",
    "delete forever",
  ]);
});

test("strings that are code are left alone in code", () => {
  const source = `import { t } from "./i18n";

element.className = "menu-item danger";
element.classList.add("table-edge row");
element.setAttribute("role", "menuitem");
document.querySelector(".tree a.row");
if (event.key === "Arrow Down") void 0;
const path = \`/spaces/\${id}/tree\`;
notify(t("page.trashed"));
const label = t("tree.expand", { name });
// i18n-ignore
const markdown = "[x] done";
type Mode = "read only" | "edit";
`;
  assert.deepEqual(found("quiet.ts", source), []);
});
