// WP11: discovery caps (with the host's kept threads), lookup by id and older threads, unarchive, the socket-ownership
// fix, the per-target mesh policy, the uncertain holder taken over (D72), stop-and-hold, `now`
// delivery for agents, and the user's queued messages — against the fake provider.

import { test } from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as net from "node:net";
import * as path from "node:path";
import { client, fakeOf, FAKE, opId, RpcFailure, sleep, startDaemon, startThread, TestClient, text, tmpHome, until } from "./helpers.js";
import { addressOf, childOf, itemsOf, kinds, stateOf, untilIdle } from "./mesh-helpers.js";
import type { Daemon } from "../src/session/daemon.js";
import type { DaemonConfig } from "../src/session/daemon.js";

const MESH_OPTS = { waitMaxMs: 3000, permissionWaitMs: 4000, remotePollMs: 200, tickMs: 0, workerPollMs: 0, stopWaitMs: 600 };

async function setup(extra: Partial<DaemonConfig> = {}, home = tmpHome("y11-")): Promise<{ d: Daemon; c: TestClient; home: string; done: (keep?: boolean) => Promise<void> }> {
  const d = await startDaemon(home, { mesh: MESH_OPTS, ...extra });
  const c = await client(d);
  return {
    d,
    c,
    home,
    done: async (keep = false) => {
      await c.close();
      await d.stop("test");
      if (!keep) fs.rmSync(home, { recursive: true, force: true });
    },
  };
}

async function sync(c: TestClient, extra: Record<string, unknown> = {}): Promise<any[]> {
  const out: any[] = [];
  let token = "";
  for (;;) {
    const r: any = await c.request("thread/sync", { token, ...extra });
    out.push(...r.changed);
    token = r.token;
    if (!r.more) return out;
  }
}

async function rescan(c: TestClient, instance = FAKE): Promise<void> {
  await c.request("provider/refresh", { instance });
}

async function addFake(c: TestClient, name: string, env: Record<string, string>): Promise<string> {
  const r: any = await c.request("providerInstance/add", { opId: opId(), kind: "fake", name, enabled: true, backend: "auto", env: { FAKE_SEED: "0", ...env } });
  return r.instance.id;
}

function native(d: Daemon, threadId: string): string {
  return d.core.thread(threadId)!.record.native;
}

function fakeFor(d: Daemon, threadId: string) {
  return fakeOf(d, d.core.thread(threadId)!.record.instance);
}

async function call(d: Daemon, threadId: string, tool: string, args: Record<string, unknown> = {}) {
  const att = fakeFor(d, threadId).meshAttachment(native(d, threadId));
  if (att) return att.call({ tool, args, meta: { threadId: native(d, threadId) } });
  // After a restart, before the harness has resumed it: the thread's stored ticket.
  const ticket = Object.entries(d.mesh.store.state.tickets).find(([, v]) => v.native === native(d, threadId))![0];
  return d.mesh.callWithTicket(ticket, { tool, args, meta: { threadId: native(d, threadId) } });
}

async function idle(d: Daemon, c: TestClient, message = "/fake say ready", extra: Record<string, unknown> = {}): Promise<string> {
  const { threadId } = await startThread(c, message, { agents: "allow", ...extra });
  await untilIdle(d, threadId);
  return threadId;
}

function texts(items: any[], kind: string): string[] {
  return items.filter((i) => i.kind === kind).map((i) => String(i.text ?? (i.parts ?? []).map((p: any) => p.text).join("")));
}

// ── Discovery caps ──────────────────────────────────────────────────────────────────────────

test("the age cap: hundreds of old conversations are not indexed, and the harness is asked for only the recent ones", async () => {
  const { d, c, done } = await setup();
  try {
    const fake = fakeOf(d);
    fake.addOldThreads(300, 40);
    fake.scannedCount = 0;
    await rescan(c);
    const listed = await sync(c);
    assert.equal(listed.filter((t) => t.nativeId?.startsWith("old-")).length, 0, "nothing older than 30 days");
    assert.ok(listed.some((t) => t.nativeId === "ext-terminal-1"), "a recent outside conversation is listed");
    assert.ok(fake.scannedCount < 10, `the scan stopped early (${fake.scannedCount} listed)`);
    assert.equal(d.core.allThreads().filter((t) => t.record.native.startsWith("old-")).length, 0, "not even remembered");
  } finally {
    await done();
  }
});

test("the number cap: only the 50 most recent discovered conversations per instance, threads started from YAAC not counted", async () => {
  const { d, c, done } = await setup();
  try {
    const own = await idle(d, c, "/fake say mine");
    const fake = fakeOf(d);
    fake.addOldThreads(200, 1, "recent-");
    fake.scannedCount = 0;
    await rescan(c);
    const listed = await sync(c);
    const discovered = listed.filter((t) => t.nativeId?.startsWith("recent-"));
    assert.ok(discovered.every((t) => t.id.startsWith("t_")));
    const recent = discovered.map((t) => t.nativeId);
    assert.equal(recent.length, 50);
    assert.ok(recent.includes("recent-0") && recent.includes("recent-49") && !recent.includes("recent-50"), "the newest 50");
    assert.ok(!listed.some((t) => t.nativeId === "ext-terminal-1"), "an older outside conversation fell outside the 50");
    assert.ok(listed.some((t) => t.id === own), "a thread started from YAAC is never cut");
    assert.ok(fake.scannedCount <= 51, `asked for 50 plus the one kept (${fake.scannedCount})`);
  } finally {
    await done();
  }
});

test("both caps at once, the never-cut rules, and an oversized index trimmed on the next scan", async () => {
  const { d, c, done } = await setup();
  try {
    const fake = fakeOf(d);
    // Caps off: a long-used harness floods the index.
    await c.request("discovery/caps", { opId: opId(), update: { enabled: false } });
    fake.addOldThreads(120, 45);
    fake.addOldThreads(80, 2, "mid-");
    await rescan(c);
    assert.ok((await sync(c)).filter((t) => t.nativeId?.startsWith("old-")).length === 120, "all indexed with the caps off");
    // The host keeps some old ones (pinned or muted there; D92): `thread/keep`.
    const byNative = (n: string) => d.core.threadByNative(FAKE, n)!.id;
    await c.request("thread/keep", { threadIds: [byNative("old-5"), byNative("old-6"), byNative("old-7")] });
    // A YAAC thread last active long ago.
    const mine = await idle(d, c, "/fake say mine");
    const rt = d.core.thread(mine)!;
    rt.record.updatedAt = "2025-01-01T00:00:00.000Z";
    rt.saveRecord();
    // Caps on again (30 days, 10 per instance): the index is trimmed by the same rules.
    const before = (await c.request("thread/sync", { token: "" })) as any;
    await c.request("discovery/caps", { opId: opId(), update: { enabled: true, maxThreads: 10 } });
    await until(async () => !(await sync(c)).some((t) => t.nativeId === "old-0"), "the trim");
    const after = await sync(c);
    const natives = after.map((t) => t.nativeId);
    assert.deepEqual(natives.filter((n) => n?.startsWith("mid-")).sort(), Array.from({ length: 10 }, (_, i) => `mid-${i}`).sort(), "the 10 newest within 30 days");
    for (const n of ["old-5", "old-6", "old-7"]) assert.ok(natives.includes(n), `${n} is kept by the host`);
    assert.ok(after.some((t) => t.id === mine), "a thread started from YAAC is kept");
    assert.ok(!natives.includes("old-8") && !natives.includes("mid-10"));
    const incr: any = await c.request("thread/sync", { token: before.token });
    assert.ok(incr.removed.includes(byNative("old-8")), "a trimmed thread is reported removed");
  } finally {
    await done();
  }
});

test("a lookup by id beyond the caps asks the harness and brings that thread in; it drops out again when nothing keeps it", async () => {
  const { d, c, done } = await setup();
  try {
    const fake = fakeOf(d);
    fake.addOldThreads(100, 60);
    await rescan(c);
    const r: any = await c.request("thread/lookup", { id: "old-42" });
    assert.equal(r.threads.length, 1);
    assert.equal(r.threads[0].nativeId, "old-42");
    assert.ok(fake.lookedUp.includes("old-42"), "asked the harness directly");
    const id = r.threads[0].id;
    assert.ok((await sync(c)).some((t) => t.id === id), "now in the index");
    // Its YAAC id works too, without asking the harness again.
    const again: any = await c.request("thread/lookup", { id });
    assert.equal(again.threads[0].id, id);
    assert.equal(fake.lookedUp.length, 1);
    // Not open, pinned, or active: the next scan drops it again.
    await rescan(c);
    assert.ok(!(await sync(c)).some((t) => t.id === id), "dropped after a scan");
    // Kept by the host (pinned there), it is listed again and stays.
    await c.request("thread/keep", { threadIds: [id] });
    assert.ok((await sync(c)).some((t) => t.id === id), "kept: listed again at once");
    await rescan(c);
    assert.ok((await sync(c)).some((t) => t.id === id), "a kept one stays");
    await c.request("thread/keep", { threadIds: [] });
    await rescan(c);
    assert.ok(!(await sync(c)).some((t) => t.id === id), "no longer kept: trimmed at the next scan");
    await assert.rejects(c.request("thread/keep", { threadIds: "x" }), (e: RpcFailure) => e.code === "invalid_params");
    const missing: any = await c.request("thread/lookup", { id: "no-such-conversation" });
    assert.equal(missing.threads.length, 0);
  } finally {
    await done();
  }
});

test("older threads are paged from the harness on demand, newest first, filtered, and usable by id", async () => {
  const { d, c, done } = await setup();
  try {
    const fake = fakeOf(d);
    fake.addOldThreads(45, 50);
    await rescan(c);
    const p1: any = await c.request("thread/older", { limit: 20 });
    assert.equal(p1.threads.length, 20);
    assert.equal(p1.threads[0].nativeId, "old-0", "newest first");
    assert.ok(p1.cursor);
    const p2: any = await c.request("thread/older", { limit: 20, cursor: p1.cursor });
    const p3: any = await c.request("thread/older", { limit: 20, cursor: p2.cursor });
    const all = [...p1.threads, ...p2.threads, ...p3.threads].map((t: any) => t.nativeId);
    assert.equal(new Set(all).size, 45, "every older one, once");
    assert.ok(!p3.cursor);
    assert.ok(!all.includes("ext-terminal-1"), "what is already listed is not repeated");
    assert.ok(!(await sync(c)).some((t) => t.nativeId?.startsWith("old-")), "not indexed by paging");
    const q: any = await c.request("thread/older", { query: "conversation 12" });
    assert.deepEqual(q.threads.map((t: any) => t.nativeId), ["old-12"]);
    const folder: any = await c.request("thread/older", { folder: p1.threads[0].project.path, limit: 5 });
    assert.equal(folder.threads.length, 5);
    const none: any = await c.request("thread/older", { folder: "/no/such/project" });
    assert.equal(none.threads.length, 0);
    // Opening one brings it in.
    const id = p1.threads[3].id;
    const read: any = await c.request("thread/read", { threadId: id });
    assert.ok(Array.isArray(read.items));
    assert.ok((await sync(c)).some((t) => t.id === id));
  } finally {
    await done();
  }
});

test("the caps are a per-target setting that persists, is pushed, and can be switched off", async () => {
  const home = tmpHome("y11-");
  let s = await setup({}, home);
  try {
    const r: any = await s.c.request("discovery/caps", {});
    assert.deepEqual(r.caps, { autoDiscover: true, enabled: true, maxAgeDays: 30, maxThreads: 50 });
    await assert.rejects(s.c.request("discovery/caps", { update: { maxThreads: 5 } }), (e: RpcFailure) => e.code === "invalid_params");
    await assert.rejects(s.c.request("discovery/caps", { opId: opId(), update: { maxThreads: -1 } }), (e: RpcFailure) => e.code === "invalid_params");
    const u: any = await s.c.request("discovery/caps", { opId: opId(), update: { maxAgeDays: 0, maxThreads: 5 } });
    assert.deepEqual(u.caps, { autoDiscover: true, enabled: true, maxAgeDays: 0, maxThreads: 5 });
    await until(() => s.c.notifications.find((n) => n.method === "discovery/updated"), "discovery/updated");
    await s.done(true);
    s = await setup({}, home);
    const back: any = await s.c.request("discovery/caps", {});
    assert.deepEqual(back.caps, { autoDiscover: true, enabled: true, maxAgeDays: 0, maxThreads: 5 });
  } finally {
    await s.done();
  }
});

test("pausing discovery discards in-flight scan results, persists, and leaves explicit lookup available", async () => {
  const home = tmpHome("discovery-");
  let s = await setup({}, home);
  let unblock: (() => void) | undefined;
  try {
    const fake = fakeOf(s.d);
    fake.addOldThreads(1, 2, "existing-");
    await rescan(s.c);
    fake.addOldThreads(2, 2, "unseen-");
    const list = fake.scanThreads.bind(fake);
    let markStarted!: () => void;
    const started = new Promise<void>((resolve) => { markStarted = resolve; });
    const held = new Promise<void>((resolve) => { unblock = resolve; });
    fake.scanThreads = async (options) => {
      const page = await list(options);
      markStarted();
      await held;
      return page;
    };
    const scanning = s.d.core.scanAll();
    await started;
    await s.c.request("discovery/caps", { opId: opId(), update: { autoDiscover: false } });
    unblock!();
    await scanning;
    fake.scanThreads = list;

    const passive = fakeOf(s.d).addOutsideThread("Passive external session");
    await rescan(s.c);
    const current = await sync(s.c);
    assert.ok(current.some((row) => row.nativeId === "existing-0"));
    assert.ok(!current.some((row) => row.nativeId === passive));
    assert.ok(!current.some((row) => row.nativeId?.startsWith("unseen-")));
    const lookedUp: any = await s.c.request("thread/lookup", { id: "unseen-0" });
    assert.equal(lookedUp.threads[0].nativeId, "unseen-0");
    const mine = await idle(s.d, s.c);
    assert.ok((await sync(s.c)).some((row) => row.id === mine));

    await s.done(true);
    s = await setup({}, home);
    const saved: any = await s.c.request("discovery/caps", {});
    assert.equal(saved.caps.autoDiscover, false);
    fakeOf(s.d).addOldThreads(1, 2, "after-restart-");
    await rescan(s.c);
    assert.ok(!(await sync(s.c)).some((row) => row.nativeId === "after-restart-0"));
    await s.c.request("discovery/caps", { opId: opId(), update: { autoDiscover: true } });
    await until(async () => (await sync(s.c)).some((row) => row.nativeId === "after-restart-0"), "discovery resumed");
  } finally {
    unblock?.();
    await s.done();
  }
});

// ── Dismiss and unarchive ───────────────────────────────────────────────────────────────────

test("a thread the host keeps stays indexed beyond the age cap, across a daemon restart", async () => {
  const home = tmpHome("y11-");
  let s = await setup({}, home);
  try {
    fakeOf(s.d).addOldThreads(5, 90);
    const found: any = await s.c.request("thread/lookup", { id: "old-3" });
    const id = found.threads[0].id;
    await s.c.request("thread/keep", { threadIds: [id] });
    await s.done(true);
    s = await setup({}, home);
    await rescan(s.c);
    const listed = (await sync(s.c)).find((t) => t.id === id);
    assert.ok(listed, "a pinned thread older than the age cap stays indexed");
    assert.equal(listed.muted, undefined);
    assert.equal(listed.pinned, undefined);
    assert.equal(listed.dismissed, undefined, "no view flags in a summary");
  } finally {
    await s.done();
  }
});

test("unarchive uses the harness's own unarchive", async () => {
  const { d, c, done } = await setup();
  try {
    const t = await idle(d, c);
    await c.request("thread/archive", { opId: opId(), threadId: t });
    assert.equal(d.core.summaryOf(d.core.thread(t)!).archived, true);
    await c.request("thread/unarchive", { opId: opId(), threadId: t });
    const s = d.core.summaryOf(d.core.thread(t)!);
    assert.equal(s.archived, false);
    const fake = fakeOf(d);
    assert.equal((await fake.lookupThread(native(d, t)))!.archived, false, "the harness unarchived it");
  } finally {
    await done();
  }
});

test("a discovered thread archived from YAAC or in the harness stays listed, archived, across rescans and a restart; Unarchive brings it back (P131)", async () => {
  const home = tmpHome("y11-");
  let s = await setup({}, home);
  try {
    const fake = fakeOf(s.d);
    const fromYaac = fake.addOutsideThread("Archived from YAAC");
    const inHarness = fake.addOutsideThread("Archived in the harness");
    await rescan(s.c);
    const idOf = (rows: any[], n: string) => rows.find((t) => t.nativeId === n);
    let rows = await sync(s.c);
    await s.c.request("thread/archive", { opId: opId(), threadId: idOf(rows, fromYaac).id });
    await fake.archive(inHarness);
    await rescan(s.c);
    await s.done(true);
    s = await setup({}, home);
    await rescan(s.c);
    rows = await sync(s.c);
    for (const n of [fromYaac, inHarness]) {
      const row = idOf(rows, n);
      assert.ok(row, "still listed: the Archived section shows it");
      assert.equal(row.archived, true);
    }
    await s.c.request("thread/unarchive", { opId: opId(), threadId: idOf(rows, inHarness).id });
    assert.equal((await fakeOf(s.d).lookupThread(inHarness))!.archived, false, "the harness unarchived it");
    await rescan(s.c);
    assert.equal(idOf(await sync(s.c), inHarness).archived, false);
  } finally {
    await s.done();
  }
});

// ── The socket ─────────────────────────────────────────────────────────────────────────────

test("a stopping daemon removes only a socket it still owns", async () => {
  const home = tmpHome("y11-");
  const d = await startDaemon(home);
  const sock = d.socketPath;
  // A host that does not close its side keeps the stopping daemon busy for a moment, so a
  // newer daemon can bind the path in between (as soon as this one stops answering).
  const raw = net.createConnection({ path: sock, allowHalfOpen: true });
  await new Promise<void>((r) => raw.once("connect", () => r()));
  raw.on("error", () => {});
  const slow = new TestClient(raw);
  await slow.initialize("c_slow", "slow-host");
  const pidFile = path.join(home, "run", "yaacd.pid");
  const stopping = d.stop("test");
  await until(() => !fs.existsSync(sock), "the old daemon to stop listening");
  const newer = net.createServer((s) => s.end());
  await new Promise<void>((r) => newer.listen(sock, () => r()));
  fs.writeFileSync(pidFile, "999999\n");
  try {
    await stopping;
    assert.ok(fs.existsSync(sock), "the newer daemon's socket is still there");
    assert.equal(fs.readFileSync(pidFile, "utf8"), "999999\n", "and its pid file");
    await new Promise<void>((resolve, reject) => {
      const s = net.connect(sock, () => {
        s.destroy();
        resolve();
      });
      s.on("error", reject);
    });
  } finally {
    raw.destroy();
    await new Promise<void>((r) => newer.close(() => r()));
    fs.rmSync(home, { recursive: true, force: true });
  }
  // Its own socket is removed as before.
  const home2 = tmpHome("y11-");
  const d2 = await startDaemon(home2);
  await d2.stop("test");
  assert.ok(!fs.existsSync(d2.socketPath));
  fs.rmSync(home2, { recursive: true, force: true });
});

// ── Mesh policy and the uncertain holder ───────────────────────────────────────────────────

test("the per-target mode only makes the chat's toggle stricter: askFirst asks a chat set to allow once; alwaysAsk asks every time", async () => {
  const { d, c, done } = await setup();
  try {
    const p: any = await c.request("mesh/policy", {});
    assert.equal(p.policy.mode, "allow");
    await c.request("mesh/policy", { opId: opId(), update: { mode: "askFirst" } });
    const a = await idle(d, c);
    const answerNext = async () => {
      const rt = d.core.thread(a)!;
      const rec = await until(() => [...rt.requests.values()].find((r) => r.requestKind === "meshPermission" && r.state === "pending"), "a permission prompt");
      await c.request("request/respond", { opId: opId(), threadId: a, requestId: rec.requestId, response: { requestKind: "meshPermission", decision: "allow", alwaysAllow: false } });
    };
    const s1 = call(d, a, "spawn_thread", { text: "/fake say one" });
    await answerNext();
    assert.equal((await s1).isError, false);
    const s2 = await call(d, a, "spawn_thread", { text: "/fake say two" });
    assert.equal(s2.isError, false, "askFirst: once per orchestration");
    await c.request("mesh/policy", { opId: opId(), update: { mode: "alwaysAsk" } });
    const s3 = call(d, a, "spawn_thread", { text: "/fake say three" });
    await answerNext();
    assert.equal((await s3).isError, false);
    const s4 = call(d, a, "spawn_thread", { text: "/fake say four" });
    await answerNext();
    assert.equal((await s4).isError, false, "alwaysAsk: every time");
    await assert.rejects(c.request("mesh/policy", { opId: opId(), update: { mode: "never" } }), (e: RpcFailure) => e.code === "invalid_params");
    const now: any = await c.request("mesh/policy", {});
    assert.equal(now.policy.mode, "alwaysAsk");
  } finally {
    await done();
  }
});

test("acceptFromOtherTargets off: agents elsewhere cannot start or message threads here; replies within what this thread started still arrive", async () => {
  const { d, c, done } = await setup();
  try {
    const t = await idle(d, c);
    await c.request("mesh/roster", { selfId: "tg_here", selfName: "here", targets: [{ id: "tg_here", name: "here", reachable: true }, { id: "tg_far", name: "far", reachable: true }] });
    await c.request("mesh/policy", { opId: opId(), update: { acceptFromOtherTargets: false } });
    const p: any = await c.request("mesh/policy", {});
    assert.equal(p.policy.acceptFromOtherTargets, false);
    const msg = (id: string, kind = "message") => ({ message: { id, from: "tg_far/t_stranger", to: `tg_here/${t}`, kind, text: "hello", sentAt: new Date().toISOString(), seq: 1 } });
    await assert.rejects(c.request("mesh/deliver", msg("m_1")), (e: RpcFailure) => e.code === "policy_denied");
    await assert.rejects(c.request("mesh/query", { caller: "tg_far/t_stranger", target: "tg_here", query: { tool: "spawnThread", instance: "", title: "x", text: "/fake say hi" } }), (e: RpcFailure) => e.code === "policy_denied");
    // This thread wrote to the stranger first: the answer arrives.
    await call(d, t, "send_message", { to: "tg_far/t_stranger", text: "are you there?" });
    const ok: any = await c.request("mesh/deliver", msg("m_2"));
    assert.equal(ok.receipt.status, "delivered");
  } finally {
    await done();
  }
});

test("a thread whose control is merely uncertain is taken over and receives an agent message (D72)", async () => {
  const { d, c, done } = await setup();
  try {
    const a = await idle(d, c);
    const b = await idle(d, c, "/fake say b");
    fakeOf(d).setExternal(native(d, b), { external: false, uncertain: true, holder: "unknown" });
    await until(() => d.core.thread(b)!.external.uncertain, "uncertain control");
    assert.equal(d.core.renderSummary(d.core.summaryOf(d.core.thread(b)!), d.core.holderOf(d.core.thread(b)!), "c_test").control, "free");
    const r = await call(d, a, "send_message", { to: addressOf(d, b), text: "/fake say got it" });
    assert.equal(r.isError, false, r.text);
    await until(() => fakeOf(d).received.some((x) => x.thread === native(d, b) && x.input.origin === "mesh"), "the message handed to the harness");
    await untilIdle(d, b);
    assert.ok(texts(await itemsOf(c, b), "agentMessage").includes("got it"));
  } finally {
    await done();
  }
});

// ── Stop and hold ──────────────────────────────────────────────────────────────────────────

test("stop_thread stops the whole subtree and holds it: notices, a real outcome per thread, mail waits, the stopper's message releases", async () => {
  const home = tmpHome("y11-");
  let s = await setup({}, home);
  try {
    const { d, c } = s;
    const a = await idle(d, c);
    const sp = await call(d, a, "spawn_thread", { title: "W1", text: "/fake mesh-spawn title=W2 | /fake stall\n/fake stall" });
    assert.equal(sp.isError, false, sp.text);
    const w1 = await childOf(d, a);
    const w2 = await childOf(d, w1);
    await until(() => d.core.thread(w2)?.activeTurn, "W2 running");
    const r = await call(d, a, "stop_thread", { thread: addressOf(d, w1) });
    assert.equal(r.isError, false, r.text);
    assert.match(r.text, /"W1".*: stopped\./);
    assert.match(r.text, /"W2".*: stopped\./);
    for (const w of [w1, w2]) {
      assert.equal(stateOf(d, w), "stopped");
      const hold = d.core.summaryOf(d.core.thread(w)!).hold!;
      assert.equal(hold.by, addressOf(d, a));
      assert.ok((await kinds(c, w, "notice")).some((n) => /^Stopped by /.test(n.text)), "the worker's transcript says so");
    }
    const again = await call(d, a, "stop_thread", { thread: addressOf(d, w1), subtree: false });
    assert.match(again.text, /was not running/);
    assert.doesNotMatch(again.text, /W2/);
    await s.done(true);
    // The hold survives a restart.
    s = await setup({}, home);
    assert.equal(stateOf(s.d, w1), "stopped");
    // Another agent's message waits, and its sender is told.
    const other = await idle(s.d, s.c, "/fake say other");
    const m = await call(s.d, other, "send_message", { to: addressOf(s.d, w1), text: "/fake say from other" });
    assert.match(m.text, /stopped by/);
    await sleep(300);
    assert.equal(s.d.core.thread(w1)!.activeTurn, null, "a held thread does not start");
    // The stopper's message starts it again (and the queued one goes with it).
    const back = await call(s.d, a, "send_message", { to: addressOf(s.d, w1), text: "/fake say resumed" });
    assert.equal(back.isError, false, back.text);
    await until(() => !s.d.core.thread(w1)!.record.hold, "the hold released");
    await untilIdle(s.d, w1);
    const said = texts(await itemsOf(s.c, w1), "agentMessage");
    assert.ok(said.includes("resumed") && said.includes("from other"), said.join(" | "));
  } finally {
    await s.done();
  }
});

test("a turn that ignores the interrupt is reported as not responding, held anyway, and its process is ended where yaacd owns it", async () => {
  const { d, c, done } = await setup();
  try {
    const shared = await addFake(c, "shared", { FAKE_FORCE_STOP: "0" });
    const a = await idle(d, c);
    await call(d, a, "spawn_thread", { title: "Own", text: "/fake deaf\n/fake stall" });
    await call(d, a, "spawn_thread", { title: "Shared", instance: shared, text: "/fake deaf\n/fake stall" });
    const own = await childOf(d, a, 0);
    const sh = await childOf(d, a, 1);
    await until(() => d.core.thread(own)?.activeTurn && d.core.thread(sh)?.activeTurn, "both running");
    await sleep(300); // past the `deaf` step
    const r = await call(d, a, "stop_all");
    assert.equal(r.isError, false, r.text);
    assert.match(r.text, /"Own".*: did not respond — its harness process was ended/);
    assert.match(r.text, /"Shared".*: did not respond — YAAC does not own/);
    assert.ok(fakeOf(d).forceStopped.includes(native(d, own)));
    assert.equal(stateOf(d, sh), "stopped", "held anyway");
    assert.equal(stateOf(d, own), "stopped");
  } finally {
    await done();
  }
});

test("a pending approval in a stopped thread is not answered: it expires with the turn", async () => {
  const { d, c, done } = await setup();
  try {
    const a = await idle(d, c);
    await call(d, a, "spawn_thread", { title: "Asker", text: "/fake approve make deploy" });
    const w = await childOf(d, a);
    await until(() => d.core.thread(w)!.pendingRequestCount() > 0, "the approval");
    const r = await call(d, a, "stop_thread", { thread: addressOf(d, w) });
    assert.match(r.text, /stopped/);
    const rec = [...d.core.thread(w)!.requests.values()][0]!;
    assert.equal(rec.state, "expired");
    assert.equal(fakeOf(d).answers.length, 0, "never answered");
  } finally {
    await done();
  }
});

test("the user's Stop with hold holds the thread; the user's next message starts it again", async () => {
  const { d, c, done } = await setup();
  try {
    const { threadId, turnId } = await startThread(c, "/fake stall");
    await until(() => d.core.thread(threadId)!.activeTurn, "running");
    await c.request("turn/interrupt", { threadId, turnId, hold: true });
    await untilIdle(d, threadId);
    assert.equal(stateOf(d, threadId), "stopped");
    assert.equal(d.core.summaryOf(d.core.thread(threadId)!).hold!.label, "the user");
    const r: any = await c.request("turn/start", { opId: opId(), threadId, input: text("/fake say again") });
    assert.equal(r.outcome, "started");
    assert.equal(d.core.thread(threadId)!.record.hold, null);
    // Resume without a message.
    await untilIdle(d, threadId);
    await c.request("thread/update", { opId: opId(), threadId, held: true });
    assert.equal(stateOf(d, threadId), "stopped");
    await c.request("thread/update", { opId: opId(), threadId, held: false });
    assert.equal(stateOf(d, threadId), "idle");
  } finally {
    await done();
  }
});

// ── `now` for agents ───────────────────────────────────────────────────────────────────────

test("now: steered into a running turn where the harness can steer, interrupt-and-restart where it cannot", async () => {
  const { d, c, done } = await setup();
  try {
    const nosteer = await addFake(c, "nosteer", { FAKE_STEER: "0" });
    const a = await idle(d, c);
    await call(d, a, "spawn_thread", { title: "Steerable", text: "/fake sleep 1500\n/fake say finished" });
    await call(d, a, "spawn_thread", { title: "Plain", instance: nosteer, text: "/fake stall" });
    const s = await childOf(d, a, 0);
    const p = await childOf(d, a, 1);
    await until(() => d.core.thread(s)?.activeTurn && d.core.thread(p)?.activeTurn, "both running");
    const turnOfP = d.core.thread(p)!.activeTurn;
    const r1 = await call(d, a, "send_message", { to: addressOf(d, s), text: "look at the tests too", delivery: "now" });
    assert.match(r1.text, /steered into its running turn/);
    const r2 = await call(d, a, "send_message", { to: addressOf(d, p), text: "/fake say switched", delivery: "now" });
    assert.match(r2.text, /interrupted and a new one started/);
    await untilIdle(d, s);
    await untilIdle(d, p);
    const sItems = await itemsOf(c, s);
    assert.ok(texts(sItems, "agentMessage").some((t) => t.startsWith("Steered:")), "the steered message reached the running turn");
    assert.equal(sItems.filter((i) => i.kind === "meshReceived").length, 2, "the task and the steered message, both as agent messages");
    assert.ok(!sItems.some((i) => i.kind === "userMessage" && JSON.stringify(i).includes("look at the tests")), "never as the user");
    const pRt = d.core.thread(p)!;
    assert.notEqual(pRt.lastStatus, "interrupted");
    assert.ok(texts(await itemsOf(c, p), "agentMessage").includes("switched"));
    assert.ok(fakeFor(d, p).turnLog(native(d, p)).length >= 2 && turnOfP, "a new turn started");
    const rec = d.mesh.store.all().find((x) => x.message.text === "/fake say switched")!;
    assert.equal(rec.receipt?.handling, "interruptedAndDelivered");
  } finally {
    await done();
  }
});

test("now is only for threads beneath the sender, and never interrupts a thread waiting on its user", async () => {
  const { d, c, done } = await setup();
  try {
    const a = await idle(d, c);
    const { threadId: stranger } = await startThread(c, "/fake sleep 800\n/fake say done", { agents: "allow" });
    await until(() => d.core.thread(stranger)!.activeTurn, "running");
    const r = await call(d, a, "send_message", { to: addressOf(d, stranger), text: "/fake say later", delivery: "now" });
    assert.match(r.text, /only for threads beneath the sender/);
    const rec = d.mesh.store.all().find((x) => x.message.text === "/fake say later")!;
    assert.equal(rec.receipt?.reason, "notBeneathSender");
    assert.equal(rec.receipt?.handling, "queued");
    await untilIdle(d, stranger);
    await until(async () => texts(await itemsOf(c, stranger), "agentMessage").includes("later"), "delivered as queue afterwards");
    // A worker waiting on its user.
    await call(d, a, "spawn_thread", { title: "Asker", text: "/fake approve make it" });
    const w = await childOf(d, a);
    await until(() => d.core.thread(w)!.pendingRequestCount() > 0, "the approval");
    const r2 = await call(d, a, "send_message", { to: addressOf(d, w), text: "hurry", delivery: "now" });
    assert.match(r2.text, /waiting on the user/);
    assert.equal([...d.core.thread(w)!.requests.values()][0]!.state, "pending", "the request is untouched");
  } finally {
    await done();
  }
});

// ── The user's queued messages ─────────────────────────────────────────────────────────────

test("turn/start while a turn runs: refused without delivery; queued, visible, edited, cancelled, and delivered in order", async () => {
  const { d, c, done } = await setup();
  try {
    const { threadId } = await startThread(c, "/fake sleep 800\n/fake say first");
    await until(() => d.core.thread(threadId)!.activeTurn, "running");
    await c.request("thread/subscribe", { threadId });
    await assert.rejects(c.request("turn/start", { opId: opId(), threadId, input: text("x") }), (e: RpcFailure) => e.code === "turn_in_progress");
    const q1: any = await c.request("turn/start", { opId: opId(), threadId, input: text("/fake say one"), delivery: "queue" });
    const q2: any = await c.request("turn/start", { opId: opId(), threadId, input: text("/fake say two"), delivery: "queue" });
    const q3: any = await c.request("turn/start", { opId: opId(), threadId, input: text("/fake say three"), delivery: "queue" });
    assert.equal(q1.outcome, "queued");
    assert.equal(q1.turnId, "");
    assert.ok(q1.queueId);
    const sub: any = await c.request("thread/subscribe", { threadId });
    assert.deepEqual(sub.queued.map((q: any) => q.queueId), [q1.queueId, q2.queueId, q3.queueId]);
    await c.request("turn/queueEdit", { opId: opId(), threadId, queueId: q1.queueId, input: text("/fake say one, edited") });
    await c.request("turn/queueCancel", { opId: opId(), threadId, queueId: q2.queueId });
    await assert.rejects(c.request("turn/queueCancel", { opId: opId(), threadId, queueId: q2.queueId }), (e: RpcFailure) => e.code === "queue_entry_not_found");
    await until(() => d.core.thread(threadId)!.userQueue.length === 0 && !d.core.thread(threadId)!.activeTurn, "the queue delivered", 15000);
    const said = texts(await itemsOf(c, threadId), "agentMessage");
    assert.deepEqual(said, ["first", "one, edited", "three"]);
    const changes = c.events(threadId).filter((e) => e.event.type === "queueChanged").map((e) => (e.event as any).change);
    assert.deepEqual(changes, ["added", "added", "added", "updated", "removed", "delivered", "delivered"]);
    const users = (await itemsOf(c, threadId)).filter((i) => i.kind === "userMessage").length;
    assert.equal(users, 3, "a queued message appears as a user message only when delivered");
  } finally {
    await done();
  }
});

test("queued user messages survive a daemon restart and are delivered with no client attached", async () => {
  const home = tmpHome("y11-");
  let s = await setup({}, home);
  try {
    const { threadId } = await startThread(s.c, "/fake stall");
    await until(() => s.d.core.thread(threadId)!.activeTurn, "running");
    await s.c.request("turn/start", { opId: opId(), threadId, input: text("/fake say after restart"), delivery: "queue" });
    await s.done(true);
    // No client: the turn that was running died with the daemon; the queued message goes.
    const d = await startDaemon(home, { mesh: MESH_OPTS });
    await until(() => fakeOf(d).received.some((r) => r.input.text === "/fake say after restart"), "delivered after the restart");
    await untilIdle(d, threadId);
    assert.equal(d.core.thread(threadId)!.userQueue.length, 0);
    const c = await client(d);
    s = { d, c, home, done: async () => { await c.close(); await d.stop("test"); fs.rmSync(home, { recursive: true, force: true }); } };
    assert.ok(texts(await itemsOf(c, threadId), "agentMessage").includes("after restart"));
  } finally {
    await s.done();
  }
});

test("now from the composer: steered where the harness can steer, interrupt-and-start where it cannot; only queue while a request waits", async () => {
  const { d, c, done } = await setup();
  try {
    const nosteer = await addFake(c, "nosteer", { FAKE_STEER: "0" });
    const s1 = await startThread(c, "/fake sleep 1200\n/fake say done");
    const s2 = await startThread(c, "/fake stall", { instance: nosteer });
    await until(() => d.core.thread(s1.threadId)!.activeTurn && d.core.thread(s2.threadId)!.activeTurn, "running");
    const r1: any = await c.request("turn/start", { opId: opId(), threadId: s1.threadId, input: text("also check the docs"), delivery: "now" });
    assert.equal(r1.outcome, "steered");
    assert.equal(r1.turnId, s1.turnId);
    const r2: any = await c.request("turn/start", { opId: opId(), threadId: s2.threadId, input: text("/fake say instead"), delivery: "now" });
    assert.equal(r2.outcome, "interruptedAndStarted");
    assert.notEqual(r2.turnId, s2.turnId);
    await untilIdle(d, s1.threadId);
    await untilIdle(d, s2.threadId);
    assert.ok(texts(await itemsOf(c, s1.threadId), "agentMessage").includes("Steered: also check the docs"));
    assert.ok(texts(await itemsOf(c, s2.threadId), "agentMessage").includes("instead"));
    // A pending approval: only queue.
    const s3 = await startThread(c, "/fake approve ls");
    await until(() => d.core.thread(s3.threadId)!.pendingRequestCount() > 0, "the approval");
    await assert.rejects(c.request("turn/start", { opId: opId(), threadId: s3.threadId, input: text("go"), delivery: "now" }), (e: RpcFailure) => e.code === "awaiting_user");
    const q: any = await c.request("turn/start", { opId: opId(), threadId: s3.threadId, input: text("/fake say queued"), delivery: "queue" });
    assert.equal(q.outcome, "queued");
    await assert.rejects(c.request("turn/queuePromote", { opId: opId(), threadId: s3.threadId, queueId: q.queueId }), (e: RpcFailure) => e.code === "awaiting_user");
    assert.equal(d.core.thread(s3.threadId)!.userQueue.length, 1, "still queued");
  } finally {
    await done();
  }
});

test("the user message of a send carries its op id: started, steered, interrupted-and-started, and queued then delivered (D165)", async () => {
  const { d, c, done } = await setup();
  try {
    const nosteer = await addFake(c, "nosteer", { FAKE_STEER: "0" });
    const { threadId } = await startThread(c, "/fake say hi");
    await untilIdle(d, threadId);
    const opOf = async (t: string, said: string) => ((await itemsOf(c, t)).find((i: any) => i.kind === "userMessage" && i.parts?.[0]?.text === said) as any)?.opId;
    const started = opId();
    await c.request("turn/start", { opId: started, threadId, input: text("/fake sleep 900\n/fake say started") });
    await until(() => d.core.thread(threadId)!.activeTurn, "running");
    assert.equal(await opOf(threadId, "/fake sleep 900\n/fake say started"), started);
    const steered = opId();
    const r: any = await c.request("turn/start", { opId: steered, threadId, input: text("and the docs"), delivery: "now" });
    assert.equal(r.outcome, "steered");
    assert.equal(await opOf(threadId, "and the docs"), steered);
    const queued = opId();
    await c.request("turn/start", { opId: queued, threadId, input: text("/fake say later"), delivery: "queue" });
    await until(() => d.core.thread(threadId)!.userQueue.length === 0 && !d.core.thread(threadId)!.activeTurn, "the queue delivered", 15000);
    assert.equal(await opOf(threadId, "/fake say later"), queued, "a queued message keeps the op id that queued it");
    const sub: any = await c.request("thread/subscribe", { threadId });
    assert.ok((sub.queued ?? []).every((q: any) => q.opId === undefined), "the op id is not part of the queue's public entry");
    const s2 = await startThread(c, "/fake stall", { instance: nosteer });
    await until(() => d.core.thread(s2.threadId)!.activeTurn, "running");
    const now = opId();
    const r2: any = await c.request("turn/start", { opId: now, threadId: s2.threadId, input: text("/fake say instead"), delivery: "now" });
    assert.equal(r2.outcome, "interruptedAndStarted");
    assert.equal(await opOf(s2.threadId, "/fake say instead"), now);
    // The first message, sent with `thread/start`, carries that operation's id.
    assert.equal(typeof (await opOf(threadId, "/fake say hi")), "string");
  } finally {
    await done();
  }
});

test("Send now on a queued message promotes it with the now mechanics and leaves the others queued, in order", async () => {
  const { d, c, done } = await setup();
  try {
    const nosteer = await addFake(c, "nosteer", { FAKE_STEER: "0" });
    const { threadId } = await startThread(c, "/fake stall", { instance: nosteer });
    await until(() => d.core.thread(threadId)!.activeTurn, "running");
    const q1: any = await c.request("turn/start", { opId: opId(), threadId, input: text("/fake say one"), delivery: "queue" });
    const q2: any = await c.request("turn/start", { opId: opId(), threadId, input: text("/fake sleep 300\n/fake say two"), delivery: "queue" });
    const q3: any = await c.request("turn/start", { opId: opId(), threadId, input: text("/fake say three"), delivery: "queue" });
    const p: any = await c.request("turn/queuePromote", { opId: opId(), threadId, queueId: q2.queueId });
    assert.equal(p.outcome, "interruptedAndStarted");
    assert.deepEqual(d.core.thread(threadId)!.userQueue.map((q) => q.queueId), [q1.queueId, q3.queueId]);
    await until(() => d.core.thread(threadId)!.userQueue.length === 0 && !d.core.thread(threadId)!.activeTurn, "all delivered", 15000);
    assert.deepEqual(texts(await itemsOf(c, threadId), "agentMessage"), ["two", "one", "three"]);
    // Promoting on an idle thread just starts it.
    const idleQ = d.core.thread(threadId)!;
    assert.equal(idleQ.activeTurn, null);
  } finally {
    await done();
  }
});

test("a user's queued message goes ahead of queued agent messages", async () => {
  const { d, c, done } = await setup();
  try {
    const a = await idle(d, c);
    const { threadId: b } = await startThread(c, "/fake sleep 600\n/fake say busy", { agents: "allow" });
    await until(() => d.core.thread(b)!.activeTurn, "running");
    await call(d, a, "send_message", { to: addressOf(d, b), text: "/fake say from agent" });
    await c.request("turn/start", { opId: opId(), threadId: b, input: text("/fake say from user"), delivery: "queue" });
    await until(async () => texts(await itemsOf(c, b), "agentMessage").includes("from agent") && !d.core.thread(b)!.activeTurn, "both delivered", 15000);
    assert.deepEqual(texts(await itemsOf(c, b), "agentMessage"), ["busy", "from user", "from agent"]);
  } finally {
    await done();
  }
});

test("the fake provider's /fake edit reports a file change as a unified diff", async () => {
  const { d, c, done } = await setup();
  try {
    const t = await idle(d, c, "/fake edit src/main.rs");
    const edit = (await itemsOf(c, t)).find((i: any) => i.kind === "toolCall" && i.tool === "fileChange") as any;
    assert.equal(edit.title, "Edited src/main.rs");
    assert.match(edit.output, /^--- a\/src\/main\.rs\n\+\+\+ b\/src\/main\.rs\n@@ -1,4 \+1,5 @@\n/);
  } finally {
    await done();
  }
});
