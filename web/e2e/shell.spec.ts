// The application shell in a real browser, on the embedded front end of the real binary.

import { expect, type Page } from "@playwright/test";

import { asOwner, logIn, memberPage, test } from "./fixtures.ts";
import { PASSWORD, type TestServer } from "./server.ts";

async function submitLogin(page: Page, email: string, password: string) {
  await page.getByLabel("email").fill(email);
  await page.getByLabel("password").fill(password);
  await page.getByRole("button", { name: "log in" }).click();
}

const adminRow = (page: Page, email: string) => page.getByRole("listitem").filter({ hasText: email });

test("first run goes from setup to an empty workspace, and login comes back to it", async ({
  page,
  server,
}) => {
  await page.goto(server.url);
  await expect(page).toHaveURL(`${server.url}/setup`);

  await page.getByLabel("name", { exact: true }).fill("Ada");
  await page.getByLabel("email").fill("ada@example.org");
  await page.getByLabel("password").fill("short");
  await page.getByLabel("workspace name").fill("Acme Docs");
  await page.getByRole("button", { name: "create workspace" }).click();
  await expect(page.getByRole("alert")).toContainText("too short");

  await page.getByLabel("password").fill(PASSWORD);
  await page.getByRole("button", { name: "create workspace" }).click();
  await expect(page).toHaveURL(`${server.url}/w/acme-docs`);
  await expect(page.getByRole("heading", { name: "Acme Docs" })).toBeVisible();
  await expect(page.getByText("no spaces yet")).toBeVisible();

  await page.reload();
  await expect(page.getByRole("heading", { name: "Acme Docs" })).toBeVisible();

  await page.getByRole("button", { name: "log out" }).click();
  await expect(page).toHaveURL(`${server.url}/login`);
  await submitLogin(page, "ada@example.org", "wrong password");
  await expect(page.getByRole("alert")).toContainText("wrong email or password");
  await submitLogin(page, "ada@example.org", PASSWORD);
  await expect(page).toHaveURL(`${server.url}/w/acme-docs`);
});

test("setup is gone once an account exists, and an unknown address says so", async ({
  page,
  server,
}) => {
  await server.session();
  await page.goto(`${server.url}/setup`);
  await expect(page).toHaveURL(`${server.url}/login`);

  await asOwner(page, server);
  await page.goto(`${server.url}/nowhere`);
  await expect(page.getByRole("heading", { name: "not found" })).toBeVisible();
});

test("a login sends you back where you were going", async ({ page, server }) => {
  const id = await server.page("target");
  await page.goto(`${server.url}/?page=${id}`);
  await expect(page).toHaveURL(/\/login\?next=/);
  await submitLogin(page, "ada@example.org", PASSWORD);
  await expect(page.getByLabel("page content")).toBeVisible();
  expect(new URL(page.url()).search).toBe(`?page=${id}`);
});

test("a login never leaves the site", async ({ page, server }) => {
  await server.session();
  await page.goto(`${server.url}/login?next=//example.org/`);
  await submitLogin(page, "ada@example.org", PASSWORD);
  await expect(page).toHaveURL(`${server.url}/w/tests`);
});

test("a login with a disguised address stays on the site too", async ({ page, server }) => {
  await server.session();
  await page.goto(`${server.url}/login?next=/%09/example.org/`);
  await submitLogin(page, "ada@example.org", PASSWORD);
  await expect(page).toHaveURL(`${server.url}/w/tests`);
});

test("setup taken by someone else leads to login", async ({ page, server }) => {
  await page.goto(`${server.url}/setup`);
  await expect(page.getByRole("button", { name: "create workspace" })).toBeVisible();
  await server.session();

  await page.getByLabel("name", { exact: true }).fill("Eve");
  await page.getByLabel("email").fill("eve@example.org");
  await page.getByLabel("password").fill(PASSWORD);
  await page.getByLabel("workspace name").fill("Other");
  await page.getByRole("button", { name: "create workspace" }).click();
  await expect(page).toHaveURL(`${server.url}/login`);
});

test("logging out with no server says so, and stays logged in", async ({ page, server }) => {
  await asOwner(page, server);
  await page.goto(`${server.url}/w/tests`);
  await expect(page.getByRole("heading", { name: "Tests" })).toBeVisible();
  await page.route("**/api/v1/auth/logout", (request) => request.abort());
  await page.getByRole("button", { name: "log out" }).click();
  await expect(page.getByRole("alert")).toContainText("unreachable");
  await expect(page).toHaveURL(`${server.url}/w/tests`);
});

test("an invitation for someone else logs the session out for real", async ({ page, server }) => {
  await asOwner(page, server);
  const link = await server.invite("linus@example.org");
  await page.goto(link);
  await page.getByRole("button", { name: "log out" }).click();
  await expect(page).toHaveURL(/\/login\?next=%2Finvite%2F/);

  await page.goto(link);
  await expect(page.getByLabel("password")).toBeVisible();
});

test("a reset link used in another person's session leaves that session alone", async ({
  page,
  server,
}) => {
  await server.addMember("grace@example.org");
  await asOwner(page, server);
  await page.goto(`${server.url}/admin`);
  await adminRow(page, "grace@example.org").getByRole("button", { name: "reset link" }).click();
  await page.goto(await page.getByLabel("one-time link").inputValue());
  await page.getByLabel("new password").fill("a brand new password");
  await page.getByRole("button", { name: "set password" }).click();
  await expect(page.getByRole("status")).toContainText("password changed");
  await page.getByRole("link", { name: "go to cadocs" }).click();
  await expect(page).toHaveURL(`${server.url}/w/tests`);
});

test("a page whose code cannot load says so", async ({ page, server }) => {
  await asOwner(page, server);
  await page.route("**/assets/Page-*.js", (request) => request.abort());
  await page.goto(`${server.url}/?page=${await server.page("doc")}`);
  await expect(page.getByRole("heading", { name: "page failed to load" })).toBeVisible();
  await expect(page.getByRole("button", { name: "reload" })).toBeVisible();
});

test("a link to a file of the server leaves the shell", async ({ page, server }) => {
  await asOwner(page, server);
  await page.goto(`${server.url}/w/tests`);
  await expect(page.getByRole("heading", { name: "Tests" })).toBeVisible();
  await page.evaluate(() => {
    const link = document.createElement("a");
    link.href = "/licenses/ibm-plex-sans-ofl.txt";
    link.textContent = "licence";
    document.querySelector("main")!.append(link);
  });
  await page.getByRole("link", { name: "licence" }).click();
  await expect.poll(() => page.evaluate(() => document.contentType)).toBe("text/plain");
});

test("an invitation lands in its workspace, beside one of the same name", async ({
  page,
  server,
}) => {
  const grace = await server.addMember("grace@example.org");
  const second = await server.call("POST", "/workspaces", { name: "Tests" });
  const body = { email: "grace@example.org", role: "member" };
  const { link } = await server.call("POST", `/workspaces/${second.id}/invitations`, body);

  await logIn(page, server, grace.token);
  await page.goto(link);
  await page.getByRole("button", { name: "join tests" }).click();
  await expect(page).toHaveURL(`${server.url}/w/${second.slug}`);
});

test("a new person joins with an invitation link", async ({ page, server }) => {
  const link = await server.invite("grace@example.org");
  await page.goto(link);
  await expect(page.getByText("grace@example.org")).toBeVisible();
  await page.getByLabel("name", { exact: true }).fill("Grace");
  await page.getByLabel("password").fill(PASSWORD);
  await page.getByRole("button", { name: "join tests" }).click();
  await expect(page).toHaveURL(`${server.url}/w/tests`);

  await page.goto(link);
  await expect(page.getByText("no longer valid")).toBeVisible();
});

test("someone with an account logs in, then joins", async ({ page, server, browser }) => {
  const admin = await server.addMember("grace@example.org");
  const { workspaceId } = await server.session();
  await server.call("DELETE", `/workspaces/${workspaceId}/members/${admin.id}`);
  const link = await server.invite("grace@example.org");

  await page.goto(link);
  await page.getByRole("link", { name: "log in" }).click();
  await submitLogin(page, "grace@example.org", PASSWORD);
  await page.getByRole("button", { name: "join tests" }).click();
  await expect(page).toHaveURL(`${server.url}/w/tests`);

  const other = await (await browser.newContext()).newPage();
  await asOwner(other, server);
  await other.goto(await server.invite("linus@example.org"));
  await expect(other.getByText("logged in as ada@example.org")).toBeVisible();
});

test("an admin makes a reset link, and it sets a new password", async ({
  page,
  server,
  browser,
}) => {
  await server.addMember("grace@example.org");
  await asOwner(page, server);
  await page.goto(`${server.url}/admin`);
  await adminRow(page, "grace@example.org").getByRole("button", { name: "reset link" }).click();
  const link = await page.getByLabel("one-time link").inputValue();

  const other = await (await browser.newContext()).newPage();
  await other.goto(link);
  await other.getByLabel("new password").fill("short");
  await other.getByRole("button", { name: "set password" }).click();
  await expect(other.getByRole("alert")).toContainText("too short");
  await other.getByLabel("new password").fill("a brand new password");
  await other.getByRole("button", { name: "set password" }).click();
  await expect(other).toHaveURL(`${server.url}/login`);
  await expect(other.getByRole("status")).toContainText("password changed");
  await submitLogin(other, "grace@example.org", "a brand new password");
  await expect(other).toHaveURL(`${server.url}/w/tests`);

  await other.goto(link);
  await other.getByLabel("new password").fill("yet another password");
  await other.getByRole("button", { name: "set password" }).click();
  await expect(other.getByText("no longer valid")).toBeVisible();
});

test("the account page changes the name and checks the current password", async ({
  page,
  server,
}) => {
  await asOwner(page, server);
  await page.goto(`${server.url}/account`);
  await page.getByLabel("name", { exact: true }).fill("Ada Lovelace");
  await page.getByRole("button", { name: "save" }).click();
  await expect(page.getByRole("status")).toContainText("saved");
  await expect(page.getByRole("complementary")).toContainText("Ada Lovelace");

  await page.getByLabel("new password").fill("a brand new password");
  await page.getByLabel("current password").fill("wrong password");
  await page.getByRole("button", { name: "save" }).click();
  await expect(page.getByRole("alert")).toBeVisible();
  await page.reload();
  await expect(page.getByLabel("name", { exact: true })).toHaveValue("Ada Lovelace");
});

test("an account deletes itself, after a confirmation", async ({ page, server, browser }) => {
  const grace = await server.addMember("grace@example.org");
  const other = await memberPage(browser, server, grace);
  await other.goto(`${server.url}/account`);
  await other.getByRole("button", { name: "delete account" }).click();
  await other.getByRole("button", { name: "yes, delete" }).click();
  await expect(other).toHaveURL(`${server.url}/login`);

  await page.goto(`${server.url}/login`);
  await submitLogin(page, "grace@example.org", PASSWORD);
  await expect(page.getByRole("alert")).toContainText("wrong email or password");
});

test("instance administration lists, disables and promotes users", async ({
  page,
  server,
  browser,
}) => {
  const grace = await server.addMember("grace@example.org");
  await asOwner(page, server);
  await page.goto(`${server.url}/w/tests`);
  await page.getByRole("link", { name: "admin" }).click();
  await expect(page).toHaveURL(`${server.url}/admin`);

  const self = adminRow(page, "ada@example.org");
  await self.getByRole("button", { name: "remove admin" }).click();
  await expect(page.getByRole("alert")).toBeVisible();

  const row = adminRow(page, "grace@example.org");
  await row.getByRole("button", { name: "make admin" }).click();
  await expect(row.getByRole("button", { name: "remove admin" })).toBeVisible();
  await row.getByRole("button", { name: "remove admin" }).click();

  const other = await memberPage(browser, server, grace);
  await other.goto(`${server.url}/admin`);
  await expect(other.getByRole("heading", { name: "not found" })).toBeVisible();
  await expect(other.getByRole("link", { name: "admin" })).toHaveCount(0);

  await row.getByRole("button", { name: "disable" }).click();
  await expect(row.getByText("disabled")).toBeVisible();
  await other.reload();
  await expect(other).toHaveURL(/\/login/);
  await expect(other.getByRole("status")).toHaveCount(0);

  await row.getByRole("button", { name: "enable" }).click();
  await row.getByRole("button", { name: "delete" }).click();
  await row.getByRole("button", { name: "yes, delete" }).click();
  await expect(adminRow(page, "grace@example.org")).toHaveCount(0);
});

test("an instance admin creates a workspace and switches to it", async ({ page, server }) => {
  await asOwner(page, server);
  await page.goto(`${server.url}/admin`);
  await page.getByLabel("workspace name").fill("Second");
  await page.getByRole("button", { name: "create workspace" }).click();
  await expect(page).toHaveURL(`${server.url}/w/second`);

  await page.getByText("second ▾").click();
  await page.getByRole("link", { name: "tests" }).click();
  await expect(page).toHaveURL(`${server.url}/w/tests`);
  await expect(page.getByRole("heading", { name: "Tests" })).toBeVisible();
  const spaces = page.getByRole("navigation", { name: "spaces" });
  await expect(spaces.getByRole("button", { name: "pages/", exact: true })).toBeVisible();
});

test("a session that ends while the shell is open leads to login", async ({ page, server }) => {
  await asOwner(page, server);
  await page.goto(`${server.url}/w/tests`);
  await expect(page.getByRole("heading", { name: "Tests" })).toBeVisible();
  await server.call("POST", "/auth/logout");

  // The events socket closes with 4401, so nothing needs a click to find out.
  await expect(page).toHaveURL(/\/login\?next=%2Fw%2Ftests/);
  await expect(page.getByRole("status")).toContainText("session ended");
});

test("the theme follows the system until it is chosen, and stays chosen", async ({
  page,
  server,
}) => {
  const background = () =>
    page.evaluate(() => getComputedStyle(document.body).backgroundColor);
  const light = "rgb(245, 245, 244)";
  const dark = "rgb(14, 14, 14)";

  await server.session();
  await page.emulateMedia({ colorScheme: "dark" });
  await page.goto(`${server.url}/login`);
  await expect.poll(background).toBe(dark);
  await page.emulateMedia({ colorScheme: "light" });
  await expect.poll(background).toBe(light);

  await page.getByRole("button", { name: "theme: auto" }).click();
  await expect(page.getByRole("button", { name: "theme: light" })).toBeVisible();
  await page.getByRole("button", { name: "theme: light" }).click();
  await expect(page.getByRole("button", { name: "theme: dark" })).toBeVisible();
  await expect.poll(background).toBe(dark);

  await page.reload();
  await expect(page.getByRole("button", { name: "theme: dark" })).toBeVisible();
  await expect.poll(background).toBe(dark);
});

test("a phone gets the sidebar above the content, with no sideways scroll", async ({
  page,
  server,
}) => {
  await page.setViewportSize({ width: 360, height: 700 });
  await asOwner(page, server);

  for (const path of ["/w/tests", "/account", "/admin"]) {
    await page.goto(`${server.url}${path}`);
    await expect(page.getByRole("main").getByRole("heading").first()).toBeVisible();
    const sidebar = await page.getByRole("complementary").boundingBox();
    const main = await page.getByRole("main").boundingBox();
    expect(sidebar!.y + sidebar!.height).toBeLessThanOrEqual(main!.y);
    const overflow = await page.evaluate(
      () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
    );
    expect(overflow, path).toBe(0);
  }
});

test("every screen links to the source code", async ({ page, server }) => {
  const source = page.getByRole("link", { name: "source" });
  const href = /^https:\/\/github\.com\/MathieuMarthy\/cadocs\/tree\/\S+$/;

  await page.goto(`${server.url}/setup`);
  await expect(source).toHaveAttribute("href", href);
  await server.session();
  await page.goto(`${server.url}/login`);
  await expect(source).toHaveAttribute("href", href);
  await asOwner(page, server);
  await page.goto(`${server.url}/w/tests`);
  await expect(source).toHaveAttribute("href", href);
  await page.goto(`${server.url}/?page=${await server.page("any")}`);
  await expect(source).toHaveAttribute("href", href);
});

test.describe("an open page reacts to its socket closing", () => {
  async function openPage(page: Page, server: TestServer) {
    await page.goto(`${server.url}/?page=${await server.page("doc")}`);
    await expect(page.getByRole("status")).toContainText("synced");
  }

  test("4401: the session ended, so it goes to login and comes back", async ({ page, server }) => {
    await asOwner(page, server);
    await openPage(page, server);
    await server.call("POST", "/auth/logout");
    await expect(page).toHaveURL(/\/login\?next=/);
    await expect(page.getByRole("status")).toContainText("session ended");
  });

  test("4403: access was revoked, so the page is gone", async ({ page, server, browser }) => {
    const { workspaceId } = await server.session();
    const grace = await server.addMember("grace@example.org");
    const other = await memberPage(browser, server, grace);
    await openPage(other, server);
    await server.call("DELETE", `/workspaces/${workspaceId}/members/${grace.id}`);
    await expect(other.getByRole("heading", { name: "access revoked" })).toBeVisible();
    await expect(other.getByLabel("page content")).toHaveCount(0);
    await page.close();
  });

  test("4404: the page was deleted, so it is gone", async ({ page, server }) => {
    await asOwner(page, server);
    await openPage(page, server);
    await server.call("DELETE", `/pages/${await server.page("doc")}`);
    await expect(page.getByRole("heading", { name: "page deleted" })).toBeVisible();
    await expect(page.getByLabel("page content")).toHaveCount(0);
  });

  test("a page opens even when REST fails for a moment", async ({ page, server }) => {
    await asOwner(page, server);
    const id = await server.page("doc");
    await page.route(`**/api/v1/pages/${id}`, (request) => request.abort());
    await openPage(page, server);
    await expect(page.getByLabel("page content")).toBeVisible();
  });

  test("a page that does not exist says so instead of retrying", async ({ page, server }) => {
    await asOwner(page, server);
    await page.goto(`${server.url}/?page=0190a0a0-0000-7000-8000-000000000000`);
    await expect(page.getByRole("heading", { name: "page not found" })).toBeVisible();
  });
});
