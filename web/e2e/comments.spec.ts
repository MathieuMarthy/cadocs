// Comments (step 38): the side panel, anchors from the selection, highlights, lost anchors,
// replies, resolve and reopen, in two browsers.

import { expect, type Page } from "@playwright/test";

import { bothOnline, DOC, editor, expectText, grant, status, test } from "./editing.ts";

const panel = (page: Page) => page.getByRole("region", { name: "comments", exact: true });
const toggle = (page: Page) => page.getByRole("banner").getByRole("button", { name: "comments", exact: true });
const marks = (page: Page) => editor(page).locator(".comment-mark");
const thread = (page: Page, text: string) => panel(page).getByRole("article").filter({ hasText: text });

/** Opens the panel, unless it shows already. */
async function openPanel(page: Page) {
  if (await panel(page).isVisible()) return;
  await toggle(page).click();
  await expect(panel(page)).toBeVisible();
}

/** Selects `words` in the editor through the DOM selection, which the editor follows. */
async function select(page: Page, words: string) {
  await editor(page).evaluate((root, words) => {
    // Highlights split the text into several nodes: find the words across them.
    const nodes: Text[] = [];
    const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      // The name on a remote caret is not text of the page.
      if (!node.parentElement?.closest(".remote-caret")) nodes.push(node as Text);
    }
    const text = nodes.map((node) => node.data).join("");
    const at = text.indexOf(words);
    if (at < 0) throw new Error(`no "${words}" in the editor`);
    const place = (offset: number, end: boolean) => {
      for (const node of nodes) {
        if (offset < node.length || (end && offset === node.length)) return { node, offset };
        offset -= node.length;
      }
      throw new Error("out of the text");
    };
    const start = place(at, false);
    const finish = place(at + words.length, true);
    // A range set in the page, as a mouse drag would leave it: Chrome widens a drag to whole
    // words and trailing spaces, so a drag cannot pick exact words.
    const range = document.createRange();
    range.setStart(start.node, start.offset);
    range.setEnd(finish.node, finish.offset);
    const selection = getSelection()!;
    selection.removeAllRanges();
    selection.addRange(range);
  }, words);
  // The editor reads the selection from the DOM, read-only or not.
  await expect.poll(() => page.evaluate(() => getSelection()?.toString())).toBe(words);
}

/** Writes a comment from the panel's composer, on the selection when there is one. */
async function comment(page: Page, body: string, on: "selection" | "page") {
  await openPanel(page);
  await panel(page).getByRole("button", { name: `comment on the ${on}`, exact: true }).click();
  await panel(page).getByRole("textbox", { name: "comment", exact: true }).fill(body);
  await panel(page).getByRole("button", { name: "comment", exact: true }).click();
  await expect(thread(page, body)).toBeVisible();
}

const counter = (page: Page) =>
  editor(page)
    .locator(".comment-count")
    .evaluate((element) => getComputedStyle(element, "::after").content);

test("a commenter comments on a selection, which the other browser sees, answers and resolves", async ({
  left,
  right,
  server,
  member,
}) => {
  await bothOnline(left, right);
  await editor(left).click();
  await left.keyboard.type("The quick brown fox jumps");
  await expectText(editor(right).locator("p"), "The quick brown fox jumps");

  await grant(server, member, "commenter");
  await expect(status(right)).toHaveText("synced · 2 online · read only");

  // Grace's editor is read-only, and her selection still makes the anchor.
  await select(right, "brown fox");
  await openPanel(right);
  await expect(panel(right).getByRole("button", { name: "comment on the selection" })).toBeVisible();
  await comment(right, "Which shade?", "selection");
  await expect(thread(right, "Which shade?")).toContainText("brown fox");
  await expect(marks(right)).toHaveText("brown fox");
  // The text stays the document's: the counter is drawn by CSS.
  await expectText(editor(right).locator("p"), "The quick brown fox jumps");
  await expect.poll(() => counter(right)).toBe('"[1]"');

  // Ada's browser hears of it through the events socket, and a click on it opens the thread.
  await expect(marks(left)).toHaveText("brown fox");
  await marks(left).click();
  await expect(panel(left)).toBeVisible();
  await expect(thread(left, "Which shade?")).toHaveAttribute("aria-current", "true");

  await thread(left, "Which shade?").getByRole("button", { name: "reply", exact: true }).click();
  await thread(left, "Which shade?").getByRole("textbox", { name: "reply", exact: true }).fill("Dark brown.");
  await thread(left, "Which shade?").getByRole("button", { name: "reply", exact: true }).click();
  await expect(thread(right, "Which shade?")).toContainText("Dark brown.");
  await expect.poll(() => counter(right)).toBe('"[2]"');

  // A commenter may resolve any thread: the highlight goes in both browsers.
  await thread(right, "Which shade?").getByRole("button", { name: "resolve", exact: true }).click();
  await expect(marks(right)).toHaveCount(0);
  await expect(marks(left)).toHaveCount(0);
  await expect(thread(left, "Which shade?")).toHaveCount(0);
  await expect(panel(left).getByText("no open comments")).toBeVisible();

  await panel(left).getByRole("button", { name: /^resolved/ }).click();
  await expect(thread(left, "Which shade?")).toContainText("resolved by Grace");
  // The quote of a resolved thread still shows its passage, picked, while the thread is picked.
  await thread(left, "Which shade?").getByRole("button", { name: "brown fox", exact: true }).click();
  await expect(marks(left)).toHaveText("brown fox");
  await expect(marks(left)).toHaveAttribute("data-active", "true");
  await expect(marks(right)).toHaveCount(0);
  await thread(left, "Which shade?").getByRole("button", { name: "reopen", exact: true }).click();
  await expect(marks(right)).toHaveText("brown fox");
  await expect(marks(left)).toHaveText("brown fox");

  // In a read-only editor too, a click on a highlight opens its thread.
  await right.reload();
  await expect(status(right)).toHaveText("synced · 2 online · read only");
  await expect(panel(right)).toHaveCount(0);
  await marks(right).click();
  await expect(thread(right, "Which shade?")).toHaveAttribute("aria-current", "true");
});

test("a highlight follows the text when someone types before it", async ({ left, right }) => {
  await bothOnline(left, right);
  await editor(left).click();
  await left.keyboard.type("alpha beta gamma");
  await select(left, "beta");
  await comment(left, "Greek letter", "selection");
  await expect(marks(left)).toHaveText("beta");
  await expect(marks(right)).toHaveText("beta");

  await editor(left).click();
  await left.keyboard.press("Control+Home");
  await left.keyboard.type("zero ");
  await expectText(editor(right).locator("p"), "zero alpha beta gamma");
  await expect(marks(left)).toHaveText("beta");
  await expect(marks(right)).toHaveText("beta");

  // In the other browser too, typing before it leaves it on the same word.
  await editor(right).click();
  await right.keyboard.press("Control+Home");
  await right.keyboard.type("one ");
  await expectText(editor(left).locator("p"), "one zero alpha beta gamma");
  await expect(marks(left)).toHaveText("beta");
  await expect(marks(right)).toHaveText("beta");

  // A reload resolves the anchor again from its stored positions.
  await right.reload();
  await expect(marks(right)).toHaveText("beta");
});

test("a writer's caret passing the marker of a heading leaves its comment in place", async ({
  left,
  right,
}) => {
  await bothOnline(left, right);
  await editor(left).click();
  await left.keyboard.type("## Heading words");
  await left.keyboard.press("Enter");
  await left.keyboard.type("Body text");
  await expect(editor(right).locator("h1, h2")).toHaveCount(1);
  await select(left, "words");
  await comment(left, "On the heading", "selection");
  await expect(marks(right)).toHaveText("words");

  // On its marker, the heading is written out as its source, then read back (ADR 0018).
  await editor(right).locator("h1, h2").click();
  await right.keyboard.press("Home");
  await right.keyboard.press("ArrowLeft");
  await expect(editor(right).locator(".md-source")).toHaveCount(1);
  await right.keyboard.press("ArrowDown");
  await expect(editor(right).locator(".md-source")).toHaveCount(0);
  await expect(editor(left).locator("h1, h2")).toHaveCount(1);
  await expect(marks(left)).toHaveText("words");
  await expect(marks(right)).toHaveText("words");
});

test("a comment whose passage was deleted keeps its quote, with no highlight", async ({
  left,
  right,
}) => {
  await bothOnline(left, right);
  await editor(left).click();
  await left.keyboard.type("keep this, drop that, keep the rest");
  await select(left, "drop that");
  await comment(left, "Should go", "selection");
  await expect(marks(right)).toHaveText("drop that");
  await openPanel(right);
  await expect(thread(right, "Should go")).not.toContainText("passage removed");

  await select(left, "drop that, ");
  await left.keyboard.press("Backspace");
  await expectText(editor(right).locator("p"), "keep this, keep the rest");
  await expect(marks(left)).toHaveCount(0);
  await expect(marks(right)).toHaveCount(0);
  await expect(thread(left, "Should go")).toContainText("drop that");
  await expect(thread(left, "Should go")).toContainText("passage removed");
  await expect(thread(right, "Should go")).toContainText("passage removed");

  await right.reload();
  await openPanel(right);
  await expect(thread(right, "Should go")).toContainText("drop that");
  await expect(thread(right, "Should go")).toContainText("passage removed");
  await expect(marks(right)).toHaveCount(0);
});

test("a viewer reads comments but writes none, and a comment on the page has no quote", async ({
  left,
  right,
  server,
  member,
}) => {
  await bothOnline(left, right);
  await comment(left, "A remark on the whole page", "page");
  await expect(thread(left, "A remark on the whole page")).toContainText("on the page");

  await grant(server, member, "viewer");
  await expect(status(right)).toHaveText("synced · 2 online · read only");
  await openPanel(right);
  await expect(thread(right, "A remark on the whole page")).toBeVisible();
  await expect(panel(right).getByRole("button", { name: /^comment on/ })).toHaveCount(0);
  await expect(thread(right, "A remark on the whole page").getByRole("button", { name: "reply" })).toHaveCount(0);
  await expect(thread(right, "A remark on the whole page").getByRole("button", { name: "resolve" })).toHaveCount(0);

  // A role given back shows the composer without a reload.
  await grant(server, member, "commenter");
  await expect(panel(right).getByRole("button", { name: "comment on the page" })).toBeVisible();
});

test("a new comment scrolls into view, even when the event's read answers first", async ({ left, server }) => {
  const page = await server.page(DOC);
  for (let index = 1; index <= 12; index += 1) {
    await server.call("POST", `/pages/${page}/comments`, { body: `Older remark ${index}\nwith\nseveral\nlines` });
  }
  await expect(status(left)).toHaveText("synced · 1 online");
  await openPanel(left);
  await expect(thread(left, "Older remark 12")).toHaveCount(1);
  await expect(toggle(left)).toHaveAttribute("title", "comments · 12 open");

  // The read that follows the POST answers once the one the `comment` event starts is under way,
  // so it is dropped as older, and the event's read brings the new thread a moment later.
  let reads = 0;
  await left.route("**/api/v1/pages/*/comments?*", async (route) => {
    if (route.request().method() !== "GET" || reads >= 2) return route.fallback();
    reads += 1;
    await new Promise((done) => setTimeout(done, reads === 1 ? 500 : 1500));
    await route.fallback();
  });
  await comment(left, "The newest remark", "page");
  await expect(thread(left, "The newest remark")).toHaveAttribute("aria-current", "true");
  await expect(thread(left, "The newest remark")).toBeInViewport();
});

test("the panel sits beside the page, and covers its right side in a narrow window", async ({ left }) => {
  await expect(status(left)).toHaveText("synced · 1 online");
  await editor(left).click();
  await left.keyboard.type("A passage worth a remark, and the rest of the line.");

  // A comment left unwritten goes with the panel, and its passage is no longer highlighted.
  await select(left, "worth a remark");
  await openPanel(left);
  await panel(left).getByRole("button", { name: "comment on the selection" }).click();
  await expect(marks(left)).toHaveText("worth a remark");
  await toggle(left).click();
  await expect(marks(left)).toHaveCount(0);
  await toggle(left).click();
  await expect(panel(left).getByRole("button", { name: "comment on the selection" })).toBeVisible();

  await comment(left, "Is it, though?", "selection");
  await thread(left, "Is it, though?").getByRole("button", { name: "reply", exact: true }).click();
  await thread(left, "Is it, though?").getByRole("textbox", { name: "reply" }).fill("It is.");
  await thread(left, "Is it, though?").getByRole("button", { name: "reply", exact: true }).click();
  await expect(thread(left, "Is it, though?")).toContainText("It is.");
  await expect(toggle(left)).toContainText("1");

  const sideways = () => left.evaluate(() => document.documentElement.scrollWidth - innerWidth);
  for (const theme of ["light", "dark"] as const) {
    await left.emulateMedia({ colorScheme: theme });
    const [page, side] = [await left.locator("main.page").boundingBox(), await panel(left).boundingBox()];
    expect(side!.x).toBeGreaterThanOrEqual(page!.x + page!.width - 1);
    expect(await sideways()).toBe(0);
    await left.screenshot({ path: test.info().outputPath(`${theme}-comments.png`) });
  }

  await left.setViewportSize({ width: 900, height: 700 });
  await expect(panel(left)).toHaveCSS("position", "fixed");
  expect(await sideways()).toBe(0);
  await left.screenshot({ path: test.info().outputPath("narrow-comments.png") });
  await panel(left).getByRole("button", { name: "close comments" }).click();
  await expect(panel(left)).toHaveCount(0);
  await expect(toggle(left)).toBeFocused();
});

test("an author edits and deletes their comment", async ({ left, right }) => {
  await bothOnline(left, right);
  await comment(left, "First draft", "page");
  await openPanel(right);
  await expect(thread(right, "First draft")).toBeVisible();

  // Grace is an editor, not an admin: no actions on Ada's comment.
  await expect(thread(right, "First draft").getByRole("button", { name: /^actions on/ })).toHaveCount(0);

  // A refused resolve says why in its thread.
  const refusal = { error: { code: "unavailable", message: "Storage is failing" } };
  await left.route("**/api/v1/comments/*/resolve", (route) => route.fulfill({ status: 503, json: refusal }));
  await thread(left, "First draft").getByRole("button", { name: "resolve", exact: true }).click();
  await expect(thread(left, "First draft").getByRole("alert")).toHaveText("storage is failing");
  await left.unroute("**/api/v1/comments/*/resolve");

  // While it is edited, the body is the field's value, not text: the one thread is found by role.
  const own = panel(left).getByRole("article");
  await own.getByRole("button", { name: "actions on the comment by Ada" }).click();
  await left.getByRole("menuitem", { name: "edit" }).click();
  await expect(own.getByRole("textbox", { name: "edit the comment", exact: true })).toHaveValue("First draft");
  await own.getByRole("textbox", { name: "edit the comment", exact: true }).fill("Second draft");
  await own.getByRole("button", { name: "save", exact: true }).click();
  await expect(thread(right, "Second draft")).toContainText("edited");

  await thread(left, "Second draft").getByRole("button", { name: "actions on the comment by Ada" }).click();
  await left.getByRole("menuitem", { name: "delete" }).click();
  await thread(left, "Second draft").getByRole("button", { name: "delete the thread", exact: true }).click();
  await expect(thread(right, "Second draft")).toHaveCount(0);
  await expect(panel(right).getByText("no open comments")).toBeVisible();
});
