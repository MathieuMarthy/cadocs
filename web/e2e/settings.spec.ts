// Workspace and space settings: members, invitations, grants, trash of a space (step 13).

import { expect, type Page } from "@playwright/test";

import { asOwner, memberPage, test } from "./fixtures.ts";
import { PASSWORD, type TestServer } from "./server.ts";

const main = (page: Page) => page.getByRole("main");
const tree = (page: Page) => page.getByRole("navigation", { name: "spaces" });
const section = (page: Page, name: string) => main(page).getByRole("region", { name });
const editor = (page: Page) => page.getByLabel("page content");
const status = (page: Page) => page.getByRole("banner").getByRole("status");
const notice = (page: Page) => page.locator(".notice-region");

/** A page at the root of the first space, created over REST. */
async function newPage(server: TestServer, title: string) {
  const { spaceId } = await server.session();
  const page = await server.call("POST", `/spaces/${spaceId}/pages`, {});
  await server.call("PATCH", `/pages/${page.id}`, { title });
  return page.id as string;
}

test("an admin invites a user, opens a space to them, and raises their role while their page is open", async ({
  page,
  server,
  browser,
}) => {
  const id = await newPage(server, "Plans");
  await asOwner(page, server);

  // The settings are one row of the sidebar away.
  await page.goto(`${server.url}/w/tests`);
  await page.getByRole("navigation", { name: "settings" }).getByRole("link", { name: "workspace" }).click();
  await expect(page).toHaveURL(/\/w\/tests\/settings$/);

  const invitations = section(page, "invitations");
  await invitations.getByLabel("email").fill("grace@example.org");
  await invitations.getByRole("button", { name: "invite" }).click();
  const link = await invitations.getByLabel("invitation link", { exact: true }).inputValue();
  expect(link).toMatch(/\/invite\//);
  await expect(invitations.getByRole("listitem").filter({ hasText: "grace@example.org" })).toBeVisible();

  const guest = await (await browser.newContext()).newPage();
  await guest.goto(link);
  await guest.getByLabel("name").fill("Grace");
  await guest.getByLabel("password").fill(PASSWORD);
  await guest.getByRole("button", { name: "join Tests" }).click();
  await expect(guest).toHaveURL(/\/w\/tests$/);

  // The space has no default role yet: the space settings open it to every member, read only.
  await page.goto(`${server.url}/w/tests`);
  await tree(page).getByRole("button", { name: "Pages/", exact: true }).hover();
  await tree(page).getByRole("button", { name: "more actions for Pages" }).click();
  await page.getByRole("menuitem", { name: "settings" }).click();
  await expect(page).toHaveURL(/\/w\/tests\/settings\/pages$/);
  const general = section(page, "space");
  await general.getByLabel("default role").selectOption("viewer");
  await general.getByRole("button", { name: "save" }).click();
  await expect(notice(page)).toContainText("saved");

  await guest.goto(`${server.url}/?page=${id}`);
  await expect(editor(guest)).toHaveAttribute("contenteditable", "false");
  await expect(status(guest)).toContainText("read only");

  // Raising Grace to workspace admin makes her admin of every space, and her page follows.
  await page.goto(`${server.url}/w/tests/settings`);
  const members = section(page, "members");
  await expect(members.getByRole("listitem").filter({ hasText: "grace@example.org" })).toBeVisible();
  await members.getByLabel("role of Grace").selectOption("admin");
  await expect(notice(page)).toContainText("role changed");
  await expect(editor(guest)).toHaveAttribute("contenteditable", "true");
  await expect(status(guest)).not.toContainText("read only");
  // And her own shell learns it: she now manages spaces.
  await expect(tree(guest).getByRole("button", { name: "new space" })).toBeVisible();
});

test("a grant lowers one member below the default, and the open page follows", async ({
  page,
  server,
  browser,
}) => {
  const id = await newPage(server, "Plans");
  const member = await server.addMember("grace@example.org");
  const guest = await memberPage(browser, server, member);
  await guest.goto(`${server.url}/?page=${id}`);
  await expect(editor(guest)).toHaveAttribute("contenteditable", "true");

  await asOwner(page, server);
  await page.goto(`${server.url}/w/tests/settings/pages`);
  const access = section(page, "access");
  // The owner is admin of every space: no grant applies to her.
  const ada = access.getByRole("listitem").filter({ hasText: "ada@example.org" });
  await expect(ada).toContainText("workspace admin");
  await expect(ada.getByRole("combobox")).toHaveCount(0);

  await expect(access.getByLabel("access of Grace")).toHaveValue("editor");
  await access.getByLabel("access of Grace").selectOption("viewer");
  await expect(notice(page)).toContainText("access changed");
  await expect(editor(guest)).toHaveAttribute("contenteditable", "false");
  // A revocation that lands while the socket reconnects reads as a page not found, since REST
  // hides what one cannot read: let the reconnection end first.
  await expect(status(guest)).toHaveText(/^synced.*read only$/);
  const { spaceId } = await server.session();
  const grants = await server.call("GET", `/spaces/${spaceId}/grants`);
  expect(grants.items).toMatchObject([{ user_id: member.id, role: "viewer" }]);

  // Back to the default, which is none here: the page is gone for her.
  await access.getByLabel("access of Grace").selectOption("");
  await expect(guest.getByRole("main")).toContainText("access revoked");
});

test("a workspace admin renames the workspace, revokes an invitation, and a member only reads", async ({
  page,
  server,
  browser,
}) => {
  const member = await server.addMember("grace@example.org");
  await server.invite("linus@example.org", "admin");
  await asOwner(page, server);
  await page.goto(`${server.url}/w/tests/settings`);

  const general = section(page, "workspace");
  await general.getByLabel("name").fill("Team");
  await general.getByLabel("language").selectOption("fr");
  await general.getByRole("button", { name: "save" }).click();
  await expect(notice(page)).toContainText("saved");
  const { workspaceId } = await server.session();
  expect(await server.call("GET", `/workspaces/${workspaceId}`)).toMatchObject({
    name: "Team",
    language: "fr",
  });
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Team settings");

  const invitations = section(page, "invitations");
  const linus = invitations.getByRole("listitem").filter({ hasText: "linus@example.org" });
  await expect(linus).toContainText("admin");
  await linus.getByRole("button", { name: "revoke the invitation of linus@example.org" }).click();
  await expect(linus).toHaveCount(0);
  expect((await server.call("GET", `/workspaces/${workspaceId}/invitations`)).items).toEqual([]);

  // The owner cannot demote or remove herself from here.
  const self = section(page, "members").getByRole("listitem").filter({ hasText: "ada@example.org" });
  await expect(self).toContainText("(you)");
  await expect(self.getByRole("combobox")).toHaveCount(0);
  await expect(self.getByRole("button", { name: "remove" })).toHaveCount(0);

  const guest = await memberPage(browser, server, member);
  await guest.goto(`${server.url}/w/tests/settings`);
  await expect(main(guest)).toContainText("only workspace admins change these settings");
  await expect(main(guest).getByRole("combobox")).toHaveCount(0);
  await expect(section(guest, "invitations")).toHaveCount(0);
  await expect(section(guest, "members").getByRole("listitem")).toHaveCount(2);

  // A member leaves on their own.
  const leave = section(guest, "leave");
  await leave.getByRole("button", { name: "leave the workspace" }).click();
  await leave.getByRole("button", { name: "leave", exact: true }).click();
  await expect(guest.getByRole("heading", { name: "no workspace" })).toBeVisible();
  expect((await server.call("GET", `/workspaces/${workspaceId}/members`)).items).toHaveLength(1);
});

test("an admin removes a member", async ({ page, server }) => {
  await server.addMember("grace@example.org");
  await asOwner(page, server);
  await page.goto(`${server.url}/w/tests/settings`);
  const grace = section(page, "members").getByRole("listitem").filter({ hasText: "grace@example.org" });
  await grace.getByRole("button", { name: "remove" }).click();
  await grace.getByRole("button", { name: "remove Grace" }).click();
  await expect(grace).toHaveCount(0);
  const { workspaceId } = await server.session();
  expect((await server.call("GET", `/workspaces/${workspaceId}/members`)).items).toHaveLength(1);
});

test("a space is renamed, moved to trash with an undo, and restored from the workspace settings", async ({
  page,
  server,
}) => {
  await asOwner(page, server);
  await page.goto(`${server.url}/w/tests/settings/pages`);
  const general = section(page, "space");
  await general.getByLabel("name").fill("Docs");
  await general.getByLabel("description").fill("What we know");
  await general.getByRole("button", { name: "save" }).click();
  await expect(tree(page).getByRole("button", { name: "Docs/", exact: true })).toBeVisible();
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Docs settings");

  await section(page, "trash").getByRole("button", { name: "move to trash" }).click();
  await expect(page).toHaveURL(/\/w\/tests$/);
  await expect(notice(page)).toContainText("moved to trash");
  await expect(tree(page).getByRole("button", { name: "Docs/", exact: true })).toHaveCount(0);
  await notice(page).getByRole("button", { name: "undo" }).click();
  await expect(tree(page).getByRole("button", { name: "Docs/", exact: true })).toBeVisible();

  await page.goto(`${server.url}/w/tests/settings/pages`);
  await section(page, "trash").getByRole("button", { name: "move to trash" }).click();
  await expect(page).toHaveURL(/\/w\/tests$/);
  await page.goto(`${server.url}/w/tests/settings`);
  const trashed = section(page, "trashed spaces");
  await trashed.getByRole("button", { name: "restore Docs" }).click();
  await expect(trashed).toHaveCount(0);
  await expect(tree(page).getByRole("button", { name: "Docs/", exact: true })).toBeVisible();
});

test("an owner deletes a workspace once its name is typed", async ({ page, server }) => {
  await server.call("POST", "/workspaces", { name: "Scratch" });
  await asOwner(page, server);
  await page.goto(`${server.url}/w/scratch/settings`);
  const danger = section(page, "delete");
  const button = danger.getByRole("button", { name: "delete the workspace" });
  await expect(button).toBeDisabled();
  await danger.getByLabel("type the workspace name to confirm").fill("Scratch");
  await button.click();
  await expect(page).toHaveURL(/\/w\/tests$/);
  const { items } = await server.call("GET", "/workspaces");
  expect(items.map((item: { name: string }) => item.name)).toEqual(["Tests"]);
});

test("a refused role or grant shows why, and its picker goes back to what is stored", async ({
  page,
  server,
}) => {
  await server.addMember("grace@example.org");
  await asOwner(page, server);
  const conflict = { error: { code: "conflict", message: "A workspace keeps one owner" } };
  await page.route("**/api/v1/workspaces/*/members/*", (route) =>
    route.request().method() === "PATCH"
      ? route.fulfill({ status: 409, contentType: "application/json", body: JSON.stringify(conflict) })
      : route.fallback(),
  );
  await page.goto(`${server.url}/w/tests/settings`);
  const members = section(page, "members");
  await members.getByLabel("role of Grace").selectOption("admin");
  await expect(members.getByRole("alert")).toHaveText("a workspace keeps one owner");
  await expect(members.getByLabel("role of Grace")).toHaveValue("member");

  const refused = { error: { code: "validation_failed", message: "Invalid", fields: { grants: "unknown_target" } } };
  await page.route("**/api/v1/spaces/*/grants", (route) =>
    route.request().method() === "PUT"
      ? route.fulfill({ status: 400, contentType: "application/json", body: JSON.stringify(refused) })
      : route.fallback(),
  );
  await page.goto(`${server.url}/w/tests/settings/pages`);
  const access = section(page, "access");
  await access.getByLabel("access of Grace").selectOption("viewer");
  await expect(access.getByRole("alert")).toHaveText("grants: unknown target");
  await expect(access.getByLabel("access of Grace")).toHaveValue("editor");
});
