// Step 20: a Markdown file imported from a space's menu, exported from the page's menu, and the
// page printed alone.

import { crc32, deflateRawSync } from "node:zlib";

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

test("a page duplicates from the page menu into another space, with its subpages", async ({
  page,
  server,
}) => {
  await asOwner(page, server);
  const { spaceId, workspaceId } = await server.session();
  const source = await server.page("runbook");
  await server.rename("runbook", "Runbook");
  const child = await server.call("POST", `/spaces/${spaceId}/pages`, { parent_id: source });
  await server.call("PATCH", `/pages/${child.id}`, { title: "Steps" });
  const archive = await server.call("POST", `/workspaces/${workspaceId}/spaces`, {
    name: "Archive",
  });
  await server.titleStored(child.id, "Steps");

  await page.goto(`${server.url}/?page=${source}`);
  await expect(page.getByLabel("page title")).toHaveValue("Runbook");
  await page.getByRole("banner").getByRole("button", { name: "more actions for Runbook" }).click();
  await page.getByRole("menuitem", { name: "duplicate" }).click();
  await page.getByLabel("copy into").selectOption({ label: "Archive" });
  await page.getByRole("button", { name: "duplicate", exact: true }).click();

  await expect(page).not.toHaveURL(new RegExp(source));
  await expect(page.getByLabel("page title")).toHaveValue("Runbook");
  const copy = new URL(page.url()).searchParams.get("page");
  expect((await server.call("GET", `/pages/${copy}`)).space_id).toBe(archive.id);
  const below = await server.call("GET", `/spaces/${archive.id}/tree?parent=${copy}`);
  expect(below.items.map((row: { title: string }) => row.title)).toEqual(["Steps"]);
});

/** A deflated zip, as desktop tools make them. */
function zip(files: Record<string, string>) {
  const entries: Buffer[] = [];
  const directory: Buffer[] = [];
  let offset = 0;
  for (const [name, text] of Object.entries(files)) {
    const data = deflateRawSync(text);
    // Version, flags, method, time, date, CRC, both sizes, name and extra lengths.
    const fields = Buffer.alloc(26);
    fields.writeUInt16LE(20, 0);
    fields.writeUInt16LE(8, 4);
    fields.writeUInt32LE(crc32(text), 10);
    fields.writeUInt32LE(data.length, 14);
    fields.writeUInt32LE(Buffer.byteLength(text), 18);
    fields.writeUInt16LE(Buffer.byteLength(name), 22);
    const local = Buffer.concat([Buffer.from("PK\x03\x04"), fields, Buffer.from(name), data]);
    const at = Buffer.alloc(14);
    at.writeUInt32LE(offset, 10);
    directory.push(
      Buffer.concat([Buffer.from("PK\x01\x02\x14\x00"), fields, at, Buffer.from(name)]),
    );
    entries.push(local);
    offset += local.length;
  }
  const records = Buffer.concat(directory);
  const end = Buffer.alloc(22);
  end.write("PK\x05\x06");
  end.writeUInt16LE(directory.length, 8);
  end.writeUInt16LE(directory.length, 10);
  end.writeUInt32LE(records.length, 12);
  end.writeUInt32LE(offset, 16);
  return Buffer.concat([...entries, records, end]);
}

test("a zip of pages imports under the page picked in the space menu", async ({ page, server }) => {
  await asOwner(page, server);
  const { spaceId } = await server.session();
  const target = await server.page("target");
  await server.rename("target", "Target");
  const child = await server.call("POST", `/spaces/${spaceId}/pages`, { parent_id: target });
  await server.call("PATCH", `/pages/${child.id}`, { title: "Inbox" });
  await server.titleStored(child.id, "Inbox");

  await page.goto(`${server.url}/w/tests`);
  const actions = tree(page).getByRole("button", {
    name: "more actions for Pages",
    exact: true,
    includeHidden: true,
  });
  await actions.locator("xpath=ancestor::div[@role='group'][1]").hover();
  await actions.click();
  await page.getByRole("menuitem", { name: "import a zip of pages" }).click();
  await page.getByLabel("zip of markdown files").setInputFiles({
    name: "notes.zip",
    mimeType: "application/zip",
    buffer: zip({ "Alpha.md": "One.\n", "Alpha/Beta.md": "Two.\n" }),
  });
  await page.getByLabel("import under").selectOption({ label: "Target" });
  await page.getByLabel("subpage").selectOption({ label: "Inbox" });
  await page.getByRole("button", { name: "import", exact: true }).click();

  await expect(page.getByText("2 pages imported")).toBeVisible();
  await expect(page.getByLabel("zip of markdown files")).toBeHidden();
  const under = await server.call("GET", `/spaces/${spaceId}/tree?parent=${child.id}`);
  expect(under.items).toHaveLength(1);
  await expect(tree(page).getByRole("link", { name: "Alpha/", exact: true })).toBeVisible();
});
