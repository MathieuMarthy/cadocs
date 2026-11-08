// Page actions that the tree and the page's top bar share, so they say the same thing.

import type { Nav, PageRow } from "./nav.svelte";
import { notify } from "./notice.svelte";
import { reopened, shownPage } from "./router.svelte";

export const pageLink = (id: string) => new URL(`/?page=${id}`, location.origin).href;

/** Copies `text` and says so in the notice. */
export async function copy(text: string, what = "link") {
  try {
    await navigator.clipboard.writeText(text);
    notify(`${what} copied`);
  } catch {
    notify(`could not copy the ${what}`);
  }
}

/** Moves a page to trash, with an `undo` that restores it. */
export async function trashPage(nav: Nav, page: PageRow) {
  await nav.trash(page);
  notify("moved to trash", async () => {
    await nav.restore(page);
    // The page view gave up on it: open it again.
    if (shownPage() === page.id) reopened.count += 1;
  });
}
