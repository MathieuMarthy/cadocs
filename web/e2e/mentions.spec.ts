// Mentions (step 39): `@` in the page and in comments, `[[` in comments, names and titles shown
// as each reader may see them, and a comment body that stays text.

import { expect, type Page } from "@playwright/test";

import { bothOnline, command, DOC, editor, expectText, newLine, status, test } from "./editing.ts";

const people = (page: Page) => page.getByRole("listbox", { name: "people" });
const pages = (page: Page) => page.getByRole("listbox", { name: "pages" });
const panel = (page: Page) => page.getByRole("region", { name: "comments", exact: true });
const toggle = (page: Page) => page.getByRole("banner").getByRole("button", { name: "comments", exact: true });

test("a member mentioned with @ shows by name in both browsers", async ({ left, right }) => {
  await bothOnline(left, right);
  await editor(left).click();
  await left.keyboard.type("Ask ");
  await left.keyboard.type("@gra");
  await expect(people(left).getByRole("option")).toHaveText(["Grace"]);
  await left.keyboard.press("Enter");
  await expect(people(left)).toHaveCount(0);
  await left.keyboard.type("first.");

  await expect(editor(right).locator(".mention")).toHaveText("@Grace");
  await expectText(editor(right).locator("p").first(), "Ask @Grace first.");

  // From the slash menu too, and an address with `@` inside opens nothing.
  await newLine(left);
  await command(left, "mention someone");
  await left.keyboard.type("ada");
  await expect(people(left).getByRole("option")).toHaveText(["Ada"]);
  await left.keyboard.press("Enter");
  await left.keyboard.type("or mail ada@example.org");
  await expect(people(left)).toHaveCount(0);
  await expect(editor(right).locator(".mention")).toHaveText(["@Grace", "@Ada"]);
});

test("a comment mentions a member and a page, and its body stays text", async ({ left, right, server, member }) => {
  const target = await server.page("target");
  await server.call("PATCH", `/pages/${target}`, { title: "Deploy notes" });
  await server.titleStored(target, "Deploy notes");
  await bothOnline(left, right);

  await toggle(left).click();
  await panel(left).getByRole("button", { name: "comment on the page", exact: true }).click();
  const field = panel(left).getByRole("textbox", { name: "comment", exact: true });
  await field.click();
  await left.keyboard.type("<b>Ask</b> ");
  await left.keyboard.type("@gra");
  await expect(people(left).getByRole("option")).toHaveText(["Grace"]);
  await left.keyboard.press("Enter");
  await left.keyboard.type("about ");
  await left.keyboard.type("[[depl");
  await expect(pages(left).getByRole("option")).toHaveText(["Deploy notes"]);
  await left.keyboard.press("Enter");
  await expect(field).toHaveText("<b>Ask</b> @Grace about Deploy notes ");
  await panel(left).getByRole("button", { name: "comment", exact: true }).click();

  // Grace reads it with her name and the page's title, and the markup as typed.
  await toggle(right).click();
  const body = panel(right).locator(".comment-body");
  await expect(body).toHaveText("<b>Ask</b> @Grace about Deploy notes ");
  await expect(body.locator("b")).toHaveCount(0);
  await expect(body.locator(".mention")).toHaveText("@Grace");
  await expect(body.getByRole("link", { name: "Deploy notes" })).toHaveAttribute("href", `/?page=${target}`);

  // The stored body holds ids only, trimmed by the server.
  const { items } = await server.call("GET", `/pages/${await server.page(DOC)}/comments`);
  expect(items[0].body).toBe(`<b>Ask</b> <user:${member.id}> about <page:${target}>`);

  // Editing shows the mentions again as names, and keeps them.
  const own = panel(left).getByRole("article");
  await own.getByRole("button", { name: "actions on the comment by Ada" }).click();
  await left.getByRole("menuitem", { name: "edit" }).click();
  const edit = own.getByRole("textbox", { name: "edit the comment", exact: true });
  await expect(edit.locator(".mention")).toHaveText("@Grace");
  await edit.click();
  await left.keyboard.press("Control+End");
  await left.keyboard.type(" today");
  await own.getByRole("button", { name: "save", exact: true }).click();
  await expect(body).toHaveText("<b>Ask</b> @Grace about Deploy notes today");
  await expect(body.locator(".mention")).toHaveText("@Grace");
});

test("a page link clicked in a comment composer opens another tab and keeps the draft", async ({ left, server }) => {
  const target = await server.page("target");
  await server.call("PATCH", `/pages/${target}`, { title: "Deploy notes" });
  await server.titleStored(target, "Deploy notes");
  await expect(status(left)).toHaveText("synced · 1 online");

  await toggle(left).click();
  await panel(left).getByRole("button", { name: "comment on the page", exact: true }).click();
  const field = panel(left).getByRole("textbox", { name: "comment", exact: true });
  await field.click();
  await left.keyboard.type("see ");
  await left.keyboard.type("[[depl");
  await expect(pages(left).getByRole("option")).toHaveText(["Deploy notes"]);
  await left.keyboard.press("Enter");
  await left.keyboard.type("first");

  const opened = left.context().waitForEvent("page");
  await field.getByRole("link", { name: "Deploy notes" }).click();
  const tab = await opened;
  await expect(tab).toHaveURL(`${server.url}/?page=${target}`);
  await tab.close();
  await expect(left).toHaveURL(`${server.url}/?page=${await server.page(DOC)}`);
  await expect(field).toHaveText("see Deploy notes first");
});
