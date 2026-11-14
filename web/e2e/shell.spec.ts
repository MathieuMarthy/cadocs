// The application shell in a real browser, on the embedded front end of the real binary.

import { expect, type Page } from "@playwright/test";

import { asOwner, logIn, memberPage, test } from "./fixtures.ts";
import { type Member, PASSWORD, TestServer } from "./server.ts";

/** Logs out from the user menu of the sidebar footer. */
async function logOut(page: Page, name = "Ada") {
  await page.getByRole("button", { name, exact: true }).click();
  await page.getByRole("menuitem", { name: "log out" }).click();
}

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
  const fill = async (password: string) => {
    await page.getByLabel("name", { exact: true }).fill("Ada");
    await page.getByLabel("email").fill("ada@example.org");
    await page.getByLabel("password").fill(password);
    await page.getByLabel("workspace name").fill("Acme Docs");
  };
  await page.goto(server.url);
  await expect(page).toHaveURL(`${server.url}/setup`);

  // Without the link of the server's log, the page asks for its code.
  await page.getByLabel("setup code").fill("0".repeat(64));
  await fill("short");
  await page.getByRole("button", { name: "create workspace" }).click();
  await expect(page.getByRole("alert").filter({ hasText: "wrong" })).toBeVisible();
  await expect(page.getByRole("alert").filter({ hasText: "too short" })).toBeVisible();

  const code = await server.setupCode();
  await page.goto(`${server.url}/setup/${code}`);
  await expect(page.getByRole("button", { name: "create workspace" })).toBeVisible();
  await expect(page.getByLabel("setup code")).toHaveCount(0);
  await fill(PASSWORD);
  await page.getByRole("button", { name: "create workspace" }).click();
  await expect(page).toHaveURL(`${server.url}/w/acme-docs`);
  await expect(page.getByRole("heading", { name: "Acme Docs" })).toBeVisible();
  await expect(page.getByText("no spaces yet")).toBeVisible();

  await page.reload();
  await expect(page.getByRole("heading", { name: "Acme Docs" })).toBeVisible();

  await logOut(page);
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

  await page.getByLabel("setup code").fill(await server.setupCode());
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
  await logOut(page);
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
  await adminRow(page, "grace@example.org").getByRole("button", { name: "more actions for Grace" }).click();
  await page.getByRole("menuitem", { name: "new reset link" }).click();
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
  await page.getByRole("button", { name: "join Tests" }).click();
  await expect(page).toHaveURL(`${server.url}/w/${second.slug}`);
});

test("a new person joins with an invitation link", async ({ page, server }) => {
  const link = await server.invite("grace@example.org");
  await page.goto(link);
  await expect(page.getByText("grace@example.org")).toBeVisible();
  await page.getByLabel("name", { exact: true }).fill("Grace");
  await page.getByLabel("password").fill(PASSWORD);
  await page.getByRole("button", { name: "join Tests" }).click();
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
  await page.getByRole("button", { name: "join Tests" }).click();
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
  await adminRow(page, "grace@example.org").getByRole("button", { name: "more actions for Grace" }).click();
  await page.getByRole("menuitem", { name: "new reset link" }).click();
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
  const profile = page.getByRole("form", { name: "profile" });
  await profile.getByLabel("name", { exact: true }).fill("Ada Lovelace");
  await profile.getByRole("button", { name: "save" }).click();
  await expect(profile.getByRole("status")).toContainText("saved");
  await expect(page.getByRole("complementary")).toContainText("Ada Lovelace");

  const login = page.getByRole("form", { name: "email and password" });
  await login.getByLabel("new password").fill("a brand new password");
  await login.getByLabel("current password").fill("wrong password");
  await login.getByRole("button", { name: "save" }).click();
  await expect(login.getByRole("alert")).toBeVisible();
  await page.reload();
  await expect(page.getByLabel("name", { exact: true })).toHaveValue("Ada Lovelace");
});

test("an account deletes itself, after a confirmation", async ({ page, server, browser }) => {
  const grace = await server.addMember("grace@example.org");
  const other = await memberPage(browser, server, grace);
  await other.goto(`${server.url}/account`);
  await other.getByRole("button", { name: "delete account" }).click();
  await other.getByRole("button", { name: "delete account" }).click();
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

  const row = adminRow(page, "grace@example.org");
  const act = async (name: string) => {
    await row.getByRole("button", { name: "more actions for Grace" }).click();
    await page.getByRole("menuitem", { name, exact: true }).click();
  };
  await act("make admin");
  await expect(row.getByText("admin", { exact: true })).toBeVisible();
  await act("remove admin");
  await expect(row.getByText("admin", { exact: true })).toHaveCount(0);

  const other = await memberPage(browser, server, grace);
  await other.goto(`${server.url}/admin`);
  await expect(other.getByRole("heading", { name: "not found" })).toBeVisible();
  await expect(other.getByRole("link", { name: "admin" })).toHaveCount(0);

  await act("disable");
  await expect(row.getByText("disabled")).toBeVisible();
  await other.reload();
  await expect(other).toHaveURL(/\/login/);
  await expect(other.getByRole("status")).toHaveCount(0);

  await act("enable");
  await act("delete");
  await row.getByRole("button", { name: "delete", exact: true }).click();
  await expect(adminRow(page, "grace@example.org")).toHaveCount(0);
});

test("a plain member creates a workspace from the switcher and switches back", async ({
  browser,
  server,
}) => {
  const grace = await memberPage(browser, server, await server.addMember("grace@example.org"));
  await grace.goto(`${server.url}/w/tests`);
  await grace.getByText("Tests ▾").click();
  await grace.getByRole("button", { name: "new workspace" }).click();
  // Escape gives up, and gives the row back.
  await grace.getByLabel("workspace name").press("Escape");
  await expect(grace.getByLabel("workspace name")).toHaveCount(0);
  await grace.getByRole("button", { name: "new workspace" }).click();
  await grace.getByLabel("workspace name").fill("Second");
  await grace.getByRole("button", { name: "create workspace" }).click();
  await expect(grace).toHaveURL(`${server.url}/w/second`);
  await expect(grace.getByRole("heading", { name: "Second" })).toBeVisible();
  // Its owner creates spaces in it.
  await expect(grace.getByRole("button", { name: "new space" }).first()).toBeVisible();

  await grace.getByText("Second ▾").click();
  await grace.getByRole("link", { name: "Tests", exact: true }).click();
  await expect(grace).toHaveURL(`${server.url}/w/tests`);
  await expect(grace.getByRole("heading", { name: "Tests" })).toBeVisible();
  const spaces = grace.getByRole("navigation", { name: "spaces" });
  await expect(spaces.getByRole("button", { name: "Pages/", exact: true })).toBeVisible();
});

/** Takes `member` out of the workspace of the first account. */
async function removeMember(server: TestServer, member: Member) {
  const { workspaceId } = await server.session();
  await server.call("DELETE", `/workspaces/${workspaceId}/members/${member.id}`);
}

test("a user with no workspace creates one from their screen", async ({ browser, server }) => {
  const member = await server.addMember("grace@example.org");
  await removeMember(server, member);
  const grace = await memberPage(browser, server, member);
  await grace.goto(server.url);
  await expect(grace.getByRole("heading", { name: "no workspace" })).toBeVisible();
  await grace.getByLabel("workspace name").fill("Mine");
  await grace.getByRole("button", { name: "create workspace" }).click();
  await expect(grace).toHaveURL(`${server.url}/w/mine`);
  await expect(grace.getByRole("heading", { name: "Mine" })).toBeVisible();
});

const adminsOnly = test.extend({
  server: async ({}, use) => {
    const server = await TestServer.start({ CADOCS_WORKSPACE_CREATION: "admins" });
    await use(server);
    await server.dispose();
  },
});

adminsOnly("only instance admins see the new workspace form when the server says so", async ({
  page,
  browser,
  server,
}) => {
  const member = await server.addMember("grace@example.org");
  const grace = await memberPage(browser, server, member);
  await grace.goto(`${server.url}/w/tests`);
  await grace.getByText("Tests ▾").click();
  await expect(grace.getByRole("link", { name: "Tests", exact: true })).toBeVisible();
  await expect(grace.getByRole("button", { name: "new workspace" })).toHaveCount(0);

  await removeMember(server, member);
  await grace.goto(server.url);
  await expect(grace.getByRole("heading", { name: "no workspace" })).toBeVisible();
  await expect(grace.getByText("ask a workspace admin for an invitation.")).toBeVisible();
  await expect(grace.getByLabel("workspace name")).toHaveCount(0);

  await asOwner(page, server);
  await page.goto(`${server.url}/w/tests`);
  await page.getByText("Tests ▾").click();
  await page.getByRole("button", { name: "new workspace" }).click();
  await page.getByLabel("workspace name").fill("Second");
  await page.getByRole("button", { name: "create workspace" }).click();
  await expect(page).toHaveURL(`${server.url}/w/second`);
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

test("a phone shows the content first, with no sideways scroll", async ({ page, server }) => {
  await page.setViewportSize({ width: 360, height: 700 });
  await asOwner(page, server);

  for (const path of ["/w/tests", "/account", "/admin"]) {
    await page.goto(`${server.url}${path}`);
    await expect(page.getByRole("main").getByRole("heading").first()).toBeInViewport();
    await expect(page.getByRole("complementary")).toBeHidden();
    const overflow = await page.evaluate(
      () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
    );
    expect(overflow, path).toBe(0);
  }
});

test("the user menu leads to the account, changes the theme and logs out", async ({
  page,
  server,
}) => {
  await asOwner(page, server);
  await page.goto(`${server.url}/w/tests`);
  const sidebar = page.getByRole("complementary");

  // The source link and the license share one line under the menu.
  const source = sidebar.getByRole("link", { name: "source" });
  const license = sidebar.getByText(/agpl-3\.0/);
  const [a, b] = [await source.boundingBox(), await license.boundingBox()];
  expect(Math.abs(a!.y + a!.height / 2 - (b!.y + b!.height / 2))).toBeLessThan(2);

  const user = sidebar.getByRole("button", { name: "Ada", exact: true });
  await user.click();
  await page.getByRole("menuitem", { name: "account" }).click();
  await expect(page).toHaveURL(`${server.url}/account`);

  await user.click();
  await page.getByRole("menuitem", { name: "theme: auto" }).click();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");
  await expect(user).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(page.getByRole("menuitem", { name: "account" })).toBeFocused();
  await expect(page.getByRole("menuitem", { name: "theme: light" })).toBeVisible();
  await page.keyboard.press("Escape");

  await logOut(page);
  await expect(page).toHaveURL(`${server.url}/login`);
});

test("the breadcrumb names the workspace once, and none outside it", async ({ page, server }) => {
  await asOwner(page, server);
  const crumbs = page.getByRole("navigation", { name: "location" });
  await page.goto(`${server.url}/w/tests`);
  await expect(crumbs).toHaveText(/^\s*Tests\s*$/);
  for (const [path, title] of [["/account", "account"], ["/admin", "admin"]]) {
    await page.goto(`${server.url}${path}`);
    await expect(crumbs).toHaveText(new RegExp(`^\\s*${title}\\s*$`));
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
    await expect(page.getByRole("banner").getByRole("status")).toContainText("synced");
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
    // A page at the root of its space has no parent: the way out is its workspace.
    await page.getByRole("link", { name: "open the workspace" }).click();
    await expect(page).toHaveURL(`${server.url}/w/tests`);
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
