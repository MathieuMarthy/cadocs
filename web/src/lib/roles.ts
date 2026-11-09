// The roles and languages the settings screens offer, worded for people.

/** A choice of a `select`. */
export interface Option {
  value: string;
  label: string;
}

/** A workspace owner or admin manages it, and is admin of every space (PERMISSIONS.md). */
export const managesWorkspace = (role: string) => role === "owner" || role === "admin";

export const SPACE_ROLES = ["viewer", "commenter", "editor", "admin"];

/** What `default_role` may be, `""` standing for `null`. */
export const DEFAULT_ROLES: Option[] = [
  { value: "", label: "none: grants only" },
  { value: "viewer", label: "viewer" },
  { value: "commenter", label: "commenter" },
  { value: "editor", label: "editor" },
];

/** The languages of `PATCH /workspaces/{ws}` (ADR 0007), which stemming uses. */
export const LANGUAGES: Option[] = [
  ["ar", "arabic"],
  ["da", "danish"],
  ["de", "german"],
  ["el", "greek"],
  ["en", "english"],
  ["es", "spanish"],
  ["fi", "finnish"],
  ["fr", "french"],
  ["hu", "hungarian"],
  ["it", "italian"],
  ["nl", "dutch"],
  ["no", "norwegian"],
  ["pt", "portuguese"],
  ["ro", "romanian"],
  ["ru", "russian"],
  ["sv", "swedish"],
  ["ta", "tamil"],
  ["tr", "turkish"],
].map(([value, label]) => ({ value: value!, label: label! }));

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
