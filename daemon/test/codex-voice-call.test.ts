// Codex voice call (src/providers/codex/voice-call.ts) against the fake app-server: no network,
// no media, no real Codex. Notification shapes are codex-cli 0.172.0's `generate-ts
// --experimental` (ThreadRealtimeSdpNotification, ThreadRealtimeTranscriptDeltaNotification, …).

import { test } from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { CodexProvider } from "../src/providers/codex/index.js";
import type { Exec } from "../src/providers/codex/backend.js";
import type { ProviderEvent } from "../src/providers/types.js";
import { FakeCodex } from "./fixtures/codex/fake-codex.js";

const THREAD = "01a1fa3e-0061-7e63-844f-8fa25836991b";
const OFFER = "v=0\r\no=- 1 2 IN IP4 127.0.0.1\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\nm=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\n";

const failExec: Exec = async (file, args) => {
  throw new Error(`unexpected exec: ${file} ${args.join(" ")}`);
};

async function until(pred: () => boolean, ms = 8000): Promise<void> {
  const end = Date.now() + ms;
  while (!pred()) {
    if (Date.now() > end) throw new Error("condition not met in time");
    await new Promise((r) => setTimeout(r, 5));
  }
}

async function rig(opts: { account?: string; realtime?: boolean; start?: () => unknown } = {}) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "yaac-cxc-"));
  const fake = await FakeCodex.start(path.join(dir, "s.sock"));
  const provider = new CodexProvider(
    { config: { id: "pi_codex_test", kind: "codex", enabled: true, backend: "auto" }, stateDir: path.join(dir, "state"), home: dir, env: {}, log: () => {} },
    { socket: fake.socket, exec: failExec, reconnectMinMs: 20, reconnectMaxMs: 50, baseEnv: {} },
  );
  fake.handlers.set("account/read", () => ({ account: { type: opts.account ?? "chatgpt" }, requiresOpenaiAuth: true }));
  fake.handlers.set("model/list", () => ({ data: [], nextCursor: null }));
  if (opts.realtime !== false) fake.handlers.set("thread/realtime/listVoices", () => ({ voices: { v1: [], v2: [], defaultV1: "x", defaultV2: "y" } }));
  fake.handlers.set("thread/read", (p) => ({ thread: { id: p.threadId, status: { type: "idle" }, cwd: "/work" } }));
  fake.handlers.set("thread/resume", (p) => ({ thread: { id: p.threadId, status: { type: "idle" }, cwd: "/work" }, model: "m" }));
  fake.handlers.set("thread/realtime/start", opts.start ?? (() => ({})));
  fake.handlers.set("thread/realtime/stop", () => {
    setTimeout(() => fake.push({ method: "thread/realtime/closed", params: { threadId: THREAD, reason: "requested" } }), 5);
    return {};
  });
  const events: ProviderEvent[] = [];
  provider.events.on((e) => events.push(e));
  return {
    fake, provider, events,
    async done() {
      await provider.dispose();
      await fake.close();
      fs.rmSync(dir, { recursive: true, force: true });
    },
  };
}

function recorder() {
  const answers: string[] = [];
  const lines: Array<{ role: string; text: string; final: boolean }> = [];
  const ended: Array<string | undefined> = [];
  return {
    answers, lines, ended,
    events: {
      answer: (sdp: string) => answers.push(sdp),
      transcript: (role: "user" | "assistant", text: string, final: boolean) => lines.push({ role, text, final }),
      ended: (e?: string) => ended.push(e),
    },
  };
}

function push(fake: FakeCodex, method: string, params: Record<string, unknown>): void {
  fake.push({ method, params: { threadId: THREAD, ...params } });
}

test("voiceCall is offered whenever the realtime methods answer, whatever the sign-in", async () => {
  for (const c of [{ account: "chatgpt", realtime: true }, { account: "apiKey", realtime: true }, { account: "chatgpt", realtime: false }]) {
    const r = await rig(c);
    try {
      const probe = await r.provider.probe();
      assert.equal(probe.capabilities.includes("voiceCall"), c.realtime, JSON.stringify(c));
      const eff = await r.provider.effective(THREAD);
      assert.equal(!!eff.capabilities?.includes("voiceCall"), c.realtime, JSON.stringify(c));
      assert.equal(!!eff.capabilities?.includes("voiceCallLanguage"), c.realtime, JSON.stringify(c));
      if (!c.realtime) assert.throws(() => r.provider.startVoiceCall(THREAD, OFFER, {}, recorder().events), /not available/);
    } finally {
      await r.done();
    }
  }
});

test("a call starts realtime over WebRTC on the user's thread, relays the answer and transcripts, and stops", async () => {
  const r = await rig();
  try {
    await r.provider.probe();
    const rec = recorder();
    const call = r.provider.startVoiceCall(THREAD, OFFER, {}, rec.events);
    await call.accepted;

    assert.equal(r.fake.requests("thread/resume").length, 1, "the thread is subscribed first");
    assert.equal(r.fake.requests("thread/start").length, 0, "no helper thread");
    const [rt] = r.fake.requests("thread/realtime/start");
    assert.equal(rt!.params.threadId, THREAD);
    assert.equal(rt!.params.outputModality, "audio");
    assert.deepEqual(rt!.params.transport, { type: "webrtc", sdp: OFFER });
    assert.equal(rt!.params.version, "v3");
    assert.equal(rt!.params.model, undefined, "Codex picks the model (invariant 3)");
    assert.equal(rt!.params.voice, undefined);
    assert.equal(rt!.params.initialItems, undefined, "no language asked: Codex's own default");

    push(r.fake, "thread/realtime/started", { realtimeSessionId: "s", version: "v3" });
    push(r.fake, "thread/realtime/sdp", { sdp: "v=answer\r\n" });
    push(r.fake, "thread/realtime/sdp", { sdp: "v=again\r\n" });
    push(r.fake, "thread/realtime/transcript/delta", { role: "user", delta: "run the" });
    push(r.fake, "thread/realtime/transcript/delta", { role: "user", delta: " tests" });
    push(r.fake, "thread/realtime/transcript/done", { role: "user", text: "Run the tests." });
    push(r.fake, "thread/realtime/transcript/delta", { role: "assistant", delta: "On it" });
    push(r.fake, "thread/realtime/transcript/delta", { threadId: "someone-else", role: "user", delta: "not ours" });
    await until(() => rec.lines.length === 4);
    assert.deepEqual(rec.answers, ["v=answer\r\n"], "the answer once");
    assert.deepEqual(rec.lines, [
      { role: "user", text: "run the", final: false },
      { role: "user", text: "run the tests", final: false },
      { role: "user", text: "Run the tests.", final: true },
      { role: "assistant", text: "On it", final: false },
    ]);

    // The model hands work to the thread: an ordinary turn of the user's, not "in use elsewhere".
    push(r.fake, "turn/started", { turn: { id: "turn-call-1", status: "inProgress", items: [] } });
    await until(() => r.events.some((e) => e.type === "turnStarted"));
    const started = r.events.find((e) => e.type === "turnStarted") as Extract<ProviderEvent, { type: "turnStarted" }>;
    assert.equal(started.turnId, "turn-call-1");
    assert.equal(started.origin, "user");
    assert.equal((await r.provider.control(THREAD)).external, false);

    call.end();
    await until(() => rec.ended.length === 1);
    assert.deepEqual(rec.ended, [undefined]);
    assert.equal(rec.lines.at(-1)!.final, true, "the assistant's open utterance is closed");
    assert.equal(r.fake.requests("thread/realtime/stop").length, 1);
    assert.equal(r.fake.requests("thread/realtime/stop")[0]!.params.threadId, THREAD);
  } finally {
    await r.done();
  }
});

test("Codex refusing the start is the start's error; an error during the call ends it with words", async () => {
  const refused = await rig({ start: () => ({ __error: { code: -32603, message: "AVAS realtime calls require conversational realtime" } }) });
  try {
    await refused.provider.probe();
    const rec = recorder();
    const call = refused.provider.startVoiceCall(THREAD, OFFER, {}, rec.events);
    await assert.rejects(call.accepted, (e: Error & { code?: string }) => e.code === "provider_unavailable" && /conversational/.test(e.message));
    assert.deepEqual(rec.ended, []);
  } finally {
    await refused.done();
  }
  const r = await rig();
  try {
    await r.provider.probe();
    const rec = recorder();
    const call = r.provider.startVoiceCall(THREAD, OFFER, {}, rec.events);
    await call.accepted;
    push(r.fake, "thread/realtime/error", { message: "sideband dropped" });
    await until(() => rec.ended.length === 1);
    assert.match(rec.ended[0]!, /sideband dropped/);
    // After the call a turn started elsewhere is foreign again.
    push(r.fake, "turn/started", { turn: { id: "turn-other", status: "inProgress", items: [] } });
    await until(() => r.events.some((e) => e.type === "turnStarted"));
    assert.equal((r.events.find((e) => e.type === "turnStarted") as { origin: string }).origin, "external");
  } finally {
    await r.done();
  }
});

test("Codex closing the session for its own reason ends the call with that reason", async () => {
  const r = await rig();
  try {
    await r.provider.probe();
    const rec = recorder();
    const call = r.provider.startVoiceCall(THREAD, OFFER, {}, rec.events);
    await call.accepted;
    push(r.fake, "thread/realtime/closed", { reason: "session_expired" });
    await until(() => rec.ended.length === 1);
    assert.match(rec.ended[0]!, /session_expired/);
  } finally {
    await r.done();
  }
});

test("a call language is one developer initial item beside Codex's own instructions", async () => {
  const r = await rig();
  try {
    await r.provider.probe();
    const call = r.provider.startVoiceCall(THREAD, OFFER, { language: "pt-BR" }, recorder().events);
    await call.accepted;
    const [rt] = r.fake.requests("thread/realtime/start");
    assert.equal(rt!.params.version, "v3", "initialItems are v3 only");
    const items = rt!.params.initialItems as Array<{ role: string; text: string }>;
    assert.equal(items.length, 1);
    assert.equal(items[0]!.role, "developer");
    assert.match(items[0]!.text, /^Speak with the user in Brazilian Portuguese \(pt-BR\)/);
    assert.match(items[0]!.text, /code identifiers/);
    // Codex's own prompt and the backing model's start instructions are left alone.
    assert.equal(rt!.params.prompt, undefined);
    assert.equal(rt!.params.realtimeStartInstructions, undefined);
    assert.equal(rt!.params.model, undefined, "invariant 3");
    call.end();
  } finally {
    await r.done();
  }
});
