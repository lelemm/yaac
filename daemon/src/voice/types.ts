// Voice input (docs/plan/voice.md), the part every harness shares.
//
// The host records the microphone and streams raw audio on a data stream; yaacd hands it to the
// thread's harness, whose own speech-to-text turns it into words. A provider implements
// `HarnessProvider.startDictation` with a `Dictation`; `VoiceSessions` (./sessions.ts) owns the
// stream, its credit, and the notifications. Audio is never written to disk or journaled.

import type { VoiceFormat } from "../protocol/generated/types.js";

export type { VoiceFormat };

/** What a dictation reports. yaacd forwards each call to the host that started it. */
export interface DictationEvents {
  /**
   * The whole transcript so far (finished words, then the current guess). It replaces the
   * previous one. `final`: it will not change again (sent once, before `ended`).
   */
  transcript(text: string, final: boolean): void;
  /** The dictation is over. Called exactly once; `error` in words for the user. */
  ended(error?: string): void;
}

/**
 * The harness refused to begin a dictation. `code` is the error `voice/start` answers with:
 * `needs_auth` (it wants a sign-in), `provider_unavailable` (any other refusal; the message
 * carries the harness's reason in words for the user).
 */
export class DictationRefused extends Error {
  constructor(readonly code: "needs_auth" | "provider_unavailable", message: string) {
    super(message);
    this.name = "DictationRefused";
  }
}

/** One running dictation inside a provider. */
export interface Dictation {
  /**
   * Optional: settles once the harness has accepted (resolves) or refused (rejects, with a
   * `DictationRefused` for a refusal) the dictation. When present, `voice/start` answers only
   * then, so a refusal is the start's error and no stream is left behind; events reported
   * before a refusal are dropped. Absent: the start is answered at once and a refusal
   * arrives as `ended(error)`.
   */
  readonly accepted?: Promise<void>;
  /**
   * Audio in the thread's `voiceInput` format, in order. Resolves when the harness has taken
   * these bytes (yaacd then returns their credit to the host), or when the dictation is over.
   */
  audio(pcm: Buffer): Promise<void>;
  /** No more audio: the final transcript and `ended` follow. */
  stop(): void;
  /** Drop the dictation and its words: `ended` follows (without an error). */
  cancel(): void;
}

export interface DictationOptions {
  /** The spoken language as a code (`en`, `pt-BR`); absent: the harness's own setting. */
  language?: string;
}

// ── Voice calls (docs/plan/voice-call.md) ─────────────────────────────────────────────────────
// A speech-to-speech conversation with a thread. The host is the WebRTC peer: only its SDP
// offer, the service's answer, and transcript text pass through yaacd; never audio.

/** What a call reports. yaacd forwards each one to the host that started it. */
export interface VoiceCallEvents {
  /** The service's SDP answer (once). */
  answer(sdp: string): void;
  /** The whole current utterance of `role`; `final` closes it. */
  transcript(role: "user" | "assistant", text: string, final: boolean): void;
  /** The call is over. Called exactly once; `error` in words for the user. */
  ended(error?: string): void;
}

/** One running call inside a provider. */
export interface VoiceCall {
  /**
   * Settles once the harness accepted (resolves) or refused (rejects; a `DictationRefused`
   * for a refusal) the call. `voiceCall/start` answers only then.
   */
  readonly accepted: Promise<void>;
  /** Hang up: `ended` follows (without an error). */
  end(): void;
}
