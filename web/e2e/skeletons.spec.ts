// Loading skeletons (step 75): a page, the tree and search results show their shape while they
// load, after a short delay, and nothing in them moves.

import { expect, type Locator, type Page } from "@playwright/test";

import { asOwner, test } from "./fixtures.ts";

/** Holds every request to `url` until the returned function is called. */
async function hold(page: Page, url: string) {
  let release = () => {};
  const held = new Promise<void>((resolve) => (release = resolve));
  await page.route(url, async (request) => {
    await held;
    await request.continue();
  });
  return release;
}

/** A skeleton on screen: its bars show, and none of them moves. */
async function expectSkeleton(where: Locator, label: string) {
  await expect(where.getByRole("status")).toHaveText(label);
  await expect(where.locator(".skeleton-bar").first()).toBeVisible();
  const moving = await where
    .locator(".skeleton")
    .evaluate((skeleton) => skeleton.getAnimations({ subtree: true }).length);
  expect(moving).toBe(0);
}

test("a page shows its skeleton while it is read and synced, then its content", async ({
  page,
  server,
}) => {
  const { spaceId } = await server.session();
  const { id } = await server.call("POST", `/spaces/${spaceId}/pages`, {});
  await server.call("PATCH", `/pages/${id}`, { title: "Slow" });
  await server.titleStored(id, "Slow");
  await asOwner(page, server);
  const read = await hold(page, `**/api/v1/pages/${id}`);
  let sync = () => {};
  const synced = new Promise<void>((resolve) => (sync = resolve));
  await page.routeWebSocket(/\/ws\/pages/, (socket) => {
    const upstream = socket.connectToServer();
    upstream.onMessage(async (message) => {
      await synced;
      socket.send(message);
    });
  });

  await page.goto(`${server.url}/?page=${id}`);
  const main = page.getByRole("main");
  await expectSkeleton(main, "loading…");
  await expect(page.getByLabel("page title")).toBeHidden();

  // Read but not synced: the skeleton stays, so no empty page shows in between.
  read();
  await expect(page.getByRole("banner").getByRole("button", { name: "comments" })).toBeVisible();
  await expectSkeleton(main, "loading…");
  await expect(page.getByLabel("page title")).toBeHidden();

  sync();
  await expect(page.getByLabel("page title")).toHaveValue("Slow");
  await expect(main.locator(".skeleton")).toHaveCount(0);
});

test.describe("with reduced motion", () => {
  test.use({ reducedMotion: "reduce" });

  test("the tree shows its skeleton while its spaces load, then the spaces", async ({
    page,
    server,
  }) => {
    await asOwner(page, server);
    const release = await hold(page, "**/api/v1/workspaces/*/spaces*");

    await page.goto(`${server.url}/w/tests`);
    const tree = page.getByRole("navigation", { name: "spaces" });
    await expectSkeleton(tree, "loading…");

    release();
    await expect(tree.getByRole("button", { name: "Pages/", exact: true })).toBeVisible();
    await expect(tree.locator(".skeleton")).toHaveCount(0);
  });
});

test("search results show their skeleton while they load, then the answer", async ({
  page,
  server,
}) => {
  await asOwner(page, server);
  const release = await hold(page, "**/api/v1/workspaces/*/search*");

  await page.goto(`${server.url}/w/tests/search?q=nothing`);
  const main = page.getByRole("main");
  await expectSkeleton(main, "searching…");

  release();
  await expect(main.getByRole("status")).toHaveText("no page matches");
  await expect(main.locator(".skeleton")).toHaveCount(0);
});
