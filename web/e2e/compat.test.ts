// The real `yjs` and `y-websocket` clients against the real binary (docs/API.md).

import assert from "node:assert/strict";
import { DatabaseSync } from "node:sqlite";
import { describe, test, type TestContext } from "node:test";

import { WebsocketProvider } from "y-websocket";
import * as Y from "yjs";

import { TestServer } from "./server.ts";

/** Its reconnect timer, which is not exported: `messageReconnectTimeout` in y-websocket. */
const Y_WEBSOCKET_SILENCE_MS = 30_000;

const sleep = (ms: number) => new Promise((done) => setTimeout(done, ms));

async function until(check: () => boolean, what: string, ms = 8_000) {
  const deadline = Date.now() + ms;
  while (!check()) {
    if (Date.now() > deadline) assert.fail(`timed out waiting for ${what}`);
    await sleep(10);
  }
}

async function startServer(t: TestContext, env: Record<string, string> = {}) {
  const server = await TestServer.start(env);
  t.after(() => server.dispose());
  return server;
}

function connect(t: TestContext, server: TestServer, room: string) {
  const doc = new Y.Doc();
  const provider = new WebsocketProvider(server.wsUrl, room, doc, { disableBc: true });
  t.after(() => {
    provider.destroy();
    doc.destroy();
  });
  return { doc, provider, awareness: provider.awareness };
}

async function connected(t: TestContext, server: TestServer, room: string) {
  const client = connect(t, server, room);
  await until(() => client.provider.synced, "the first sync");
  return client;
}

function paragraph(text: string, attributes: Record<string, string> = {}) {
  const element = new Y.XmlElement("paragraph");
  for (const [key, value] of Object.entries(attributes)) element.setAttribute(key, value);
  element.insert(0, [new Y.XmlText(text)]);
  return element;
}

/** Everything a client can read, in the layout of docs/DATA_MODEL.md. */
function content(doc: Y.Doc) {
  return {
    title: doc.getText("title").toString(),
    body: doc.getXmlFragment("default").toString(),
  };
}

function sameContent(left: Y.Doc, right: Y.Doc) {
  return JSON.stringify(content(left)) === JSON.stringify(content(right));
}

describe("yjs and y-websocket compatibility", { concurrency: true }, () => {
  test("edits reach the other client, in both directions", async (t) => {
    const server = await startServer(t);
    const left = await connected(t, server, "both-ways");
    const right = await connected(t, server, "both-ways");

    left.doc.getText("title").insert(0, "from left");
    right.doc.getXmlFragment("default").insert(0, [paragraph("from right", { align: "left" })]);

    await until(() => right.doc.getText("title").toString() === "from left", "the title");
    await until(() => sameContent(left.doc, right.doc), "the body");
    assert.equal(
      content(left.doc).body,
      '<paragraph align="left">from right</paragraph>',
    );
  });

  test("rooms do not leak into each other", async (t) => {
    const server = await startServer(t);
    const first = await connected(t, server, "first");
    const second = await connected(t, server, "second");
    const witness = await connected(t, server, "first");

    first.doc.getText("title").insert(0, "private");
    await until(() => witness.doc.getText("title").length > 0, "the same room");

    assert.equal(second.doc.getText("title").toString(), "");
    // A new client reads what the server holds, with no frame still on its way.
    const late = await connected(t, server, "second");
    assert.equal(late.doc.getText("title").toString(), "");
  });

  test("a late client gets the whole document", async (t) => {
    const server = await startServer(t);
    const writer = await connected(t, server, "late");
    const title = writer.doc.getText("title");
    title.insert(0, "a title with a typo");
    title.delete(13, 6);
    title.insert(13, "fix");
    writer.doc.getXmlFragment("default").insert(0, [paragraph("one"), paragraph("two")]);
    const witness = await connected(t, server, "late");
    await until(() => sameContent(writer.doc, witness.doc), "the first late client");

    const late = await connected(t, server, "late");

    assert.deepEqual(content(late.doc), {
      title: "a title with fix",
      body: "<paragraph>one</paragraph><paragraph>two</paragraph>",
    });
  });

  test("offline edits on both sides converge without loss", async (t) => {
    const server = await startServer(t);
    const left = await connected(t, server, "offline");
    const right = await connected(t, server, "offline");
    left.doc.getText("title").insert(0, "shared");
    await until(() => sameContent(left.doc, right.doc), "the shared start");

    left.provider.disconnect();
    right.provider.disconnect();
    left.doc.getText("title").insert(0, "left ");
    right.doc.getText("title").insert(6, " right");
    left.doc.getXmlFragment("default").insert(0, [paragraph("left")]);
    right.doc.getXmlFragment("default").insert(0, [paragraph("right")]);
    left.provider.connect();
    right.provider.connect();

    await until(() => sameContent(left.doc, right.doc), "convergence");
    assert.equal(left.doc.getText("title").toString(), "left shared right");
    const body = content(left.doc).body;
    assert.match(body, /<paragraph>left<\/paragraph>/);
    assert.match(body, /<paragraph>right<\/paragraph>/);
  });

  test("presence shows, changes and goes away when a client leaves", async (t) => {
    const server = await startServer(t);
    const left = await connected(t, server, "presence");
    const right = await connected(t, server, "presence");
    const id = left.doc.clientID;

    left.awareness.setLocalStateField("user", { name: "ada" });
    await until(
      () => (right.awareness.getStates().get(id)?.user as { name?: string })?.name === "ada",
      "the state",
    );
    left.awareness.setLocalStateField("user", { name: "grace" });
    await until(
      () => (right.awareness.getStates().get(id)?.user as { name?: string })?.name === "grace",
      "the change",
    );

    // A closed socket, with no goodbye from the client: only the server can announce this.
    left.provider.ws?.close();
    left.provider.shouldConnect = false;

    await until(() => !right.awareness.getStates().has(id), "the removal", 3_000);
  });

  test("a client alone in a room is not silent long enough to reconnect", async (t) => {
    const server = await startServer(t);
    const alone = await connected(t, server, "alone");
    let closes = 0;
    alone.provider.on("connection-close", () => closes++);

    await sleep(Y_WEBSOCKET_SILENCE_MS + 5_000);

    assert.equal(closes, 0);
    assert.ok(alone.provider.wsconnected);
  });

  test("a clean stop flushes what the room holds", async (t) => {
    const server = await startServer(t, { CADOCS_FLUSH_INTERVAL_MS: "60000" });
    const writer = await connected(t, server, "clean-stop");
    const witness = await connected(t, server, "clean-stop");
    writer.doc.getText("title").insert(0, "before");
    writer.doc.getXmlFragment("default").insert(0, [paragraph("kept")]);
    // Seen by another client means it is in the room, and the long interval means it is not flushed.
    await until(() => sameContent(writer.doc, witness.doc), "the relay");
    assert.equal(storedRows(server, "clean-stop").updates, 0);

    await server.stop("SIGTERM");
    // Otherwise they would hand the document back, and the disk would prove nothing.
    writer.provider.disconnect();
    witness.provider.disconnect();
    await server.run();

    const fresh = await connected(t, server, "clean-stop");
    assert.deepEqual(content(fresh.doc), { title: "before", body: "<paragraph>kept</paragraph>" });
  });

  test("open clients reconnect after a restart and carry on", async (t) => {
    const server = await startServer(t);
    const writer = await connected(t, server, "restart");
    const witness = await connected(t, server, "restart");
    writer.doc.getText("title").insert(0, "before");
    await until(() => sameContent(writer.doc, witness.doc), "the relay");

    await server.restart("SIGTERM");

    await until(() => writer.provider.wsconnected && writer.provider.synced, "the reconnection");
    await until(() => witness.provider.wsconnected && witness.provider.synced, "the other one");
    writer.doc.getText("title").insert(6, " and after");
    await until(() => witness.doc.getText("title").toString() === "before and after", "the edit");
  });

  test("after a crash, open clients bring back what was not flushed", async (t) => {
    const server = await startServer(t, { CADOCS_FLUSH_INTERVAL_MS: "60000" });
    const writer = await connected(t, server, "crash");
    const witness = await connected(t, server, "crash");
    writer.doc.getText("title").insert(0, "never flushed");
    await until(() => sameContent(writer.doc, witness.doc), "the relay");
    assert.equal(storedRows(server, "crash").updates, 0);

    await server.restart("SIGKILL");

    await until(() => writer.provider.wsconnected && writer.provider.synced, "the reconnection");
    const fresh = await connected(t, server, "crash");
    await until(() => sameContent(writer.doc, fresh.doc), "the recovered edit");
    assert.equal(fresh.doc.getText("title").toString(), "never flushed");
  });

  test("a flushed edit survives a crash with no client left", async (t) => {
    const server = await startServer(t, { CADOCS_FLUSH_INTERVAL_MS: "20" });
    const writer = await connected(t, server, "flushed");
    writer.doc.getText("title").insert(0, "on disk");
    await until(() => storedRows(server, "flushed").updates > 0, "the flush");
    writer.provider.destroy();

    await server.restart("SIGKILL");

    const fresh = await connected(t, server, "flushed");
    assert.equal(fresh.doc.getText("title").toString(), "on disk");
  });

  test("yjs reads back a document compacted and unloaded by the server", async (t) => {
    const server = await startServer(t, {
      CADOCS_FLUSH_INTERVAL_MS: "20",
      CADOCS_COMPACT_UPDATES: "3",
      CADOCS_ROOM_IDLE_SECS: "1",
    });
    const writer = await connected(t, server, "compacted");
    const title = writer.doc.getText("title");
    const body = writer.doc.getXmlFragment("default");
    for (let index = 0; index < 12; index++) {
      title.insert(title.length, `${index} `);
      body.insert(body.length, [paragraph(`line ${index}`, { index: String(index) })]);
      if (index % 3 === 2) body.delete(0);
      await sleep(40);
    }
    await until(() => storedRows(server, "compacted").snapshots === 1, "the compaction");
    title.insert(0, "last ");
    await until(() => storedRows(server, "compacted").updates > 0, "the last flush");
    const expected = content(writer.doc);
    writer.provider.destroy();

    // Unloading compacts, and nothing else would with no edit coming.
    await until(() => storedRows(server, "compacted").updates === 0, "the unload");
    const reloaded = await connected(t, server, "compacted");
    assert.deepEqual(content(reloaded.doc), expected);
    reloaded.provider.destroy();

    await server.restart("SIGTERM");
    const restarted = await connected(t, server, "compacted");
    assert.deepEqual(content(restarted.doc), expected);
  });
});

function storedRows(server: TestServer, doc: string) {
  const database = new DatabaseSync(server.database, { readOnly: true });
  try {
    const count = (table: string) =>
      database.prepare(`SELECT count(*) AS rows FROM ${table} WHERE doc_id = ?`).get(doc)
        ?.rows as number;
    return { updates: count("doc_updates"), snapshots: count("doc_snapshots") };
  } finally {
    database.close();
  }
}
