// The catalogue's functions (`src/lib/i18n`), loaded through Vite as `schema.ts` loads the editor.

import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import { after, test } from "node:test";

import { createServer } from "vite";

// A server, not `runnerImport`: that one closes before `useLanguage` can import a catalogue.
const vite = await createServer({
  configFile: false,
  logLevel: "silent",
  server: { middlewareMode: true, hmr: false, ws: false, watch: null },
});
after(() => vite.close());
const load = <T>(file: string) =>
  vite.ssrLoadModule(fileURLToPath(new URL(`../src/lib/i18n/${file}`, import.meta.url))) as Promise<T>;
const i18n = await load<typeof import("../src/lib/i18n/index.ts")>("index.ts");

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

test("the interface comes in English and French, and the last choice wins", async () => {
  assert.deepEqual(i18n.LANGUAGES, ["en", "fr"]);
  await i18n.useLanguage("fr");
  assert.equal(i18n.currentLanguage(), "fr");
  assert.equal(i18n.t("common.cancel"), "annuler");
  assert.equal(i18n.t("groups.count", { count: 1 }), "1 membre");
  assert.equal(i18n.t("groups.count", { count: 2 }), "2 membres");
  assert.equal(i18n.t("comments.toggleOpen", { count: 1 }), "commentaires · 1 ouvert");
  assert.equal(i18n.t("comments.toggleOpen", { count: 2 }), "commentaires · 2 ouverts");
  assert.equal(i18n.t("space.trashed"), "mis à la corbeille");
  // The second call is made before the first has its catalogue.
  await Promise.all([i18n.useLanguage("en"), i18n.useLanguage("fr")]);
  assert.equal(i18n.currentLanguage(), "fr");
  await Promise.all([i18n.useLanguage("en"), i18n.useLanguage("fr"), i18n.useLanguage("en")]);
  assert.equal(i18n.currentLanguage(), "en");
});

test("French has every entry, with the parameters and forms of English", async () => {
  const module = await load<typeof import("../src/lib/i18n/fr.ts")>("fr.ts");
  const source = await load<typeof import("../src/lib/i18n/en.ts")>("en.ts");
  type Entry = string | { one?: string; other: string };
  const shape = (entry: Entry) =>
    (typeof entry === "string" ? [entry] : Object.values(entry)).map((text) =>
      (text.match(/\{\w+\}/g) ?? []).sort().join(),
    );
  assert.deepEqual(Object.keys(module.fr), Object.keys(source.en));
  for (const [key, entry] of Object.entries(source.en as Record<string, Entry>)) {
    assert.deepEqual(shape(module.fr[key as keyof typeof module.fr]), shape(entry), key);
  }
});
