import { createServer, type Server, type Socket } from "node:net";

const WAIT_MS = 10_000;

export interface Message {
  /** The argument of `RCPT TO`, without its brackets. */
  to: string;
  /** Headers and body, with the soft line breaks of quoted-printable taken out. */
  text: string;
}

/** A mail relay that keeps what it is sent: enough SMTP for cadocs, in plain text. */
export class FakeRelay {
  readonly messages: Message[] = [];
  private readonly server: Server;

  private constructor(server: Server) {
    this.server = server;
    server.on("connection", (socket) => this.talk(socket));
  }

  static start() {
    return new Promise<FakeRelay>((done, fail) => {
      const server = createServer();
      server.once("error", fail);
      server.listen(0, "127.0.0.1", () => done(new FakeRelay(server)));
    });
  }

  /** The settings that make a server send its mail here. */
  get env(): Record<string, string> {
    const address = this.server.address();
    if (!address || typeof address !== "object") throw new Error("the relay has no port");
    return {
      CADOCS_SMTP_HOST: "127.0.0.1",
      CADOCS_SMTP_PORT: String(address.port),
      CADOCS_SMTP_TLS: "none",
      CADOCS_SMTP_FROM: "wiki@example.org",
    };
  }

  /** The message number `count`, once it arrived. */
  async message(count = 1) {
    const until = Date.now() + WAIT_MS;
    while (Date.now() < until) {
      const message = this.messages[count - 1];
      if (message) return message;
      await new Promise((done) => setTimeout(done, 20));
    }
    throw new Error(`message ${count} never arrived`);
  }

  dispose() {
    return new Promise<void>((done) => this.server.close(() => done()));
  }

  private talk(socket: Socket) {
    let pending = "";
    let to = "";
    let data: string[] | null = null;
    socket.on("error", () => {});
    socket.write("220 fake ESMTP\r\n");
    socket.on("data", (chunk) => {
      pending += chunk.toString("latin1");
      for (;;) {
        const end = pending.indexOf("\r\n");
        if (end < 0) return;
        const line = pending.slice(0, end);
        pending = pending.slice(end + 2);
        if (data) {
          if (line === ".") {
            const text = data.join("\r\n").replaceAll("=\r\n", "");
            this.messages.push({ to, text });
            data = null;
            socket.write("250 queued\r\n");
          } else {
            // Dot stuffing: a line that starts with a dot arrives with one more.
            data.push(line.startsWith(".") ? line.slice(1) : line);
          }
          continue;
        }
        const command = line.toUpperCase();
        if (command.startsWith("RCPT TO:")) {
          to = line.slice("RCPT TO:".length).trim().replace(/^<|>$/g, "");
          socket.write("250 ok\r\n");
        } else if (command === "DATA") {
          data = [];
          socket.write("354 go on\r\n");
        } else if (command === "QUIT") {
          socket.end("221 bye\r\n");
        } else {
          socket.write("250 ok\r\n");
        }
      }
    });
  }
}
