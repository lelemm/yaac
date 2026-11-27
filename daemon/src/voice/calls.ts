// Voice calls (docs/plan/voice-call.md): `voiceCall/start`, `voiceCall/end`, and the
// `voiceCall/answer` / `voiceCall/state` / `voiceCall/transcript` notifications.
// Provider-agnostic: a harness supplies a `VoiceCall` (./types.ts); this module owns the call
// id, one call per thread, ordering (the start's result before any notification), and the
// guarantee that `state: ended` is the last message for a call. Only SDP and transcript text
// pass through here, in memory; nothing is journaled or logged.

import { randomUUID } from "node:crypto";
import type { ConnectionView } from "../protocol/connection.js";
import { NOTIFICATIONS } from "../protocol/generated/constants.js";
import type { VoiceCallStartResult } from "../protocol/generated/types.js";
import { RpcError } from "../protocol/rpc.js";
import { DictationRefused, type VoiceCall, type VoiceCallEvents } from "./types.js";

/** Longest SDP offer accepted (a real offer is a few KiB). */
export const MAX_SDP_BYTES = 64 * 1024;

interface Active {
  view: ConnectionView;
  callId: string;
  threadId: string;
  call?: VoiceCall;
  /** Set once `ended` was sent (or the start was refused). */
  over: boolean;
  /** `voiceCall/end` arrived before the provider handed back its call. */
  pendingEnd: boolean;
}

export class VoiceCalls {
  private byId = new Map<string, Active>();

  /** Whether a call is running in `threadId`. */
  has(threadId: string): boolean {
    for (const a of this.byId.values()) if (a.threadId === threadId) return true;
    return false;
  }

  /**
   * Starts one call and waits for the harness to accept it. Notifications for the call are
   * deferred until the result has gone out: `connecting` first, then whatever happened.
   */
  async start(view: ConnectionView, threadId: string, open: (events: VoiceCallEvents) => VoiceCall): Promise<VoiceCallStartResult> {
    if (this.has(threadId)) throw new RpcError("limit_exceeded", "a voice call is already running in this thread");
    const callId = `call-${randomUUID()}`;
    const a: Active = { view, callId, threadId, over: false, pendingEnd: false };
    this.byId.set(callId, a);
    let ready = false;
    const deferred: Array<() => void> = [];
    const later = (f: () => void) => (ready ? f() : deferred.push(f));
    const events: VoiceCallEvents = {
      answer: (sdp) => later(() => {
        if (a.over) return;
        view.notify(NOTIFICATIONS.VOICE_CALL_ANSWER, { callId, sdpAnswer: sdp });
        view.notify(NOTIFICATIONS.VOICE_CALL_STATE, { callId, state: "active" });
      }),
      transcript: (role, text, final) => later(() => {
        if (!a.over) view.notify(NOTIFICATIONS.VOICE_CALL_TRANSCRIPT, { callId, role, text, final });
      }),
      ended: (error) => later(() => this.finish(a, error)),
    };
    try {
      a.call = open(events);
      await a.call.accepted;
    } catch (e) {
      a.over = true;
      this.byId.delete(callId);
      if (a.call && !(e instanceof DictationRefused)) a.call.end();
      if (e instanceof DictationRefused) throw new RpcError(e.code, e.message);
      throw e instanceof RpcError ? e : new RpcError("provider_unavailable", (e as Error)?.message ?? String(e));
    }
    if (a.over && !this.byId.has(callId)) {
      // The connection went away while the harness was starting: the call was hung up.
      throw new RpcError("internal_error", "the connection closed while the call was starting");
    }
    setImmediate(() => {
      ready = true;
      if (!a.over) view.notify(NOTIFICATIONS.VOICE_CALL_STATE, { callId, state: "connecting" });
      for (const f of deferred.splice(0)) f();
      if (a.pendingEnd && !a.over) a.call?.end();
    });
    return { callId };
  }

  /** `voiceCall/end`. Ending an unknown or finished call is not an error. */
  end(view: ConnectionView, callId: unknown): void {
    const a = typeof callId === "string" ? this.byId.get(callId) : undefined;
    if (!a || a.view !== view || a.over) return;
    if (!a.call) {
      a.pendingEnd = true;
      return;
    }
    a.call.end();
  }

  /** The connection went away: its calls are hung up (nobody is left to hear them). */
  drop(view: ConnectionView): void {
    for (const a of [...this.byId.values()]) if (a.view === view) this.abandon(a);
  }

  /** The thread went away: its call ends with a reason. */
  endThread(threadId: string, reason: string): void {
    for (const a of [...this.byId.values()]) {
      if (a.threadId !== threadId) continue;
      this.finish(a, reason);
      a.call?.end();
    }
  }

  shutdown(): void {
    for (const a of [...this.byId.values()]) this.abandon(a);
  }

  get size(): number {
    return this.byId.size;
  }

  private abandon(a: Active): void {
    this.byId.delete(a.callId);
    a.over = true;
    a.call?.end();
  }

  private finish(a: Active, error?: string): void {
    if (a.over) return;
    a.over = true;
    this.byId.delete(a.callId);
    a.view.notify(NOTIFICATIONS.VOICE_CALL_STATE, { callId: a.callId, state: "ended", ...(error ? { error } : {}) });
  }
}
