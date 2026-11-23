// The "Thread control" row of the test strategy, plus the per-thread dispatcher.

import { test } from "node:test";
import assert from "node:assert/strict";
import { client, fakeOf, opId, readAll, sleep, startDaemon, startThread, text, tmpHome, until, waitTurnDone, RpcFailure } from "./helpers.js";

const inUse = (e: unknown) => e instanceof RpcFailure && e.code === "thread_in_use";

test("a thread in use elsewhere: turn/start, turn/steer, and request/respond are rejected; the view keeps updating; control returns", async () => {
  const home = tmpHome();
  const daemon = await startDaemon(home);
  const c = await client(daemon);
  const { threadId, turnId } = await startThread(c, "/fake say hi\n/fake in-use a terminal");
  await c.request("thread/subscribe", { threadId, epoch: 1, afterSeq: 0 });
  await waitTurnDone(c, threadId, turnId);
  const ctl = await c.waitEvent((e) => e.event.type === "controlChanged" && (e.event as any).control === "external", "external control");
  assert.equal((ctl.event as any).holder, "a terminal");
  const sub: any = await c.request("thread/subscribe", { threadId });
  assert.equal(sub.control, "external");
  assert.equal(sub.controlHolder, "a terminal");
  await assert.rejects(c.request("turn/start", { opId: opId(), threadId, input: text("hello") }), inUse);
  await assert.rejects(c.request("turn/steer", { opId: opId(), threadId, turnId, input: text("more") }), inUse);
  const sync: any = await c.request("thread/sync", {});
  assert.equal(sync.changed.find((s: any) => s.id === threadId).control, "external");

  // The other program runs a turn that asks for approval: shown live, not answerable here.
  const fake = fakeOf(daemon);
  const native = daemon.core.thread(threadId)!.record.native;
  fake.setExternal(native, { external: false });
  const ext = fake.runExternalTurn(native, "Working from the terminal.", 300, [{ k: "resolveElsewhere", ms: 400 }, { k: "approve", command: "make release" }]);
  const raised = await c.waitEvent((e) => e.event.type === "itemStarted" && (e.event as any).item.kind === "request", "the external request");
  const started = c.events(threadId).find((e) => e.event.type === "turnStarted" && (e.event as any).origin === "external");
  assert.ok(started, "the external turn is visible");
  await assert.rejects(
    c.request("request/respond", { opId: opId(), threadId, requestId: (raised.event as any).item.requestId, response: { requestKind: "approval", decision: "allow" } }),
    inUse,
  );
  await ext;
  const msg = await c.waitEvent((e) => e.event.type === "itemCompleted" && (e.event as any).item.text === "Working from the terminal.", "live text");
  assert.ok(msg);
  // Control returns when the other party lets go.
  await c.waitEvent((e) => e.event.type === "controlChanged" && (e.event as any).control === "free" && e.seq > msg.seq, "control to return");
  const again: any = await c.request("turn/start", { opId: opId(), threadId, input: text("/fake say mine again") });
  await waitTurnDone(c, threadId, again.turnId);
  await c.close();
  await daemon.stop("test");
});

test("another YAAC client drives the turn: read-only until it ends, then free", async () => {
  const home = tmpHome();
  const daemon = await startDaemon(home);
  const a = await client(daemon, "c_a", "host-a");
  const b = await client(daemon, "c_b", "host-b");
  const { threadId, turnId } = await startThread(a, "/fake say starting\n/fake stall");
  await a.request("thread/subscribe", { threadId, epoch: 1, afterSeq: 0 });
  const subA: any = await a.request("thread/subscribe", { threadId, epoch: 1, afterSeq: 0 });
  assert.equal(subA.control, "mine");
  assert.equal(subA.activeTurn, turnId);
  const subB: any = await b.request("thread/subscribe", { threadId, epoch: 1, afterSeq: 0 });
  assert.equal(subB.control, "otherClient");
  assert.equal(subB.controlHolder, "host-a");
  await assert.rejects(b.request("turn/start", { opId: opId(), threadId, input: text("me too") }), inUse);
  await assert.rejects(b.request("turn/steer", { opId: opId(), threadId, turnId, input: text("x") }), inUse);
  await assert.rejects(b.request("turn/interrupt", { threadId, turnId }), inUse);
  await assert.rejects(a.request("turn/start", { opId: opId(), threadId, input: text("again") }), (e: RpcFailure) => e.code === "turn_in_progress");
  await assert.rejects(a.request("turn/interrupt", { threadId, turnId: "turn-999" }), (e: RpcFailure) => e.code === "turn_mismatch");
  // Both clients see the same events with the same numbering.
  await until(() => b.events(threadId).length >= 4 && a.events(threadId).length >= 4, "events at both");
  const seqs = (c: typeof a) => c.events(threadId).slice(0, 4).map((e) => `${e.seq}:${e.event.type}`);
  assert.deepEqual(seqs(a), seqs(b));
  const ctlA = a.events(threadId).find((e) => e.event.type === "controlChanged")!;
  const ctlB = b.events(threadId).find((e) => e.seq === ctlA.seq)!;
  assert.equal((ctlA.event as any).control, "mine");
  assert.equal((ctlB.event as any).control, "otherClient", "control is relative to the receiving client");
  await a.request("turn/interrupt", { threadId, turnId });
  await waitTurnDone(b, threadId, turnId);
  await b.waitEvent((e) => e.event.type === "controlChanged" && (e.event as any).control === "free", "free at B");
  const mine: any = await b.request("turn/start", { opId: opId(), threadId, input: text("/fake say now me") });
  await a.waitEvent((e) => e.event.type === "controlChanged" && (e.event as any).control === "otherClient", "A sees B drive");
  await waitTurnDone(a, threadId, mine.turnId);
  await a.close();
  await b.close();
  await daemon.stop("test");
});

test("when unsure whether a thread is in use, it is free: only positive evidence makes it read-only", async () => {
  const home = tmpHome();
  const daemon = await startDaemon(home);
  const c = await client(daemon);
  const { threadId, turnId } = await startThread(c, "/fake say hi");
  await c.request("thread/subscribe", { threadId, epoch: 1, afterSeq: 0 });
  await waitTurnDone(c, threadId, turnId);
  const fake = fakeOf(daemon);
  // The provider cannot tell (it throws, or reports uncertain without external).
  fake.control = async () => {
    throw new Error("cannot tell");
  };
  assert.equal(await daemon.core.isHeldAndIdle(threadId), true);
  const t2: any = await c.request("turn/start", { opId: opId(), threadId, input: text("/fake say hello") });
  await waitTurnDone(c, threadId, t2.turnId);
  fake.control = async () => ({ external: false, uncertain: true, holder: "maybe a terminal" });
  const sync: any = await c.request("thread/sync", {});
  const s = sync.changed.find((x: any) => x.id === threadId);
  assert.equal(s.control, "free");
  assert.equal(s.controlHolder, undefined);
  // The dispatcher may hand an agent's message over too.
  const h = daemon.core.enqueueAgentInput(threadId, { input: text("/fake say from an agent") });
  const t3 = await h.started;
  await waitTurnDone(c, threadId, t3);
  await c.close();
  await daemon.stop("test");
});

test("dispatcher: agent input starts a turn on an idle held thread, queues behind a running one, and waits while in use elsewhere", async () => {
  const home = tmpHome();
  const daemon = await startDaemon(home);
  const c = await client(daemon);
  const { threadId, turnId } = await startThread(c, "/fake say first");
  await c.request("thread/subscribe", { threadId, epoch: 1, afterSeq: 0 });
  await waitTurnDone(c, threadId, turnId);
  const core = daemon.core;
  assert.equal(await core.isHeldAndIdle(threadId), true);

  // Idle and held: a new turn, origin mesh.
  const intents: string[] = [];
  const h1 = core.enqueueAgentInput(threadId, {
    input: text("/fake approve make test"),
    beforeStart: () => void intents.push("intent"),
    onStarted: (t) => void intents.push(`started ${t}`),
  });
  const t1 = await h1.started;
  assert.deepEqual(intents, ["intent", `started ${t1}`]);
  assert.equal(await core.isHeldAndIdle(threadId), false);

  // Busy (and waiting on an approval): queued, never answers the request, never joins the turn.
  const h2 = core.enqueueAgentInput(threadId, { input: text("/fake say second") });
  await sleep(100);
  assert.equal(core.queuedInputs(threadId), 1);
  const req = await c.waitEvent((e) => e.event.type === "itemStarted" && (e.event as any).item.kind === "request", "request");
  assert.equal((await readAll(c, threadId)).flatMap((p) => p.items).find((i) => i.id === (req.event as any).item.id)!.kind, "request");
  await c.request("request/respond", { opId: opId(), threadId, requestId: (req.event as any).item.requestId, response: { requestKind: "approval", decision: "deny" } });
  const t2 = await h2.started;
  assert.notEqual(t2, t1);
  await waitTurnDone(c, threadId, t2);

  // In use elsewhere: stays queued until the other program lets go.
  const fake = fakeOf(daemon);
  const native = core.thread(threadId)!.record.native;
  fake.setExternal(native, { external: true, holder: "a terminal" });
  const idleSeen: string[] = [];
  core.heldAndIdle.on((id) => idleSeen.push(id));
  const h3 = core.enqueueAgentInput(threadId, { input: text("/fake say third") });
  await sleep(100);
  assert.equal(core.queuedInputs(threadId), 1);
  fake.setExternal(native, { external: false });
  const t3 = await h3.started;
  await waitTurnDone(c, threadId, t3);
  await until(() => idleSeen.includes(threadId), "held-and-idle signal");
  // A queued dispatch can be withdrawn before it is sent.
  fake.setExternal(native, { external: true });
  const h4 = core.enqueueAgentInput(threadId, { input: text("never") });
  assert.equal(h4.cancel(), true);
  await assert.rejects(h4.started);
  await c.close();
  await daemon.stop("test");
});

test("manual compaction owns the thread, deduplicates its operation, and lets queued input wait", async () => {
  const daemon = await startDaemon(tmpHome());
  const a = await client(daemon, "a", "host-a");
  const b = await client(daemon, "b", "host-b");
  let finish!: () => void;
  const pending = new Promise<void>((resolve) => { finish = resolve; });
  try {
    const { threadId, turnId } = await startThread(a, "/fake say before");
    await a.request("thread/subscribe", { threadId, epoch: 1, afterSeq: 0 });
    await waitTurnDone(a, threadId, turnId);
    const rt = daemon.core.thread(threadId)!;
    const fake = fakeOf(daemon) as ReturnType<typeof fakeOf> & { compact: (thread: string) => Promise<void> };
    const effective = await fake.effective(rt.record.native);
    rt.effective = { ...effective, capabilities: [...(effective.capabilities ?? []), "compact"] };
    let compactCalls = 0;
    fake.compact = async () => {
      compactCalls++;
      await pending;
      daemon.core.noticeItem(rt, "compaction", "Context compacted");
    };

    const params = { threadId, opId: opId() };
    const first = a.request("thread/compact", params);
    await until(() => compactCalls === 1, "compaction to be dispatched");
    const same = a.request("thread/compact", params);
    await assert.rejects(b.request("thread/compact", { threadId, opId: opId() }), inUse);
    await assert.rejects(a.request("thread/compact", { threadId, opId: opId() }), (e: RpcFailure) => e.code === "turn_in_progress");
    const queued = await a.request("turn/start", { threadId, opId: opId(), delivery: "queue", input: text("/fake say after") });
    const { outcome, queueId } = queued as { outcome: string; queueId: string };
    assert.equal(outcome, "queued");
    await assert.rejects(a.request("turn/queuePromote", { threadId, opId: opId(), queueId }), (e: RpcFailure) => e.code === "turn_in_progress");
    assert.equal(daemon.core.queuedOf(rt)[0]?.queueId, queueId);
    assert.equal(rt.activeTurn, null);

    finish();
    assert.deepEqual(await first, {});
    assert.deepEqual(await same, {});
    assert.equal(compactCalls, 1);
    await a.waitEvent((e) => e.event.type === "itemCompleted" && e.event.item.kind === "notice" && e.event.item.text === "Context compacted", "compaction confirmation");
    await a.waitEvent((e) => e.event.type === "itemCompleted" && e.event.item.kind === "agentMessage" && e.event.item.text === "after", "queued turn");
    const items = (await readAll(a, threadId)).flatMap((page) => page.items);
    assert.ok(items.some((item) => item.kind === "agentMessage" && item.text === "before"));
  } finally {
    finish();
    await a.close();
    await b.close();
    await daemon.stop("test");
  }
});
