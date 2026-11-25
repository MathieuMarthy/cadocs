// The words of the interface (step 34). Every string a person reads comes from a catalogue, by
// key: `en.ts` is the source, and `scripts/strings.ts` fails the build on words written anywhere
// else. Plain TypeScript with no `$state`, so the editor's registry still loads under Node.

import { en } from "./en";

export type Key = keyof typeof en;
/** A string, or its forms by `Intl.PluralRules` category, picked by the `count` parameter. */
export type Entry = string | { one?: string; other: string };
export type Catalogue = Record<Key, Entry>;
export type Params = Record<string, string | number>;

/** The languages the interface comes in, and how each catalogue loads. */
const CATALOGUES: Record<string, () => Promise<Catalogue>> = {
  en: async () => en,
  fr: async () => (await import("./fr")).fr,
};

export const LANGUAGES = Object.keys(CATALOGUES);

let language = "en";
let strings: Catalogue = en;
let plurals = new Intl.PluralRules(language);

/** The language the interface shows, a BCP 47 tag that `Intl` takes too. */
export const currentLanguage = () => language;

/** The text of `key`, with each `{name}` replaced by `params.name`. */
export function t(key: Key, params: Params = {}): string {
  const entry: Entry | undefined = strings[key] ?? en[key];
  // A key built from a value, as a role, that the catalogue lacks shows as itself.
  if (entry === undefined) return key;
  let text: string;
  if (typeof entry === "string") text = entry;
  else {
    const category = plurals.select(Number(params.count ?? 0)) as "one" | "other";
    text = entry[category] ?? entry.other;
  }
  return text.replace(/\{(\w+)\}/g, (whole, name: string) =>
    name in params ? String(params[name]) : whole,
  );
}

/**
 * The language to show: the person's choice if the interface has it, else the first of the
 * browser's it has, matched on the whole tag then on its primary subtag, else English.
 */
export function pickLanguage(
  chosen: string | null | undefined,
  browser: readonly string[],
  available: readonly string[] = LANGUAGES,
): string {
  if (chosen && available.includes(chosen)) return chosen;
  for (const wanted of browser) {
    const tag = wanted.toLowerCase();
    if (available.includes(tag)) return tag;
    const primary = tag.split("-")[0]!;
    if (available.includes(primary)) return primary;
  }
  return "en";
}

/**
 * The locale of numeric dates: the browser's first, else the interface's. A chosen language does
 * not move them: day and month order is the region's habit. Dates in words (`5 minutes ago`)
 * take the interface's language.
 */
export function dateLocale(interfaceLanguage: string, browser: readonly string[]) {
  return browser[0] ?? interfaceLanguage;
}

/** What `Intl` formats dates with, from `dateLocale`. */
export const formatLocale = () =>
  dateLocale(language, typeof navigator === "undefined" ? [] : navigator.languages);

/** The last language asked for: a catalogue that loads late must not replace a newer choice. */
let wanted = language;

/** Shows the interface in the language picked for `chosen`, `null` following the browser's. */
export async function useLanguage(chosen: string | null | undefined) {
  const browser = typeof navigator === "undefined" ? [] : navigator.languages;
  const picked = pickLanguage(chosen, browser);
  wanted = picked;
  if (picked === language) return;
  const loaded = await CATALOGUES[picked]!();
  if (wanted !== picked) return;
  strings = loaded;
  language = picked;
  plurals = new Intl.PluralRules(language);
  if (typeof document !== "undefined") document.documentElement.lang = language;
}
