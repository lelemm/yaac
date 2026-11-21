// Fast mode (D125): a per-model capability, a thread setting that sticks for its next turns,
// and the flag a provider maps to its harness's own option. Against the fake provider, and
// the Codex provider against a fake app-server.

import { test } from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { FAKE, RpcFailure, client, fakeOf, opId, startDaemon, startThread, text, tmpHome, waitTurnDone } from "./helpers.js";
import { CodexProvider } from "../src/providers/codex/index.js";
import { fastTier, mapModel } from "../src/providers/codex/mapping.js";
import type { Exec } from "../src/providers/codex/backend.js";
import type { ProviderInput } from "../src/providers/types.js";
import { FakeCodex } from "./fixtures/codex/fake-codex.js";

const summaryOf = (d: Awaited<ReturnType<typeof startDaemon>>, threadId: string) => d.core.summaryOf(d.core.thread(threadId)!);

test("fast mode: on the model descriptor, carried by thread/start and turn/start, sticky, kept across a restart", async () => {
  const home = tmpHome();
  let daemon = await startDaemon(home);
  let c = await client(daemon);
  // The capability is on each model, as the provider reports it.
  const list: any = await c.request("provider/list");
  const fake = list.providers.find((p: any) => p.instance === FAKE);
  const support = (id: string) => fake.models.find((m: any) => m.id === id).capabilities?.find((x: any) => x.capability === "fastMode")?.support;
  assert.equal(support("fake-large"), "supported");
  assert.equal(support("fake-small"), "unsupported");
  assert.equal(support("fake-mystery"), undefined, "a model that reports nothing is not offered fast mode");

  // Started in fast mode: the flag reaches the provider, the summary says so, the thread's
  // effective capabilities include it.
  const { threadId, turnId } = await startThread(c, "/fake say quick", { model: "fake-large", fast: true });
  await c.request("thread/subscribe", { threadId, epoch: 1, afterSeq: 0 });
  assert.equal(fakeOf(daemon).received.at(-1)!.fast, true);
  assert.equal(summaryOf(daemon, threadId).fast, true);
  assert.ok((await daemon.core.computeEffective(daemon.core.thread(threadId)!)).capabilities!.includes("fastMode"));
  await waitTurnDone(c, threadId, turnId);

  // A turn that says nothing about it keeps it.
  const t2: any = await c.request("turn/start", { opId: opId(), threadId, input: text("/fake say again") });
  assert.equal(fakeOf(daemon).received.at(-1)!.fast, true);
  await waitTurnDone(c, threadId, t2.turnId);
  await c.close();
  await daemon.stop("test");

  // After a restart the thread is still in fast mode, and its next turn runs in it.
  daemon = await startDaemon(home);
  c = await client(daemon);
  await c.request("thread/subscribe", { threadId, epoch: daemon.core.position(threadId).epoch, afterSeq: 0 });
  assert.equal(summaryOf(daemon, threadId).fast, true);
  const t3: any = await c.request("turn/start", { opId: opId(), threadId, input: text("/fake say after the restart") });
  assert.equal(fakeOf(daemon).received.at(-1)!.fast, true);
  await waitTurnDone(c, threadId, t3.turnId);

  // Turned off: the provider is told, and it stays off.
  const t4: any = await c.request("turn/start", { opId: opId(), threadId, input: text("/fake say slower"), fast: false });
  assert.equal(fakeOf(daemon).received.at(-1)!.fast, false);
  assert.equal(summaryOf(daemon, threadId).fast, undefined);
  await waitTurnDone(c, threadId, t4.turnId);
  const t5: any = await c.request("turn/start", { opId: opId(), threadId, input: text("/fake say still slow") });
  assert.equal(fakeOf(daemon).received.at(-1)!.fast, false);
  await waitTurnDone(c, threadId, t5.turnId);

  // Asking for it on a model without it is refused, and nothing is sent.
  const before = fakeOf(daemon).received.length;
  await assert.rejects(
    c.request("turn/start", { opId: opId(), threadId, input: text("x"), model: "fake-small", fast: true }),
    (e: unknown) => e instanceof RpcFailure && e.code === "invalid_params",
  );
  await assert.rejects(c.request("turn/start", { opId: opId(), threadId, input: text("x"), fast: "yes" }), (e: unknown) => e instanceof RpcFailure && e.code === "invalid_params");
  assert.equal(fakeOf(daemon).received.length, before);

  // On again, then a switch to a model without fast mode: that turn runs without it and the
  // thread's setting goes off.
  const t6: any = await c.request("turn/start", { opId: opId(), threadId, input: text("/fake say fast"), fast: true });
  await waitTurnDone(c, threadId, t6.turnId);
  assert.equal(summaryOf(daemon, threadId).fast, true);
  const t7: any = await c.request("turn/start", { opId: opId(), threadId, input: text("/fake say small"), model: "fake-small" });
  assert.equal(fakeOf(daemon).received.at(-1)!.fast, false);
  assert.equal(summaryOf(daemon, threadId).fast, undefined);
  assert.ok(!(await daemon.core.computeEffective(daemon.core.thread(threadId)!)).capabilities!.includes("fastMode"));
  await waitTurnDone(c, threadId, t7.turnId);
  await c.close();
  await daemon.stop("test");
});

test("fast mode: a queued message carries its own setting, applied when it is delivered", async () => {
  const home = tmpHome();
  const daemon = await startDaemon(home);
  const c = await client(daemon);
  const { threadId, turnId } = await startThread(c, "/fake stall", { model: "fake-large" });
  await c.request("thread/subscribe", { threadId, epoch: 1, afterSeq: 0 });
  const q: any = await c.request("turn/start", { opId: opId(), threadId, input: text("/fake say queued"), fast: true, delivery: "queue" });
  assert.equal(q.outcome, "queued");
  assert.equal(summaryOf(daemon, threadId).fast, undefined, "not before it is delivered");
  await c.request("turn/interrupt", { threadId, turnId });
  await waitTurnDone(c, threadId, turnId);
  const delivered = await c.waitEvent((e) => e.threadId === threadId && e.event.type === "turnStarted" && (e.event as any).turnId !== turnId, "the queued turn");
  assert.equal(fakeOf(daemon).received.at(-1)!.fast, true);
  assert.equal(summaryOf(daemon, threadId).fast, true);
  await waitTurnDone(c, threadId, (delivered.event as any).turnId);
  await c.close();
  await daemon.stop("test");
});

// ── Codex: a model's "Fast" service tier ───────────────────────────────────────────────────

test("codex: a model's Fast service tier is its fastMode; no tier list is unknown", () => {
  // Shapes as Codex 0.171's model/list reports them.
  const fast = { id: "m-fast", model: "m-fast", displayName: "M", serviceTiers: [{ id: "priority", name: "Fast", description: "1.5x speed" }, { id: "ultrafast", name: "Ultrafast", description: "" }] };
  const none = { id: "m-none", model: "m-none", serviceTiers: [] };
  const old = { id: "m-old", model: "m-old" };
  const support = (m: any) => mapModel(m).capabilities!.find((c) => c.capability === "fastMode")!.support;
  assert.equal(fastTier(fast), "priority");
  assert.equal(support(fast), "supported");
  assert.equal(fastTier(none), null);
  assert.equal(support(none), "unsupported");
  assert.equal(support(old), "unknown");
  // The tier Codex names Fast wins over the id; an id alone is enough.
  assert.equal(fastTier({ id: "x", serviceTiers: [{ id: "speedy", name: "Fast" }, { id: "priority", name: "Other" }] } as any), "speedy");
  assert.equal(fastTier({ id: "x", serviceTiers: [{ id: "priority" }] } as any), "priority");
  assert.equal(fastTier({ id: "x", serviceTiers: [{ id: "flex", name: "Flex" }] } as any), null);
});

const failExec: Exec = async (file, args) => {
  throw new Error(`unexpected exec: ${file} ${args.join(" ")}`);
};

function input(t: string): ProviderInput {
  return { parts: [{ type: "text", text: t }], text: t, images: [], files: [], origin: "user" };
}

test("codex: fast mode becomes serviceTierForTurn — the fast tier when on, default when off, nothing for a model without one", async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "yaac-cx-fast-"));
  const fake = await FakeCodex.start(path.join(dir, "s.sock"));
  const provider = new CodexProvider(
    { config: { id: "pi_codex_fast", kind: "codex", enabled: true, backend: "auto" }, stateDir: path.join(dir, "state"), home: dir, env: {}, log: () => {} },
    { socket: fake.socket, exec: failExec, reconnectMinMs: 20, reconnectMaxMs: 50 },
  );
  try {
    fake.handlers.set("account/read", () => ({ account: { type: "apiKey" } }));
    fake.handlers.set("model/list", () => ({
      data: [
        { id: "m-fast", model: "m-fast", displayName: "M fast", isDefault: true, serviceTiers: [{ id: "priority", name: "Fast", description: "1.5x speed" }] },
        { id: "m-slow", model: "m-slow", displayName: "M slow", serviceTiers: [] },
      ],
      nextCursor: null,
    }));
    const probe = await provider.probe();
    assert.equal(probe.models.find((m) => m.id === "m-fast")!.capabilities!.find((c) => c.capability === "fastMode")!.support, "supported");
    const T = "01a1fa3e-0061-7e63-844f-8fa25836991b";
    fake.handlers.set("thread/start", () => ({ thread: { id: T, status: { type: "idle" }, cwd: "/w" }, model: "m-fast", reasoningEffort: null }));
    fake.handlers.set("thread/read", (p) => ({ thread: { id: p.threadId, status: { type: "idle" }, cwd: "/w" } }));
    let n = 0;
    fake.handlers.set("turn/start", () => ({ turn: { id: `t${++n}`, status: "inProgress" } }));
    const finish = (id: string) => fake.push({ method: "turn/completed", params: { threadId: T, turn: { id, status: "completed", items: [] } } });
    const tierOf = (i: number) => fake.requests("turn/start")[i]!.params.serviceTierForTurn;
    const settled = async (id: string) => {
      finish(id);
      await new Promise((r) => setTimeout(r, 50));
    };

    await provider.startThread({ cwd: "/w", fast: true }, input("go"));
    assert.equal(tierOf(0), "priority", "the default model's fast tier");
    assert.equal(fake.requests("thread/start")[0]!.params.serviceTier, undefined, "the thread's own tier is not touched");
    await settled("t1");
    await provider.startTurn(T, input("off"), { fast: false });
    assert.equal(tierOf(1), "default");
    await settled("t2");
    await provider.startTurn(T, input("unsaid"), {});
    assert.equal(tierOf(2), undefined, "no fast flag: nothing sent");
    await settled("t3");
    await provider.startTurn(T, input("slow model"), { model: "m-slow", fast: false });
    assert.equal(tierOf(3), undefined, "a model without a fast tier gets no tier");
    await settled("t4");
  } finally {
    await provider.dispose();
    await fake.close();
    fs.rmSync(dir, { recursive: true, force: true });
  }
});
