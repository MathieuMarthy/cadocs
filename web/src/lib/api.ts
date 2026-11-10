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

/** A call under `/api/v1` that answered 2xx, or the refusal as an `ApiError`. */
async function call(method: string, path: string, headers: Record<string, string>, body?: BodyInit) {
  if (csrfToken && method !== "GET") headers["x-csrf-token"] = csrfToken;
  let response: Response;
  try {
    response = await fetch(`/api/v1${path}`, { method, headers, body });
  } catch {
    throw new ApiError(0, "unreachable", "Server unreachable");
  }
  if (!response.ok) {
    const error = await refusal(response);
    if (error.code === "unauthenticated") onUnauthenticated();
    throw error;
  }
  return response;
}

export async function api<T>(method: string, path: string, body?: unknown): Promise<T> {
  const headers: Record<string, string> = {};
  if (body !== undefined) headers["content-type"] = "application/json";
  const response = await call(method, path, headers, body === undefined ? undefined : JSON.stringify(body));
  return (response.status === 204 ? undefined : await response.json()) as T;
}

export interface Attachment {
  id: string;
  file_name: string;
  content_type: string;
  size: number;
}

/** Where an attachment is served, as the server's renderers link to it. */
export const attachmentUrl = (id: string) => `/api/v1/attachments/${encodeURIComponent(id)}`;

/** Sends `file` as the raw body, so the server streams it to disk with no parser in between. */
export async function uploadAttachment(page: string, file: File): Promise<Attachment> {
  const path = `/pages/${encodeURIComponent(page)}/attachments?name=${encodeURIComponent(file.name || "file")}`;
  const headers = { "content-type": file.type || "application/octet-stream" };
  const response = await call("POST", path, headers, file);
  return (await response.json()) as Attachment;
}
