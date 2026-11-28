import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import * as fs from "node:fs";
import { connect } from "node:net";
import * as path from "node:path";
import { createInterface } from "node:readline";
import type { Readable, Writable } from "node:stream";
import { test, type TestContext } from "node:test";
import { fileURLToPath } from "node:url";
import type { DialogOpenParams, DialogResultParams, InboxListResult, InboxUpdatedParams, RequestItem, ThreadReadResult } from "../src/protocol/generated/types.js";
import { decodeMessage, type Message } from "../src/protocol/rpc.js";
import { channelPathFor, connectDaemonSocket, listenChannels, socketPathFor } from "../src/session/server.js";
import { opId, readAll, RpcFailure, startDaemon, startThread, TestClient, tmpHome, until, waitTurnDone } from "./helpers.js";

class ChannelPeer {
  readonly received: Message[] = [];
  private nextId = 1;

  constructor(from: Readable, private readonly to: Writable) {
    createInterface({ input: from }).on("line", (line) => this.received.push(decodeMessage(line)));
  }

  async call<T>(method: string, params: object = {}): Promise<T> {
    const id = this.nextId++;
    this.to.write(`${JSON.stringify({ jsonrpc: "2.0", id, method, params })}\n`);
    return this.response<T>(id);
  }

  async response<T>(id: number | string): Promise<T> {
    const msg = await until(() => this.received.find((m) => (m.type === "response" || m.type === "error") && m.id === id), `channel response ${id}`);
    if (msg.type === "error") throw new RpcFailure(msg.error);
    assert.equal(msg.type, "response");
    return msg.result as T;
  }

  results(): DialogResultParams[] {
    return this.received.flatMap((m) => m.type === "notification" && m.method === "dialog/result" ? [m.params as DialogResultParams] : []);
  }
}

const layout: DialogOpenParams["layout"] = {
  type: "column",
  children: [
    { type: "text", text: "These choices are supplied at runtime." },
    { type: "select", id: "team", required: true, options: [{ value: "ops", label: "Operations" }, { value: "dev", label: "Development" }] },
    { type: "textInput", id: "token", secret: true, required: true },
    { type: "row", children: [{ type: "button", id: "save", label: "Save", kind: "primary" }] },
  ],
};
const dialog = (requestId: string, threadId?: string): DialogOpenParams => ({ requestId, threadId, title: "Integrate deployment", layout });
const respond = (action: string, values: Record<string, unknown> = {}) => ({ requestKind: "dialog", action, values });
const cancel = { requestKind: "dialog", cancelled: true };
const failedWith = (code: string) => (e: RpcFailure) => e.code === code;

async function connectHost(socketPath: string, id?: string): Promise<TestClient> {
  const socket = await connectDaemonSocket(socketPath);
  assert.ok(socket, "the daemon transport is listening");
  const host = new TestClient(socket);
  await host.initialize(id);
  return host;
}

async function setup(t: TestContext) {
  const home = tmpHome();
  const daemon = await startDaemon(home);
  t.after(async () => {
    await daemon.stop("test");
    fs.rmSync(home, { recursive: true, force: true });
  });
  const host = await connectHost(daemon.socketPath);
  return { home, daemon, host };
}

async function direct(socketPath: string) {
  const socket = connect(socketPath);
  await once(socket, "connect");
  return { socket, peer: new ChannelPeer(socket, socket) };
}

function inboxEvents(host: TestClient): RequestItem[] {
  return host.notifications.flatMap((n) => n.method === "inbox/updated" ? [(n.params as InboxUpdatedParams).request] : []);
}

function filesContain(dir: string, secret: string): boolean {
  return fs.readdirSync(dir, { withFileTypes: true }).some((entry) => {
    const file = path.join(dir, entry.name);
    return entry.isDirectory() ? filesContain(file, secret) : entry.isFile() && fs.readFileSync(file).includes(secret);
  });
}

test("the Windows pipe name is fixed for the OS account across daemon homes", () => {
  assert.equal(channelPathFor("C:/OtherHome", "win32", "Example User"), String.raw`\\.\pipe\yaac-channel-Example User`);
  assert.equal(channelPathFor("C:/FirstHome", "win32", "Example User"), channelPathFor("D:/SecondHome", "win32", "Example User"));
  assert.equal(channelPathFor("/home/account/.yaac", "linux"), path.join("/home/account/.yaac", "run", "channel.sock"));
});

test("the Unix channel socket is private, separate, and a second listener cannot take it", { skip: process.platform === "win32" }, async (t) => {
  const { home, daemon, host } = await setup(t);
  const endpoint = channelPathFor(home);
  assert.equal(endpoint, path.join(home, "run", "channel.sock"));
  assert.notEqual(endpoint, daemon.socketPath);
  const before = fs.lstatSync(endpoint);
  assert.ok(before.isSocket());
  assert.equal(before.mode & 0o777, 0o600);
  await assert.rejects(listenChannels(endpoint, (s) => s.destroy()), /already in use/);
  assert.equal(fs.lstatSync(endpoint).ino, before.ino);
  const { peer } = await direct(endpoint);
  await peer.call("channel/register", { name: "Original listener" });
  await assert.rejects(host.request("channel/register", { name: "Host connection" }), failedWith("method_not_found"));
  await daemon.stop("test endpoint cleanup");
  assert.equal(fs.existsSync(endpoint), false);
});

test("the Unix endpoint is recovered after its owner crashes", { skip: process.platform === "win32" }, async (t) => {
  const home = tmpHome();
  const endpoint = channelPathFor(home);
  fs.mkdirSync(path.dirname(endpoint), { mode: 0o700 });
  const old = spawn(process.execPath, [
    "-e", 'require("node:net").createServer().listen(process.argv[1], () => console.log("ready"))', endpoint,
  ], { stdio: ["ignore", "pipe", "inherit"] });
  let current: Awaited<ReturnType<typeof startDaemon>> | undefined;
  t.after(async () => {
    old.kill();
    await current?.stop("test");
    fs.rmSync(home, { recursive: true, force: true });
  });
  await once(old.stdout, "data");
  const stopped = once(old, "exit");
  old.kill("SIGKILL");
  await stopped;
  assert.ok(fs.lstatSync(endpoint).isSocket());
  current = await startDaemon(home);
  const { peer } = await direct(endpoint);
  await peer.call("channel/register", { name: "New listener" });
});

test("one inbox answer wins, and only the channel receives its secret value", async (t) => {
  const { home, daemon, host } = await setup(t);
  const other = await connectHost(daemon.socketPath, "c_phone");
  const { peer } = await direct(channelPathFor(daemon.config.home));
  await assert.rejects(peer.call("dialog/open", dialog("choice")), failedWith("not_registered"));
  const registration = await peer.call<{ channelId: string }>("channel/register", { name: "Deployment", description: "Test integrator" });
  await assert.rejects(peer.call("channel/register", { name: "Other name" }), failedWith("already_registered"));
  await assert.rejects(peer.call("thread/sync"), failedWith("method_not_found"));
  await until(() => host.notifications.find((n) => n.method === "channel/registered" && n.params.channelId === registration.channelId));
  assert.deepEqual(await peer.call("dialog/open", { ...dialog("choice", "unknown-thread"), source: { name: "Forged" } }), { accepted: true });
  const inbox = await host.request<InboxListResult>("inbox/list");
  assert.deepEqual(inbox.channels, [{ ...registration, name: "Deployment", description: "Test integrator" }]);
  const request = inbox.requests[0]!;
  assert.equal(request.requestKind, "dialog");
  assert.notEqual(request.requestId, "choice");
  assert.deepEqual(request.source, { ...registration, name: "Deployment" });
  const file = path.join(home, "dialogs", `${request.requestId}.json`);
  assert.equal(JSON.parse(fs.readFileSync(file, "utf8")).item.state, "pending", "already durable when open acknowledges");
  await until(() => inboxEvents(host).find((r) => r.requestId === request.requestId));
  await assert.rejects(peer.call("dialog/open", dialog("choice")), failedWith("duplicate_request"));

  const SECRET = "temporary-dialog-token-never-on-disk-555";
  const requestId = request.requestId;
  await assert.rejects(host.request("inbox/respond", { opId: opId(), requestId, response: respond("save", { team: "unknown", token: SECRET }) }), failedWith("invalid_params"));
  await assert.rejects(host.request("inbox/respond", { opId: opId(), requestId, response: respond("save", { team: "ops" }) }), /token.*required/);
  const response = respond("save", { team: "ops", token: SECRET });
  const actions = await Promise.allSettled([host, other].map((client) => client.request("inbox/respond", { opId: opId(), requestId, response })));
  assert.equal(actions.filter((r) => r.status === "fulfilled").length, 1);
  assert.equal(actions.find((r) => r.status === "rejected")?.reason.code, "request_not_pending");
  assert.deepEqual(await until(() => peer.results()[0]), { requestId: "choice", outcome: "answered", action: "save", values: { team: "ops", token: SECRET } });
  await assert.rejects(peer.call("dialog/close", { requestId: "choice" }), failedWith("request_not_pending"));
  assert.equal(peer.results().length, 1);
  assert.deepEqual((await host.request<InboxListResult>("inbox/list")).requests, []);
  const resolved = await until(() => inboxEvents(host).find((r) => r.requestId === requestId && r.state === "resolved"));
  assert.deepEqual(resolved.resolution?.answer, { requestKind: "dialog", action: "save", cancelled: false, values: { team: "ops" } });
  assert.equal(filesContain(home, SECRET), false);
  assert.equal(JSON.stringify(host.messages).includes(SECRET), false);
  assert.equal(JSON.stringify(other.messages).includes(SECRET), false);
});

test("thread dialogs use the journal and survive a daemon restart with the original channel closed", async (t) => {
  const { home, daemon, host } = await setup(t);
  const { threadId, turnId } = await startThread(host, "/fake say Ready");
  await host.request("thread/subscribe", { threadId, epoch: 1, afterSeq: 0 });
  await waitTurnDone(host, threadId, turnId);
  const { peer } = await direct(channelPathFor(daemon.config.home));
  await peer.call("channel/register", { name: "Deployment" });
  await peer.call("dialog/open", dialog("withdrawn", threadId));
  const started = await host.waitEvent((msg) => msg.event.type === "itemStarted" && msg.event.item.kind === "request" && msg.event.item.requestKind === "dialog");
  assert.equal(started.event.type, "itemStarted");
  assert.equal(started.event.item.kind, "request");
  const withdrawn = started.event.item.requestId;
  await peer.call("dialog/close", { requestId: "withdrawn" });
  assert.deepEqual(await until(() => peer.results()[0]), { requestId: "withdrawn", outcome: "cancelled" });
  await host.waitEvent((msg) => msg.event.type === "itemUpdated" && msg.event.item.kind === "request" && msg.event.item.requestId === withdrawn && msg.event.item.state === "expired");

  await peer.call("dialog/open", dialog("thread-answer", threadId));
  const page = await host.request<ThreadReadResult>("thread/read", { threadId, maxItems: 1 });
  const answered = page.items[0];
  assert.ok(answered?.kind === "request" && answered.requestKind === "dialog");
  const threadSecret = "thread-only-dialog-secret-421";
  await host.request("request/respond", {
    opId: opId(), threadId, requestId: answered.requestId, response: respond("save", { team: "dev", token: threadSecret }),
  });
  assert.deepEqual(await until(() => peer.results().find((r) => r.requestId === "thread-answer")), {
    requestId: "thread-answer", outcome: "answered", action: "save", values: { team: "dev", token: threadSecret },
  });
  const completed = await host.waitEvent((msg) => msg.event.type === "itemUpdated" && msg.event.item.kind === "request" && msg.event.item.requestId === answered.requestId && msg.event.item.state === "resolved");
  assert.ok(completed.event.type === "itemUpdated" && completed.event.item.kind === "request");
  assert.deepEqual(completed.event.item.resolution?.answer, {
    requestKind: "dialog", action: "save", cancelled: false, values: { team: "dev" },
  });
  assert.equal(filesContain(home, threadSecret), false);

  await peer.call("dialog/open", dialog("survive", threadId));
  await peer.call("dialog/open", dialog("inbox"));
  const items = (await readAll(host, threadId)).flatMap((r) => r.items);
  const open = items.find((i) => i.kind === "request" && i.requestKind === "dialog" && i.state === "pending");
  assert.ok(open && open.kind === "request" && open.requestKind === "dialog" && open.channelOpen);
  const requestId = open.requestId;
  const inboxId = (await host.request<InboxListResult>("inbox/list")).requests[0]!.requestId;
  assert.equal(fs.readdirSync(path.join(home, "dialogs")).length, 2);
  // Take a copy while the channel is open, as if the daemon lost power before disconnecting.
  const restartHome = tmpHome();
  fs.cpSync(home, restartHome, { recursive: true, filter: (src) => src !== path.join(home, "run") });
  const restarted = await startDaemon(restartHome);
  t.after(async () => {
    await restarted.stop("test");
    fs.rmSync(restartHome, { recursive: true, force: true });
  });
  const reconnect = await connectHost(restarted.socketPath);
  const thread = await reconnect.request<ThreadReadResult>("thread/read", { threadId, maxItems: 1 });
  const restored = [...thread.items, ...(thread.requiredItems ?? [])].find((i) => i.kind === "request" && i.requestId === requestId);
  assert.ok(restored && restored.kind === "request" && restored.requestKind === "dialog");
  assert.equal(restored.state, "pending");
  assert.equal(restored.channelOpen, false);
  const inbox = await reconnect.request<InboxListResult>("inbox/list");
  assert.deepEqual(inbox.channels, []);
  assert.equal(inbox.requests[0]?.requestId, inboxId);
  assert.equal(inbox.requests[0]?.requestKind === "dialog" && inbox.requests[0].channelOpen, false);
  await assert.rejects(reconnect.request("request/respond", { opId: opId(), threadId, requestId, response: respond("save") }), /Deployment is no longer connected/);
  await reconnect.request("request/respond", { opId: opId(), threadId, requestId, response: cancel });
  await reconnect.request("inbox/respond", { opId: opId(), requestId: inboxId, response: cancel });
  assert.equal(restarted.busy().pendingRequests, 0);
});

test("disconnect marks only that channel's dialogs closed while another channel remains answerable", async (t) => {
  const { daemon, host } = await setup(t);
  const first = await direct(channelPathFor(daemon.config.home));
  const second = await direct(channelPathFor(daemon.config.home));
  const { channelId } = await first.peer.call<{ channelId: string }>("channel/register", { name: "First" });
  await second.peer.call("channel/register", { name: "Second" });
  await first.peer.call("dialog/open", dialog("same-id"));
  await second.peer.call("dialog/open", dialog("same-id"));
  first.socket.end();
  await until(() => host.notifications.find((n) => n.method === "channel/closed" && n.params.channelId === channelId));
  const inbox = await host.request<InboxListResult>("inbox/list");
  const a = inbox.requests.find((r) => r.requestKind === "dialog" && r.source.name === "First");
  const b = inbox.requests.find((r) => r.requestKind === "dialog" && r.source.name === "Second");
  assert.ok(a?.requestKind === "dialog" && !a.channelOpen);
  assert.ok(b?.requestKind === "dialog" && b.channelOpen);
  await assert.rejects(host.request("inbox/respond", { opId: opId(), requestId: a.requestId, response: respond("save") }), failedWith("channel_closed"));
  await host.request("inbox/respond", { opId: opId(), requestId: a.requestId, response: cancel });
  await host.request("inbox/respond", { opId: opId(), requestId: b.requestId, response: cancel });
  assert.deepEqual(await until(() => second.peer.results()[0]), { requestId: "same-id", outcome: "cancelled" });
});

test("layout validation rejects unsafe defaults, duplicate IDs, excessive depth, and unknown required inputs", async (t) => {
  const { home, daemon, host } = await setup(t);
  const { peer } = await direct(channelPathFor(daemon.config.home));
  await peer.call("channel/register", { name: "Layouts" });
  const open = (layout: object) => peer.call("dialog/open", { requestId: "validate", title: "Validate", layout });
  const SECRET = "secret-default-should-never-persist";
  await assert.rejects(open({ type: "textInput", id: "token", secret: true, default: SECRET }), /cannot have a default/);
  await assert.rejects(open({ type: "row", children: [{ type: "textInput", id: "same" }, { type: "button", id: "same", label: "Done" }] }), /Duplicate dialog node ID/);
  let deep: object = { type: "divider" };
  for (let i = 0; i < 12; i++) deep = { type: "group", children: [deep] };
  await assert.rejects(open(deep), failedWith("limit_exceeded"));
  await assert.rejects(open({ type: "text", text: "a".repeat(256 * 1024) }), failedWith("limit_exceeded"));
  await assert.rejects(open({ type: "column", children: Array.from({ length: 500 }, () => ({ type: "divider" })) }), failedWith("limit_exceeded"));
  await assert.rejects(open({ type: "image", src: "file:///tmp/local.png", alt: "" }), /Expected an HTTPS image/);
  assert.deepEqual(fs.readdirSync(path.join(home, "dialogs")), []);
  assert.equal(filesContain(home, SECRET), false);
  await open({
    type: "column",
    children: [{ type: "newInput", id: "future", required: true, default: SECRET },
      { type: "button", id: "send", label: "Send" }, { type: "button", id: "skip", label: "Skip", submit: false }],
  });
  const { requestId } = (await host.request<InboxListResult>("inbox/list")).requests[0]!;
  await assert.rejects(host.request("inbox/respond", { opId: opId(), requestId, response: respond("send") }), /future.*required/);
  await host.request("inbox/respond", { opId: opId(), requestId, response: respond("skip") });
  assert.deepEqual(await until(() => peer.results()[0]), { requestId: "validate", outcome: "answered", action: "skip", values: {} });
  assert.equal(filesContain(home, SECRET), false);
});

test("yaacd channel registers over stdio and receives a dialog result", async (t) => {
  const home = tmpHome();
  const bundle = fileURLToPath(new URL("../../yaacd.mjs", import.meta.url));
  const bridge = spawn(process.execPath, ["--no-turbofan", bundle, "channel", "--name", "CLI integrator", "--description", "Built bundle"], {
    env: { ...process.env, YAAC_HOME: home, YAAC_FAKE_PROVIDER: "1", YAAC_CODEX_PROVIDER: "0", YAAC_CLAUDE_PROVIDER: "0", YAAC_IDLE_TIMEOUT_MS: "3000" },
    stdio: ["pipe", "pipe", "inherit"],
  });
  const peer = new ChannelPeer(bridge.stdout, bridge.stdin);
  const pid = path.join(home, "run", "yaacd.pid");
  t.after(async () => {
    bridge.stdin.end();
    if (fs.existsSync(pid)) process.kill(Number(fs.readFileSync(pid, "utf8")), "SIGTERM");
    await until(() => !fs.existsSync(pid), "daemon exit");
    fs.rmSync(home, { recursive: true, force: true });
  });
  const { channelId } = await peer.response<{ channelId: string }>("yaacd-channel-register");
  const host = await connectHost(socketPathFor(home));
  assert.ok(host.messages.some((m) => m.type === "response" && (m.result as { features?: string[] }).features?.includes("channels")));
  await peer.call("dialog/open", dialog("cli"));
  const inbox = await host.request<InboxListResult>("inbox/list");
  assert.deepEqual(inbox.channels, [{ channelId, name: "CLI integrator", description: "Built bundle" }]);
  await host.request("inbox/respond", { opId: opId(), requestId: inbox.requests[0]?.requestId, response: cancel });
  assert.deepEqual(await until(() => peer.results()[0]), { requestId: "cli", outcome: "cancelled" });
  bridge.stdin.end();
  await once(bridge, "exit");
  await host.close();
});
