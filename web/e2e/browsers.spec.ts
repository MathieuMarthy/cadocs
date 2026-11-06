// Two separate browsers on the embedded front end of the real binary.

import { type Browser, expect, type Page, test as base } from "@playwright/test";

import { WebsocketProvider } from "y-websocket";
import * as Y from "yjs";

import { TestServer } from "./server.ts";

const DOC = "shared";

const test = base.extend<{
  serverEnv: Record<string, string>;
  server: TestServer;
  left: Page;
  right: Page;
}>({
  serverEnv: [{}, { option: true }],
  server: async ({ serverEnv }, use) => {
    const server = await TestServer.start(serverEnv);
    await use(server);
    await server.dispose();
  },
  left: async ({ playwright, server }, use) => {
    await withPage(await playwright.chromium.launch(), server, use);
  },
  right: async ({ playwright, server }, use) => {
    await withPage(await playwright.chromium.launch(), server, use);
  },
});

function collectErrors(page: Page) {
  const errors: Error[] = [];
  page.on("pageerror", (error) => errors.push(error));
  return errors;
}

async function withPage(browser: Browser, server: TestServer, use: (page: Page) => Promise<void>) {
  let errors: Error[] = [];
  try {
    const page = await browser.newPage();
    errors = collectErrors(page);
    await page.goto(`${server.url}/?doc=${DOC}`);
    await use(page);
  } finally {
    await browser.close();
  }
  expect(errors).toEqual([]);
}

const editor = (page: Page) => page.getByLabel("page content");
const title = (page: Page) => page.getByLabel("page title");
const status = (page: Page) => page.getByRole("status");
const paragraphs = (page: Page) => editor(page).locator("p");

async function expectOnline(page: Page, count: number) {
  await expect(status(page)).toHaveText(`synced · ${count} online`);
}

/** The text of each paragraph, without the name shown on a remote caret. */
function bodyLines(page: Page) {
  return paragraphs(page).evaluateAll((lines) =>
    lines.map((line) => {
      const copy = line.cloneNode(true) as HTMLElement;
      copy.querySelectorAll(".remote-caret").forEach((caret) => caret.remove());
      return copy.textContent;
    }),
  );
}

async function expectBody(page: Page, lines: string[]) {
  await expect.poll(() => bodyLines(page)).toEqual(lines);
}

function avatarNames(page: Page) {
  return page
    .locator(".avatar")
    .evaluateAll((avatars) => avatars.map((avatar) => (avatar as HTMLElement).title));
}

/** Two lines on both sides, so that the next edits land in different paragraphs. */
async function writeTwoLines(left: Page, right: Page) {
  await expectOnline(left, 2);
  await expectOnline(right, 2);
  await editor(left).click();
  await left.keyboard.type("first");
  await left.keyboard.press("Enter");
  await left.keyboard.type("second");
  await expectBody(right, ["first", "second"]);
}

test("two browsers edit one document at the same time, with visible cursors", async ({
  left,
  right,
}) => {
  await writeTwoLines(left, right);

  await paragraphs(left).first().click();
  await left.keyboard.press("End");
  await paragraphs(right).last().click();
  await right.keyboard.press("End");

  await Promise.all([
    left.keyboard.type(" from left", { delay: 15 }),
    right.keyboard.type(" from right", { delay: 15 }),
  ]);

  await expectBody(left, ["first from left", "second from right"]);
  await expectBody(right, ["first from left", "second from right"]);

  // Each page shows the other's caret, on the line that person is editing.
  await expect(paragraphs(left).last().locator(".remote-caret")).toBeVisible();
  await expect(paragraphs(right).first().locator(".remote-caret")).toBeVisible();
  await expect(left.locator(".remote-caret")).toHaveCount(1);
  await expect(right.locator(".remote-caret")).toHaveCount(1);
  await expect(left.locator(".avatar")).toHaveCount(2);
  expect(await avatarNames(left)).toContain(await left.locator(".remote-caret-label").textContent());
  expect(await avatarNames(right)).toContain(
    await right.locator(".remote-caret-label").textContent(),
  );
});

test("the title follows, in the input and in the tab", async ({ left, right }) => {
  await expectOnline(right, 2);

  await title(left).click();
  await left.keyboard.type("Shared notes");

  await expect(title(right)).toHaveValue("Shared notes");
  await expect(right).toHaveTitle("Shared notes · cadocs");
});

test("presence follows a third person coming and going", async ({
  left,
  right,
  server,
  playwright,
}) => {
  await writeTwoLines(left, right);
  const browser = await playwright.chromium.launch();
  try {
    const third = await browser.newPage();
    const errors = collectErrors(third);
    await third.goto(`${server.url}/?doc=${DOC}`);

    await expectOnline(left, 3);
    await expect(left.locator(".avatar")).toHaveCount(3);
    expect(errors).toEqual([]);
  } finally {
    await browser.close();
  }

  await expectOnline(left, 2);
  await expect(left.locator(".avatar")).toHaveCount(2);
});

for (const signal of ["SIGTERM", "SIGKILL"] as const) {
  test(`content survives a server restart after ${signal}`, async ({ left, right, server }) => {
    await writeTwoLines(left, right);
    await title(left).fill("Before the restart");
    await expect(title(right)).toHaveValue("Before the restart");

    await server.stop(signal);
    await expect(status(left)).toHaveText("offline, retrying");
    await expect(status(right)).toHaveText("offline, retrying");

    // Typed with no server at all, so only the page holds this.
    await paragraphs(left).last().click();
    await left.keyboard.press("End");
    await left.keyboard.type(" offline");

    await server.run();
    await expectOnline(left, 2);
    await expectOnline(right, 2);
    await expectBody(right, ["first", "second offline"]);

    await paragraphs(right).first().click();
    await right.keyboard.press("End");
    await right.keyboard.type(" online");
    await expectBody(left, ["first online", "second offline"]);

    await left.reload();
    await right.reload();
    await expectOnline(left, 2);
    await expectBody(left, ["first online", "second offline"]);
    await expectBody(right, ["first online", "second offline"]);
    await expect(title(left)).toHaveValue("Before the restart");
    await expect(right).toHaveTitle("Before the restart · cadocs");
  });
}

test("content comes back from the disk with no browser left to resend it", async ({
  left,
  right,
  server,
}) => {
  await writeTwoLines(left, right);
  await title(left).fill("On disk");
  await expect(title(right)).toHaveValue("On disk");

  await server.stop("SIGTERM");
  const browser = left.context().browser()!;
  await left.close();
  await right.close();
  await server.run();

  const page = await browser.newPage();
  const errors = collectErrors(page);
  await page.goto(`${server.url}/?doc=${DOC}`);
  await expectOnline(page, 1);
  await expectBody(page, ["first", "second"]);
  await expect(title(page)).toHaveValue("On disk");
  expect(errors).toEqual([]);
});

test.describe("with a send queue of one frame", () => {
  test.use({ serverEnv: { CADOCS_SEND_QUEUE: "1" } });

  test("a page closed as too slow (4408) reconnects on its own", async ({ left, server }) => {
    await expectOnline(left, 1);
    let reconnected = false;
    left.once("websocket", () => (reconnected = true));

    const doc = new Y.Doc();
    const writer = new WebsocketProvider(server.wsUrl, DOC, doc, { disableBc: true });
    try {
      await expect.poll(() => writer.synced).toBe(true);
      const filler = "x".repeat(2_000);
      for (let round = 0; round < 200 && !reconnected; round++) {
        for (let index = 0; index < 50; index++) doc.getText("filler").insert(0, filler);
        await new Promise((done) => setTimeout(done, 5));
      }
      await expect.poll(() => reconnected).toBe(true);

      await expect(status(left)).toHaveText(/^synced/);
      doc.getText("title").insert(0, "After the close");
      await expect(title(left)).toHaveValue("After the close");
    } finally {
      writer.destroy();
      doc.destroy();
    }
  });
});
