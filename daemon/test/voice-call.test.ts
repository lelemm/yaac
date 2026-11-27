// Voice calls (docs/plan/voice-call.md): the provider-agnostic call registry. No media, no
// harness: a scripted call stands in for Codex.

import { test } from "node:test";
import assert from "node:assert/strict";
import type { ConnectionView } from "../src/protocol/connection.js";
import { VoiceCalls } from "../src/voice/calls.js";
import { DictationRefused, type VoiceCall, type VoiceCallEvents } from "../src/voice/types.js";

const tick = (ms = 5) => new Promise((r) => setTimeout(r, ms));

class FakeView {
  readonly notes: Array<[string, Record<string, unknown>]> = [];
  notify(method: string, params: unknown) {
    this.notes.push([method, params as Record<string, unknown>]);
    return true;
  }
}

class ScriptedCall implements VoiceCall {
  ended = 0;
  accepted: Promise<void>;
  constructor(readonly events: VoiceCallEvents, accepted: Promise<void> = Promise.resolve()) {
    this.accepted = accepted;
  }
  end() {
    this.ended++;
    this.events.ended();
  }
}

const v = (view: FakeView) => view as unknown as ConnectionView;

test("voiceCall/start answers first; connecting, answer, active, transcripts follow; ended is last", async () => {
  const calls = new VoiceCalls();
  const view = new FakeView();
  let call!: ScriptedCall;
  const r = await calls.start(v(view), "thread-1", (ev) => {
    call = new ScriptedCall(ev);
    ev.answer("v=answer"); // reported while the result is still on its way
    return call;
  });
  assert.match(r.callId, /^call-/);
  assert.equal(view.notes.length, 0, "nothing before the result");
  await tick();
  call.events.transcript("user", "hello", false);
  call.events.transcript("user", "Hello.", true);
  calls.end(v(view), r.callId);
  call.events.transcript("assistant", "late", false);
  assert.deepEqual(view.notes, [
    ["voiceCall/state", { callId: r.callId, state: "connecting" }],
    ["voiceCall/answer", { callId: r.callId, sdpAnswer: "v=answer" }],
    ["voiceCall/state", { callId: r.callId, state: "active" }],
    ["voiceCall/transcript", { callId: r.callId, role: "user", text: "hello", final: false }],
    ["voiceCall/transcript", { callId: r.callId, role: "user", text: "Hello.", final: true }],
    ["voiceCall/state", { callId: r.callId, state: "ended" }],
  ]);
  assert.equal(calls.size, 0);
  calls.end(v(view), r.callId); // ending an ended call is not an error
  assert.equal(call.ended, 1);
});

test("one call per thread; another connection cannot end it", async () => {
  const calls = new VoiceCalls();
  const view = new FakeView();
  const other = new FakeView();
  let call!: ScriptedCall;
  const r = await calls.start(v(view), "t", (ev) => (call = new ScriptedCall(ev)));
  await assert.rejects(calls.start(v(other), "t", (ev) => new ScriptedCall(ev)), (e: Error & { code?: string }) => e.code === "limit_exceeded");
  calls.end(v(other), r.callId);
  assert.equal(call.ended, 0);
  assert.ok(calls.has("t"));
});

test("a refusal is the start's error and leaves no call", async () => {
  const calls = new VoiceCalls();
  const view = new FakeView();
  await assert.rejects(
    calls.start(v(view), "t", (ev) => new ScriptedCall(ev, Promise.reject(new DictationRefused("needs_auth", "sign in to Codex")))),
    (e: Error & { code?: string }) => e.code === "needs_auth",
  );
  await tick();
  assert.equal(view.notes.length, 0);
  assert.equal(calls.has("t"), false);
});

test("a dropped connection hangs up its calls without sending anything", async () => {
  const calls = new VoiceCalls();
  const view = new FakeView();
  let call!: ScriptedCall;
  await calls.start(v(view), "t", (ev) => (call = new ScriptedCall(ev)));
  view.notes.length = 0;
  await tick();
  view.notes.length = 0;
  calls.drop(v(view));
  assert.equal(call.ended, 1);
  assert.equal(view.notes.length, 0);
  assert.equal(calls.size, 0);
});

test("an end asked for while the harness is still starting is applied once it accepted", async () => {
  const calls = new VoiceCalls();
  const view = new FakeView();
  let accept!: () => void;
  let call!: ScriptedCall;
  const starting = calls.start(v(view), "t", (ev) => (call = new ScriptedCall(ev, new Promise((r) => (accept = r)))));
  await tick();
  // The host has no callId yet, but the thread is taken.
  assert.ok(calls.has("t"));
  accept();
  const r = await starting;
  calls.end(v(view), r.callId);
  await tick();
  assert.equal(call.ended, 1);
  assert.deepEqual(view.notes.at(-1), ["voiceCall/state", { callId: r.callId, state: "ended" }]);
});

test("a removed thread ends its call with a reason", async () => {
  const calls = new VoiceCalls();
  const view = new FakeView();
  const r = await calls.start(v(view), "gone", (ev) => new ScriptedCall(ev));
  await tick();
  calls.endThread("gone", "The thread was removed.");
  assert.deepEqual(view.notes.at(-1), ["voiceCall/state", { callId: r.callId, state: "ended", error: "The thread was removed." }]);
});
