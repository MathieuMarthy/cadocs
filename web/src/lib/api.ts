/** REST calls under `/api/v1`, and what `docs/API.md` says about their errors. */

export interface User {
  id: string;
  email: string;
  name: string;
  is_instance_admin: boolean;
}

export interface Workspace {
  id: string;
  slug: string;
  name: string;
  role: string;
}

export interface Session {
  user: User;
  workspaces: Workspace[];
  csrf_token: string;
}

export interface Page<T> {
  items: T[];
  next_cursor: string | null;
}

export class ApiError extends Error {
  constructor(
    /** The HTTP status, or 0 when no answer came. */
    readonly status: number,
    readonly code: string,
    message: string,
    readonly fields: Record<string, string> = {},
    readonly retryAfter: number | null = null,
  ) {
    super(message);
  }
}

let csrfToken: string | null = null;
let onUnauthenticated: () => void = () => {};

export function setCsrfToken(token: string | null) {
  csrfToken = token;
}

/** Called when any answer says the session is over, whichever screen asked. */
export function setUnauthenticatedHandler(handler: () => void) {
  onUnauthenticated = handler;
}

async function refusal(response: Response) {
  const body = await response.json().catch(() => null);
  const error = body?.error ?? {};
  const retryAfter = Number(response.headers.get("retry-after")) || null;
  return new ApiError(
    response.status,
    error.code ?? "internal",
    error.message ?? `The server answered ${response.status}`,
    error.fields ?? {},
    retryAfter,
  );
}

export async function api<T>(method: string, path: string, body?: unknown): Promise<T> {
  const headers: Record<string, string> = {};
  if (body !== undefined) headers["content-type"] = "application/json";
  if (csrfToken && method !== "GET") headers["x-csrf-token"] = csrfToken;

  let response: Response;
  try {
    response = await fetch(`/api/v1${path}`, {
      method,
      headers,
      body: body === undefined ? undefined : JSON.stringify(body),
    });
  } catch {
    throw new ApiError(0, "unreachable", "Server unreachable");
  }
  if (!response.ok) {
    const error = await refusal(response);
    if (error.code === "unauthenticated") onUnauthenticated();
    throw error;
  }
  return (response.status === 204 ? undefined : await response.json()) as T;
}
