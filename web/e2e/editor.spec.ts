// The editor's blocks, slash menu, shortcuts, read-only mode and content check, in two browsers.

import { expect, type Page } from "@playwright/test";

import { WebsocketProvider } from "y-websocket";
import * as Y from "yjs";

import {
  bothOnline,
  command,
  commands,
  DOC,
  editor,
  expectText,
  grant,
  newLine,
  status,
  test,
} from "./editing.ts";
import { logIn } from "./fixtures.ts";
import { TestServer } from "./server.ts";

test("the slash menu lists the blocks, filters them, and moves with the arrows", async ({
  left,
  right,
}) => {
  await bothOnline(left, right);
  await editor(left).click();
  await left.keyboard.type("/");

  await expect(commands(left)).toBeVisible();
  await expect(left.getByText("commands · 15")).toBeVisible();
  const options = commands(left).getByRole("option");
  await expect(options.first()).toHaveAttribute("aria-selected", "true");
  await left.keyboard.press("ArrowDown");
  await expect(options.nth(1)).toHaveAttribute("aria-selected", "true");
  await left.keyboard.press("ArrowUp");
  await left.keyboard.press("ArrowUp");
  await expect(options.last()).toHaveAttribute("aria-selected", "true");

  await left.keyboard.type("quo");
  await expect(options).toHaveCount(1);
  await expect(options).toHaveText(/quote/);

  await left.keyboard.press("Escape");
  await expect(commands(left)).toBeHidden();
  // Escape leaves what was typed, as text.
  await expectText(editor(right).locator("p"), "/quo");
});

const BLOCKS: { query: string; text: string; selector: string }[] = [
  { query: "h1", text: "Title one", selector: "h1" },
  { query: "h2", text: "Title two", selector: "h2" },
  { query: "h3", text: "Title three", selector: "h3" },
  { query: "bullet", text: "A bullet", selector: "ul:not([data-type]) > li" },
  { query: "numbered", text: "A number", selector: "ol > li" },
  { query: "task", text: "A task", selector: 'ul[data-type="taskList"] > li > div' },
  { query: "code", text: "let x = 1;", selector: "pre code" },
  { query: "quote", text: "A quote", selector: "blockquote" },
];

test("each block made from the slash menu reaches the other browser", async ({ left, right }) => {
  await bothOnline(left, right);
  await editor(left).click();
  for (const block of BLOCKS) {
    await command(left, block.query);
    await left.keyboard.type(block.text);
    await expectText(editor(right).locator(block.selector), block.text);
    await newLine(left);
    // A code block ends on a third Enter at its end, the other blocks through `text`.
    if (block.query === "code") {
      await left.keyboard.press("Enter");
      await left.keyboard.press("Enter");
    }
    // Leave a list or a quote for a plain paragraph again.
    await command(left, "text");
  }
  await command(left, "divider");
  await expect(editor(right).locator("hr")).toHaveCount(1);
});

test("Markdown shortcuts make the blocks and marks", async ({ left, right }) => {
  await bothOnline(left, right);
  await editor(left).click();
  const lines = [
    "# Heading",
    "## Section",
    "### Detail",
    "- bullet",
    "1. numbered",
    "[ ] task",
    "> quote",
  ];
  for (const line of lines) {
    await left.keyboard.type(line);
    await newLine(left);
    await command(left, "text");
  }
  await left.keyboard.type("---");
  await left.keyboard.type("**bold** *italic* ~~struck~~ `inline` [cadocs](https://example.org/a) ");

  const right_ = editor(right);
  await expectText(right_.locator("h1"), "Heading");
  await expectText(right_.locator("h2"), "Section");
  await expectText(right_.locator("h3"), "Detail");
  await expectText(right_.locator("ul:not([data-type]) > li"), "bullet");
  await expectText(right_.locator("ol > li"), "numbered");
  await expectText(right_.locator('ul[data-type="taskList"] > li > div'), "task");
  await expectText(right_.locator("blockquote"), "quote");
  await expect(right_.locator("hr")).toHaveCount(1);
  await expectText(right_.locator("strong"), "bold");
  await expectText(right_.locator("em"), "italic");
  await expectText(right_.locator("s"), "struck");
  await expectText(right_.locator("p code"), "inline");
  const link = right_.locator("a");
  await expect(link).toHaveText("cadocs");
  await expect(link).toHaveAttribute("href", "https://example.org/a");

  // The text typed after a link is not part of it.
  await left.keyboard.type("after");
  await expect(right_.locator("p:has(a)")).toContainText("cadocs after");
  await expect(link).toHaveText("cadocs");
});

test("a link to a script is not made", async ({ left, right }) => {
  await bothOnline(left, right);
  await editor(left).click();
  await left.keyboard.type("[click](javascript:alert(1)) ");
  await expectText(editor(right).locator("p"), "[click](javascript:alert(1)) ");
  await expect(editor(right).locator("a")).toHaveCount(0);
});

test("a typed address becomes a link", async ({ left, right }) => {
  await bothOnline(left, right);
  await editor(left).click();
  await left.keyboard.type("see https://example.org/docs now");
  await expect(editor(right).locator("a")).toHaveAttribute("href", "https://example.org/docs");
});

test("a task checked in one browser shows checked in the other", async ({ left, right }) => {
  await bothOnline(left, right);
  await editor(left).click();
  await left.keyboard.type("[ ] ship it");
  const item = (page: Page) => editor(page).locator('ul[data-type="taskList"] > li');
  await expect(item(right)).toHaveAttribute("data-checked", "false");

  await item(left).getByRole("checkbox").check();
  await expect(item(right)).toHaveAttribute("data-checked", "true");
  await expect(item(right).getByRole("checkbox")).toBeChecked();
});

test("code is highlighted in both browsers, by its language or by a guess", async ({
  left,
  right,
}) => {
  await bothOnline(left, right);
  await editor(left).click();
  await left.keyboard.type("```rust ");
  await left.keyboard.type("fn main() {}");
  const code = editor(right).locator("pre code");
  await expect(code).toHaveClass(/language-rust/);
  await expect(code.locator(".hljs-keyword").first()).toHaveText("fn");
  await expectText(editor(left).locator("pre .hljs-keyword").first(), "fn");
});

test("a viewer reads without writing, and a change of role applies without a reload", async ({
  left,
  right,
  server,
  member,
}) => {
  await bothOnline(left, right);
  await editor(left).click();
  await left.keyboard.type("from the owner");
  await expectText(editor(right).locator("p"), "from the owner");

  await grant(server, member, "viewer");
  await expect(editor(right)).toHaveAttribute("contenteditable", "false");
  await expect(right.getByLabel("page title")).not.toBeEditable();
  await expect(status(right)).toHaveText("synced · 2 online · read only");
  await editor(right).click();
  await right.keyboard.type("/");
  await expect(commands(right)).toHaveCount(0);
  await expectText(editor(left).locator("p"), "from the owner");

  await grant(server, member, "editor");
  await expect(editor(right)).toHaveAttribute("contenteditable", "true");
  await expect(status(right)).toHaveText("synced · 2 online");
  await editor(right).click();
  await right.keyboard.press("Control+End");
  await right.keyboard.type(", and Grace");
  await expectText(editor(left).locator("p"), "from the owner, and Grace");
});

test("a demotion stops writing at once, before the role is read again", async ({
  left,
  right,
  server,
  member,
}) => {
  await bothOnline(left, right);
  // The page cannot be read again, so only the close itself can make the editor read-only.
  await right.route("**/api/v1/pages/*", (route) => route.abort());
  await grant(server, member, "viewer");
  await expect(editor(right)).toHaveAttribute("contenteditable", "false");
  await expect(right.getByLabel("page title")).not.toBeEditable();
});

test("a read older than a demotion does not make the page writable again", async ({
  left,
  right,
  server,
  member,
}) => {
  await bothOnline(left, right);
  // The first read after this holds its answer, fetched while Grace still edits, for 3 seconds.
  let held: () => void;
  const started = new Promise<void>((done) => (held = done));
  let first = true;
  await right.route("**/api/v1/pages/*", async (route) => {
    const response = await route.fetch();
    if (first) {
      first = false;
      held();
      await new Promise((done) => setTimeout(done, 3_000));
    }
    await route.fulfill({ response });
  });
  // A rename is a change to the space, which makes the page read itself again.
  await server.rename(DOC, "Renamed");
  await started;
  await grant(server, member, "viewer");
  await expect(status(right)).toHaveText("synced · 2 online · read only");
  await right.waitForTimeout(3_500);
  await expect(editor(right)).toHaveAttribute("contenteditable", "false");
});

/** A client from a later version, whose schema has a block this one lacks. */
async function writeFutureBlock(server: TestServer) {
  const doc = new Y.Doc();
  const provider = new WebsocketProvider(server.wsUrl, await server.page(DOC), doc, {
    disableBc: true,
    WebSocketPolyfill: await server.webSocketClass(),
  });
  await expect.poll(() => provider.synced).toBe(true);
  const block = new Y.XmlElement("futureBlock");
  block.insert(0, [new Y.XmlText("from the future")]);
  doc.getXmlFragment("default").push([block]);
  return { doc, provider };
}

/** The names of the top-level nodes the server holds, as a fresh client reads them. */
async function storedNodes(server: TestServer) {
  const doc = new Y.Doc();
  const provider = new WebsocketProvider(server.wsUrl, await server.page(DOC), doc, {
    disableBc: true,
    WebSocketPolyfill: await server.webSocketClass(),
  });
  try {
    await expect.poll(() => provider.synced).toBe(true);
    return doc
      .getXmlFragment("default")
      .toArray()
      .map((node) => (node instanceof Y.XmlElement ? node.nodeName : "text"));
  } finally {
    provider.destroy();
    doc.destroy();
  }
}

test("a block this tab does not know stays intact for the others", async ({
  left,
  right,
  server,
}) => {
  await bothOnline(left, right);
  await editor(left).click();
  await left.keyboard.type("known");
  await expectText(editor(right).locator("p"), "known");

  // Left is open as the block arrives, right opens after it.
  await right.close();
  const writer = await writeFutureBlock(server);
  try {
    const banner = left.getByRole("alert");
    await expect(banner).toContainText("this page holds content this version cannot show");
    await expect(banner.getByRole("button", { name: "reload" })).toBeVisible();
    await expect(editor(left)).toHaveAttribute("contenteditable", "false");

    const late = await (await left.context().browser()!.newContext()).newPage();
    await logIn(late, server, (await server.session()).token);
    await late.goto(left.url());
    await expect(late.getByRole("alert")).toContainText("cannot show");
    await expect(editor(late)).toHaveAttribute("contenteditable", "false");

    // Nothing either tab did removed the block, for the client that knows it.
    await expect.poll(() => storedNodes(server)).toEqual(["paragraph", "futureBlock"]);
    expect(
      writer.doc.getXmlFragment("default").toArray().map((node) => (node as Y.XmlElement).nodeName),
    ).toEqual(["paragraph", "futureBlock"]);

    await banner.getByRole("button", { name: "reload" }).click();
    await expect(left.getByRole("alert")).toContainText("cannot show");
    await late.close();
  } finally {
    writer.provider.destroy();
    writer.doc.destroy();
  }
});

// The Markdown source of a block shows where the caret is, and is edited as text (ADR 0018).

const tasks = (page: Page) => editor(page).locator('ul[data-type="taskList"] > li');

test("every heading level carries its number", async ({ left, right }) => {
  await bothOnline(left, right);
  await editor(left).click();
  for (const line of ["# One", "## Two", "### Three", "## Four"]) {
    await left.keyboard.type(line);
    await newLine(left);
  }
  const numbers = editor(right).locator("h1, h2, h3");
  await expect(numbers).toHaveCount(4);
  await expect
    .poll(() => numbers.evaluateAll((all) => all.map((heading) => heading.dataset.number)))
    .toEqual(["01", "01.1", "01.1.1", "01.2"]);
  const shown = await numbers
    .first()
    .evaluate((heading) => getComputedStyle(heading, "::before").content);
  expect(shown).toBe('"01"');
});

test("the caret in a heading shows its hashes, in its own browser only", async ({ left, right }) => {
  await bothOnline(left, right);
  await editor(left).click();
  await left.keyboard.type("## Section");
  await expect(editor(left).locator("h2 .md-hint")).toHaveText("##");
  await expectText(editor(right).locator("h2"), "Section");
  await expect(editor(right).locator(".md-hint")).toHaveCount(0);

  await left.keyboard.press("Enter");
  await expect(editor(left).locator(".md-hint")).toHaveCount(0);
});

test("arrowing onto the hashes edits them as text, and leaving applies them", async ({
  left,
  right,
}) => {
  await bothOnline(left, right);
  await editor(left).click();
  await left.keyboard.type("## Section");
  await left.keyboard.press("Home");
  await left.keyboard.press("ArrowLeft");
  // While edited, the source is text, and the others see it too.
  await expectText(editor(right).locator("h2"), "## Section");
  await left.keyboard.type("#");
  await left.keyboard.press("End");
  await left.keyboard.press("Enter");
  await expectText(editor(right).locator("h3"), "Section");
  await expect(editor(right).locator("h2")).toHaveCount(0);
});

test("backspace at the start of a heading edits its hashes, down to plain text", async ({
  left,
  right,
}) => {
  await bothOnline(left, right);
  await editor(left).click();
  await left.keyboard.type("# Title");
  await left.keyboard.press("Home");
  await left.keyboard.press("Backspace");
  await expectText(editor(right).locator("h1"), "#Title");
  await left.keyboard.press("Backspace");
  await newLine(left);
  await expect(editor(right).locator("h1")).toHaveCount(0);
  await expectText(editor(right).locator("p"), "Title", "", "");
});

test("the brackets of a task are edited as text, and an x checks it", async ({ left, right }) => {
  await bothOnline(left, right);
  await editor(left).click();
  await left.keyboard.type("[ ] buy bread");
  await left.keyboard.press("Home");
  await left.keyboard.press("ArrowLeft");
  await expect(tasks(left).first().getByRole("checkbox")).toBeHidden();
  await left.keyboard.press("ArrowLeft");
  await left.keyboard.press("Backspace");
  await left.keyboard.type("x");
  await expectText(tasks(right).locator("> div"), "[x] buy bread");

  await newLine(left);
  await expect(tasks(right).first()).toHaveAttribute("data-checked", "true");
  await expectText(tasks(right).first().locator("> div"), "buy bread");
  await expect(tasks(left).first().getByRole("checkbox")).toBeVisible();
});

test("removing the dash of a bullet makes it plain text", async ({ left, right }) => {
  await bothOnline(left, right);
  await editor(left).click();
  await left.keyboard.type("intro");
  await left.keyboard.press("Enter");
  await left.keyboard.type("- item");
  await left.keyboard.press("Home");
  await left.keyboard.press("ArrowLeft");
  await left.keyboard.press("Backspace");
  await left.keyboard.press("Delete");
  await left.keyboard.press("Control+Home");
  await expect(editor(right).locator("ul")).toHaveCount(0);
  await expectText(editor(right).locator("p"), "intro", "item", "");
});

test("a divider turned back into dashes comes back once the caret leaves", async ({
  left,
  right,
}) => {
  await bothOnline(left, right);
  await editor(left).click();
  await left.keyboard.type("above");
  await left.keyboard.press("Enter");
  await left.keyboard.type("---");
  await expect(editor(right).locator("hr")).toHaveCount(1);

  await left.keyboard.press("Backspace");
  await expect(editor(right).locator("hr")).toHaveCount(0);
  await expectText(editor(right).locator("p"), "above", "---");
  await left.keyboard.press("ArrowUp");
  await expect(editor(right).locator("hr")).toHaveCount(1);
  await expect(editor(right).locator("p", { hasText: "---" })).toHaveCount(0);

  // Arrowing onto the divider shows its dashes again.
  await left.keyboard.press("End");
  await left.keyboard.press("ArrowDown");
  await expect(editor(right).locator("hr")).toHaveCount(0);
  await left.keyboard.press("Control+Home");
  await expect(editor(right).locator("hr")).toHaveCount(1);
});

test("a code block shows its language, and its fence edits it", async ({ left, right }) => {
  await bothOnline(left, right);
  await editor(left).click();
  await left.keyboard.type("intro");
  await left.keyboard.press("Enter");
  await left.keyboard.type("```rust ");
  await left.keyboard.type("fn main() {}");
  const block = (page: Page) => editor(page).locator("pre");
  await expect(block(right)).toHaveAttribute("data-language", "rust");
  const label = await block(left).evaluate((pre) => getComputedStyle(pre, "::before").content);
  expect(label).toBe('"rust"');

  await left.keyboard.press("Control+Home");
  await left.keyboard.press("ArrowDown");
  await left.keyboard.press("Home");
  await left.keyboard.press("ArrowLeft");
  await expectText(block(right).locator("code"), "```rust\nfn main() {}");
  for (let i = 0; i < 4; i++) await left.keyboard.press("Backspace");
  await left.keyboard.type("python");
  await left.keyboard.press("Control+Home");
  await expect(block(right)).toHaveAttribute("data-language", "python");
  await expect(block(right).locator("code")).toHaveClass(/language-python/);
  await expectText(block(right).locator("code"), "fn main() {}");
});

test("a source being edited survives the other browser's typing", async ({ left, right }) => {
  await bothOnline(left, right);
  await editor(left).click();
  await left.keyboard.type("# Head");
  await left.keyboard.press("Enter");
  await left.keyboard.type("body");
  await expectText(editor(right).locator("p"), "body", "");
  await left.keyboard.press("Control+Home");
  await left.keyboard.press("ArrowLeft");
  await expectText(editor(right).locator("h1"), "# Head");

  await editor(right).locator("p").first().click();
  await right.keyboard.press("End");
  await right.keyboard.type(" more");
  await expectText(editor(left).locator("p"), "body more", "");
  // With all its hashes gone, only this tab knows the block was a heading written out.
  await left.keyboard.press("Backspace");
  await left.keyboard.press("Delete");
  await left.keyboard.press("ArrowDown");
  await expect(editor(right).locator("h1")).toHaveCount(0);
  await expectText(editor(right).locator("p"), "Head", "body more", "");
});

test("a source written out in one browser is left alone by a caret passing in the other", async ({
  left,
  right,
}) => {
  await bothOnline(left, right);
  await editor(left).click();
  await left.keyboard.type("## Intro");
  await left.keyboard.press("Enter");
  await left.keyboard.type("body");
  await expectText(editor(right).locator("p"), "body", "");

  await editor(right).locator("h2").click();
  await left.keyboard.press("Control+Home");
  await left.keyboard.press("ArrowLeft");
  await expectText(editor(right).locator("h2"), "## Intro");
  await editor(right).locator("p").first().click();
  await right.keyboard.type("!");
  await expectText(editor(left).locator("p").first(), "body!");
  await expectText(editor(left).locator("h2"), "## Intro");
});

test("hashes removed before the other browser types still make plain text", async ({
  left,
  right,
}) => {
  await bothOnline(left, right);
  await editor(left).click();
  await left.keyboard.type("# Head");
  await left.keyboard.press("Enter");
  await left.keyboard.type("body");
  await left.keyboard.press("Control+Home");
  await left.keyboard.press("Backspace");
  await left.keyboard.press("Backspace");
  await expectText(editor(right).locator("h1"), "Head");

  await editor(right).locator("p").first().click();
  await right.keyboard.press("End");
  await right.keyboard.type(" more");
  await expectText(editor(left).locator("p"), "body more", "");
  await left.keyboard.press("ArrowDown");
  await expect(editor(right).locator("h1")).toHaveCount(0);
  await expectText(editor(right).locator("p"), "Head", "body more", "");
});

test("a fence typed inside code stays code", async ({ left, right }) => {
  await bothOnline(left, right);
  await editor(left).click();
  await left.keyboard.type("intro");
  await left.keyboard.press("Enter");
  await left.keyboard.type("```md ");
  await left.keyboard.type("```js");
  await left.keyboard.press("Enter");
  await left.keyboard.type("x");
  await left.keyboard.press("Control+Home");
  await expectText(editor(right).locator("pre code"), "```js\nx");
  await expect(editor(right).locator("pre")).toHaveAttribute("data-language", "md");
});

test("an item taken out of a numbered list leaves the next ones numbered on", async ({
  left,
  right,
}) => {
  await bothOnline(left, right);
  await editor(left).click();
  await left.keyboard.type("1. one");
  for (const word of ["two", "three", "four"]) {
    await left.keyboard.press("Enter");
    await left.keyboard.type(word);
  }
  await left.keyboard.press("ArrowUp");
  await left.keyboard.press("ArrowUp");
  await left.keyboard.press("Home");
  await left.keyboard.press("ArrowLeft");
  await left.keyboard.press("Backspace");
  await left.keyboard.press("Backspace");
  await left.keyboard.press("Delete");
  await left.keyboard.press("Control+End");
  await expectText(editor(right).locator(":scope > p").first(), "two");
  await expect
    .poll(() => editor(right).locator("ol").evaluateAll((lists) => lists.map((list) => list.getAttribute("start"))))
    .toEqual([null, "3"]);
});

test("undoing a written source leaves the block as it was", async ({ left, right }) => {
  await bothOnline(left, right);
  await editor(left).click();
  await left.keyboard.type("## Title");
  await left.keyboard.press("Enter");
  await left.keyboard.type("body");
  // Yjs groups the changes of half a second into one undo step: this one stands alone.
  await left.waitForTimeout(600);
  await left.keyboard.press("Control+Home");
  await left.keyboard.press("ArrowLeft");
  await expectText(editor(right).locator("h2"), "## Title");
  await left.waitForTimeout(600);
  await left.keyboard.press("Control+z");
  await expectText(editor(right).locator("h2"), "Title");
  await left.keyboard.press("ArrowDown");
  await left.keyboard.type("!");
  await expectText(editor(right).locator("h2"), "Title");
});
