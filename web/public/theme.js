// Runs before the first paint, as a file because the CSP refuses inline scripts.
try {
  const theme = localStorage.getItem("cadocs-theme");
  if (theme === "light" || theme === "dark") document.documentElement.dataset.theme = theme;
} catch {
  // Storage can be blocked. The system theme applies.
}
