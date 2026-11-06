import { defineConfig, type Plugin } from "vite";

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

export default defineConfig({
  plugins: [dropWoff],
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
