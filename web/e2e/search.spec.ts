// Step 18: search, from what a browser writes to what another finds.

import { expect } from "@playwright/test";

import { editor, test } from "./editing.ts";

test("a page written in one browser is found by its words, and opens from its result", async ({
  left,
  right,
}) => {
  await left.getByLabel("page title").click();
  await left.keyboard.type("Release notes");
  await editor(left).click();
  await left.keyboard.type("How we deployed the servers last week.");

  // `Ctrl K` from any screen of the workspace leads to search, with the field focused.
  // The shortcut belongs to the shell of a workspace, which a page has once it has loaded.
  await expect(right.getByRole("link", { name: "search", exact: true })).toBeVisible();
  await right.keyboard.press("Control+k");
  await expect(right).toHaveURL(/\/w\/[^/]+\/search$/);
  const field = right.getByRole("searchbox", { name: "words" });
  await expect(field).toBeFocused();

  await right.keyboard.type("deploying");
  await expect(right).toHaveURL(/\/search\?q=deploying$/);
  const results = right.getByRole("list", { name: "results" });
  // The index follows an edit after a short delay, so the search is asked again until it does.
  await expect(async () => {
    await right.reload();
    await expect(results.getByRole("link", { name: "Release notes" })).toBeVisible({ timeout: 1000 });
  }).toPass({ timeout: 20_000 });
  await expect(results.locator("mark")).toHaveText("deployed");
  await expect(results.getByText("Pages", { exact: true })).toBeVisible();

  await results.getByRole("link", { name: "Release notes" }).click();
  await expect(right).toHaveURL(/\?page=/);
  await expect(editor(right)).toContainText("How we deployed the servers");
});

test("a search with no result says so, and the sidebar field opens search", async ({ left }) => {
  await left.getByRole("link", { name: "search", exact: true }).click();
  await left.getByRole("searchbox", { name: "words" }).fill("nothing like this");
  await expect(left.getByRole("main").getByRole("status")).toHaveText("no page matches");
});
