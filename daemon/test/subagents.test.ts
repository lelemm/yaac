// Harness sub-agents (D151): the sub-agents a harness runs itself are threads of their own,
// listed under their parent, live while the parent runs, readable, and read-only.

import { test } from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as path from "node:path";
import { SUBAGENT_READ_ONLY } from "../src/providers/fake/index.js";
import { client, fakeOf, opId, readAll, startDaemon, startThread, text, tmpHome, until, waitTurnDone, RpcFailure } from "./helpers.js";

const inUse = (e: unknown) => e instanceof RpcFailure && e.code === "thread_in_use";

async function summaries(c: Awaited<ReturnType<typeof client>>): Promise<any[]> {
  const out: any[] = [];
  let token = "";
  for (;;) {
    const page: any = await c.request("thread/sync", { token });
    out.push(...page.changed);
    token = page.token;
    if (!page.more) return out;
  }
}

test("sub-agents: one started in a turn appears at once under its parent, running, then done", async () => {
  const home = tmpHome();
  const daemon = await startDaemon(home);
  const c = await client(daemon);
  const { threadId, turnId } = await startThread(c, "/fake say looking\n/fake subagent Explore | Map the parser | The parser has three stages.\n/fake subagent general-purpose | Try the fix | It broke. | fail");
  await c.request("thread/subscribe", { threadId, epoch: 1, afterSeq: 0 });
  await waitTurnDone(c, threadId, turnId);
  const parentNative = daemon.core.thread(threadId)!.record.native;

  // Each sub-agent was pushed as soon as it started, running, linked to its parent.
  const updates = () => c.notifications.filter((n) => n.method === "thread/updated").map((n) => n.params.thread);
  const first = await until(() => updates().find((s: any) => s.subagent && s.title === "Map the parser" && s.subagent.status === "running"), "the running sub-agent");
  assert.match(first.spawnedBy, new RegExp(`/${threadId}$`), "its parent is spawnedBy");
  assert.equal(first.subagent.agentType, "Explore");
  assert.equal(first.subagent.agentId, first.nativeId.split(":").pop());
  assert.equal(first.subagent.readOnly, SUBAGENT_READ_ONLY);
  assert.equal(first.control, "external", "held by the agent that started it");
  assert.equal(first.state, "running");

  // Finished: done, and the failed one failed.
  const done = await until(() => updates().find((s: any) => s.id === first.id && s.subagent.status === "completed"), "the sub-agent done");
  assert.equal(done.state, "done");
  const failed = await until(() => updates().find((s: any) => s.title === "Try the fix" && s.subagent?.status === "failed"), "the failed sub-agent");
  assert.equal(failed.state, "failed");

  // The parent counts them among its children; the index lists both with their parent.
  const all = await summaries(c);
  const parent = all.find((s) => s.id === threadId);
  assert.equal(parent.children, 2);
  const kids = all.filter((s) => s.subagent);
  assert.equal(kids.length, 2);
  for (const k of kids) {
    assert.equal(k.spawnedBy, first.spawnedBy);
    assert.ok(k.nativeId.startsWith(`${parentNative}:`));
  }

  // Readable: its own transcript, streamed live into its own journal.
  const pages = await readAll(c, first.id);
  const items = pages.flatMap((p) => p.items);
  assert.ok(items.some((i: any) => i.kind === "agentMessage" && i.text === "The parser has three stages."));
  assert.ok(!items.some((i: any) => i.kind === "agentMessage" && i.text === "looking"), "not the parent's");
  await c.close();
  await daemon.stop("test");
});

test("sub-agents: read-only with the harness's reason — no turns, no writes, no takeover", async () => {
  const home = tmpHome();
  const daemon = await startDaemon(home);
  const c = await client(daemon);
  const { threadId, turnId } = await startThread(c, "/fake subagent Explore | Look around | Found it.");
  await c.request("thread/subscribe", { threadId, epoch: 1, afterSeq: 0 });
  await waitTurnDone(c, threadId, turnId);
  const kid = await until(async () => (await summaries(c)).find((s) => s.subagent?.status === "completed"), "the sub-agent");

  const sub: any = await c.request("thread/subscribe", { threadId: kid.id });
  assert.equal(sub.control, "external");
  assert.equal(sub.takeOverBlocked, SUBAGENT_READ_ONLY);
  for (const cap of ["threadResume", "threadRename", "threadArchive", "threadTakeOver", "interrupt", "threadMove", "modelSwitch"]) {
    assert.ok(!sub.effective.capabilities.includes(cap), `no ${cap}`);
  }
  assert.ok(sub.effective.capabilities.includes("threadRead"));

  await assert.rejects(c.request("turn/start", { opId: opId(), threadId: kid.id, input: text("hello") }), inUse);
  await assert.rejects(c.request("thread/rename", { opId: opId(), threadId: kid.id, title: "x" }), (e: unknown) => e instanceof RpcFailure && /fake agent that started/.test(e.message));
  await assert.rejects(c.request("thread/takeOver", { opId: opId(), threadId: kid.id }), (e: unknown) => e instanceof RpcFailure);
  // Nothing was handed to the harness for it.
  const kidNative = daemon.core.thread(kid.id)!.record.native;
  assert.ok(!fakeOf(daemon).received.some((r) => r.thread === kidNative));
  await c.close();
  await daemon.stop("test");
});

test("sub-agents: an approval a sub-agent asks for is on its own thread, journaled, and answerable there (D162)", async () => {
  const home = tmpHome();
  const daemon = await startDaemon(home);
  const c = await client(daemon);
  const { threadId, turnId } = await startThread(c, "/fake subagent general-purpose | Build it | Built. | approve npm run build");
  await c.request("thread/subscribe", { threadId, epoch: 1, afterSeq: 0 });
  const kid = await until(async () => (await summaries(c)).find((s) => s.subagent && s.state === "needsAttention"), "the sub-agent needing the user");
  const kidRt = daemon.core.thread(kid.id)!;
  const rec = [...kidRt.requests.values()][0]!;
  assert.equal(rec.state, "pending");
  // Journaled on the sub-agent's thread, not the parent's.
  const kept = JSON.parse(fs.readFileSync(path.join(kidRt.dir, "requests.json"), "utf8"));
  assert.deepEqual(kept.map((r: any) => [r.requestId, r.state]), [[rec.requestId, "pending"]]);
  assert.equal(daemon.core.thread(threadId)!.requests.size, 0, "nothing on the parent");
  const parent = (await summaries(c)).find((s) => s.id === threadId);
  assert.notEqual(parent.state, "needsAttention");

  // The sub-agent is read-only for sending, but its approval is the user's: answered there.
  await assert.rejects(c.request("turn/start", { opId: opId(), threadId: kid.id, input: text("hello") }), inUse);
  await c.request("request/respond", { opId: opId(), threadId: kid.id, requestId: rec.requestId, response: { requestKind: "approval", decision: "allow" } });
  const kidNative = kidRt.record.native;
  assert.deepEqual(fakeOf(daemon).answers.map((a) => [a.thread, a.requestId]), [[kidNative, rec.requestId]]);
  await waitTurnDone(c, threadId, turnId);
  await until(() => rec.state === "resolved", "the request resolved");
  await c.close();
  await daemon.stop("test");
});

test("sub-agents: stored ones are listed under their parent, outside the number cap, old ones cut by age", async () => {
  const home = tmpHome();
  const daemon = await startDaemon(home);
  const fake = fakeOf(daemon);
  const parent = fake.addOutsideThread("An orchestrator in a terminal");
  const lonely = fake.addOutsideThread("Another conversation");
  fake.addStoredSubagent(parent, { title: "Recent helper", agentType: "Explore" });
  fake.addStoredSubagent(parent, { title: "Old helper", daysOld: 90 });
  for (let i = 0; i < 5; i++) fake.addStoredSubagent(lonely, { title: `Helper ${i}` });
  const c = await client(daemon);
  await c.request("discovery/caps", { opId: opId(), update: { maxThreads: 4 } });
  await until(async () => (await summaries(c)).some((s) => s.title === "Recent helper"), "the stored sub-agent");
  const all = await summaries(c);
  const p = all.find((s) => s.nativeId === parent);
  const recent = all.find((s) => s.title === "Recent helper");
  assert.ok(recent.spawnedBy.endsWith(`/${p.id}`));
  assert.equal(recent.subagent.status, "completed");
  assert.equal(recent.state, "done");
  assert.ok(!all.some((s) => s.title === "Old helper"), "beyond the age cap");
  // Sub-agents do not use up the number cap: every top-level conversation within it is listed.
  const top = all.filter((s) => !s.subagent && !s.archived);
  assert.ok(top.length <= 4);
  assert.ok(top.some((s) => s.nativeId === parent) && top.some((s) => s.nativeId === lonely));
  assert.equal(all.filter((s) => s.subagent && s.spawnedBy.endsWith(`/${all.find((x) => x.nativeId === lonely).id}`)).length, 4, "at most maxThreads per parent");
  assert.equal(p.children, 1);
  await c.close();
  await daemon.stop("test");
});
