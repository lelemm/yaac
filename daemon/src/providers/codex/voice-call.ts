// Voice call with a Codex thread (docs/plan/voice-call.md, docs/04-providers.md § Codex voice
// call), through the app-server's EXPERIMENTAL `thread/realtime/*` methods — what Codex's own
// `/voice` does.
//
// Verified against codex-cli 0.172.0 (generated schema and the tagged source rust-v0.172.0,
// app-server/tests/suite/v2/realtime_conversation.rs and tui/src/app_server_session/realtime.rs):
//
// - `thread/realtime/start { threadId, outputModality: "audio", transport: { type: "webrtc",
//   sdp }, version: "v3" }` takes the client's SDP offer. Codex posts it to its realtime calls
//   endpoint with its own sign-in (a ChatGPT sign-in works), answers `{}`, then emits
//   `thread/realtime/started` and `thread/realtime/sdp { threadId, sdp }` (the answer) and joins
//   the call through a sideband websocket. WebRTC needs a conversational session, v1 or v3;
//   the TUI uses v3.
// - Media runs between the host's peer and OpenAI over RTP: no audio reaches yaacd.
// - The realtime model's delegations start or steer turns in this same thread. The provider
//   treats turns that start while the call runs as the user's own (the call is the user's).
// - `model` and `voice` stay unset: Codex picks them (invariant 3).
// - Language (docs/plan/voice-call.md § Language): `prompt` *replaces* the realtime model's whole
//   default prompt (codex-prompts `prepare_realtime_backend_prompt`), and
//   `realtimeStartInstructions` replaces the default start instructions of the *backing* Codex
//   model, not the voice. `initialItems` (v3 only, "complete role-bearing text items to the
//   initial Frameless Bidi session history") go into the session Codex posts with the offer
//   (`session.initial_items`, codex-api `methods_frameless_bidi::session_json`) beside Codex's
//   own instructions. So a chosen language is one `developer` initial item. Nothing in the
//   schema takes a language or locale, and the TUI's `/voice` sets none.
//
// The call runs on the user's thread, so nothing here creates a thread or touches config.

import type { CodexConnection } from "./rpc.js";
import { RpcError } from "../../protocol/rpc.js";
import { DictationRefused, type VoiceCall, type VoiceCallEvents, type VoiceCallOptions } from "../../voice/types.js";
import { callLanguageInstruction } from "../../voice/language.js";

/** The realtime protocol version the TUI's `/voice` uses (0.172.0); WebRTC refuses v2. */
export const CALL_REALTIME_VERSION = "v3";

export interface CodexVoiceCallDeps {
  /** The live connection to the Codex daemon (the provider's one connection). */
  connection(): Promise<CodexConnection>;
  /** Makes sure this connection is subscribed to the thread (resumes it when needed). */
  prepare(): Promise<void>;
  /** The call is live in the thread (true) or over (false): turns it starts are the user's. */
  live(on: boolean): void;
  /** Diagnostic log: never SDP, never words. */
  log(line: string): void;
  /** How long `end` waits for Codex to report the session closed. */
  closeWaitMs?: number;
}

/** Words for the user when Codex refuses or fails. */
export function callErrorText(message: string): string {
  if (/require[s]? conversational/i.test(message)) {
    return "Codex voice calls need a conversational realtime session; this target's Codex config sets realtime.type to transcription.";
  }
  return `Codex voice call failed: ${message}`;
}

export function startCodexVoiceCall(thread: string, sdpOffer: string, deps: CodexVoiceCallDeps, events: VoiceCallEvents, opts: VoiceCallOptions = {}): VoiceCall {
  return new CodexVoiceCall(thread, sdpOffer, deps, events, opts);
}

/** `thread/realtime/start`'s `initialItems` for `opts`: the language, as one developer item. */
export function callInitialItems(opts: VoiceCallOptions): Array<{ role: "developer"; text: string }> | undefined {
  return opts.language ? [{ role: "developer", text: callLanguageInstruction(opts.language) }] : undefined;
}

class CodexVoiceCall implements VoiceCall {
  readonly accepted: Promise<void>;
  private over = false;
  private realtimeLive = false;
  private answered = false;
  private ending = false;
  private conn: CodexConnection | null = null;
  private offs: Array<() => void> = [];
  private current: Record<"user" | "assistant", string> = { user: "", assistant: "" };

  constructor(
    private readonly thread: string,
    private readonly offer: string,
    private readonly deps: CodexVoiceCallDeps,
    private readonly events: VoiceCallEvents,
    private readonly opts: VoiceCallOptions,
  ) {
    this.accepted = this.open().catch((e) => {
      this.cleanup();
      const message = (e as Error)?.message ?? String(e);
      throw e instanceof DictationRefused || e instanceof RpcError ? e : new DictationRefused("provider_unavailable", callErrorText(message));
    });
  }

  private async open(): Promise<void> {
    const conn = await this.deps.connection();
    this.conn = conn;
    this.offs.push(conn.notifications.on(({ method, params }) => this.onNotification(method, params)));
    this.offs.push(conn.closed.on(() => this.finish("Lost the connection to Codex during the call.")));
    await this.deps.prepare();
    if (this.over) throw new Error("the call ended while it was starting");
    this.deps.live(true);
    const initialItems = callInitialItems(this.opts);
    await conn.request("thread/realtime/start", {
      threadId: this.thread,
      outputModality: "audio",
      transport: { type: "webrtc", sdp: this.offer },
      version: CALL_REALTIME_VERSION,
      ...(initialItems ? { initialItems } : {}),
      // `model` and `voice` unset: Codex picks them (invariant 3). Handoffs stay automatic,
      // so Codex's replies reach the realtime model, which speaks them.
    }, 20_000);
    this.realtimeLive = true;
    this.deps.log("Codex voice call started");
    if (this.over) void this.stopRealtime();
  }

  end(): void {
    if (this.over || this.ending) return;
    this.ending = true;
    void (async () => {
      await this.accepted.catch(() => {});
      if (this.over) return;
      await this.stopRealtime();
      const wait = this.deps.closeWaitMs ?? 4_000;
      const from = Date.now();
      while (!this.over && Date.now() - from < wait) await new Promise((r) => setTimeout(r, 25));
      this.finish();
    })();
  }

  private async stopRealtime(): Promise<void> {
    const conn = this.conn;
    if (!this.realtimeLive || !conn?.isOpen) return;
    this.realtimeLive = false;
    await conn.request("thread/realtime/stop", { threadId: this.thread }, 4_000).catch(() => {});
  }

  private onNotification(method: string, raw: unknown): void {
    if (!method.startsWith("thread/realtime/")) return;
    const p = (raw && typeof raw === "object" ? raw : {}) as Record<string, unknown>;
    if (p.threadId !== this.thread || this.over) return;
    switch (method) {
      case "thread/realtime/sdp":
        if (this.answered || typeof p.sdp !== "string" || !p.sdp) return;
        this.answered = true;
        this.events.answer(p.sdp);
        return;
      case "thread/realtime/transcript/delta":
      case "thread/realtime/transcript/done": {
        const role = p.role === "user" || p.role === "assistant" ? p.role : null;
        if (!role) return;
        if (method.endsWith("/delta")) {
          if (typeof p.delta !== "string" || !p.delta) return;
          this.current[role] += p.delta;
          this.events.transcript(role, this.current[role], false);
        } else {
          const text = typeof p.text === "string" ? p.text : this.current[role];
          this.current[role] = "";
          if (text.trim()) this.events.transcript(role, text, true);
        }
        return;
      }
      case "thread/realtime/error":
        void this.stopRealtime();
        return this.finish(callErrorText(typeof p.message === "string" ? p.message : "unknown error"));
      case "thread/realtime/closed": {
        this.realtimeLive = false;
        const reason = typeof p.reason === "string" ? p.reason : "";
        // A close we asked for ("requested"), or after our stop, is a normal end.
        const normal = this.ending || reason === "requested" || reason === "";
        return this.finish(normal ? undefined : `Codex ended the call: ${reason}`);
      }
      default:
        // started, items, output audio deltas (none over WebRTC), …: nothing for the host.
        return;
    }
  }

  private finish(error?: string): void {
    if (this.over) return;
    this.over = true;
    for (const role of ["user", "assistant"] as const) {
      const rest = this.current[role];
      this.current[role] = "";
      if (rest.trim()) this.events.transcript(role, rest, true);
    }
    this.events.ended(error);
    this.cleanup();
  }

  private cleanup(): void {
    this.over = true;
    for (const off of this.offs.splice(0)) off();
    this.deps.live(false);
    if (this.realtimeLive) void this.stopRealtime();
  }
}
