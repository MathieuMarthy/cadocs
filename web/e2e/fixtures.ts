// What the browser specs share: a server per test, page errors that fail it, and logins.

import { type Browser, expect, type Page, test as base } from "@playwright/test";

import { type Member, TestServer } from "./server.ts";

export const test = base.extend<{ server: TestServer; errors: Error[] }>({
  server: async ({}, use) => {
    const server = await TestServer.start();
    await use(server);
    await server.dispose();
  },
  errors: [
    async ({ page }, use) => {
      const errors: Error[] = [];
      page.on("pageerror", (error) => errors.push(error));
      await use(errors);
      expect(errors).toEqual([]);
    },
    { auto: true },
  ],
});

/** Logs this browser in with a token, as a person who logged in earlier. */
export async function logIn(page: Page, server: TestServer, token: string) {
  await page.context().addCookies([{ name: "session", value: token, url: server.url }]);
}

export async function asOwner(page: Page, server: TestServer) {
  await logIn(page, server, (await server.session()).token);
}

/** A browser of its own, logged in as `member`. */
export async function memberPage(browser: Browser, server: TestServer, member: Member) {
  const page = await (await browser.newContext()).newPage();
  await logIn(page, server, member.token);
  return page;
}
