// Voice input for Claude threads (docs/04-providers.md#voice-input): Claude Code's own host
// dictation, the `dictate` control request (verified in docs/research/harness-surfaces.md).
//
// Claude Code answers `dictate` only for a host it treats as the VS Code extension
// (`CLAUDE_CODE_ENTRYPOINT=claude-vscode`) and only while its server-side feature flag is on
// for the account; it then streams the audio to the same speech-to-text service `/voice` uses,
// with the CLI's own claude.ai sign-in. yaacd never sees a credential.
//
// Because that entrypoint changes how Claude Code behaves, it is never given to a thread's
// query. Each dictation runs in a separate dictation-only Claude Code process (started through
// the Agent SDK, no prompt, no tools, no MCP servers, hooks off, nothing persisted) that is kept
// warm for a short while for the next dictation. The request is `@internal`: it is sent with
// the query's runtime `request()` method, and everything here tolerates its absence.
//
// Request (2.1.330): `{subtype:"dictate", op:"start"|"audio"|"stop"|"cancel", language?, pcm?}`
// with `pcm` base64 of 16 kHz mono s16le, at most two seconds per request. Reply:
// `{status:"ok", text, interim, done}` or `{status:"unavailable", reason, text}`.

import type { Options } from "@anthropic-ai/claude-agent-sdk";
import { DictationRefused, type Dictation, type DictationEvents, type DictationOptions, type VoiceFormat } from "../../voice/types.js";
import type { ClaudeSdk, SdkQuery } from "./sdk.js";

/** What Claude Code's dictation takes (the request's own description). */
export const CLAUDE_VOICE_FORMAT: VoiceFormat = { format: "pcm16le", sampleRate: 16000, channels: 1, maxSeconds: 120 };

const BYTES_PER_SECOND = 32000;
/** The request's limit: two seconds of audio. */
const MAX_CHUNK = 2 * BYTES_PER_SECOND;
/** Send audio in batches of at least this much (100 ms) … */
const MIN_CHUNK = BYTES_PER_SECOND / 10;
/** … or after this long, whichever comes first. */
const BATCH_MS = 100;
/** A warm dictation process is closed after this long unused. */
export const DICTATION_IDLE_MS = 90_000;
/** A dictation request that has no answer by then counts as failed. */
const REQUEST_TIMEOUT_MS = 20_000;

type DictateReply =
  | { status: "ok"; text: string; interim: string; done: boolean }
  | { status: "unavailable"; reason: string; text: string };

/** A query with the SDK's runtime-only control request (present on 0.3.329). */
type ControlQuery = SdkQuery & { request?(request: Record<string, unknown>): Promise<{ response?: unknown }> };

/** Words for the user, by Claude Code's `unavailable` reason. */
export function unavailableMessage(reason: string): string {
  switch (reason) {
    case "not_signed_in": return "Voice input needs Claude Code signed in with a claude.ai account on this target.";
    case "provider": return "Voice input works only when Claude Code uses Anthropic directly (not an API gateway, Bedrock or Vertex).";
    case "nonessential_traffic": return "Voice input is off because Claude Code's nonessential traffic is disabled on this target.";
    case "policy": return "Voice input is turned off by Claude Code's settings or your organization's policy.";
    case "started_by_session": return "Voice input is not available while yaacd runs inside a Claude Code session.";
    case "launcher_started": return "Voice input is not available when Claude Code's sign-in is managed by a launcher.";
    case "not_available_here": return "Claude Code's voice input is not available on this target.";
    case "not_started": return "The dictation timed out (no audio for 15 seconds).";
    case "connection_failed": return "Claude Code could not reach its speech-to-text service.";
    default: return `Claude Code could not take dictation (${reason}).`;
  }
}

function parseReply(value: unknown): DictateReply {
  const r = (value ?? {}) as Record<string, unknown>;
  const text = typeof r.text === "string" ? r.text : "";
  if (r.status === "ok") return { status: "ok", text, interim: typeof r.interim === "string" ? r.interim : "", done: r.done === true };
  if (r.status === "unavailable") return { status: "unavailable", reason: typeof r.reason === "string" ? r.reason : "unknown", text };
  throw new Error("Claude Code gave an unrecognised dictation reply");
}

/** The environment of the dictation-only process: the provider's, as the VS Code host. */
export function dictationEnv(base: NodeJS.ProcessEnv): NodeJS.ProcessEnv {
  return { ...base, CLAUDE_CODE_ENTRYPOINT: "claude-vscode" };
}

/** One dictation-only Claude Code process. Requests go out one at a time, in order. */
export class DictationProcess {
  private chain: Promise<unknown> = Promise.resolve();
  private failed: Error | null = null;
  private readonly input = new NeverInput();
  readonly query: ControlQuery;
  closed = false;

  constructor(sdk: ClaudeSdk, options: Options, private readonly log: (line: string) => void) {
    const query = sdk.query({ prompt: this.input, options }) as ControlQuery;
    this.query = query;
    void (async () => {
      try {
        for await (const _ of query) void _;
      } catch (e) {
        this.failed = e as Error;
      }
      if (!this.closed) this.failed ??= new Error("the dictation process ended");
    })();
  }

  get usable(): boolean {
    return !this.closed && !this.failed;
  }

  /** Sends one `dictate` request after the ones before it. */
  dictate(request: { op: "start" | "audio" | "stop" | "cancel"; language?: string; pcm?: string }): Promise<DictateReply> {
    const run = async (): Promise<DictateReply> => {
      if (this.failed) throw this.failed;
      if (this.closed) throw new Error("the dictation process was closed");
      if (typeof this.query.request !== "function") throw new Error("this Agent SDK cannot send Claude Code's dictation request");
      let timer: NodeJS.Timeout | undefined;
      const timeout = new Promise<never>((_, reject) => {
        timer = setTimeout(() => reject(new Error("Claude Code did not answer the dictation request")), REQUEST_TIMEOUT_MS);
        timer.unref?.();
      });
      try {
        const res = await Promise.race([this.query.request({ subtype: "dictate", ...request }), timeout]);
        return parseReply(res?.response);
      } finally {
        clearTimeout(timer);
      }
    };
    const next = this.chain.then(run, run);
    this.chain = next.catch(() => {});
    return next;
  }

  close(): void {
    if (this.closed) return;
    this.closed = true;
    this.input.end();
    try {
      this.query.close();
    } catch (e) {
      this.log(`closing the dictation process failed: ${(e as Error).message}`);
    }
  }
}

/** A prompt stream that never yields: the dictation process is never asked anything. */
class NeverInput implements AsyncIterable<never> {
  private waiters: Array<(r: IteratorResult<never>) => void> = [];
  private ended = false;
  end(): void {
    this.ended = true;
    for (const w of this.waiters.splice(0)) w({ value: undefined as never, done: true });
  }
  [Symbol.asyncIterator](): AsyncIterator<never> {
    return {
      next: () => (this.ended ? Promise.resolve({ value: undefined as never, done: true }) : new Promise((r) => this.waiters.push(r))),
    };
  }
}

export interface DictationHostOptions {
  sdk: () => Promise<ClaudeSdk>;
  /** The executable and environment of the provider; null when Claude Code is not installed. */
  executable: () => string | null;
  env: NodeJS.ProcessEnv;
  cwd: string;
  log(line: string): void;
  idleMs?: number;
}

/** Starts dictations, each in a dictation process, and keeps one warm process for the next. */
export class ClaudeDictationHost {
  private warm: DictationProcess | null = null;
  private warmTimer?: NodeJS.Timeout;
  private readonly active = new Set<ClaudeDictation>();
  private disposed = false;

  constructor(private readonly opts: DictationHostOptions) {}

  private options(): Options {
    const exe = this.opts.executable();
    if (!exe) throw new Error("Claude Code (`claude`) is not installed on this target");
    return {
      cwd: this.opts.cwd,
      pathToClaudeCodeExecutable: exe,
      env: dictationEnv(this.opts.env),
      persistSession: false,
      strictMcpConfig: true,
      tools: [],
      settings: { disableAllHooks: true },
      permissionMode: "default",
      canUseTool: async () => ({ behavior: "deny", message: "This Claude Code process only takes dictation." }),
      stderr: () => {},
    };
  }

  private async take(): Promise<DictationProcess> {
    if (this.disposed) throw new Error("the Claude provider was closed");
    clearTimeout(this.warmTimer);
    const warm = this.warm;
    this.warm = null;
    if (warm?.usable) return warm;
    warm?.close();
    return new DictationProcess(await this.opts.sdk(), this.options(), this.opts.log);
  }

  /** A dictation is over: keep its process warm when it is healthy and nothing else is. */
  release(p: DictationProcess, healthy: boolean): void {
    if (this.disposed || !healthy || !p.usable || this.warm) {
      p.close();
      return;
    }
    this.warm = p;
    clearTimeout(this.warmTimer);
    this.warmTimer = setTimeout(() => {
      if (this.warm === p) this.warm = null;
      p.close();
    }, this.opts.idleMs ?? DICTATION_IDLE_MS);
    this.warmTimer.unref?.();
  }

  /**
   * Whether Claude Code takes dictation from this host: a `dictate {op:"cancel"}`, which needs
   * no network and no audio, is answered only where the request is enabled. The process is
   * kept warm when it is. Sign-in and policy are checked by Claude Code at `start`.
   */
  async probe(timeoutMs: number): Promise<{ available: boolean; detail?: string }> {
    let p: DictationProcess | undefined;
    try {
      p = await this.take();
      const pending = p.dictate({ op: "cancel" });
      let timer: NodeJS.Timeout | undefined;
      const timeout = new Promise<never>((_, reject) => {
        timer = setTimeout(() => reject(new Error("no answer")), timeoutMs);
        timer.unref?.();
      });
      const reply = await Promise.race([pending, timeout]).finally(() => clearTimeout(timer));
      const ok = reply.status === "ok";
      this.release(p, ok);
      return ok ? { available: true } : { available: false, detail: reply.status === "unavailable" ? reply.reason : undefined };
    } catch (e) {
      p?.close();
      return { available: false, detail: (e as Error).message };
    }
  }

  start(opts: DictationOptions, events: DictationEvents): Dictation {
    const d = new ClaudeDictation(this, opts, events, () => this.active.delete(d));
    this.active.add(d);
    d.begin(this.take());
    return d;
  }

  dispose(): void {
    this.disposed = true;
    clearTimeout(this.warmTimer);
    this.warm?.close();
    this.warm = null;
    for (const d of [...this.active]) d.fail("the Claude provider was closed");
  }
}

interface Pending {
  bytes: Buffer;
  done: () => void;
}

/** One dictation: batches audio into `dictate` requests and reports the words. */
export class ClaudeDictation implements Dictation {
  private process: DictationProcess | null = null;
  private state: "starting" | "running" | "stopping" | "over" = "starting";
  private pending: Pending[] = [];
  private pendingBytes = 0;
  private carry: Buffer | null = null;
  private sending = false;
  private batchTimer?: NodeJS.Timeout;
  private lastText = "";
  /** Claude Code accepted `start`. */
  private started = false;
  private settleReady!: (error?: Error) => void;
  /** Settles when Claude Code accepted or refused `start` (see `Dictation.accepted`). */
  readonly accepted: Promise<void> = new Promise<void>((resolve, reject) => {
    let settled = false;
    this.settleReady = (error) => {
      if (settled) return;
      settled = true;
      if (error) reject(error);
      else resolve();
    };
  });

  constructor(
    private readonly host: ClaudeDictationHost,
    private readonly opts: DictationOptions,
    private readonly events: DictationEvents,
    private readonly onOver: () => void,
  ) {
    this.accepted.catch(() => {});
  }

  private settle(error?: Error): void {
    this.settleReady(error);
  }

  begin(taken: Promise<DictationProcess>): void {
    void (async () => {
      let p: DictationProcess;
      try {
        p = await taken;
      } catch (e) {
        this.fail(`Claude Code could not start for dictation: ${(e as Error).message}`);
        return;
      }
      if (this.state === "over") {
        this.host.release(p, true);
        return;
      }
      this.process = p;
      let reply: DictateReply;
      try {
        reply = await p.dictate({ op: "start", ...(this.opts.language ? { language: this.opts.language } : {}) });
      } catch (e) {
        this.fail(`Claude Code refused dictation: ${(e as Error).message}`);
        return;
      }
      if (this.isOver()) return;
      if (reply.status === "unavailable") {
        this.refused(reply.reason === "not_signed_in" ? "needs_auth" : "provider_unavailable", unavailableMessage(reply.reason));
        this.finishWith(reply.text, unavailableMessage(reply.reason), true);
        return;
      }
      this.started = true;
      this.settle();
      if (this.state === "starting") this.state = "running";
      this.pump();
    })();
  }

  audio(pcm: Buffer): Promise<void> {
    if (this.state === "over" || this.state === "stopping") return Promise.resolve();
    return new Promise((resolve) => {
      this.pending.push({ bytes: pcm, done: resolve });
      this.pendingBytes += pcm.length;
      this.pump();
    });
  }

  stop(): void {
    if (this.state === "over" || this.state === "stopping") return;
    // While starting, `begin` sees "stopping" and its pump sends the rest, then `stop`.
    const starting = this.state === "starting";
    this.state = "stopping";
    if (!starting) this.pump();
  }

  cancel(): void {
    if (this.state === "over") return;
    const p = this.process;
    this.over();
    this.events.ended();
    if (p) {
      // Drops the dictation in Claude Code (and its words); the process stays usable.
      p.dictate({ op: "cancel" }).then(() => this.host.release(p, true), () => this.host.release(p, false));
    }
  }

  /** Ends with an error (the process is not reused). */
  fail(message: string): void {
    if (this.state === "over") return;
    this.refused("provider_unavailable", message);
    const p = this.process;
    this.over();
    if (p) this.host.release(p, false);
    if (this.lastText) this.events.transcript(this.lastText, true);
    this.events.ended(message);
  }

  private isOver(): boolean {
    return this.state === "over";
  }

  /** Before Claude Code accepted `start`, an end is a refusal of the start. */
  private refused(code: "needs_auth" | "provider_unavailable", message: string): void {
    if (!this.started) this.settle(new DictationRefused(code, message));
  }

  private over(): void {
    this.settle();
    this.state = "over";
    clearTimeout(this.batchTimer);
    for (const x of this.pending.splice(0)) x.done();
    this.pendingBytes = 0;
    this.carry = null;
    this.onOver();
  }

  private finishWith(text: string, error: string | undefined, healthy: boolean): void {
    if (this.state === "over") return;
    const p = this.process;
    this.over();
    if (p) this.host.release(p, healthy);
    const words = text || this.lastText;
    if (words || !error) this.events.transcript(words, true);
    this.events.ended(error);
  }

  private show(text: string, interim: string): void {
    const whole = [text, interim].filter((s) => s.length > 0).join(" ");
    if (whole === this.lastText) return;
    this.lastText = whole;
    this.events.transcript(whole, false);
  }

  /** Takes up to MAX_CHUNK bytes (an even count) off the queue, resolving what was taken. */
  private takeChunk(all: boolean): Buffer | null {
    const parts: Buffer[] = this.carry ? [this.carry] : [];
    let size = this.carry?.length ?? 0;
    this.carry = null;
    while (this.pending.length && size < MAX_CHUNK) {
      const head = this.pending[0]!;
      const room = MAX_CHUNK - size;
      if (head.bytes.length <= room) {
        this.pending.shift();
        parts.push(head.bytes);
        size += head.bytes.length;
        this.pendingBytes -= head.bytes.length;
        head.done();
      } else {
        parts.push(head.bytes.subarray(0, room));
        head.bytes = head.bytes.subarray(room);
        this.pendingBytes -= room;
        size += room;
      }
    }
    if (size === 0) return null;
    let chunk = parts.length === 1 ? parts[0]! : Buffer.concat(parts, size);
    if (chunk.length % 2 === 1) {
      // Keep whole 16-bit samples: the odd byte goes with the next batch (dropped at the end).
      this.carry = chunk.subarray(chunk.length - 1);
      chunk = chunk.subarray(0, chunk.length - 1);
      if (all) this.carry = null;
    }
    return chunk.length ? chunk : null;
  }

  private pump(): void {
    if (this.sending || !this.process || !this.started || this.state === "over") return;
    const stopping = this.state === "stopping";
    const queued = this.pendingBytes + (this.carry?.length ?? 0);
    if (!stopping && queued < MIN_CHUNK) {
      if (queued > 0 && !this.batchTimer) {
        this.batchTimer = setTimeout(() => {
          this.batchTimer = undefined;
          this.send(this.takeChunk(false));
        }, BATCH_MS);
        this.batchTimer.unref?.();
      }
      return;
    }
    clearTimeout(this.batchTimer);
    this.batchTimer = undefined;
    const chunk = this.takeChunk(stopping);
    if (chunk) this.send(chunk);
    else if (stopping) this.sendStop();
  }

  private send(chunk: Buffer | null): void {
    const p = this.process;
    if (!chunk || !p || this.sending || this.state === "over") return;
    this.sending = true;
    p.dictate({ op: "audio", pcm: chunk.toString("base64") }).then(
      (reply) => {
        this.sending = false;
        if (this.state === "over") return;
        if (reply.status === "unavailable") return this.finishWith(reply.text, unavailableMessage(reply.reason), true);
        this.show(reply.text, reply.interim);
        // The two-minute limit: Claude Code stopped the dictation itself and sent all words.
        if (reply.done) return this.finishWith(reply.text, undefined, true);
        this.pump();
      },
      (e: Error) => {
        this.sending = false;
        this.fail(`Claude Code stopped taking dictation: ${e.message}`);
      },
    );
  }

  private sendStop(): void {
    const p = this.process;
    if (!p || this.sending || this.state !== "stopping") return;
    this.sending = true;
    p.dictate({ op: "stop" }).then(
      (reply) => {
        this.sending = false;
        if (reply.status === "unavailable") this.finishWith(reply.text, unavailableMessage(reply.reason), true);
        else this.finishWith(reply.text, undefined, true);
      },
      (e: Error) => {
        this.sending = false;
        this.fail(`Claude Code stopped taking dictation: ${e.message}`);
      },
    );
  }
}
