// `npm audit` with an ignore list, as `deny.toml` keeps one for Rust: a high or critical advisory
// fails, unless it is listed below with its reason. npm has no ignore flag of its own. Run by the
// security workflow as `node scripts/audit.ts`; lower severities only show in the log.

import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

/** Each advisory let through, by its GHSA id, with why. Remove one once a fixed version exists. */
export const IGNORED: Record<string, string> = {
  // ReDoS in picomatch <= 4.0.7, with no fixed version yet. Only Vite and its globbing use it, at
  // build time, on the project's own patterns: nothing of it ships in web/dist or the binary.
  "GHSA-fc5q-39xc-vjpf": "build-time only, no fixed version (picomatch <= 4.0.7)",
};

const FAILING = new Set(["high", "critical"]);

type Advisory = { url?: string; title?: string; name?: string; severity?: string };
type Report = { vulnerabilities?: Record<string, { via?: (string | Advisory)[] }> };

export type Found = { id: string; title: string; name: string; severity: string };

/** What the report fails on, what the list lets through, and listed ids the report no longer has. */
export function verdict(report: Report, ignored: Record<string, string>) {
  const seen = new Map<string, Found>();
  for (const entry of Object.values(report.vulnerabilities ?? {})) {
    // A string names another vulnerable package, whose own entry carries the advisory.
    for (const via of entry.via ?? []) {
      if (typeof via === "string" || !via.url) continue;
      const id = via.url.split("/").pop() ?? via.url;
      seen.set(id, { id, title: via.title ?? "", name: via.name ?? "", severity: via.severity ?? "" });
    }
  }
  const high = [...seen.values()].filter((item) => FAILING.has(item.severity));
  return {
    failing: high.filter((item) => !(item.id in ignored)),
    ignored: high.filter((item) => item.id in ignored).map((item) => item.id),
    stale: Object.keys(ignored).filter((id) => !seen.has(id)),
  };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const cwd = fileURLToPath(new URL("..", import.meta.url));
  // Exits non-zero whenever it finds anything, so only a missing report is an error here.
  const run = spawnSync("npm", ["audit", "--json"], { cwd, encoding: "utf8", maxBuffer: 64 << 20 });
  const report = JSON.parse(run.stdout || "null") as (Report & { error?: unknown }) | null;
  if (!report || report.error) {
    console.error("npm audit gave no report:", run.stderr || JSON.stringify(report?.error));
    process.exit(1);
  }
  const { failing, ignored, stale } = verdict(report, IGNORED);
  for (const id of ignored) console.log(`ignored ${id}: ${IGNORED[id]}`);
  for (const id of stale) console.log(`no longer found, remove it from the list: ${id}`);
  for (const item of failing) {
    console.error(`${item.severity} ${item.id} in ${item.name}: ${item.title}`);
  }
  if (failing.length) process.exit(1);
  console.log("no high or critical advisory beyond the list");
}
