// Step 19: revisions, from edits in one browser to a restore that reaches the other.

import { expect } from "@playwright/test";

import { DOC, editor, expectText, test as base } from "./editing.ts";
import { TestServer } from "./server.ts";

/** A revision a second after typing stops, so a test does not wait two minutes. */
const test = base.extend({
  server: async ({}, use) => {
    const server = await TestServer.start({
      CADOCS_REVISION_PAUSE_SECS: "1",
      CADOCS_REVISION_MAX_SECS: "60",
    });
    await use(server);
    await server.dispose();
  },
});

/** Waits until the page has `count` revisions. */
async function revisions(server: TestServer, count: number) {
  const id = await server.page(DOC);
  await expect(async () => {
    const page = await server.call("GET", `/pages/${id}/revisions`);
    expect(page.items).toHaveLength(count);
  }).toPass({ timeout: 20_000 });
}

test("a revision restored in one browser reaches the other, and the replaced content stays", async ({
  server,
  left,
  right,
}) => {
  await left.getByLabel("page title").click();
  await left.keyboard.type("Plans");
  await editor(left).click();
  await left.keyboard.type("first draft");
  await revisions(server, 1);
  await left.keyboard.press("Control+a");
  await left.keyboard.type("second draft");
  await revisions(server, 2);
  await expectText(editor(right).locator("p").first(), "second draft");

  await left.getByRole("button", { name: "more actions for Plans" }).click();
  await left.getByRole("menuitem", { name: "history" }).click();
  await expect(left.getByRole("heading", { name: "history", level: 1 })).toBeVisible();
  const list = left.getByRole("navigation", { name: "revisions" });
  await expect(list.getByRole("link")).toHaveText(["#2 Plans", "#1 Plans"]);
  const shown = (number: number) => left.getByRole("region", { name: `revision ${number}` });
  await expect(shown(2)).toContainText("second draft");

  await list.getByRole("link", { name: "#1 Plans" }).click();
  await expect(shown(1)).toContainText("first draft");
  await shown(1).getByRole("button", { name: "restore" }).click();
  await left.getByRole("button", { name: "restore revision 1" }).click();

  await expect(left).toHaveURL(/\?page=[^&]+$/);
  await expectText(editor(left).locator("p").first(), "first draft");
  await expectText(editor(right).locator("p").first(), "first draft");
  // The restore is an edit like any other: it gets a revision of its own, after the one it kept.
  await revisions(server, 3);
  const id = await server.page(DOC);
  const latest = await server.call("GET", `/pages/${id}/revisions/3`);
  expect(latest.html).toBe("<p>first draft</p>");
});

test("a reader sees the history but cannot restore", async ({ server, member, right }) => {
  const id = await server.page(DOC);
  const { spaceId } = await server.session();
  await server.call("PUT", `/spaces/${spaceId}/grants`, {
    grants: [{ user_id: member.id, role: "viewer" }],
  });
  await right.goto(`${server.url}/?page=${id}&history`);
  await expect(right.getByRole("heading", { name: "history", level: 1 })).toBeVisible();
  await expect(right.getByText("no revision yet")).toBeVisible();
  await expect(right.getByRole("button", { name: "restore" })).toHaveCount(0);
});
