// Transcription as an instance's service (docs/plan/voice.md, P392): yaacd advertises per
// instance whether it can transcribe (capability, format, or the reason it cannot), and
// `voice/start` names the instance — a thread of another instance, or no thread at all (a
// new-thread draft), dictates through it. The words never reach a conversation.

import { test } from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as path from "node:path";
import { FAKE, RpcFailure, client, opId, startDaemon, startThread, tmpHome, until, waitTurnDone, fakeOf } from "./helpers.js";

const VOICE = "pi_voice";
const REFUSED = "pi_refused";

/** `pi_fake` (no voice surface), `pi_voice` (transcribes) and `pi_refused` (cannot, says why). */
function home(): string {
  const dir = tmpHome();
  fs.writeFileSync(path.join(dir, "instances.json"), JSON.stringify({ instances: [
    { id: VOICE, kind: "fake", name: "voice", enabled: true, backend: "auto", env: { FAKE_SEED: "0", FAKE_TRANSCRIBE: "ready" } },
    { id: REFUSED, kind: "fake", name: "refused", enabled: true, backend: "auto", env: { FAKE_SEED: "0", FAKE_TRANSCRIBE: "refused" } },
  ] }));
  return dir;
}

test("instances advertise transcription with its format or the reason, and threads never do", async () => {
  const daemon = await startDaemon(home());
  try {
    const c = await client(daemon);
    const { providers } = await until(async () => {
      const r = await c.request("provider/list", {});
      return r.providers.every((p: any) => p.status === "ready") && r.providers.length >= 3 ? r : null;
    }, "probed instances");
    const by = (id: string) => providers.find((p: any) => p.instance === id);
    assert.ok(by(VOICE).capabilities.includes("transcription"));
    assert.deepEqual(by(VOICE).transcription, { format: { format: "pcm16le", sampleRate: 16000, channels: 1 } });
    assert.ok(!by(REFUSED).capabilities.includes("transcription"));
    assert.match(by(REFUSED).transcription.unavailable, /FAKE_TRANSCRIBE=ready/);
    assert.equal(by(FAKE).transcription, undefined, "no voice surface, nothing said");
    // A thread of the transcribing instance still has no such capability: it is not the thread's.
    const { threadId, turnId } = await startThread(c, "hello", { instance: VOICE });
    await c.request("thread/subscribe", { threadId, epoch: 1, afterSeq: 0 });
    await waitTurnDone(c, threadId, turnId);
    const effective = await daemon.core.computeEffective(daemon.core.thread(threadId)!);
    assert.ok(!(effective.capabilities ?? []).includes("transcription"), JSON.stringify(effective.capabilities));
    assert.equal((effective as unknown as Record<string, unknown>).voiceInput, undefined);
    await c.close();
  } finally {
    await daemon.stop("test");
  }
});

test("a thread of an instance that cannot transcribe dictates through one that can", async () => {
  const daemon = await startDaemon(home());
  try {
    const c = await client(daemon);
    const { threadId, turnId } = await startThread(c, "hello");
    await c.request("thread/subscribe", { threadId, epoch: 1, afterSeq: 0 });
    await waitTurnDone(c, threadId, turnId);
    const before = c.events(threadId).length;
    await until(async () => (await c.request("provider/list", {})).providers.some((p: any) => p.instance === VOICE && p.capabilities.includes("transcription")), "the transcriber");
    const r = await c.request("voice/start", { opId: opId(), instance: VOICE, threadId, language: "pt-BR" });
    assert.deepEqual(r.format, { format: "pcm16le", sampleRate: 16000, channels: 1 });
    c.sendData(r.stream, Buffer.alloc(6400));
    await c.request("voice/stop", { stream: r.stream, bytes: 6400 });
    const ended = await until(() => c.notifications.find((n) => n.method === "voice/ended" && n.params.stream === r.stream), "ended");
    assert.equal(ended.params.error, undefined);
    const last = c.notifications.filter((n) => n.method === "voice/transcript" && n.params.stream === r.stream).at(-1);
    assert.deepEqual(last?.params, { stream: r.stream, text: "word1 word2", final: true });
    assert.deepEqual(fakeOf(daemon, VOICE).dictations, [{ language: "pt-BR", bytes: 6400 }]);
    assert.equal(c.events(threadId).length, before, "nothing reached the thread");
    assert.equal(fakeOf(daemon, FAKE).received.length, 1, "no input went to the thread's harness");
    await c.close();
  } finally {
    await daemon.stop("test");
  }
});

test("a draft dictates without a thread; an instance that cannot transcribe is refused with its reason", async () => {
  const daemon = await startDaemon(home());
  try {
    const c = await client(daemon);
    await until(async () => (await c.request("provider/list", {})).providers.some((p: any) => p.instance === VOICE && p.capabilities.includes("transcription")), "the transcriber");
    const a = await c.request("voice/start", { opId: opId(), instance: VOICE });
    // No thread: no per-thread limit either.
    const b = await c.request("voice/start", { opId: opId(), instance: VOICE });
    assert.notEqual(a.stream, b.stream);
    await c.request("voice/cancel", { stream: a.stream });
    await c.request("voice/cancel", { stream: b.stream });
    const refused = await c.request("voice/start", { opId: opId(), instance: REFUSED }).catch((e) => e);
    assert.ok(refused instanceof RpcFailure);
    assert.equal(refused.code, "capability_missing");
    assert.match(refused.error.message, /FAKE_TRANSCRIBE=ready/);
    const none = await c.request("voice/start", { opId: opId(), instance: FAKE }).catch((e) => e);
    assert.equal(none.code, "capability_missing");
    const missing = await c.request("voice/start", { opId: opId() }).catch((e) => e);
    assert.equal(missing.code, "invalid_params");
    await c.close();
  } finally {
    await daemon.stop("test");
  }
});
