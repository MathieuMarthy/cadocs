// Dates as people read them: relative on screen, exact on hover.

import { currentLanguage, formatLocale, t } from "./i18n";

const STEPS: [Intl.RelativeTimeFormatUnit, number][] = [
  ["second", 60],
  ["minute", 60],
  ["hour", 24],
  ["day", 7],
  ["week", 4.35],
  ["month", 12],
  ["year", Infinity],
];

/** `5 minutes ago`, `yesterday`, or `just now` under ten seconds. */
export function relative(date: Date, now = Date.now()) {
  let amount = (date.getTime() - now) / 1000;
  if (Math.abs(amount) < 10) return t("time.now");
  const words = new Intl.RelativeTimeFormat(currentLanguage(), { numeric: "auto" });
  for (const [unit, size] of STEPS) {
    if (Math.abs(amount) < size) return words.format(Math.round(amount), unit);
    amount /= size;
  }
  return date.toLocaleDateString(formatLocale());
}

export const exact = (date: Date) => date.toLocaleString(formatLocale());
