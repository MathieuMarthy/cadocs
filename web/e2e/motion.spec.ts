// Motion (step 27): tree levels, menus, the notice and the confirm buttons move, briefly, and
// nothing moves for a reader who asks for reduced motion. Svelte bakes its easing into the
// keyframes, so linearity is left to `lib/motion.ts`.

import { expect, type Page } from "@playwright/test";

import { asOwner, test } from "./fixtures.ts";
import type { TestServer } from "./server.ts";

/** What moved: one entry per animation that lasted, named by its element. */
interface Move {
  what: string;
  duration: number;
  /** The tallest height its keyframes reach, in pixels, or 0 without one. */
  height: number;
}

declare global {
  interface Window {
    moved: Move[];
  }
}

/** Records every animation the page starts, Svelte transitions included, which use `animate`. */
function recordMotion() {
  window.moved = [];
  const animate = Element.prototype.animate;
  Element.prototype.animate = function (this: Element, keyframes, options) {
    const animation = animate.call(this, keyframes, options);
    const timing = animation.effect?.getTiming();
    const duration = Number(timing?.duration ?? 0);
    if (duration > 0) {
      const what = this.getAttribute("role") ?? this.className.toString();
      const heights =
        animation.effect instanceof KeyframeEffect ? animation.effect.getKeyframes() : [];
      const height = Math.max(
        0,
        ...heights.map((frame) => Number.parseFloat(String(frame.height)) || 0),
      );
      window.moved.push({ what, duration, height });
    }
    return animation;
  };
  // CSS transitions and animations do not go through `animate`.
  for (const type of ["transitionrun", "animationstart"]) {
    document.addEventListener(type, (event) => {
      const what = `${type} ${(event.target as Element).className}`;
      window.moved.push({ what, duration: 1, height: 0 });
    });
  }
}

const moved = (page: Page) => page.evaluate(() => window.moved.splice(0));

/** Nothing is moving, CSS transitions and animations included. */
const still = (page: Page) => page.evaluate(() => document.getAnimations().length);

async function newPage(server: TestServer, title: string, parent_id?: string) {
  const { spaceId } = await server.session();
  const page = await server.call("POST", `/spaces/${spaceId}/pages`, {
    parent_id,
  });
  await server.call("PATCH", `/pages/${page.id}`, { title });
  await server.titleStored(page.id, title);
  return page.id as string;
}

/**
 * Opens and closes a tree level, a menu, the notice and the confirm buttons, and gives what moved
 * for each, once the screen is still again.
 */
async function walk(page: Page, server: TestServer) {
  const parent = await newPage(server, "Parent");
  await newPage(server, "Child", parent);
  const gone = await newPage(server, "Gone");
  await server.call("DELETE", `/pages/${gone}`);
  await asOwner(page, server);
  await page.addInitScript(recordMotion);
  await page.goto(`${server.url}/w/tests`);
  const tree = page.getByRole("navigation", { name: "spaces" });
  await expect(tree.getByRole("button", { name: "Pages/", exact: true })).toBeVisible();
  await expect.poll(() => still(page)).toBe(0);
  await moved(page);
  const seen: Record<string, Move[]> = {};
  const settle = async (name: string) => {
    await expect.poll(() => still(page)).toBe(0);
    seen[name] = await moved(page);
  };

  await tree.getByRole("button", { name: "Pages/", exact: true }).click();
  await tree.getByRole("button", { name: "expand Parent" }).click();
  await expect(tree.getByRole("link", { name: "Child", exact: true })).toBeVisible();
  await settle("tree opens");
  await tree.getByRole("button", { name: "collapse Parent" }).click();
  await expect(tree.getByRole("link", { name: "Child", exact: true })).toBeHidden();
  await settle("tree closes");

  const row = tree.getByRole("group").filter({ hasText: "Parent/" });
  await row.hover();
  await row.getByRole("button", { name: "more actions for Parent" }).click();
  await expect(page.getByRole("menu")).toBeVisible();
  await settle("menu opens");
  await page.keyboard.press("Escape");
  await expect(page.getByRole("menu")).toBeHidden();
  await settle("menu closes");

  await row.hover();
  await row.getByRole("button", { name: "more actions for Parent" }).click();
  await page.getByRole("menuitem", { name: "move to trash" }).click();
  const notice = page.getByRole("status").filter({ hasText: "moved to trash" });
  await expect(notice).toBeVisible();
  await settle("notice opens");
  await notice.getByRole("button", { name: "undo" }).click();
  await expect(notice).toBeHidden();
  await settle("notice closes");

  await page.goto(`${server.url}/w/tests/trash/pages`);
  const trashed = page.getByRole("main").getByRole("listitem").filter({ hasText: "Gone" });
  await expect.poll(() => still(page)).toBe(0);
  await moved(page);
  await trashed.getByRole("button", { name: "delete forever" }).click();
  await expect(trashed.getByRole("button", { name: "cancel" })).toBeVisible();
  await settle("confirm opens");
  await trashed.getByRole("button", { name: "cancel" }).click();
  await expect(trashed.getByRole("button", { name: "delete forever" })).toBeVisible();
  await settle("confirm closes");
  return seen;
}

test("tree levels, menus, the notice and the confirm buttons move briefly", async ({
  page,
  server,
}) => {
  const seen = await walk(page, server);
  const expected = {
    "tree opens": "rows",
    "tree closes": "rows",
    "menu opens": "menu",
    "menu closes": "menu",
    "notice opens": "status",
    "notice closes": "status",
    "confirm opens": "button",
    "confirm closes": "icon-button",
  };
  for (const [name, what] of Object.entries(expected)) {
    const moves = seen[name] ?? [];
    expect(
      moves.map((move) => move.what).some((each) => each.split(" ").includes(what)),
      name,
    ).toBe(true);
    for (const move of moves) {
      expect(move.duration, name).toBeLessThanOrEqual(200);
    }
  }
  // A level unfolds to the height of its rows, not from 0 to 0 while they load.
  for (const name of ["tree opens", "tree closes"]) {
    const rows = (seen[name] ?? []).filter((move) => move.what === "rows");
    expect(Math.max(0, ...rows.map((move) => move.height)), name).toBeGreaterThan(0);
  }
});

test.describe("with reduced motion", () => {
  test.use({ reducedMotion: "reduce" });

  test("nothing moves", async ({ page, server }) => {
    const seen = await walk(page, server);
    expect(seen).toEqual({
      "tree opens": [],
      "tree closes": [],
      "menu opens": [],
      "menu closes": [],
      "notice opens": [],
      "notice closes": [],
      "confirm opens": [],
      "confirm closes": [],
    });
  });
});
