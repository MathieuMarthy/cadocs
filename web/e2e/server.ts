import { type ChildProcess, spawn } from "node:child_process";
import { existsSync, mkdtempSync, rmSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const STARTUP_MS = 10_000;
const STOP_MS = 15_000;

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

/** The real binary on its own data folder and port. */
export class TestServer {
  readonly dataDir: string;
  readonly port: number;
  private readonly env: Record<string, string>;
  private child: ChildProcess | null = null;
  private log = "";

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
    return `ws://127.0.0.1:${this.port}/ws`;
  }

  get database() {
    return join(this.dataDir, "cadocs.db");
  }

  async run() {
    const child = spawn(binary(), {
      env: {
        ...process.env,
        CADOCS_BIND: `127.0.0.1:${this.port}`,
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
