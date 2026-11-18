// Step 45: a public page under `/s/{token}` runs no script, neither one written in its content nor
// one injected into the page it serves. Step 46: the share dialog of a page lists, creates, copies
// and revokes its public links, for editors only.

import { expect, type Page } from "@playwright/test";

import { grant } from "./editing.ts";
import { asOwner, memberPage, test } from "./fixtures.ts";
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

interface ShareRow {
  token: string;
  include_children: boolean;
  indexable: boolean;
  expires_at: number | null;
}

const dialog = (page: Page) => page.getByRole("dialog", { name: "public links" });
const links = (page: Page) => dialog(page).getByRole("list", { name: "public links" }).getByRole("listitem");

/** Opens the share dialog from the top bar menu of a page that has no title. */
async function openShares(page: Page) {
  await page.getByRole("banner").getByRole("button", { name: "more actions for untitled" }).click();
  await page.getByRole("menuitem", { name: "share publicly" }).click();
  await expect(dialog(page)).toBeVisible();
}

/** `YYYY-MM-DD` of the day `days` from today, in local time, as a date field takes it. */
function day(days: number) {
  const date = new Date();
  date.setDate(date.getDate() + days);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
}

test("an editor creates a public link with its options, copies it and revokes it", async ({
  page,
  server,
}) => {
  const id = await server.page("shared");
  await page.context().grantPermissions(["clipboard-read", "clipboard-write"], { origin: server.url });
  await asOwner(page, server);
  await page.goto(`${server.url}/?page=${id}`);
  await openShares(page);
  await expect(dialog(page).getByText("no public links yet")).toBeVisible();

  await dialog(page).getByRole("checkbox", { name: "include subpages" }).check();
  await dialog(page).getByRole("checkbox", { name: "let search engines index it" }).check();
  await dialog(page).getByLabel("valid until (optional)").fill(day(7));
  await dialog(page).getByRole("button", { name: "create link" }).click();
  await expect(links(page)).toHaveCount(1);

  const [share] = (await server.call("GET", `/pages/${id}/shares`)).items as ShareRow[];
  expect(share.include_children).toBe(true);
  expect(share.indexable).toBe(true);
  // The link works through the day picked, so it expires as the next one starts.
  const [year, month, date] = day(7).split("-").map(Number);
  expect(share.expires_at).toBe(new Date(year, month - 1, date + 1).getTime());

  const url = `${server.url}/s/${share.token}`;
  const row = links(page).first();
  await expect(row).toContainText(url);
  await expect(row).toContainText("with subpages");
  await expect(row).toContainText("indexed by search engines");
  await expect(row).toContainText(`until ${new Date(year, month - 1, date).toLocaleDateString("en-US")}`);
  await row.getByRole("button", { name: "copy the public link" }).click();
  await expect(dialog(page).getByRole("status")).toHaveText("link copied");
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(url);
  expect((await fetch(url)).status).toBe(200);

  await row.getByRole("button", { name: "revoke", exact: true }).click();
  await row.getByRole("button", { name: "revoke the link" }).click();
  await expect(links(page)).toHaveCount(0);
  await expect(dialog(page).getByRole("status")).toHaveText("link revoked");
  expect((await fetch(url)).status).toBe(404);

  // Escape closes it, and focus goes back to the menu button.
  await page.keyboard.press("Escape");
  await expect(dialog(page)).toHaveCount(0);
  await expect(page.getByRole("banner").getByRole("button", { name: "more actions for untitled" })).toBeFocused();
});

test("the dialog lists the links a page has, an expired one as such", async ({ page, server }) => {
  const id = await server.page("shared");
  await server.call("POST", `/pages/${id}/shares`, {});
  await server.call("POST", `/pages/${id}/shares`, { expires_at: Date.now() + 1000 });
  await new Promise((done) => setTimeout(done, 1500));
  await asOwner(page, server);
  await page.goto(`${server.url}/?page=${id}`);
  await openShares(page);

  await expect(links(page)).toHaveCount(2);
  await expect(links(page).nth(0)).toContainText("this page only");
  await expect(links(page).nth(0)).toContainText("hidden from search engines");
  await expect(links(page).nth(0)).toContainText("no expiry");
  await expect(links(page).nth(1)).toContainText("expired");
});

test("a date already past is refused", async ({ page, server }) => {
  const id = await server.page("shared");
  await asOwner(page, server);
  await page.goto(`${server.url}/?page=${id}`);
  await openShares(page);
  const field = dialog(page).getByLabel("valid until (optional)");
  // The field offers no day before today; one typed anyway is refused by the server.
  await expect(field).toHaveAttribute("min", day(0));
  await field.evaluate((input: HTMLInputElement, value) => {
    input.min = "";
    input.value = value;
    input.dispatchEvent(new Event("input", { bubbles: true }));
  }, day(-2));
  await dialog(page).getByRole("button", { name: "create link" }).click();
  await expect(dialog(page).getByText("in the past")).toBeVisible();
  await expect(links(page)).toHaveCount(0);
});

test("only who may write a page sees its share dialog", async ({ browser, server }) => {
  const id = await server.page("shared");
  const member = await server.addMember("grace@example.com");
  await grant(server, member, "commenter");
  const page = await memberPage(browser, server, member);
  await page.goto(`${server.url}/?page=${id}`);
  const more = page.getByRole("banner").getByRole("button", { name: "more actions for untitled" });
  await more.click();
  await expect(page.getByRole("menuitem", { name: "copy link" })).toBeVisible();
  await expect(page.getByRole("menuitem", { name: "share publicly" })).toHaveCount(0);
  await page.keyboard.press("Escape");

  await grant(server, member, "editor");
  await page.reload();
  await more.click();
  await expect(page.getByRole("menuitem", { name: "share publicly" })).toBeVisible();
});

test("a link created before the list answers stays in it", async ({ page, server }) => {
  const id = await server.page("shared");
  await asOwner(page, server);
  // The list answers with what the server held before the link, and only after it is created.
  let created: () => void = () => {};
  const createdYet = new Promise<void>((done) => (created = done));
  let listed: () => void = () => {};
  const listedYet = new Promise<void>((done) => (listed = done));
  await page.route(`**/api/v1/pages/${id}/shares?**`, async (route) => {
    const response = await route.fetch();
    await createdYet;
    await route.fulfill({ response });
    listed();
  });
  await page.goto(`${server.url}/?page=${id}`);
  await openShares(page);
  const posted = page.waitForResponse(
    (response) =>
      response.url().endsWith(`/pages/${id}/shares`) && response.request().method() === "POST",
  );
  await dialog(page).getByRole("button", { name: "create link" }).click();
  await posted;
  await expect(links(page)).toHaveCount(1);
  created();
  await listedYet;
  await expect(dialog(page).getByText("no public links yet")).toHaveCount(0);
  await expect(links(page)).toHaveCount(1);
});

test("a checkbox shows its state in forced colors", async ({ page, server }) => {
  const id = await server.page("shared");
  await asOwner(page, server);
  await page.emulateMedia({ forcedColors: "active" });
  await page.goto(`${server.url}/?page=${id}`);
  await openShares(page);
  // Forced colors drop backgrounds and shadows, so the box is the system's own.
  const box = dialog(page).getByRole("checkbox", { name: "include subpages" });
  expect(await box.evaluate((input) => getComputedStyle(input).appearance)).toBe("auto");
});

test("the same result said twice is said again", async ({ page, server }) => {
  const id = await server.page("shared");
  await server.call("POST", `/pages/${id}/shares`, {});
  await page.context().grantPermissions(["clipboard-read", "clipboard-write"], { origin: server.url });
  await asOwner(page, server);
  await page.goto(`${server.url}/?page=${id}`);
  await openShares(page);
  const copyButton = links(page).first().getByRole("button", { name: "copy the public link" });
  const said = dialog(page).getByRole("status");
  await copyButton.click();
  await expect(said).toHaveText("link copied");
  // A screen reader hears a live region when its text changes: count the changes.
  await said.evaluate((region) => {
    const changes = { count: 0 };
    (window as { changes?: typeof changes }).changes = changes;
    new MutationObserver((records) => (changes.count += records.length)).observe(region, {
      childList: true,
      characterData: true,
      subtree: true,
    });
  });
  await copyButton.click();
  await expect
    .poll(() => page.evaluate(() => (window as { changes?: { count: number } }).changes?.count))
    .toBeGreaterThan(0);
  await expect(said).toHaveText("link copied");
});

test("a role that drops closes the dialog and gives focus back to the menu", async ({
  browser,
  server,
}) => {
  const id = await server.page("shared");
  const member = await server.addMember("grace@example.com");
  const page = await memberPage(browser, server, member);
  await page.goto(`${server.url}/?page=${id}`);
  await expect(page.getByRole("banner").getByRole("status")).toContainText("synced");
  await openShares(page);
  await grant(server, member, "commenter");
  await expect(dialog(page)).toHaveCount(0);
  await expect(
    page.getByRole("banner").getByRole("button", { name: "more actions for untitled" }),
  ).toBeFocused();
});
