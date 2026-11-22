// Take over (D140): continuing here a thread another program holds.

import { test } from "node:test";
import assert from "node:assert/strict";
import { client, fakeOf, opId, readAll, startDaemon, startThread, text, tmpHome, until, waitTurnDone, RpcFailure } from "./helpers.js";

const inUse = (e: unknown) => e instanceof RpcFailure && e.code === "thread_in_use";
const terminal = { external: true, holder: "a terminal", holderIds: ["pid:4242"] };

async function heldThread(home: string) {
  const daemon = await startDaemon(home);
  const c = await client(daemon);
  const { threadId, turnId } = await startThread(c, "/fake say hi");
  await c.request("thread/subscribe", { threadId, epoch: 1, afterSeq: 0 });
  await waitTurnDone(c, threadId, turnId);
  const native = daemon.core.thread(threadId)!.record.native;
  fakeOf(daemon).setExternal(native, terminal);
  await c.waitEvent((e) => e.event.type === "controlChanged" && (e.event as any).control === "external", "external control");
  return { daemon, c, threadId, native };
}

test("take over: the thread becomes writable at once, a notice is journaled, and a replay returns the same result", async () => {
  const home = tmpHome();
  const { daemon, c, threadId } = await heldThread(home);
  await assert.rejects(c.request("turn/start", { opId: opId(), threadId, input: text("/fake say no") }), inUse);

  // The capability is announced on the thread.
  const sub: any = await c.request("thread/subscribe", { threadId });
  assert.ok(sub.effective.capabilities.includes("threadTakeOver"));
  assert.equal(sub.takeOverBlocked, undefined);

  const op = opId();
  assert.deepEqual(await c.request("thread/takeOver", { opId: op, threadId }), {});
  const notice = await c.waitEvent((e) => e.event.type === "itemCompleted" && (e.event as any).item.kind === "notice" && (e.event as any).item.noticeKind === "takenOver", "the notice");
  assert.equal((notice.event as any).item.text, "Taken over from a terminal · the other program is still open and will not see these messages");
  await c.waitEvent((e) => e.event.type === "controlChanged" && (e.event as any).control === "free" && e.seq > notice.seq, "control to be free");

  // The other program is still there, and still reported: the thread stays writable.
  const again: any = await c.request("thread/subscribe", { threadId });
  assert.equal(again.control, "free");
  const sync: any = await c.request("thread/sync", {});
  assert.equal(sync.changed.find((s: any) => s.id === threadId).control, "free");
  const t: any = await c.request("turn/start", { opId: opId(), threadId, input: text("/fake say mine now") });
  await waitTurnDone(c, threadId, t.turnId);

  // The same operation id: the same answer, nothing done twice.
  assert.deepEqual(await c.request("thread/takeOver", { opId: op, threadId }), {});
  const pages = await readAll(c, threadId);
  const notices = pages.flatMap((p) => p.items).filter((i: any) => i.kind === "notice" && i.noticeKind === "takenOver");
  assert.equal(notices.length, 1);
  await c.close();
  await daemon.stop("test");
});

test("take over: a different holder re-locks the thread; the same holder going away just ends the takeover", async () => {
  const home = tmpHome();
  const { daemon, c, threadId, native } = await heldThread(home);
  const fake = fakeOf(daemon);
  await c.request("thread/takeOver", { opId: opId(), threadId });
  await c.waitEvent((e) => e.event.type === "controlChanged" && (e.event as any).control === "free", "free");

  // The same holder reported again (a refresh): nothing changes.
  fake.setExternal(native, terminal);
  const t: any = await c.request("turn/start", { opId: opId(), threadId, input: text("/fake say still mine") });
  await waitTurnDone(c, threadId, t.turnId);

  // Another process opens it: read-only again, and the transcript says why.
  const mark = c.events(threadId).length;
  fake.setExternal(native, { external: true, holder: "another terminal", holderIds: ["pid:4242", "pid:5151"] });
  const ctl = await c.waitEvent((e) => e.event.type === "controlChanged" && (e.event as any).control === "external" && e.seq > c.events(threadId)[mark - 1]!.seq, "re-locked");
  assert.equal((ctl.event as any).holder, "another terminal");
  assert.ok(c.events(threadId).some((e) => e.event.type === "itemCompleted" && /read-only here again/.test((e.event as any).item.text ?? "")));
  await assert.rejects(c.request("turn/start", { opId: opId(), threadId, input: text("/fake say no") }), inUse);
  assert.equal(daemon.core.thread(threadId)!.record.takenOver, null);

  // Taken over again; then the holders go away: the takeover is simply over.
  await c.request("thread/takeOver", { opId: opId(), threadId });
  assert.ok(daemon.core.thread(threadId)!.record.takenOver);
  fake.setExternal(native, { external: false });
  await until(() => daemon.core.thread(threadId)!.record.takenOver === null, "the takeover to end");
  // A holder that comes back later is a new appearance.
  fake.setExternal(native, terminal);
  await c.waitEvent((e) => e.event.type === "controlChanged" && (e.event as any).control === "external" && (e.event as any).holder === "a terminal" && e.seq > ctl.seq, "read-only again");
  await c.close();
  await daemon.stop("test");
});

test("take over: survives a yaacd restart", async () => {
  const home = tmpHome();
  const first = await heldThread(home);
  await first.c.request("thread/takeOver", { opId: opId(), threadId: first.threadId });
  await first.c.close();
  await first.daemon.stop("test restart");

  const daemon = await startDaemon(home);
  const c = await client(daemon);
  // The same program still has it open after the restart.
  fakeOf(daemon).setExternal(first.native, terminal);
  const sub: any = await c.request("thread/subscribe", { threadId: first.threadId });
  assert.equal(sub.control, "free");
  const t: any = await c.request("turn/start", { opId: opId(), threadId: first.threadId, input: text("/fake say after restart") });
  await waitTurnDone(c, first.threadId, t.turnId);
  await c.close();
  await daemon.stop("test");
});

test("take over: refused with the harness's reason when the holder will not let go; a free thread needs nothing", async () => {
  const home = tmpHome();
  const { daemon, c, threadId, native } = await heldThread(home);
  const reason = "Claude Code runs it as a background session; it cannot be continued elsewhere until that session is stopped there";
  fakeOf(daemon).setExternal(native, { external: true, holder: "a background session", holderIds: ["bg:abc"], takeOverBlocked: reason });
  const ctl = await c.waitEvent((e) => e.event.type === "controlChanged" && (e.event as any).takeOverBlocked === reason, "the reason on the control event");
  assert.equal((ctl.event as any).control, "external");
  const sub: any = await c.request("thread/subscribe", { threadId });
  assert.equal(sub.takeOverBlocked, reason);
  await assert.rejects(c.request("thread/takeOver", { opId: opId(), threadId }), (e: RpcFailure) => e.code === "thread_in_use" && e.error.message === reason);
  assert.equal(daemon.core.thread(threadId)!.record.takenOver ?? null, null);

  // Nobody holds it: success, no notice.
  fakeOf(daemon).setExternal(native, { external: false });
  assert.deepEqual(await c.request("thread/takeOver", { opId: opId(), threadId }), {});
  const pages = await readAll(c, threadId);
  assert.equal(pages.flatMap((p) => p.items).filter((i: any) => i.kind === "notice" && i.noticeKind === "takenOver").length, 0);
  await c.close();
  await daemon.stop("test");
});
