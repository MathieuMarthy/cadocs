// The sidebar tree, the trash and recent pages, in two real browsers kept in step by the events
// socket.

import { type Browser, expect, type Page } from "@playwright/test";

import { asOwner, memberPage, test } from "./fixtures.ts";
import type { TestServer } from "./server.ts";

const tree = (page: Page) => page.getByRole("navigation", { name: "spaces" });
const pageLink = (page: Page, label: string) =>
  tree(page).getByRole("link", { name: label, exact: true });
const spaceButton = (page: Page, name: string) =>
  tree(page).getByRole("button", { name, exact: true });

/** The labels of the page links of the tree, in order. */
const pageLabels = (page: Page) => tree(page).locator("a[href^='/?page=']").allTextContents();

/** A button of a tree row, there even while the row's actions are out of layout. */
const rowButton = (page: Page, name: string) =>
  tree(page).getByRole("button", { name, exact: true, includeHidden: true });

/** Points at a tree row, which shows its actions, and clicks one of them. */
async function press(page: Page, name: string) {
  const button = rowButton(page, name);
  await button.locator("xpath=ancestor::div[@role='group'][1]").hover();
  await button.click();
}

/** Opens the action menu of a tree row and picks `action`, with the mouse. */
async function pick(page: Page, name: string, action: string) {
  await press(page, `more actions for ${name}`);
  await page.getByRole("menuitem", { name: action, exact: true }).click();
}

/** A page at the root of the first space, with a title, created over REST. */
async function newPage(server: TestServer, title: string, parent_id?: string) {
  const { spaceId } = await server.session();
  const page = await server.call("POST", `/spaces/${spaceId}/pages`, { parent_id });
  await server.call("PATCH", `/pages/${page.id}`, { title });
  return page.id as string;
}

/** The owner and a member, both on the workspace home with the first space open. */
async function twoBrowsers(page: Page, server: TestServer, browser: Browser) {
  const member = await server.addMember("grace@example.org");
  await asOwner(page, server);
  const other = await memberPage(browser, server, member);
  for (const each of [page, other]) {
    await each.goto(`${server.url}/w/tests`);
    await spaceButton(each, "pages/").click();
  }
  return { member, other };
}

test("two browsers see each other's tree changes", async ({ page, server, browser }) => {
  const { other } = await twoBrowsers(page, server, browser);
  await expect(tree(other).getByText("no pages")).toBeVisible();

  // A new page opens, and the other browser sees it arrive, then its title.
  await press(page, "new page in pages");
  await expect(page).toHaveURL(/\?page=/);
  const runbooks = new URL(page.url()).searchParams.get("page")!;
  await expect(pageLink(other, "untitled")).toBeVisible();
  await server.call("PATCH", `/pages/${runbooks}`, { title: "Runbooks" });
  await expect(pageLink(other, "Runbooks")).toBeVisible();

  await page.goBack();
  const deploy = await newPage(server, "Deploy");
  await expect(pageLink(page, "Deploy")).toBeVisible();
  await expect(pageLink(other, "Deploy")).toBeVisible();

  // Dropped in the middle of a row, a page goes under it.
  await pageLink(page, "Deploy").dragTo(pageLink(page, "Runbooks"));
  await expect(pageLink(other, "Runbooks/")).toBeVisible();
  await expect(pageLink(other, "Deploy")).toBeHidden();
  expect((await server.call("GET", `/pages/${deploy}`)).parent_id).toBe(runbooks);
  await tree(other).getByRole("button", { name: "expand Runbooks" }).click();
  await expect(pageLink(other, "Deploy")).toBeVisible();

  // A child page made from the tree lands under its parent, for both. The drop opened Runbooks.
  await press(page, "new page in Deploy");
  await expect(page).toHaveURL(/\?page=/);
  await expect(pageLink(other, "Deploy/")).toBeVisible();
  await tree(other).getByRole("button", { name: "expand Deploy" }).click();
  await expect(pageLink(other, "untitled")).toBeVisible();
});

test("pages reorder by drag and drop, and the trash restores and deletes forever", async ({
  page,
  server,
  browser,
}) => {
  const one = await newPage(server, "One");
  await newPage(server, "Two");
  const { other } = await twoBrowsers(page, server, browser);
  await expect.poll(() => pageLabels(other)).toEqual(["One", "Two"]);

  // Dropped on the top edge of a row, a page goes before it.
  await pageLink(page, "Two").dragTo(pageLink(page, "One"), { targetPosition: { x: 4, y: 1 } });
  await expect.poll(() => pageLabels(other)).toEqual(["Two", "One"]);
  // And on the bottom edge, after it.
  const box = (await pageLink(page, "One").boundingBox())!;
  await pageLink(page, "Two").dragTo(pageLink(page, "One"), {
    targetPosition: { x: 4, y: box.height - 1 },
  });
  await expect.poll(() => pageLabels(other)).toEqual(["One", "Two"]);

  await pick(page, "Two", "move to trash");
  await expect.poll(() => pageLabels(other)).toEqual(["One"]);

  // The member restores it from the trash of the space.
  await pick(other, "pages", "open the trash");
  await expect(other).toHaveURL(`${server.url}/w/tests/trash/pages`);
  const trashed = other.getByRole("main").getByRole("listitem").filter({ hasText: "Two" });
  await trashed.getByRole("button", { name: "restore" }).click();
  await expect(trashed).toBeHidden();
  await expect.poll(() => pageLabels(page)).toEqual(["One", "Two"]);

  // Only a space admin deletes forever: the member gets no button, the owner does.
  await pick(page, "One", "move to trash");
  const gone = other.getByRole("main").getByRole("listitem").filter({ hasText: "One" });
  await expect(gone).toBeVisible();
  await expect(gone.getByRole("button", { name: "delete forever" })).toHaveCount(0);
  await page.goto(`${server.url}/w/tests/trash/pages`);
  const purged = page.getByRole("main").getByRole("listitem").filter({ hasText: "One" });
  await purged.getByRole("button", { name: "delete forever" }).click();
  await purged.getByRole("button", { name: "delete forever" }).click();
  await expect(purged).toBeHidden();
  await expect(gone).toBeHidden();
  await expect(server.call("GET", `/pages/${one}`)).rejects.toThrow(/404/);
});

test("a new space reaches the readers it is granted to", async ({ page, server, browser }) => {
  const { member, other } = await twoBrowsers(page, server, browser);

  await page.getByLabel("space name").fill("Handbook");
  await page.getByRole("button", { name: "+ new space" }).click();
  await expect(spaceButton(page, "handbook/")).toBeVisible();
  await expect(spaceButton(other, "handbook/")).toBeHidden();
  // A member cannot create a space, so the form is not there for them.
  await expect(other.getByLabel("space name")).toHaveCount(0);

  const { workspaceId } = await server.session();
  const spaces = await server.call("GET", `/workspaces/${workspaceId}/spaces`);
  const handbook = spaces.items.find((space: { name: string }) => space.name === "Handbook");
  const grants = [{ user_id: member.id, role: "viewer" }];
  await server.call("PUT", `/spaces/${handbook.id}/grants`, { grants });
  await expect(spaceButton(other, "handbook/")).toBeVisible();

  // A viewer neither creates nor trashes pages there.
  await spaceButton(other, "handbook/").click();
  const space = tree(other).getByRole("listitem").filter({
    has: other.getByRole("button", { name: "handbook/", exact: true }),
  });
  await expect(space.getByText("no pages")).toBeVisible();
  await expect(tree(other).getByText("no pages")).toHaveCount(2);
  const create = (name: string) => ({ name, includeHidden: true });
  await expect(space.getByRole("button", create("new page in handbook"))).toHaveCount(0);
  await expect(tree(other).getByRole("button", create("new page in pages"))).toHaveCount(1);
});

test("recent pages follow changes, and the tree survives a server restart", async ({
  page,
  server,
  browser,
}) => {
  const id = await newPage(server, "Notes");
  const { other } = await twoBrowsers(page, server, browser);
  const recent = other.getByRole("region", { name: "recent pages" });
  await expect(recent.getByRole("link", { name: "Notes" })).toBeVisible();
  await server.call("PATCH", `/pages/${id}`, { title: "Meeting notes" });
  await expect(recent.getByRole("link", { name: "Meeting notes" })).toBeVisible();

  await server.restart();
  // The socket comes back on its own, so a later change still arrives.
  await newPage(server, "After restart");
  await expect(pageLink(other, "After restart")).toBeVisible();
  await expect(recent.getByRole("link", { name: "After restart" })).toBeVisible();
});

test("the tree shows over REST even when the events socket never opens", async ({
  page,
  server,
}) => {
  await newPage(server, "Offline");
  await page.routeWebSocket(/\/ws\/events/, (socket) => socket.close({ code: 1011 }));
  await asOwner(page, server);
  await page.goto(`${server.url}/w/tests`);
  await spaceButton(page, "pages/").click();
  await expect(pageLink(page, "Offline")).toBeVisible();
  const recent = page.getByRole("region", { name: "recent pages" });
  await expect(recent.getByRole("link", { name: "Offline" })).toBeVisible();
});

test("a demoted editor loses the page actions of the tree", async ({ page, server, browser }) => {
  await newPage(server, "Kept");
  const { member, other } = await twoBrowsers(page, server, browser);
  await pick(other, "Kept", "copy link");
  await expect(other.getByRole("menuitem", { name: "move to trash" })).toHaveCount(0);
  await press(other, "more actions for Kept");
  await expect(other.getByRole("menuitem", { name: "move to trash" })).toHaveCount(1);
  await other.keyboard.press("Escape");
  await expect(other.getByRole("menu")).toHaveCount(0);
  const { spaceId } = await server.session();
  const grants = [{ user_id: member.id, role: "viewer" }];
  await server.call("PUT", `/spaces/${spaceId}/grants`, { grants });
  await expect(rowButton(other, "new page in pages")).toHaveCount(0);
  await press(other, "more actions for Kept");
  await expect(other.getByRole("menuitem", { name: "copy link" })).toBeVisible();
  await expect(other.getByRole("menuitem", { name: "move to trash" })).toHaveCount(0);
});

test("a page moves to trash from its menu by keyboard alone, and undo brings it back", async ({
  page,
  server,
}) => {
  await newPage(server, "Engineering handbook");
  await asOwner(page, server);
  await page.goto(`${server.url}/w/tests`);
  await spaceButton(page, "pages/").click();
  const link = pageLink(page, "Engineering handbook");
  await expect(link).toBeVisible();
  // The space has no number in front of it any more.
  await expect(tree(page).getByText(/^01/)).toHaveCount(0);

  // Out of hover and focus, the actions take no width: the label reaches the end of its row.
  await page.mouse.move(800, 400);
  const fit = await link.evaluate((label) => {
    const row = label.closest(".tree-row")!;
    const end = row.getBoundingClientRect().right - parseFloat(getComputedStyle(row).paddingRight);
    return { gap: end - label.getBoundingClientRect().right, cut: label.scrollWidth > label.clientWidth };
  });
  expect(Math.abs(fit.gap)).toBeLessThan(1);
  expect(fit.cut).toBe(false);

  // Tab reaches the row's buttons, Escape closes the menu and gives focus back to its button.
  const more = tree(page).getByRole("button", { name: "more actions for Engineering handbook" });
  await link.focus();
  await page.keyboard.press("Tab");
  await expect(tree(page).getByRole("button", { name: "new page in Engineering handbook" })).toBeFocused();
  await page.keyboard.press("Tab");
  await expect(more).toBeFocused();
  await expect(more).toHaveAttribute("title", "more actions");
  await page.keyboard.press("Enter");
  const menu = page.getByRole("menu");
  await expect(menu.getByRole("menuitem").first()).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(menu).toHaveCount(0);
  await expect(more).toBeFocused();

  // Arrows move through the menu, and Enter picks.
  await page.keyboard.press("Enter");
  await page.keyboard.press("ArrowDown");
  const trash = menu.getByRole("menuitem", { name: "move to trash" });
  await expect(trash).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(link).toBeHidden();
  const notice = page.getByRole("status").filter({ hasText: "moved to trash" });
  await expect(notice).toBeVisible();
  const undo = notice.getByRole("button", { name: "undo" });
  await expect(undo).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(link).toBeVisible();
  await expect(notice).toBeHidden();
});

test("a page opens beside the tree, selected in it", async ({ page, server }) => {
  await newPage(server, "Guide");
  await asOwner(page, server);
  await page.goto(`${server.url}/w/tests`);
  const sidebar = page.getByRole("complementary");
  await expect(sidebar.getByText("settings", { exact: true })).toBeVisible();
  await spaceButton(page, "pages/").click();

  await pageLink(page, "Guide").click();
  await expect(page).toHaveURL(/\?page=/);
  await expect(page.getByLabel("page title")).toHaveValue("Guide");
  await expect(pageLink(page, "Guide")).toHaveAttribute("aria-current", "page");
  const crumbs = page.getByRole("navigation", { name: "location" });
  await expect(crumbs).toHaveText(/tests\s*\/\s*pages\s*\/\s*Guide/);

  // The way back to the workspace stays in the sidebar.
  await sidebar.getByRole("link", { name: "home" }).click();
  await expect(page).toHaveURL(`${server.url}/w/tests`);
  await expect(pageLink(page, "Guide")).not.toHaveAttribute("aria-current", "page");
});

test("a page whose first read fails waits to know its workspace", async ({ page, server }) => {
  const other = await server.call("POST", "/workspaces", { name: "Second" });
  const space = await server.call("POST", `/workspaces/${other.id}/spaces`, { name: "Notes" });
  const created = await server.call("POST", `/spaces/${space.id}/pages`, {});
  await server.call("PATCH", `/pages/${created.id}`, { title: "Elsewhere" });
  await asOwner(page, server);
  let failures = 2;
  await page.route(`**/api/v1/pages/${created.id}`, (request) =>
    failures-- > 0 ? request.abort() : request.continue(),
  );
  await page.goto(`${server.url}/?page=${created.id}`);
  const crumbs = page.getByRole("navigation", { name: "location" });
  // Never the first workspace's shell, then its own once a read answers.
  await expect(page.getByLabel("page content")).toBeVisible();
  await expect(crumbs).not.toContainText("tests");
  await expect(crumbs).toHaveText(/second\s*\/\s*notes\s*\/\s*Elsewhere/);
  await expect(tree(page).getByRole("button", { name: "notes/", exact: true })).toBeVisible();
});

test("the breadcrumb follows a page moved to another space", async ({ page, server }) => {
  const id = await newPage(server, "Wanderer");
  const { workspaceId } = await server.session();
  const target = await server.call("POST", `/workspaces/${workspaceId}/spaces`, { name: "Archive" });
  await asOwner(page, server);
  await page.goto(`${server.url}/?page=${id}`);
  const crumbs = page.getByRole("navigation", { name: "location" });
  await expect(crumbs).toHaveText(/pages\s*\/\s*Wanderer/);
  await server.call("POST", `/pages/${id}/move`, { space_id: target.id });
  await expect(crumbs).toHaveText(/archive\s*\/\s*Wanderer/);
});

test("a page trashed while open closes its socket", async ({ page, server }) => {
  const id = await newPage(server, "Doomed");
  await asOwner(page, server);
  let open = 0;
  page.on("websocket", (socket) => {
    if (!socket.url().includes("/ws/pages/")) return;
    open += 1;
    socket.on("close", () => (open -= 1));
  });
  await page.goto(`${server.url}/?page=${id}`);
  await expect(page.getByRole("banner").getByRole("status")).toContainText("synced");
  await server.call("DELETE", `/pages/${id}`);
  await expect(page.getByRole("heading", { name: /page (not found|deleted)/ })).toBeVisible();
  await expect.poll(() => open).toBe(0);
});

test("a page trashed while open comes back with undo, and the notice then goes", async ({
  page,
  server,
}) => {
  const id = await newPage(server, "Open one");
  await newPage(server, "Other");
  await asOwner(page, server);
  await page.goto(`${server.url}/?page=${id}`);
  await expect(page.getByRole("banner").getByRole("status")).toContainText("synced");
  await spaceButton(page, "pages/").click();

  await pick(page, "Open one", "move to trash");
  await expect(page.getByRole("heading", { name: "page deleted" })).toBeVisible();
  const notice = page.getByRole("status").filter({ hasText: "moved to trash" });
  await notice.getByRole("button", { name: "undo" }).click();
  await expect(page.getByLabel("page title")).toHaveValue("Open one");

  // Focus given to undo does not hold the notice: it leaves on its own.
  await pick(page, "Other", "move to trash");
  await page.mouse.move(800, 400);
  await expect(notice).toBeVisible();
  await expect(notice).toBeHidden({ timeout: 15_000 });
});
