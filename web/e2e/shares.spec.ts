// Step 45: a public page under `/s/{token}` runs no script, neither one written in its content nor
// one injected into the page it serves.

import { expect, type Page } from "@playwright/test";

import { test } from "./fixtures.ts";
import type { TestServer } from "./server.ts";

const ATTACKS = [
  "# Public",
  "",
  "<script>window.pwned = 'script'</script>",
  "",
  `<img src="/nothing" onerror="window.pwned = 'image'">`,
  "",
  "[click me](javascript:window.pwned='link')",
  "",
].join("\n");

/** What injected code would set: still unset when nothing ran. */
const pwned = (page: Page) => page.evaluate(() => (window as { pwned?: string }).pwned);

/** The public address of a new page imported from `markdown`. */
async function shared(server: TestServer, markdown: string) {
  const { token, csrf, spaceId } = await server.session();
  const response = await fetch(`${server.url}/api/v1/spaces/${spaceId}/import?name=public.md`, {
    method: "POST",
    headers: {
      "content-type": "text/markdown",
      cookie: `session=${token}`,
      "x-csrf-token": csrf,
    },
    body: markdown,
  });
  expect(response.status).toBe(201);
  const page = await response.json();
  const share = await server.call("POST", `/pages/${page.id}/shares`, {});
  return `${server.url}/s/${share.token}`;
}

test("a script written in content does not run on a public page", async ({ page, server }) => {
  const url = await shared(server, ATTACKS);
  const dialogs: string[] = [];
  page.on("dialog", (dialog) => {
    dialogs.push(dialog.message());
    void dialog.dismiss();
  });

  // A visitor with no session.
  await page.goto(url);
  await expect(page.getByRole("heading", { name: "Public", level: 1 })).toBeVisible();
  await expect(page.getByText("<script>window.pwned = 'script'</script>")).toBeVisible();
  await page.getByText("click me").click();
  expect(await page.locator("script").count()).toBe(0);
  expect(await pwned(page)).toBeUndefined();
  expect(dialogs).toEqual([]);
});

const INJECTED =
  `<script>window.pwned = "inline"</script>` +
  `<img src="/nothing" onerror="window.pwned = 'handler'">` +
  `<a id="injected" href="javascript:window.pwned='link'">injected</a></main>`;

/** Serves the real answer with scripts added, and its headers kept or the policy dropped. */
async function inject(page: Page, url: string, keepPolicy: boolean) {
  await page.route(url, async (route) => {
    const response = await route.fetch();
    const headers = response.headers();
    if (!keepPolicy) delete headers["content-security-policy"];
    const body = (await response.text()).replace("</main>", INJECTED);
    await route.fulfill({ response, headers, body });
  });
}

test("the policy of a public page blocks a script injected into it", async ({ page, server }) => {
  const url = await shared(server, "# Public\n\nHello\n");
  const blocked: string[] = [];
  page.on("console", (message) => {
    if (message.type() === "error" && message.text().includes("Content Security Policy")) {
      blocked.push(message.text());
    }
  });

  await inject(page, url, true);
  await page.goto(url);
  await expect(page.getByText("Hello")).toBeVisible();
  await page.locator("#injected").click();
  expect(await pwned(page)).toBeUndefined();
  await expect.poll(() => blocked.length).toBeGreaterThanOrEqual(2);
});

test("the same injection runs once the policy is gone, so the test above means something", async ({
  page,
  server,
}) => {
  const url = await shared(server, "# Public\n\nHello\n");
  await inject(page, url, false);
  await page.goto(url);
  await expect(page.getByText("Hello")).toBeVisible();
  await expect.poll(() => pwned(page)).toBeDefined();
});
