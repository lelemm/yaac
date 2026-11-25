// The "yaacd session layer" row of the test strategy (docs/08-roadmap.md#testing-strategy),
// against the fake provider.

import { test } from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as path from "node:path";
import { createHash } from "node:crypto";
import {
  ClientModel,
  FAKE,
  client,
  fakeOf,
  norm,
  opId,
  readAll,
  sleep,
  startDaemon,
  startThread,
  text,
  tmpHome,
  until,
  waitTurnDone,
  RpcFailure,
  TestClient,
} from "./helpers.js";
import { threadDirName } from "../src/session/thread.js";

const SCRIPT = [
  "Please do the thing.",
  "/fake reason Let me think about the best way to do this carefully.",
  "/fake say Here is a fairly long answer that arrives in many small deltas, one word at a time, as models do.",
  "/fake tool ls -la => total 0",
  "/fake say Done.",
].join("\n");

test("oversized journal items are replayed in frames, and legacy connections remain usable", async () => {
  const home = tmpHome();
  const daemon = await startDaemon(home);
  try {
    const legacy = await client(daemon);
    const { threadId, turnId } = await startThread(legacy, "/fake say small turn");
    await until(() => !daemon.core.thread(threadId)!.activeTurn, "turn to finish");
    const rt = daemon.core.thread(threadId)!;
    const journal = rt.journal!;
    const before = daemon.core.position(threadId);
    const input = "\"🎨\n".repeat(280_000);
    journal.append({ type: "itemCompleted", item: { id: "large", turnId,
      kind: "toolCall", tool: "other", title: "Large tool result", status: "completed", input } });
    journal.append({ type: "itemCompleted", item: { id: "after-large", turnId,
      kind: "agentMessage", status: "completed", text: "Still here" } });
    rt.projection = null;

    await subscribeAndCollect(legacy, threadId, before);
    assert.deepEqual(await legacy.request("ping"), {});
    assert.ok(!legacy.closed);
    assert.ok(!legacy.events(threadId).some((entry) => entry.seq > before.seq));

    const current = await TestClient.connect(daemon.socketPath);
    assert.equal((await current.initialize("chunks", "test-host", true)).messageChunks, true);
    await subscribeAndCollect(current, threadId, before);
    const oversized = await current.waitEvent((e) => e.event.type === "itemCompleted" && e.event.item.id === "large");
    assert.ok(oversized.event.type === "itemCompleted" && oversized.event.item.kind === "toolCall");
    assert.equal(oversized.seq, before.seq + 1);
    assert.equal(oversized.event.item.input, input);
    const after = await current.waitEvent((e) => e.event.type === "itemCompleted" && e.event.item.id === "after-large");
    assert.equal(after.seq, oversized.seq + 1);
    assert.ok((await readAll(current, threadId)).flatMap((page) => page.items)
      .some((item) => item.id === "large" && item.kind === "toolCall" && item.input === input));
    assert.deepEqual(await current.request("ping", { ignored: input }), {});
    await current.close();
    await legacy.close();
  } finally {
    await daemon.stop("test");
    fs.rmSync(home, { recursive: true, force: true });
  }
});

async function subscribeAndCollect(c: Awaited<ReturnType<typeof client>>, threadId: string, from: { epoch: number; seq: number } | null) {
  const sub: any = await c.request("thread/subscribe", { threadId, ...(from ? { epoch: from.epoch, afterSeq: from.seq } : {}) });
  return sub;
}

test("a full turn with no client attached is replayed intact", async () => {
  const home = tmpHome();
  const daemon = await startDaemon(home);
  const a = await client(daemon, "c_host");
  const { threadId, turnId } = await startThread(a, SCRIPT);
  await a.close(); // nobody is attached while the turn runs
  await until(() => !daemon.core.thread(threadId)!.activeTurn, "turn to finish unattended");

  const b = await client(daemon, "c_host");
  const sub = await subscribeAndCollect(b, threadId, { epoch: 1, seq: 0 });
  assert.equal(sub.mode, "replay");
  assert.equal(sub.fromSeq, 1);
  await waitTurnDone(b, threadId, turnId);
  const model = new ClientModel();
  model.epoch = 1;
  for (const e of b.events(threadId)) model.apply(e);
  assert.equal(model.seq, sub.headSeq);
  const snapshot = await readAll(b, threadId);
  assert.deepEqual(norm(model.list()), norm([...snapshot.flatMap((p) => p.items)].reverse()));
  const kinds = model.list().map((i) => i.kind);
  assert.deepEqual(kinds, ["userMessage", "reasoning", "agentMessage", "toolCall", "agentMessage"]);
  const answer = model.list()[2] as any;
  assert.match(answer.text, /^Here is a fairly long answer/);
  await b.close();
  await daemon.stop("test");
});

test("deltas are coalesced before sequence numbers are assigned", async () => {
  const home = tmpHome();
  const daemon = await startDaemon(home, { coalesceMs: 200 });
  const c = await client(daemon);
  const { threadId, turnId } = await startThread(c, "/fake pace 0\n/fake say one two three four five six seven eight nine ten eleven twelve");
  await c.request("thread/subscribe", { threadId, epoch: 1, afterSeq: 0 });
  await waitTurnDone(c, threadId, turnId);
  const deltas = c.events(threadId).filter((e) => e.event.type === "itemDelta");
  assert.ok(deltas.length <= 2, `12 provider deltas became ${deltas.length} journal events`);
  await c.close();
  await daemon.stop("test");
});

test("replay across a compacted range yields the finished item", async () => {
  const home = tmpHome();
  const daemon = await startDaemon(home, { coalesceMs: 0, coalesceBytes: 1 });
  const c = await client(daemon);
  const { threadId, turnId } = await startThread(c, "/fake say alpha beta gamma delta epsilon zeta eta theta");
  await c.request("thread/subscribe", { threadId, epoch: 1, afterSeq: 0 });
  await waitTurnDone(c, threadId, turnId);
  const journal = daemon.core.thread(threadId)!.journal!;
  const compacted = journal.entries().find((e) => e.through !== undefined);
  assert.ok(compacted, "the delta run was compacted");
  assert.equal(compacted.event.type, "itemUpdated");
  // A client that saw only part of the run replays across it.
  const mid = compacted.seq + 1;
  const b = await client(daemon, "c_b");
  const sub: any = await b.request("thread/subscribe", { threadId, epoch: journal.epoch, afterSeq: mid });
  assert.equal(sub.mode, "replay");
  await waitTurnDone(b, threadId, turnId);
  const first = b.events(threadId)[0]!;
  assert.equal(first.event.type, "itemUpdated");
  assert.equal((first.event as any).throughSeq, compacted.through);
  assert.equal((first.event as any).item.text, "alpha beta gamma delta epsilon zeta eta theta");
  // From zero, the replay reconstructs the same items as a snapshot.
  const z = await client(daemon, "c_z");
  await z.request("thread/subscribe", { threadId, epoch: journal.epoch, afterSeq: 0 });
  await waitTurnDone(z, threadId, turnId);
  const model = new ClientModel();
  model.epoch = journal.epoch;
  for (const e of z.events(threadId)) model.apply(e);
  assert.deepEqual(norm(model.list()), norm((await readAll(z, threadId)).flatMap((p) => p.items).reverse()));
  for (const x of [c, b, z]) await x.close();
  await daemon.stop("test");
});

test("reset when the position predates the journal (trimmed by acknowledgements)", async () => {
  const home = tmpHome();
  const daemon = await startDaemon(home);
  const c = await client(daemon);
  const { threadId, turnId } = await startThread(c, SCRIPT);
  await c.request("thread/subscribe", { threadId, epoch: 1, afterSeq: 0 });
  await waitTurnDone(c, threadId, turnId);
  const head = daemon.core.position(threadId);
  await c.request("thread/ack", { threadId, epoch: head.epoch, seq: head.seq - 2 });
  const journal = daemon.core.thread(threadId)!.journal!;
  assert.equal(journal.trimmedThrough >= 1, true, "acknowledged events were trimmed");
  const sub: any = await c.request("thread/subscribe", { threadId, epoch: head.epoch, afterSeq: 1 });
  assert.equal(sub.mode, "reset");
  // A position at or after the trim point still replays.
  const ok: any = await c.request("thread/subscribe", { threadId, epoch: head.epoch, afterSeq: head.seq - 2 });
  assert.equal(ok.mode, "replay");
  // No position at all: reset.
  const none: any = await c.request("thread/subscribe", { threadId });
  assert.equal(none.mode, "reset");
  // After trimming, a snapshot still has the whole conversation (from the harness's store).
  const pages = await readAll(c, threadId);
  assert.equal(pages.flatMap((p) => p.items).length, 5);
  await c.close();
  await daemon.stop("test");
});

test("reset when the epoch changed (a corrupt journal is started afresh)", async () => {
  const home = tmpHome();
  let daemon = await startDaemon(home);
  let c = await client(daemon);
  const { threadId, turnId } = await startThread(c, "/fake say hello there");
  await c.request("thread/subscribe", { threadId, epoch: 1, afterSeq: 0 });
  await waitTurnDone(c, threadId, turnId);
  const before = daemon.core.position(threadId);
  await c.close();
  await daemon.stop("test");
  // Corrupt a line in the middle of the journal.
  const file = path.join(home, "threads", threadDirName(threadId), "journal", "events.jsonl");
  const lines = fs.readFileSync(file, "utf8").split("\n");
  lines[1] = "{not json";
  fs.writeFileSync(file, lines.join("\n"));
  daemon = await startDaemon(home);
  c = await client(daemon);
  const sub: any = await c.request("thread/subscribe", { threadId, epoch: before.epoch, afterSeq: before.seq });
  assert.equal(sub.mode, "reset");
  assert.notEqual(sub.epoch, before.epoch);
  const pages = await readAll(c, threadId);
  assert.equal(pages[0]!.position.epoch, sub.epoch);
  assert.ok(pages.flatMap((p) => p.items).some((i) => i.kind === "agentMessage"), "history comes back from the harness");
  await c.close();
  await daemon.stop("test");
});

test("no gap and no duplicate between thread/read and live events", async () => {
  const home = tmpHome();
  const daemon = await startDaemon(home);
  const c = await client(daemon);
  const words = Array.from({ length: 60 }, (_, i) => `w${i}`).join(" ");
  const { threadId, turnId } = await startThread(c, `/fake pace 4\n/fake say ${words}\n/fake tool make => built\n/fake say ${words}`);
  await sleep(60); // the turn is well under way
  const sub: any = await c.request("thread/subscribe", { threadId });
  assert.equal(sub.mode, "reset");
  // Page the snapshot slowly, in small pages, while events keep arriving.
  const pages = [];
  let cursor: string | null | undefined = null;
  for (;;) {
    const page: any = await c.request("thread/read", { threadId, cursor, maxBytes: 1024 });
    pages.push(page);
    await sleep(15);
    cursor = page.cursor;
    if (!cursor) break;
  }
  const position = pages[0].position;
  assert.ok(pages.every((p: any) => p.position.seq === position.seq), "every page is bound to one snapshot");
  await waitTurnDone(c, threadId, turnId, 20000);
  const model = new ClientModel();
  model.loadSnapshot(pages);
  for (const e of c.events(threadId)) model.apply(e); // throws on a gap or an overlap
  const final = await readAll(c, threadId);
  assert.deepEqual(norm(model.list()), norm(final.flatMap((p) => p.items).reverse()));
  assert.equal(model.seq, final[0]!.position.seq);
  await c.close();
  await daemon.stop("test");
});

test("interactive history pages are bounded and keep a stable anchor as live items arrive", async () => {
  const daemon = await startDaemon(tmpHome());
  const c = await client(daemon);
  try {
    const { threadId } = await startThread(c, "/fake say One\n/fake tool check => okay\n/fake say Three");
    const idle = async () => !(await c.request("thread/subscribe", { threadId }) as { activeTurn?: string }).activeTurn;
    await until(idle, "initial history to settle");
    const before = (await readAll(c, threadId)).flatMap((page) => page.items);
    assert.ok(before.length > 2);
    const first: any = await c.request("thread/read", { threadId, maxItems: 2 });
    assert.deepEqual(first.items.map((item: any) => item.id), before.slice(0, 2).map((item) => item.id));
    assert.match(first.cursor, /^history:/);
    const prompt = before.at(-1)!;
    assert.deepEqual(first.prompts.map((entry: any) => entry.itemId), [prompt.id]);
    assert.ok(!first.items.some((item: any) => item.id === prompt.id));
    await c.request("turn/start", { threadId, opId: opId(), input: text("/fake say Latest") });
    await until(idle, "new history to settle");
    const older: any = await c.request("thread/read", { threadId, cursor: first.cursor, maxItems: 2 });
    assert.deepEqual(older.items.map((item: any) => item.id), before.slice(2, 4).map((item) => item.id));
    const sought: any = await c.request("thread/read", { threadId, around: prompt.id, maxItems: 1 });
    assert.deepEqual(sought.items.map((item: any) => item.id), [prompt.id]);
    assert.match(sought.newerCursor, /^history-newer:/);
    const newer: any = await c.request("thread/read", { threadId, cursor: sought.newerCursor, maxItems: 1 });
    assert.deepEqual(newer.items.map((item: any) => item.id), [before.at(-2)!.id]);
    const back: any = await c.request("thread/read", { threadId, cursor: newer.cursor, maxItems: 1 });
    assert.deepEqual(back.items.map((item: any) => item.id), [prompt.id]);
    const latest: any = await c.request("thread/read", { threadId, maxItems: 1 });
    assert.equal(latest.prompts.length, 2);
  } finally {
    await c.close();
    await daemon.stop("test");
  }
});

test("whole-turn history extends the item limit and flags byte-sized fragments", async () => {
  const daemon = await startDaemon(tmpHome());
  const c = await client(daemon);
  try {
    const command = "/fake tool check => " + "output ".repeat(350);
    const { threadId } = await startThread(c, ["/fake say Intro", command, "/fake say Result"].join("\n"));
    const idle = async () => !(await c.request("thread/subscribe", { threadId }) as { activeTurn?: string }).activeTurn;
    await until(idle, "older turn to settle");
    await c.request("turn/start", { threadId, opId: opId(), input: text("/fake say Latest") });
    await until(idle, "latest turn to settle");

    const all = (await readAll(c, threadId)).flatMap((page) => page.items);
    const latest = all.filter((item) => item.turnId === all[0]!.turnId);
    const older = all.filter((item) => item.turnId !== all[0]!.turnId);
    const first: any = await c.request("thread/read", { threadId, maxItems: 1, wholeTurns: true });
    assert.deepEqual(first.items.map((item: any) => item.id), latest.map((item) => item.id));
    assert.ok(!first.continuesTurn);
    const request = { threadId, maxItems: 1, maxBytes: 1024, wholeTurns: true };
    let page: any = await c.request("thread/read", { ...request, cursor: first.cursor });
    assert.equal(page.continuesTurn, true);
    const ids = page.items.map((item: any) => item.id);
    while (page.continuesTurn) {
      page = await c.request("thread/read", { ...request, cursor: page.cursor });
      ids.push(...page.items.map((item: any) => item.id));
    }
    assert.deepEqual(ids, older.map((item) => item.id));

    const sought: any = await c.request("thread/read", { threadId, maxItems: 1, wholeTurns: true, around: older[0]!.id });
    assert.deepEqual(sought.items.map((item: any) => item.id), older.map((item) => item.id));
    const next: any = await c.request("thread/read", { ...request, cursor: sought.newerCursor });
    assert.deepEqual(next.items.map((item: any) => item.id), latest.map((item) => item.id));
  } finally {
    await c.close();
    await daemon.stop("test");
  }
});

test("the journal survives a daemon restart", async () => {
  const home = tmpHome();
  let daemon = await startDaemon(home);
  let c = await client(daemon, "c_r");
  const { threadId, turnId } = await startThread(c, SCRIPT);
  await c.request("thread/subscribe", { threadId, epoch: 1, afterSeq: 0 });
  await waitTurnDone(c, threadId, turnId);
  const head = daemon.core.position(threadId);
  const items = (await readAll(c, threadId)).flatMap((p) => p.items);
  await c.close();
  await daemon.stop("test");

  daemon = await startDaemon(home);
  c = await client(daemon, "c_r");
  const sub: any = await c.request("thread/subscribe", { threadId, epoch: head.epoch, afterSeq: 0 });
  assert.equal(sub.mode, "replay");
  assert.equal(sub.headSeq, head.seq);
  await waitTurnDone(c, threadId, turnId);
  const model = new ClientModel();
  model.epoch = head.epoch;
  for (const e of c.events(threadId)) model.apply(e);
  assert.deepEqual(norm(model.list()), norm([...items].reverse()));
  assert.deepEqual(norm((await readAll(c, threadId)).flatMap((p) => p.items)), norm(items));
  // A new turn continues the same journal.
  const next: any = await c.request("turn/start", { opId: opId(), threadId, input: text("/fake say again") });
  const done = await waitTurnDone(c, threadId, next.turnId);
  assert.ok(done.seq > head.seq);
  await c.close();
  await daemon.stop("test");
});

test("backpressure: a client that falls behind gets one itemUpdated with throughSeq", async () => {
  const home = tmpHome();
  const daemon = await startDaemon(home, { coalesceMs: 0, coalesceBytes: 1, client: { collapseBehind: 4 } });
  const c = await client(daemon);
  const words = Array.from({ length: 40 }, (_, i) => `x${i}`).join(" ");
  const { threadId } = await startThread(c, `/fake pace 1\n/fake say ${words}\n/fake stall`);
  await until(() => daemon.core.thread(threadId)!.projection === null && (daemon.core.thread(threadId)!.journal!.entries().filter((e) => e.event.type === "itemDelta").length >= 40), "deltas journaled");
  // The item is still running (stalled), so nothing is compacted in the journal.
  const sub: any = await c.request("thread/subscribe", { threadId, epoch: 1, afterSeq: 0 });
  assert.equal(sub.mode, "replay");
  await c.waitEvent((e) => e.threadId === threadId && e.seq + 0 >= 0 && ((e.event as any).throughSeq ?? e.seq) >= sub.headSeq, "caught up");
  const collapsed = c.events(threadId).filter((e) => e.event.type === "itemUpdated" && (e.event as any).throughSeq);
  assert.ok(collapsed.length >= 1, "at least one collapsed run");
  const deltasSent = c.events(threadId).filter((e) => e.event.type === "itemDelta").length;
  assert.ok(deltasSent < 40, `fewer deltas than journaled (${deltasSent})`);
  const model = new ClientModel();
  model.epoch = 1;
  for (const e of c.events(threadId)) model.apply(e);
  const msg = model.list().find((i) => i.kind === "agentMessage") as any;
  assert.equal(msg.text, words);
  assert.equal(model.seq, sub.headSeq);
  const turn = daemon.core.thread(threadId)!.activeTurn!;
  await c.request("turn/interrupt", { threadId, turnId: turn });
  await c.close();
  await daemon.stop("test");
});

test("a retried opId acts once", async () => {
  const home = tmpHome();
  const daemon = await startDaemon(home);
  const c = await client(daemon);
  const fake = fakeOf(daemon);
  const before = fake.nativeIds().length;
  const params = { opId: "op-once", instance: FAKE, cwd: home, input: text("/fake say once") };
  const [r1, r2] = await Promise.all([c.request("thread/start", params), c.request("thread/start", params)]);
  const r3 = await c.request("thread/start", params);
  assert.deepEqual(r1, r2);
  assert.deepEqual(r1, r3);
  assert.equal(fake.nativeIds().length, before + 1, "one conversation in the harness");
  const t = r1 as any;
  await c.request("thread/subscribe", { threadId: t.threadId, epoch: 1, afterSeq: 0 });
  await waitTurnDone(c, t.threadId, t.turnId);
  const turn = { opId: "op-turn", threadId: t.threadId, input: text("/fake say twice?") };
  const a: any = await c.request("turn/start", turn);
  const b: any = await c.request("turn/start", turn);
  assert.equal(a.turnId, b.turnId);
  await waitTurnDone(c, t.threadId, a.turnId);
  assert.equal(fake.turnLog(daemon.core.thread(t.threadId)!.record.native).length, 2, "two paid turns, not three");
  // The recorded outcome survives a restart.
  await c.close();
  await daemon.stop("test");
  const d2 = await startDaemon(home);
  const c2 = await client(d2);
  assert.deepEqual(await c2.request("thread/start", params), r1);
  await c2.close();
  await d2.stop("test");
});

test("a crash between handing over and recording yields outcome_unknown", async () => {
  const home = tmpHome();
  const daemon = await startDaemon(home);
  const c = await client(daemon);
  const t: any = await startThread(c, "/fake say first");
  await c.request("thread/subscribe", { threadId: t.threadId, epoch: 1, afterSeq: 0 });
  await waitTurnDone(c, t.threadId, t.turnId);
  const fake = fakeOf(daemon);
  // The harness accepts the turn, then yaacd "dies" before it records the outcome.
  let release!: () => void;
  fake.afterAccept = () => new Promise<void>((r) => (release = r));
  const pending = c.call("turn/start", { opId: "op-crash", threadId: t.threadId, input: text("/fake say second") });
  await until(() => release, "the harness to accept");
  // Simulate the crash: the op log has its "begin" record and nothing else.
  const opsFile = path.join(home, "ops.jsonl");
  const snapshot = fs.readFileSync(opsFile, "utf8");
  assert.match(snapshot, /"op":"op-crash","method":"turn\/start","phase":"begin"/);
  const crashedHome = tmpHome();
  fs.cpSync(home, crashedHome, { recursive: true });
  fs.writeFileSync(path.join(crashedHome, "ops.jsonl"), snapshot);
  release();
  await pending;
  await c.close();
  await daemon.stop("test");
  const d2 = await startDaemon(crashedHome);
  const c2 = await client(d2);
  const again = await c2.call("turn/start", { opId: "op-crash", threadId: t.threadId, input: text("/fake say second") });
  assert.equal(again.type, "error");
  assert.equal((again as any).error.code, "outcome_unknown");
  // Never re-sent: the same answer again, and no new turn in the harness.
  const third = await c2.call("turn/start", { opId: "op-crash", threadId: t.threadId, input: text("/fake say second") });
  assert.equal((third as any).error.code, "outcome_unknown");
  await c2.close();
  await d2.stop("test");
});

function sha(b: Buffer): string {
  return createHash("sha256").update(b).digest("hex");
}

test("uploads: bytes under credit, verified on end; a hash mismatch is upload_mismatch", async () => {
  const home = tmpHome();
  const daemon = await startDaemon(home);
  const c = await client(daemon);
  const data = Buffer.alloc(700 * 1024, 7);
  const begin: any = await c.request("upload/begin", { name: "../../etc/evil name.bin", size: data.length, sha256: sha(data) });
  assert.equal(begin.exists, false);
  assert.ok(begin.credit > 0 && begin.credit < data.length);
  // Send within credit; more credit arrives as the daemon writes bytes to disk.
  let credit = begin.credit as number;
  let sent = 0;
  while (sent < data.length) {
    if (credit === 0) {
      const grants = () => c.notifications.filter((n) => n.method === "stream/credit").reduce((a, n) => a + n.params.bytes, 0);
      const had = grants();
      await until(() => grants() > had, "credit");
      credit += grants() - had;
    }
    const n = Math.min(16 * 1024, credit, data.length - sent);
    c.sendData(begin.stream, data.subarray(sent, sent + n));
    sent += n;
    credit -= n;
  }
  const end: any = await c.request("upload/end", { uploadId: begin.uploadId });
  assert.equal(end.size, data.length);
  // The same content again: no bytes need to be sent.
  const again: any = await c.request("upload/begin", { name: "copy.bin", size: data.length, sha256: sha(data) });
  assert.equal(again.exists, true);
  const read: any = await c.request("upload/read", { uploadId: begin.uploadId, maxBytes: 10 });
  assert.equal(read.present, true);
  assert.equal(Buffer.from(read.dataBase64, "base64").length, 10);
  assert.equal(read.more, true);
  // A mismatch: declared digest does not match the bytes.
  const bad = Buffer.from("not what was declared");
  const b2: any = await c.request("upload/begin", { name: "x.txt", size: bad.length, sha256: sha(Buffer.from("something else")) });
  c.sendData(b2.stream, bad);
  await assert.rejects(c.request("upload/end", { uploadId: b2.uploadId }), (e: RpcFailure) => e.code === "upload_mismatch");
  const partialDir = path.join(daemon.core.uploads.root, "partial");
  assert.deepEqual(fs.readdirSync(partialDir), [], "the partial file was removed");
  // The upload store is private, and file names are sanitised.
  const st = fs.statSync(daemon.core.uploads.root);
  assert.equal(st.mode & 0o077, 0);
  const t: any = await startThread(c, "/fake say got it", { input: [{ type: "text", text: "see file" }, { type: "file", uploadId: begin.uploadId }] });
  await c.request("thread/subscribe", { threadId: t.threadId, epoch: 1, afterSeq: 0 });
  await waitTurnDone(c, t.threadId, t.turnId);
  const fake = fakeOf(daemon);
  const given = fake.received.at(-1)!.input;
  assert.equal(given.files.length, 1);
  assert.match(given.files[0]!.path, /\/1-evil_name\.bin$/);
  assert.match(given.text, /Attached files \(on this machine\):\n- \//);
  const page: any = await c.request("thread/read", { threadId: t.threadId });
  assert.deepEqual(page.uploads, [{ uploadId: begin.uploadId, present: true }]);
  await c.close();
  await daemon.stop("test");
});

test("uploads: the size of one file and of the unfinished uploads together are limited", async () => {
  const home = tmpHome();
  const daemon = await startDaemon(home);
  const c = await client(daemon);
  const digest = "0".repeat(64);
  await assert.rejects(
    c.request("upload/begin", { name: "huge.bin", size: 600 * 1024 * 1024, sha256: digest }),
    (e: RpcFailure) => e.code === "limit_exceeded",
  );
  // Declared, not yet sent: five of 400 MiB fit, a sixth would pass the total.
  const ids: string[] = [];
  for (let i = 0; i < 5; i++) {
    const b: any = await c.request("upload/begin", { name: `p${i}.bin`, size: 400 * 1024 * 1024, sha256: digest });
    ids.push(b.uploadId);
  }
  await assert.rejects(
    c.request("upload/begin", { name: "p5.bin", size: 400 * 1024 * 1024, sha256: digest }),
    (e: RpcFailure) => e.code === "limit_exceeded" && /together/.test(e.message),
  );
  for (const id of ids) await c.request("upload/cancel", { uploadId: id });
  assert.deepEqual(fs.readdirSync(path.join(daemon.core.uploads.root, "partial")), []);
  await c.close();
  await daemon.stop("test");
});

test("a very large tool output is bounded: a head, its true size, the rest on disk", async () => {
  const home = tmpHome();
  const daemon = await startDaemon(home);
  const c = await client(daemon);
  const t: any = await startThread(c, "/fake big-output 3000000");
  await c.request("thread/subscribe", { threadId: t.threadId, epoch: 1, afterSeq: 0 });
  await waitTurnDone(c, t.threadId, t.turnId, 20000);
  const pages = await readAll(c, t.threadId);
  const tool = pages.flatMap((p) => p.items).find((i) => i.kind === "toolCall") as any;
  assert.equal(tool.outputTruncated, true);
  assert.equal(tool.outputBytes, 3000000);
  assert.ok(Buffer.byteLength(tool.output) <= 16 * 1024);
  const bytes = c.events(t.threadId).reduce((a, e) => a + Buffer.byteLength(JSON.stringify(e)), 0);
  assert.ok(bytes < 200 * 1024, `the live stream stayed small (${bytes} bytes)`);
  const rest: any = await c.request("item/read", { threadId: t.threadId, itemId: tool.id, offset: 2999990, maxBytes: 100 });
  assert.equal(rest.present, true);
  assert.equal(rest.totalBytes, 3000000);
  assert.equal(rest.data.length, 10);
  assert.equal(rest.nextOffset, 3000000);
  assert.equal(rest.more, false);
  const head: any = await c.request("item/read", { threadId: t.threadId, itemId: tool.id, offset: 0, maxBytes: 1000 });
  assert.equal(head.data.length, 1000);
  assert.equal(head.more, true);
  for (const p of pages) assert.ok(Buffer.byteLength(JSON.stringify(p)) < 70 * 1024, "pages stay within 64 KiB");
  await c.close();
  await daemon.stop("test");
});
