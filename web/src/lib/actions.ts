// Page actions that the tree and the page's top bar share, so they say the same thing.

import type { Nav, PageRow } from "./nav.svelte";
import { notify } from "./notice.svelte";
import { navigate, reopened, shownPage } from "./router.svelte";
import { t } from "./i18n";

export const pageLink = (id: string) => new URL(`/?page=${id}`, location.origin).href;

/** Copies `text` and says so in the notice, by default as a link. */
export async function copy(text: string, copied = t("copy.link"), failed = t("copy.linkFailed")) {
  try {
    await navigator.clipboard.writeText(text);
    notify(copied);
  } catch {
    notify(failed);
  }
}

/**
 * The page on screen, after its ancestors from the root, which the page view keeps up to date so
 * a trash from anywhere knows whether it takes that page away.
 */
let onScreen: string[] = [];

/** Records the page on screen and its ancestors, and returns what forgets them. */
export function showPage(path: string[]) {
  onScreen = path;
  return () => {
    if (onScreen === path) onScreen = [];
  };
}

/**
 * Moves a page to trash, with an `undo` that restores it. When that takes the page on screen
 * away, itself or under it, the parent opens, or the workspace for a page at the root.
 */
export async function trashPage(nav: Nav, page: PageRow) {
  const shown = shownPage();
  const away = shown !== null && (shown === page.id || onScreen.includes(page.id));
  await nav.trash(page);
  const left = away ? (page.parent_id ? `/?page=${page.parent_id}` : `/w/${nav.workspace.slug}`) : null;
  if (left) navigate(left);
  notify(t("page.trashed"), async () => {
    await nav.restore(page);
    if (left && shown && location.pathname + location.search === left) {
      // Still where the trash sent us: go back to the page that showed.
      navigate(`/?page=${shown}`);
    } else if (shownPage() === page.id || (left && shownPage() === shown)) {
      // The page view gave up on it, or on a page under it, after going back: open it again.
      reopened.count += 1;
    }
  });
}
