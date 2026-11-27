// Voice input streams (docs/plan/voice.md): `voice/start`, the audio data stream, `voice/stop`,
// `voice/cancel`, and the `voice/transcript` / `voice/ended` notifications. Provider-agnostic:
// a harness supplies a `Dictation` (./types.ts); this module owns the stream number, the
// credit window, ordering, the stop's byte count, and the guarantee that `voice/ended` is the
// last message for a stream. Audio lives only in memory, bounded by the credit window, and is
// never journaled or logged.

import type { ConnectionView } from "../protocol/connection.js";
import { NOTIFICATIONS } from "../protocol/generated/constants.js";
import type { VoiceStartResult } from "../protocol/generated/types.js";
import { RpcError } from "../protocol/rpc.js";
import { ReceiveWindow } from "../protocol/scheduler.js";
import { DictationRefused, type Dictation, type DictationEvents, type VoiceFormat } from "./types.js";

/** Seconds of audio the host may send ahead of what the harness has taken. */
export const VOICE_WINDOW_SECONDS = 4;

/** How long `voice/stop` waits for audio it overtook on the control lane. */
export const VOICE_STOP_GRACE_MS = 3000;

interface Active {
  view: ConnectionView;
  stream: number;
  threadId: string;
  window: ReceiveWindow;
  dictation?: Dictation;
  /** Audio bytes received on the stream. */
  received: number;
  /** Set once `voice/ended` was sent (or the start was refused): nothing more for the stream. */
  over: boolean;
  /** stop/cancel asked for before the provider handed back its dictation. */
  pendingEnd?: "stop" | "cancel";
  /** A stop waiting for this many bytes. */
  stopAt?: { bytes: number; timer: NodeJS.Timeout };
}

function bytesPerSecond(f: VoiceFormat): number {
  return f.sampleRate * f.channels * 2;
}

export class VoiceSessions {
  private byStream = new Map<number, Active>();

  constructor(
    private readonly allocate: () => number,
    private readonly stopGraceMs = VOICE_STOP_GRACE_MS,
  ) {}

  /**
   * Opens a stream for one dictation and asks the provider for it. When the dictation has a
   * `accepted` promise the answer waits for it, and a refusal is this request's error. The
   * response goes out before any notification for the stream (they are deferred a tick).
   */
  async start(
    view: ConnectionView,
    threadId: string,
    format: VoiceFormat,
    open: (events: DictationEvents) => Dictation,
  ): Promise<VoiceStartResult> {
    if (format.format !== "pcm16le" || !(format.sampleRate > 0) || !(format.channels > 0)) {
      throw new RpcError("capability_missing", "this thread's harness reports no audio format yaacd can carry");
    }
    for (const a of this.byStream.values()) {
      if (a.threadId === threadId) throw new RpcError("limit_exceeded", "a dictation is already running in this thread");
    }
    const stream = this.allocate();
    const credit = bytesPerSecond(format) * VOICE_WINDOW_SECONDS;
    const a: Active = { view, stream, threadId, window: new ReceiveWindow(stream, credit), received: 0, over: false };
    this.byStream.set(stream, a);
    let ready = false;
    const deferred: Array<() => void> = [];
    const later = (f: () => void) => (ready ? f() : deferred.push(f));
    const events: DictationEvents = {
      transcript: (text, final) => later(() => {
        if (!a.over) view.notify(NOTIFICATIONS.VOICE_TRANSCRIPT, { stream, text, final });
      }),
      ended: (error) => later(() => this.finish(a, error)),
    };
    try {
      a.dictation = open(events);
      await a.dictation.accepted;
    } catch (e) {
      // Refused or failed before it began: no stream, nothing more is sent for it.
      a.over = true;
      this.byStream.delete(stream);
      if (a.stopAt) clearTimeout(a.stopAt.timer);
      if (a.dictation && !(e instanceof DictationRefused)) a.dictation.cancel();
      if (e instanceof DictationRefused) throw new RpcError(e.code, e.message);
      throw e instanceof RpcError ? e : new RpcError("provider_unavailable", (e as Error)?.message ?? String(e));
    }
    if (a.over && !this.byStream.has(stream)) {
      // The connection went away while the harness was starting.
      throw new RpcError("internal_error", "the connection closed while the dictation was starting");
    }
    setImmediate(() => {
      ready = true;
      for (const f of deferred.splice(0)) f();
      if (a.pendingEnd && !a.over) this.end(a, a.pendingEnd);
    });
    return { stream, initialCredit: credit, format };
  }

  /** A data frame. False when the stream is not a voice stream of this connection. */
  receive(view: ConnectionView, stream: number, bytes: Buffer): boolean {
    const a = this.byStream.get(stream);
    if (!a || a.view !== view) return false;
    // Over credit is a protocol violation, fatal for the connection (as for uploads).
    a.window.onReceived(bytes.length);
    a.received += bytes.length;
    const consumed = () => {
      const grant = a.window.onConsumed(bytes.length);
      if (grant && !a.over) view.grantCredit(stream, grant);
    };
    if (a.over || !a.dictation || bytes.length === 0) consumed();
    else a.dictation.audio(bytes).then(consumed, consumed);
    if (a.stopAt && a.received >= a.stopAt.bytes) this.stopNow(a);
    return true;
  }

  /**
   * `voice/stop`. `bytes` is every audio byte the host sent: the request can overtake the last
   * data frames, so the dictation stops once that many arrived, or after a short grace period.
   */
  stop(view: ConnectionView, stream: unknown, bytes?: unknown): void {
    const a = this.require(view, stream);
    if (a.stopAt || a.over) return;
    const want = typeof bytes === "number" && Number.isFinite(bytes) && bytes > 0 ? bytes : 0;
    if (a.received >= want) {
      this.end(a, "stop");
      return;
    }
    const timer = setTimeout(() => this.stopNow(a), this.stopGraceMs);
    timer.unref?.();
    a.stopAt = { bytes: want, timer };
  }

  cancel(view: ConnectionView, stream: unknown): void {
    const a = this.require(view, stream);
    if (a.stopAt) clearTimeout(a.stopAt.timer);
    a.stopAt = undefined;
    this.end(a, "cancel");
  }

  /** The connection went away: its dictations are cancelled (nobody is left to read them). */
  drop(view: ConnectionView): void {
    for (const a of [...this.byStream.values()]) if (a.view === view) this.abandon(a);
  }

  /** The thread went away (deleted, forgotten): its dictations end with a reason. */
  endThread(threadId: string, reason: string): void {
    for (const a of [...this.byStream.values()]) {
      if (a.threadId !== threadId) continue;
      this.finish(a, reason);
      a.dictation?.cancel();
    }
  }

  shutdown(): void {
    for (const a of [...this.byStream.values()]) this.abandon(a);
  }

  /** Dictations running now (diagnostics, idle tracking). */
  get size(): number {
    return this.byStream.size;
  }

  private require(view: ConnectionView, stream: unknown): Active {
    const a = typeof stream === "number" ? this.byStream.get(stream) : undefined;
    if (!a || a.view !== view) throw new RpcError("invalid_params", "no such dictation (it may have ended)");
    return a;
  }

  private stopNow(a: Active): void {
    if (!a.stopAt) return;
    clearTimeout(a.stopAt.timer);
    a.stopAt = undefined;
    this.end(a, "stop");
  }

  private end(a: Active, how: "stop" | "cancel"): void {
    if (a.over) return;
    if (!a.dictation) {
      a.pendingEnd = how;
      return;
    }
    if (how === "stop") a.dictation.stop();
    else a.dictation.cancel();
  }

  private abandon(a: Active): void {
    this.byStream.delete(a.stream);
    a.over = true;
    if (a.stopAt) clearTimeout(a.stopAt.timer);
    a.dictation?.cancel();
  }

  private finish(a: Active, error?: string): void {
    if (a.over) return;
    a.over = true;
    if (a.stopAt) clearTimeout(a.stopAt.timer);
    this.byStream.delete(a.stream);
    a.view.notify(NOTIFICATIONS.VOICE_ENDED, { stream: a.stream, ...(error ? { error } : {}) });
  }
}
