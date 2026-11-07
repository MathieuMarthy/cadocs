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
  await tree(page).getByRole("button", { name: "new page in pages" }).click();
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
  await tree(page).getByRole("button", { name: "new page in Deploy" }).click();
  await expect(page).toHaveURL(/\?page=/);
  await expect(pageLink(other, "Deploy/")).toBeVisible();
  await tree(other).getByRole("button", { name: "expand Deploy" }).click();
  await expect(pageLink(other, "untitled")).toBeVisible();
});

test("pages reorder by drag and drop, and the trash restores and purges", async ({
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

  await tree(page).getByRole("button", { name: "trash Two" }).click();
  await expect.poll(() => pageLabels(other)).toEqual(["One"]);

  // The member restores it from the trash of the space.
  await tree(other).getByRole("link", { name: "trash of pages" }).click();
  await expect(other).toHaveURL(`${server.url}/w/tests/trash/pages`);
  const trashed = other.getByRole("main").getByRole("listitem").filter({ hasText: "Two" });
  await trashed.getByRole("button", { name: "restore" }).click();
  await expect(trashed).toBeHidden();
  await expect.poll(() => pageLabels(page)).toEqual(["One", "Two"]);

  // Only a space admin purges: the member gets no button, the owner does.
  await tree(page).getByRole("button", { name: "trash One" }).click();
  const gone = other.getByRole("main").getByRole("listitem").filter({ hasText: "One" });
  await expect(gone).toBeVisible();
  await expect(gone.getByRole("button", { name: "purge" })).toHaveCount(0);
  await page.goto(`${server.url}/w/tests/trash/pages`);
  const purged = page.getByRole("main").getByRole("listitem").filter({ hasText: "One" });
  await purged.getByRole("button", { name: "purge" }).click();
  await purged.getByRole("button", { name: "purge for good" }).click();
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
  await expect(space.getByRole("button", { name: "new page in handbook" })).toHaveCount(0);
  await expect(tree(other).getByRole("button", { name: "new page in pages" })).toHaveCount(1);
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
  await expect(tree(other).getByRole("button", { name: "trash Kept" })).toHaveCount(1);
  const { spaceId } = await server.session();
  const grants = [{ user_id: member.id, role: "viewer" }];
  await server.call("PUT", `/spaces/${spaceId}/grants`, { grants });
  await expect(tree(other).getByRole("button", { name: "new page in pages" })).toHaveCount(0);
  await expect(tree(other).getByRole("button", { name: "trash Kept" })).toHaveCount(0);
});
