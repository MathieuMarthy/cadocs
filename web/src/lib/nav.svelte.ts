/**
 * The spaces and the page tree of one workspace, kept fresh by its events socket (ADR 0011).
 * Events are hints: each one refetches over REST what it may have changed.
 */

import { SvelteMap, SvelteSet } from "svelte/reactivity";

import { api, ApiError, type Page, type Workspace } from "./api";
import { describe } from "./form.svelte";
import { refreshSession, sessionEnded } from "./session.svelte";

export interface Space {
  id: string;
  workspace_id: string;
  slug: string;
  name: string;
  description: string | null;
  default_role: string | null;
  role: string;
  deleted_at: string | null;
  created_at: string;
  updated_at: string;
}

export interface PageRow {
  id: string;
  workspace_id: string;
  space_id: string;
  parent_id: string | null;
  position: string;
  slug: string;
  title: string;
  role: string;
  has_children: boolean;
  created_by: string;
  updated_by: string;
  deleted_at: string | null;
  deleted_by: string | null;
  created_at: string;
  updated_at: string;
}

/** Where a dragged page lands: the fields of `POST /pages/{page}/move`. */
export interface Place {
  space_id: string;
  parent_id: string | null;
  after?: string | null;
}

/** A page of a level, as the API pages them. */
const BATCH = 100;
/** At most this many spaces are listed, which no workspace this size should reach. */
const MAX_SPACES = 1000;
const RETRY_MS = [1000, 2000, 5000, 10000];

export const canEdit = (role: string) => role === "editor" || role === "admin";

/** Every item of a paginated list, up to `limit`. */
async function collect<T>(path: string, limit: number) {
  const items: T[] = [];
  let cursor: string | null = null;
  do {
    const query = new URLSearchParams({ limit: String(BATCH) });
    if (cursor) query.set("cursor", cursor);
    const join = path.includes("?") ? "&" : "?";
    const page: Page<T> = await api<Page<T>>("GET", `${path}${join}${query}`);
    items.push(...page.items);
    cursor = page.next_cursor;
  } while (cursor && items.length < limit);
  return { items, next: cursor };
}

/** One level of the tree: the children of a page, or the root of a space. */
export class Level {
  items = $state<PageRow[]>([]);
  more = $state(false);
  loaded = $state(false);
  problem = $state<string | null>(null);
  /** Only the last fetch started may write, so a slow answer cannot undo a newer one. */
  generation = 0;

  constructor(
    readonly space: string,
    readonly parent: string | null,
  ) {}

  get path() {
    const parent = this.parent ? `?parent=${this.parent}` : "";
    return `/spaces/${this.space}/tree${parent}`;
  }
}

const levelKey = (space: string, parent: string | null) => `${space}/${parent ?? ""}`;

export class Nav {
  spaces = $state<Space[]>([]);
  spacesLoaded = $state(false);
  problem = $state<string | null>(null);
  online = $state(false);
  /** Spaces and pages whose children show. */
  readonly open = new SvelteSet<string>();
  readonly levels = new SvelteMap<string, Level>();
  /** Bumped by every change to the pages of a space, for the views that list them. */
  readonly versions = new SvelteMap<string, number>();
  /** Bumped by every change to any page. */
  version = $state(0);

  private socket: WebSocket | null = null;
  private running = false;
  private failures = 0;
  private retry: ReturnType<typeof setTimeout> | null = null;
  private pending = new Map<string, ReturnType<typeof setTimeout>>();

  /** Kept current by `acquireNav`, as a rename changes the slug. */
  workspace = $state<Workspace>()!;
  private spacesAsked = 0;

  constructor(workspace: Workspace) {
    this.workspace = workspace;
  }

  start() {
    if (this.running) return;
    this.running = true;
    // REST does not wait for the socket, which a proxy may never let through.
    if (!this.spacesLoaded) void this.loadSpaces();
    this.connect();
  }

  stop() {
    this.running = false;
    this.online = false;
    if (this.retry) clearTimeout(this.retry);
    this.retry = null;
    for (const timer of this.pending.values()) clearTimeout(timer);
    this.pending.clear();
    const socket = this.socket;
    this.socket = null;
    socket?.close(1000);
  }

  private connect() {
    const scheme = location.protocol === "https:" ? "wss:" : "ws:";
    const url = `${scheme}//${location.host}/ws/events?workspace=${this.workspace.id}`;
    const socket = new WebSocket(url);
    this.socket = socket;
    socket.onopen = () => {
      this.failures = 0;
      this.online = true;
      // Whatever happened while this socket was away is lost: read everything again.
      void this.reloadAll();
    };
    socket.onmessage = (message) => {
      try {
        this.handle(JSON.parse(String(message.data)));
      } catch {
        // A frame this client does not understand changes nothing it shows.
      }
    };
    socket.onclose = (event) => {
      if (this.socket !== socket) return;
      this.socket = null;
      this.online = false;
      this.closed(event.code);
    };
  }

  private closed(code: number) {
    if (!this.running) return;
    if (code === 4401) {
      this.stop();
      sessionEnded();
      return;
    }
    if (code === 4403 || code === 4404) {
      // The workspace is gone for this user: the session says so, and the router follows.
      this.stop();
      void refreshSession().catch(() => {});
      return;
    }
    this.failures += 1;
    // A socket cannot read why an upgrade failed, so the session tells every few tries.
    if (this.failures % 3 === 0) void refreshSession().catch(() => {});
    const delay = RETRY_MS[Math.min(this.failures - 1, RETRY_MS.length - 1)];
    this.retry = setTimeout(() => {
      this.retry = null;
      if (this.running) this.connect();
    }, delay);
  }

  private handle(event: { type?: string; space?: string }) {
    switch (event.type) {
      case "page.created":
      case "page.moved":
      case "page.renamed":
      case "page.trashed":
      case "page.restored":
      case "page.purged":
        if (event.space) this.spaceChanged(event.space);
        break;
      case "space.created":
      case "space.updated":
      case "space.trashed":
      case "space.restored":
      case "space.purged":
      case "access.changed":
        void this.loadSpaces();
        break;
      case "resync":
        void this.reloadAll();
        break;
    }
  }

  /** Events come in bursts, as a move sends one per page of the subtree: one refetch covers them. */
  spaceChanged(space: string) {
    if (this.pending.has(space)) return;
    this.pending.set(
      space,
      setTimeout(() => {
        this.pending.delete(space);
        this.bump(space);
        void this.refreshSpace(space);
      }, 50),
    );
  }

  private bump(space: string) {
    this.versions.set(space, (this.versions.get(space) ?? 0) + 1);
    this.version += 1;
  }

  async reloadAll() {
    await this.loadSpaces();
    for (const space of this.spaces) this.bump(space.id);
    await Promise.all([...this.levels.values()].map((level) => this.loadLevel(level)));
  }

  async loadSpaces() {
    const asked = ++this.spacesAsked;
    try {
      const { items } = await collect<Space>(`/workspaces/${this.workspace.id}/spaces`, MAX_SPACES);
      // An older answer arriving last would bring back a stale list.
      if (asked !== this.spacesAsked) return;
      const before = new Map(this.spaces.map((space) => [space.id, space.role]));
      this.spaces = items;
      // Each row of the tree carries the role, so a new role reads its levels again.
      for (const space of items) {
        const role = before.get(space.id);
        if (role !== undefined && role !== space.role) void this.refreshSpace(space.id);
      }
      this.problem = null;
      const ids = new Set(items.map((space) => space.id));
      for (const [key, level] of this.levels) {
        if (ids.has(level.space)) continue;
        this.levels.delete(key);
        this.open.delete(level.parent ?? level.space);
      }
    } catch (error) {
      if (asked === this.spacesAsked) this.problem = describe(error);
    } finally {
      if (asked === this.spacesAsked) this.spacesLoaded = true;
    }
  }

  space(id: string) {
    return this.spaces.find((space) => space.id === id) ?? null;
  }

  /** The level under `parent` if it was asked for, without fetching it: templates read this. */
  loaded(space: string, parent: string | null) {
    return this.levels.get(levelKey(space, parent)) ?? null;
  }

  /** The level under `parent`, fetched the first time it is asked for. */
  level(space: string, parent: string | null) {
    const key = levelKey(space, parent);
    let level = this.levels.get(key);
    if (!level) {
      level = new Level(space, parent);
      this.levels.set(key, level);
      void this.loadLevel(level);
    }
    return level;
  }

  /** Fetches a level again, as far as it was read, or one batch further with `more`. */
  async loadLevel(level: Level, more = false) {
    const generation = ++level.generation;
    const want = more ? level.items.length + BATCH : Math.max(level.items.length, BATCH);
    try {
      const { items, next } = await collect<PageRow>(level.path, want);
      if (generation !== level.generation) return;
      level.items = items;
      level.more = next !== null;
      level.problem = null;
    } catch (error) {
      if (generation !== level.generation) return;
      if (error instanceof ApiError && error.status === 404) {
        // The parent left the space or the trash took it: its level goes with it.
        this.close(level.parent ?? level.space, level.space);
        return;
      }
      level.problem = describe(error);
    } finally {
      if (generation === level.generation) level.loaded = true;
    }
  }

  private refreshSpace(space: string) {
    const levels = [...this.levels.values()].filter((level) => level.space === space);
    return Promise.all(levels.map((level) => this.loadLevel(level)));
  }

  toggle(id: string, space: string) {
    if (this.open.has(id)) this.close(id, space);
    else {
      this.open.add(id);
      this.level(space, id === space ? null : id);
    }
  }

  expand(id: string, space: string) {
    if (!this.open.has(id)) this.toggle(id, space);
  }

  /**
   * Hides a node's children, and stops fetching them and what lies under them, which closes too:
   * a node is open exactly when its level exists.
   */
  private close(id: string, space: string) {
    const drop = (node: string) => {
      this.open.delete(node);
      const key = levelKey(space, node === space ? null : node);
      const level = this.levels.get(key);
      if (!level) return;
      this.levels.delete(key);
      level.generation += 1;
      for (const item of level.items) drop(item.id);
    };
    drop(id);
  }

  async create(space: string, parent: string | null) {
    const body = parent ? { parent_id: parent } : {};
    const page = await api<PageRow>("POST", `/spaces/${space}/pages`, body);
    this.expand(parent ?? space, space);
    this.spaceChanged(space);
    return page;
  }

  async move(page: PageRow, place: Place) {
    await api("POST", `/pages/${page.id}/move`, place);
    if (place.parent_id) this.expand(place.parent_id, place.space_id);
    this.spaceChanged(page.space_id);
    if (place.space_id !== page.space_id) this.spaceChanged(place.space_id);
  }

  async trash(page: PageRow) {
    await api("DELETE", `/pages/${page.id}`);
    this.spaceChanged(page.space_id);
  }

  async restore(page: PageRow) {
    await api("POST", `/pages/${page.id}/restore`);
    this.spaceChanged(page.space_id);
  }
}

/** The one workspace whose tree shows, kept while screens come and go. */
let current: { key: string; nav: Nav; users: number; idle: ReturnType<typeof setTimeout> | null } | null =
  null;

/**
 * The navigation of `workspace` for `user`, with its socket open until every caller releases it.
 * Screens each mount their own shell, so a short grace period keeps the socket between them.
 */
export function acquireNav(user: string, workspace: Workspace) {
  const key = `${user}/${workspace.id}`;
  if (current?.key !== key) {
    current?.nav.stop();
    if (current?.idle) clearTimeout(current.idle);
    current = { key, nav: new Nav(workspace), users: 0, idle: null };
  }
  const held = current;
  held.nav.workspace = workspace;
  held.users += 1;
  if (held.idle) clearTimeout(held.idle);
  held.idle = null;
  held.nav.start();
  let released = false;
  return {
    nav: held.nav,
    release() {
      if (released) return;
      released = true;
      held.users -= 1;
      if (held.users > 0) return;
      held.idle = setTimeout(() => {
        held.idle = null;
        held.nav.stop();
      }, 2000);
    },
  };
}
