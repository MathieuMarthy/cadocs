// The roles and languages the settings screens offer, worded for people.

import { currentLanguage, type Key, t } from "./i18n";

/** A choice of a `select`. */
export interface Option {
  value: string;
  label: string;
}

/** A workspace owner or admin manages it, and is admin of every space (PERMISSIONS.md). */
export const managesWorkspace = (role: string) => role === "owner" || role === "admin";

export const SPACE_ROLES = ["viewer", "commenter", "editor", "admin"];

/** A role as people read it. */
export const roleName = (role: string) => t(`role.${role}` as Key);

/** What `default_role` may be, `""` standing for `null`. A function, so it reads the language. */
export const defaultRoles = (): Option[] => [
  { value: "", label: t("role.none") },
  ...["viewer", "commenter", "editor"].map((value) => ({ value, label: roleName(value) })),
];

/** The languages of `PATCH /workspaces/{ws}` (ADR 0007), which stemming uses. */
const STEMMING = [
  ...["ar", "da", "de", "el", "en", "es", "fi", "fr", "hu"],
  ...["it", "nl", "no", "pt", "ro", "ru", "sv", "ta", "tr"],
];

/** A language's name in the interface's language, lowercase as the interface writes words. */
export function languageName(code: string) {
  const names = new Intl.DisplayNames(currentLanguage(), { type: "language" });
  return (names.of(code) ?? code).toLocaleLowerCase(currentLanguage());
}

/** The choices of the workspace's language. */
export const stemmingLanguages = (): Option[] =>
  STEMMING.map((value) => ({ value, label: languageName(value) }));

/** A user of the workspace, as `GET /workspaces/{ws}/members` answers. */
export interface Member {
  id: string;
  email: string;
  name: string;
  role: string;
  disabled: boolean;
  created_at: string;
}

/** At most this many members are listed, which no workspace this size should reach. */
export const MAX_MEMBERS = 1000;

/** A group of the workspace, as `GET /workspaces/{ws}/groups` answers. */
export interface Group {
  id: string;
  name: string;
  member_count: number;
  created_at: string;
}

/** The higher of two space roles, `""` standing for none. */
export const higherRole = (a: string, b: string) =>
  SPACE_ROLES.indexOf(a) >= SPACE_ROLES.indexOf(b) ? a : b;
