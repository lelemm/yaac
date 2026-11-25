// P342: a tool approval in a thread whose mode never asks is approved by yaacd itself —
// journaled first, recorded `by: yaacd` with the mode — while questions, forms and every
// prompt in an asking mode still reach the user. Driven through the fake provider (the rule
// lives in the session core) and through Codex's fake app-server.

import { test } from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { codexProviderFactory } from "../src/providers/codex/index.js";
import type { Daemon } from "../src/session/daemon.js";
import { FAKE, client, fakeOf, opId, readAll, sleep, startDaemon, startThread, text, tmpHome, until, waitTurnDone, type TestClient } from "./helpers.js";
import { FakeCodex } from "./fixtures/codex/fake-codex.js";

const MODES = [
  { id: "f-ask", displayName: "Fake ask", isDefault: true, autonomy: "ask" },
  { id: "f-bypass", displayName: "Fake bypass", skipsApprovals: true, autonomy: "full" },
];

async function setup(): Promise<{ d: Daemon; c: TestClient; done: () => Promise<void> }> {
  const home = tmpHome("yaa-");
  fs.writeFileSync(
    path.join(home, "instances.json"),
    JSON.stringify({ instances: [{ id: FAKE, kind: "fake", enabled: true, backend: "auto", implicit: true, env: { FAKE_MODES: JSON.stringify(MODES) } }] }),
  );
  const d = await startDaemon(home);
  const c = await client(d);
  return {
    d,
    c,
    done: async () => {
      await c.close();
      await d.stop("test");
      fs.rmSync(home, { recursive: true, force: true });
    },
  };
}

const requestsOf = async (c: TestClient, threadId: string) => (await readAll(c, threadId)).flatMap((p) => p.items).filter((i) => i.kind === "request") as any[];

test("P342: a tool approval in a never-ask mode is journaled, then approved by yaacd with the mode and the reason", async () => {
  const { d, c, done } = await setup();
  try {
    const { threadId, turnId } = await startThread(c, "/fake approve rm -rf build", { mode: "f-bypass" });
    await c.request("thread/subscribe", { threadId, epoch: 1, afterSeq: 0 });
    await waitTurnDone(c, threadId, turnId);
    // The harness got the narrowest grant on offer, and the tool ran.
    assert.deepEqual(fakeOf(d).answers.map((a) => a.response), [{ requestKind: "approval", decision: "allow" }]);
    const items = (await readAll(c, threadId)).flatMap((p) => p.items);
    assert.ok(items.some((i) => i.kind === "toolCall" && (i as any).status === "completed"), "the command ran");
    const [req] = await requestsOf(c, threadId);
    assert.equal(req.state, "resolved");
    assert.equal(req.resolution.by, "yaacd");
    assert.deepEqual(req.resolution.automatic, { mode: "f-bypass", modeName: "Fake bypass" });
    assert.deepEqual(req.resolution.answer, { requestKind: "approval", decision: "allow" });
    assert.equal(req.reason, "The fake agent wants to run a command.", "the harness's reason stays on the item");
    assert.equal(req.command, "rm -rf build");
    // Journaled before the answer was handed over: it enters the transcript already submitted
    // by yaacd (never shown as waiting for the user), and the harness's confirmation resolves it.
    const rt = d.core.threadByNative(FAKE, d.core.requireThread(threadId).record.native)!;
    const entries = rt.journal!.entries().map((e) => e.event as any);
    const raisedAt = entries.findIndex((e) => e.type === "itemStarted" && e.item.kind === "request");
    assert.equal(entries[raisedAt].item.state, "submitted");
    assert.equal(entries[raisedAt].item.resolution.by, "yaacd");
    const changes = entries.filter((e) => e.type === "requestChanged").map((e) => e.state);
    assert.deepEqual(changes, ["resolved"]);
    assert.ok(entries.findIndex((e) => e.type === "requestChanged") > raisedAt);
    assert.ok(!c.events(threadId).some((e) => e.event.type === "itemStarted" && (e.event as any).item.kind === "request" && (e.event as any).item.state === "pending"), "no window ever saw it pending");
    const onDisk = JSON.parse(fs.readFileSync(path.join(rt.dir, "requests.json"), "utf8"));
    assert.match(JSON.stringify(onDisk), /"by":"yaacd"/);
    // It never needed the user.
    assert.equal(d.busy().pendingRequests, 0);
    const sync: any = await c.request("thread/sync", {});
    assert.notEqual(sync.changed.find((s: any) => s.id === threadId).state, "needsAttention");
  } finally {
    await done();
  }
});

test("P342: a question in a never-ask mode is never answered automatically", async () => {
  const { d, c, done } = await setup();
  try {
    const { threadId, turnId } = await startThread(c, "/fake ask Which fix do you want? | Patch decoder | Rewrite parser", { mode: "f-bypass" });
    await c.request("thread/subscribe", { threadId, epoch: 1, afterSeq: 0 });
    await until(() => d.busy().pendingRequests === 1, "the question to be pending");
    await sleep(300);
    assert.equal(d.busy().pendingRequests, 1);
    assert.equal(fakeOf(d).answers.length, 0);
    const [req] = await requestsOf(c, threadId);
    assert.equal(req.requestKind, "question");
    assert.equal(req.state, "pending");
    await c.request("request/respond", { opId: opId(), threadId, requestId: req.requestId, response: { requestKind: "question", answers: { q1: { selected: ["Patch decoder"] } } } });
    await waitTurnDone(c, threadId, turnId);
    const [after] = await requestsOf(c, threadId);
    assert.equal(after.resolution.by, "client");
  } finally {
    await done();
  }
});

test("P342: an asking mode still asks, and a mode switch mid-thread changes the outcome both ways", async () => {
  const { d, c, done } = await setup();
  try {
    // The default mode asks; the card carries the reason.
    const { threadId, turnId } = await startThread(c, "/fake approve make deploy");
    await c.request("thread/subscribe", { threadId, epoch: 1, afterSeq: 0 });
    await until(() => d.busy().pendingRequests === 1, "the approval to be pending");
    await sleep(200);
    let reqs = await requestsOf(c, threadId);
    assert.equal(reqs[0].state, "pending");
    assert.equal(reqs[0].reason, "The fake agent wants to run a command.");
    assert.equal(fakeOf(d).answers.length, 0);
    await c.request("request/respond", { opId: opId(), threadId, requestId: reqs[0].requestId, response: { requestKind: "approval", decision: "allow" } });
    await waitTurnDone(c, threadId, turnId);

    // Into the never-ask mode: approved by yaacd.
    const t2: any = await c.request("turn/start", { opId: opId(), threadId, input: text("/fake approve make deploy"), mode: "f-bypass" });
    await waitTurnDone(c, threadId, t2.turnId);
    reqs = await requestsOf(c, threadId);
    assert.equal(reqs.length, 2);
    assert.equal(reqs.find((r) => r.turnId === t2.turnId).resolution.by, "yaacd");

    // Back to asking: the prompt reaches the user again.
    const t3: any = await c.request("turn/start", { opId: opId(), threadId, input: text("/fake approve make deploy"), mode: "f-ask" });
    await until(() => d.busy().pendingRequests === 1, "the third approval to be pending");
    reqs = await requestsOf(c, threadId);
    const third = reqs.find((r) => r.turnId === t3.turnId);
    assert.equal(third.state, "pending");
    await c.request("request/respond", { opId: opId(), threadId, requestId: third.requestId, response: { requestKind: "approval", decision: "deny" } });
    await waitTurnDone(c, threadId, t3.turnId);
    assert.equal(fakeOf(d).answers.length, 3);
  } finally {
    await done();
  }
});

test("P342 with Codex: full access approves a command itself; an asking mode leaves the next one to the user", async () => {
  const home = tmpHome("yaa-cx-");
  const fake = await FakeCodex.start(path.join(home, "codex.sock"));
  const thread = { id: "auto-thread", cwd: home, status: { type: "idle" } };
  let turnN = 0;
  fake.handlers.set("account/read", () => ({ account: { type: "apiKey" }, requiresOpenaiAuth: true }));
  fake.handlers.set("model/list", () => ({ data: [], nextCursor: null }));
  fake.handlers.set("thread/list", () => ({ data: [], nextCursor: null }));
  fake.handlers.set("thread/start", () => ({ thread }));
  fake.handlers.set("thread/read", () => ({ thread }));
  fake.handlers.set("turn/start", () => ({ turn: { id: `turn-${++turnN}`, status: "inProgress" } }));
  const daemon = await startDaemon(home, { factories: [codexProviderFactory], env: { YAAC_FAKE_PROVIDER: "0", YAAC_CODEX_PROVIDER: "0" } });
  try {
    const c = await client(daemon);
    const added = await c.request("providerInstance/add", { opId: opId(), kind: "codex", endpoint: { socket: fake.socket } });
    const { threadId } = await c.request("thread/start", { opId: opId(), instance: added.instance.id, cwd: os.tmpdir(), input: text("go"), mode: "codex:full-access" });
    await c.request("thread/subscribe", { threadId, epoch: 1, afterSeq: 0 });
    const approval = (id: number, turn: string, itemId: string) =>
      fake.push({ id, method: "item/commandExecution/requestApproval", params: { kind: "command", threadId: thread.id, turnId: turn, itemId, startedAtMs: 0, environmentId: null, command: "nohup sleep 30 &", reason: "starts a background process", availableDecisions: ["accept", "acceptForSession", "decline", "cancel"] } });
    approval(31, "turn-1", "exec-a");
    await until(() => fake.replies.has(31), "yaacd to answer Codex");
    assert.deepEqual(fake.replies.get(31), { decision: "accept" });
    fake.push({ method: "serverRequest/resolved", params: { threadId: thread.id, requestId: 31 } });
    await until(() => c.events(threadId).some((e) => e.event.type === "requestChanged" && (e.event as any).state === "resolved"), "resolved");
    const resolved = c.events(threadId).find((e) => (e.event as any).state === "resolved")!.event as any;
    assert.equal(resolved.resolution.by, "yaacd");
    assert.equal(resolved.resolution.automatic.mode, "codex:full-access");
    assert.equal(resolved.resolution.automatic.modeName, "Full access");
    const [req] = await requestsOf(c, threadId);
    assert.equal(req.reason, "starts a background process");
    fake.push({ method: "turn/completed", params: { threadId: thread.id, turn: { id: "turn-1", status: "completed", items: [], error: null } } });
    await until(() => c.events(threadId).some((e) => e.event.type === "turnCompleted"), "turn 1 done");

    await c.request("turn/start", { opId: opId(), threadId, input: text("again"), mode: "codex:auto" });
    approval(32, "turn-2", "exec-b");
    await until(() => daemon.busy().pendingRequests === 1, "the second approval to be pending");
    await sleep(200);
    assert.equal(fake.replies.has(32), false, "an asking mode leaves it to the user");
    await c.close();
  } finally {
    await daemon.stop("test");
    await fake.close();
    fs.rmSync(home, { recursive: true, force: true });
  }
});

test("P342: in a never-ask thread, an approval the user's own ask rule forced and a form still reach the user", async () => {
  const { d, c, done } = await setup();
  try {
    const { threadId, turnId } = await startThread(c, "/fake say ready", { mode: "f-bypass" });
    await c.request("thread/subscribe", { threadId, epoch: 1, afterSeq: 0 });
    await waitTurnDone(c, threadId, turnId);
    const native = d.core.requireThread(threadId).record.native;
    const raise = (requestId: string, request: unknown, extra: Record<string, unknown> = {}) =>
      (d.core as any).handleProviderEvent(FAKE, { type: "requestRaised", thread: native, turnId: "", requestId, request, blocking: true, mode: "f-bypass", ...extra });
    raise("r-rule", { requestKind: "approval", approvalKind: "command", title: "Run `rm -rf d`", command: "rm -rf d", reason: null, decisions: [{ id: "allow", label: "Allow once", kind: "allow" }, { id: "deny", label: "Deny", kind: "deny" }] }, { userRule: "Bash(rm:*) (projectSettings)" });
    raise("r-form", { requestKind: "form", message: "Sign in to the server", source: "mcp-x", schema: null });
    await until(() => d.busy().pendingRequests === 2, "both to be pending");
    await sleep(200);
    assert.equal(fakeOf(d).answers.length, 0);
    const reqs = await requestsOf(c, threadId);
    assert.deepEqual(reqs.map((r) => r.state).sort(), ["pending", "pending"]);
    assert.ok(reqs.every((r) => !r.resolution));
  } finally {
    await done();
  }
});

test("P356 applies an idle mode once, refuses an unconfirmed interruption and resumes with a YAAC notice", async () => {
  const { d, c, done } = await setup();
  try {
    const { threadId, turnId } = await startThread(c, "/fake sleep 3000", { mode: "f-ask" });
    const params = { opId: opId(), threadId, sourceTarget: "another", sourceInstance: "different", sourceTitle: "Coordinator",
      mode: MODES[1], allowInterrupt: false };
    await assert.rejects(c.request("thread/setMode", params), /interrupt/i);
    assert.equal(d.core.requireThread(threadId).activeTurn, turnId);
    const confirmed = { ...params, opId: opId(), allowInterrupt: true };
    await c.request("thread/setMode", confirmed);
    await c.request("thread/setMode", confirmed);
    const rt = d.core.requireThread(threadId);
    assert.equal(rt.record.selectedMode, "f-bypass");
    const events = rt.journal!.entries().map(e => e.event);
    assert.equal(events.filter(e => e.type === "turnStarted" && e.origin === "yaac").length, 1);
    assert.ok(events.some(e => e.type === "itemCompleted" && e.item.kind === "notice" && e.item.text.includes("from Coordinator by you")));
    assert.ok(events.some(e => e.type === "itemCompleted" && e.item.kind === "notice" && e.item.text.includes("[YAAC notice]")));
    await until(() => !rt.activeTurn, "the resumed turn to finish");
    await c.request("thread/setMode", { ...params, opId: opId(), mode: MODES[0] });
    assert.equal(rt.activeTurn, null);
    assert.equal(rt.record.selectedMode, "f-ask");
  } finally {
    await done();
  }
});

test("P356 resolves an eligible existing approval when the user selects never-ask", async () => {
  const { d, c, done } = await setup();
  try {
    const { threadId } = await startThread(c, "/fake approve echo ok", { mode: "f-ask" });
    await until(() => d.busy().pendingRequests === 1, "approval");
    await c.request("thread/setMode", { threadId, opId: opId(), sourceTarget: "local", sourceInstance: FAKE,
      sourceTitle: "Coordinator", mode: MODES[1], allowInterrupt: true });
    const [request] = await requestsOf(c, threadId);
    assert.equal(request.resolution.by, "yaacd");
    assert.equal(request.resolution.automatic.mode, "f-bypass");
  } finally {
    await done();
  }
});

test("P356 changes a live worker in place and rejects a cross-instance mode above the ceiling", async () => {
  const { d, c, done } = await setup();
  try {
    const { threadId, turnId } = await startThread(c, "/fake ask Continue?", { mode: "f-ask" });
    await until(() => d.busy().pendingRequests === 1, "the live worker's question");
    const fake = fakeOf(d);
    const effective = fake.effective.bind(fake);
    fake.effective = async thread => {
      const settings = await effective(thread);
      return { ...settings, capabilities: [...(settings.capabilities ?? []), "modeChangeLive"] };
    };
    const descriptor = d.core.registry.require(FAKE).descriptor;
    descriptor.capabilities = [...(descriptor.capabilities ?? []), "modeChangeLive"];
    const changes: string[] = [];
    Object.assign(fake, { setModeLive: async (_thread: string, mode: string) => { changes.push(mode); } });
    const change = (mode: object) => c.request("thread/setMode", {
      opId: opId(), threadId, sourceTarget: "remote", sourceInstance: "other", sourceTitle: "Parent", mode,
    });
    await change(MODES[1]!);
    await change(MODES[0]!);
    await assert.rejects(change({ id: "read", displayName: "Read only", autonomy: "readOnly" }), /equivalent/i);
    assert.deepEqual(changes, ["f-bypass", "f-ask"]);
    assert.equal(d.core.requireThread(threadId).activeTurn, turnId);
    assert.equal(d.busy().pendingRequests, 1);
  } finally {
    await done();
  }
});
