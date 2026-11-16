// What a mail relay turns on (ADR 0026), and what stays hidden without one.

import { expect } from "@playwright/test";

import { asOwner, test } from "./fixtures.ts";
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

test("without a relay, login offers no link and the screens do not exist", async ({
  page,
  server,
}) => {
  await server.session();
  await page.goto(`${server.url}/login`);
  await expect(page.getByText("ask an administrator for a reset link")).toBeVisible();
  await expect(page.getByRole("link", { name: "forgot your password?" })).toHaveCount(0);

  await expect(page.getByRole("link", { name: "create an account" })).toHaveCount(0);

  await page.goto(`${server.url}/forgot-password`);
  await expect(page.getByRole("heading", { name: "not found" })).toBeVisible();
  await expect(page.getByRole("button", { name: "send reset link" })).toHaveCount(0);
  await page.goto(`${server.url}/register`);
  await expect(page.getByRole("heading", { name: "not found" })).toBeVisible();
  await expect(page.getByRole("button", { name: "send code" })).toHaveCount(0);

  await asOwner(page, server);
  await page.goto(`${server.url}/account`);
  await expect(page.getByRole("heading", { name: "delete account" })).toBeVisible();
  await expect(page.getByRole("region", { name: "second factor" })).toHaveCount(0);
});

withMail("a code read from the mail makes an account with no workspace", async ({
  page,
  server,
  relay,
}) => {
  await server.session();
  await page.goto(`${server.url}/login`);
  await page.getByRole("link", { name: "create an account" }).click();
  await expect(page).toHaveURL(`${server.url}/register`);
  await page.getByLabel("email").fill("Grace@example.org");
  await page.getByRole("button", { name: "send code" }).click();
  await expect(page.getByRole("status")).toContainText("a code is on its way");

  const message = await relay.message();
  expect(message.to).toBe("grace@example.org");
  const code = /^ {4}(\d{6})$/m.exec(message.text)?.[1];
  expect(code).toBeTruthy();
  const wrong = String((Number(code) + 1) % 1_000_000).padStart(6, "0");

  await page.getByLabel("code").fill(wrong);
  await page.getByLabel("name").fill("Grace");
  await page.getByLabel("password").fill(NEW_PASSWORD);
  await page.getByRole("button", { name: "create account" }).click();
  await expect(page.getByRole("alert")).toHaveText("wrong");
  await page.getByLabel("code").fill(code!);
  await page.getByRole("button", { name: "create account" }).click();
  await expect(page.getByRole("heading", { name: "no workspace" })).toBeVisible();
  await expect(page).toHaveURL(`${server.url}/`);
});

withMail("a new code can be asked for from the code form", async ({ page, server, relay }) => {
  await server.session();
  await page.goto(`${server.url}/register`);
  await page.getByLabel("email").fill("grace@example.org");
  await page.getByRole("button", { name: "send code" }).click();
  await relay.message();
  await page.getByRole("button", { name: "send a new code" }).click();
  await expect(page.getByLabel("email")).toHaveValue("grace@example.org");
  await page.getByRole("button", { name: "send code" }).click();
  expect((await relay.message(2)).to).toBe("grace@example.org");
});

const closed = withMail.extend({
  server: async ({ relay }, use) => {
    const server = await TestServer.start({ ...relay.env, CADOCS_REGISTRATION: "closed" });
    await use(server);
    await server.dispose();
  },
});

closed("a closed instance offers recovery and no registration", async ({ page, server }) => {
  await server.session();
  await page.goto(`${server.url}/login`);
  await expect(page.getByRole("link", { name: "forgot your password?" })).toBeVisible();
  await expect(page.getByRole("link", { name: "create an account" })).toHaveCount(0);
  await page.goto(`${server.url}/register`);
  await expect(page.getByRole("heading", { name: "not found" })).toBeVisible();
  await expect(page.getByRole("button", { name: "send code" })).toHaveCount(0);
});

withMail("a logged-in person is sent home from the registration screen", async ({
  page,
  server,
}) => {
  const { token } = await server.session();
  await page.context().addCookies([{ name: "session", value: token, url: server.url }]);
  await page.goto(`${server.url}/register`);
  await expect(page).toHaveURL(`${server.url}/w/tests`);
});

withMail("a second factor turned on in the account makes a login ask for a mailed code", async ({
  page,
  server,
  relay,
}) => {
  await asOwner(page, server);
  await page.goto(`${server.url}/account`);
  const factor = page.getByRole("region", { name: "second factor" });
  await factor.getByRole("button", { name: "turn on" }).click();
  await expect(factor.getByRole("status")).toContainText("a code is on its way");
  const code = /^ {4}(\d{6})$/m.exec((await relay.message()).text)?.[1];
  expect(code).toBeTruthy();
  await factor.getByLabel("code").fill(code!);
  await factor.getByRole("button", { name: "turn on" }).click();
  await expect(factor).toContainText("on: each login needs a code");

  await page.getByRole("button", { name: "Ada", exact: true }).click();
  await page.getByRole("menuitem", { name: "log out" }).click();
  await expect(page).toHaveURL(`${server.url}/login`);
  await page.getByLabel("email").fill("ada@example.org");
  await page.getByLabel("password").fill(PASSWORD);
  await page.getByRole("button", { name: "log in" }).click();
  await expect(page.getByRole("status")).toContainText("a code is on its way to ada@example.org");
  await expect(page).toHaveURL(`${server.url}/login`);

  const login = /^ {4}(\d{6})$/m.exec((await relay.message(2)).text)?.[1];
  expect(login).toBeTruthy();
  const wrong = String((Number(login) + 1) % 1_000_000).padStart(6, "0");
  await page.getByLabel("code").fill(wrong);
  await page.getByRole("button", { name: "log in" }).click();
  await expect(page.getByText("wrong")).toBeVisible();
  await page.getByLabel("code").fill(login!);
  await page.getByRole("button", { name: "log in" }).click();
  await expect(page).toHaveURL(`${server.url}/w/tests`);

  await page.goto(`${server.url}/account`);
  await factor.getByLabel("current password").fill(PASSWORD);
  await factor.getByRole("button", { name: "turn off" }).click();
  await expect(factor).toContainText("off: after your password");
});
