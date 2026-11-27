// Direct fallback for Claude voice input. Contract statically verified against Claude Code
// 2.1.330 (docs/research/harness-surfaces.md); tests inject a local WebSocket URL.

import WebSocket from "ws";
import { DictationRefused, type Dictation, type DictationEvents, type DictationOptions } from "../../voice/types.js";

type Inbound =
  | { type: "TranscriptInterim" | "TranscriptText"; data?: string }
  | { type: "TranscriptEndpoint" }
  | { type: "TranscriptError"; description?: string; error_code?: string }
  | { type: "error"; message?: string };

export interface VoiceStreamOptions {
  token(): Promise<string | null>;
  version(): string | null;
  /** The provider's effective Claude Code environment, used exactly as the CLI uses it for headers. */
  env?: NodeJS.ProcessEnv;
  entrypointOnRequests?: string;
  workload?: string;
  /** Config keyterms already selected by the provider; absent means the optional header is absent. */
  keyterms?: string[];
  endpoint?: string;
  websocket?: typeof WebSocket;
  keepAliveMs?: number;
  quietMs?: number;
  safetyMs?: number;
}

export function voiceClientPlatform(entrypoint: string | undefined): string {
  if (entrypoint === "claude-vscode") return "claude_code_vscode";
  if (["remote", "remote_baku", "remote_cowork", "remote_desktop", "remote_mobile", "remote_projects", "claude-in-teams"].includes(entrypoint ?? "")) return "claude_code_remote";
  if (["sdk-cli", "sdk-ts", "sdk-py"].includes(entrypoint ?? "")) return "claude_code_sdk";
  if (entrypoint === "mcp") return "claude_code_mcp";
  if (entrypoint === "claude-code-github-action") return "claude_code_github_action";
  if (entrypoint === "local-agent") return "claude_code_local_agent";
  if (entrypoint === "claude_in_slack" || entrypoint === "claude-in-slack") return entrypoint;
  return "claude_code_cli";
}

export function voiceUserAgent(version: string, env: NodeJS.ProcessEnv, entrypointOnRequests?: string, workload?: string): string {
  const entrypoint = entrypointOnRequests ?? env.CLAUDE_CODE_ENTRYPOINT ?? "cli";
  const suffixes = [
    env.CLAUDE_AGENT_SDK_VERSION && `agent-sdk/${env.CLAUDE_AGENT_SDK_VERSION}`,
    env.CLAUDE_AGENT_SDK_CLIENT_APP && `client-app/${env.CLAUDE_AGENT_SDK_CLIENT_APP}`,
    workload && `workload/${workload}`,
  ].filter((x): x is string => !!x);
  return `claude-cli/${version} (external, ${[entrypoint, ...suffixes].join(", ")})`;
}

export function voiceKeyterms(terms: readonly string[]): string | null {
  const out: string[] = [];
  let length = 0;
  for (const raw of terms) {
    const term = raw.replaceAll(",", " ").replace(/[^\x20-\x7e]/g, "").replace(/\s+/g, " ").trim();
    if (!term || out.includes(term)) continue;
    const next = length + (out.length ? 1 : 0) + term.length;
    if (next > 1024) break;
    out.push(term);
    length = next;
  }
  return out.length ? out.join(",") : null;
}

const LANGUAGES = new Map<string, string>();
for (const [code, ...names] of [
  ["en", "english"], ["es", "spanish", "español", "espanol"], ["fr", "french", "français", "francais"],
  ["ja", "japanese", "日本語"], ["de", "german", "deutsch"], ["pt", "portuguese", "português", "portugues"],
  ["it", "italian", "italiano"], ["ko", "korean", "한국어"], ["hi", "hindi", "हिन्दी", "हिंदी"],
  ["id", "indonesian", "bahasa indonesia", "bahasa"], ["ru", "russian", "русский"], ["pl", "polish", "polski"],
  ["tr", "turkish", "türkçe", "turkce"], ["nl", "dutch", "nederlands"], ["uk", "ukrainian", "українська"],
  ["el", "greek", "ελληνικά"], ["cs", "czech", "čeština", "cestina"], ["da", "danish", "dansk"],
  ["sv", "swedish", "svenska"], ["no", "norwegian", "norsk"],
] as const) {
  LANGUAGES.set(code, code);
  for (const name of names) LANGUAGES.set(name, code);
}

export function voiceLanguage(language: string | undefined): string {
  const normalized = language?.trim().toLowerCase() ?? "";
  const prefix = normalized.split("-")[0]!;
  return LANGUAGES.get(normalized) ?? (prefix.length === 2 ? LANGUAGES.get(prefix) : undefined) ?? "en";
}

export class ClaudeVoiceStream implements Dictation {
  private socket: WebSocket | null = null;
  private state: "opening" | "running" | "stopping" | "over" = "opening";
  private complete = "";
  private pending = "";
  private opened = false;
  private closeSent = false;
  private keepAlive?: NodeJS.Timeout;
  private quiet?: NodeJS.Timeout;
  private safety?: NodeJS.Timeout;
  private settle!: (error?: Error) => void;
  readonly accepted = new Promise<void>((resolve, reject) => {
    let done = false;
    this.settle = (error) => {
      if (done) return;
      done = true;
      error ? reject(error) : resolve();
    };
  });

  constructor(private readonly options: VoiceStreamOptions, private readonly dictation: DictationOptions, private readonly events: DictationEvents) {
    this.accepted.catch(() => {});
    void Promise.resolve().then(() => this.open())
      .catch(() => this.fail("Claude voice input could not open the connection.", "provider_unavailable"));
  }

  private url(): string {
    const url = new URL(this.options.endpoint ?? "wss://api.anthropic.com/api/ws/speech_to_text/voice_stream");
    for (const [key, value] of [
      ["encoding", "linear16"], ["sample_rate", "16000"], ["channels", "1"],
      ["endpointing_ms", "300"], ["utterance_end_ms", "1000"],
      ["language", voiceLanguage(this.dictation.language)], ["use_conversation_engine", "true"],
    ]) url.searchParams.set(key, value);
    if (/^(1|true|yes|on)$/i.test(this.options.env?.CLAUDE_CODE_VOICE_FORWARD_INTERIMS_TYPED?.trim() ?? "")) {
      url.searchParams.set("forward_interims", "typed");
    }
    return url.toString();
  }

  private async open(): Promise<void> {
    if (this.state === "over") return;
    let token: string | null;
    try { token = await this.options.token(); } catch {
      return this.fail("Could not read Claude Code's OAuth session.", "provider_unavailable");
    }
    if (!token) return this.fail("Claude Code is not signed in on this target.", "needs_auth");
    if (this.isOver()) return;
    const WS = this.options.websocket ?? WebSocket;
    const version = this.options.version() ?? "unknown";
    const env = this.options.env ?? {};
    const keyterms = voiceKeyterms(this.options.keyterms ?? []);
    const socket = new WS(this.url(), {
      handshakeTimeout: 10_000,
      followRedirects: false,
      maxPayload: 1_048_576,
      headers: {
        Authorization: `Bearer ${token}`,
        "User-Agent": voiceUserAgent(version, env, this.options.entrypointOnRequests, this.options.workload),
        "x-app": "cli",
        "anthropic-client-platform": voiceClientPlatform(env.CLAUDE_CODE_ENTRYPOINT),
        ...(keyterms ? { "x-config-keyterms": keyterms } : {}),
      },
    });
    this.socket = socket;
    socket.binaryType = "nodebuffer";
    socket.once("open", () => {
      if (this.state === "over") return socket.close();
      const stopping = this.state === "stopping";
      this.state = "running";
      this.opened = true;
      this.sendControl("KeepAlive");
      this.keepAlive = setInterval(() => this.sendControl("KeepAlive"), this.options.keepAliveMs ?? 8_000);
      this.keepAlive.unref?.();
      this.settle();
      if (stopping) this.stop();
    });
    socket.on("message", (data, binary) => {
      if (binary || this.state === "over") return;
      let message: Inbound;
      try { message = JSON.parse(data.toString()) as Inbound; } catch { return; }
      if (message && typeof message === "object") this.receive(message);
    });
    socket.once("unexpected-response", (_request, response) => {
      response.resume();
      const auth = response.statusCode === 401 || response.statusCode === 403;
      this.fail(auth ? "Claude Code's OAuth session was refused." : `Claude voice input answered HTTP ${response.statusCode}.`, auth ? "needs_auth" : "provider_unavailable");
    });
    socket.on("error", () => this.fail("Claude voice input lost its connection.", "provider_unavailable"));
    socket.once("close", () => {
      if (this.state === "over") return;
      if (this.state === "stopping" && this.opened) this.finish();
      else this.fail("Claude voice input closed the connection.", "provider_unavailable");
    });
  }

  private isOver(): boolean { return this.state === "over"; }

  private receive(message: Inbound): void {
    if (message.type === "TranscriptInterim" || message.type === "TranscriptText") {
      if (typeof message.data === "string" && message.data) {
        this.pending = message.data.trim();
        this.events.transcript(this.whole(), false);
        if (this.closeSent) clearTimeout(this.quiet);
      }
      return;
    }
    if (message.type === "TranscriptEndpoint") {
      this.commitPending();
      if (this.closeSent) this.finish();
      return;
    }
    if (message.type === "TranscriptError" || message.type === "error") {
      this.fail("Claude voice input reported a transcription error.", "provider_unavailable");
    }
  }

  private whole(): string { return [this.complete, this.pending].filter(Boolean).join(" "); }
  private commitPending(): void {
    const text = this.pending.trim();
    if (text) this.complete = [this.complete, text].filter(Boolean).join(" ");
    this.pending = "";
  }
  private sendControl(type: "KeepAlive" | "CloseStream"): void {
    if (this.socket?.readyState === WebSocket.OPEN) this.socket.send(JSON.stringify({ type }));
  }

  async audio(pcm: Buffer): Promise<void> {
    if (this.state !== "running" || this.socket?.readyState !== WebSocket.OPEN) return;
    await new Promise<void>((resolve) => this.socket!.send(pcm, { binary: true }, (error) => {
      if (error) this.fail("Claude voice input stopped accepting audio.", "provider_unavailable");
      resolve();
    }));
  }

  stop(): void {
    if (this.state === "over" || this.state === "stopping") return;
    if (this.state === "opening") { this.state = "stopping"; return; }
    this.state = "stopping";
    setTimeout(() => {
      if (this.state !== "stopping") return;
      this.closeSent = true;
      this.sendControl("CloseStream");
      this.armQuiet();
    }, 0);
    this.safety = setTimeout(() => this.finish(), this.options.safetyMs ?? 5_000);
    this.safety.unref?.();
  }

  private armQuiet(): void {
    clearTimeout(this.quiet);
    this.quiet = setTimeout(() => this.finish(), this.options.quietMs ?? 1_500);
    this.quiet.unref?.();
  }

  cancel(): void {
    if (this.state === "over") return;
    this.over();
    this.socket?.terminate();
    this.events.ended();
  }

  private finish(): void {
    if (this.state === "over") return;
    this.commitPending();
    const text = this.complete;
    this.over();
    this.events.transcript(text, true);
    this.events.ended();
    this.socket?.close();
  }

  private fail(message: string, code: "needs_auth" | "provider_unavailable"): void {
    if (this.state === "over") return;
    if (!this.opened) this.settle(new DictationRefused(code, message));
    const text = this.whole();
    this.over();
    if (text) this.events.transcript(text, true);
    this.events.ended(message);
    this.socket?.terminate();
  }

  private over(): void {
    this.settle();
    this.state = "over";
    clearInterval(this.keepAlive);
    clearTimeout(this.quiet);
    clearTimeout(this.safety);
  }
}
