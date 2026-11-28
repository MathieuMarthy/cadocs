// Workspace and space settings: members, invitations, grants, trash of a space (step 13), groups
// (step 36).

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
  // A read of the page that answers after the socket closed does not change what it says.
  await guest.waitForTimeout(1_500);
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
  // Opened by its address, with no screen before it, the save goes to the workspace home.
  await expect(page).toHaveURL(/\/w\/tests$/);
  await expect(tree(page).getByRole("button", { name: "Docs/", exact: true })).toBeVisible();
  await page.goto(`${server.url}/w/tests/settings/pages`);
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

test("saving a space's settings goes back to the page they were opened from", async ({
  page,
  server,
}) => {
  const id = await newPage(server, "Plans");
  await asOwner(page, server);
  const general = await settingsFrom(page, server, id);
  await general.getByLabel("description").fill("What we know");
  await general.getByRole("button", { name: "save" }).click();
  await expect(page).toHaveURL(`${server.url}/?page=${id}`);
  await expect(notice(page)).toContainText("saved");
});

/** Opens the settings of the first space from the tree, on the page `id`. */
async function settingsFrom(page: Page, server: TestServer, id: string) {
  await page.goto(`${server.url}/?page=${id}`);
  await expect(editor(page)).toBeVisible();
  await tree(page).getByRole("button", { name: "Pages/", exact: true }).hover();
  await tree(page).getByRole("button", { name: "more actions for Pages" }).click();
  await page.getByRole("menuitem", { name: "settings" }).click();
  await expect(page).toHaveURL(/\/w\/tests\/settings\/pages$/);
  return section(page, "space");
}

test("a space's settings left while they save do not bring the person back", async ({
  page,
  server,
}) => {
  const id = await newPage(server, "Plans");
  const other = await newPage(server, "Other");
  await server.titleStored(other, "Other");
  await asOwner(page, server);
  let release!: () => void;
  const held = new Promise<void>((done) => (release = done));
  await page.route("**/api/v1/spaces/*", async (route) => {
    if (route.request().method() === "PATCH") await held;
    await route.fallback();
  });

  const general = await settingsFrom(page, server, id);
  await general.getByLabel("description").fill("What we know");
  await general.getByRole("button", { name: "save" }).click();
  await tree(page).getByRole("link", { name: "Other", exact: true }).click();
  await expect(page).toHaveURL(`${server.url}/?page=${other}`);
  release();
  await expect(notice(page)).toContainText("saved");
  await page.waitForTimeout(300);
  await expect(page).toHaveURL(`${server.url}/?page=${other}`);
});

test("a space's settings save once while the spaces read again", async ({ page, server }) => {
  const id = await newPage(server, "Plans");
  await asOwner(page, server);
  let patches = 0;
  await page.route("**/api/v1/spaces/*", async (route) => {
    if (route.request().method() === "PATCH") patches += 1;
    await route.fallback();
  });
  // The list of spaces read after the save answers late.
  const spaces = (url: URL) => /\/workspaces\/[^/]+\/spaces$/.test(url.pathname);
  await page.route(spaces, async (route) => {
    if (patches > 0) await new Promise((done) => setTimeout(done, 2000));
    await route.fallback();
  });

  const general = await settingsFrom(page, server, id);
  await general.getByLabel("description").fill("What we know");
  const save = general.getByRole("button", { name: "save" });
  const reading = page.waitForRequest((request) => patches > 0 && spaces(new URL(request.url())));
  await save.click();
  await reading;
  // `force`, since a second press is what a disabled button must refuse.
  await save.click({ force: true });
  await expect(notice(page)).toContainText("saved");
  await page.waitForTimeout(300);
  await expect(page).toHaveURL(`${server.url}/?page=${id}`);
  expect(patches).toBe(1);
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

test("an admin makes a group, fills it, renames it and deletes it, and a member only reads it", async ({
  page,
  server,
  browser,
}) => {
  const member = await server.addMember("grace@example.org");
  await asOwner(page, server);
  await page.goto(`${server.url}/w/tests/settings`);
  const groups = section(page, "groups");
  await groups.getByLabel("group name").fill("Writers");
  await groups.getByRole("button", { name: "create group" }).click();
  const writers = groups.getByRole("listitem").filter({ hasText: "Writers" });
  await expect(writers).toContainText("0 members");

  // A second group of the same name is refused, and says why.
  await groups.getByLabel("group name").fill("Writers");
  await groups.getByRole("button", { name: "create group" }).click();
  await expect(groups.getByRole("alert")).toHaveText("taken");

  await groups.getByRole("button", { name: "members of Writers" }).click();
  await expect(groups.getByRole("button", { name: "members of Writers" })).toHaveAttribute("aria-expanded", "true");
  await groups.getByLabel("add to Writers").selectOption({ label: "Grace" });
  await groups.getByRole("button", { name: "add", exact: true }).click();
  await expect(writers).toContainText("1 member");
  await expect(groups.getByRole("button", { name: "remove Grace from Writers" })).toBeVisible();

  await groups.getByLabel("name of Writers").fill("Editors");
  await groups.getByRole("button", { name: "rename", exact: true }).click();
  const editors = groups.getByRole("listitem").filter({ hasText: "Editors" });
  await expect(editors).toContainText("1 member");

  // A member sees the groups and who is in them, and changes nothing.
  const guest = await memberPage(browser, server, member);
  await guest.goto(`${server.url}/w/tests/settings`);
  const seen = section(guest, "groups");
  await expect(seen.getByRole("listitem").filter({ hasText: "Editors" })).toContainText("1 member");
  await seen.getByRole("button", { name: "members of Editors" }).click();
  await expect(seen).toContainText("grace@example.org");
  await expect(seen.getByLabel("group name")).toHaveCount(0);
  await expect(seen.getByRole("button", { name: "remove Grace from Editors" })).toHaveCount(0);
  await expect(seen.getByRole("button", { name: "delete the group" })).toHaveCount(0);

  await groups.getByRole("button", { name: "remove Grace from Editors" }).click();
  await expect(editors).toContainText("0 members");

  // A member removed from the workspace leaves its groups, and the count follows.
  await groups.getByLabel("add to Editors").selectOption({ label: "Grace" });
  await groups.getByRole("button", { name: "add", exact: true }).click();
  await expect(editors).toContainText("1 member");
  await section(page, "members").getByRole("button", { name: "remove" }).click();
  await section(page, "members").getByRole("button", { name: "remove Grace" }).click();
  await expect(editors).toContainText("0 members");

  await groups.getByRole("button", { name: "delete the group" }).click();
  await groups.getByRole("button", { name: "delete Editors for good?" }).click();
  await expect(editors).toHaveCount(0);
  const { workspaceId } = await server.session();
  expect((await server.call("GET", `/workspaces/${workspaceId}/groups`)).items).toEqual([]);
});

test("a group grant raises its members above their own grant, and the open page follows", async ({
  page,
  server,
  browser,
}) => {
  const id = await newPage(server, "Plans");
  const member = await server.addMember("grace@example.org");
  const { workspaceId, spaceId } = await server.session();
  await server.call("PUT", `/spaces/${spaceId}/grants`, { grants: [{ user_id: member.id, role: "viewer" }] });
  const group = await server.call("POST", `/workspaces/${workspaceId}/groups`, { name: "Writers" });
  await server.call("PUT", `/groups/${group.id}/members/${member.id}`);
  const guest = await memberPage(browser, server, member);
  await guest.goto(`${server.url}/?page=${id}`);
  await expect(editor(guest)).toHaveAttribute("contenteditable", "false");

  await asOwner(page, server);
  await page.goto(`${server.url}/w/tests/settings/pages`);
  const access = section(page, "access");
  const writers = access.getByRole("listitem").filter({ hasText: "Writers" });
  await expect(writers).toContainText("1 member");
  await expect(access.getByLabel("access of the group Writers")).toHaveValue("");
  await access.getByLabel("access of the group Writers").selectOption("editor");
  await expect(notice(page)).toContainText("access changed");
  await expect(editor(guest)).toHaveAttribute("contenteditable", "true");
  // The socket reconnects after its `4409`: a demotion before then would reach no socket.
  await expect(status(guest)).toHaveText(/^synced(?!.*read only)/);
  // Her own grant stays, and the screen says where the higher one comes from.
  const grants = await server.call("GET", `/spaces/${spaceId}/grants`);
  expect(grants.items).toMatchObject([
    { user_id: member.id, role: "viewer" },
    { group_id: group.id, role: "editor" },
  ]);
  await expect(access.getByLabel("access of Grace")).toHaveValue("viewer");
  await expect(access.getByRole("listitem").filter({ hasText: "grace@example.org" })).toContainText(
    "editor through Writers",
  );

  // Without the group's grant, her own applies again.
  await access.getByLabel("access of the group Writers").selectOption("");
  await expect(editor(guest)).toHaveAttribute("contenteditable", "false");
  await expect(access.getByRole("listitem").filter({ hasText: "grace@example.org" })).not.toContainText(
    "through Writers",
  );
});

test("a group opened while another loads shows its own members, and a group made meanwhile shows", async ({
  page,
  server,
}) => {
  const member = await server.addMember("grace@example.org");
  const { workspaceId } = await server.session();
  const slow = await server.call("POST", `/workspaces/${workspaceId}/groups`, { name: "Slow" });
  const quick = await server.call("POST", `/workspaces/${workspaceId}/groups`, { name: "Quick" });
  await server.call("PUT", `/groups/${quick.id}/members/${member.id}`);
  let release = () => {};
  const held = new Promise<void>((done) => (release = done));
  await page.route(`**/api/v1/groups/${slow.id}/members*`, async (route) => {
    await held;
    await route.fallback();
  });
  await asOwner(page, server);
  await page.goto(`${server.url}/w/tests/settings`);
  const groups = section(page, "groups");
  await groups.getByRole("button", { name: "members of Slow" }).click();
  await groups.getByLabel("group name").fill("Fresh");
  await groups.getByRole("button", { name: "create group" }).click();
  await expect(groups.getByRole("listitem").filter({ hasText: "Fresh" })).toContainText("0 members");

  await groups.getByRole("button", { name: "members of Quick" }).click();
  await expect(groups.getByRole("button", { name: "remove Grace from Quick" })).toBeVisible();
  await expect(groups.getByLabel("add to Quick")).toBeVisible();
  // The slow answer, for a group no longer open, changes nothing.
  release();
  await page.waitForTimeout(500);
  await expect(groups.getByRole("button", { name: "remove Grace from Quick" })).toBeVisible();
});

test("a group does not say everyone is in it while the workspace's members are unknown", async ({
  page,
  server,
}) => {
  const { workspaceId } = await server.session();
  await server.call("POST", `/workspaces/${workspaceId}/groups`, { name: "Writers" });
  await page.route("**/api/v1/workspaces/*/members*", (route) =>
    route.request().method() === "GET" ? route.fulfill({ status: 500, body: "" }) : route.fallback(),
  );
  await asOwner(page, server);
  await page.goto(`${server.url}/w/tests/settings`);
  const groups = section(page, "groups");
  await groups.getByRole("button", { name: "members of Writers" }).click();
  await expect(groups).toContainText("nobody is in this group yet.");
  await expect(groups).not.toContainText("every member of the workspace is in it.");
  await expect(groups.getByLabel("add to Writers")).toHaveCount(0);
});

test("a group whose members fail to load says so, until another group opens", async ({ page, server }) => {
  const { workspaceId } = await server.session();
  const broken = await server.call("POST", `/workspaces/${workspaceId}/groups`, { name: "Broken" });
  await server.call("POST", `/workspaces/${workspaceId}/groups`, { name: "Fine" });
  await page.route(`**/api/v1/groups/${broken.id}/members*`, (route) => route.fulfill({ status: 500, body: "" }));
  await asOwner(page, server);
  await page.goto(`${server.url}/w/tests/settings`);
  const groups = section(page, "groups");
  await groups.getByRole("button", { name: "members of Broken" }).click();
  await expect(groups.getByRole("alert")).toBeVisible();
  await groups.getByRole("button", { name: "members of Fine" }).click();
  await expect(groups).toContainText("nobody is in this group yet.");
  await expect(groups.getByRole("alert")).toHaveCount(0);
});

test("a language picked on the account screen redraws it, and outlives a reload", async ({
  page,
  server,
}) => {
  await asOwner(page, server);
  await page.goto(`${server.url}/account`);
  const profile = page.getByRole("region", { name: "profile" });
  await profile.getByLabel("language").selectOption("fr");
  await profile.getByRole("button", { name: "save" }).click();
  await expect(page.getByRole("heading", { name: "compte", level: 1 })).toBeVisible();
  await expect(page.locator("html")).toHaveAttribute("lang", "fr");
  await expect(page.getByRole("status").filter({ hasText: "enregistré" })).toBeVisible();
  await page.reload();
  const profil = page.getByRole("region", { name: "profil" });
  await profil.getByLabel("langue").selectOption("");
  await profil.getByRole("button", { name: "enregistrer" }).click();
  await expect(page.getByRole("heading", { name: "account", level: 1 })).toBeVisible();

  // The browser's own language picked by name redraws nothing: one confirmation, not two.
  await page.getByRole("region", { name: "profile" }).getByLabel("language").selectOption("en");
  await page.reload();
  const same = page.getByRole("region", { name: "profile" });
  await same.getByLabel("language").selectOption("en");
  await same.getByRole("button", { name: "save" }).click();
  const saved = page.getByRole("status").filter({ hasText: "saved" });
  await expect(saved.first()).toBeVisible();
  // Read once: a retried count would wait for the notice to leave.
  expect(await saved.count()).toBe(1);
});

test("an existing account is suggested, and joins from the workspace selector without the link", async ({
  page,
  server,
  browser,
}) => {
  const member = await server.addMember("grace@example.org");
  const second = await server.call("POST", "/workspaces", { name: "Second" });
  const third = await server.call("POST", "/workspaces", { name: "Third" });
  const body = { email: "grace@example.org", role: "member" };
  await server.call("POST", `/workspaces/${third.id}/invitations`, body);

  // An invitation made before the page loads shows at load, and can be declined.
  const grace = await memberPage(browser, server, member);
  await grace.goto(`${server.url}/w/tests`);
  const selector = grace.locator(".selector");
  await expect(selector.locator("summary")).toContainText("1 invitation");
  await selector.locator("summary").click();
  const fromThird = selector.getByRole("listitem").filter({ hasText: "Third" });
  await fromThird.getByRole("button", { name: "decline the invitation to Third" }).click();
  await expect(fromThird).toHaveCount(0);
  expect((await server.call("GET", `/workspaces/${third.id}/invitations`)).items).toEqual([]);

  // The form suggests the account after 3 characters, and still takes any address.
  await asOwner(page, server);
  await page.goto(`${server.url}/w/${second.slug}/settings`);
  const invitations = section(page, "invitations");
  const email = invitations.getByLabel("email");
  const people = invitations.getByRole("listbox", { name: "people" });
  await email.fill("gr");
  await page.waitForTimeout(400);
  await expect(people.getByRole("option")).toHaveCount(0);
  await email.fill("gra");
  const suggestion = people.getByRole("option", { name: /Grace/ });
  await expect(suggestion).toContainText("grace@example.org");
  await email.press("ArrowDown");
  await email.press("Enter");
  await expect(email).toHaveValue("grace@example.org");
  await expect(people.getByRole("option")).toHaveCount(0);
  await invitations.getByRole("button", { name: "invite" }).click();
  await expect(invitations.getByRole("listitem").filter({ hasText: "grace@example.org" })).toBeVisible();

  // The event brings it to the open selector, and joining needs no link.
  const fromSecond = selector.getByRole("listitem").filter({ hasText: "Second" });
  await fromSecond.getByRole("button", { name: "join Second" }).click();
  await expect(grace).toHaveURL(`${server.url}/w/${second.slug}`);
  await expect(selector.locator("summary")).not.toContainText("invitation");
  await expect(grace.getByRole("heading", { level: 1 })).toHaveText("Second");
});

test("suggestions close when the form submits, and a link accepted in the tab leaves the selector", async ({
  page,
  server,
  browser,
}) => {
  const member = await server.addMember("grace@example.org");
  const second = await server.call("POST", "/workspaces", { name: "Second" });
  await asOwner(page, server);
  await page.goto(`${server.url}/w/${second.slug}/settings`);
  const invitations = section(page, "invitations");
  const email = invitations.getByLabel("email");
  await email.fill("grace@example.org");
  await expect(invitations.getByRole("listbox", { name: "people" }).getByRole("option")).toHaveCount(1);
  await email.press("Enter");
  const link = await invitations.getByLabel("invitation link", { exact: true }).inputValue();
  await expect(email).toHaveValue("");
  await page.waitForTimeout(400);
  await expect(invitations.getByRole("listbox", { name: "people" })).toHaveCount(0);

  const grace = await memberPage(browser, server, member);
  await grace.goto(`${server.url}/w/tests`);
  await expect(grace.locator(".selector summary")).toContainText("1 invitation");
  await grace.evaluate((to) => {
    history.pushState({}, "", new URL(to).pathname);
    dispatchEvent(new PopStateEvent("popstate"));
  }, link);
  await grace.getByRole("button", { name: "join Second" }).click();
  await expect(grace).toHaveURL(`${server.url}/w/${second.slug}`);
  await expect(grace.locator(".selector summary")).not.toContainText("invitation");
});
