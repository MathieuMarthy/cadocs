import { execSync } from "node:child_process";

import { svelte } from "@sveltejs/vite-plugin-svelte";
import { defineConfig, type Plugin } from "vite";

import { version } from "./package.json";

// Fontsource lists woff beside woff2. Every browser cadocs supports takes woff2 first, so the
// woff files would only make the binary larger.
const dropWoff: Plugin = {
  name: "drop-woff",
  generateBundle(_options, bundle) {
    for (const name of Object.keys(bundle)) {
      if (name.endsWith(".woff")) delete bundle[name];
    }
  },
};

/** The commit being built, or the release tag when the build has no Git history (ADR 0010). */
function sourceRef() {
  try {
    return execSync("git rev-parse HEAD", { stdio: ["ignore", "pipe", "ignore"] })
      .toString()
      .trim();
  } catch {
    return `v${version}`;
  }
}

export default defineConfig({
  plugins: [svelte(), dropWoff],
  define: {
    __VERSION__: JSON.stringify(version),
    __SOURCE_URL__: JSON.stringify(
      `https://github.com/MathieuMarthy/cadocs/tree/${sourceRef()}`,
    ),
  },
  build: {
    // The CSP has no `data:` for fonts or scripts, so nothing may be inlined.
    assetsInlineLimit: 0,
  },
  server: {
    proxy: {
      "/ws": { target: "ws://127.0.0.1:3000", ws: true },
      "/api": { target: "http://127.0.0.1:3000" },
    },
  },
});
