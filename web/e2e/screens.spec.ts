// The screens inside the shell: home, page, trash, account and administration (step 12c).

import { expect, type Page } from "@playwright/test";

import { asOwner, memberPage, test } from "./fixtures.ts";
import { PASSWORD, type TestServer } from "./server.ts";

const tree = (page: Page) => page.getByRole("navigation", { name: "spaces" });
const crumbs = (page: Page) => page.getByRole("navigation", { name: "location" });
const notice = (page: Page) => page.locator(".notice-region");
const adminRow = (page: Page, email: string) =>
  page.getByRole("main").getByRole("listitem").filter({ hasText: email });

/** A page at the root of the first space, with a title, created over REST. */
async function newPage(server: TestServer, title: string) {
  const { spaceId } = await server.session();
  const page = await server.call("POST", `/spaces/${spaceId}/pages`, {});
  await server.call("PATCH", `/pages/${page.id}`, { title });
  return page.id as string;
}

/** The width a screen scrolls sideways by, which must be none. */
const sideways = (page: Page) =>
  page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);

test("home lists recent pages with their space and a relative date, with no role line", async ({
  page,
  server,
}) => {
  await newPage(server, "Notes");
  await asOwner(page, server);
  await page.goto(`${server.url}/w/tests`);
  const recent = page.getByRole("region", { name: "recent pages" });
  const row = recent.getByRole("listitem").filter({ hasText: "Notes" });
  await expect(row.getByRole("link", { name: "Notes", exact: true })).toBeVisible();
  await expect(row).toContainText("Pages");
  const when = row.locator("time");
  await expect(when).toHaveText(/^(just now|\d+ (seconds?|minutes?) ago)$/);
  // The exact date is there on hover.
  expect(await when.getAttribute("title")).toMatch(/\d/);
  expect(Date.parse((await when.getAttribute("datetime"))!)).not.toBeNaN();
  await expect(page.getByRole("main")).not.toContainText("role");
  // The new space form moved to the sidebar.
  await expect(page.getByRole("main").getByLabel("space name")).toHaveCount(0);
});

test("an empty space offers a new page, and an empty workspace a new space", async ({
  page,
  server,
}) => {
  await asOwner(page, server);
  await page.goto(`${server.url}/w/tests`);
  const main = page.getByRole("main");
  await expect(main).toContainText("no pages yet");
  await main.getByRole("button", { name: "new page" }).dblclick();
  await expect(page).toHaveURL(/\?page=/);
  const { spaceId } = await server.session();
  expect((await server.call("GET", `/spaces/${spaceId}/tree`)).items).toHaveLength(1);
  await expect(page.getByLabel("page title")).toBeVisible();

  const empty = await server.call("POST", "/workspaces", { name: "Empty" });
  await page.goto(`${server.url}/w/${empty.slug}`);
  await expect(main).toContainText("no spaces yet");
  await main.getByRole("button", { name: "new space" }).click();
  // Escape closes the form and gives focus back to its button.
  await page.getByLabel("space name").press("Escape");
  await expect(main.getByRole("button", { name: "new space" })).toBeFocused();
  await main.getByRole("button", { name: "new space" }).click();
  await page.getByLabel("space name").fill("Handbook");
  await page.getByLabel("space name").press("Enter");
  await expect(tree(page).getByRole("button", { name: "Handbook/", exact: true })).toBeVisible();
  await expect(main).toContainText("no pages yet");
});

test("a member without spaces is told who can add one", async ({ server, browser }) => {
  const member = await server.addMember("grace@example.org");
  const { spaceId } = await server.session();
  await server.call("PUT", `/spaces/${spaceId}/grants`, { grants: [] });
  const other = await memberPage(browser, server, member);
  await other.goto(`${server.url}/w/tests`);
  const main = other.getByRole("main");
  await expect(main).toContainText("no spaces yet");
  await expect(main).toContainText("a workspace admin");
  await expect(main.getByRole("button", { name: "new space" })).toHaveCount(0);
  await expect(tree(other).getByRole("button", { name: "new space" })).toHaveCount(0);
});

test("a new space starts from the plus beside the spaces label", async ({ page, server }) => {
  await asOwner(page, server);
  await page.goto(`${server.url}/w/tests`);
  await tree(page).getByRole("button", { name: "new space" }).click();
  const field = tree(page).getByLabel("space name");
  await expect(field).toBeFocused();
  // Escape gives the plus back.
  await field.press("Escape");
  await expect(field).toHaveCount(0);
  await tree(page).getByRole("button", { name: "new space" }).click();
  await field.fill("Runbooks");
  await field.press("Enter");
  await expect(tree(page).getByRole("button", { name: "Runbooks/", exact: true })).toBeVisible();
  await expect(field).toHaveCount(0);
});

test("a page title wraps, and presence shows from two people", async ({
  page,
  server,
  browser,
}) => {
  const id = await newPage(server, "A title long enough to need a second line on a page column");
  await asOwner(page, server);
  await page.setViewportSize({ width: 900, height: 800 });
  await page.goto(`${server.url}/?page=${id}`);
  const title = page.getByLabel("page title");
  await expect(title).toHaveValue(/^A title long/);
  // It grows instead of offering a resize handle.
  await expect(title).toHaveCSS("resize", "none");
  const box = (await title.boundingBox())!;
  const line = await title.evaluate((element) => parseFloat(getComputedStyle(element).lineHeight));
  expect(box.height).toBeGreaterThan(line * 1.5);
  expect(await title.evaluate((element) => element.scrollHeight - element.clientHeight)).toBe(0);
  expect(await sideways(page)).toBe(0);

  const status = page.getByRole("banner").getByRole("status");
  await expect(status).toHaveText("synced · 1 online");
  await expect(page.locator(".avatar")).toHaveCount(0);
  const other = await (await browser.newContext()).newPage();
  await asOwner(other, server);
  await other.goto(`${server.url}/?page=${id}`);
  await expect(status).toHaveText("synced · 2 online");
  await expect(page.locator(".avatar")).toHaveCount(2);
  await other.close();
  await expect(status).toHaveText("synced · 1 online");
  await expect(page.locator(".avatar")).toHaveCount(0);
});

test("the top bar menu of a page copies its link and moves it to trash", async ({
  page,
  server,
}) => {
  const id = await newPage(server, "Kept");
  await page.context().grantPermissions(["clipboard-read", "clipboard-write"], {
    origin: server.url,
  });
  await asOwner(page, server);
  await page.goto(`${server.url}/?page=${id}`);
  const banner = page.getByRole("banner");
  await banner.getByRole("button", { name: "more actions for Kept" }).click();
  await page.getByRole("menuitem", { name: "copy link" }).click();
  await expect(notice(page)).toContainText("link copied");
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(
    `${server.url}/?page=${id}`,
  );

  await banner.getByRole("button", { name: "more actions for Kept" }).click();
  await page.getByRole("menuitem", { name: "move to trash" }).click();
  await expect(notice(page)).toContainText("moved to trash");
  await expect(page).toHaveURL(`${server.url}/w/tests`);
  await notice(page).getByRole("button", { name: "undo" }).click();
  await expect(page.getByLabel("page title")).toHaveValue("Kept");
});

test("a viewer's page menu has no move to trash", async ({ server, browser }) => {
  const id = await newPage(server, "Read only");
  const member = await server.addMember("grace@example.org");
  const { spaceId } = await server.session();
  await server.call("PUT", `/spaces/${spaceId}/grants`, {
    grants: [{ user_id: member.id, role: "viewer" }],
  });
  const other = await memberPage(browser, server, member);
  await other.goto(`${server.url}/?page=${id}`);
  await other.getByRole("banner").getByRole("button", { name: "more actions for Read only" }).click();
  await expect(other.getByRole("menuitem", { name: "copy link" })).toBeVisible();
  await expect(other.getByRole("menuitem", { name: "move to trash" })).toHaveCount(0);
});

test("the trash names its space in the breadcrumb, in plain words", async ({ page, server }) => {
  await asOwner(page, server);
  await page.goto(`${server.url}/w/tests/trash/pages`);
  await expect(page.getByRole("heading", { name: "trash", exact: true })).toBeVisible();
  await expect(crumbs(page)).toHaveText(/^\s*Tests\s*\/\s*Pages\s*\/\s*trash\s*$/);
  await expect(page.getByRole("main")).toContainText("restoring a page also restores its subpages");
  await expect(page.getByRole("main")).toContainText("the trash is empty");
});

test("the account has a profile form without password, and a password form", async ({
  page,
  server,
}) => {
  await asOwner(page, server);
  await page.goto(`${server.url}/account`);
  const profile = page.getByRole("form", { name: "profile" });
  const login = page.getByRole("form", { name: "email and password" });
  await expect(profile.getByLabel("current password")).toHaveCount(0);
  await profile.getByLabel("name", { exact: true }).fill("Ada Lovelace");
  await profile.getByRole("button", { name: "save" }).click();
  await expect(profile.getByRole("status")).toContainText("saved");
  await expect(page.getByRole("complementary")).toContainText("Ada Lovelace");

  // A current password alone changes nothing, and says so.
  await login.getByLabel("current password").fill(PASSWORD);
  await login.getByRole("button", { name: "save" }).click();
  await expect(login.getByRole("alert")).toContainText("nothing to save");
  await expect(login.getByRole("status")).toHaveCount(0);
  await login.getByLabel("current password").fill("");

  // A new email needs the current password, asked in the second form only.
  await login.getByLabel("email").fill("ada@example.net");
  await login.getByRole("button", { name: "save" }).click();
  await expect(login.getByRole("alert")).toBeVisible();
  await login.getByLabel("current password").fill(PASSWORD);
  await login.getByRole("button", { name: "save" }).click();
  await expect(login.getByRole("status")).toContainText("saved");
  await expect(login.getByLabel("current password")).toHaveValue("");

  // Deleting the account is a final action, in danger.
  const remove = page.getByRole("button", { name: "delete account" });
  await expect(remove).toHaveClass(/danger/);
  await remove.click();
  await expect(page.getByRole("button", { name: "delete account" })).toHaveClass(/danger/);
  await page.getByRole("button", { name: "cancel" }).click();
});

test("administration offers each user a menu, and the current admin no self actions", async ({
  page,
  server,
}) => {
  await server.addMember("grace@example.org");
  await page.context().grantPermissions(["clipboard-read", "clipboard-write"], {
    origin: server.url,
  });
  await asOwner(page, server);
  await page.goto(`${server.url}/admin`);

  const self = adminRow(page, "ada@example.org");
  await expect(self).toContainText("(you)");
  await expect(self.locator(".badge")).toHaveText(["admin"]);
  await self.getByRole("button", { name: "more actions for Ada" }).click();
  const menu = page.getByRole("menu");
  await expect(menu.getByRole("menuitem", { name: "new reset link" })).toBeVisible();
  for (const name of ["remove admin", "disable", "delete"]) {
    await expect(menu.getByRole("menuitem", { name })).toHaveCount(0);
  }
  await page.keyboard.press("Escape");

  const row = adminRow(page, "grace@example.org");
  await expect(row).not.toContainText("(you)");
  await expect(row.locator(".badge")).toHaveCount(0);
  const act = async (name: string) => {
    await row.getByRole("button", { name: "more actions for Grace" }).click();
    await page.getByRole("menuitem", { name, exact: true }).click();
  };
  await act("make admin");
  await expect(row.locator(".badge")).toHaveText(["admin"]);
  await act("disable");
  await expect(row.locator(".badge")).toHaveText(["admin", "disabled"]);
  await act("enable");
  await act("remove admin");
  await expect(row.locator(".badge")).toHaveCount(0);

  await act("new reset link");
  const link = await row.getByLabel("one-time link").inputValue();
  await row.getByRole("button", { name: "copy" }).click();
  await expect(notice(page)).toContainText("link copied");
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(link);

  // Cancelling the deletion, with the button or Escape, gives focus back to the menu.
  const more = row.getByRole("button", { name: "more actions for Grace" });
  await act("delete");
  await row.getByRole("button", { name: "cancel" }).click();
  await expect(more).toBeFocused();
  await act("delete");
  await page.keyboard.press("Escape");
  await expect(row.getByRole("button", { name: "cancel" })).toHaveCount(0);
  await expect(more).toBeFocused();

  // Delete asks once more, in danger.
  await act("delete");
  const confirm = row.getByRole("button", { name: "delete", exact: true });
  await expect(confirm).toHaveClass(/danger/);
  await confirm.click();
  await expect(adminRow(page, "grace@example.org")).toHaveCount(0);
});

for (const theme of ["light", "dark"] as const) {
  test(`every screen shows in the ${theme} theme`, async ({ page, server }) => {
    const id = await newPage(server, "Notes");
    await page.addInitScript((value) => localStorage.setItem("cadocs-theme", value), theme);
    await page.emulateMedia({ colorScheme: theme === "light" ? "dark" : "light" });
    await asOwner(page, server);
    const screens: [string, string][] = [
      ["/w/tests", "Tests"],
      [`/?page=${id}`, ""],
      ["/w/tests/trash/pages", "trash"],
      ["/account", "account"],
      ["/admin", "admin"],
    ];
    const backgrounds = new Set<string>();
    for (const [path, heading] of screens) {
      await page.goto(`${server.url}${path}`);
      if (heading) await expect(page.getByRole("heading", { name: heading, exact: true })).toBeVisible();
      else await expect(page.getByLabel("page title")).toHaveValue("Notes");
      await expect(page.locator("html")).toHaveAttribute("data-theme", theme);
      expect(await sideways(page)).toBe(0);
      // A wide window has no drawer, so neither of its buttons.
      await expect(page.getByRole("button", { name: /^(open|close) the sidebar$/ })).toHaveCount(0);
      backgrounds.add(await page.evaluate(() => getComputedStyle(document.body).backgroundColor));
      const name = path.replace(/[^a-z]+/g, "-").replace(/^-|-$/g, "") || "page";
      await page.screenshot({ path: test.info().outputPath(`${theme}-${name}.png`), fullPage: true });
    }
    // One background per theme, whatever the system says.
    expect(backgrounds.size).toBe(1);
    const [background] = backgrounds;
    const dark = theme === "dark";
    const channels = background!.match(/\d+/g)!.slice(0, 3).map(Number);
    const light = channels.reduce((sum, value) => sum + value, 0) / 3 > 128;
    expect(light).toBe(!dark);
  });
}

test("a menu opened with the mouse marks no item, and with the keyboard its first", async ({
  page,
  server,
}) => {
  await newPage(server, "Notes");
  await asOwner(page, server);
  await page.goto(`${server.url}/w/tests`);
  const button = tree(page).getByRole("button", { name: "more actions for Pages" });
  await tree(page).getByRole("button", { name: "Pages/", exact: true }).hover();
  await button.click();
  const menu = page.getByRole("menu");
  await expect(menu).toBeFocused();
  await expect(menu.locator(":focus")).toHaveCount(0);
  // The arrows still reach the items.
  await page.keyboard.press("ArrowDown");
  await expect(menu.getByRole("menuitem").first()).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(button).toBeFocused();

  await page.keyboard.press("Enter");
  await expect(menu.getByRole("menuitem").first()).toBeFocused();
});
