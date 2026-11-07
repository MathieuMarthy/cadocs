/** Who is logged in, for every screen. */

import {
  api,
  ApiError,
  type Session,
  setCsrfToken,
  setUnauthenticatedHandler,
  type User,
} from "./api";
import { navigate } from "./router.svelte";

export const app = $state<{
  phase: "loading" | "failed" | "ready";
  session: Session | null;
  setupRequired: boolean;
  /** What the login screen says on arrival. */
  notice: string | null;
}>({ phase: "loading", session: null, setupRequired: false, notice: null });

export function signIn(session: Session) {
  setCsrfToken(session.csrf_token);
  app.session = session;
  app.setupRequired = false;
  app.notice = null;
}

/** The session ended on its own: the router sends the person to login, and back here after. */
export function sessionEnded() {
  if (!app.session) return;
  setCsrfToken(null);
  app.session = null;
  app.notice = "session ended, log in again";
}

setUnauthenticatedHandler(sessionEnded);

/** The person ended the session: where they were does not matter any more. */
export function signOut(notice: string | null = null, to = "/login") {
  navigate(to);
  setCsrfToken(null);
  app.session = null;
  app.notice = notice;
}

export function updateUser(user: User) {
  if (app.session) app.session.user = user;
}

/** Reads the session again, after a change that may have altered it. */
export async function refreshSession() {
  signIn(await api<Session>("GET", "/session"));
}

export async function boot() {
  app.phase = "loading";
  try {
    try {
      signIn(await api<Session>("GET", "/session"));
    } catch (error) {
      if (!(error instanceof ApiError) || error.status !== 401) throw error;
      const { required } = await api<{ required: boolean }>("GET", "/auth/setup");
      app.setupRequired = required;
    }
    app.phase = "ready";
  } catch {
    app.phase = "failed";
  }
}
