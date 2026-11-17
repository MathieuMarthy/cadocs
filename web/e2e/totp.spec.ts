// A second factor from an authenticator app (RFC 6238), set up from the account and asked at login.

import { createHmac } from "node:crypto";

import { expect, type Page } from "@playwright/test";

import { asOwner, test } from "./fixtures.ts";
import { PASSWORD, type TestServer } from "./server.ts";

const BASE32 = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

function decodeBase32(text: string) {
  let bits = "";
  for (const char of text.replace(/\s/g, "").toUpperCase()) {
    bits += BASE32.indexOf(char).toString(2).padStart(5, "0");
  }
  const bytes = bits.match(/.{8}/g) ?? [];
  return Buffer.from(bytes.map((byte) => parseInt(byte, 2)));
}

/** What an app shows for `secret` at a time step: an implementation apart from the server's. */
function codeAt(secret: Buffer, step: number) {
  const counter = Buffer.alloc(8);
  counter.writeBigUInt64BE(BigInt(step));
  const digest = createHmac("sha1", secret).update(counter).digest();
  const offset = digest[19] & 0x0f;
  const value = digest.readUInt32BE(offset) & 0x7fffffff;
  return String(value % 1_000_000).padStart(6, "0");
}

const stepNow = () => Math.floor(Date.now() / 30_000);

async function logOut(page: Page) {
  await page.getByRole("button", { name: "Ada", exact: true }).click();
  await page.getByRole("menuitem", { name: "log out" }).click();
}

async function logInWithPassword(page: Page, server: TestServer) {
  await expect(page).toHaveURL(`${server.url}/login`);
  await page.getByLabel("email").fill("ada@example.org");
  await page.getByLabel("password").fill(PASSWORD);
  await page.getByRole("button", { name: "log in" }).click();
}

test("an authenticator app set up in the account makes a login ask for its code", async ({
  page,
  server,
}) => {
  await asOwner(page, server);
  await page.goto(`${server.url}/account`);
  const app = page.getByRole("region", { name: "authenticator app" });
  await expect(app).toContainText("off: after your password");
  await app.getByLabel("current password").fill(PASSWORD);
  await app.getByRole("button", { name: "set up" }).click();
  await expect(app.getByRole("img", { name: "QR code for your app" })).toBeVisible();
  const secret = decodeBase32((await app.locator("code").textContent()) ?? "");
  expect(secret.length).toBe(20);

  // Logins share a budget of 10 password guesses per client: this test spends 9.
  const used = stepNow();
  await app.getByLabel("code from the app").fill(codeAt(secret, used));
  await app.getByRole("button", { name: "turn on" }).click();
  const list = app.getByRole("list", { name: "recovery codes" });
  await expect(list.getByRole("listitem")).toHaveCount(10);
  const recovery = await list.getByRole("listitem").allTextContents();
  await app.getByRole("button", { name: "done" }).click();
  await expect(app).toContainText("on: each login needs a code from your app");

  // A recovery code stands in for the app, once.
  await logOut(page);
  await logInWithPassword(page, server);
  await expect(page.getByRole("status")).toContainText("type the code your authenticator app shows");
  await page.getByLabel("app or recovery code").fill(recovery[0]);
  await page.getByRole("button", { name: "log in" }).click();
  await expect(page).toHaveURL(`${server.url}/w/tests`);

  await logOut(page);
  await logInWithPassword(page, server);
  await page.getByLabel("app or recovery code").fill(recovery[0]);
  await page.getByRole("button", { name: "log in" }).click();
  await expect(page.getByText("wrong")).toBeVisible();
  // The step the setup used is spent: a later code works.
  await page.getByLabel("app or recovery code").fill(codeAt(secret, Math.max(stepNow(), used + 1)));
  await page.getByRole("button", { name: "log in" }).click();
  await expect(page).toHaveURL(`${server.url}/w/tests`);

  await page.goto(`${server.url}/account`);
  await app.getByLabel("current password").fill(PASSWORD);
  await app.getByRole("button", { name: "turn off" }).click();
  await expect(app).toContainText("off: after your password");
});
