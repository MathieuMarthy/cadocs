// The check of `audit.ts`: which advisories of `npm audit --json` fail the build.

import assert from "node:assert/strict";
import { test } from "node:test";

import { verdict } from "./audit.ts";

const advisory = (id: string, severity: string) => ({
  source: 1,
  name: "pkg",
  title: `advisory ${id}`,
  url: `https://github.com/advisories/${id}`,
  severity,
});

const report = (...via: object[]) => ({
  vulnerabilities: {
    pkg: { severity: "high", via },
    // A package vulnerable only through another names it, and adds no advisory of its own.
    parent: { severity: "high", via: ["pkg"] },
  },
});

test("a high or critical advisory fails, a lower one does not", () => {
  const found = verdict(report(advisory("GHSA-aaaa", "high"), advisory("GHSA-bbbb", "critical"), advisory("GHSA-cccc", "moderate")), {});
  assert.deepEqual(found.failing.map((item) => item.id), ["GHSA-aaaa", "GHSA-bbbb"]);
});

test("an ignored advisory does not fail, and one ignored but gone is reported", () => {
  const found = verdict(report(advisory("GHSA-aaaa", "high")), { "GHSA-aaaa": "reason", "GHSA-gone": "reason" });
  assert.deepEqual(found.failing, []);
  assert.deepEqual(found.ignored, ["GHSA-aaaa"]);
  assert.deepEqual(found.stale, ["GHSA-gone"]);
});

test("an advisory reached through several packages counts once", () => {
  const twice = {
    vulnerabilities: {
      a: { severity: "high", via: [advisory("GHSA-aaaa", "high")] },
      b: { severity: "high", via: [advisory("GHSA-aaaa", "high")] },
    },
  };
  assert.deepEqual(verdict(twice, {}).failing.map((item) => item.id), ["GHSA-aaaa"]);
});
