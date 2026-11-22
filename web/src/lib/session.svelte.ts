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
import { t, useLanguage } from "./i18n";

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
  void useLanguage(session.user.language);
}

/** The session ended on its own: the router sends the person to login, and back here after. */
export function sessionEnded() {
  if (!app.session) return;
  setCsrfToken(null);
  app.session = null;
  app.notice = t("session.ended");
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
  void useLanguage(user.language);
}

/** Reads the session again, after a change that may have altered it. */
export async function refreshSession() {
  signIn(await api<Session>("GET", "/session"));
}

/** What a mail relay turns on (ADR 0026). */
export interface MailFeatures {
  /** The instance sends mail: a forgotten password comes back by mail. */
  mail: boolean;
  /** Anyone can make an account. */
  registration: boolean;
  /** The ids of the single sign-on providers (ADR 0028). */
  oidc: string[];
}

/**
 * Read when a screen needs it: a person who logs out reaches login with no new boot. A failed
 * read counts as no mail.
 */
export async function mailFeatures(): Promise<MailFeatures> {
  try {
    const state = await api<{ mail?: boolean; registration?: boolean; oidc?: string[] }>(
      "GET",
      "/auth/setup",
    );
    return {
      mail: state.mail === true,
      registration: state.registration === true,
      oidc: state.oidc ?? [],
    };
  } catch {
    return { mail: false, registration: false, oidc: [] };
  }
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
    // The person's language, or the browser's, before anything shows.
    await useLanguage(app.session?.user.language ?? null);
    app.phase = "ready";
  } catch {
    app.phase = "failed";
  }
}
