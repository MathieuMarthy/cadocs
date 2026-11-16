// The catalogue's functions (`src/lib/i18n`), loaded through Vite as `schema.ts` loads the editor.

import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import { test } from "node:test";

import { runnerImport } from "vite";

const { module: i18n } = await runnerImport<typeof import("../src/lib/i18n/index.ts")>(
  fileURLToPath(new URL("../src/lib/i18n/index.ts", import.meta.url)),
);

test("the person's choice comes first, if the interface has it", () => {
  assert.equal(i18n.pickLanguage("fr", ["de-DE", "en"], ["en", "fr"]), "fr");
  assert.equal(i18n.pickLanguage("xx", ["fr-CA"], ["en", "fr"]), "fr");
});

test("without a choice, the first browser language the interface has wins", () => {
  assert.equal(i18n.pickLanguage(null, ["de-DE", "fr-FR", "en"], ["en", "fr"]), "fr");
  assert.equal(i18n.pickLanguage(undefined, ["pt-BR", "pt"], ["en", "pt-br"]), "pt-br");
  assert.equal(i18n.pickLanguage(null, ["de", "ja"], ["en", "fr"]), "en");
  assert.equal(i18n.pickLanguage(null, [], ["en", "fr"]), "en");
});

test("parameters fill their places, and a missing one stays visible", () => {
  assert.equal(i18n.t("page.newIn", { parent: "Notes" }), "new page in Notes");
  assert.equal(i18n.t("history.restored", { number: 3 }), "revision 3 restored");
  assert.equal(i18n.t("page.newIn"), "new page in {parent}");
});

test("an unknown key shows as itself instead of failing", () => {
  assert.equal(i18n.t("role.guest" as never), "role.guest");
});

test("numeric dates follow the browser's first locale, as before step 34", () => {
  assert.equal(i18n.dateLocale("en", ["en-GB", "fr-FR"]), "en-GB");
  assert.equal(i18n.dateLocale("en", ["fr-FR", "en-AU"]), "fr-FR");
  assert.equal(i18n.dateLocale("en", ["fr-FR"]), "fr-FR");
  assert.equal(i18n.dateLocale("en", []), "en");
  assert.equal(i18n.dateLocale("fr", ["fr-CA"]), "fr-CA");
});

test("today the interface comes in English, whatever the browser asks", async () => {
  assert.deepEqual(i18n.LANGUAGES, ["en"]);
  await i18n.useLanguage("fr");
  assert.equal(i18n.currentLanguage(), "en");
});
