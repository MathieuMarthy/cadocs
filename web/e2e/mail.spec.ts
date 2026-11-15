// What a mail relay turns on (ADR 0026), and what stays hidden without one.

import { expect } from "@playwright/test";

import { test } from "./fixtures.ts";
import { FakeRelay } from "./relay.ts";
import { PASSWORD, TestServer } from "./server.ts";

const NEW_PASSWORD = "another horse battery";

/** A server that sends its mail to a relay the test can read. */
const withMail = test.extend<{ relay: FakeRelay }>({
  relay: async ({}, use) => {
    const relay = await FakeRelay.start();
    await use(relay);
    await relay.dispose();
  },
  server: async ({ relay }, use) => {
    const server = await TestServer.start(relay.env);
    await use(server);
    await server.dispose();
  },
});

withMail("a forgotten password comes back through a mailed link", async ({
  page,
  server,
  relay,
}) => {
  await server.session();
  await page.goto(`${server.url}/login`);
  await page.getByRole("link", { name: "forgot your password?" }).click();
  await expect(page).toHaveURL(`${server.url}/forgot-password`);
  await page.getByLabel("email").fill("Ada@example.org");
  await page.getByRole("button", { name: "send reset link" }).click();
  await expect(page.getByRole("status")).toContainText("a reset link is on its way");

  const message = await relay.message();
  expect(message.to).toBe("ada@example.org");
  const link = new RegExp(`${server.url}/reset-password/[0-9a-f]{64}`).exec(message.text)?.[0];
  expect(link).toBeTruthy();
  await page.goto(link!);
  await page.getByLabel("new password").fill(NEW_PASSWORD);
  await page.getByRole("button", { name: "set password" }).click();
  await expect(page).toHaveURL(`${server.url}/login`);

  await page.getByLabel("email").fill("ada@example.org");
  await page.getByLabel("password").fill(PASSWORD);
  await page.getByRole("button", { name: "log in" }).click();
  await expect(page.getByRole("alert")).toContainText("wrong email or password");
  await page.getByLabel("password").fill(NEW_PASSWORD);
  await page.getByRole("button", { name: "log in" }).click();
  await expect(page).toHaveURL(`${server.url}/w/tests`);
});

withMail("an unknown address reads the same, and gets nothing", async ({ page, server, relay }) => {
  await server.session();
  await page.goto(`${server.url}/forgot-password`);
  await page.getByLabel("email").fill("nobody@example.org");
  await page.getByRole("button", { name: "send reset link" }).click();
  await expect(page.getByRole("status")).toContainText("a reset link is on its way");

  // The one after it would arrive second if the first had been sent.
  await page.getByRole("link", { name: "back to log in" }).click();
  await page.getByRole("link", { name: "forgot your password?" }).click();
  await page.getByLabel("email").fill("ada@example.org");
  await page.getByRole("button", { name: "send reset link" }).click();
  expect((await relay.message()).to).toBe("ada@example.org");
  await page.waitForTimeout(300);
  expect(relay.messages).toHaveLength(1);
});

withMail("a logged-in person is sent home from the forgot password screen", async ({
  page,
  server,
}) => {
  const { token } = await server.session();
  await page.context().addCookies([{ name: "session", value: token, url: server.url }]);
  await page.goto(`${server.url}/forgot-password`);
  await expect(page).toHaveURL(`${server.url}/w/tests`);
});

test("without a relay, login offers no link and the screen does not exist", async ({
  page,
  server,
}) => {
  await server.session();
  await page.goto(`${server.url}/login`);
  await expect(page.getByText("ask an administrator for a reset link")).toBeVisible();
  await expect(page.getByRole("link", { name: "forgot your password?" })).toHaveCount(0);

  await page.goto(`${server.url}/forgot-password`);
  await expect(page.getByRole("heading", { name: "not found" })).toBeVisible();
  await expect(page.getByRole("button", { name: "send reset link" })).toHaveCount(0);
});
