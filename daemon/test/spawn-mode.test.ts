// A spawned worker inherits its parent's permission mode (D146; docs/10-agent-mesh.md#spawning):
// the same mode on the same instance, the equivalent one on another provider (mapped by
// `skipsApprovals` and `autonomy`), never anything more permissive, else the harness default
// with a note. Two fake instances with different mode sets stand in for two providers.

import { test } from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as path from "node:path";
import { client, FAKE, fakeOf, startDaemon, startThread, text, tmpHome, until, type TestClient } from "./helpers.js";
import { childOf, kinds, untilIdle } from "./mesh-helpers.js";
import type { Daemon } from "../src/session/daemon.js";
import type { ModeInfo } from "../src/protocol/generated/types.js";
import { chooseWorkerMode, ModeRefused, rankOf } from "../src/mesh/modes.js";

const OTHER = "pi_beta";

/** "Alpha": its never-ask mode says only `skipsApprovals` (no `autonomy`). */
const ALPHA: ModeInfo[] = [
  { id: "a-plan", displayName: "Alpha plan", autonomy: "readOnly" },
  { id: "a-ask", displayName: "Alpha ask", isDefault: true, autonomy: "ask" },
  { id: "a-edits", displayName: "Alpha edits", autonomy: "edits" },
  { id: "a-bypass", displayName: "Alpha bypass", skipsApprovals: true },
];
/** "Beta": nothing as restrictive as `readOnly`, and one mode that says nothing about itself. */
const BETA: ModeInfo[] = [
  { id: "b-ask", displayName: "Beta ask", isDefault: true, autonomy: "ask" },
  { id: "b-work", displayName: "Beta workspace", autonomy: "workspace" },
  { id: "b-yolo", displayName: "Beta full access", skipsApprovals: true, autonomy: "full" },
  { id: "b-mystery", displayName: "Beta mystery" },
];

async function setup(): Promise<{ d: Daemon; c: TestClient; done: () => Promise<void> }> {
  const home = tmpHome("ysm-");
  fs.writeFileSync(
    path.join(home, "instances.json"),
    JSON.stringify({
      instances: [
        { id: FAKE, kind: "fake", enabled: true, backend: "auto", implicit: true, env: { FAKE_MODES: JSON.stringify(ALPHA) } },
        { id: OTHER, kind: "fake", name: "beta", enabled: true, backend: "auto", env: { FAKE_SEED: "0", FAKE_MODES: JSON.stringify(BETA) } },
      ],
    }),
  );
  const d = await startDaemon(home, { mesh: { waitMaxMs: 3000, permissionWaitMs: 4000, remotePollMs: 200, tickMs: 0 } });
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

/** An idle parent thread in `mode`, allowed to work with agents. */
async function parentIn(d: Daemon, c: TestClient, mode: string): Promise<string> {
  const { threadId } = await startThread(c, "/fake say ready", { agents: "allow", mode });
  await untilIdle(d, threadId);
  return threadId;
}

async function spawn(d: Daemon, parent: string, args: Record<string, unknown>) {
  const native = d.core.thread(parent)!.record.native;
  return fakeOf(d).meshAttachment(native)!.call({ tool: "spawn_thread", args: { text: "/fake say working", ...args }, meta: { threadId: native } });
}

async function modeOf(d: Daemon, threadId: string): Promise<string | null | undefined> {
  return (await d.core.computeEffective(d.core.thread(threadId)!)).applied?.mode;
}

async function noticeOf(c: TestClient, threadId: string): Promise<string> {
  const n = (await kinds(c, threadId, "notice")).find((i) => String(i.id).startsWith("notice:mode:"));
  assert.ok(n, "the worker's transcript says which mode it runs in");
  assert.equal(n.noticeKind, "info");
  return String(n.text);
}

test("same instance: the worker runs in its parent's mode, and both the result and its transcript say so", async () => {
  const { d, c, done } = await setup();
  try {
    const parent = await parentIn(d, c, "a-bypass");
    const r = await spawn(d, parent, {});
    assert.equal(r.isError, false, r.text);
    assert.match(r.text, /Runs in Alpha bypass, as its parent\./);
    const child = await childOf(d, parent);
    assert.equal(await modeOf(d, child), "a-bypass");
    assert.equal(await noticeOf(c, child), "This worker runs in Alpha bypass, as its parent.");
  } finally {
    await done();
  }
});

test("another provider: a parent that never asks maps to that provider's never-ask mode, by the flag alone", async () => {
  const { d, c, done } = await setup();
  try {
    const parent = await parentIn(d, c, "a-bypass");
    const r = await spawn(d, parent, { instance: OTHER });
    assert.equal(r.isError, false, r.text);
    assert.match(r.text, /Runs in Beta full access, as its parent \(which runs in Alpha bypass\)\./);
    const child = await childOf(d, parent);
    assert.equal(d.core.thread(child)!.record.instance, OTHER);
    assert.equal(await modeOf(d, child), "b-yolo");
  } finally {
    await done();
  }
});

test("another provider: never more permissive — the closest mode at or below the parent's", async () => {
  const { d, c, done } = await setup();
  try {
    const parent = await parentIn(d, c, "a-edits");
    const r = await spawn(d, parent, { instance: OTHER });
    assert.equal(r.isError, false, r.text);
    assert.match(r.text, /Runs in Beta ask: the closest .* mode to its parent's Alpha edits that is no more permissive\./);
    assert.equal(await modeOf(d, await childOf(d, parent)), "b-ask", "not Beta workspace, which allows more than edits");
  } finally {
    await done();
  }
});

test("nothing as restrictive there: the harness default, and the note says so", async () => {
  const { d, c, done } = await setup();
  try {
    const parent = await parentIn(d, c, "a-plan");
    const r = await spawn(d, parent, { instance: OTHER });
    assert.equal(r.isError, false, r.text);
    assert.match(r.text, /Runs in Beta ask, .*default mode: .*has no mode as restrictive as its parent's \(Alpha plan\)\./);
    const child = await childOf(d, parent);
    assert.equal(await modeOf(d, child), "b-ask");
    const input = fakeOf(d, OTHER).received.find((x) => x.thread === d.core.thread(child)!.record.native);
    assert.ok(input, "the worker started");
    assert.match(await noticeOf(c, child), /^This worker runs in Beta ask, .*default mode: .*no mode as restrictive/);
  } finally {
    await done();
  }
});

test("an explicit mode is allowed only up to the parent's permissiveness", async () => {
  const { d, c, done } = await setup();
  try {
    const parent = await parentIn(d, c, "a-edits");
    const more = await spawn(d, parent, { instance: OTHER, mode: "b-work" });
    assert.equal(more.isError, true);
    assert.match(more.text, /Beta workspace is more permissive than your own mode \(Alpha edits\).*Allowed: b-ask/);
    const unknown = await spawn(d, parent, { instance: OTHER, mode: "b-mystery" });
    assert.equal(unknown.isError, true);
    assert.match(unknown.text, /does not say how permissive Beta mystery is/);
    const missing = await spawn(d, parent, { instance: OTHER, mode: "nope" });
    assert.equal(missing.isError, true);
    assert.match(missing.text, /has no mode "nope"\. Its modes: b-ask/);
    const bypass = await spawn(d, parent, { mode: "Alpha bypass" });
    assert.equal(bypass.isError, true, "the same instance is capped too");
    assert.equal(d.mesh.store.peekThread(parent)?.children.length ?? 0, 0, "nothing was started");
    const less = await spawn(d, parent, { mode: "a-plan" });
    assert.equal(less.isError, false, less.text);
    assert.match(less.text, /Runs in Alpha plan, as asked \(no more permissive than its parent's Alpha edits\)\./);
    assert.equal(await modeOf(d, await childOf(d, parent)), "a-plan");
  } finally {
    await done();
  }
});

test("a worker keeps its mode when its parent's changes later", async () => {
  const { d, c, done } = await setup();
  try {
    const parent = await parentIn(d, c, "a-edits");
    assert.equal((await spawn(d, parent, {})).isError, false);
    const child = await childOf(d, parent);
    await c.request("turn/start", { opId: "op-later", threadId: parent, mode: "a-bypass", input: text("/fake say now bypass") });
    await until(async () => (await modeOf(d, parent)) === "a-bypass", "the parent in its new mode");
    await untilIdle(d, parent);
    assert.equal(await modeOf(d, child), "a-edits");
  } finally {
    await done();
  }
});

test("list_models lists each instance's modes, the never-ask one marked", async () => {
  const { d, c, done } = await setup();
  try {
    const parent = await parentIn(d, c, "a-ask");
    const native = d.core.thread(parent)!.record.native;
    const r = await fakeOf(d).meshAttachment(native)!.call({ tool: "list_models", args: {}, meta: { threadId: native } });
    assert.match(r.text, /modes \(for every model of this instance\): a-plan "Alpha plan", a-ask "Alpha ask" \(default\), a-edits "Alpha edits", a-bypass "Alpha bypass" \(never asks for approval\)/);
    assert.match(r.text, /b-yolo "Beta full access" \(never asks for approval\)/);
  } finally {
    await done();
  }
});

// ── The rule itself, without a daemon ──────────────────────────────────────────────────────

test("the mapping: unknown never compares, the parent's id wins a tie, and a mode-less provider gets its default", () => {
  assert.equal(rankOf({ id: "x", displayName: "x" }), null);
  assert.equal(rankOf({ id: "x", displayName: "x", skipsApprovals: true, autonomy: "ask" }), 5, "the flag outranks a lower autonomy");
  const beta = { instance: OTHER, displayName: "Beta", modes: BETA };
  const unknownParent = chooseWorkerMode({ instance: FAKE, id: "a-?", info: { id: "a-?", displayName: "Alpha ?" } }, beta, null);
  assert.equal(unknownParent.mode, null);
  assert.match(unknownParent.note, /its parent's mode \(Alpha \?\) cannot be compared/);
  const tie = chooseWorkerMode(
    { instance: FAKE, id: "dup", info: { id: "dup", displayName: "Dup", autonomy: "ask" } },
    { instance: OTHER, displayName: "Twin", modes: [{ id: "first", displayName: "First", autonomy: "ask" }, { id: "dup", displayName: "Dup", autonomy: "ask" }] },
    null,
  );
  assert.deepEqual(tie, { mode: "dup", note: "Runs in Dup, as its parent." });
  const none = chooseWorkerMode({ instance: FAKE, id: "a-ask", info: ALPHA[1]! }, { instance: OTHER, displayName: "Bare", modes: [] }, null);
  assert.deepEqual(none, { mode: null, note: "Runs in Bare's default mode: Bare offers no modes to choose from." });
  assert.throws(() => chooseWorkerMode({ instance: FAKE, id: null, info: null }, beta, "b-ask"), ModeRefused, "a parent whose mode is unknown cannot hand out a mode");
});
