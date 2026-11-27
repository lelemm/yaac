// Voice input for Codex threads (docs/plan/voice.md, docs/04-providers.md § Codex voice input),
// through the app-server's EXPERIMENTAL `thread/realtime/*` methods.
//
// Verified against codex-cli 0.172.0 (generated schema and the tagged source, rust-v0.172.0):
//
// - The realtime session type (`conversational` or `transcription`) is not a request parameter.
//   It is the thread's config `realtime.type`, read when the thread is loaded. A
//   `conversational` session gives the realtime model a "send to the background agent" tool
//   that starts a Codex turn in the thread, and `clientManagedHandoffs` does not prevent that
//   (it only stops Codex's replies being forwarded back to the realtime model). A
//   `transcription` session has no instructions, no tools, and no responses: only
//   `conversation.item.input_audio_transcription.*`, which reach us as
//   `thread/realtime/transcript/{delta,done}` with `role: "user"`.
// - So a dictation never runs on the user's thread. It runs on a private helper thread that
//   yaacd starts for it: `ephemeral: true` (never written to disk), with the config override
//   `realtime.type = "transcription"`, `realtime.version = "v2"` (Codex folds every other
//   version back to conversational), `realtime.transport = "websocket"`. Nothing is added to
//   the user's thread, and the user's own Codex config is not touched.
// - The websocket transport needs API-key auth in 0.172.0 ("realtime conversation requires API
//   key auth"): a ChatGPT sign-in only gets WebRTC, which Codex allows for conversational
//   sessions only. The provider offers `transcription` only where a key is available.
// - Audio: base64 PCM16 mono at 24 kHz (`REALTIME_AUDIO_SAMPLE_RATE` in codex-api), sent with
//   `thread/realtime/appendAudio`. Codex never commits the input buffer itself, so the server's
//   default turn detection ends each utterance on silence; on stop yaacd appends a short
//   silence so the last words are transcribed before the session closes.
//
// Audio is never written anywhere: it is base64-encoded in memory, sent, and dropped. Should the
// helper session ever produce anything but the user's transcript (an assistant transcript,
// output audio, a turn, an item), the dictation ends at once with an error.

import type { CodexConnection } from "./rpc.js";
import type { Dictation, DictationEvents, DictationOptions, VoiceFormat } from "../../voice/types.js";

/** `threadSource` of the private dictation threads, so they are never listed or followed. */
export const VOICE_SOURCE = "yaac.voice-input";

/** What Codex's realtime session takes (codex-api `REALTIME_AUDIO_SAMPLE_RATE`, 0.172.0). */
export const CODEX_VOICE_FORMAT: VoiceFormat = { format: "pcm16le", sampleRate: 24_000, channels: 1 };

/** Config overrides for the helper thread: a transcription-only realtime session. */
export const TRANSCRIPTION_CONFIG = {
  "realtime.type": "transcription",
  "realtime.version": "v2",
  "realtime.transport": "websocket",
} as const;

export interface CodexDictationTimings {
  /** Silence appended on stop, so the server's turn detection closes the last utterance. */
  silenceMs: number;
  /** After the silence: stop once no transcript has moved for this long. */
  settleMs: number;
  /** After the silence: stop after this long whatever is still pending. */
  graceMs: number;
}

export const DEFAULT_TIMINGS: CodexDictationTimings = { silenceMs: 800, settleMs: 1_500, graceMs: 6_000 };

export interface CodexDictationDeps {
  /** The live connection to the Codex daemon (the provider's one connection). */
  connection(): Promise<CodexConnection>;
  /** Working directory of the helper thread (yaacd's private state directory). */
  cwd: string;
  /** Config overrides that quiet the helper thread (MCP servers off, …), merged under ours. */
  quietConfig?(conn: CodexConnection): Promise<Record<string, unknown>>;
  /** Marks a helper thread id as private (never listed, followed, or reported). */
  remember(threadId: string): void;
  /** Diagnostic log: never audio, never words. */
  log(line: string): void;
  timings?: Partial<CodexDictationTimings>;
}

/** Bytes per appendAudio request: one second of audio. */
const MAX_CHUNK_BYTES = CODEX_VOICE_FORMAT.sampleRate * CODEX_VOICE_FORMAT.channels * 2;

type Phase = "starting" | "live" | "stopping" | "over";

/** Words for the user when Codex refuses or fails. */
export function voiceErrorText(message: string): string {
  if (/requires API key auth/i.test(message)) {
    return "Codex voice input needs OpenAI API-key sign-in on this target (a ChatGPT sign-in only offers voice in Codex's own apps).";
  }
  return `Codex voice input failed: ${message}`;
}

export function startCodexDictation(deps: CodexDictationDeps, events: DictationEvents, _opts: DictationOptions = {}): Dictation {
  return new CodexDictation(deps, events).begin();
}

class CodexDictation implements Dictation {
  private phase: Phase = "starting";
  private conn: CodexConnection | null = null;
  private threadId: string | null = null;
  private realtimeLive = false;
  private closedByCodex = false;
  private offs: Array<() => void> = [];
  private ready!: Promise<boolean>;
  private chain: Promise<void> = Promise.resolve();
  private carry: Buffer | null = null;
  private segments: string[] = [];
  private current = "";
  private pending = false;
  private lastMove = 0;
  private lastSent = "";
  private readonly t: CodexDictationTimings;

  constructor(private readonly deps: CodexDictationDeps, private readonly events: DictationEvents) {
    this.t = { ...DEFAULT_TIMINGS, ...deps.timings };
  }

  /** A method, not a field read: the phase changes across awaits. */
  private isOver(): boolean {
    return this.phase === "over";
  }

  begin(): this {
    this.ready = this.open().then(
      () => true,
      (e) => {
        this.finish(voiceErrorText((e as Error)?.message ?? String(e)));
        return false;
      },
    );
    return this;
  }

  private async open(): Promise<void> {
    const conn = await this.deps.connection();
    if (this.isOver()) return;
    this.conn = conn;
    this.offs.push(conn.notifications.on(({ method, params }) => this.onNotification(method, params)));
    this.offs.push(conn.closed.on(() => this.finish("Lost the connection to Codex during dictation.")));
    const quiet = this.deps.quietConfig ? await this.deps.quietConfig(conn).catch(() => ({})) : {};
    const { thread } = await conn.request("thread/start", {
      cwd: this.deps.cwd,
      ephemeral: true,
      threadSource: VOICE_SOURCE,
      approvalPolicy: "never",
      sandbox: "read-only",
      config: { ...quiet, ...TRANSCRIPTION_CONFIG },
    }, 8_000) as { thread: { id: string; ephemeral?: boolean } };
    this.threadId = thread.id;
    this.deps.remember(thread.id);
    // Only a thread Codex will not write to disk may carry the session.
    if (thread.ephemeral !== true) throw new Error("Codex did not start a private (ephemeral) session");
    if (this.isOver()) return void this.cleanup();
    await conn.request("thread/realtime/start", {
      threadId: thread.id,
      outputModality: "text",
      transport: { type: "websocket" },
      version: "v2",
      clientManagedHandoffs: true,
      includeStartupContext: false,
      // `model` stays unset: Codex picks its own (invariant 3).
    }, 15_000);
    this.realtimeLive = true;
    if (this.isOver()) return void this.cleanup();
    if (this.phase === "starting") this.phase = "live";
    this.deps.log("Codex dictation started");
  }

  audio(pcm: Buffer): Promise<void> {
    const next = this.chain.then(async () => {
      if (!(await this.ready) || this.isOver()) return;
      await this.send(pcm);
    });
    this.chain = next.catch(() => {});
    return this.chain;
  }

  private async send(pcm: Buffer): Promise<void> {
    let buf = this.carry ? Buffer.concat([this.carry, pcm]) : pcm;
    this.carry = null;
    if (buf.length % 2 === 1) {
      this.carry = buf.subarray(buf.length - 1);
      buf = buf.subarray(0, buf.length - 1);
    }
    for (let off = 0; off < buf.length && !this.isOver(); off += MAX_CHUNK_BYTES) {
      const chunk = buf.subarray(off, Math.min(buf.length, off + MAX_CHUNK_BYTES));
      try {
        await this.conn!.request("thread/realtime/appendAudio", {
          threadId: this.threadId,
          audio: {
            data: chunk.toString("base64"),
            sampleRate: CODEX_VOICE_FORMAT.sampleRate,
            numChannels: CODEX_VOICE_FORMAT.channels,
            samplesPerChannel: chunk.length / 2,
            itemId: null,
          },
        }, 10_000);
      } catch (e) {
        this.finish(voiceErrorText((e as Error).message));
        return;
      }
    }
  }

  stop(): void {
    if (this.isOver() || this.phase === "stopping") return;
    this.phase = "stopping";
    void this.drainAndStop();
  }

  private async drainAndStop(): Promise<void> {
    await this.chain;
    if (!(await this.ready) || this.isOver()) return;
    const silence = Buffer.alloc(Math.round((CODEX_VOICE_FORMAT.sampleRate * this.t.silenceMs) / 1000) * 2);
    await this.send(silence);
    const from = Date.now();
    this.lastMove = Math.max(this.lastMove, from);
    while (!this.isOver() && !this.closedByCodex) {
      const now = Date.now();
      if (now - from >= this.t.graceMs) break;
      if (!this.pending && now - this.lastMove >= this.t.settleMs) break;
      await new Promise((r) => setTimeout(r, 50));
    }
    this.finish();
  }

  cancel(): void {
    if (this.isOver()) return;
    this.phase = "over";
    this.events.ended();
    void this.cleanup();
  }

  private onNotification(method: string, raw: unknown): void {
    const p = (raw && typeof raw === "object" ? raw : {}) as Record<string, unknown>;
    if (!this.threadId || p.threadId !== this.threadId || this.isOver()) return;
    switch (method) {
      case "thread/realtime/started":
      case "thread/realtime/itemAdded":
      case "thread/realtime/item/started":
      case "thread/realtime/item/completed":
      case "thread/status/changed":
      case "thread/tokenUsage/updated":
        return;
      case "thread/realtime/transcript/delta":
      case "thread/realtime/transcript/done": {
        if (p.role !== "user") return this.unsafe(method);
        if (method.endsWith("/delta")) {
          if (typeof p.delta !== "string" || !p.delta) return;
          this.current += p.delta;
          this.pending = true;
        } else {
          const done = typeof p.text === "string" ? p.text.trim() : this.current.trim();
          if (done) this.segments.push(done);
          this.current = "";
          this.pending = false;
        }
        this.lastMove = Date.now();
        this.report(false);
        return;
      }
      case "thread/realtime/error":
        return this.finish(voiceErrorText(typeof p.message === "string" ? p.message : "unknown error"));
      case "thread/realtime/closed":
        this.realtimeLive = false;
        this.closedByCodex = true;
        if (this.phase !== "stopping") this.finish(`Codex closed the voice session${typeof p.reason === "string" && p.reason ? `: ${p.reason}` : "."}`);
        return;
      default:
        // Output audio, a turn, an item, a request: the session is not transcription-only.
        if (method.startsWith("thread/realtime/") || method.startsWith("turn/") || method.startsWith("item/")) this.unsafe(method);
    }
  }

  private unsafe(method: string): void {
    this.deps.log(`Codex dictation ended: unexpected ${method} in a transcription session`);
    this.finish("Codex did not keep the voice session transcription-only; dictation was stopped.");
  }

  private text(): string {
    return [...this.segments, this.current.trim()].filter(Boolean).join(" ");
  }

  private report(final: boolean): void {
    const text = this.text();
    if (!final && text === this.lastSent) return;
    this.lastSent = text;
    this.events.transcript(text, final);
  }

  /** Ends the dictation once: the final transcript (if there are words), then `ended`. */
  private finish(error?: string): void {
    if (this.isOver()) return;
    this.phase = "over";
    if (this.text()) this.report(true);
    this.events.ended(error);
    void this.cleanup();
  }

  private async cleanup(): Promise<void> {
    for (const off of this.offs.splice(0)) off();
    const conn = this.conn;
    const threadId = this.threadId;
    if (!conn || !threadId || !conn.isOpen) return;
    if (this.realtimeLive) {
      this.realtimeLive = false;
      await conn.request("thread/realtime/stop", { threadId }, 4_000).catch(() => {});
    }
    await conn.request("thread/unsubscribe", { threadId }, 2_000).catch(() => {});
  }
}
