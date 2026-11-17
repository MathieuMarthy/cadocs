// The members of a workspace, as mentions need them: the `@` picker offers them, and a mention
// shows the name of the one it holds. Any member may list them (`GET /workspaces/{ws}/members`).

import { api, type Page } from "./api";

export interface Person {
  id: string;
  name: string;
}

/** Members are read again after this long, so a newcomer or a new name shows. */
const FRESH_MS = 10_000;
/** Members read at most, oldest accounts first. */
const READ = 1000;

let cached: { workspace: string; at: number; rows: Promise<Person[]> } | null = null;

/** The members of `workspace`, shared by every mention shown meanwhile. */
export function people(workspace: string): Promise<Person[]> {
  if (!cached || cached.workspace !== workspace || Date.now() - cached.at > FRESH_MS) {
    const entry = { workspace, at: Date.now(), rows: readPeople(workspace) };
    // A failure is not kept: the next mention asks again.
    entry.rows.catch(() => {
      if (cached === entry) cached = null;
    });
    cached = entry;
  }
  return cached.rows;
}

/** The name of a member, `null` for someone who is not one (any more). */
export async function nameOf(workspace: string, id: string): Promise<string | null> {
  const rows = await people(workspace);
  return rows.find((person) => person.id === id)?.name ?? null;
}

async function readPeople(workspace: string) {
  const rows: Person[] = [];
  let cursor: string | null = null;
  do {
    const query = new URLSearchParams({ limit: "100" });
    if (cursor) query.set("cursor", cursor);
    const page: Page<Person> = await api("GET", `/workspaces/${workspace}/members?${query}`);
    rows.push(...page.items.map(({ id, name }) => ({ id, name })));
    cursor = page.next_cursor;
  } while (cursor && rows.length < READ);
  return rows;
}
