import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "e2e",
  testMatch: "*.spec.ts",
  reporter: "list",
  timeout: 60_000,
  expect: { timeout: 10_000 },
  use: {
    browserName: "chromium",
    launchOptions: { executablePath: process.env.CHROMIUM_PATH || undefined },
  },
});
