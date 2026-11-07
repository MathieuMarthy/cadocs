/** The address of the page. The server answers `index.html` for every route (`assets.rs`). */

export const route = $state({ path: location.pathname, search: location.search });

/** Bumped to mount the page view again, as after its page comes back from the trash. */
export const reopened = $state({ count: 0 });

/** The page the address shows, if any. */
export const shownPage = () =>
  route.path === "/" ? new URLSearchParams(route.search).get("page") : null;

function sync() {
  route.path = location.pathname;
  route.search = location.search;
}

export function navigate(to: string, { replace = false } = {}) {
  if (replace) history.replaceState(null, "", to);
  else history.pushState(null, "", to);
  sync();
  if (!replace) scrollTo(0, 0);
}

/** A `next` parameter comes from the address, so it must stay on this site. */
export function safeNext(next: string | null) {
  try {
    // The parser decides, since it drops tabs and turns backslashes into slashes.
    const url = new URL(next ?? "/", location.origin);
    return url.origin === location.origin ? url.pathname + url.search + url.hash : "/";
  } catch {
    return "/";
  }
}

/** First segments the server answers itself, as `is_front_end_route` in `assets.rs` lists them. */
const SERVER_PATHS = ["api", "assets", "licenses", "ws"];

function followLink(event: MouseEvent) {
  const modified = event.metaKey || event.ctrlKey || event.shiftKey || event.altKey;
  if (event.defaultPrevented || event.button !== 0 || modified) return;
  const link = (event.target as Element).closest("a");
  if (!link || link.target || link.origin !== location.origin) return;
  if (SERVER_PATHS.includes(link.pathname.split("/")[1] ?? "")) return;
  event.preventDefault();
  navigate(link.pathname + link.search + link.hash);
}

export function startRouter() {
  addEventListener("popstate", sync);
  document.addEventListener("click", followLink);
}
