import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "e2e",
  testMatch: "*.spec.ts",
  reporter: "list",
  timeout: 120_000,
  // A freshly launched browser can stall all its requests for several seconds, and a page needs
  // a few round trips before its socket opens. See `docs/notes/web.md`.
  expect: { timeout: 30_000 },
  use: {
    browserName: "chromium",
    launchOptions: { executablePath: process.env.CHROMIUM_PATH || undefined },
  },
});
