// In use elsewhere, kept true without any client action (P386), "Interrupt and take over"
// (P387) and the on-demand re-check `thread/checkControl` (P388), against the fake provider.

import { test } from "node:test";
import assert from "node:assert/strict";
import { client, fakeOf, opId, readAll, startDaemon, startThread, tmpHome, waitTurnDone, RpcFailure } from "./helpers.js";

const BUSY = "the other Claude Code session is running a turn; it can be taken over when that turn ends";
const terminal = { external: true, holder: "another Claude Code session", holderIds: ["pid:4242"] };

async function held(controlRefreshMs: number, control: object = terminal) {
  const home = tmpHome();
  const daemon = await startDaemon(home, { controlRefreshMs, takeOverWaitMs: 2000 });
  const c = await client(daemon);
  const { threadId, turnId } = await startThread(c, "/fake say hi");
  await c.request("thread/subscribe", { threadId, epoch: 1, afterSeq: 0 });
  await waitTurnDone(c, threadId, turnId);
  const native = daemon.core.thread(threadId)!.record.native;
  fakeOf(daemon).setExternal(native, control as never);
  const ext = await c.waitEvent((e) => e.event.type === "controlChanged" && (e.event as any).control === "external", "external control");
  return { daemon, c, threadId, native, fake: fakeOf(daemon), seq: ext.seq };
}

test("in use: the holder leaving clears the banner on its own, with no client action", async () => {
  const { daemon, c, threadId, native, fake, seq } = await held(50);
  // The harness changes without saying so (Claude Code is polled, not told).
  fake.setExternal(native, { external: false }, { quiet: true });
  await c.waitEvent((e) => e.threadId === threadId && e.seq > seq && e.event.type === "controlChanged" && (e.event as any).control === "free", "control to be free");
  assert.equal(daemon.core.thread(threadId)!.external.external, false);
  await c.close();
  await daemon.stop("test");
});

test("in use: a busy holder going idle enables Take over on its own", async () => {
  const { daemon, c, threadId, native, fake } = await held(50, { ...terminal, takeOverBlocked: BUSY });
  const sub: any = await c.request("thread/subscribe", { threadId });
  assert.equal(sub.takeOverBlocked, BUSY);
  fake.setExternal(native, terminal, { quiet: true });
  await c.waitEvent((e) => e.event.type === "controlChanged" && (e.event as any).control === "external" && !(e.event as any).takeOverBlocked, "take over to be allowed");
  assert.deepEqual(await c.request("thread/takeOver", { opId: opId(), threadId }), {});
  await c.close();
  await daemon.stop("test");
});

test("in use: a stale blocked state never refuses a press that would succeed (takeOver asks the harness again)", async () => {
  const { daemon, c, threadId, native, fake } = await held(0, { ...terminal, takeOverBlocked: BUSY });
  // No re-check timer: the thread still shows blocked when the holder has gone idle.
  fake.setExternal(native, terminal, { quiet: true });
  assert.equal(daemon.core.thread(threadId)!.external.takeOverBlocked, BUSY);
  assert.deepEqual(await c.request("thread/takeOver", { opId: opId(), threadId }), {});
  assert.ok(daemon.core.thread(threadId)!.record.takenOver);
  await c.close();
  await daemon.stop("test");
});

test("in use: thread/checkControl asks the harness afresh and pushes the change", async () => {
  const { daemon, c, threadId, native, fake, seq } = await held(0);
  const sub: any = await c.request("thread/subscribe", { threadId });
  assert.ok(sub.effective.capabilities.includes("threadCheckControl"));
  fake.setExternal(native, { external: false }, { quiet: true });
  const before = fake.controlCalls.length;
  assert.deepEqual(await c.request("thread/checkControl", { threadId }), {});
  assert.ok(fake.controlCalls.slice(before).some((k) => k.thread === native && k.fresh), "a fresh look");
  await c.waitEvent((e) => e.seq > seq && e.event.type === "controlChanged" && (e.event as any).control === "free", "control to be free");
  await assert.rejects(c.request("thread/checkControl", { threadId: "nope" }), (e: RpcFailure) => e.code === "thread_not_found");
  await c.close();
  await daemon.stop("test");
});

test("interrupt and take over: only when asked; the holder is interrupted, then the thread is taken over", async () => {
  const { daemon, c, threadId, native, fake } = await held(0, { ...terminal, takeOverBlocked: BUSY });
  const sub: any = await c.request("thread/subscribe", { threadId });
  assert.ok(sub.effective.capabilities.includes("threadTakeOverInterrupt"));
  // A plain press is refused, and interrupts nothing.
  await assert.rejects(c.request("thread/takeOver", { opId: opId(), threadId }), (e: RpcFailure) => e.code === "thread_in_use" && e.error.message === BUSY);
  assert.deepEqual(fake.holderInterrupts, []);
  assert.deepEqual(await c.request("thread/takeOver", { opId: opId(), threadId, interrupt: true }), {});
  assert.deepEqual(fake.holderInterrupts, [native]);
  assert.ok(daemon.core.thread(threadId)!.record.takenOver);
  const items = (await readAll(c, threadId)).flatMap((p) => p.items) as any[];
  assert.ok(items.some((i) => i.kind === "notice" && /^Interrupted another Claude Code session/.test(i.text)));
  assert.ok(items.some((i) => i.kind === "notice" && i.noticeKind === "takenOver"));
  await c.close();
  await daemon.stop("test");
});

test("interrupt and take over: a holder that stays busy refuses the takeover, and nothing is sent again", async () => {
  const { daemon, c, threadId, fake } = await held(0, { ...terminal, takeOverBlocked: BUSY });
  fake.holderStopMs = 60_000;
  await assert.rejects(c.request("thread/takeOver", { opId: opId(), threadId, interrupt: true }), (e: RpcFailure) => e.code === "thread_in_use");
  assert.equal(fake.holderInterrupts.length, 1);
  assert.equal(daemon.core.thread(threadId)!.record.takenOver ?? null, null);
  await c.close();
  await daemon.stop("test");
});
