// The blocks of step 17: images and files, uploaded from the slash menu or pasted.

import { expect, type Page } from "@playwright/test";

import { bothOnline, editor, expectText, newLine, test } from "./editing.ts";

/** A 1×1 PNG. */
const PNG = Buffer.from(
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==",
  "base64",
);

const images = (page: Page) => editor(page).locator('figure[data-type="image"] img');
const files = (page: Page) => editor(page).locator('div[data-type="file"] .file-name');

/** Picks the slash command, and answers the file picker it opens. */
async function upload(page: Page, query: string, file: { name: string; mimeType: string; buffer: Buffer }) {
  const chooser = page.waitForEvent("filechooser");
  await page.keyboard.type(`/${query}`);
  await expect(page.getByRole("listbox", { name: "commands" }).getByRole("option")).toHaveCount(1);
  await page.keyboard.press("Enter");
  await (await chooser).setFiles(file);
}

/** Whether the image was served and decoded, not only placed. */
const loaded = (page: Page) =>
  images(page).evaluateAll((all) => all.map((image) => (image as HTMLImageElement).naturalWidth));

test("an image and a file uploaded in one browser show in the other, and download", async ({
  left,
  right,
}) => {
  await bothOnline(left, right);
  await editor(left).click();
  await upload(left, "image", { name: "dot.png", mimeType: "image/png", buffer: PNG });
  await expect(images(right)).toHaveCount(1);
  await expect.poll(() => loaded(right)).toEqual([1]);

  await newLine(left);
  const notes = Buffer.from("meeting notes");
  await upload(left, "file", { name: "notes.txt", mimeType: "text/plain", buffer: notes });
  await expectText(files(right), "notes.txt");
  await expectText(editor(right).locator('div[data-type="file"] .file-size'), "13 B");

  // The reader downloads it with their own session.
  const href = await editor(right).locator('div[data-type="file"] a').getAttribute("href");
  expect(href).toMatch(/^\/api\/v1\/attachments\/[0-9a-f-]+$/);
  const response = await right.request.get(new URL(href!, right.url()).href);
  expect(response.status()).toBe(200);
  expect(await response.body()).toEqual(notes);
  expect(response.headers()["content-disposition"]).toMatch(/^attachment; filename="notes.txt"/);
});

test("a file pasted into the page is uploaded, and an image pasted shows as one", async ({ left, right }) => {
  await bothOnline(left, right);
  await editor(left).click();
  await editor(left).evaluate((target, png) => {
    const data = new DataTransfer();
    data.items.add(new File([new Uint8Array(png)], "pasted.png", { type: "image/png" }));
    data.items.add(new File(["a,b\n1,2\n"], "table.csv", { type: "text/csv" }));
    target.dispatchEvent(new ClipboardEvent("paste", { clipboardData: data, bubbles: true, cancelable: true }));
  }, [...PNG]);

  await expectText(files(right), "table.csv");
  await expectText(editor(right).locator('div[data-type="file"] .file-size'), "8 B");
  // The file went after the image instead of replacing it.
  await expect(images(right)).toHaveCount(1);
  await expect.poll(() => loaded(right)).toEqual([1]);
});
