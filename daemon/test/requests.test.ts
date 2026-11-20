// The "Requests" row of the test strategy: approvals and questions, every request state, and
// secret answers.

import { test } from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as path from "node:path";
import { client, fakeOf, opId, readAll, sleep, startDaemon, startThread, tmpHome, until, waitTurnDone, RpcFailure, text } from "./helpers.js";
import type { Item } from "../src/protocol/generated/types.js";

function requests(items: Item[]): any[] {
  return items.filter((i) => i.kind === "request");
}

function grepTree(dir: string, needle: string): string[] {
  const hits: string[] = [];
  const walk = (d: string) => {
    for (const e of fs.readdirSync(d, { withFileTypes: true })) {
      const p = path.join(d, e.name);
      if (e.isDirectory()) walk(p);
      else if (e.isFile() && fs.readFileSync(p).includes(needle)) hits.push(p);
    }
  };
  walk(dir);
  return hits;
}

test("an approval and a question asked while no client is attached are delivered on reconnect and answerable", async () => {
  const home = tmpHome();
  const daemon = await startDaemon(home);
  const a = await client(daemon, "c_host", "laptop");
  const { threadId, turnId } = await startThread(a, "/fake approve make deploy\n/fake ask Which fix do you want? | Patch decoder | Rewrite parser");
  await a.close();
  await until(() => daemon.busy().pendingRequests === 1, "the approval to be pending");
  await sleep(300);
  assert.equal(daemon.busy().pendingRequests, 1, "never answered or timed out by yaacd");

  const b = await client(daemon, "c_host", "laptop");
  const sync: any = await b.request("thread/sync", {});
  const summary = sync.changed.find((s: any) => s.id === threadId);
  assert.equal(summary.state, "needsAttention");
  assert.equal(summary.control, "mine", "the reconnecting client still drives its turn");
  const sub: any = await b.request("thread/subscribe", { threadId });
  assert.equal(sub.mode, "reset");
  assert.equal(sub.control, "mine");
  const pending = requests((await readAll(b, threadId)).flatMap((p) => p.items));
  assert.equal(pending.length, 1);
  assert.equal(pending[0].requestKind, "approval");
  assert.equal(pending[0].state, "pending");
  assert.deepEqual(pending[0].decisions.map((d: any) => d.id), ["allow", "allowSession", "deny"]);
  await b.request("request/respond", { opId: opId(), threadId, requestId: pending[0].requestId, response: { requestKind: "approval", decision: "allow" } });
  const states = () => b.events(threadId).filter((e) => e.event.type === "requestChanged").map((e) => (e.event as any).state);
  await until(() => states().includes("resolved"), "the approval to resolve");
  assert.deepEqual(states().slice(0, 2), ["submitted", "resolved"]);
  const resolved = b.events(threadId).find((e) => (e.event as any).state === "resolved")!.event as any;
  assert.equal(resolved.resolution.by, "client");
  assert.equal(resolved.resolution.clientId, "c_host");
  assert.equal(resolved.resolution.clientHost, "laptop");
  // The question follows.
  const q = await b.waitEvent((e) => e.event.type === "itemStarted" && (e.event as any).item.requestKind === "question", "the question");
  const item = (q.event as any).item;
  assert.equal(item.questions[0].text, "Which fix do you want?");
  assert.deepEqual(item.questions[0].options.map((o: any) => o.label), ["Patch decoder", "Rewrite parser"]);
  await b.request("request/respond", { opId: opId(), threadId, requestId: item.requestId, response: { requestKind: "question", answers: { q1: { selected: ["Patch decoder"] } } } });
  await waitTurnDone(b, threadId, turnId);
  const final = (await readAll(b, threadId)).flatMap((p) => p.items);
  assert.ok(final.some((i) => i.kind === "agentMessage" && (i as any).text === "You chose: Patch decoder."));
  assert.ok(requests(final).every((r) => r.state === "resolved"), "the answers stay in the transcript");
  await b.close();
  await daemon.stop("test");
});

test("resolved elsewhere: someone else answered, and answering it now is request_not_pending", async () => {
  const home = tmpHome();
  const daemon = await startDaemon(home);
  const c = await client(daemon);
  const { threadId, turnId } = await startThread(c, "/fake resolve-elsewhere 150\n/fake approve rm -rf build");
  await c.request("thread/subscribe", { threadId, epoch: 1, afterSeq: 0 });
  const raised = await c.waitEvent((e) => e.event.type === "itemStarted" && (e.event as any).item.kind === "request", "request");
  const requestId = (raised.event as any).item.requestId;
  const done = await c.waitEvent((e) => (e.event as any).state === "resolved", "resolution");
  assert.equal((done.event as any).resolution.by, "elsewhere");
  await assert.rejects(
    c.request("request/respond", { opId: opId(), threadId, requestId, response: { requestKind: "approval", decision: "deny" } }),
    (e: RpcFailure) => e.code === "request_not_pending",
  );
  await waitTurnDone(c, threadId, turnId);
  await c.close();
  await daemon.stop("test");
});

test("resolved by another YAAC client (on a turn no client drives)", async () => {
  const home = tmpHome();
  const daemon = await startDaemon(home);
  const a = await client(daemon, "c_a", "host-a");
  const b = await client(daemon, "c_b", "host-b");
  const { threadId, turnId } = await startThread(a, "/fake say ready");
  await a.request("thread/subscribe", { threadId, epoch: 1, afterSeq: 0 });
  await waitTurnDone(a, threadId, turnId);
  // Agent-originated input (the mesh) starts a turn no client drives.
  const handle = daemon.core.enqueueAgentInput(threadId, { input: text("/fake approve make test") });
  const meshTurn = await handle.started;
  await b.request("thread/subscribe", { threadId, epoch: 1, afterSeq: 0 });
  const raised = await a.waitEvent((e) => e.event.type === "itemStarted" && (e.event as any).item.kind === "request", "request");
  const requestId = (raised.event as any).item.requestId;
  await b.request("request/respond", { opId: opId(), threadId, requestId, response: { requestKind: "approval", decision: "allow" } });
  const seenByA = await a.waitEvent((e) => (e.event as any).state === "resolved", "resolution at A");
  assert.equal((seenByA.event as any).resolution.by, "client");
  assert.equal((seenByA.event as any).resolution.clientId, "c_b", "A can tell another client answered");
  assert.equal((seenByA.event as any).resolution.clientHost, "host-b");
  await waitTurnDone(a, threadId, meshTurn);
  const started = a.events(threadId).find((e) => e.event.type === "turnStarted" && (e.event as any).turnId === meshTurn)!;
  assert.equal((started.event as any).origin, "mesh");
  await a.close();
  await b.close();
  await daemon.stop("test");
});

test("expired: the harness dropped it, or the turn was interrupted", async () => {
  const home = tmpHome();
  const daemon = await startDaemon(home);
  const c = await client(daemon);
  const t1 = await startThread(c, "/fake expire 100\n/fake approve make deploy");
  await c.request("thread/subscribe", { threadId: t1.threadId, epoch: 1, afterSeq: 0 });
  await c.waitEvent((e) => e.threadId === t1.threadId && (e.event as any).state === "expired", "expiry by the harness");
  await waitTurnDone(c, t1.threadId, t1.turnId);
  const t2 = await startThread(c, "/fake ask Continue? | yes | no");
  await c.request("thread/subscribe", { threadId: t2.threadId, epoch: 1, afterSeq: 0 });
  await c.waitEvent((e) => e.threadId === t2.threadId && e.event.type === "itemStarted" && (e.event as any).item.kind === "request", "question");
  await c.request("turn/interrupt", { threadId: t2.threadId, turnId: t2.turnId });
  const done = await waitTurnDone(c, t2.threadId, t2.turnId);
  assert.equal((done.event as any).status, "interrupted");
  const items = (await readAll(c, t2.threadId)).flatMap((p) => p.items);
  const q = requests(items)[0];
  assert.equal(q.state, "expired");
  assert.equal(q.questions[0].text, "Continue?", "the question text remains in the transcript");
  assert.equal(daemon.busy().pendingRequests, 0);
  await c.close();
  await daemon.stop("test");
});

test("unknown: yaacd stopped between handing over an answer and its confirmation", async () => {
  const home = tmpHome();
  const daemon = await startDaemon(home);
  const c = await client(daemon);
  const { threadId } = await startThread(c, "/fake approve make deploy");
  await c.request("thread/subscribe", { threadId, epoch: 1, afterSeq: 0 });
  const raised = await c.waitEvent((e) => e.event.type === "itemStarted" && (e.event as any).item.kind === "request", "request");
  const requestId = (raised.event as any).item.requestId;
  const fake = fakeOf(daemon);
  fake.respond = () => new Promise(() => {}); // the answer is in flight forever
  void c.call("request/respond", { opId: opId(), threadId, requestId, response: { requestKind: "approval", decision: "allow" } });
  await c.waitEvent((e) => (e.event as any).state === "submitted", "submitted");
  // The crash: state on disk as it is now.
  const crashed = tmpHome();
  fs.cpSync(home, crashed, { recursive: true });
  await c.close();
  await daemon.stop("test");
  const d2 = await startDaemon(crashed);
  const c2 = await client(d2);
  await c2.request("thread/subscribe", { threadId, epoch: 1, afterSeq: 0 });
  const items = (await readAll(c2, threadId)).flatMap((p) => p.items);
  const r = requests(items)[0];
  assert.equal(r.state, "unknown");
  await assert.rejects(
    c2.request("request/respond", { opId: opId(), threadId, requestId, response: { requestKind: "approval", decision: "allow" } }),
    (e: RpcFailure) => e.code === "request_not_pending",
    "never re-sent",
  );
  await c2.close();
  await d2.stop("test");
});

test("a secret answer reaches the harness but is never journaled or sent back in clear", async () => {
  const home = tmpHome();
  const daemon = await startDaemon(home);
  const c = await client(daemon);
  const { threadId, turnId } = await startThread(c, "/fake ask-secret Paste the deploy token");
  await c.request("thread/subscribe", { threadId, epoch: 1, afterSeq: 0 });
  const raised = await c.waitEvent((e) => e.event.type === "itemStarted" && (e.event as any).item.kind === "request", "question");
  const item = (raised.event as any).item;
  assert.equal(item.questions[0].secret, true);
  const SECRET = "hunter2-very-secret-token";
  await c.request("request/respond", { opId: opId(), threadId, requestId: item.requestId, response: { requestKind: "question", answers: { q1: { other: SECRET } } } });
  await waitTurnDone(c, threadId, turnId);
  const fake = fakeOf(daemon);
  assert.equal((fake.answers.at(-1)!.response as any).answers.q1.other, SECRET, "the harness got it");
  const resolved = c.events(threadId).find((e) => (e.event as any).state === "resolved")!.event as any;
  assert.deepEqual(resolved.resolution.answer.answers.q1, { redacted: true });
  const received = c.messages.map((m) => JSON.stringify(m)).join("\n");
  assert.ok(!received.includes(SECRET), "nothing the host received (and could cache) contains it");
  await c.close();
  await daemon.stop("test");
  // Nothing yaacd wrote under its home contains it.
  const hits = grepTree(home, SECRET);
  assert.deepEqual(hits, []);
});
