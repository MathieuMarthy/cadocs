/** `web/public/theme.js` applies the choice before the first paint, and this changes it. */

const KEY = "cadocs-theme";
const ORDER = ["auto", "light", "dark"] as const;

type Theme = (typeof ORDER)[number];

function stored(): Theme {
  const value = document.documentElement.dataset.theme;
  return value === "light" || value === "dark" ? value : "auto";
}

export const theme = $state({ current: stored() });

export function nextTheme() {
  const next = ORDER[(ORDER.indexOf(theme.current) + 1) % ORDER.length]!;
  theme.current = next;
  if (next === "auto") delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = next;
  try {
    if (next === "auto") localStorage.removeItem(KEY);
    else localStorage.setItem(KEY, next);
  } catch {
    // Storage can be blocked. The choice then lasts until the page closes.
  }
}
