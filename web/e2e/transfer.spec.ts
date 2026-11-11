// Step 20: a Markdown file imported from a space's menu, exported from the page's menu, and the
// page printed alone.

import { expect, type Page } from "@playwright/test";

import { asOwner, test } from "./fixtures.ts";

const tree = (page: Page) => page.getByRole("navigation", { name: "spaces" });

// Ends on a paragraph: after a final list, the editor adds one (`TrailingNode`, docs/notes/web.md).
const MARKDOWN = "# Runbook\n\nRestart the **worker** first.\n\n- [ ] check the logs\n\nThen wait.\n";

test("a markdown file imports from the space menu and exports back from the page menu", async ({
  page,
  server,
}) => {
  await asOwner(page, server);
  await page.goto(`${server.url}/w/tests`);
  const actions = tree(page).getByRole("button", {
    name: "more actions for Pages",
    exact: true,
    includeHidden: true,
  });
  await actions.locator("xpath=ancestor::div[@role='group'][1]").hover();
  await actions.click();
  const chooser = page.waitForEvent("filechooser");
  await page.getByRole("menuitem", { name: "import markdown" }).click();
  await (await chooser).setFiles({
    name: "runbook.md",
    mimeType: "text/markdown",
    buffer: Buffer.from(MARKDOWN),
  });

  await expect(page).toHaveURL(/\?page=[^&]+$/);
  await expect(page.getByLabel("page title")).toHaveValue("Runbook");
  await expect(page.locator(".tiptap strong")).toHaveText("worker");
  await expect(tree(page).getByRole("link", { name: "Runbook", exact: true })).toBeVisible();

  await page.getByRole("banner").getByRole("button", { name: "more actions for Runbook" }).click();
  const download = page.waitForEvent("download");
  await page.getByRole("menuitem", { name: "export as markdown" }).click();
  const file = await download;
  expect(file.suggestedFilename()).toBe("Runbook.md");
  const stream = await file.createReadStream();
  const chunks: Buffer[] = [];
  for await (const chunk of stream) chunks.push(chunk as Buffer);
  expect(Buffer.concat(chunks).toString()).toBe(MARKDOWN);
});

test("printing shows the page alone", async ({ page, server }) => {
  await asOwner(page, server);
  const id = await server.page("printed");
  await page.goto(`${server.url}/?page=${id}`);
  await expect(page.getByLabel("page title")).toBeVisible();
  await page.emulateMedia({ media: "print" });
  await expect(page.getByRole("complementary")).toBeHidden();
  await expect(page.getByRole("banner")).toBeHidden();
  await expect(page.getByLabel("page title")).toBeVisible();
});
