// What the editor specs share: two browsers on one page, and how to read and drive the editor.

import { type Browser, expect, type Locator, type Page, test as base } from "@playwright/test";

import { logIn } from "./fixtures.ts";
import { type Member, TestServer } from "./server.ts";

export const DOC = "blocks";

export const test = base.extend<{
  server: TestServer;
  member: Member;
  /** The owner, Ada. */
  left: Page;
  /** Grace, an editor of the space. */
  right: Page;
}>({
  server: async ({}, use) => {
    const server = await TestServer.start();
    await use(server);
    await server.dispose();
  },
  member: async ({ server }, use) => {
    await use(await server.addMember("grace@example.org"));
  },
  left: async ({ playwright, server }, use) => {
    const token = (await server.session()).token;
    await withPage(await playwright.chromium.launch(), server, token, use);
  },
  right: async ({ playwright, server, member }, use) => {
    await withPage(await playwright.chromium.launch(), server, member.token, use);
  },
});

export async function withPage(
  browser: Browser,
  server: TestServer,
  token: string,
  use: (page: Page) => Promise<void>,
) {
  const errors: Error[] = [];
  try {
    const page = await browser.newPage();
    page.on("pageerror", (error) => errors.push(error));
    await logIn(page, server, token);
    await page.goto(`${server.url}/?page=${await server.page(DOC)}`);
    await use(page);
  } finally {
    await browser.close();
  }
  expect(errors).toEqual([]);
}

export const editor = (page: Page) => page.getByLabel("page content");
export const status = (page: Page) => page.getByRole("banner").getByRole("status");
export const commands = (page: Page) => page.getByRole("listbox", { name: "commands" });

/**
 * The text of each match, without the names shown on remote carets. A fresh browser may load
 * the page twice (`docs/notes/web.md`), so a read that the reload cut short counts as no text.
 */
export function texts(locator: Locator) {
  return locator
    .evaluateAll((elements) =>
      elements.map((element) => {
        const copy = element.cloneNode(true) as HTMLElement;
        copy.querySelectorAll(".remote-caret").forEach((caret) => caret.remove());
        return copy.textContent;
      }),
    )
    .catch(() => null);
}

export async function expectText(locator: Locator, ...expected: string[]) {
  await expect.poll(() => texts(locator)).toEqual(expected);
}

export async function bothOnline(left: Page, right: Page) {
  await expect(status(left)).toHaveText("synced · 2 online");
  await expect(status(right)).toHaveText("synced · 2 online");
}

/** Starts a new line at the end of the content, so each block lands on its own. */
export async function newLine(page: Page) {
  // No click: under load, a click could place the caret after Control+End has run.
  await page.keyboard.press("Control+End");
  await page.keyboard.press("Enter");
}

/** Picks the one command the query finds, once the menu shows it alone. */
export async function command(page: Page, query: string) {
  await page.keyboard.type(`/${query}`);
  await expect(commands(page).getByRole("option")).toHaveCount(1);
  await page.keyboard.press("Enter");
  await expect(commands(page)).toHaveCount(0);
}

/** The grant of the member on the test space. */
export async function grant(server: TestServer, member: Member, role: string) {
  const { spaceId } = await server.session();
  await server.call("PUT", `/spaces/${spaceId}/grants`, { grants: [{ user_id: member.id, role }] });
}
