// Codex voice input (src/providers/codex/voice.ts) against the fake app-server: no network,
// no real Codex. The realtime notifications are the shapes of codex-cli 0.172.0's
// `generate-ts --experimental` (ThreadRealtimeTranscriptDeltaNotification, …).

import { test } from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { CodexProvider } from "../src/providers/codex/index.js";
import type { Exec } from "../src/providers/codex/backend.js";
import type { ProviderEvent } from "../src/providers/types.js";
import type { Dictation } from "../src/voice/types.js";
import { FakeCodex } from "./fixtures/codex/fake-codex.js";

const USER_THREAD = "01a1fa3e-0061-7e63-844f-8fa25836991b";
const VOICE_THREAD = "01a1fa3e-7777-7e63-844f-voice0000001";

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

interface Rig {
  fake: FakeCodex;
  provider: CodexProvider;
  events: ProviderEvent[];
  stateDir: string;
  done(): Promise<void>;
}

async function rig(opts: { account?: string; realtime?: boolean; env?: Record<string, string> } = {}): Promise<Rig> {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "yaac-cxv-"));
  const fake = await FakeCodex.start(path.join(dir, "s.sock"));
  const stateDir = path.join(dir, "state");
  const provider = new CodexProvider(
    { config: { id: "pi_codex_test", kind: "codex", enabled: true, backend: "auto" }, stateDir, home: dir, env: {}, log: () => {} },
    { socket: fake.socket, exec: failExec, reconnectMinMs: 20, reconnectMaxMs: 50, baseEnv: opts.env ?? {} },
  );
  fake.handlers.set("account/read", () => ({ account: { type: opts.account ?? "apiKey" }, requiresOpenaiAuth: true }));
  fake.handlers.set("model/list", () => ({ data: [], nextCursor: null }));
  if (opts.realtime !== false) fake.handlers.set("thread/realtime/listVoices", () => ({ voices: { v1: [], v2: [], defaultV1: "x", defaultV2: "y" } }));
  fake.handlers.set("thread/read", (p) => ({ thread: { id: p.threadId, status: { type: "idle" }, cwd: "/work" } }));
  fake.handlers.set("thread/start", () => ({ thread: { id: VOICE_THREAD, ephemeral: true, status: { type: "idle" }, cwd: stateDir, threadSource: "yaac.voice-input" } }));
  fake.handlers.set("thread/realtime/start", () => ({}));
  fake.handlers.set("thread/realtime/appendAudio", () => ({}));
  fake.handlers.set("thread/realtime/stop", () => ({}));
  fake.handlers.set("thread/unsubscribe", () => ({ status: "unsubscribed" }));
  const events: ProviderEvent[] = [];
  provider.events.on((e) => events.push(e));
  return {
    fake, provider, events, stateDir,
    async done() {
      await provider.dispose();
      await fake.close();
      fs.rmSync(dir, { recursive: true, force: true });
    },
  };
}

function recorder() {
  const transcripts: Array<{ text: string; final: boolean }> = [];
  const ended: Array<string | undefined> = [];
  return { transcripts, ended, events: { transcript: (text: string, final: boolean) => transcripts.push({ text, final }), ended: (e?: string) => ended.push(e) } };
}

function push(fake: FakeCodex, method: string, params: Record<string, unknown>): void {
  fake.push({ method, params: { threadId: VOICE_THREAD, ...params } });
}

test("voiceInput is offered only when the realtime methods answer and a key is available", async () => {
  const cases: Array<{ name: string; opts: Parameters<typeof rig>[0]; offered: boolean }> = [
    { name: "api key", opts: {}, offered: true },
    { name: "no realtime methods", opts: { realtime: false }, offered: false },
    { name: "ChatGPT sign-in", opts: { account: "chatgpt" }, offered: false },
    { name: "ChatGPT sign-in with OPENAI_API_KEY", opts: { account: "chatgpt", env: { OPENAI_API_KEY: "k" } }, offered: true },
  ];
  for (const c of cases) {
    const r = await rig(c.opts);
    try {
      const probe = await r.provider.probe();
      assert.equal(probe.capabilities.includes("voiceInput"), c.offered, c.name);
      const eff = await r.provider.effective(USER_THREAD);
      assert.equal(!!eff.capabilities?.includes("voiceInput"), c.offered, c.name);
      if (c.offered) assert.deepEqual(eff.voiceInput, { format: "pcm16le", sampleRate: 24000, channels: 1 });
      else {
        assert.equal(eff.voiceInput, undefined);
        assert.throws(() => r.provider.startDictation(USER_THREAD, {}, recorder().events), /not available/);
      }
    } finally {
      await r.done();
    }
  }
});

test("a dictation runs on a private transcription-only thread and never touches the user's thread", async () => {
  const r = await rig();
  try {
    await r.provider.probe();
    const rec = recorder();
    const d: Dictation = r.provider.startDictation(USER_THREAD, { language: "pt-BR" }, rec.events);
    // Three bytes: one whole sample now, the odd byte waits for the next chunk.
    await d.audio(Buffer.from([1, 0, 2]));
    await d.audio(Buffer.from([0]));

    const [start] = r.fake.requests("thread/start");
    assert.equal(start!.params.ephemeral, true);
    assert.equal(start!.params.cwd, r.stateDir);
    assert.equal(start!.params.threadSource, "yaac.voice-input");
    assert.equal(start!.params.model, undefined);
    assert.equal(start!.params.config["realtime.type"], "transcription");
    assert.equal(start!.params.config["realtime.version"], "v2");
    assert.equal(start!.params.config["realtime.transport"], "websocket");

    const [rt] = r.fake.requests("thread/realtime/start");
    assert.equal(rt!.params.threadId, VOICE_THREAD);
    assert.equal(rt!.params.outputModality, "text");
    assert.deepEqual(rt!.params.transport, { type: "websocket" });
    assert.equal(rt!.params.version, "v2");
    assert.equal(rt!.params.clientManagedHandoffs, true);
    assert.equal(rt!.params.includeStartupContext, false);
    assert.equal(rt!.params.model, undefined);

    const audio = r.fake.requests("thread/realtime/appendAudio").map((m) => m.params);
    assert.equal(audio.length, 2);
    assert.deepEqual(audio.map((a) => Buffer.from(a.audio.data, "base64")), [Buffer.from([1, 0]), Buffer.from([2, 0])]);
    assert.deepEqual(audio.map((a) => [a.audio.sampleRate, a.audio.numChannels, a.audio.samplesPerChannel]), [[24000, 1, 1], [24000, 1, 1]]);

    push(r.fake, "thread/realtime/started", { realtimeSessionId: "s", version: "v2" });
    push(r.fake, "thread/realtime/transcript/delta", { role: "user", delta: "olá" });
    push(r.fake, "thread/realtime/transcript/delta", { role: "user", delta: " mundo" });
    await until(() => rec.transcripts.length === 2);
    push(r.fake, "thread/realtime/transcript/done", { role: "user", text: "Olá, mundo." });
    push(r.fake, "thread/realtime/transcript/delta", { role: "user", delta: "segunda" });
    await until(() => rec.transcripts.length === 4);
    assert.deepEqual(rec.transcripts.map((t) => t.text), ["olá", "olá mundo", "Olá, mundo.", "Olá, mundo. segunda"]);
    assert.ok(rec.transcripts.every((t) => !t.final));

    d.stop();
    // Stop appends silence so the server's turn detection closes the last utterance.
    await until(() => r.fake.requests("thread/realtime/appendAudio").length === 3);
    const silence = Buffer.from(r.fake.requests("thread/realtime/appendAudio")[2]!.params.audio.data, "base64");
    assert.ok(silence.length > 0 && silence.every((b) => b === 0));
    push(r.fake, "thread/realtime/transcript/done", { role: "user", text: "Segunda frase." });
    await until(() => rec.ended.length === 1);
    assert.deepEqual(rec.ended, [undefined]);
    assert.deepEqual(rec.transcripts.at(-1), { text: "Olá, mundo. Segunda frase.", final: true });
    assert.equal(rec.transcripts.filter((t) => t.final).length, 1);
    await until(() => r.fake.requests("thread/unsubscribe").length === 1);
    assert.equal(r.fake.requests("thread/realtime/stop").length, 1);

    // Nothing reached the user's thread, and the helper thread was never reported.
    for (const m of r.fake.received) {
      if (m.params && typeof m.params === "object") assert.notEqual(m.params.threadId, USER_THREAD, m.method);
    }
    assert.equal(r.fake.requests("turn/start").length, 0);
    r.fake.push({ method: "thread/started", params: { thread: { id: VOICE_THREAD, ephemeral: true, cwd: r.stateDir, threadSource: "yaac.voice-input" } } });
    await new Promise((res) => setTimeout(res, 50));
    assert.equal(r.events.filter((e) => e.type === "threadChanged").length, 0);
  } finally {
    await r.done();
  }
});

test("anything but the user's transcript ends the dictation with an error", async () => {
  for (const [method, params] of [
    ["thread/realtime/transcript/delta", { role: "assistant", delta: "Sure, I can" }],
    ["thread/realtime/outputAudio/delta", { audio: { data: "", sampleRate: 24000, numChannels: 1, samplesPerChannel: 0, itemId: null } }],
    ["turn/started", { turn: { id: "t", status: "inProgress" } }],
  ] as const) {
    const r = await rig();
    try {
      await r.provider.probe();
      const rec = recorder();
      const d = r.provider.startDictation(USER_THREAD, {}, rec.events);
      await d.audio(Buffer.alloc(4));
      push(r.fake, method, params);
      await until(() => rec.ended.length === 1);
      assert.match(rec.ended[0]!, /transcription-only/, method);
      await until(() => r.fake.requests("thread/realtime/stop").length === 1);
      d.stop();
      d.cancel();
      assert.equal(rec.ended.length, 1);
    } finally {
      await r.done();
    }
  }
});

test("a refused start, a realtime error, and a cancel each end once", async () => {
  // Codex 0.172.0 refuses the websocket transport without an API key.
  let r = await rig();
  try {
    await r.provider.probe();
    r.fake.handlers.set("thread/realtime/start", () => ({ __error: { code: -32603, message: "failed to start realtime conversation: realtime conversation requires API key auth" } }));
    const rec = recorder();
    const d = r.provider.startDictation(USER_THREAD, {}, rec.events);
    await d.audio(Buffer.alloc(2));
    await until(() => rec.ended.length === 1);
    assert.match(rec.ended[0]!, /API-key sign-in/);
    assert.equal(r.fake.requests("thread/realtime/appendAudio").length, 0);
    await until(() => r.fake.requests("thread/unsubscribe").length === 1);
  } finally {
    await r.done();
  }

  r = await rig();
  try {
    await r.provider.probe();
    const rec = recorder();
    const d = r.provider.startDictation(USER_THREAD, {}, rec.events);
    await d.audio(Buffer.alloc(2));
    push(r.fake, "thread/realtime/transcript/delta", { role: "user", delta: "half" });
    await until(() => rec.transcripts.length === 1);
    push(r.fake, "thread/realtime/error", { message: "boom" });
    await until(() => rec.ended.length === 1);
    assert.match(rec.ended[0]!, /boom/);
    assert.deepEqual(rec.transcripts.at(-1), { text: "half", final: true });
  } finally {
    await r.done();
  }

  r = await rig();
  try {
    await r.provider.probe();
    const rec = recorder();
    const d = r.provider.startDictation(USER_THREAD, {}, rec.events);
    await d.audio(Buffer.alloc(2));
    push(r.fake, "thread/realtime/transcript/delta", { role: "user", delta: "dropped" });
    await until(() => rec.transcripts.length === 1);
    d.cancel();
    d.stop();
    assert.deepEqual(rec.ended, [undefined]);
    assert.equal(rec.transcripts.filter((t) => t.final).length, 0);
    await until(() => r.fake.requests("thread/unsubscribe").length === 1);
    assert.equal(r.fake.requests("thread/realtime/stop").length, 1);
  } finally {
    await r.done();
  }
});
