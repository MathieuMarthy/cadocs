import { type ChildProcess, spawn } from "node:child_process";
import { existsSync, mkdtempSync, rmSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const STARTUP_MS = 10_000;
const STOP_MS = 15_000;
export const PASSWORD = "correct horse battery";

function binary() {
  const path = process.env.CADOCS_BIN
    ? resolve(process.env.CADOCS_BIN)
    : resolve(import.meta.dirname, "../../target/release/cadocs");
  if (!existsSync(path)) {
    throw new Error(`${path} is missing: run cargo build --release, or set CADOCS_BIN`);
  }
  return path;
}

/** The port stays the same across restarts, so that clients can reconnect on their own. */
function freePort() {
  return new Promise<number>((done, fail) => {
    const probe = createServer();
    probe.once("error", fail);
    probe.listen(0, "127.0.0.1", () => {
      const address = probe.address();
      probe.close(() => {
        if (address && typeof address === "object") done(address.port);
        else fail(new Error("no port"));
      });
    });
  });
}

interface Login {
  token: string;
  csrf: string;
  spaceId: string;
  workspaceId: string;
}

/** Someone other than the first account, as `addMember` makes them. */
export interface Member {
  id: string;
  email: string;
  token: string;
}

/** The real binary on its own data folder and port. */
export class TestServer {
  readonly dataDir: string;
  readonly port: number;
  private readonly env: Record<string, string>;
  private child: ChildProcess | null = null;
  private log = "";
  private login: Promise<Login> | null = null;
  /** Page ids by the names the tests use. */
  private readonly pages = new Map<string, Promise<string>>();

  private constructor(port: number, env: Record<string, string>) {
    this.port = port;
    this.env = env;
    this.dataDir = mkdtempSync(join(tmpdir(), "cadocs-e2e-"));
  }

  static async start(env: Record<string, string> = {}) {
    const server = new TestServer(await freePort(), env);
    try {
      await server.run();
    } catch (error) {
      await server.dispose();
      throw error;
    }
    return server;
  }

  get url() {
    return `http://127.0.0.1:${this.port}`;
  }

  get wsUrl() {
    return `ws://127.0.0.1:${this.port}/ws/pages`;
  }

  get database() {
    return join(this.dataDir, "cadocs.db");
  }

  private async post(path: string, body: unknown, headers: Record<string, string> = {}) {
    const response = await fetch(`${this.url}/api/v1${path}`, {
      method: "POST",
      headers: { "content-type": "application/json", ...headers },
      body: JSON.stringify(body),
    });
    if (!response.ok) throw new Error(`${path}: ${response.status} ${await response.text()}`);
    return response;
  }

  /** First run, then a space to put pages in. Every client of a test uses this one account. */
  private async setUp(): Promise<Login> {
    const response = await this.post("/auth/setup", {
      email: "ada@example.org",
      name: "Ada",
      password: PASSWORD,
      workspace_name: "Tests",
    });
    const token = /^session=([^;]*)/.exec(response.headers.getSetCookie()[0] ?? "")?.[1];
    if (!token) throw new Error("no session cookie");
    const session = await response.json();
    const headers = { cookie: `session=${token}`, "x-csrf-token": session.csrf_token };
    const path = `/workspaces/${session.workspaces[0].id}/spaces`;
    const space = await this.post(path, { name: "Pages" }, headers);
    return {
      token,
      csrf: session.csrf_token,
      spaceId: (await space.json()).id,
      workspaceId: session.workspaces[0].id,
    };
  }

  /** A REST call as the first account, whose answer must be a success. */
  async call(method: string, path: string, body?: unknown) {
    const { token, csrf } = await this.session();
    const response = await fetch(`${this.url}/api/v1${path}`, {
      method,
      headers: {
        "content-type": "application/json",
        cookie: `session=${token}`,
        "x-csrf-token": csrf,
      },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    if (!response.ok) throw new Error(`${path}: ${response.status} ${await response.text()}`);
    return response.status === 204 ? null : response.json();
  }

  /** An invitation link to the workspace of the first account. */
  async invite(email: string, role = "member") {
    const { workspaceId } = await this.session();
    const body = { email, role };
    return (await this.call("POST", `/workspaces/${workspaceId}/invitations`, body)).link as string;
  }

  /** A second account in the workspace, and an editor of the space of the first one. */
  async addMember(email: string): Promise<Member> {
    const { spaceId } = await this.session();
    const token = new URL(await this.invite(email)).pathname.split("/").pop();
    const response = await this.post(`/invitations/${token}/accept`, {
      name: "Grace",
      password: PASSWORD,
    });
    const cookie = /^session=([^;]*)/.exec(response.headers.getSetCookie()[0] ?? "")?.[1];
    if (!cookie) throw new Error("no session cookie");
    const { user } = await response.json();
    const grants = [{ user_id: user.id, role: "editor" }];
    await this.call("PUT", `/spaces/${spaceId}/grants`, { grants });
    return { id: user.id, email, token: cookie };
  }

  async session() {
    this.login ??= this.setUp();
    return this.login;
  }

  private async createPage() {
    const { token, csrf, spaceId } = await this.session();
    const headers = { cookie: `session=${token}`, "x-csrf-token": csrf };
    const response = await this.post(`/spaces/${spaceId}/pages`, {}, headers);
    return (await response.json()).id as string;
  }

  /** The id of the page a test calls `name`, created the first time. */
  async page(name: string) {
    let id = this.pages.get(name);
    if (!id) {
      id = this.createPage();
      this.pages.set(name, id);
    }
    return id;
  }

  /** The `WebSocket` of a logged-in client outside a browser: a browser sends the cookie itself. */
  async webSocketClass(): Promise<typeof WebSocket> {
    const { token } = await this.session();
    return class extends WebSocket {
      constructor(url: string | URL, protocols?: string | string[]) {
        const options = { protocols, headers: { cookie: `session=${token}` } };
        super(url, options as unknown as string[]);
      }
    };
  }

  /** Reads a page over REST. */
  async pageRow(name: string) {
    const { token } = await this.session();
    const response = await fetch(`${this.url}/api/v1/pages/${await this.page(name)}`, {
      headers: { cookie: `session=${token}` },
    });
    if (!response.ok) throw new Error(`page ${name}: ${response.status}`);
    return response.json();
  }

  /** Changes a page's title over REST. */
  async rename(name: string, title: string) {
    const { token, csrf } = await this.session();
    const response = await fetch(`${this.url}/api/v1/pages/${await this.page(name)}`, {
      method: "PATCH",
      headers: {
        "content-type": "application/json",
        cookie: `session=${token}`,
        "x-csrf-token": csrf,
      },
      body: JSON.stringify({ title }),
    });
    if (!response.ok) throw new Error(`rename ${name}: ${response.status}`);
  }

  async run() {
    const child = spawn(binary(), {
      env: {
        ...process.env,
        CADOCS_BIND: `127.0.0.1:${this.port}`,
        CADOCS_PUBLIC_URL: this.url,
        CADOCS_DATA_DIR: this.dataDir,
        CADOCS_LOG: "info",
        ...this.env,
      },
      stdio: ["ignore", "pipe", "pipe"],
    });
    this.child = child;
    const logStart = this.log.length;
    const keep = (chunk: Buffer) => (this.log += chunk.toString());
    child.stdout?.on("data", keep);
    child.stderr?.on("data", keep);

    const deadline = Date.now() + STARTUP_MS;
    while (Date.now() < deadline) {
      if (child.exitCode !== null) break;
      // Its own log line, because anything else on this port would answer `/health` too.
      if (this.log.includes("listening", logStart)) {
        const healthy = await fetch(`${this.url}/health`, {
          signal: AbortSignal.timeout(1_000),
        }).then(
          (response) => response.ok,
          () => false,
        );
        if (healthy) return;
      }
      await new Promise((done) => setTimeout(done, 20));
    }
    child.kill("SIGKILL");
    throw new Error(`the server did not start:\n${this.log}`);
  }

  /** `SIGTERM` is a clean stop and must exit with 0. `SIGKILL` is a crash. */
  async stop(signal: "SIGTERM" | "SIGKILL" = "SIGTERM") {
    const child = this.child;
    if (!child) return;
    this.child = null;
    if (child.exitCode !== null || child.signalCode !== null) {
      throw new Error(`the server had already stopped:\n${this.log}`);
    }
    const exit = new Promise<number | null>((done) => child.once("exit", done));
    child.kill(signal);
    const timer = setTimeout(() => child.kill("SIGKILL"), STOP_MS);
    const code = await exit;
    clearTimeout(timer);
    if (signal === "SIGTERM" && code !== 0) {
      throw new Error(`the server exited with ${code}:\n${this.log}`);
    }
  }

  async restart(signal: "SIGTERM" | "SIGKILL" = "SIGTERM") {
    await this.stop(signal);
    await this.run();
  }

  /** Ends the test, whatever state the server is in, and fails if it died on its own. */
  async dispose() {
    const child = this.child;
    this.child = null;
    const died = child !== null && (child.exitCode !== null || child.signalCode !== null);
    if (child && !died) {
      const exit = new Promise((done) => child.once("exit", done));
      child.kill("SIGKILL");
      await exit;
    }
    rmSync(this.dataDir, { recursive: true, force: true });
    if (died) throw new Error(`the server died during the test:\n${this.log}`);
  }
}
