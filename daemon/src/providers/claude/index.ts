// The Claude provider (kind `claude`): drives Claude Code through the Claude Agent SDK, using the
// `claude` executable installed on the target (docs/04-providers.md#claude).
//
// One `query()` in streaming-input mode per live thread; the SDK runs Claude Code as a child of
// yaacd. A thread's id is the Claude session id, chosen up front for a new thread. Idle queries
// are closed and resumed on the next message, and only so many are kept open at once — but a
// query whose process runs background work (subagents, shells) is never closed (D160).

import { randomUUID } from "node:crypto";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import type {
  ElicitationRequest,
  ElicitationResult,
  HookInput,
  HookJSONOutput,
  McpServerConfig,
  ModelInfo as SdkModelInfo,
  Options,
  PermissionMode,
  PermissionResult,
  SDKMessage,
  SDKUserMessage,
} from "@anthropic-ai/claude-agent-sdk";
import type {
  AppliedSettings,
  Capability,
  ModeInfo,
  ModelInfo,
  RequestResponse,
  RunningCommand,
  SkillsListResult,
  SubagentStatus,
  ThreadEffective,
  TokenUsage,
  TurnId,
  TurnOrigin,
  TurnStatus,
} from "../../protocol/generated/types.js";
import { RpcError } from "../../protocol/rpc.js";
import { ClaudeCommands, claudeCommandLogs } from "./commands.js";
import { Emitter } from "../../util/emitter.js";
import { readJson, sha256Hex, writeJsonAtomic } from "../../util/fsx.js";
import type {
  ExternalControl,
  ForkOptions,
  HarnessProvider,
  ItemPage,
  MeshHooks,
  NativeId,
  NativeThreadInfo,
  PageRequest,
  ScanRequest,
  ProbeResult,
  ProviderContext,
  ProviderEvent,
  ProviderFactory,
  ProviderInput,
  RaisedRequest,
  StartThreadParams,
  ThreadPage,
  TurnOverrides,
} from "../types.js";
import { CompactionUnconfirmed, InputOutcomeUnknown, PeerInputDenied } from "../types.js";
import { peerXml, supportsPeerInput } from "../../mesh/format.js";
import { ASK_TOOL, answeredInput, approvalRequest, approvalResult, normaliseQuestions, type ToolPermissionContext } from "./questions.js";
import { EXECUTABLE_VAR, findExecutable, loadRealSdk, realCli, subagentFiles, type AgentEntry, type ClaudeCli, type ClaudeSdk, type SdkQuery, type SubagentFile } from "./sdk.js";
import { flagsOf, historyItems, isToolResultMessage, LiveTranslator, type ItemEvent } from "./translate.js";
import { previewText } from "./markup.js";
import { claudeSkills } from "../skills.js";
import { ClaudeRateLimits, readWithOwnQuery } from "./ratelimits.js";
import type { RateLimitReport } from "../ratelimits.js";
import type { StoreToolImage } from "../tool-images.js";

/** A refresh of the rate limits starts a query only after this long without news (D161). */
const CLAUDE_LIMITS_QUIET_MS = 15 * 60 * 1000;

export const CLAUDE_KIND = "claude";

/** Instance-configuration variables read by yaacd (never passed on to Claude Code). */
export const IDLE_MS_VAR = "YAAC_CLAUDE_IDLE_MS";
export const MAX_LIVE_VAR = "YAAC_CLAUDE_MAX_LIVE";
export const DEFAULT_IDLE_MS = 5 * 60 * 1000;
export const DEFAULT_MAX_LIVE = 4;

/** Media types sent to Claude Code as image content blocks (docs/06-attachments.md). */
const IMAGE_FORMATS = ["image/png", "image/jpeg", "image/gif", "image/webp"];

/**
 * Permission modes yaacd offers. These are the SDK's `PermissionMode` values (an API enum, not
 * anything about models). `default` is the default because yaacd always passes a mode, so that
 * approvals reach the user through `canUseTool`; a thread never enters another mode on its own.
 *
 * `bypassPermissions` is offered, marked `skipsApprovals` (the only mode that never asks: the UI
 * confirms before switching to it). The SDK requires `allowDangerouslySkipPermissions` when the
 * query is created for that mode to be reachable at all. Verified (Claude Code 2.1.330, SDK
 * 0.3.329): with the option, a query opened in `default` still asks through `canUseTool`, and
 * `setPermissionMode` moves it into and out of bypass on the live query; without it,
 * `setPermissionMode("bypassPermissions")` is refused. So a query is opened with the option and
 * a mode change never re-opens the process (D160) — a re-open would kill the background work
 * running in it. The one exception: Claude Code exits at start-up when it runs as root outside a
 * sandbox and is given the option, so as root it is given only to a query opened in bypass (as
 * before; bypass is refused there anyway). In bypass, `AskUserQuestion` still reaches the user
 * as a question (`askQuestion`).
 */
export const BYPASS_MODE: PermissionMode = "bypassPermissions";
// `autonomy` (D146) restates the SDK's own description of each mode (sdk.d.ts, `PermissionMode`)
// on the scale every provider shares; the agent mesh compares modes on it. `dontAsk` acts on
// exactly what the user's rules allow, as `default` does without asking, so it is `ask` too.
const MODES: Array<ModeInfo & { id: PermissionMode; needsAutoMode?: boolean }> = [
  { id: "default", displayName: "Ask before acting", isDefault: true, autonomy: "ask" },
  { id: "acceptEdits", displayName: "Accept edits", description: "File edits are accepted; other actions still ask.", autonomy: "edits" },
  { id: "plan", displayName: "Plan only", description: "Claude plans and does not change anything.", autonomy: "readOnly" },
  { id: "dontAsk", displayName: "Don't ask", description: "Anything not already allowed by a rule is denied without asking.", autonomy: "ask" },
  { id: "auto", displayName: "Auto", description: "A classifier approves or denies each action and asks when unsure.", needsAutoMode: true, autonomy: "judged" },
  { id: BYPASS_MODE, displayName: "Bypass permissions", description: "Claude acts without asking for any approval.", skipsApprovals: true, autonomy: "full" },
];

// ── The mesh hook ────────────────────────────────────────────────────────────────────────

/** The MCP server name the agent mesh is attached under. */
const MESH_SERVER = "yaac";

/** What a caller (the agent mesh) may add to one thread's query. */
export interface ClaudeThreadExtras {
  /** Extra MCP servers, by name. An in-process server (`type: "sdk"`, from `createSdkMcpServer`) is allowed. */
  mcpServers?: Record<string, McpServerConfig>;
  /** Text appended to Claude Code's own system prompt (the identity block). */
  appendSystemPrompt?: string;
  /** Tool permission rules allowed without prompting (the mesh's own tools: `mcp__yaac`). */
  allowedTools?: string[];
}

/**
 * Called every time a query is opened for a thread — when it is created, and on every resume
 * (including after idle eviction), so the extras are supplied again on a cold resume.
 */
export type ClaudeExtrasHook = (thread: { native: NativeId; cwd: string; resumed: boolean }) => ClaudeThreadExtras | undefined | Promise<ClaudeThreadExtras | undefined>;

// ── Internal state ───────────────────────────────────────────────────────────────────────

interface ThreadSettings {
  cwd: string;
  model?: string | null;
  effort?: string | null;
  mode?: string | null;
  /** Fast mode (D125): Claude Code's `fastMode` setting for this session's turns. */
  fast?: boolean;
  /** Whether Claude Code has stored the session (resume, rather than create, on the next open). */
  stored?: boolean;
  /** Folders kept readable for the session's whole life (a handoff fork's transcript). */
  readable?: string[];
  /** The thread's attachment folder: readable from the first open on, so attaching never re-opens. */
  uploads?: string;
}

/** One background task running in a live query's process (`background_tasks_changed`). */
interface BackgroundTask {
  type: string;
  description: string;
  subagentType?: string;
}

interface Pending {
  requestId: string;
  /** The thread it was raised on: the session's own, or one of its subagents' (D162). */
  thread: NativeId;
  /** The turn it belongs to; "" for one bound to no turn (it outlives turns, D162). */
  turnId: TurnId;
  request: RaisedRequest;
  answer(response: RequestResponse): void;
  expire(): void;
}

interface TurnState {
  turnId: TurnId;
  origin: TurnOrigin;
  interrupting: boolean;
  compact?: { confirmed: boolean; error: string | null; connectionLost: boolean };
  ended: Promise<void>;
  end: () => void;
}

class InputQueue implements AsyncIterable<SDKUserMessage> {
  private items: SDKUserMessage[] = [];
  private waiters: Array<(r: IteratorResult<SDKUserMessage>) => void> = [];
  private ended = false;

  push(m: SDKUserMessage): boolean {
    if (this.ended) return false;
    const w = this.waiters.shift();
    if (w) w({ value: m, done: false });
    else this.items.push(m);
    return true;
  }

  end(): void {
    this.ended = true;
    for (const w of this.waiters.splice(0)) w({ value: undefined, done: true });
  }

  [Symbol.asyncIterator](): AsyncIterator<SDKUserMessage> {
    return {
      next: () => {
        const m = this.items.shift();
        if (m) return Promise.resolve({ value: m, done: false });
        if (this.ended) return Promise.resolve({ value: undefined, done: true });
        return new Promise((resolve) => this.waiters.push(resolve));
      },
      return: () => {
        this.end();
        return Promise.resolve({ value: undefined, done: true });
      },
    };
  }
}

class Live {
  query!: SdkQuery;
  readonly input = new InputQueue();
  pid: number | null = null;
  pidKnown!: Promise<void>;
  peerInput = false;
  cancelQueued = false;
  /** Native lifecycle acknowledgements and optional follow-on turns, by command uuid. */
  readonly peerCommands = new Map<string, { acknowledge: (error?: Error) => void; beforeFollowOn?: () => void; started?: boolean; expired?: boolean }>();
  turn: TurnState | null = null;
  readonly requests = new Map<string, Pending>();
  readonly ours = new Set<string>();
  /** Questions in flight, by tool_use id: `canUseTool` and the PreToolUse hook share one. */
  readonly asking = new Map<string, Promise<Record<string, unknown> | null>>();
  /** Whether the query was created with `allowDangerouslySkipPermissions`. */
  bypassAllowed = false;
  /**
   * Background work running in the process (subagents, shells, workflows), by task id: Claude
   * Code's own level signal (`background_tasks_changed`, replace semantics), not paired edges.
   * Ambient tasks (housekeeping watchers) are left out. The process must not be closed while
   * any runs (D160): closing it kills them.
   */
  readonly background = new Map<string, BackgroundTask>();
  readonly commands = new ClaudeCommands();
  commandOwners = new Set<NativeId>();
  /** A re-open wanted once the background work ends (why, for the log), and folders it adds. */
  reopenWanted: string | null = null;
  readonly wantedDirs = new Set<string>();
  lastUsed = Date.now();
  idleTimer: NodeJS.Timeout | null = null;
  closing = false;
  stderrTail = "";
  translator: LiveTranslator;
  /** The settings the query is running with. */
  model: string | null;
  effort: string | null;
  mode: string;
  /** Fast mode as last given to the query (its `fastMode` flag setting). */
  fast: boolean;
  /** Subagents this query ran (D151), by agent id (= their task id), and by the Agent tool call. */
  readonly subagents = new Map<string, LiveSubagent>();
  readonly subagentByToolUse = new Map<string, string>();

  /**
   * How many of this query's subagents are running now (from its task events, D151): a
   * process with any is not idle. The one place that answers it, for the lifecycle code too.
   */
  runningSubagents(): number {
    let n = 0;
    for (const s of this.subagents.values()) if (s.running) n++;
    return n;
  }

  constructor(
    readonly native: NativeId,
    readonly cwd: string,
    readonly dirs: Set<string>,
    settings: ThreadSettings,
    emit: (ev: ItemEvent) => void,
    storeImage?: StoreToolImage,
  ) {
    this.model = settings.model ?? null;
    this.effort = settings.effort ?? null;
    this.mode = settings.mode ?? "default";
    this.fast = settings.fast === true;
    this.translator = new LiveTranslator(emit, cwd, storeImage);
  }

  hasQueuedPeer(): boolean {
    return [...this.peerCommands.values()].some((command) => !command.started && !command.expired);
  }

  busy(): boolean {
    return this.turn !== null || this.requests.size > 0 || this.background.size > 0 || this.hasQueuedPeer();
  }

  /** Whether closing the process now would kill background work. */
  hasBackground(): boolean {
    return this.background.size > 0;
  }
}

export interface ClaudeProviderOptions {
  /** The SDK, or how to load it. Tests pass a stub. */
  sdk?: ClaudeSdk | (() => Promise<ClaudeSdk>);
  /** The `claude` command line. Tests pass a stub. */
  cli?: ClaudeCli;
  /** The installed `claude` (resolved), or null when none was found. */
  executable: string | null;
  /** Environment for Claude Code processes. */
  env: NodeJS.ProcessEnv;
  /** Where the per-thread settings are kept. */
  stateDir: string;
  storeToolImage?: StoreToolImage;
  /** Close a live query after this long without a turn, an open request, or background work. 0: never. */
  idleMs?: number;
  /** At most this many live queries; the least recently used idle one is closed first. */
  maxLive?: number;
  /** How long one `claude agents --json` answer is reused. */
  controlCacheMs?: number;
  /** A session's subagent files (D151). Tests pass a stub; default: read from the config folder. */
  subagentFiles?: (configDir: string, sessionId: string) => Map<string, SubagentFile>;
  log?: (line: string) => void;
}

const iso = (ms: number | undefined | null): string => new Date(typeof ms === "number" && Number.isFinite(ms) ? ms : Date.now()).toISOString();

// ── Subagents (D151) ─────────────────────────────────────────────────────────────────────

/** Why YAAC does not send to a Claude Code subagent (verified on 2.1.330, P192). */
export const CLAUDE_SUBAGENT_READ_ONLY =
  "Only the agent that started a Claude Code subagent can continue it (its SendMessage tool); Claude Code offers no way to message one from outside. Message that agent instead.";

/** A subagent stored without an ending is taken as still running while its transcript changed this recently. */
export const SUBAGENT_LIVE_MS = 2 * 60 * 1000;

/**
 * A subagent's native id: its parent session and its agent id, as Claude Code stores it
 * (`<sessionId>/subagents/agent-<agentId>.jsonl`). Stable for its whole life.
 */
export function subagentNative(sessionId: string, agentId: string): NativeId {
  return `${sessionId}/agent-${agentId}`;
}

/** The parts of a subagent's native id; null for a session. */
export function parseSubagentNative(native: NativeId): { sessionId: string; agentId: string } | null {
  const m = /^([^/]+)\/agent-([^/]+)$/.exec(native);
  return m ? { sessionId: m[1]!, agentId: m[2]! } : null;
}

/** What one live subagent of a running query is (from its task events). */
interface LiveSubagent {
  native: NativeId;
  agentId: string;
  toolUseId: string | null;
  agentType: string | null;
  description: string;
  prompt: string | null;
  turnId: TurnId;
  createdAt: string;
  translator: LiveTranslator;
  running: boolean;
}

/** Claude Code's per-model `supportsFastMode` as a three-valued support (absent: unknown). */
export function fastSupport(v: boolean | undefined): "supported" | "unsupported" | "unknown" {
  return v === true ? "supported" : v === false ? "unsupported" : "unknown";
}

function sniffImage(head: Buffer): string | null {
  if (head.length >= 8 && head.subarray(0, 8).equals(Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]))) return "image/png";
  if (head.length >= 3 && head[0] === 0xff && head[1] === 0xd8 && head[2] === 0xff) return "image/jpeg";
  if (head.length >= 6 && (head.subarray(0, 6).toString("latin1") === "GIF87a" || head.subarray(0, 6).toString("latin1") === "GIF89a")) return "image/gif";
  if (head.length >= 12 && head.subarray(0, 4).toString("latin1") === "RIFF" && head.subarray(8, 12).toString("latin1") === "WEBP") return "image/webp";
  return null;
}

function usageOf(u: unknown): TokenUsage | null {
  if (!u || typeof u !== "object") return null;
  const r = u as Record<string, unknown>;
  const n = (k: string) => (typeof r[k] === "number" ? (r[k] as number) : null);
  const input = n("input_tokens");
  const output = n("output_tokens");
  const cached = n("cache_read_input_tokens");
  const created = n("cache_creation_input_tokens");
  const totalIn = (input ?? 0) + (cached ?? 0) + (created ?? 0);
  return { input: totalIn, cachedInput: cached, output, total: totalIn + (output ?? 0) };
}

export class ClaudeProvider implements HarnessProvider {
  readonly events = new Emitter<ProviderEvent>();
  private sdkPromise: Promise<ClaudeSdk> | null = null;
  private readonly cli: ClaudeCli | null;
  private readonly live = new Map<NativeId, Live>();
  private settings: Record<NativeId, ThreadSettings>;
  private readonly stateFile: string;
  private readonly idleMs: number;
  private readonly maxLive: number;
  private readonly controlCacheMs: number;
  private readonly log: (line: string) => void;
  private sdkModels: SdkModelInfo[] = [];
  private models: ModelInfo[] = [];
  private lastProbe: ProbeResult | null = null;
  private extrasHook: ClaudeExtrasHook | null = null;
  /** The agent mesh is attached (the `meshTools` capability). */
  private meshAttached = false;
  /** Pids of Claude Code processes yaacd ran, kept a while after they end (their registry entry can linger). */
  private ownPids = new Map<number, number>();
  private agentsCache: { at: number; entries: Promise<AgentEntry[]> } | null = null;
  private scanSnapshot: { id: string; threads: NativeThreadInfo[] } | null = null;
  private disposed = false;
  /** Claude Code's configuration folder (where `projects/` is), as the last probe found it. */
  private configDir: string;
  /** The account and its rate-limit windows (D161). */
  private readonly limits = new ClaudeRateLimits((report) => this.emit({ type: "rateLimitsChanged", report }));

  constructor(readonly options: ClaudeProviderOptions) {
    this.cli = options.cli ?? (options.executable ? realCli(options.executable, options.env) : null);
    this.idleMs = options.idleMs ?? DEFAULT_IDLE_MS;
    this.maxLive = Math.max(1, options.maxLive ?? DEFAULT_MAX_LIVE);
    this.controlCacheMs = options.controlCacheMs ?? 1000;
    this.log = options.log ?? (() => {});
    fs.mkdirSync(options.stateDir, { recursive: true, mode: 0o700 });
    this.stateFile = path.join(options.stateDir, "threads.json");
    this.settings = readJson<Record<NativeId, ThreadSettings>>(this.stateFile) ?? {};
    this.configDir = options.env.CLAUDE_CONFIG_DIR ?? path.join(options.env.HOME ?? os.homedir(), ".claude");
  }

  // ── Hooks and test controls ────────────────────────────────────────────────────────────

  /** The mesh hook: extra MCP servers and system-prompt text, per thread, on every open. */
  setThreadExtras(hook: ClaudeExtrasHook | null): void {
    this.extrasHook = hook;
  }

  /**
   * The agent mesh (WP7): every query a thread opens gets the `yaac` tools as an in-process
   * MCP server (`type: "sdk"`) carrying that thread's attachment ticket, the identity block
   * appended to the system prompt, and the server's tools allowed without a permission prompt
   * (what agents may do with each other is governed by the mesh's own permission and budget).
   */
  attachMesh(hooks: MeshHooks | null): void {
    this.meshAttached = !!hooks;
    if (!hooks) {
      this.setThreadExtras(null);
      return;
    }
    this.setThreadExtras((t) => {
      const att = hooks.open({ native: t.native, cwd: t.cwd, phase: t.resumed ? "resume" : "start" });
      if (!att) return undefined;
      const server = { type: "sdk", name: MESH_SERVER, instance: att.inProcessServer(), timeout: att.toolTimeoutSec * 1000 } as unknown as McpServerConfig;
      return { mcpServers: { [MESH_SERVER]: server }, appendSystemPrompt: att.identity, allowedTools: [`mcp__${MESH_SERVER}`] };
    });
  }

  /** Native ids of the threads that have a live query now, least recently used first. */
  liveThreads(): NativeId[] {
    return [...this.live.values()].sort((a, b) => a.lastUsed - b.lastUsed).map((l) => l.native);
  }

  // ── Plumbing ───────────────────────────────────────────────────────────────────────────

  private sdk(): Promise<ClaudeSdk> {
    if (!this.sdkPromise) {
      const s = this.options.sdk;
      this.sdkPromise = typeof s === "function" ? s() : s ? Promise.resolve(s) : loadRealSdk(() => this.configDir);
      this.sdkPromise.catch(() => {
        this.sdkPromise = null;
      });
    }
    return this.sdkPromise;
  }

  private emit(event: ProviderEvent): void {
    if (!this.disposed) this.events.emit(event);
  }

  private save(): void {
    writeJsonAtomic(this.stateFile, this.settings);
  }

  private requireExecutable(): string {
    if (!this.options.executable) throw new Error("Claude Code (`claude`) is not installed on this target");
    return this.options.executable;
  }

  private async threadSettings(native: NativeId): Promise<ThreadSettings> {
    let s = this.settings[native];
    if (!s) {
      const info = await (await this.sdk()).getSessionInfo(native).catch(() => undefined);
      if (!info) throw new Error(`no Claude Code session ${native} on this target`);
      s = { cwd: info.cwd ?? os.homedir(), stored: true };
      this.settings[native] = s;
      this.save();
    }
    return s;
  }

  // ── Probe ──────────────────────────────────────────────────────────────────────────────

  private baseCapabilities(): Capability[] {
    return [
      "threadList",
      "threadRead",
      "threadResume",
      "threadFork",
      "threadForkAtTurn",
      "threadRename",
      "threadArchive",
      "threadDelete",
      "threadTakeOver",
      "streamText",
      "streamReasoning",
      "interrupt",
      "compact",
      "approvals",
      "userQuestions",
      "modes",
      "modelSwitch",
      "effortSwitch",
      "imageInput",
      "reasoningSummary",
      "tokenUsage",
      "modelList",
      "plan",
      "subagents",
      "subagentThreads",
      "webSearch",
      // `/name` at the start of a message runs a skill; the list is read from disk (`skills/list`).
      "skills",
      "nativeFileMentions",
      // accountInfo(), the usage report, and the stream's rate_limit_event (D161).
      "rateLimits",
    ];
  }

  private result(status: ProbeResult["status"], message: string | null, extra: Partial<ProbeResult> = {}): ProbeResult {
    return {
      displayName: "Claude",
      backend: "sdk",
      harnessVersion: null,
      store: { key: null },
      status,
      statusMessage: message,
      capabilities: [],
      models: [],
      modelsComplete: false,
      modes: [],
      ...extra,
    };
  }

  async probe(): Promise<ProbeResult> {
    const exe = this.options.executable;
    if (!exe || !this.cli) return this.result("notInstalled", "Claude Code (`claude`) was not found on this target");
    const version = await this.cli.version();
    if (!version) return this.result("unreachable", `Claude Code at ${exe} could not be run`);
    const auth = await this.cli.authStatus();
    const configDir = auth?.configDirectory ?? this.options.env.CLAUDE_CONFIG_DIR ?? path.join(this.options.env.HOME ?? os.homedir(), ".claude");
    this.configDir = configDir;
    const store = { key: claudeStoreKey(configDir), home: configDir };
    if (auth && !auth.loggedIn) {
      return this.result("needsAuth", "Claude Code is not signed in on this target; run `claude auth login` there", { harnessVersion: version, store });
    }
    const sdk = await this.sdk();
    const input = new InputQueue();
    const q = sdk.query({
      prompt: input,
      options: {
        cwd: this.options.stateDir,
        pathToClaudeCodeExecutable: exe,
        env: this.options.env,
        persistSession: false,
        permissionMode: "default",
        canUseTool: async () => ({ behavior: "deny", message: "yaacd is only reading the model list" }),
      },
    });
    const drain = (async () => {
      try {
        for await (const _ of q) void _;
      } catch {
        /* the probe query is closed below */
      }
    })();
    let models: SdkModelInfo[];
    let account: Awaited<ReturnType<SdkQuery["accountInfo"]>> | null = null;
    let applied: { model?: string | null; effort?: string | null } | null = null;
    let harnessVersion = version;
    try {
      const timeout = new Promise<never>((_, reject) => setTimeout(() => reject(new Error("Claude Code did not answer within 60 s")), 60000).unref());
      models = await Promise.race([q.supportedModels(), timeout]);
      account = await Promise.race([q.accountInfo(), timeout]).catch(() => null);
      const init = await Promise.race([q.initializationResult(), timeout]).catch(() => null);
      if (typeof init?.claude_code_version === "string") harnessVersion = init.claude_code_version;
      if (typeof q.getSettings === "function") applied = (await Promise.race([q.getSettings(), timeout]).catch(() => null))?.applied ?? null;
      await this.limits.readFrom(q, 15000, this.log);
    } catch (e) {
      return this.result("unreachable", `Claude Code did not report its models: ${(e as Error).message}`, { harnessVersion: version, store });
    } finally {
      input.end();
      q.close();
      await Promise.race([drain, new Promise((r) => setTimeout(r, 2000).unref())]);
    }
    this.sdkModels = models;
    // The model a thread gets when none is chosen: the one Claude Code says it applies by
    // default (matched by its value first, then by what an alias resolves to).
    const defaultModel = applied?.model
      ? (models.find((m) => m.value === applied!.model) ?? models.find((m) => m.resolvedModel === applied!.model) ?? null)
      : null;
    this.models = models.map((m) => {
      const efforts = (m.supportedEffortLevels ?? []).map((id) => ({ id: String(id) }));
      const isDefault = m === defaultModel;
      return {
        id: m.value,
        ...(m.resolvedModel ? { resolvedModel: m.resolvedModel } : {}),
        displayName: m.displayName,
        description: m.description ?? null,
        isDefault,
        // The SDK's model info has no image-input field. Claude Code takes images as content
        // blocks, so image input is reported as supported for every model: a statement about
        // the harness, and the only such default in this provider (docs/04-providers.md#claude).
        // Fast mode is what Claude Code reports per model (`supportsFastMode`); absent: unknown.
        capabilities: [{ capability: "imageInput", support: "supported" }, { capability: "fastMode", support: fastSupport(m.supportsFastMode) }],
        efforts,
        defaultEffort: isDefault && applied?.effort && efforts.some((e) => e.id === applied!.effort) ? applied.effort : null,
      } satisfies ModelInfo;
    });
    const capabilities = this.baseCapabilities();
    if (this.meshAttached) capabilities.push("meshTools");
    if (this.models.some((m) => (m.efforts ?? []).length > 0)) capabilities.push("reasoning");
    const autoMode = models.some((m) => m.supportsAutoMode === true);
    const modes = MODES.filter((m) => !m.needsAutoMode || autoMode).map(({ needsAutoMode: _n, ...m }) => m as ModeInfo);
    this.lastProbe = this.result("ready", null, {
      harnessVersion,
      store: { ...store, account: account?.email ?? account?.organization ?? null },
      capabilities,
      models: this.models,
      modelsComplete: true,
      modes,
    });
    return this.lastProbe;
  }

  /** The skills Claude Code would find for this folder: its project's and the user's. */
  async skills(cwd: string): Promise<SkillsListResult> {
    return claudeSkills(cwd, this.configDir, this.options.env.HOME ?? this.options.env.USERPROFILE ?? os.homedir());
  }

  /**
   * The last account and windows. A refresh reads them with a short query of its own, but
   * only when Claude Code said nothing for 15 minutes: a turn's events keep them current.
   */
  async rateLimits(opts: { refresh: boolean }): Promise<RateLimitReport | null> {
    const exe = this.options.executable;
    if (opts.refresh && exe && Date.now() - this.limits.lastSeen >= CLAUDE_LIMITS_QUIET_MS && this.lastProbe?.status === "ready") {
      await readWithOwnQuery(await this.sdk(), { cwd: this.options.stateDir, pathToClaudeCodeExecutable: exe, env: this.options.env }, this.limits, this.log);
    }
    return this.limits.report();
  }

  // ── Thread index and history ───────────────────────────────────────────────────────────

  async scanThreads(page: ScanRequest): Promise<ThreadPage> {
    const sdk = await this.sdk();
    const size = Math.max(1, page.limit ?? 100);
    let snap = this.scanSnapshot;
    let offset = 0;
    if (page.cursor) {
      const [id, off] = page.cursor.split(":");
      if (!snap || snap.id !== id) throw new Error("the listing changed while it was being read; start again");
      offset = Number.parseInt(off ?? "0", 10) || 0;
    } else {
      // The SDK labels no session as programmatic; asking without them tells them apart.
      // Sessions come newest first (by last modification). Under the discovery caps only the
      // first `max` are asked for: the interactive ones among them are within the first `max`
      // of the interactive listing too, so the two listings still tell them apart.
      const limit = page.max && page.max > 0 ? { limit: page.max } : {};
      const [all, interactive] = await Promise.all([sdk.listSessions({ includeWorktrees: true, ...limit }), sdk.listSessions({ includeWorktrees: true, includeProgrammatic: false, ...limit })]);
      const human = new Set(interactive.map((s) => s.sessionId));
      snap = {
        id: randomUUID().slice(0, 8),
        threads: all.map((s) => ({
          native: s.sessionId,
          title: s.customTitle ?? s.summary ?? null,
          preview: previewText(s.firstPrompt),
          cwd: s.cwd ?? this.settings[s.sessionId]?.cwd ?? "",
          model: this.settings[s.sessionId]?.model ?? null,
          createdAt: iso(s.createdAt ?? s.lastModified),
          updatedAt: iso(s.lastModified),
          originKind: human.has(s.sessionId) ? "interactive" : "programmatic",
          archived: s.archived === true,
        })) as NativeThreadInfo[],
      };
      if (page.activeSince) snap.threads = snap.threads.filter((t) => t.updatedAt >= page.activeSince!);
      if (page.search) snap.threads = snap.threads.filter((t) => (t.title ?? "").toLowerCase().includes(page.search!.toLowerCase()));
      // Each listed session's subagents (D151), after the sessions: yaacd files them under
      // their parent and caps them by its own rules.
      if (!page.search) {
        const kids: NativeThreadInfo[] = [];
        for (const parent of snap.threads) kids.push(...(await this.storedSubagents(sdk, parent)));
        snap.threads.push(...kids);
      }
      this.scanSnapshot = snap;
    }
    const threads = snap.threads.slice(offset, offset + size);
    const next = offset + size < snap.threads.length ? `${snap.id}:${offset + size}` : null;
    return { threads, cursor: next, complete: true };
  }

  /** A session's stored subagents (D151), as conversations of their own under it. */
  private async storedSubagents(sdk: ClaudeSdk, parent: NativeThreadInfo): Promise<NativeThreadInfo[]> {
    if (typeof sdk.listSubagents !== "function") return [];
    let ids: string[];
    try {
      ids = await sdk.listSubagents(parent.native);
    } catch (e) {
      this.log(`listing the subagents of ${parent.native} failed: ${(e as Error).message}`);
      return [];
    }
    if (ids.length === 0) return [];
    const files = (this.options.subagentFiles ?? subagentFiles)(this.configDir, parent.native);
    return ids.map((agentId) => this.subagentInfo(parent, agentId, files.get(agentId) ?? null));
  }

  /** What is known of one subagent: its live task when this query runs it, else its files. */
  private subagentInfo(parent: Pick<NativeThreadInfo, "native" | "cwd" | "updatedAt" | "archived">, agentId: string, file: SubagentFile | null): NativeThreadInfo {
    const native = subagentNative(parent.native, agentId);
    const live = this.live.get(parent.native)?.subagents.get(agentId);
    const meta = file?.meta ?? null;
    let status: SubagentStatus;
    if (live) status = live.running ? "running" : "completed";
    else if (meta?.finished || file?.outcome === "completed") status = "completed";
    else if (file?.outcome === "failed") status = "failed";
    else if (meta?.stoppedByUser) status = "stopped";
    // No ending stored: running while its transcript is still being written (a session in a
    // terminal), else it ended without saying (its process went away).
    else status = file && Date.now() - file.modifiedMs < SUBAGENT_LIVE_MS ? "running" : "stopped";
    const agentType = live?.agentType ?? meta?.agentType ?? null;
    const title = live?.description ?? meta?.description ?? agentType ?? "Subagent";
    return {
      native,
      title,
      preview: live?.prompt ? previewText(live.prompt) : title,
      cwd: parent.cwd,
      model: meta?.model ?? null,
      createdAt: live?.createdAt ?? (file ? iso(file.createdMs) : parent.updatedAt),
      updatedAt: file ? iso(file.modifiedMs) : live ? new Date().toISOString() : parent.updatedAt,
      originKind: "programmatic",
      archived: parent.archived ?? false,
      parent: parent.native,
      subagent: { agentId, ...(agentType ? { agentType } : {}), status, readOnly: CLAUDE_SUBAGENT_READ_ONLY },
    };
  }

  async readThread(thread: NativeId, page: PageRequest): Promise<ItemPage> {
    const sdk = await this.sdk();
    const sub = parseSubagentNative(thread);
    const messages = sub ? await sdk.getSubagentMessages(sub.sessionId, sub.agentId) : await sdk.getSessionMessages(thread);
    const start = Math.max(0, Number.parseInt(page.cursor ?? "0", 10) || 0);
    const size = Math.max(1, page.limit ?? 200);
    const items = historyItems(messages, this.settings[sub?.sessionId ?? thread]?.cwd ?? null, {
      subagent: !!sub,
      storeImage: this.options.storeToolImage,
      imageRange: { start, end: start + size },
    });
    return { items: items.slice(start, start + size), cursor: start + size < items.length ? String(start + size) : null };
  }

  private async refreshInfo(native: NativeId): Promise<void> {
    try {
      const info = await (await this.sdk()).getSessionInfo(native);
      if (!info) return;
      const s = this.settings[native];
      this.emit({
        type: "threadChanged",
        info: {
          native,
          title: info.customTitle ?? info.summary ?? null,
          preview: previewText(info.firstPrompt),
          cwd: info.cwd ?? s?.cwd ?? "",
          model: s?.model ?? null,
          createdAt: iso(info.createdAt ?? info.lastModified),
          updatedAt: iso(info.lastModified),
          originKind: "programmatic",
          archived: info.archived === true,
        },
      });
    } catch {
      /* a listing will catch up */
    }
  }

  // ── Live queries ───────────────────────────────────────────────────────────────────────

  /**
   * The folders Claude Code must be able to read for this input. They are fixed when a query
   * opens, so the thread's attachment folder (`uploadsDir`, created by yaacd) is among them from
   * the first open on and is kept in the thread's settings: a later attachment then lies in a
   * folder the query already has, and never forces a re-open (D160).
   */
  private attachmentDirs(native: NativeId, input: ProviderInput): string[] {
    const s = this.settings[native];
    const real = (dir: string) => {
      try {
        return fs.realpathSync(dir);
      } catch {
        return path.resolve(dir);
      }
    };
    if (s && input.uploadsDir) {
      const up = real(input.uploadsDir);
      if (s.uploads !== up) {
        s.uploads = up;
        this.save();
      }
    }
    const dirs = new Set<string>([...(s?.readable ?? []), ...(s?.uploads ? [s.uploads] : []), ...(input.readable ?? [])]);
    for (const p of [...input.images.map((i) => i.path), ...input.files.map((f) => f.path)]) dirs.add(real(path.dirname(p)));
    return [...dirs];
  }

  /**
   * Makes room under the cap: closes least recently used idle queries. One with a turn, an open
   * request or background work is never a victim (D160); when all are, the cap is exceeded.
   */
  private makeRoom(): void {
    while (this.live.size >= this.maxLive) {
      const victim = [...this.live.values()].filter((l) => !l.busy()).sort((a, b) => a.lastUsed - b.lastUsed)[0];
      if (!victim) {
        const bg = [...this.live.values()].filter((l) => l.hasBackground()).length;
        this.log(`${this.live.size} Claude Code processes are busy${bg > 0 ? ` (${bg} with background work)` : ""}; opening one more than the limit of ${this.maxLive}`);
        return;
      }
      this.closeLive(victim, "the limit of live Claude Code processes");
    }
  }

  private closeLive(live: Live, why: string): void {
    if (live.closing) return;
    this.log(`closing the query of ${live.native} (${why})`);
    live.closing = true;
    if (live.idleTimer) clearTimeout(live.idleTimer);
    live.idleTimer = null;
    this.live.delete(live.native);
    if (live.pid !== null) this.ownPids.set(live.pid, Date.now());
    live.input.end();
    try {
      live.query.close();
    } catch {
      /* already gone */
    }
  }

  private touch(live: Live): void {
    live.lastUsed = Date.now();
    if (live.idleTimer) clearTimeout(live.idleTimer);
    live.idleTimer = null;
    if (this.idleMs > 0 && !live.busy()) {
      live.idleTimer = setTimeout(() => {
        live.idleTimer = null;
        if (!live.busy() && this.live.get(live.native) === live) this.closeLive(live, "idle");
      }, this.idleMs);
      live.idleTimer.unref();
    }
  }

  /**
   * The live query of a thread, opened as needed. A query is re-opened (with `resume`) only while
   * it is not busy — no turn, no open request, no background work (D160) — for a readable folder
   * it lacks (rare: the attachment folder is there from the first open) or for a change that was
   * deferred while background work ran. Otherwise the query is kept as it is.
   */
  private async ensureLive(native: NativeId, dirs: string[]): Promise<Live> {
    const existing = this.live.get(native);
    if (!existing) return this.openLive(native, new Set(dirs));
    const missing = dirs.filter((d) => !existing.dirs.has(d));
    if (missing.length === 0 && !existing.reopenWanted) return existing;
    if (existing.busy()) {
      if (missing.length > 0) {
        for (const d of missing) existing.wantedDirs.add(d);
        this.log(`not re-opening the query of ${native} for a new readable folder while it is busy (${existing.background.size} background task(s)); Claude Code asks before reading there`);
      }
      return existing;
    }
    const all = new Set([...existing.dirs, ...existing.wantedDirs, ...dirs]);
    this.closeLive(existing, missing.length > 0 ? "a new readable folder" : (existing.reopenWanted ?? "a deferred change"));
    return this.openLive(native, all);
  }

  private async openLive(native: NativeId, dirs: Set<string>): Promise<Live> {
    const exe = this.requireExecutable();
    const sdk = await this.sdk();
    const s = await this.threadSettings(native);
    this.makeRoom();
    const live = new Live(native, s.cwd, dirs, s, (ev) => this.emit({ ...ev, thread: native } as ProviderEvent), this.options.storeToolImage);
    const resumed = s.stored === true;
    const extras = (await this.extrasHook?.({ native, cwd: s.cwd, resumed })) ?? {};
    // Allowed (never entered unless the thread's mode is bypass), so that switching to bypass
    // later is `setPermissionMode` on the live query, not a re-open — except as root, where
    // Claude Code refuses to start with it (see BYPASS_MODE).
    live.bypassAllowed = live.mode === BYPASS_MODE || !runsAsRoot();
    const options: Options = {
      cwd: s.cwd,
      pathToClaudeCodeExecutable: exe,
      env: this.options.env,
      includePartialMessages: true,
      // Subagents' own text and thinking too, so that each one's transcript streams (D151).
      forwardSubagentText: true,
      ...(resumed ? { resume: native } : { sessionId: native }),
      ...(live.model ? { model: live.model } : {}),
      ...(live.effort ? { effort: live.effort as NonNullable<Options["effort"]> } : {}),
      // Fast mode (D125) is Claude Code's `fastMode` setting; set in the flag layer it is also
      // the Agent SDK's opt-in to fast mode.
      ...(live.fast ? { settings: { fastMode: true } } : {}),
      permissionMode: live.mode as PermissionMode,
      ...(live.bypassAllowed ? { allowDangerouslySkipPermissions: true } : {}),
      // Acts only while the query is in bypass mode (`askHook`); hooks are fixed at open.
      hooks: { PreToolUse: [{ matcher: ASK_TOOL, hooks: [(h, id, o) => this.askHook(live, h, id, o.signal)] }] },
      additionalDirectories: [...dirs],
      systemPrompt: { type: "preset", preset: "claude_code", ...(extras.appendSystemPrompt ? { append: extras.appendSystemPrompt } : {}) },
      ...(extras.mcpServers && Object.keys(extras.mcpServers).length > 0 ? { mcpServers: extras.mcpServers } : {}),
      ...(extras.allowedTools && extras.allowedTools.length > 0 ? { allowedTools: extras.allowedTools } : {}),
      canUseTool: (toolName, input, opts) => this.canUseTool(live, toolName, input, opts),
      onElicitation: (request, opts) => this.onElicitation(live, request, opts.signal),
      stderr: (data) => {
        live.stderrTail = (live.stderrTail + data).slice(-2000);
      },
    };
    live.query = sdk.query({ prompt: live.input, options });
    this.live.set(native, live);
    if (!resumed) {
      s.stored = true;
      this.save();
    }
    live.pidKnown = live.query
      .initializationResult()
      .then((r) => {
        live.peerInput = supportsPeerInput(r?.claude_code_version ?? this.lastProbe?.harnessVersion, [2, 1, 330]);
        if (typeof r?.pid === "number") {
          live.pid = r.pid;
          this.ownPids.set(r.pid, Number.POSITIVE_INFINITY);
        }
      })
      .catch(() => {});
    void this.consume(live);
    return live;
  }

  private async consume(live: Live): Promise<void> {
    let failure: string | null = null;
    try {
      for await (const m of live.query) this.onMessage(live, m);
    } catch (e) {
      failure = (e as Error).message;
    }
    for (const command of live.peerCommands.values()) command.acknowledge(new InputOutcomeUnknown(failure ?? "the Claude Code process closed during peer delivery"));
    live.peerCommands.clear();
    if (live.pid !== null) this.ownPids.set(live.pid, Date.now());
    const wasLive = this.live.get(live.native) === live;
    if (wasLive) this.live.delete(live.native);
    if (live.idleTimer) clearTimeout(live.idleTimer);
    // Its subagents end with it (D151).
    for (const sub of live.subagents.values()) if (sub.running) this.endSubagent(live, sub, "stopped", "the Claude Code process ended");
    // The process is gone: nothing it asked can be answered any more.
    for (const p of [...live.requests.values()]) p.expire();
    // So is any background work it ran: said in the thread, never lost silently (D160).
    if (live.background.size > 0) {
      const why = live.closing ? "was closed" : "ended";
      this.log(`the Claude Code process of ${live.native} ${why} with ${live.background.size} background task(s) running`);
      this.notice(live, "warning", `Claude Code's process ${why} while background work was running in it; this stopped: ${describeTasks([...live.background.values()])}.`);
      live.background.clear();
    }
    live.commands.active.clear();
    this.publishCommands(live);
    if (live.turn) {
      if (live.turn.compact && !live.turn.interrupting) live.turn.compact.connectionLost = true;
      const detail = failure ?? (live.stderrTail.trim().split("\n").pop() || null);
      const why = live.closing ? "the Claude Code process was closed" : "the Claude Code process ended";
      this.finishTurn(live, live.turn.interrupting ? "interrupted" : "failed", detail ? `${why}: ${detail}` : why, null);
    }
  }

  private beginTurn(live: Live, turnId: TurnId, origin: TurnOrigin): TurnState {
    let end!: () => void;
    const ended = new Promise<void>((r) => (end = r));
    const turn: TurnState = { turnId, origin, interrupting: false, ended, end };
    live.turn = turn;
    live.translator.begin(turnId);
    if (live.idleTimer) clearTimeout(live.idleTimer);
    live.idleTimer = null;
    this.emit({ type: "turnStarted", thread: live.native, turnId, origin, at: new Date().toISOString() });
    return turn;
  }

  /** Work Claude Code started on its own (a background task finishing, say) gets a turn of its own. */
  private ensureTurn(live: Live, turnId?: string): TurnState {
    return live.turn ?? this.beginTurn(live, turnId ?? `auto-${randomUUID()}`, "external");
  }

  private finishTurn(live: Live, status: TurnStatus, error: string | null, usage: TokenUsage | null): void {
    const turn = live.turn;
    if (!turn) return;
    if (turn.compact && status !== "completed") turn.compact.error = error ?? "Claude Code interrupted compaction";
    for (const p of [...live.requests.values()]) if (p.turnId === turn.turnId) p.expire();
    live.translator.finish(status === "completed");
    live.asking.clear();
    live.turn = null;
    this.emit({ type: "turnCompleted", thread: live.native, turnId: turn.turnId, status, error, usage, at: new Date().toISOString() });
    turn.end();
    this.touch(live);
    void this.refreshInfo(live.native);
  }

  private onMessage(live: Live, m: SDKMessage): void {
    // Emitted by the SDK on the streaming CLI's stdout, absent from SDKMessage.
    const lifecycle = m as unknown as { type: string; command_uuid: string; state: string };
    if (lifecycle.type === "command_lifecycle") {
      const { command_uuid: id, state } = lifecycle;
      const command = live.peerCommands.get(id);
      if (!command) return;
      if (state === "refused" || state === "discarded" || state === "cancelled") {
        command.acknowledge(state === "cancelled"
          ? new InputOutcomeUnknown("Claude Code cancelled the queued peer command")
          : new PeerInputDenied("Claude Code declined the message under the recipient’s cross-session policy"));
        live.peerCommands.delete(id);
        if (!live.busy()) this.touch(live);
        return;
      }
      if (state === "queued" || state === "started" || state === "completed") command.acknowledge();
      if (state === "started") {
        command.started = true;
        if (!live.turn) {
          command.beforeFollowOn?.();
          this.beginTurn(live, id, "mesh");
        }
      }
      if (state === "completed") live.peerCommands.delete(id);
      return;
    }
    if (live.commands.message(m)) this.publishCommands(live);
    if (m.type === "rate_limit_event") return this.limits.applyEvent(m.rate_limit_info);
    const msg = m as SDKMessage & { parent_tool_use_id?: string | null; uuid?: string };
    // Inside a subagent (D151): its own transcript, not this one's (the Agent tool call stands for it here).
    if (msg.parent_tool_use_id || (m as { agent_id?: unknown }).agent_id) {
      this.onSubagentMessage(live, m);
      return;
    }
    switch (m.type) {
      case "stream_event":
        this.ensureTurn(live);
        live.translator.stream(m.event as unknown as Record<string, unknown>);
        return;
      case "assistant": {
        const turn = this.ensureTurn(live);
        // Claude's local command refusal can arrive as a synthetic assistant message and
        // even be followed by a successful result. Keep its actual error for the reply.
        if (turn.compact && !turn.compact.confirmed && m.message.model === "<synthetic>") {
          const text = m.message.content.flatMap((block) => block.type === "text" ? [block.text] : []).join("\n").trim();
          if (text.startsWith("Error:")) {
            turn.compact.error = text;
            return;
          }
        }
        live.translator.assistant(m.message as unknown as Record<string, unknown>, m.uuid, m.error ?? null);
        return;
      }
      case "user": {
        const content = (m.message as { content?: unknown }).content;
        const uuid = typeof msg.uuid === "string" ? msg.uuid : null;
        if (uuid && live.ours.has(uuid)) return;
        if ((m as { isReplay?: boolean }).isReplay) return;
        this.ensureTurn(live, uuid ?? undefined);
        live.translator.user(content, uuid, (m as { origin?: unknown }).origin, (m as { tool_use_result?: unknown }).tool_use_result, flagsOf(m));
        return;
      }
      case "result": {
        const turn = live.turn;
        if (!turn) return;
        const usage = usageOf(m.usage);
        // Asked for fast mode, and Claude Code says the turn did not get it (D125): said in
        // the transcript, with Claude Code's reason, never hidden.
        const fms = (m as { fast_mode_state?: string; fast_mode_disabled_reason?: string });
        if (live.fast && typeof fms.fast_mode_state === "string" && fms.fast_mode_state !== "on") {
          const why = fms.fast_mode_disabled_reason ? ` (${fms.fast_mode_disabled_reason})` : fms.fast_mode_state === "cooldown" ? " (cooling down after a rate limit)" : "";
          const item = { id: `notice:fast:${turn.turnId}`, turnId: turn.turnId, status: "completed", kind: "notice", noticeKind: "warning", text: `Claude Code did not run this turn in fast mode${why}.${fms.fast_mode_state === "cooldown" ? "" : " Fast mode is now off for this thread."}` } as const;
          this.emit({ type: "itemStarted", thread: live.native, item });
          this.emit({ type: "itemCompleted", thread: live.native, item });
          // A refusal (not a cooldown, which passes): fast mode goes off for this thread.
          // (`live.fast` stays as it is: the next turn, asked without fast mode, switches the
          // query's setting off through `applyFlagSettings`.)
          if (fms.fast_mode_state !== "cooldown") this.emit({ type: "fastRefused", thread: live.native });
        }
        if (m.subtype === "success" && !m.is_error && turn.compact && !turn.compact.confirmed) {
          this.finishTurn(live, "failed", turn.compact.error ?? "Claude Code did not confirm compaction", usage);
        } else if (m.subtype === "success" && !m.is_error) this.finishTurn(live, "completed", null, usage);
        else if (turn.interrupting) this.finishTurn(live, "interrupted", null, usage);
        else {
          const errors = "errors" in m && Array.isArray(m.errors) ? m.errors.join("; ") : "";
          this.finishTurn(live, "failed", errors || ("result" in m ? String(m.result) : m.subtype), usage);
        }
        return;
      }
      case "system": {
        const sys = m as {
          subtype?: string; model?: string; permissionMode?: string; uuid?: string; capabilities?: string[];
          message_uuid?: string; state?: string;
        };
        if (sys.subtype === "init") {
          live.cancelQueued = sys.capabilities?.includes("interrupt_cancel_queued_v1") === true;
          const applied: AppliedSettings = {};
          if (live.model) applied.model = live.model;
          else if (sys.model) applied.model = sys.model;
          if (sys.permissionMode) applied.mode = sys.permissionMode;
          if (live.effort) applied.effort = live.effort;
          this.emit({ type: "settingsChanged", thread: live.native, applied });
        } else if (sys.subtype === "peer_message_hold" && sys.state === "held" && sys.message_uuid) {
          const command = live.peerCommands.get(sys.message_uuid);
          if (command) {
            command.expired = true;
            command.acknowledge(new InputOutcomeUnknown("Claude Code held the message for review under the recipient’s cross-session policy"));
          }
        } else if (sys.subtype === "background_tasks_changed") {
          this.onBackgroundTasks(live, (m as { tasks?: unknown }).tasks);
        } else if (sys.subtype === "compact_boundary" && live.turn) {
          const metadata = (m as { compact_metadata?: { trigger?: string } }).compact_metadata;
          if (live.turn.compact && metadata?.trigger === "manual") {
            live.turn.compact.confirmed = true;
            live.turn.compact.error = null;
          }
          const item = { id: `compact:${sys.uuid ?? randomUUID()}`, turnId: live.turn.turnId, status: "completed", kind: "notice", noticeKind: "compaction", text: "Context compacted" } as const;
          this.emit({ type: "itemStarted", thread: live.native, item });
          this.emit({ type: "itemCompleted", thread: live.native, item });
        } else if (sys.subtype === "local_command_output" && live.turn?.compact && !live.turn.compact.confirmed) {
          const content = (m as { content?: string }).content;
          if (content) live.turn.compact.error = content.trim();
        } else if (sys.subtype === "task_started" || sys.subtype === "task_updated" || sys.subtype === "task_notification") {
          this.onTask(live, m as unknown as Record<string, unknown>);
        }
        return;
      }
      default:
        return;
    }
  }

  // ── Live subagents (D151) ──────────────────────────────────────────────────────────────

  /**
   * A task event of the running query. A subagent task (`local_agent`, or one that names a
   * subagent type) is one of Claude Code's own subagents: it becomes a conversation of its
   * own under this one the moment it starts, runs a turn there while it works, and ends with
   * the harness's status. Its task id is its agent id (sdk.d.ts: `agent_id` is "the
   * `task_id` of its task events").
   */
  private onTask(live: Live, t: Record<string, unknown>): void {
    const taskId = typeof t.task_id === "string" ? t.task_id : null;
    if (!taskId) return;
    const str = (v: unknown) => (typeof v === "string" && v ? v : null);
    if (t.subtype === "task_started") {
      const isAgent = t.task_type === "local_agent" || typeof t.subagent_type === "string";
      if (!isAgent || t.skip_transcript === true) return;
      const known = live.subagents.get(taskId);
      const native = subagentNative(live.native, taskId);
      const sub: LiveSubagent = known ?? {
        native,
        agentId: taskId,
        toolUseId: str(t.tool_use_id),
        agentType: str(t.subagent_type),
        description: str(t.description) ?? str(t.subagent_type) ?? "Subagent",
        prompt: str(t.prompt),
        turnId: `task:${str(t.run_id) ?? taskId}`,
        createdAt: new Date().toISOString(),
        translator: new LiveTranslator((ev) => this.emit({ ...ev, thread: native } as ProviderEvent), live.cwd, this.options.storeToolImage),
        running: false,
      };
      if (known) {
        // A resumed subagent: a new run, the same conversation.
        known.turnId = `task:${str(t.run_id) ?? `${taskId}:${randomUUID().slice(0, 8)}`}`;
        if (str(t.prompt)) known.prompt = str(t.prompt);
      }
      live.subagents.set(taskId, sub);
      if (sub.toolUseId) {
        live.subagentByToolUse.set(sub.toolUseId, taskId);
        // The Agent call that started it names it, and opens its conversation (D164).
        live.translator.subagentStarted(sub.toolUseId, taskId, str(t.description), sub.agentType);
      }
      sub.running = true;
      this.emitSubagent(live, sub);
      this.publishCommands(live);
      sub.translator.begin(sub.turnId);
      this.emit({ type: "turnStarted", thread: native, turnId: sub.turnId, origin: "external", at: new Date().toISOString() });
      return;
    }
    const sub = live.subagents.get(taskId);
    if (!sub) return;
    if (t.subtype === "task_updated") {
      const status = (t.patch && typeof t.patch === "object" ? (t.patch as { status?: unknown }).status : undefined) ?? null;
      if (status === "completed") this.endSubagent(live, sub, "completed", null);
      else if (status === "failed") this.endSubagent(live, sub, "failed", str((t.patch as { error?: unknown }).error));
      else if (status === "killed") this.endSubagent(live, sub, "stopped", null);
      return;
    }
    // task_notification: the run settled.
    const status = t.status === "completed" ? "completed" : t.status === "failed" ? "failed" : "stopped";
    this.endSubagent(live, sub, status, status === "failed" ? str(t.summary) : null);
  }

  /** A message a subagent produced: into its own conversation. */
  private onSubagentMessage(live: Live, m: SDKMessage): void {
    const msg = m as SDKMessage & { parent_tool_use_id?: string | null; agent_id?: string; uuid?: string };
    const agentId = msg.agent_id ?? (msg.parent_tool_use_id ? live.subagentByToolUse.get(msg.parent_tool_use_id) : undefined);
    const sub = agentId ? live.subagents.get(agentId) : undefined;
    if (!sub || !sub.running) return; // not one shown as its own conversation, or already over
    if (m.type === "assistant") sub.translator.assistant(m.message as unknown as Record<string, unknown>, m.uuid, m.error ?? null);
    else if (m.type === "user") {
      const content = (m.message as { content?: unknown }).content;
      sub.translator.user(content, typeof msg.uuid === "string" ? msg.uuid : null, (m as { origin?: unknown }).origin, (m as { tool_use_result?: unknown }).tool_use_result, flagsOf(m));
    }
  }

  /** A subagent's run ended (or its process went away): its turn ends with that status. */
  private endSubagent(live: Live, sub: LiveSubagent, status: SubagentStatus, error: string | null): void {
    if (!sub.running) return;
    sub.running = false;
    // Its run is over: nothing waits on what it asked any more (D162).
    for (const p of [...live.requests.values()]) if (p.thread === sub.native && p.turnId === sub.turnId) p.expire();
    sub.translator.finish(status === "completed");
    const turnStatus: TurnStatus = status === "completed" ? "completed" : status === "failed" ? "failed" : "interrupted";
    this.emit({ type: "turnCompleted", thread: sub.native, turnId: sub.turnId, status: turnStatus, error, usage: null, at: new Date().toISOString() });
    this.emitSubagent(live, sub, status);
  }

  private emitSubagent(live: Live, sub: LiveSubagent, status?: SubagentStatus): void {
    const s = this.settings[live.native];
    const info = this.subagentInfo({ native: live.native, cwd: live.cwd ?? s?.cwd ?? "", updatedAt: new Date().toISOString(), archived: false }, sub.agentId, null);
    info.subagent = { ...info.subagent!, status: status ?? (sub.running ? "running" : "completed") };
    info.updatedAt = new Date().toISOString();
    this.emit({ type: "threadChanged", info });
  }

  /**
   * Claude Code's full set of live background tasks (replace semantics; verified: emitted on
   * every start and end, for the main thread's and subagents' tasks alike, and empty once all
   * end). While it is non-empty the query is busy: no idle timer runs and it is never chosen to
   * make room. When it empties, the idle clock starts.
   */
  private onBackgroundTasks(live: Live, tasks: unknown): void {
    if (!Array.isArray(tasks)) return;
    const had = live.background.size > 0;
    live.background.clear();
    for (const t of tasks) {
      const r = (t ?? {}) as { task_id?: unknown; task_type?: unknown; description?: unknown; subagent_type?: unknown; ambient?: unknown };
      if (typeof r.task_id !== "string" || r.ambient === true) continue;
      live.background.set(r.task_id, {
        type: typeof r.task_type === "string" ? r.task_type : "task",
        description: typeof r.description === "string" ? r.description : "",
        ...(typeof r.subagent_type === "string" ? { subagentType: r.subagent_type } : {}),
      });
    }
    live.commands.replace(tasks, typeof live.query.stopTask === "function");
    this.publishCommands(live);
    const has = live.background.size > 0;
    if (has && live.idleTimer) {
      clearTimeout(live.idleTimer);
      live.idleTimer = null;
    }
    if (had && !has && !live.busy()) this.touch(live);
  }

  private commandOwner(live: Live, parent?: string): NativeId {
    return (parent && live.subagents.get(parent)?.native) || live.native;
  }

  private publishCommands(live: Live): void {
    const owners = new Map<NativeId, RunningCommand[]>();
    for (const shell of live.commands.active.values()) {
      const owner = this.commandOwner(live, shell.parent);
      const command = live.commands.enrich(shell);
      if (command) {
        const turnId = (shell.parent && live.subagents.get(shell.parent)?.turnId) || live.turn?.turnId || "";
        const item = { id: `notice:command:${shell.view.id}`, turnId, status: "completed", kind: "notice", noticeKind: "info", text: `Started background command: ${command}` } as const;
        this.emit({ type: "itemCompleted", thread: owner, item });
      }
      const views = owners.get(owner) ?? [];
      views.push(shell.view);
      owners.set(owner, views);
    }
    for (const owner of new Set([...live.commandOwners, ...owners.keys()])) {
      this.emit({ type: "runningCommands", thread: owner, commands: owners.get(owner) ?? [], ...(owner !== live.native ? { controller: live.native } : {}) });
    }
    live.commandOwners = new Set(owners.keys());
  }

  private findCommand(native: NativeId, id: string) {
    for (const live of this.live.values()) {
      if (live.closing) continue;
      for (const shell of live.commands.active.values()) {
        if (shell.view.id === id && this.commandOwner(live, shell.parent) === native) return { live, shell };
      }
    }
    return null;
  }

  async commandLogs(native: NativeId, id: string) {
    return claudeCommandLogs(this.findCommand(native, id)?.shell.outputPath);
  }

  async stopCommand(native: NativeId, id: string): Promise<boolean> {
    const found = this.findCommand(native, id);
    if (!found || typeof found.live.query.stopTask !== "function") return false;
    try {
      await found.live.query.stopTask(found.shell.taskId);
    } catch {
      throw new RpcError("outcome_unknown", "Claude Code did not confirm whether the command stopped.");
    }
    return true;
  }

  /** A notice item in the thread's transcript, in its running turn if there is one. */
  private notice(live: Live, noticeKind: "warning" | "info", text: string): void {
    const item = { id: `notice:${randomUUID()}`, turnId: live.turn?.turnId ?? "", status: "completed", kind: "notice", noticeKind, text } as const;
    this.emit({ type: "itemStarted", thread: live.native, item });
    this.emit({ type: "itemCompleted", thread: live.native, item });
  }

  /**
   * A change that needs a re-open (with `resume`; the query opens with the thread's settings).
   * While background work runs it is kept for the first turn after the work ends (`ensureLive`)
   * and said in the transcript (D160); otherwise the query is re-opened now.
   */
  private async reopenFor(live: Live, why: string, deferredText: string): Promise<Live> {
    if (live.hasBackground()) {
      if (live.reopenWanted !== why) this.notice(live, "info", deferredText);
      live.reopenWanted = why;
      this.log(`deferring the re-open of ${live.native} (${why}): ${live.background.size} background task(s) run in it`);
      return live;
    }
    const dirs = new Set([...live.dirs, ...live.wantedDirs]);
    this.closeLive(live, why);
    return this.openLive(live.native, dirs);
  }

  // ── Requests ───────────────────────────────────────────────────────────────────────────

  /**
   * Where a request is raised (D162). One a subagent makes (`canUseTool`'s `agentID`, the
   * hook's `agent_id`: the subagent's task id, verified on 2.1.330) goes to that subagent's own
   * thread, in its running turn: the parent's turn is never opened or held for it. One from the
   * session's own conversation goes to its running turn. Any other — a subagent YAAC does not
   * show, or a request while no turn runs — stays on the parent bound to no turn: no turn is
   * opened for it, so the composer stays usable (Claude Code takes a new message while a
   * permission prompt is pending), and no turn's end withdraws it.
   */
  private requestHome(live: Live, agentId: string | undefined): { thread: NativeId; turnId: TurnId } {
    if (agentId) {
      const sub = live.subagents.get(agentId);
      if (sub?.running) return { thread: sub.native, turnId: sub.turnId };
      return { thread: live.native, turnId: "" };
    }
    return { thread: live.native, turnId: live.turn?.turnId ?? "" };
  }

  private raise(live: Live, agentId: string | undefined, requestId: string, request: RaisedRequest, signal: AbortSignal, settle: (response: RequestResponse | null) => void): void {
    const home = this.requestHome(live, agentId);
    let open = true;
    const close = () => {
      open = false;
      live.requests.delete(requestId);
      signal.removeEventListener("abort", onAbort);
      if (!live.busy()) this.touch(live);
    };
    const pending: Pending = {
      requestId,
      thread: home.thread,
      turnId: home.turnId,
      request,
      answer: (response) => {
        if (!open) return;
        close();
        settle(response);
      },
      expire: () => {
        if (!open) return;
        close();
        this.emit({ type: "requestExpired", thread: home.thread, requestId });
        settle(null);
      },
    };
    const onAbort = () => pending.expire();
    live.requests.set(requestId, pending);
    if (live.idleTimer) clearTimeout(live.idleTimer);
    live.idleTimer = null;
    signal.addEventListener("abort", onAbort, { once: true });
    // Raised at once; it stays pending for as long as the user takes. yaacd never answers it.
    this.emit({ type: "requestRaised", thread: home.thread, turnId: home.turnId, requestId, request, blocking: true });
    if (signal.aborted) pending.expire();
  }

  private canUseTool(
    live: Live,
    toolName: string,
    input: Record<string, unknown>,
    opts: Parameters<NonNullable<Options["canUseTool"]>>[2],
  ): Promise<PermissionResult> {
    const ctx: ToolPermissionContext = {
      toolName,
      input,
      cwd: live.cwd,
      ...(opts.suggestions ? { suggestions: opts.suggestions } : {}),
      ...(opts.blockedPath ? { blockedPath: opts.blockedPath } : {}),
      ...(opts.decisionReason ? { decisionReason: opts.decisionReason } : {}),
      ...(opts.title ? { title: opts.title } : {}),
      ...(opts.displayName ? { displayName: opts.displayName } : {}),
      ...(opts.description ? { description: opts.description } : {}),
      ...(opts.suppressAlwaysAllowRule ? { suppressAlwaysAllowRule: true } : {}),
      ...(opts.mcpServer ? { mcpServer: opts.mcpServer } : {}),
      ...(opts.agentID ? { agentID: opts.agentID } : {}),
    };
    // The tool_use id, so that the request item has the same id in history.
    const requestId = opts.toolUseID || opts.requestId || randomUUID();
    if (toolName === ASK_TOOL) {
      return this.askQuestion(live, opts.agentID, requestId, input, opts.signal).then((answered) =>
        answered ? { behavior: "allow", updatedInput: answered } : { behavior: "deny", message: "The request was withdrawn before the user answered." },
      );
    }
    const request = approvalRequest(ctx);
    return new Promise<PermissionResult>((resolve) => {
      this.raise(live, opts.agentID, requestId, request, opts.signal, (response) => {
        if (!response) resolve({ behavior: "deny", message: "The request was withdrawn before the user answered." });
        else resolve(approvalResult(ctx, response));
      });
    });
  }

  /**
   * Raises an `AskUserQuestion` call as a question request, once per tool_use id, and resolves
   * with the tool input with the answers filled in (null when withdrawn). In bypass mode
   * Claude Code may not consult `canUseTool`, so a PreToolUse hook asks too; whichever comes
   * first raises the question and the other waits for the same answer.
   */
  private askQuestion(live: Live, agentId: string | undefined, requestId: string, input: Record<string, unknown>, signal: AbortSignal): Promise<Record<string, unknown> | null> {
    const inFlight = live.asking.get(requestId);
    if (inFlight) return inFlight;
    const answer = new Promise<Record<string, unknown> | null>((resolve) => {
      this.raise(live, agentId, requestId, { requestKind: "question", questions: normaliseQuestions(input) }, signal, (response) => {
        resolve(response ? answeredInput(input, response) : null);
      });
    });
    live.asking.set(requestId, answer);
    return answer;
  }

  /**
   * The PreToolUse hook for `AskUserQuestion`. Registered on every query (hooks are fixed at
   * open, and a query enters bypass with `setPermissionMode`); it acts only in bypass mode, where
   * Claude Code may not consult `canUseTool`.
   */
  private async askHook(live: Live, hook: HookInput, toolUseId: string | undefined, signal: AbortSignal): Promise<HookJSONOutput> {
    if (hook.hook_event_name !== "PreToolUse" || hook.tool_name !== ASK_TOOL || live.mode !== BYPASS_MODE) return {};
    const input = (hook.tool_input && typeof hook.tool_input === "object" ? hook.tool_input : {}) as Record<string, unknown>;
    const answered = await this.askQuestion(live, hook.agent_id, toolUseId || hook.tool_use_id || randomUUID(), input, signal);
    if (!answered) return { hookSpecificOutput: { hookEventName: "PreToolUse", permissionDecision: "deny", permissionDecisionReason: "The request was withdrawn before the user answered." } };
    return { hookSpecificOutput: { hookEventName: "PreToolUse", permissionDecision: "allow", updatedInput: answered } };
  }

  private onElicitation(live: Live, request: ElicitationRequest, signal: AbortSignal): Promise<ElicitationResult> {
    const raised: RaisedRequest = {
      requestKind: "form",
      message: request.message,
      source: request.serverName,
      schema: request.mode === "url" ? { url: request.url ?? null } : (request.requestedSchema ?? null),
    };
    return new Promise((resolve) => {
      // An MCP server's form does not say which agent asked: the session's own (D162).
      this.raise(live, undefined, `elicit-${randomUUID()}`, raised, signal, (response) => {
        if (!response) resolve({ action: "cancel" });
        else if (response.requestKind === "form") resolve({ action: "accept", content: response.values as NonNullable<ElicitationResult["content"]> });
        else resolve({ action: "decline" });
      });
    });
  }

  async respond(thread: NativeId, requestId: string, response: RequestResponse): Promise<void> {
    // A subagent's request is held by its session's query (D162).
    const live = this.live.get(thread) ?? this.live.get(parseSubagentNative(thread)?.sessionId ?? "");
    const pending = live?.requests.get(requestId);
    if (!live || !pending || pending.thread !== thread) throw new Error(`request ${requestId} is not pending in Claude Code`);
    if (response.requestKind !== pending.request.requestKind) {
      throw new Error(`request ${requestId} is a ${pending.request.requestKind}, not a ${response.requestKind}`);
    }
    pending.answer(response);
    this.emit({ type: "requestResolved", thread, requestId, by: "client", answer: response });
  }

  // ── Turns ──────────────────────────────────────────────────────────────────────────────

  private async userContent(input: ProviderInput): Promise<SDKUserMessage["message"]["content"]> {
    if (input.images.length === 0) return input.text;
    const blocks: Array<Record<string, unknown>> = [];
    if (input.text) blocks.push({ type: "text", text: input.text });
    for (const img of input.images) {
      const data = await fs.promises.readFile(img.path);
      const mediaType = img.mediaType && IMAGE_FORMATS.includes(img.mediaType) ? img.mediaType : sniffImage(data.subarray(0, 16));
      if (!mediaType) throw new Error(`${path.basename(img.path)} is not an image Claude Code accepts (${IMAGE_FORMATS.join(", ")})`);
      blocks.push({ type: "image", source: { type: "base64", media_type: mediaType, data: data.toString("base64") } });
    }
    return blocks as unknown as SDKUserMessage["message"]["content"];
  }

  private peerMessage(live: Live, input: ProviderInput, id: string): SDKUserMessage | null {
    const source = input.mesh?.find((m) => m.kind !== "notice");
    if (!live.peerInput || !source) return null;
    const sameSender = input.mesh!.every((m) => m.kind === "notice" || m.from === source.from);
    const from = sameSender ? source.from : "yaac";
    const name = sameSender ? source.fromLabel : "YAAC";
    const fromMode = sameSender && typeof source.senderSkipsApprovals === "boolean"
      ? (source.senderSkipsApprovals ? "bypass" : "prompting")
      : undefined;
    return {
      type: "user",
      message: { role: "user", content: `<cross-session-message from="${peerXml(from)}" from-name="${peerXml(name)}">\n${peerXml(input.text)}\n</cross-session-message>` },
      parent_tool_use_id: null,
      uuid: id as SDKUserMessage["uuid"],
      priority: "next",
      // The mailbox authenticates the sender's address. Its provider permission class
      // is unknown here, so leave it for Claude Code to handle under its inbound policy.
      origin: { kind: "peer", from, name, ...(fromMode ? { fromMode } : {}), body: input.text },
      isSynthetic: true,
      client_composed: true,
    };
  }

  private async enqueuePeer(live: Live, id: string, message: SDKUserMessage, beforeFollowOn?: () => void): Promise<boolean> {
    let acknowledge!: (error?: Error) => void;
    const ack = new Promise<void>((resolve, reject) => {
      acknowledge = (error) => error ? reject(error) : resolve();
    });
    live.peerCommands.set(id, { acknowledge, ...(beforeFollowOn ? { beforeFollowOn } : {}) });
    if (!live.input.push(message)) {
      live.peerCommands.delete(id);
      return false;
    }
    live.ours.add(id);
    const timer = setTimeout(() => {
      const command = live.peerCommands.get(id);
      if (command) command.expired = true;
      acknowledge(new InputOutcomeUnknown("Claude Code did not acknowledge the peer message"));
    }, 5000);
    timer.unref();
    try {
      await ack;
      return true;
    } finally {
      clearTimeout(timer);
    }
  }

  async deliverPeer(thread: NativeId, turn: TurnId, input: ProviderInput, beforeSend: () => boolean, beforeFollowOn: () => void): Promise<TurnId | null> {
    const live = this.live.get(thread);
    if (!live || live.closing || live.turn?.turnId !== turn || live.turn.interrupting || live.requests.size > 0) return null;
    const id = randomUUID();
    const message = this.peerMessage(live, input, id);
    if (!message || !beforeSend()) return null;
    return await this.enqueuePeer(live, id, message, beforeFollowOn) ? turn : null;
  }

  private async send(live: Live, input: ProviderInput): Promise<TurnId> {
    const content = await this.userContent(input);
    const turnId = randomUUID();
    // Spawn tasks use the first-prompt path. A mailbox wake on an existing session uses
    // the native peer command's uuid as the turn id when Claude reports it has started.
    const message = input.mesh?.some((m) => m.task) ? null : this.peerMessage(live, input, turnId);
    if (message) {
      if (!await this.enqueuePeer(live, turnId, message)) throw new Error("Claude Code closed its input stream");
      if (!live.turn && live.peerCommands.has(turnId)) this.beginTurn(live, turnId, input.origin);
      return turnId;
    }
    live.ours.add(turnId);
    if (!live.input.push({ type: "user", message: { role: "user", content }, parent_tool_use_id: null, uuid: turnId as SDKUserMessage["uuid"] })) {
      live.ours.delete(turnId);
      throw new Error("Claude Code closed its input stream");
    }
    this.beginTurn(live, turnId, input.origin);
    return turnId;
  }

  async startThread(params: StartThreadParams, input: ProviderInput): Promise<{ thread: NativeId; turn: TurnId }> {
    this.requireExecutable();
    const native = randomUUID();
    this.settings[native] = {
      cwd: params.cwd,
      model: params.model ?? null,
      effort: params.effort ?? null,
      mode: params.mode ?? null,
      ...(params.fast ? { fast: true } : {}),
      ...(input.readable && input.readable.length > 0 ? { readable: [...input.readable] } : {}),
    };
    this.save();
    const live = await this.ensureLive(native, this.attachmentDirs(native, input));
    const turn = await this.send(live, input);
    return { thread: native, turn };
  }

  async startTurn(thread: NativeId, input: ProviderInput, overrides: TurnOverrides): Promise<TurnId> {
    const s = await this.threadSettings(thread);
    const current = this.live.get(thread);
    if (current?.turn || current?.hasQueuedPeer()) throw new Error("a turn or peer command is already running in this Claude Code session");
    if (overrides.model) s.model = overrides.model;
    if (overrides.effort) s.effort = overrides.effort;
    if (overrides.mode) s.mode = overrides.mode;
    if (typeof overrides.fast === "boolean") s.fast = overrides.fast;
    this.save();
    let live = await this.ensureLive(thread, this.attachmentDirs(thread, input));
    live = await this.applySettings(live, s);
    if (input.mesh?.length) await live.pidKnown;
    if (live.turn || live.hasQueuedPeer()) throw new Error("a turn or peer command is already running in this Claude Code session");
    return this.send(live, input);
  }

  async compact(thread: NativeId): Promise<void> {
    const live = await this.ensureLive(thread, []);
    if (live.turn) throw new Error("a turn is already running in this Claude Code session");
    const turnId = randomUUID();
    const turn = this.beginTurn(live, turnId, "user");
    const compact: NonNullable<TurnState["compact"]> = { confirmed: false, error: null, connectionLost: false };
    turn.compact = compact;
    live.ours.add(turnId);
    live.input.push({ type: "user", message: { role: "user", content: "/compact" }, parent_tool_use_id: null, uuid: turnId as SDKUserMessage["uuid"] });
    await turn.ended;
    if (compact.connectionLost && !compact.confirmed) throw new CompactionUnconfirmed(compact.error ?? "Claude Code ended before compaction was confirmed");
    if (compact.error || !compact.confirmed) throw new Error(compact.error ?? "Claude Code did not confirm compaction");
  }

  /** Brings a live query to the thread's settings: `setModel`, effort, `setPermissionMode`. */
  private async applySettings(live: Live, s: ThreadSettings): Promise<Live> {
    let changed = false;
    let effortDirty = false;
    if (s.model && s.model !== live.model) {
      await live.query.setModel(s.model);
      live.model = s.model;
      changed = true;
      // A model switch puts effort back to the new model's default.
      effortDirty = live.effort !== null;
      live.effort = null;
    }
    if (s.effort) {
      const offered = this.sdkModels.find((m) => m.value === live.model)?.supportedEffortLevels;
      const fits = !offered || !live.model || offered.map(String).includes(s.effort);
      if (fits && (s.effort !== live.effort || effortDirty)) {
        try {
          await live.query.applyFlagSettings({ effortLevel: s.effort });
          live.effort = s.effort;
          changed = true;
        } catch (e) {
          // Effort could not be set on the live query: re-open it, unless background work runs.
          this.log(`effort change on a live query failed (${(e as Error).message}); re-opening it`);
          return this.reopenFor(live, "an effort change", "The new effort applies once the background work in this thread finishes.");
        }
      } else if (!fits) {
        s.effort = null;
        this.save();
      }
    }
    const fast = s.fast === true;
    if (fast !== live.fast) {
      try {
        await live.query.applyFlagSettings({ fastMode: fast });
        live.fast = fast;
      } catch (e) {
        // Fast mode could not be set on the live query: re-open it, unless background work runs.
        this.log(`fast mode change on a live query failed (${(e as Error).message}); re-opening it`);
        return this.reopenFor(live, "a fast mode change", `Fast mode turns ${fast ? "on" : "off"} once the background work in this thread finishes.`);
      }
    }
    const mode = s.mode ?? "default";
    if (mode !== live.mode) {
      // Every mode, bypass included, is set on the live query. Only when that cannot be done
      // (opened without `allowDangerouslySkipPermissions`, as root; or Claude Code refuses) is
      // the query re-opened in the mode — and then only once its background work ends (D160).
      let failure: string | null = mode === BYPASS_MODE && !live.bypassAllowed ? "the query was opened without allowDangerouslySkipPermissions" : null;
      if (!failure) {
        try {
          await live.query.setPermissionMode(mode as PermissionMode);
          live.mode = mode;
          changed = true;
        } catch (e) {
          failure = (e as Error).message;
        }
      }
      if (failure) {
        this.log(`permission mode change to ${mode} on a live query failed (${failure}); re-opening it`);
        const name = MODES.find((m) => m.id === mode)?.displayName ?? mode;
        const reopened = await this.reopenFor(live, `a permission mode change to ${mode}`, `Switching to ${name} when the background work in this thread finishes.`);
        if (reopened !== live) {
          this.emit({ type: "settingsChanged", thread: reopened.native, applied: await this.applied(reopened) });
          return reopened;
        }
      }
    }
    if (changed) this.emit({ type: "settingsChanged", thread: live.native, applied: await this.applied(live) });
    return live;
  }

  /** What the harness reports it is using; what was asked for, where it reports nothing. */
  private async applied(live: Live): Promise<AppliedSettings> {
    const out: AppliedSettings = { model: live.model, effort: live.effort, mode: live.mode };
    if (typeof live.query.getSettings !== "function") return out;
    try {
      const a = (await live.query.getSettings()).applied;
      if (a?.effort !== undefined) out.effort = a.effort ?? null;
      if (a?.model) {
        const row = this.sdkModels.find((m) => m.value === live.model);
        // Keep the id the user chose when it is what the harness resolved it to.
        if (!live.model || (a.model !== live.model && a.model !== row?.resolvedModel)) out.model = a.model;
      }
    } catch {
      /* report what was asked for */
    }
    return out;
  }

  async interrupt(thread: NativeId, turn: TurnId): Promise<void> {
    const live = this.live.get(thread);
    if (!live?.turn || live.turn.turnId !== turn) return;
    const t = live.turn;
    t.interrupting = true;
    for (const p of [...live.requests.values()]) if (p.turnId === turn) p.expire();
    try {
      await live.query.interrupt(live.cancelQueued && live.peerCommands.size ? { cancelQueued: true } : undefined);
    } catch (e) {
      this.log(`interrupt failed: ${(e as Error).message}`);
    }
    await Promise.race([t.ended, new Promise((r) => setTimeout(r, 30000).unref())]);
  }

  async effective(thread: NativeId): Promise<ThreadEffective> {
    const s = this.settings[thread];
    const live = this.live.get(thread);
    const model = live?.model ?? s?.model ?? null;
    const info = (model ? this.models.find((m) => m.id === model) : null) ?? this.models.find((m) => m.isDefault) ?? null;
    const caps = this.lastProbe?.capabilities ?? this.baseCapabilities();
    const efforts = info?.efforts ?? [];
    return {
      capabilities: caps.filter((c) => (c !== "effortSwitch" && c !== "reasoning") || efforts.length > 0),
      imageInput: "supported",
      imageFormats: IMAGE_FORMATS,
      efforts,
      modes: this.lastProbe?.modes ?? [],
      applied: {
        model: model ?? info?.id ?? null,
        effort: live?.effort ?? s?.effort ?? info?.defaultEffort ?? null,
        mode: live?.mode ?? s?.mode ?? "default",
      },
    };
  }

  // ── In use elsewhere ───────────────────────────────────────────────────────────────────

  private agents(): Promise<AgentEntry[]> {
    const now = Date.now();
    if (this.agentsCache && now - this.agentsCache.at < this.controlCacheMs) return this.agentsCache.entries;
    if (!this.cli) return Promise.reject(new Error("claude is not installed"));
    const entries = this.cli.agents();
    this.agentsCache = { at: now, entries };
    entries.catch(() => {
      if (this.agentsCache?.entries === entries) this.agentsCache = null;
    });
    return entries;
  }

  async control(thread: NativeId): Promise<ExternalControl> {
    const live = this.live.get(thread);
    if (live && live.pid === null) await Promise.race([live.pidKnown, new Promise((r) => setTimeout(r, 10000).unref())]);
    let entries: AgentEntry[];
    try {
      entries = await this.agents();
    } catch {
      return { external: false, uncertain: true };
    }
    const now = Date.now();
    for (const [pid, until] of this.ownPids) if (until !== Number.POSITIVE_INFINITY && now - until > 60000) this.ownPids.delete(pid);
    let uncertain = false;
    const holders: AgentEntry[] = [];
    for (const e of entries) {
      if (typeof e.sessionId !== "string") {
        uncertain = true; // an entry we cannot place could be this thread
        continue;
      }
      if (e.sessionId !== thread) continue;
      if (typeof e.pid === "number" && this.ownPids.has(e.pid)) continue; // a process yaacd runs
      if (live && live.pid === null && typeof e.pid === "number") {
        uncertain = true; // ours, probably, but not shown to be
        continue;
      }
      holders.push(e);
    }
    if (holders.length > 0) {
      const bg = holders.find((h) => h.kind === "background");
      const holderIds = holders.map(holderId);
      // Verified (Claude Code 2.1.330): resuming a session a background session holds is
      // refused at start-up (`session_held_by_background`); one an interactive `claude` holds
      // resumes, and that terminal does not see what is sent. A terminal mid-turn is not
      // interrupted by YAAC: it can be taken over once its turn ends.
      const takeOverBlocked = bg
        ? `Claude Code runs it as a background session; it cannot be continued elsewhere until that session is stopped there${typeof bg.id === "string" ? ` (claude stop ${bg.id})` : ""}`
        : holders.some((h) => h.status === "busy")
          ? "the other Claude Code session is running a turn; it can be taken over when that turn ends"
          : null;
      return { external: true, holder: bg ? "a Claude Code background session" : "another Claude Code session", holderIds, ...(takeOverBlocked ? { takeOverBlocked } : {}) };
    }
    return uncertain ? { external: false, uncertain: true } : { external: false };
  }

  // ── Fork, rename, archive, delete ──────────────────────────────────────────────────────

  async forkThread(thread: NativeId, opts: ForkOptions): Promise<NativeId> {
    const sdk = await this.sdk();
    const src = await this.threadSettings(thread);
    let upToMessageId: string | undefined;
    const live = this.live.get(thread);
    const cut = opts.atTurn ?? null;
    if (cut || live?.turn) {
      // Up to the last message of the chosen turn (or of the last finished one, when a turn runs).
      const messages = (await sdk.getSessionMessages(thread)).filter((m) => !m.parent_tool_use_id);
      const starts = messages
        .map((m, i) => ({ m, i }))
        .filter(({ m }) => m.type === "user" && !flagsOf(m).meta && !isToolResultMessage((m.message as { content?: unknown } | null)?.content));
      const target = cut ?? live?.turn?.turnId;
      const at = starts.findIndex(({ m }) => m.uuid === target);
      if (cut) {
        if (at < 0) throw new Error(`turn ${cut} is not in the stored conversation`);
        const end = at + 1 < starts.length ? starts[at + 1]!.i - 1 : messages.length - 1;
        upToMessageId = messages[end]?.uuid;
      } else if (at > 0) {
        upToMessageId = messages[starts[at]!.i - 1]?.uuid;
      }
    }
    const res = await sdk.forkSession(thread, { ...(upToMessageId ? { upToMessageId } : {}) });
    // A native fork stays in the conversation's own project folder.
    this.settings[res.sessionId] = {
      cwd: src.cwd,
      model: opts.model ?? src.model ?? null,
      effort: opts.effort ?? src.effort ?? null,
      mode: opts.mode ?? src.mode ?? null,
      stored: true,
    };
    this.save();
    void this.refreshInfo(res.sessionId);
    return res.sessionId;
  }

  async rename(thread: NativeId, title: string): Promise<void> {
    await (await this.sdk()).renameSession(thread, title);
    await this.refreshInfo(thread);
  }

  async archive(thread: NativeId): Promise<void> {
    await (await this.sdk()).archiveSession(thread);
    await this.refreshInfo(thread);
  }

  async unarchive(thread: NativeId): Promise<void> {
    await (await this.sdk()).unarchiveSession(thread);
    await this.refreshInfo(thread);
  }

  async lookupThread(native: NativeId): Promise<NativeThreadInfo | null> {
    const sub = parseSubagentNative(native);
    if (sub) {
      // A subagent (D151): known when its session lists it.
      const sdk = await this.sdk();
      const parent = await sdk.getSessionInfo(sub.sessionId);
      if (!parent || typeof sdk.listSubagents !== "function" || !(await sdk.listSubagents(sub.sessionId)).includes(sub.agentId)) return null;
      const files = (this.options.subagentFiles ?? subagentFiles)(this.configDir, sub.sessionId);
      return this.subagentInfo(
        { native: sub.sessionId, cwd: parent.cwd ?? this.settings[sub.sessionId]?.cwd ?? "", updatedAt: iso(parent.lastModified), archived: parent.archived === true },
        sub.agentId,
        files.get(sub.agentId) ?? null,
      );
    }
    const info = await (await this.sdk()).getSessionInfo(native);
    if (!info) return null;
    const s = this.settings[native];
    return {
      native,
      title: info.customTitle ?? info.summary ?? null,
      preview: previewText(info.firstPrompt),
      cwd: info.cwd ?? s?.cwd ?? "",
      model: s?.model ?? null,
      createdAt: iso(info.createdAt ?? info.lastModified),
      updatedAt: iso(info.lastModified),
      // One session alone cannot be told apart; the listing says interactive or not.
      originKind: "interactive",
      archived: info.archived === true,
    };
  }

  /** yaacd owns the Claude Code process of a live thread: a turn that ignores the interrupt ends with it. */
  async forceStop(thread: NativeId): Promise<void> {
    const live = this.live.get(thread);
    if (!live) return;
    const pid = live.pid;
    this.closeLive(live, "its turn did not stop when interrupted");
    if (pid !== null) {
      try {
        process.kill(pid, "SIGKILL");
      } catch {
        /* already gone */
      }
    }
  }

  async remove(thread: NativeId): Promise<void> {
    const live = this.live.get(thread);
    if (live) this.closeLive(live, "the thread was deleted");
    await (await this.sdk()).deleteSession(thread);
    delete this.settings[thread];
    this.save();
  }

  async dispose(): Promise<void> {
    for (const live of [...this.live.values()]) this.closeLive(live, "yaacd is stopping");
    this.disposed = true;
    const sdk = await this.sdkPromise?.catch(() => null);
    sdk?.dispose?.();
  }
}

/** Background tasks as one readable list: `subagent "sleeper", shell "npm test"`. */
function describeTasks(tasks: BackgroundTask[]): string {
  const kind = (t: BackgroundTask) => (t.type === "local_agent" ? "subagent" : t.type === "local_bash" ? "shell" : t.type.replace(/^local_/, ""));
  const shown = tasks.slice(0, 5).map((t) => `${kind(t)}${t.subagentType && t.subagentType !== "general-purpose" ? ` (${t.subagentType})` : ""}${t.description ? ` "${t.description}"` : ""}`);
  return shown.join(", ") + (tasks.length > shown.length ? ` and ${tasks.length - shown.length} more` : "");
}

/** yaacd (and so Claude Code, its child) runs as root. Tests set the override. */
let rootOverride: boolean | null = null;
export function setRunsAsRootForTests(v: boolean | null): void {
  rootOverride = v;
}
function runsAsRoot(): boolean {
  return rootOverride ?? (typeof process.getuid === "function" && process.getuid() === 0);
}

// ── The factory ──────────────────────────────────────────────────────────────────────────

/** The environment for Claude Code: yaacd's, plus the instance's, without yaacd's own variables. */
function childEnv(base: NodeJS.ProcessEnv, extra: Record<string, string> | undefined): NodeJS.ProcessEnv {
  const env: NodeJS.ProcessEnv = { ...base, ...(extra ?? {}) };
  for (const k of Object.keys(env)) if (k.startsWith("YAAC_")) delete env[k];
  return env;
}

function intVar(v: string | undefined, fallback: number): number {
  const n = v === undefined ? Number.NaN : Number.parseInt(v, 10);
  return Number.isFinite(n) && n >= 0 ? n : fallback;
}

/**
 * Names the holder of a session for a takeover (D140): a background session by its id (its
 * pid is the shared background host's), an interactive one by its process.
 */
function holderId(e: AgentEntry): string {
  if (e.kind === "background" && typeof e.id === "string") return `bg:${e.id}`;
  if (typeof e.pid === "number") return `pid:${e.pid}`;
  if (typeof e.id === "string") return `id:${e.id}`;
  return `session:${String(e.sessionId)}`;
}

/**
 * The Claude kind. An implicit default instance exists when `claude` can be found from the
 * daemon's environment (and `YAAC_CLAUDE_PROVIDER` is not `0`).
 */
export const claudeProviderFactory: ProviderFactory = {
  kind: CLAUDE_KIND,
  implicit: (env) => env.YAAC_CLAUDE_PROVIDER !== "0" && findExecutable(env) !== null,
  create(ctx: ProviderContext): ClaudeProvider {
    const cfg = ctx.config.env ?? {};
    const base = ctx.env;
    return new ClaudeProvider({
      executable: findExecutable({ ...base, ...cfg }, cfg[EXECUTABLE_VAR] ?? null),
      env: childEnv(base, cfg),
      stateDir: ctx.stateDir,
      storeToolImage: ctx.storeToolImage,
      idleMs: intVar(cfg[IDLE_MS_VAR] ?? base[IDLE_MS_VAR], DEFAULT_IDLE_MS),
      maxLive: intVar(cfg[MAX_LIVE_VAR] ?? base[MAX_LIVE_VAR], DEFAULT_MAX_LIVE) || DEFAULT_MAX_LIVE,
      log: ctx.log,
    });
  },
  // The harness home (P300) is `CLAUDE_CONFIG_DIR`: sessions, settings and sign-in follow it.
  home: {
    variable: "CLAUDE_CONFIG_DIR",
    defaultHome: (env) => path.join(env.HOME ?? env.USERPROFILE ?? os.homedir(), ".claude"),
    storeKey: claudeStoreKey,
  },
};

/** The store identity key of a Claude Code configuration folder (as the probe reports it). */
export function claudeStoreKey(configDir: string): string {
  return `claude:${sha256Hex(path.resolve(configDir)).slice(0, 16)}`;
}
