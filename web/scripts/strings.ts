// Finds words a person reads that are written in the code instead of the catalogue
// (`src/lib/i18n/en.ts`, step 34). Run as a script, it reads every `.svelte` and `.ts` file under
// `src/` and fails on the first file that has any. `npm run build` runs it.
//
// It reads the syntax tree, from Svelte's parser, which also reads TypeScript:
// - in a template, any text with a letter, any attribute but the ones in `CODE_ATTRIBUTES`, and
//   the strings an `{expression}` can show;
// - in code, a string given to a property, variable or attribute in `TEXT_NAMES`, to a call in
//   `TEXT_CALLS`, or returned, and any string with two words outside the places code strings go.
// A string that must stay as written takes `// i18n-ignore` on its line, or the line before.

import { readdirSync, readFileSync } from "node:fs";
import { join, relative } from "node:path";
import { fileURLToPath } from "node:url";

import { parse } from "svelte/compiler";

export interface Leftover {
  line: number;
  text: string;
}

/** Shown as written in every language. */
const ALLOWED = new Set(["cadocs"]);

/** Attributes whose value is read by code, not by people. */
const CODE_ATTRIBUTES = new Set([
  "accept",
  "autocomplete",
  "class",
  "d",
  "datetime",
  "dir",
  "enterkeyhint",
  "fill",
  "focusable",
  "for",
  "form",
  "height",
  "href",
  "icon",
  "id",
  "inputmode",
  "kind",
  "lang",
  "loading",
  "maxlength",
  "method",
  "name",
  "pattern",
  "popover",
  "popovertarget",
  "rel",
  "role",
  "spellcheck",
  "src",
  "stroke",
  "stroke-linecap",
  "stroke-linejoin",
  "stroke-width",
  "style",
  "tabindex",
  "target",
  "to",
  "type",
  "value",
  "variant",
  "viewBox",
  "width",
  "xmlns",
]);

/** `aria-*` attributes that name or describe something; the others are states and ids. */
const TEXT_ARIA = new Set(["aria-label", "aria-description", "aria-roledescription"]);

/** Properties, variables and attributes whose value is shown. */
const TEXT_NAMES = new Set([
  "alt",
  "aria-label",
  "ariaLabel",
  "confirm",
  "description",
  "empty",
  "heading",
  "hint",
  "innerText",
  "label",
  "message",
  "notice",
  "placeholder",
  "problem",
  "text",
  "textContent",
  "title",
  "verb",
]);

/** Calls whose string arguments are shown. */
const TEXT_CALLS = new Set([
  "append",
  "createTextNode",
  "notify",
  "prepend",
  "replaceChildren",
  "signOut",
]);

/** Calls whose string arguments are code: selectors, classes, events, keys of the catalogue. */
const CODE_CALLS = new Set([
  "add",
  "addEventListener",
  "closest",
  "contains",
  "createElement",
  "createElementNS",
  "getAttribute",
  "matches",
  "querySelector",
  "querySelectorAll",
  "remove",
  "removeAttribute",
  "removeEventListener",
  "t",
  "toggle",
]);

/** Properties whose value is code even with spaces in it. */
const CODE_NAMES = new Set(["class", "className", "rel", "selector", "style"]);

const words = (text: string) => /\p{L}/u.test(text) && !ALLOWED.has(text.trim());
/** Two words, and nothing of markup or code. */
const prose = (text: string) => /\p{L}\S*\s+\S*\p{L}/u.test(text) && !/[<>={}]/.test(text);

type Node = { type: string; start: number; [key: string]: any };

const isNode = (value: unknown): value is Node =>
  typeof value === "object" && value !== null && typeof (value as Node).type === "string";

/** The name of a property, member or variable, whatever its form. */
function nameOf(node: Node | undefined): string | null {
  if (!node) return null;
  if (node.type === "Identifier") return node.name;
  if (node.type === "Literal" && typeof node.value === "string") return node.value;
  if (node.type === "MemberExpression") return nameOf(node.property);
  return null;
}

/** The strings an expression can come out as: its branches, its fallbacks, its parts. */
function shown(node: Node | undefined, found: Node[]) {
  if (!node) return;
  switch (node.type) {
    case "Literal":
      if (typeof node.value === "string") found.push(node);
      break;
    case "TemplateLiteral":
      found.push(node);
      for (const expression of node.expressions) shown(expression, found);
      break;
    case "ConditionalExpression":
      shown(node.consequent, found);
      shown(node.alternate, found);
      break;
    case "LogicalExpression":
      shown(node.left, found);
      shown(node.right, found);
      break;
    case "BinaryExpression":
      if (node.operator === "+") {
        shown(node.left, found);
        shown(node.right, found);
      }
      break;
    case "TSAsExpression":
    case "TSSatisfiesExpression":
    case "ParenthesizedExpression":
      shown(node.expression, found);
      break;
  }
}

const textOf = (node: Node) =>
  node.type === "Literal"
    ? String(node.value)
    : node.type === "TemplateLiteral"
      ? node.quasis.map((quasi: Node) => quasi.value.cooked).join("…")
      : String(node.data ?? "");

/** Keys whose subtree is types or positions, never a value. */
const SKIPPED_KEYS = new Set([
  "metadata",
  "loc",
  "returnType",
  "typeAnnotation",
  "typeParameters",
  "typeArguments",
  "superTypeParameters",
  "implements",
]);

function children(node: Node): Node[] {
  const found: Node[] = [];
  for (const [key, value] of Object.entries(node)) {
    if (SKIPPED_KEYS.has(key)) continue;
    for (const item of Array.isArray(value) ? value : [value]) if (isNode(item)) found.push(item);
  }
  return found;
}

/** Walks code: `report` takes the strings shown, `seen` the ones that are code. */
function walkCode(node: Node, report: (node: Node) => void, seen: Set<Node>) {
  const shownBy = (expression: Node | undefined) => {
    const found: Node[] = [];
    shown(expression, found);
    for (const item of found) {
      seen.add(item);
      report(item);
    }
  };
  const code = (expression: Node | undefined) => {
    if (expression?.type === "Literal" || expression?.type === "TemplateLiteral")
      seen.add(expression);
  };
  switch (node.type) {
    case "ImportDeclaration":
    case "ExportAllDeclaration":
    case "TSTypeAliasDeclaration":
    case "TSInterfaceDeclaration":
    case "TSModuleDeclaration":
      return;
    case "ExportNamedDeclaration":
      if (node.source) return;
      break;
    case "Property": {
      const name = nameOf(node.key);
      if (!node.computed) code(node.key);
      if (name && TEXT_NAMES.has(name)) shownBy(node.value);
      if (name && CODE_NAMES.has(name)) code(node.value);
      break;
    }
    case "AssignmentExpression": {
      const name = nameOf(node.left);
      if (name && TEXT_NAMES.has(name)) shownBy(node.right);
      if (name && CODE_NAMES.has(name)) code(node.right);
      break;
    }
    case "VariableDeclarator": {
      const name = nameOf(node.id);
      if (name && TEXT_NAMES.has(name)) shownBy(node.init);
      break;
    }
    case "ReturnStatement":
      shownBy(node.argument);
      break;
    case "ArrowFunctionExpression":
      if (node.expression) shownBy(node.body);
      break;
    case "CallExpression": {
      const name = nameOf(node.callee);
      const [first, second] = node.arguments as Node[];
      if (name === "setAttribute" && first?.type === "Literal") {
        code(first);
        if (TEXT_NAMES.has(String(first.value))) shownBy(second);
        else code(second);
      } else if (name && TEXT_CALLS.has(name)) {
        for (const argument of node.arguments) shownBy(argument);
      } else if (name && CODE_CALLS.has(name)) {
        for (const argument of node.arguments) code(argument);
      }
      break;
    }
    case "BinaryExpression":
      if (["===", "!==", "==", "!=", "in"].includes(node.operator)) {
        code(node.left);
        code(node.right);
      }
      break;
    case "SwitchCase":
      code(node.test);
      break;
    case "Literal":
      if (typeof node.value !== "string") break;
    // falls through
    case "TemplateLiteral":
      // A string anywhere else counts only once it reads as a phrase.
      if (!seen.has(node) && prose(textOf(node))) report(node);
      break;
  }
  for (const child of children(node)) walkCode(child, report, seen);
}

/** Walks a template: text, attributes and `{expressions}`. */
function walkTemplate(node: Node, report: (node: Node) => void, walkScript: (node: Node) => void) {
  const shownBy = (expression: Node) => {
    const found: Node[] = [];
    shown(expression, found);
    for (const item of found) report(item);
  };
  switch (node.type) {
    case "Text":
      report(node);
      return;
    case "ExpressionTag":
      shownBy(node.expression);
      return;
    case "ConstTag":
      for (const declarator of node.declaration.declarations) shownBy(declarator.init);
      return;
    case "Attribute": {
      const name: string = node.name;
      const code =
        CODE_ATTRIBUTES.has(name) ||
        name.startsWith("data-") ||
        name.startsWith("on") ||
        (name.startsWith("aria-") && !TEXT_ARIA.has(name));
      if (code || node.value === true) return;
      for (const part of Array.isArray(node.value) ? node.value : [node.value]) {
        if (part.type === "Text") report(part);
        else if (part.type === "ExpressionTag") shownBy(part.expression);
      }
      return;
    }
    case "Comment":
    case "StyleSheet":
      return;
    case "Script":
      walkScript(node.content);
      return;
  }
  for (const [key, value] of Object.entries(node)) {
    // Conditions, lists and directives hold code: only fragments and attributes are walked.
    if (!["fragment", "nodes", "attributes", "consequent", "alternate", "body", "pending", "then", "catch", "fallback"].includes(key))
      continue;
    for (const item of Array.isArray(value) ? value : [value]) {
      if (isNode(item)) walkTemplate(item, report, walkScript);
    }
  }
}

/** The leftovers of one file, `.svelte` or `.ts`. */
export function leftovers(file: string, source: string): Leftover[] {
  const svelte = file.endsWith(".svelte");
  const prefix = svelte ? "" : '<script lang="ts">';
  const text = svelte ? source : `${prefix}${source}</script>`;
  const lines = source.split("\n");
  const lineOf = (offset: number) => source.slice(0, offset - prefix.length).split("\n").length;
  const ignored = (line: number) =>
    [lines[line - 1], lines[line - 2]].some((at) => at?.includes("i18n-ignore"));

  const found: Leftover[] = [];
  const report = (node: Node) => {
    const value = textOf(node);
    // A path is code.
    if (!words(value) || value.startsWith("/")) return;
    const line = lineOf(node.start);
    if (ignored(line)) return;
    found.push({ line, text: value.trim().replace(/\s+/g, " ") });
  };
  const walkScript = (program: Node) => walkCode(program, report, new Set());

  const ast = parse(text, { modern: true }) as unknown as Record<string, Node | null>;
  if (ast.module) walkScript(ast.module.content);
  if (ast.instance) walkScript(ast.instance.content);
  if (svelte && ast.fragment) walkTemplate(ast.fragment, report, walkScript);
  return found.sort((a, b) => a.line - b.line);
}

function* files(folder: string): Generator<string> {
  for (const entry of readdirSync(folder, { withFileTypes: true })) {
    const path = join(folder, entry.name);
    if (entry.isDirectory()) yield* files(path);
    else if (/\.(svelte|ts)$/.test(entry.name) && !/\.d\.ts$/.test(entry.name)) yield path;
  }
}

/** The files whose words are the catalogue itself. */
const CATALOGUES = /lib[\\/]i18n[\\/]/;

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const root = fileURLToPath(new URL("../src", import.meta.url));
  let count = 0;
  for (const path of files(root)) {
    if (CATALOGUES.test(path)) continue;
    for (const { line, text } of leftovers(path, readFileSync(path, "utf8"))) {
      console.error(`src/${relative(root, path)}:${line}: ${text}`);
      count += 1;
    }
  }
  if (count > 0) {
    console.error(`${count} strings are not in the catalogue (src/lib/i18n/en.ts)`);
    process.exit(1);
  }
}
