// The built bundle, end to end: `yaacd connect` over stdio starts the daemon detached,
// initializes, and runs turns against the fake provider. Real crashes (SIGKILL) test the
// outcome_unknown and request-unknown windows.

import { test } from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as path from "node:path";
import { spawn, type ChildProcess } from "node:child_process";
import { fileURLToPath } from "node:url";
import { FrameDecoder, encodeMessageFrame } from "../src/protocol/frame.js";
import { decodeMessage, encodeMessage, type Message } from "../src/protocol/rpc.js";
import { tmpHome, until, text } from "./helpers.js";

const here = path.dirname(fileURLToPath(import.meta.url));
const BUNDLE = path.resolve(here, "../../yaacd.mjs");

class StdioClient {
  messages: Message[] = [];
  private decoder = new FrameDecoder();
  private id = 1;
  exited = false;
  constructor(readonly child: ChildProcess) {
    child.stdout!.on("data", (chunk: Buffer) => {
      for (const f of this.decoder.decode(chunk)) if (f.kind === "message") this.messages.push(decodeMessage(f.payload));
    });
    child.on("exit", () => (this.exited = true));
  }
  static spawn(home: string): StdioClient {
    const child = spawn(process.execPath, ["--no-turbofan", BUNDLE, "connect"], {
      env: { ...process.env, YAAC_HOME: home, YAAC_FAKE_PROVIDER: "1", YAAC_CODEX_PROVIDER: "0", YAAC_CLAUDE_PROVIDER: "0", TMPDIR: path.join(home, "tmp"), YAAC_IDLE_TIMEOUT_MS: "0" },
      stdio: ["pipe", "pipe", "inherit"],
    });
    return new StdioClient(child);
  }
  async request(method: string, params: unknown = {}): Promise<any> {
    const id = this.id++;
    this.child.stdin!.write(encodeMessageFrame(encodeMessage({ type: "request", id, method, params })));
    const m = await until(() => this.messages.find((x) => (x.type === "response" || x.type === "error") && x.id === id), `${method} answer`, 15000);
    if (m.type === "error") throw Object.assign(new Error(m.error.message), { code: m.error.code });
    return (m as any).result;
  }
  events(): any[] {
    return this.messages.filter((m) => m.type === "notification" && m.method === "thread/event").map((m) => (m as any).params);
  }
  async turnDone(turnId: string): Promise<any> {
    return until(() => this.events().find((e) => e.event.type === "turnCompleted" && e.event.turnId === turnId), "turn done", 15000);
  }
  async close(): Promise<void> {
    this.child.stdin!.end();
    await until(() => this.exited, "bridge exit", 5000);
  }
}

const init = { protocol: { min: 1, max: 1 }, client: { name: "yaac-e2e", version: "0.0.0", id: "c_e2e", host: "e2e" } };

test("yaacd connect over stdio: starts the daemon, initializes, and completes a turn", async () => {
  assert.ok(fs.existsSync(BUNDLE), "npm run build first");
  const home = tmpHome("yaacd-e2e-");
  fs.mkdirSync(path.join(home, "tmp"));
  const c = StdioClient.spawn(home);
  const hello = await c.request("initialize", init);
  assert.equal(hello.protocol.agreed, 1);
  assert.equal(typeof hello.schemaHash, "string");
  assert.equal(hello.daemon.stateFormat, 1);
  assert.ok(hello.target.hostname && hello.target.user && hello.target.home);
  const providers = await c.request("provider/list");
  const fake = providers.providers.find((p: any) => p.kind === "fake");
  assert.equal(fake.status, "ready");
  assert.ok(fake.models.every((m: any) => m.id.startsWith("fake-")));
  const started = await c.request("thread/start", { opId: "e2e-1", instance: fake.instance, cwd: home, input: text("/fake reason hmm\n/fake say Hello from the bundle") });
  await c.request("thread/subscribe", { threadId: started.threadId, epoch: 1, afterSeq: 0 });
  const done = await c.turnDone(started.turnId);
  assert.equal(done.event.status, "completed");
  const page = await c.request("thread/read", { threadId: started.threadId });
  assert.equal(page.items[0].text, "Hello from the bundle");
  await c.close();
  // The daemon outlives the bridge; a second bridge attaches to the same one.
  const c2 = StdioClient.spawn(home);
  await c2.request("initialize", init);
  const status = await c2.request("daemon/status");
  assert.equal(status.busy.attachedClients, 1);
  const sync = await c2.request("thread/sync", {});
  assert.ok(sync.changed.some((s: any) => s.id === started.threadId && s.originKind === "yaac"));
  assert.equal((await c2.request("daemon/handover", { when: "now" })).state, "exiting");
  await until(() => c2.exited, "the bridge to end with the daemon");
  const endpoint = process.platform === "win32" ? "yaacd.pid" : "yaacd.sock";
  await until(() => !fs.existsSync(path.join(home, "run", endpoint)), "the daemon endpoint to be released");
});

test("killed between handing a turn over and recording it: outcome_unknown, never re-sent", async () => {
  const home = tmpHome("yaacd-e2e-");
  fs.mkdirSync(path.join(home, "tmp"));
  const c = StdioClient.spawn(home);
  await c.request("initialize", init);
  const t = await c.request("thread/start", { opId: "e2e-t", instance: "pi_fake", cwd: home, input: text("/fake say one") });
  await c.request("thread/subscribe", { threadId: t.threadId, epoch: 1, afterSeq: 0 });
  await c.turnDone(t.turnId);
  // The fake harness accepts the turn, then kills the daemon (SIGKILL) before it records it.
  const params = { opId: "e2e-crash", threadId: t.threadId, input: text("/fake crash\n/fake say two") };
  void c.request("turn/start", params).catch(() => {});
  await until(() => c.exited, "the bridge to lose the daemon");
  const c2 = StdioClient.spawn(home); // starts a fresh daemon
  await c2.request("initialize", init);
  await assert.rejects(c2.request("turn/start", params), (e: any) => e.code === "outcome_unknown");
  await assert.rejects(c2.request("turn/start", params), (e: any) => e.code === "outcome_unknown");
  // The interrupted turn is shown as such, not left running.
  const sync = await c2.request("thread/sync", {});
  assert.equal(sync.changed.find((s: any) => s.id === t.threadId).state, "idle");
  await c2.request("daemon/handover", { when: "now" });
  await until(() => c2.exited, "exit");
});

test("killed between handing an answer over and its confirmation: the request is unknown", async () => {
  const home = tmpHome("yaacd-e2e-");
  fs.mkdirSync(path.join(home, "tmp"));
  const c = StdioClient.spawn(home);
  await c.request("initialize", init);
  const t = await c.request("thread/start", { opId: "e2e-q", instance: "pi_fake", cwd: home, input: text("/fake crash-on-answer\n/fake approve make deploy") });
  await c.request("thread/subscribe", { threadId: t.threadId, epoch: 1, afterSeq: 0 });
  const raised = await until(() => c.events().find((e) => e.event.type === "itemStarted" && e.event.item.kind === "request"), "request", 15000);
  void c.request("request/respond", { opId: "e2e-a", threadId: t.threadId, requestId: raised.event.item.requestId, response: { requestKind: "approval", decision: "allow" } }).catch(() => {});
  await until(() => c.exited, "the daemon to die");
  const c2 = StdioClient.spawn(home);
  await c2.request("initialize", init);
  const page = await c2.request("thread/read", { threadId: t.threadId });
  const req = page.items.find((i: any) => i.kind === "request");
  assert.equal(req.state, "unknown");
  await c2.request("daemon/handover", { when: "now" });
  await until(() => c2.exited, "exit");
});
