// The blocks of step 15: callouts, toggles, tables, links to pages and the table of contents.

import { expect, type Page } from "@playwright/test";

import {
  bothOnline,
  command,
  commands,
  editor,
  expectText,
  grant,
  newLine,
  status,
  test,
} from "./editing.ts";

const callout = (page: Page) => editor(page).locator('div[data-type="callout"]');
const toggle = (page: Page) => editor(page).locator('div[data-type="details"]');
const contents = (page: Page) => editor(page).getByRole("navigation", { name: "table of contents" });
const pages = (page: Page) => page.getByRole("listbox", { name: "pages" });

test("a callout reaches the other browser, and so does a change of its kind", async ({ left, right }) => {
  await bothOnline(left, right);
  await editor(left).click();
  await command(left, "callout");
  await left.keyboard.type("Back up first.");
  await expectText(callout(right).locator("p"), "Back up first.");
  await expect(callout(right)).toHaveAttribute("data-kind", "note");

  await callout(left).getByLabel("callout kind").selectOption("warning");
  await expect(callout(right)).toHaveAttribute("data-kind", "warning");
  await expect(callout(right).getByLabel("callout kind")).toHaveValue("warning");
  // The text stayed where it was.
  await expectText(callout(right).locator("p"), "Back up first.");
});

test("a toggle's summary and content reach the other browser, which opens it on its own", async ({
  left,
  right,
}) => {
  await bothOnline(left, right);
  await editor(left).click();
  await command(left, "toggle");
  await left.keyboard.type("Details");
  await expectText(toggle(right).locator("summary"), "Details");

  await toggle(left).getByRole("button", { name: "open" }).click();
  await toggle(left).locator('div[data-type="detailsContent"] p').click();
  await left.keyboard.type("Hidden text");

  // Opening is a view of each tab, not an edit: the other browser keeps it closed.
  const content = toggle(right).locator('div[data-type="detailsContent"]');
  await expect(content).toBeHidden();
  await toggle(right).getByRole("button", { name: "open" }).click();
  await expect(content).toBeVisible();
  await expectText(content.locator("p"), "Hidden text");
  await expect(toggle(left).getByRole("button", { name: "close" })).toBeVisible();
});

test("a table is filled and grows in one browser, and the other follows", async ({ left, right }) => {
  await bothOnline(left, right);
  await editor(left).click();
  // `table` also finds the table of contents, below it.
  await left.keyboard.type("/table");
  await expect(commands(left).getByRole("option")).toHaveText(["table", "table of contents"]);
  await left.keyboard.press("Enter");
  await left.keyboard.type("Name");
  await left.keyboard.press("Tab");
  await left.keyboard.type("Role");
  await expectText(editor(right).locator("table th"), "Name", "Role", "");
  await expect(editor(right).locator("table tr")).toHaveCount(3);

  // Table commands only show inside a table.
  await left.keyboard.press("Tab");
  await left.keyboard.press("Tab");
  await left.keyboard.type("Ada");
  // A slash opens the menu only after a space or in an empty cell.
  await left.keyboard.press("Tab");
  await command(left, "add row below");
  await expect(editor(right).locator("table tr")).toHaveCount(4);
  await command(left, "add column right");
  await expect(editor(right).locator("table tr").first().locator("th")).toHaveCount(4);
  await command(left, "delete column");
  await expect(editor(right).locator("table tr").first().locator("th")).toHaveCount(3);
  await expectText(editor(right).locator("table td").first(), "Ada");

  await command(left, "delete table");
  await expect(editor(right).locator("table")).toHaveCount(0);
});

test("table commands are not offered outside a table", async ({ left }) => {
  await expect(status(left)).toContainText("synced");
  await editor(left).click();
  await left.keyboard.type("/add");
  await expect(commands(left).getByRole("option")).toHaveCount(0);
  await expect(commands(left)).toContainText("no match");
  // A space after a query that finds nothing ends the menu: the slash was text.
  await left.keyboard.type(" row");
  await expect(commands(left)).toHaveCount(0);
  await left.keyboard.press("Enter");
  await expectText(editor(left).locator("p").first(), "/add row");
});

test("a link to a page is picked by its title, shows its title, and opens it", async ({
  left,
  right,
  server,
}) => {
  const target = await server.page("target");
  await server.call("PATCH", `/pages/${target}`, { title: "Deploy notes" });
  await bothOnline(left, right);
  await editor(left).click();
  await left.keyboard.type("See ");
  await left.keyboard.type("[[depl");
  await expect(pages(left).getByRole("option")).toHaveText(["Deploy notes"]);
  await left.keyboard.press("Enter");
  await expect(pages(left)).toHaveCount(0);
  await left.keyboard.type("for more.");

  const link = editor(right).locator("a.page-link").first();
  await expect(link).toHaveText("Deploy notes");
  await expect(link).toHaveAttribute("href", `/?page=${target}`);
  await expectText(editor(right).locator("p").first(), "See Deploy notes for more.");

  // From the slash menu too.
  await newLine(left);
  await command(left, "link to page");
  await left.keyboard.type("deploy");
  await expect(pages(left).getByRole("option")).toHaveText(["Deploy notes"]);
  await left.keyboard.press("Enter");
  await expect(editor(right).locator("a.page-link")).toHaveCount(2);

  // A writer's click opens the page in this tab only, not in a new one too.
  await link.click();
  await expect(right).toHaveURL(new RegExp(`\\?page=${target}$`));
  await expect.poll(() => right.context().pages().length).toBe(1);
  await expect(right.getByLabel("page title")).toHaveValue("Deploy notes");
});

test("the arrows still move the caret while the page picker finds nothing", async ({ left }) => {
  await expect(status(left)).toContainText("synced");
  await editor(left).click();
  await left.keyboard.type("first");
  await left.keyboard.press("Enter");
  await left.keyboard.type("[[zzz");
  await expect(pages(left)).toContainText("no match");
  await left.keyboard.press("ArrowUp");
  await left.keyboard.type("X");
  await expectText(editor(left).locator("p").first(), "firstX");
});

test("a link to a page the reader cannot open does not show its title", async ({
  left,
  right,
  server,
}) => {
  // A page in a space with no default role, where Grace has no grant.
  const { workspaceId } = await server.session();
  const space = await server.call("POST", `/workspaces/${workspaceId}/spaces`, { name: "Private" });
  const secret = await server.call("POST", `/spaces/${space.id}/pages`, {});
  await server.call("PATCH", `/pages/${secret.id}`, { title: "Salaries" });
  await bothOnline(left, right);
  await editor(left).click();
  await left.keyboard.type("[[sal");
  await expect(pages(left).getByRole("option")).toHaveText(["Salaries"]);
  await left.keyboard.press("Enter");

  await expect(editor(left).locator("a.page-link")).toHaveText("Salaries");
  await expect(editor(right).locator("a.page-link")).toHaveText("page not found");
});

test("the table of contents lists the headings with their numbers, and follows edits", async ({
  left,
  right,
}) => {
  await bothOnline(left, right);
  await editor(left).click();
  await command(left, "contents");
  await expect(contents(right)).toContainText("no headings yet");

  await newLine(left);
  await command(left, "h2");
  await left.keyboard.type("Alpha");
  await newLine(left);
  await command(left, "h3");
  await left.keyboard.type("Beta");
  await expect(contents(right).getByRole("link")).toHaveText(["01 Alpha", "01.1 Beta"]);

  await newLine(left);
  await command(left, "h2");
  await left.keyboard.type("Gamma");
  await expect(contents(right).getByRole("link")).toHaveText(["01 Alpha", "01.1 Beta", "02 Gamma"]);
  await expect(contents(left).getByRole("link")).toHaveText(["01 Alpha", "01.1 Beta", "02 Gamma"]);

  await contents(right).getByRole("link", { name: "02 Gamma" }).click();
  await expect(editor(right).locator("h2").last()).toBeInViewport();
});

test("a reader opens toggles and follows the contents, but cannot change a callout", async ({
  left,
  right,
  server,
  member,
}) => {
  await bothOnline(left, right);
  await editor(left).click();
  await command(left, "callout");
  await left.keyboard.type("Read me");
  // The paragraph the editor keeps after a final block.
  await editor(left).locator(":scope > p").last().click();
  await command(left, "toggle");
  await left.keyboard.type("More");
  await expectText(toggle(right).locator("summary"), "More");

  await grant(server, member, "viewer");
  await expect(status(right)).toContainText("read only");
  await expect(callout(right).getByLabel("callout kind")).toBeDisabled();
  await expect(callout(left).getByLabel("callout kind")).toBeEnabled();
  await toggle(right).getByRole("button", { name: "open" }).click();
  await expect(toggle(right).getByRole("button", { name: "close" })).toBeVisible();
});
