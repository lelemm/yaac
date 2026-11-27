// Voice input (docs/plan/voice.md): the shared stream module and Claude Code's `dictate`
// backend, against a fake Claude Code control channel. Nothing here records a microphone or
// reaches a speech-to-text service.

import { after, test } from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import type { Options, SDKMessage, SDKUserMessage } from "@anthropic-ai/claude-agent-sdk";
import { ClaudeProvider } from "../src/providers/claude/index.js";
import { CLAUDE_VOICE_FORMAT, ClaudeDictationHost, unavailableMessage } from "../src/providers/claude/dictation.js";
import type { ClaudeCli, ClaudeSdk, SdkQuery } from "../src/providers/claude/sdk.js";
import type { ConnectionView } from "../src/protocol/connection.js";
import { CreditExceeded } from "../src/protocol/scheduler.js";
import { VoiceSessions } from "../src/voice/sessions.js";
import { DictationRefused, type Dictation, type DictationEvents } from "../src/voice/types.js";
import { RpcError } from "../src/protocol/rpc.js";
import type WebSocket from "ws";

const tick = (ms = 5) => new Promise((r) => setTimeout(r, ms));
async function until(cond: () => boolean, what: string, ms = 3000): Promise<void> {
  const end = Date.now() + ms;
  while (!cond()) {
    if (Date.now() > end) throw new Error(`timed out waiting for ${what}`);
    await tick();
  }
}

// ── A fake Claude Code: the host-dictation handler of 2.1.330, in miniature ──────────────────

interface Behaviour {
  /** The feature flag: false answers as Claude Code does for a host it does not serve. */
  gate: boolean;
  /** `unavailable` reason for `start`, when set. */
  startReason?: string;
  /** Words per 6400 bytes of audio (one per 200 ms). */
  words: string[];
  /** Reply `done` once this many bytes arrived (the two-minute limit, scaled down). */
  limitBytes?: number;
}

class FakeQuery implements SdkQuery {
  readonly requests: Array<Record<string, unknown>> = [];
  closed = false;
  private bytes = 0;
  private started = false;
  private finished = "";
  private waiters: Array<() => void> = [];
  private crashed: Error | null = null;
  constructor(readonly options: Options, private readonly b: Behaviour) {}

  async request(req: Record<string, unknown>): Promise<{ response?: unknown }> {
    this.requests.push(req);
    await tick(1);
    if (this.crashed) throw this.crashed;
    if (req.subtype !== "dictate" || !this.b.gate || this.options.env?.CLAUDE_CODE_ENTRYPOINT !== "claude-vscode") {
      throw Object.assign(new Error(`Unsupported control request subtype: ${String(req.subtype)}`), { errorCode: "unsupported_request" });
    }
    switch (req.op) {
      case "cancel":
        this.started = false;
        return { response: { status: "ok", text: "", interim: "", done: true } };
      case "start":
        if (this.b.startReason) return { response: { status: "unavailable", reason: this.b.startReason, text: "" } };
        this.started = true;
        this.bytes = 0;
        this.finished = "";
        return { response: { status: "ok", text: "", interim: "", done: false } };
      case "audio": {
        if (!this.started) return { response: { status: "unavailable", reason: "not_started", text: "" } };
        const pcm = Buffer.from(String(req.pcm), "base64");
        assert.ok(pcm.length >= 1 && pcm.length <= 64000, `chunk of ${pcm.length} bytes`);
        this.bytes += pcm.length;
        const n = Math.floor(this.bytes / 6400);
        // Finished words lag one behind; the newest is the interim guess.
        this.finished = this.b.words.slice(0, Math.max(0, n - 1)).join(" ");
        const interim = n > 0 ? (this.b.words[n - 1] ?? "") : "";
        if (this.b.limitBytes && this.bytes >= this.b.limitBytes) {
          this.started = false;
          return { response: { status: "ok", text: this.b.words.slice(0, n).join(" "), interim: "", done: true } };
        }
        return { response: { status: "ok", text: this.finished, interim, done: false } };
      }
      case "stop": {
        if (!this.started) return { response: { status: "unavailable", reason: "not_started", text: "" } };
        this.started = false;
        const n = Math.floor(this.bytes / 6400);
        return { response: { status: "ok", text: this.b.words.slice(0, n).join(" "), interim: "", done: true } };
      }
    }
    throw new Error("dictate: op must be start, audio, stop or cancel");
  }

  crash(message: string): void {
    this.crashed = new Error(message);
    for (const w of this.waiters.splice(0)) w();
  }

  async *[Symbol.asyncIterator](): AsyncIterator<SDKMessage> {
    while (!this.closed && !this.crashed) await new Promise<void>((r) => this.waiters.push(r));
    if (this.crashed) throw this.crashed;
  }
  async interrupt() { return undefined; }
  async setModel() {}
  async setPermissionMode() {}
  async applyFlagSettings() {}
  async supportedModels() { return [{ value: "stub-model", displayName: "Stub", description: "invented" }]; }
  async accountInfo() { return { email: "someone@example.invalid", ...(this.apiProvider ? { apiProvider: this.apiProvider } : {}) } as never; }
  apiProvider?: string;
  async initializationResult() { return { pid: 1, claude_code_version: "stub-cc" }; }
  close(): void {
    this.closed = true;
    for (const w of this.waiters.splice(0)) w();
  }
}

class FakeSdk implements ClaudeSdk {
  readonly queries: FakeQuery[] = [];
  behaviour: Behaviour = { gate: true, words: ["one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten"] };
  apiProvider?: string;
  query(params: { prompt: AsyncIterable<SDKUserMessage>; options: Options }): SdkQuery {
    const q = new FakeQuery(params.options, this.behaviour);
    q.apiProvider = this.apiProvider;
    this.queries.push(q);
    return q;
  }
  dictationQueries(): FakeQuery[] {
    return this.queries.filter((q) => q.options.env?.CLAUDE_CODE_ENTRYPOINT === "claude-vscode");
  }
  async listSessions() { return []; }
  async listSubagents() { return []; }
  async getSubagentMessages() { return []; }
  async getSessionInfo() { return undefined; }
  async getSessionMessages() { return []; }
  async forkSession() { return { sessionId: "x" }; }
  async renameSession() {}
  async archiveSession() {}
  async unarchiveSession() {}
  async deleteSession() {}
}

const cli: ClaudeCli = {
  version: async () => "stub-version",
  authStatus: async () => ({ loggedIn: true, configDirectory: "/home/user/.claude" }),
  agents: async () => [],
} as unknown as ClaudeCli;

const tmpDirs: string[] = [];
after(() => {
  for (const d of tmpDirs) fs.rmSync(d, { recursive: true, force: true });
});
function tmp(): string {
  const d = fs.mkdtempSync(path.join(os.tmpdir(), "yaac-voice-test-"));
  tmpDirs.push(d);
  return d;
}

function host(sdk: FakeSdk, idleMs = 60_000): ClaudeDictationHost {
  return new ClaudeDictationHost({ sdk: async () => sdk, executable: () => "/nonexistent/claude", env: { HOME: "/home/user" }, cwd: tmp(), log: () => {}, idleMs });
}

class Recorder implements DictationEvents {
  readonly transcripts: Array<[string, boolean]> = [];
  endedWith: Array<string | undefined> = [];
  transcript(text: string, final: boolean) { this.transcripts.push([text, final]); }
  ended(error?: string) { this.endedWith.push(error); }
  get over() { return this.endedWith.length > 0; }
}

/** `seconds` of 16 kHz mono s16le silence, in 20 ms frames (640 bytes), as a microphone sends. */
function frames(seconds: number): Buffer[] {
  return Array.from({ length: Math.round(seconds * 50) }, () => Buffer.alloc(640));
}

// ── Claude provider: the capability ──────────────────────────────────────────────────────

function provider(sdk: FakeSdk, voice = true, accessToken?: () => Promise<string | null>, websocket?: typeof WebSocket) {
  return new ClaudeProvider({ sdk, cli, executable: "/nonexistent/claude", env: { HOME: "/home/user" }, stateDir: tmp(), idleMs: 0, controlCacheMs: 0, voice,
    voiceAccessToken: accessToken, ...(websocket ? { voiceStream: { websocket } } : {}) });
}

test("probe offers transcription when Claude Code answers dictate, in a separate VS Code-host process", async () => {
  const sdk = new FakeSdk();
  const p = provider(sdk);
  const probe = await p.probe();
  assert.ok(probe.capabilities.includes("transcription"));
  assert.deepEqual(probe.transcription, { format: CLAUDE_VOICE_FORMAT });
  const [dq, ...more] = sdk.dictationQueries();
  assert.ok(dq);
  assert.equal(more.length, 0);
  assert.deepEqual(dq.requests, [{ subtype: "dictate", op: "cancel" }], "the probe sends no audio and opens no dictation");
  assert.equal(dq.options.persistSession, false);
  assert.deepEqual(dq.options.tools, []);
  assert.equal(dq.options.strictMcpConfig, true);
  assert.deepEqual(dq.options.settings, { disableAllHooks: true });
  // The thread/model query keeps Claude Code's own entrypoint.
  assert.ok(sdk.queries.some((q) => q.options.env?.CLAUDE_CODE_ENTRYPOINT === undefined));
  // The instance's, not the thread's (P392).
  const eff = await p.effective("some-thread");
  assert.ok(!(eff.capabilities ?? []).includes("transcription"));
  await p.dispose();
  assert.ok(dq.closed, "the warm dictation process closes with the provider");
});

test("probe withholds transcription, with the reason, when the dictate request is refused", async () => {
  const sdk = new FakeSdk();
  sdk.behaviour.gate = false;
  const p = provider(sdk);
  const probe = await p.probe();
  assert.ok(!probe.capabilities.includes("transcription"));
  assert.equal(probe.transcription?.format, undefined);
  assert.match(probe.transcription?.unavailable ?? "", /dictation is not available here/);
  assert.ok(sdk.dictationQueries().every((q) => q.closed), "a refused probe leaves no process behind");
  await p.dispose();
});

test("Claude Code's OAuth token offers the direct fallback when dictate is gated off", async () => {
  const sdk = new FakeSdk();
  sdk.behaviour.gate = false;
  const p = provider(sdk, true, async () => "oauth-token-for-tests");
  const probe = await p.probe();
  assert.ok(probe.capabilities.includes("transcription"));
  assert.deepEqual(probe.transcription, { format: CLAUDE_VOICE_FORMAT });
  await p.dispose();
});

test("dictate remains first when both dictate and the direct OAuth session exist", async () => {
  const sdk = new FakeSdk();
  class MustNotOpen { constructor() { throw new Error("direct WebSocket was opened"); } }
  const p = provider(sdk, true, async () => "oauth-token-for-tests", MustNotOpen as unknown as typeof WebSocket);
  const probe = await p.probe();
  assert.ok(probe.capabilities.includes("transcription"));
  const ev = new Recorder();
  const d = p.startDictation({}, ev);
  await d.accepted;
  assert.equal(sdk.dictationQueries().at(-1)?.requests.at(-1)?.op, "start");
  d.cancel();
  await until(() => ev.over, "dictate cancellation");
  await p.dispose();
});

test("probe withholds transcription for an account that is not Anthropic's own", async () => {
  const sdk = new FakeSdk();
  sdk.apiProvider = "bedrock";
  const p = provider(sdk);
  const probe = await p.probe();
  assert.ok(!probe.capabilities.includes("transcription"));
  assert.match(probe.transcription?.unavailable ?? "", /claude\.ai sign-in/);
  await p.dispose();
});

test("probe does not look for dictation when voice is off", async () => {
  const sdk = new FakeSdk();
  const p = provider(sdk, false);
  const probe = await p.probe();
  assert.ok(!probe.capabilities.includes("transcription"));
  assert.equal(probe.transcription, undefined);
  assert.equal(sdk.dictationQueries().length, 0);
  await p.dispose();
});

// ── Claude dictation ─────────────────────────────────────────────────────────────────────

test("a dictation batches audio, reports the words, and ends with the final text on stop", async () => {
  const sdk = new FakeSdk();
  const h = host(sdk);
  const ev = new Recorder();
  const d = h.start({ language: "pt-BR" }, ev);
  const taken: Promise<void>[] = [];
  for (const f of frames(1.2)) {
    taken.push(d.audio(f));
    await tick(1);
  }
  await Promise.all(taken);
  d.stop();
  await until(() => ev.over, "ended");
  const q = sdk.dictationQueries()[0]!;
  const ops = q.requests.map((r) => r.op);
  assert.equal(ops[0], "start");
  assert.equal(q.requests[0]!.language, "pt-BR");
  assert.equal(ops.at(-1), "stop");
  const audio = q.requests.filter((r) => r.op === "audio").map((r) => Buffer.from(String(r.pcm), "base64").length);
  assert.equal(audio.reduce((a, b) => a + b, 0), 60 * 640, "every byte reaches Claude Code once");
  assert.ok(audio.every((n) => n % 2 === 0 && n <= 64000));
  assert.ok(audio.length < 60 / 2, `20 ms frames are batched (${audio.length} requests)`);
  assert.deepEqual(ev.transcripts.at(-1), ["one two three four five six", true]);
  assert.ok(ev.transcripts.slice(0, -1).every(([, final]) => !final));
  assert.ok(ev.transcripts.some(([t]) => t === "one"), "the interim guess is shown");
  assert.deepEqual(ev.endedWith, [undefined]);
  h.dispose();
});

test("stop right after start still sends the audio already given, then stop", async () => {
  const sdk = new FakeSdk();
  const h = host(sdk);
  const ev = new Recorder();
  const d = h.start({}, ev);
  void d.audio(Buffer.alloc(12800));
  d.stop();
  await until(() => ev.over, "ended");
  assert.deepEqual(sdk.dictationQueries()[0]!.requests.map((r) => r.op), ["start", "audio", "stop"]);
  assert.deepEqual(ev.transcripts.at(-1), ["one two", true]);
  h.dispose();
});

test("an unavailable start ends the dictation with words for the user", async () => {
  const sdk = new FakeSdk();
  sdk.behaviour.startReason = "not_signed_in";
  const h = host(sdk);
  const ev = new Recorder();
  const d = h.start({}, ev);
  await assert.rejects(d.accepted!, (e: unknown) => e instanceof DictationRefused && e.code === "needs_auth" && e.message === unavailableMessage("not_signed_in"));
  await until(() => ev.over, "ended");
  assert.deepEqual(ev.endedWith, [unavailableMessage("not_signed_in")]);
  assert.deepEqual(ev.transcripts, []);
  h.dispose();
});

test("a refused request (feature off) ends the dictation with an error", async () => {
  const sdk = new FakeSdk();
  sdk.behaviour.gate = false;
  const h = host(sdk);
  const ev = new Recorder();
  const d = h.start({}, ev);
  await assert.rejects(d.accepted!, (e: unknown) => e instanceof DictationRefused && e.code === "provider_unavailable");
  await until(() => ev.over, "ended");
  assert.match(ev.endedWith[0] ?? "", /Unsupported control request subtype: dictate/);
  h.dispose();
});

test("cancel ends at once, drops the words, and the process is reused for the next dictation", async () => {
  const sdk = new FakeSdk();
  const h = host(sdk);
  const ev = new Recorder();
  const d = h.start({}, ev);
  for (const f of frames(0.5)) void d.audio(f);
  await until(() => sdk.dictationQueries()[0]?.requests.some((r) => r.op === "audio") ?? false, "audio");
  d.cancel();
  assert.deepEqual(ev.endedWith, [undefined]);
  assert.ok(ev.transcripts.every(([, final]) => !final), "a cancelled dictation has no final text");
  await until(() => sdk.dictationQueries()[0]!.requests.at(-1)?.op === "cancel", "cancel request");
  await tick(10);
  const again = new Recorder();
  const d2 = h.start({}, again);
  void d2.audio(Buffer.alloc(6400));
  d2.stop();
  await until(() => again.over, "second ended");
  assert.equal(sdk.dictationQueries().length, 1, "the warm process took the second dictation");
  assert.deepEqual(again.transcripts.at(-1), ["one", true]);
  h.dispose();
  assert.ok(sdk.dictationQueries()[0]!.closed);
});

test("Claude Code's own limit ends the dictation with all its words", async () => {
  const sdk = new FakeSdk();
  sdk.behaviour.limitBytes = 19200;
  const h = host(sdk);
  const ev = new Recorder();
  const d = h.start({}, ev);
  for (const f of frames(1)) void d.audio(f);
  await until(() => ev.over, "ended");
  const [text, final] = ev.transcripts.at(-1)!;
  assert.match(text, /^one two three/);
  assert.equal(final, true);
  assert.deepEqual(ev.endedWith, [undefined]);
  await tick(20);
  const q = sdk.dictationQueries()[0]!;
  assert.ok(!q.requests.some((r) => r.op === "stop"), "no stop after Claude Code ended it");
  h.dispose();
});

test("a dictation process that dies ends the dictation, keeping the words it had", async () => {
  const sdk = new FakeSdk();
  const h = host(sdk);
  const ev = new Recorder();
  const d = h.start({}, ev);
  for (const f of frames(0.5)) void d.audio(f);
  await until(() => ev.transcripts.length > 0, "a transcript");
  sdk.dictationQueries()[0]!.crash("Claude Code exited (1)");
  for (const f of frames(0.3)) void d.audio(f);
  await until(() => ev.over, "ended");
  assert.match(ev.endedWith[0] ?? "", /Claude Code exited/);
  assert.equal(ev.transcripts.at(-1)![1], true);
  // The broken process is not kept for the next one.
  const next = new Recorder();
  h.start({}, next);
  await tick(20);
  assert.equal(sdk.dictationQueries().length, 2);
  h.dispose();
});

test("dispose ends running dictations", async () => {
  const sdk = new FakeSdk();
  const h = host(sdk);
  const ev = new Recorder();
  h.start({}, ev);
  await tick(10);
  h.dispose();
  assert.equal(ev.endedWith.length, 1);
  assert.ok(ev.endedWith[0]);
});

// ── The shared stream module ─────────────────────────────────────────────────────────────

class FakeView {
  readonly notes: Array<[string, Record<string, unknown>]> = [];
  readonly credit: Array<[number, number]> = [];
  notify(method: string, params: unknown) {
    this.notes.push([method, params as Record<string, unknown>]);
    return true;
  }
  grantCredit(stream: number, bytes: number) { this.credit.push([stream, bytes]); }
}

class ScriptedDictation implements Dictation {
  readonly got: Buffer[] = [];
  release: Array<() => void> = [];
  stopped = false;
  cancelled = false;
  constructor(readonly events: DictationEvents) {}
  audio(pcm: Buffer) {
    this.got.push(pcm);
    return new Promise<void>((r) => this.release.push(r));
  }
  stop() {
    this.stopped = true;
    this.events.transcript("hello world", true);
    this.events.ended();
  }
  cancel() {
    this.cancelled = true;
    this.events.ended();
  }
}

const FORMAT = { format: "pcm16le" as const, sampleRate: 16000, channels: 1 };

function sessions(graceMs?: number) {
  let next = 100;
  return new VoiceSessions(() => next++, graceMs);
}

test("voice/start answers before any notification, and voice/ended is the last one", async () => {
  const v = sessions();
  const view = new FakeView();
  let d!: ScriptedDictation;
  const r = await v.start(view as unknown as ConnectionView, "thread-1", FORMAT, (ev) => {
    d = new ScriptedDictation(ev);
    ev.transcript("early", false); // reported while the response is still on its way
    return d;
  });
  assert.equal(r.initialCredit, 16000 * 2 * 4);
  assert.deepEqual(r.format, FORMAT);
  assert.equal(view.notes.length, 0, "nothing before the response");
  await tick();
  assert.deepEqual(view.notes, [["voice/transcript", { stream: r.stream, text: "early", final: false }]]);
  v.stop(view as unknown as ConnectionView, r.stream);
  assert.ok(d.stopped);
  assert.deepEqual(view.notes.slice(1), [
    ["voice/transcript", { stream: r.stream, text: "hello world", final: true }],
    ["voice/ended", { stream: r.stream }],
  ]);
  assert.equal(v.size, 0);
  assert.throws(() => v.stop(view as unknown as ConnectionView, r.stream), /no such dictation/);
});

test("credit comes back as the harness takes audio, and over-credit is a protocol error", async () => {
  const v = sessions();
  const view = new FakeView();
  let d!: ScriptedDictation;
  const r = await v.start(view as unknown as ConnectionView, "t", FORMAT, (ev) => (d = new ScriptedDictation(ev)));
  const cv = view as unknown as ConnectionView;
  const half = r.initialCredit / 2;
  assert.equal(v.receive(cv, r.stream, Buffer.alloc(half)), true);
  assert.equal(v.receive(cv, r.stream + 1, Buffer.alloc(2)), false, "another stream is not voice");
  assert.equal(view.credit.length, 0, "no credit before the harness took the audio");
  d.release.shift()!();
  await tick();
  assert.deepEqual(view.credit, [[r.stream, half]]);
  assert.throws(() => v.receive(cv, r.stream, Buffer.alloc(r.initialCredit + 2)), CreditExceeded);
});

test("a dropped connection cancels its dictations without sending anything", async () => {
  const v = sessions();
  const view = new FakeView();
  let d!: ScriptedDictation;
  await v.start(view as unknown as ConnectionView, "t", FORMAT, (ev) => (d = new ScriptedDictation(ev)));
  await tick();
  v.drop(view as unknown as ConnectionView);
  assert.ok(d.cancelled);
  assert.equal(view.notes.length, 0);
  assert.equal(v.size, 0);
});

test("a removed thread ends its dictations with a reason", async () => {
  const v = sessions();
  const view = new FakeView();
  let d!: ScriptedDictation;
  const r = await v.start(view as unknown as ConnectionView, "gone", FORMAT, (ev) => (d = new ScriptedDictation(ev)));
  await tick();
  v.endThread("gone", "The thread was removed.");
  assert.ok(d.cancelled);
  assert.deepEqual(view.notes, [["voice/ended", { stream: r.stream, error: "The thread was removed." }]]);
});

test("a harness that ends a dictation itself closes the stream", async () => {
  const v = sessions();
  const view = new FakeView();
  const r = await v.start(view as unknown as ConnectionView, "t", FORMAT, (ev) => {
    ev.ended("Voice input needs Claude Code signed in");
    return new ScriptedDictation(ev);
  });
  await tick();
  assert.deepEqual(view.notes, [["voice/ended", { stream: r.stream, error: "Voice input needs Claude Code signed in" }]]);
  // Audio still in flight after the end is not voice any more (and is ignored).
  assert.equal(v.receive(view as unknown as ConnectionView, r.stream, Buffer.alloc(4)), false);
});

test("a refusal before the start is the start's error, and leaves no stream", async () => {
  const v = sessions();
  const view = new FakeView();
  await assert.rejects(
    v.start(view as unknown as ConnectionView, "t", FORMAT, (ev) => {
      const d = new ScriptedDictation(ev);
      return Object.assign(d, { accepted: Promise.reject(new DictationRefused("needs_auth", "Sign in first")).finally(() => ev.ended("Sign in first")) });
    }),
    (e: unknown) => e instanceof RpcError && e.code === "needs_auth" && e.message === "Sign in first",
  );
  await tick();
  assert.equal(v.size, 0);
  assert.deepEqual(view.notes, [], "nothing is sent for a stream the host never learned");
});

test("one dictation at a time per thread", async () => {
  const v = sessions();
  const view = new FakeView() as unknown as ConnectionView;
  await v.start(view, "t", FORMAT, (ev) => new ScriptedDictation(ev));
  await assert.rejects(v.start(view, "t", FORMAT, (ev) => new ScriptedDictation(ev)), (e: unknown) => e instanceof RpcError && e.code === "limit_exceeded");
  await v.start(view, "other", FORMAT, (ev) => new ScriptedDictation(ev));
  assert.equal(v.size, 2);
});

test("voice/stop waits for the audio it overtook, then stops", async () => {
  const v = sessions(1000);
  const view = new FakeView();
  const cv = view as unknown as ConnectionView;
  let d!: ScriptedDictation;
  const r = await v.start(cv, "t", FORMAT, (ev) => (d = new ScriptedDictation(ev)));
  await tick();
  v.receive(cv, r.stream, Buffer.alloc(640));
  v.stop(cv, r.stream, 1280);
  assert.equal(d.stopped, false, "640 bytes are still on their way");
  v.receive(cv, r.stream, Buffer.alloc(640));
  assert.equal(d.stopped, true);
  assert.equal(d.got.length, 2, "the last chunk reached the harness before the stop");
});

test("voice/stop stops after the grace period when audio never arrives", async () => {
  const v = sessions(30);
  const view = new FakeView();
  const cv = view as unknown as ConnectionView;
  let d!: ScriptedDictation;
  const r = await v.start(cv, "t", FORMAT, (ev) => (d = new ScriptedDictation(ev)));
  await tick();
  v.stop(cv, r.stream, 6400);
  assert.equal(d.stopped, false);
  await until(() => d.stopped, "stop after grace");
  assert.deepEqual(view.notes.at(-1), ["voice/ended", { stream: r.stream }]);
});
