// Math and Mermaid (step 43): KaTeX and Mermaid load on the first formula or diagram shown, and
// every browser renders what the others type.

import { expect, type Page } from "@playwright/test";

import { bothOnline, command, editor, grant, newLine, status, test } from "./editing.ts";

const formula = (page: Page) => editor(page).locator('span[data-type="inlineMath"]');
const preview = (page: Page) => editor(page).locator(".code-preview");

test("inline math typed between dollars renders in both browsers, and a click edits it", async ({
  left,
  right,
}) => {
  const loaded: string[] = [];
  right.on("request", (request) => loaded.push(request.url()));
  await bothOnline(left, right);
  // Nothing of KaTeX or Mermaid comes with the editor.
  expect(loaded.filter((url) => /katex|mermaid/.test(url))).toEqual([]);

  await editor(left).click();
  await left.keyboard.type("Euler: $e^{i\\pi}$ done");
  await expect(formula(right).locator(".katex")).toBeVisible();
  await expect(formula(right).locator("annotation")).toHaveText("e^{i\\pi}");
  expect(loaded.some((url) => url.includes("katex"))).toBe(true);
  expect(loaded.some((url) => url.includes("mermaid"))).toBe(false);

  await formula(left).click();
  const source = formula(left).getByRole("textbox", { name: "formula" });
  await expect(source).toHaveValue("e^{i\\pi}");
  await source.fill("x^2");
  await source.press("Enter");
  await expect(formula(right).locator("annotation")).toHaveText("x^2");
  // The caret is back in the text, after the formula.
  await left.keyboard.type("!");
  await expect(editor(right).locator("p").first()).toContainText("x2!");
  // The arrows pass over a formula without opening its field.
  for (let i = 0; i < 3; i++) await left.keyboard.press("ArrowLeft");
  await expect(source).toHaveCount(0);
  await left.keyboard.type("?");
  await expect(editor(right).locator("p").first()).toContainText("Euler: ?");
  // Enter on a formula the arrows selected opens its field.
  await left.keyboard.press("ArrowRight");
  await left.keyboard.press("Enter");
  await expect(source).toHaveValue("x^2");
  await source.press("Escape");
  await expect(formula(right).locator("annotation")).toHaveText("x^2");

  // From the slash menu, the source is asked for at once.
  await newLine(left);
  await command(left, "inline math");
  await left.keyboard.type("a+b");
  await left.keyboard.press("Enter");
  await expect(formula(right).nth(1).locator("annotation")).toHaveText("a+b");
});

test("dollars around a mention make no formula and keep the text", async ({ left, right }) => {
  await bothOnline(left, right);
  await editor(left).click();
  await left.keyboard.type("keep $a ");
  await left.keyboard.type("@gra");
  await expect(left.getByRole("listbox", { name: "people" }).getByRole("option")).toHaveText(["Grace"]);
  await left.keyboard.press("Enter");
  await left.keyboard.type(" b$ end");
  await expect(editor(right).locator(".mention")).toHaveText("@Grace");
  await expect(editor(right).locator("p").first()).toContainText("keep $a @Grace b$ end");
  await expect(formula(right)).toHaveCount(0);
});

test("a math block and a diagram render under their source, which readers do not see", async ({
  left,
  right,
  server,
  member,
}) => {
  await bothOnline(left, right);
  await editor(left).click();
  await command(left, "math block");
  await left.keyboard.type("\\frac{a}{b}");
  await expect(preview(right).locator(".katex-display annotation")).toHaveText("\\frac{a}{b}");

  await newLine(left);
  await newLine(left);
  await command(left, "diagram");
  await left.keyboard.type("graph TD");
  await left.keyboard.press("Enter");
  await left.keyboard.type("A-->B");
  await expect(preview(right).locator("svg")).toBeVisible();
  await expect(preview(right).locator("svg")).toContainText("B");

  // A source Mermaid cannot read says so, and the formula above still shows.
  await left.keyboard.type(" -->");
  await expect(preview(right).nth(1)).toHaveText("diagram error");
  await left.keyboard.press("Backspace");
  await left.keyboard.press("Backspace");
  await left.keyboard.press("Backspace");
  await left.keyboard.press("Backspace");
  await expect(preview(right).nth(1).locator("svg")).toBeVisible();

  await grant(server, member, "viewer");
  await expect(status(right)).toContainText("read only");
  await expect(editor(right).locator("pre").first()).toBeHidden();
  await expect(preview(right).first()).toBeVisible();
});
