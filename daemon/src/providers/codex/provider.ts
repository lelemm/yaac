// The Codex provider: drives Codex through its app-server daemon, and nothing else
// (docs/04-providers.md#codex). One WebSocket connection per instance, reconnected when it
// drops. The conversation key is `Thread.id`, never `sessionId`.
//
// Facts this adapter is built on (checked against daemon 0.171.0, see the WP5 report):
// - thread lifecycle notifications (`thread/started`, `thread/status/changed`, rename, archive,
//   delete) reach every connection; turn and item events reach subscribed connections only;
// - `thread/start`, `thread/resume`, `thread/fork` subscribe this connection;
// - `turn/start` on a busy thread joins the running turn and returns its id;
// - pending server requests are re-sent to a connection that subscribes later, under a new
//   JSON-RPC id; `serverRequest/resolved` goes to every subscriber;
// - an idle thread nobody is subscribed to stays loaded for a while before Codex unloads it.

import * as fs from "node:fs";
import * as path from "node:path";
import type {
  Capability,
  Item,
  ModeInfo,
  ModelInfo,
  RequestResponse,
  ThreadEffective,
  TurnId,
  TurnOrigin,
} from "../../protocol/generated/types.js";
import { Emitter } from "../../util/emitter.js";
import { randomId, readJson, sha256Hex, writeJsonAtomic } from "../../util/fsx.js";
import type {
  ExternalControl,
  ForkOptions,
  HarnessProvider,
  ItemPage,
  MeshHooks,
  NativeId,
  PageRequest,
  ScanRequest,
  NativeThreadInfo,
  ProbeResult,
  ProviderContext,
  ProviderEvent,
  ProviderInput,
  StartThreadParams,
  ThreadPage,
  TurnOverrides,
} from "../types.js";
import { connectBackend, planEndpoint, realExec, type EndpointPlan, type Exec } from "./backend.js";
import { mapItem, mapModel, mapServerRequest, planState, threadInfo, tokenUsage, turnStatus, usageDelta, type MappedRequest } from "./mapping.js";
import { CodexConnection, CodexRpcError, type ServerRequest } from "./rpc.js";
import {
  THREAD_SOURCE_KINDS,
  type GetAccountResponse,
  type InitializeResponse,
  type Model,
  type Paged,
  type Thread,
  type ThreadItem,
  type ThreadItemEntry,
  type ThreadStatus,
  type ThreadTokenUsage,
  type Turn,
  type UserInput,
} from "./wire.js";

// ── The mesh hook ──────────────────────────────────────────────────────────────────────────

/** The MCP server name the agent mesh is attached under. */
const MESH_SERVER = "yaac";

/** Extra settings for one Codex thread (docs/10-agent-mesh.md#attaching-the-mcp-server). */
export interface CodexThreadSetup {
  /** Merged into `config` on `thread/start` / `thread/resume` / `thread/fork` (dotted keys allowed). */
  config?: Record<string, unknown> | null;
  /** Passed as `developerInstructions`. */
  developerInstructions?: string | null;
  /**
   * Called with the new thread's id as soon as Codex has assigned it (`start`, `fork`), before
   * its first turn: the mesh binds the thread's attachment ticket here.
   */
  bind?: ((native: NativeId) => void) | null;
}

export interface CodexThreadSetupRequest {
  /** `start`: a new thread; `resume`: yaacd (re)subscribes an existing one; `fork`: a native fork. */
  phase: "start" | "resume" | "fork";
  /** The thread being resumed, or the fork's source; null for `start`. */
  thread: NativeId | null;
  cwd: string | null;
}

export type CodexThreadSetupHook = (req: CodexThreadSetupRequest) => CodexThreadSetup | null | Promise<CodexThreadSetup | null>;

// ── Options and state ──────────────────────────────────────────────────────────────────────

export interface CodexOptions {
  /** Explicit socket from the instance configuration: connect-only. */
  socket?: string | null;
  /** The instance's environment for `codex` (CODEX_HOME, …). */
  env?: Record<string, string>;
  baseEnv?: NodeJS.ProcessEnv;
  codexBin?: string | null;
  exec?: Exec;
  /** Reconnect backoff bounds. */
  reconnectMinMs?: number;
  reconnectMaxMs?: number;
  clientName?: string;
  clientVersion?: string;
}

/**
 * Approval and sandbox modes, as Codex's own approval-policy and sandbox enumerations define
 * them (the presets Codex offers in its interfaces). These are harness settings, not model
 * data. A thread never starts in one unless it is chosen: with no mode, nothing is sent and the
 * thread follows the user's Codex configuration.
 */
interface CodexMode extends ModeInfo {
  approvalPolicy: string;
  sandbox: string;
  sandboxPolicy: Record<string, unknown>;
}

export const CODEX_MODES: readonly CodexMode[] = [
  {
    id: "codex:read-only",
    displayName: "Read only",
    description: "Codex can read files; it asks before editing, running commands that need more, or using the network.",
    approvalPolicy: "on-request",
    sandbox: "read-only",
    sandboxPolicy: { type: "readOnly", networkAccess: false },
  },
  {
    id: "codex:auto",
    displayName: "Auto",
    description: "Codex can read and edit files in the working directory and run commands there; it asks before going beyond it.",
    approvalPolicy: "on-request",
    sandbox: "workspace-write",
    sandboxPolicy: { type: "workspaceWrite", writableRoots: [], networkAccess: false, excludeTmpdirEnvVar: false, excludeSlashTmp: false },
  },
  {
    id: "codex:full-access",
    displayName: "Full access",
    description: "Codex edits files, runs commands, and uses the network anywhere, and never asks.",
    skipsApprovals: true,
    approvalPolicy: "never",
    sandbox: "danger-full-access",
    sandboxPolicy: { type: "dangerFullAccess" },
  },
];

function modeInfo(m: CodexMode): ModeInfo {
  const { approvalPolicy: _a, sandbox: _s, sandboxPolicy: _p, ...info } = m;
  void _a;
  void _s;
  void _p;
  return info;
}

function findMode(id: string | null | undefined): CodexMode | null {
  if (!id) return null;
  const m = CODEX_MODES.find((x) => x.id === id);
  if (!m) throw new Error(`"${id}" is not a Codex mode`);
  return m;
}

const CLIENT_NAME = "yaacd";
const DEFAULT_CLIENT_VERSION = "0.1.0";

interface ThreadState {
  native: NativeId;
  /** This connection is subscribed (we started, resumed, or forked it on this connection). */
  subscribed: boolean;
  status: ThreadStatus | null;
  /** The running turn yaacd started. */
  ownTurn: TurnId | null;
  ownOrigin: TurnOrigin;
  /** A running turn yaacd did not start. */
  foreignTurn: TurnId | null;
  /** Another client drove this thread; cleared only when Codex unloads it. */
  foreignSeen: boolean;
  /** turn/start or turn/steer in flight: events are held until we know whose turn it is. */
  starting: boolean;
  held: Array<{ method: string; params: Record<string, unknown> }>;
  /** Turn ids seen in `turn/started` (to detect `turn/start` joining a running turn). */
  turnsSeen: Set<TurnId>;
  /** `clientUserMessageId`s of input yaacd sent; their `userMessage` items are not re-emitted. */
  sentClientIds: Set<string>;
  /** Inputs sent in the running own turn, for servers that do not echo `clientId`. */
  ownInputsPending: number;
  model: string | null;
  effort: string | null;
  /** The mode yaacd last applied (null: the user's Codex configuration). */
  mode: string | null;
  /** Effort chosen at fork time, applied on the next turn (fork takes no effort). */
  pendingEffort: string | null;
  usageAtStart: ThreadTokenUsage | undefined;
  lastUsage: ThreadTokenUsage | undefined;
  plan: Item | null;
  notices: number;
  lastControl: string;
}

interface LiveRequest {
  requestId: string;
  rpcId: number | string;
  gen: number;
  mapped: MappedRequest;
  answered: RequestResponse | null;
}

interface PersistedState {
  /** Running turns yaacd started, by thread: lets `runningTurn` adopt a turn after a restart. */
  ownTurns: Record<string, string>;
}

export class ThreadInUseError extends Error {}

export class CodexProvider implements HarnessProvider {
  readonly events = new Emitter<ProviderEvent>();
  private readonly plan: EndpointPlan;
  private readonly exec: Exec;
  private conn: CodexConnection | null = null;
  /** Whether the daemon of the current connection was found running or started by yaacd. */
  private backendOrigin: "foundRunning" | "startedByYaac" | null = null;
  private connecting: Promise<CodexConnection> | null = null;
  private gen = 0;
  private init: InitializeResponse | null = null;
  private lastFailure: { status: ProbeResult["status"]; message: string } | null = null;
  private threads = new Map<NativeId, ThreadState>();
  private requests = new Map<string, LiveRequest>();
  private models: ModelInfo[] = [];
  /** Sign-in mode at the last probe (lower case); undefined before the first probe. */
  private authMode: string | null | undefined = undefined;
  private sourceKinds: string[] = [...THREAD_SOURCE_KINDS];
  private setupHook: CodexThreadSetupHook | null = null;
  /** The agent mesh is attached (the `meshTools` capability). */
  private meshAttached = false;
  private reconnectTimer: NodeJS.Timeout | null = null;
  private reconnectDelay: number;
  private disposed = false;
  private readonly stateFile: string;
  private persisted: PersistedState;

  constructor(
    private readonly ctx: ProviderContext,
    private readonly options: CodexOptions = {},
  ) {
    this.plan = planEndpoint({ socket: options.socket ?? null, env: options.env ?? {}, baseEnv: options.baseEnv ?? process.env, codexBin: options.codexBin ?? null });
    this.exec = options.exec ?? realExec;
    this.reconnectDelay = options.reconnectMinMs ?? 500;
    this.stateFile = path.join(ctx.stateDir, "codex-state.json");
    this.persisted = readJson<PersistedState>(this.stateFile) ?? { ownTurns: {} };
  }

  /**
   * The mesh hook: lets a caller supply, per thread, extra `config` and `developerInstructions`
   * for `thread/start`, `thread/resume`, and `thread/fork`. Null removes it.
   */
  setThreadSetupHook(hook: CodexThreadSetupHook | null): void {
    this.setupHook = hook;
  }

  /**
   * The agent mesh (WP7): each thread gets a `mcp_servers.yaac` entry launching the
   * `yaacd mcp` bridge by absolute path with its attachment ticket, an explicit tool timeout,
   * and the identity block as `developerInstructions` (docs/10-agent-mesh.md#attaching-the-mcp-server).
   */
  attachMesh(hooks: MeshHooks | null): void {
    this.meshAttached = !!hooks;
    if (!hooks) {
      this.setThreadSetupHook(null);
      return;
    }
    this.setThreadSetupHook((req) => {
      const att = hooks.open({ native: req.phase === "resume" ? req.thread : null, cwd: req.cwd, phase: req.phase });
      if (!att) return null;
      const stdio = att.stdio();
      return {
        config: {
          // The yaac tools are governed by the chat's own Agents toggle and budget (D51), so
          // Codex is told not to ask again for each call (verified live: without it, every
          // call raises "Allow the yaac MCP server to run tool …?").
          [`mcp_servers.${MESH_SERVER}`]: { command: stdio.command, args: stdio.args, env: stdio.env, tool_timeout_sec: att.toolTimeoutSec, default_tools_approval_mode: "approve" },
        },
        developerInstructions: att.identity,
        bind: (native) => att.bind(native),
      };
    });
  }

  // ── Connection ───────────────────────────────────────────────────────────────────────────

  private emit(ev: ProviderEvent): void {
    if (!this.disposed) this.events.emit(ev);
  }

  private save(): void {
    try {
      fs.mkdirSync(this.ctx.stateDir, { recursive: true, mode: 0o700 });
      writeJsonAtomic(this.stateFile, this.persisted);
    } catch (e) {
      this.ctx.log(`could not save Codex state: ${(e as Error).message}`);
    }
  }

  private async ensure(): Promise<CodexConnection> {
    if (this.disposed) throw new Error("the Codex provider was disposed");
    if (this.conn?.isOpen) return this.conn;
    if (this.connecting) return this.connecting;
    this.connecting = this.connect().finally(() => {
      this.connecting = null;
    });
    return this.connecting;
  }

  private async connect(): Promise<CodexConnection> {
    const result = await connectBackend(this.plan, { exec: this.exec, mayStart: true, log: (l) => this.ctx.log(l) });
    if (!result.ok) {
      this.lastFailure = { status: result.status, message: result.message };
      throw new Error(result.message);
    }
    const conn = result.connection;
    this.backendOrigin = result.started ? "startedByYaac" : "foundRunning";
    let init: InitializeResponse;
    try {
      init = await conn.initialize(
        { name: this.options.clientName ?? CLIENT_NAME, title: "YAAC", version: this.options.clientVersion ?? DEFAULT_CLIENT_VERSION },
        // experimentalApi: needed for `availableDecisions` on approvals and for requestUserInput.
        { experimentalApi: true },
      );
    } catch (e) {
      conn.close();
      this.lastFailure = { status: "unreachable", message: `the Codex daemon did not complete initialize: ${(e as Error).message}` };
      throw e;
    }
    if (this.disposed) {
      conn.close();
      throw new Error("the Codex provider was disposed");
    }
    const before = this.init;
    this.init = init;
    this.lastFailure = null;
    this.conn = conn;
    this.gen += 1;
    this.reconnectDelay = this.options.reconnectMinMs ?? 500;
    conn.notifications.on((n) => this.onNotification(n.method, n.params));
    conn.requests.on((r) => this.onServerRequest(conn, r));
    conn.closed.on(() => this.onClosed(conn));
    if (before && (before.codexHome !== init.codexHome || before.userAgent !== init.userAgent)) this.emit({ type: "descriptorChanged" });
    if (before) void this.afterReconnect();
    return conn;
  }

  private onClosed(conn: CodexConnection): void {
    if (this.conn !== conn) return;
    this.conn = null;
    const subscribed: NativeId[] = [];
    for (const st of this.threads.values()) {
      if (st.subscribed) subscribed.push(st.native);
      st.subscribed = false;
    }
    this.resubscribe = new Set([...this.resubscribe, ...subscribed]);
    if (this.disposed) return;
    this.ctx.log("the connection to the Codex daemon closed; reconnecting");
    for (const st of this.threads.values()) this.publishControl(st);
    this.scheduleReconnect();
  }

  /** Threads this process was subscribed to when the connection dropped: rejoined on reconnect. */
  private resubscribe = new Set<NativeId>();

  private scheduleReconnect(): void {
    if (this.disposed || this.reconnectTimer) return;
    const delay = this.reconnectDelay;
    this.reconnectDelay = Math.min(delay * 2, this.options.reconnectMaxMs ?? 10_000);
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      this.ensure().catch(() => this.scheduleReconnect());
    }, delay);
    this.reconnectTimer.unref?.();
  }

  private async afterReconnect(): Promise<void> {
    const again = [...this.resubscribe];
    this.resubscribe.clear();
    for (const native of again) {
      const st = this.state(native);
      try {
        await this.resume(native, null);
        if (st.ownTurn && !this.isActive(st.status)) await this.reconcileFinishedTurn(st, st.ownTurn);
      } catch (e) {
        this.ctx.log(`could not rejoin a Codex thread after reconnecting: ${(e as Error).message}`);
      }
      this.publishControl(st);
    }
    for (const st of this.threads.values()) this.publishControl(st);
  }

  /** A turn of ours ended while the connection was down: report what Codex stored for it. */
  private async reconcileFinishedTurn(st: ThreadState, turnId: TurnId): Promise<void> {
    const conn = await this.ensure();
    const page = (await conn.request("thread/turns/list", { threadId: st.native, limit: 5, sortDirection: "desc", itemsView: "full" })) as Paged<Turn>;
    const turn = page.data.find((t) => t.id === turnId);
    if (!turn || turn.status === "inProgress") return;
    for (const raw of turn.items ?? []) {
      if (raw.type === "userMessage") continue;
      this.emit({ type: "itemCompleted", thread: st.native, item: mapItem(raw, turnId, "completed") });
    }
    this.finishTurn(st, turn, "own");
  }

  // ── Thread state ─────────────────────────────────────────────────────────────────────────

  private state(native: NativeId): ThreadState {
    let st = this.threads.get(native);
    if (!st) {
      st = {
        native,
        subscribed: false,
        status: null,
        ownTurn: null,
        ownOrigin: "user",
        foreignTurn: null,
        foreignSeen: false,
        starting: false,
        held: [],
        turnsSeen: new Set(),
        sentClientIds: new Set(),
        ownInputsPending: 0,
        model: null,
        effort: null,
        mode: null,
        pendingEffort: null,
        usageAtStart: undefined,
        lastUsage: undefined,
        plan: null,
        notices: 0,
        lastControl: "",
      };
      this.threads.set(native, st);
    }
    return st;
  }

  private isActive(s: ThreadStatus | null): boolean {
    return s?.type === "active";
  }

  /** Who holds the thread, from what this connection knows right now. */
  private localControl(st: ThreadState): ExternalControl {
    if (!this.conn?.isOpen) return { external: false, uncertain: true, holder: "Codex (the daemon is not reachable)" };
    if (st.foreignTurn) return { external: true, holder: "another Codex client" };
    if (this.isActive(st.status) && !st.ownTurn && !st.starting) return { external: true, holder: "another Codex client" };
    if (st.status?.type === "notLoaded") return { external: false };
    if (st.foreignSeen) return { external: true, holder: "another Codex client" };
    if (st.status === null) return { external: false, uncertain: true, holder: "Codex (state not known yet)" };
    if (st.subscribed) return { external: false };
    // Loaded on the daemon but not by this connection: it may be open in a terminal or another
    // app, or Codex has simply not unloaded it yet. It cannot be told apart, so: in use.
    return { external: false, uncertain: true, holder: "another Codex client (the thread is loaded on the Codex daemon)" };
  }

  private publishControl(st: ThreadState): void {
    const c = this.localControl(st);
    const key = JSON.stringify(c);
    if (key === st.lastControl) return;
    const first = st.lastControl === "";
    st.lastControl = key;
    if (first && !c.external && !c.uncertain) return;
    this.emit({ type: "controlChanged", thread: st.native, control: c });
  }

  private async readThreadMeta(native: NativeId): Promise<Thread> {
    const conn = await this.ensure();
    const r = (await conn.request("thread/read", { threadId: native })) as { thread: Thread };
    const st = this.state(native);
    if (r.thread.status) st.status = r.thread.status;
    if (r.thread.model !== undefined && (!st.subscribed || !st.model)) st.model = r.thread.model ?? null;
    if (r.thread.reasoningEffort !== undefined && (!st.subscribed || !st.effort)) st.effort = r.thread.reasoningEffort ?? null;
    if (!this.isActive(st.status) && st.foreignTurn) st.foreignTurn = null;
    if (st.status?.type === "notLoaded") st.foreignSeen = false;
    return r.thread;
  }

  private async setup(req: CodexThreadSetupRequest): Promise<Record<string, unknown>> {
    return (await this.setupWithBind(req)).params;
  }

  private async setupWithBind(req: CodexThreadSetupRequest): Promise<{ params: Record<string, unknown>; bind: ((native: NativeId) => void) | null }> {
    if (!this.setupHook) return { params: {}, bind: null };
    const s = await this.setupHook(req);
    const out: Record<string, unknown> = {};
    if (s?.config && Object.keys(s.config).length > 0) out.config = s.config;
    if (s?.developerInstructions) out.developerInstructions = s.developerInstructions;
    return { params: out, bind: s?.bind ?? null };
  }

  /** `thread/resume`: subscribes this connection (rejoins a loaded thread). */
  private async resume(native: NativeId, model: string | null): Promise<Thread> {
    const conn = await this.ensure();
    const st = this.state(native);
    const extras = await this.setup({ phase: "resume", thread: native, cwd: null });
    const r = (await conn.request("thread/resume", { threadId: native, excludeTurns: true, ...(model ? { model } : {}), ...extras })) as {
      thread: Thread;
      model?: string;
      reasoningEffort?: string | null;
    };
    st.subscribed = true;
    if (r.thread.status) st.status = r.thread.status;
    st.model = r.model ?? r.thread.model ?? st.model;
    st.effort = r.reasoningEffort ?? r.thread.reasoningEffort ?? st.effort;
    return r.thread;
  }

  // ── Probe ────────────────────────────────────────────────────────────────────────────────

  async probe(): Promise<ProbeResult> {
    let conn: CodexConnection;
    try {
      conn = await this.ensure();
    } catch (e) {
      const f = this.lastFailure ?? { status: "unreachable" as const, message: (e as Error).message };
      return this.failed(f.status, f.message);
    }
    const init = this.init ?? {};
    const home = init.codexHome ?? null;
    let account: string | null = null;
    let status: ProbeResult["status"] = "ready";
    let statusMessage: string | null = null;
    try {
      const acc = (await conn.request("account/read", {})) as GetAccountResponse;
      if (acc.account) {
        const a = acc.account as { type: string; email?: string | null };
        account = a.email ?? a.type;
        this.authMode = a.type.toLowerCase();
      } else if (acc.requiresOpenaiAuth !== false) {
        status = "needsAuth";
        statusMessage = "Codex is not signed in on this target · run `codex login` there";
        this.authMode = null;
      }
    } catch (e) {
      statusMessage = `could not read the Codex account: ${(e as Error).message}`;
    }
    const { models, complete } = await this.listModels(conn);
    if (complete || models.length > 0) this.models = complete ? models : this.models.length > 0 ? this.models : models;
    const caps: Capability[] = [
      "threadList",
      "threadRead",
      "threadResume",
      "threadFork",
      "threadForkAtTurn",
      "threadRename",
      "threadArchive",
      "threadDelete",
      "followExternal",
      "streamText",
      "streamReasoning",
      "interrupt",
      "steer",
      "approvals",
      "userQuestions",
      "userForms",
      "modelSwitch",
      "effortSwitch",
      "reasoning",
      "reasoningSummary",
      "tokenUsage",
      "contextUsage",
      "modelList",
      "plan",
      "webSearch",
      "modes",
    ];
    if (models.some((m) => m.capabilities?.some((c) => c.capability === "imageInput" && c.support !== "unsupported"))) caps.push("imageInput");
    if (this.meshAttached) caps.push("meshTools");
    return {
      displayName: "Codex",
      backend: "daemon",
      harnessVersion: serverVersion(init.userAgent),
      store: { key: home ? storeKey(home) : null, home, account },
      status,
      statusMessage,
      capabilities: caps,
      models,
      modelsComplete: complete,
      modes: CODEX_MODES.map(modeInfo),
      ...(this.backendOrigin ? { backendOrigin: this.backendOrigin } : {}),
    };
  }

  private failed(status: ProbeResult["status"], message: string): ProbeResult {
    return { displayName: "Codex", backend: "daemon", harnessVersion: null, store: { key: null }, status, statusMessage: message, capabilities: [], models: [], modelsComplete: false, modes: [] };
  }

  /** Every page of `model/list`, hidden models included. Partial on any failure. */
  private async listModels(conn: CodexConnection): Promise<{ models: ModelInfo[]; complete: boolean }> {
    const out: ModelInfo[] = [];
    const seen = new Set<string>();
    let cursor: string | null = null;
    for (let pages = 0; pages < 1000; pages++) {
      let page: Paged<Model>;
      try {
        page = (await conn.request("model/list", { includeHidden: true, ...(cursor ? { cursor } : {}) })) as Paged<Model>;
      } catch (e) {
        this.ctx.log(`model/list failed: ${(e as Error).message}`);
        return { models: out, complete: false };
      }
      for (const m of page.data ?? []) {
        const info = mapModel(m);
        if (seen.has(info.id)) continue;
        seen.add(info.id);
        out.push(info);
      }
      cursor = page.nextCursor ?? null;
      if (!cursor) return { models: out, complete: true };
    }
    return { models: out, complete: false };
  }

  // ── Thread index ─────────────────────────────────────────────────────────────────────────

  async scanThreads(page: ScanRequest): Promise<ThreadPage> {
    const conn = await this.ensure();
    // Cursor: which listing (active, then archived), Codex's own cursor within it, and how
    // many of that listing were returned (for the discovery caps' `max`).
    const state = decodeCursor(page.cursor);
    const params: Record<string, unknown> = {
      limit: page.limit ?? 100,
      sourceKinds: this.sourceKinds,
      archived: state.archived,
      // Newest first by last activity, so the discovery caps can stop early.
      sortKey: "updated_at",
      sortDirection: "desc",
      ...(page.search ? { searchTerm: page.search } : {}),
      ...(state.cursor ? { cursor: state.cursor } : {}),
    };
    let res: Paged<Thread>;
    try {
      res = (await conn.request("thread/list", params)) as Paged<Thread>;
    } catch (e) {
      const allowed = acceptedKinds(e);
      if (!allowed) throw e;
      // An older daemon that does not know some of our source kinds: ask for the ones it has.
      this.sourceKinds = this.sourceKinds.filter((k) => allowed.includes(k));
      res = (await conn.request("thread/list", { ...params, sourceKinds: this.sourceKinds })) as Paged<Thread>;
    }
    let threads = (res.data ?? []).map((t) => {
      const st = this.state(t.id);
      if (t.status && !st.subscribed) st.status = t.status;
      return threadInfo(t, state.archived);
    });
    // The discovery caps: this listing ends at the first conversation too old, or at `max`.
    let ended = false;
    if (page.activeSince) {
      const i = threads.findIndex((t) => t.updatedAt < page.activeSince!);
      if (i >= 0) {
        threads = threads.slice(0, i);
        ended = true;
      }
    }
    const count = (state.count ?? 0) + threads.length;
    if (page.max && count >= page.max) {
      threads = threads.slice(0, Math.max(0, page.max - (state.count ?? 0)));
      ended = true;
    }
    let next: string | null = null;
    if (res.nextCursor && !ended) next = encodeCursor({ archived: state.archived, cursor: res.nextCursor, count });
    else if (!state.archived) next = encodeCursor({ archived: true, cursor: null, count: 0 });
    return { threads, cursor: next, complete: true };
  }

  async lookupThread(native: NativeId): Promise<NativeThreadInfo | null> {
    let t: Thread;
    try {
      t = await this.readThreadMeta(native);
    } catch {
      return null;
    }
    // `thread/read` does not say whether the thread is archived.
    const info = threadInfo(t, false);
    delete info.archived;
    return info;
  }

  // ── History ──────────────────────────────────────────────────────────────────────────────

  async readThread(thread: NativeId, page: PageRequest): Promise<ItemPage> {
    const conn = await this.ensure();
    try {
      const res = (await conn.request("thread/items/list", {
        threadId: thread,
        limit: page.limit ?? 100,
        sortDirection: "asc",
        ...(page.cursor ? { cursor: page.cursor } : {}),
      })) as Paged<ThreadItemEntry>;
      return { items: (res.data ?? []).map((e) => mapItem(e.item, e.turnId, "completed")), cursor: res.nextCursor ?? null };
    } catch (e) {
      if (page.cursor) throw e;
      // Older history without item paging: one full read.
      const r = (await conn.request("thread/read", { threadId: thread, includeTurns: true })) as { thread: Thread };
      const items: Item[] = [];
      for (const turn of r.thread.turns ?? []) for (const raw of turn.items ?? []) items.push(mapItem(raw, turn.id, turn.status === "inProgress" ? "inProgress" : "completed"));
      return { items, cursor: null };
    }
  }

  // ── Turns ────────────────────────────────────────────────────────────────────────────────

  async startThread(params: StartThreadParams, input: ProviderInput): Promise<{ thread: NativeId; turn: TurnId }> {
    const conn = await this.ensure();
    const mode = findMode(params.mode);
    const { params: extras, bind } = await this.setupWithBind({ phase: "start", thread: null, cwd: params.cwd });
    const r = (await conn.request("thread/start", {
      cwd: params.cwd,
      ...(params.model ? { model: params.model } : {}),
      ...(mode ? { approvalPolicy: mode.approvalPolicy, sandbox: mode.sandbox } : {}),
      ...extras,
    })) as {
      thread: Thread;
      model?: string;
      reasoningEffort?: string | null;
    };
    const native = r.thread.id;
    // The mesh's ticket is bound to the thread before its first turn runs.
    bind?.(native);
    const st = this.state(native);
    st.subscribed = true;
    st.status = r.thread.status ?? { type: "idle" };
    st.model = r.model ?? r.thread.model ?? null;
    st.effort = r.reasoningEffort ?? null;
    st.mode = mode?.id ?? null;
    st.lastControl = JSON.stringify({ external: false });
    try {
      const turn = await this.beginTurn(st, input, { model: null, effort: params.effort ?? null, mode: null });
      return { thread: native, turn };
    } catch (e) {
      // Nothing was sent: do not leave an empty conversation behind.
      await conn.request("thread/delete", { threadId: native }).catch(() => {});
      throw e;
    }
  }

  async startTurn(thread: NativeId, input: ProviderInput, overrides: TurnOverrides): Promise<TurnId> {
    const st = this.state(thread);
    if (st.ownTurn || st.foreignTurn || st.starting) throw new Error("a turn is already running in this Codex thread");
    if (!st.subscribed) {
      const t = await this.resume(thread, null);
      if (this.isActive(t.status ?? null)) {
        st.foreignSeen = true;
        this.publishControl(st);
        throw new ThreadInUseError("the Codex thread is running a turn started elsewhere");
      }
    }
    if (this.isActive(st.status)) throw new ThreadInUseError("the Codex thread is busy");
    return this.beginTurn(st, input, { model: overrides.model ?? null, effort: overrides.effort ?? null, mode: findMode(overrides.mode) });
  }

  private userInput(input: ProviderInput): UserInput[] {
    const out: UserInput[] = [];
    if (input.text.length > 0 || input.images.length === 0) out.push({ type: "text", text: input.text, text_elements: [] });
    for (const img of input.images) out.push({ type: "localImage", path: img.path });
    return out;
  }

  private async beginTurn(st: ThreadState, input: ProviderInput, o: { model: string | null; effort: string | null; mode: CodexMode | null }): Promise<TurnId> {
    const conn = await this.ensure();
    const clientId = randomId("yaac-", 12);
    const effort = o.effort ?? st.pendingEffort;
    st.sentClientIds.add(clientId);
    st.starting = true;
    st.held = [];
    const seenBefore = new Set(st.turnsSeen);
    let turnId: TurnId;
    try {
      const r = (await conn.request("turn/start", {
        threadId: st.native,
        input: this.userInput(input),
        clientUserMessageId: clientId,
        ...(o.model ? { model: o.model } : {}),
        ...(effort ? { effort } : {}),
        ...(o.mode ? { approvalPolicy: o.mode.approvalPolicy, sandboxPolicy: o.mode.sandboxPolicy } : {}),
      })) as { turn: Turn };
      turnId = r.turn.id;
    } catch (e) {
      st.starting = false;
      st.sentClientIds.delete(clientId);
      this.flushHeld(st);
      throw e;
    }
    st.starting = false;
    if (seenBefore.has(turnId) || st.foreignTurn === turnId) {
      // Codex spliced the input into a turn that was already running. The dispatcher should
      // have made that impossible; it is reported, not hidden.
      st.foreignTurn = turnId;
      st.foreignSeen = true;
      this.flushHeld(st);
      this.publishControl(st);
      throw new ThreadInUseError(`Codex joined this input into turn ${turnId}, which was already running`);
    }
    st.pendingEffort = null;
    if (o.model) st.model = o.model;
    if (effort) st.effort = effort;
    if (o.mode) st.mode = o.mode.id;
    st.ownTurn = turnId;
    st.ownOrigin = input.origin;
    st.ownInputsPending = 1;
    st.turnsSeen.add(turnId);
    st.usageAtStart = st.lastUsage;
    st.plan = null;
    this.persisted.ownTurns[st.native] = turnId;
    this.save();
    this.emit({ type: "turnStarted", thread: st.native, turnId, origin: input.origin, at: new Date().toISOString() });
    if (o.model || effort || o.mode) this.emit({ type: "settingsChanged", thread: st.native, applied: { model: st.model, effort: st.effort, mode: st.mode } });
    this.flushHeld(st);
    return turnId;
  }

  private flushHeld(st: ThreadState): void {
    const held = st.held;
    st.held = [];
    for (const h of held) this.onThreadNotification(st, h.method, h.params);
  }

  async interrupt(thread: NativeId, turn: TurnId): Promise<void> {
    const conn = await this.ensure();
    await conn.request("turn/interrupt", { threadId: thread, turnId: turn });
  }

  async steer(thread: NativeId, turn: TurnId, input: ProviderInput): Promise<void> {
    const conn = await this.ensure();
    const st = this.state(thread);
    if (st.ownTurn !== turn) throw new Error(`turn ${turn} is not the turn yaacd is running in this thread`);
    const clientId = randomId("yaac-", 12);
    st.sentClientIds.add(clientId);
    st.ownInputsPending += 1;
    try {
      await conn.request("turn/steer", { threadId: thread, expectedTurnId: turn, input: this.userInput(input), clientUserMessageId: clientId });
    } catch (e) {
      st.sentClientIds.delete(clientId);
      st.ownInputsPending = Math.max(0, st.ownInputsPending - 1);
      throw e;
    }
  }

  async runningTurn(thread: NativeId): Promise<TurnId | null> {
    const conn = await this.ensure();
    const meta = await this.readThreadMeta(thread);
    if (!this.isActive(meta.status ?? null)) {
      if (this.persisted.ownTurns[thread]) {
        delete this.persisted.ownTurns[thread];
        this.save();
      }
      return null;
    }
    const page = (await conn.request("thread/turns/list", { threadId: thread, limit: 1, sortDirection: "desc" })) as Paged<Turn>;
    const turn = page.data?.[0];
    if (!turn || turn.status !== "inProgress") return null;
    const st = this.state(thread);
    if (this.persisted.ownTurns[thread] === turn.id) {
      // A turn yaacd started before it restarted: adopt it, and follow it again.
      await this.resume(thread, null);
      st.ownTurn = turn.id;
      st.turnsSeen.add(turn.id);
      st.ownInputsPending = 0;
    }
    return turn.id;
  }

  // ── Requests ─────────────────────────────────────────────────────────────────────────────

  private onServerRequest(conn: CodexConnection, r: ServerRequest): void {
    const mapped = mapServerRequest(r.method, r.params);
    if (!mapped) {
      // item/tool/call (yaacd registers no dynamic tools), auth refresh, attestation, or a
      // method newer than this adapter: not a question for the user.
      conn.replyError(r.id, -32601, `${r.method} is not supported by this client`);
      return;
    }
    const stable = mapped.key ?? `rpc:${this.gen}:${String(r.id)}`;
    const requestId = `codex-${sha256Hex(`${mapped.threadId}\u0000${stable}`).slice(0, 24)}`;
    const existing = this.requests.get(requestId);
    if (existing) {
      // The same request re-sent after a reconnect or a new subscription: same request, new id.
      existing.rpcId = r.id;
      existing.gen = this.gen;
      existing.mapped = mapped;
      return;
    }
    this.requests.set(requestId, { requestId, rpcId: r.id, gen: this.gen, mapped, answered: null });
    const st = this.state(mapped.threadId);
    const turnId = mapped.turnId || st.ownTurn || st.foreignTurn || "";
    this.emit({ type: "requestRaised", thread: mapped.threadId, turnId, requestId, request: mapped.request, blocking: mapped.blocking });
  }

  async respond(thread: NativeId, requestId: string, response: RequestResponse): Promise<void> {
    const live = this.requests.get(requestId);
    if (!live || live.mapped.threadId !== thread) throw new Error(`request ${requestId} is not pending in this Codex thread`);
    const conn = await this.ensure();
    if (live.gen !== this.gen) throw new Error("the request has not been re-delivered since the Codex connection was re-established");
    const body = live.mapped.encode(response);
    live.answered = response;
    conn.reply(live.rpcId, body);
    // Confirmed by `serverRequest/resolved` (or the turn ending).
  }

  private resolveRequest(live: LiveRequest): void {
    this.requests.delete(live.requestId);
    this.emit({
      type: "requestResolved",
      thread: live.mapped.threadId,
      requestId: live.requestId,
      by: live.answered ? "client" : "elsewhere",
      ...(live.answered ? { answer: live.answered } : {}),
    });
  }

  // ── Effective capabilities and control ──────────────────────────────────────────────────

  async effective(thread: NativeId): Promise<ThreadEffective> {
    const st = this.state(thread);
    if (!st.model) await this.readThreadMeta(thread).catch(() => null);
    const model = st.model ? this.models.find((m) => m.id === st.model) : this.models.find((m) => m.isDefault);
    const image = model?.capabilities?.find((c) => c.capability === "imageInput")?.support ?? "unknown";
    const efforts = model?.efforts ?? [];
    const base: Capability[] = [
      "threadList",
      "threadRead",
      "threadResume",
      "threadFork",
      "threadForkAtTurn",
      "threadRename",
      "threadArchive",
      "threadDelete",
      "followExternal",
      "streamText",
      "streamReasoning",
      "interrupt",
      "steer",
      "approvals",
      "userQuestions",
      "userForms",
      "modelSwitch",
      "tokenUsage",
      "contextUsage",
      "modelList",
      "plan",
      "webSearch",
      "reasoningSummary",
      "modes",
    ];
    if (efforts.length > 0) base.push("effortSwitch", "reasoning");
    if (image === "supported") base.push("imageInput");
    return {
      capabilities: base,
      imageInput: image,
      imageFormats: [],
      efforts,
      modes: CODEX_MODES.map(modeInfo),
      applied: { model: st.model ?? model?.id ?? null, effort: st.effort ?? model?.defaultEffort ?? null, mode: st.mode },
    };
  }

  async control(thread: NativeId): Promise<ExternalControl> {
    const st = this.state(thread);
    try {
      await this.readThreadMeta(thread);
    } catch (e) {
      if (e instanceof CodexRpcError && /not loaded|no rollout|not found/i.test(e.message) && this.conn?.isOpen) {
        // Never materialised or gone from disk: nobody can be driving it.
        if (!st.ownTurn && !st.foreignTurn) return { external: false };
      }
      return { external: false, uncertain: true, holder: `Codex (${(e as Error).message})` };
    }
    const c = this.localControl(st);
    st.lastControl = JSON.stringify(c);
    return c;
  }

  // ── Fork and lifecycle ───────────────────────────────────────────────────────────────────

  async forkThread(thread: NativeId, opts: ForkOptions): Promise<NativeId> {
    const conn = await this.ensure();
    let lastTurnId = opts.atTurn ?? null;
    if (!lastTurnId) {
      // The MVP forks from the last completed turn, never through one in progress.
      const turns = (await conn.request("thread/turns/list", { threadId: thread, limit: 5, sortDirection: "desc" })) as Paged<Turn>;
      const running = (turns.data ?? []).some((t) => t.status === "inProgress");
      if (running) {
        const done = (turns.data ?? []).find((t) => t.status !== "inProgress");
        if (!done) throw new Error("the thread has no completed turn to fork from yet");
        lastTurnId = done.id;
      }
    }
    const mode = findMode(opts.mode);
    const { params: extras, bind } = await this.setupWithBind({ phase: "fork", thread, cwd: opts.cwd });
    const r = (await conn.request("thread/fork", {
      threadId: thread,
      excludeTurns: true,
      ...(lastTurnId ? { lastTurnId } : {}),
      ...(opts.model ? { model: opts.model } : {}),
      ...(opts.cwd ? { cwd: opts.cwd } : {}),
      ...(mode ? { approvalPolicy: mode.approvalPolicy, sandbox: mode.sandbox } : {}),
      ...extras,
    })) as { thread: Thread; model?: string; reasoningEffort?: string | null };
    bind?.(r.thread.id);
    const st = this.state(r.thread.id);
    st.subscribed = true;
    st.status = r.thread.status ?? { type: "idle" };
    st.model = r.model ?? r.thread.model ?? null;
    st.effort = opts.effort ?? r.reasoningEffort ?? null;
    st.pendingEffort = opts.effort ?? null;
    st.mode = mode?.id ?? null;
    st.lastControl = JSON.stringify({ external: false });
    return r.thread.id;
  }

  async rename(thread: NativeId, title: string): Promise<void> {
    const conn = await this.ensure();
    await conn.request("thread/name/set", { threadId: thread, name: title });
  }

  async archive(thread: NativeId): Promise<void> {
    const conn = await this.ensure();
    await conn.request("thread/archive", { threadId: thread });
  }

  async unarchive(thread: NativeId): Promise<void> {
    const conn = await this.ensure();
    await conn.request("thread/unarchive", { threadId: thread });
  }

  async remove(thread: NativeId): Promise<void> {
    const conn = await this.ensure();
    await conn.request("thread/delete", { threadId: thread });
  }

  async dispose(): Promise<void> {
    this.disposed = true;
    if (this.reconnectTimer) clearTimeout(this.reconnectTimer);
    this.reconnectTimer = null;
    // Closing our connection unsubscribes us; it never stops the Codex daemon.
    this.conn?.close();
    this.conn = null;
  }

  // ── Notifications ────────────────────────────────────────────────────────────────────────

  private onNotification(method: string, raw: unknown): void {
    try {
      this.handleNotification(method, raw);
    } catch (e) {
      this.ctx.log(`Codex notification ${method} could not be handled: ${(e as Error).message}`);
    }
  }

  private handleNotification(method: string, raw: unknown): void {
    const p = (raw && typeof raw === "object" ? raw : {}) as Record<string, unknown>;
    switch (method) {
      case "thread/started": {
        const t = p.thread as Thread | undefined;
        if (!t?.id) return;
        const st = this.state(t.id);
        if (t.status && !st.status) st.status = t.status;
        this.emit({ type: "threadChanged", info: threadInfo(t, false) });
        return;
      }
      case "thread/status/changed": {
        const id = p.threadId as string | undefined;
        if (!id) return;
        const st = this.state(id);
        st.status = (p.status as ThreadStatus) ?? null;
        if (st.status?.type === "notLoaded") {
          st.foreignSeen = false;
          st.subscribed = false;
        }
        if (!this.isActive(st.status) && st.foreignTurn && !st.subscribed) st.foreignTurn = null;
        this.publishControl(st);
        return;
      }
      case "thread/closed": {
        const id = p.threadId as string | undefined;
        if (!id) return;
        const st = this.state(id);
        st.status = { type: "notLoaded" };
        st.subscribed = false;
        st.foreignSeen = false;
        this.publishControl(st);
        return;
      }
      case "thread/name/updated":
      case "thread/archived":
      case "thread/unarchived": {
        const id = p.threadId as string | undefined;
        if (!id) return;
        const archived = method === "thread/archived";
        void this.readThreadMeta(id)
          .then((t) => this.emit({ type: "threadChanged", info: threadInfo(t, method === "thread/name/updated" ? false : archived) }))
          .catch(() => {});
        return;
      }
      case "thread/deleted": {
        const id = p.threadId as string | undefined;
        if (!id) return;
        this.threads.delete(id);
        this.emit({ type: "threadRemoved", thread: id });
        return;
      }
      case "serverRequest/resolved": {
        const rid = p.requestId as number | string | undefined;
        for (const live of this.requests.values()) {
          if (live.gen === this.gen && live.rpcId === rid && live.mapped.threadId === p.threadId) {
            this.resolveRequest(live);
            return;
          }
        }
        return;
      }
      case "account/updated": {
        // Sign-in changed since the last probe: yaacd re-probes.
        const mode = typeof p.authMode === "string" ? p.authMode.toLowerCase() : null;
        if (this.authMode !== undefined && mode !== this.authMode) {
          this.authMode = mode;
          this.emit({ type: "descriptorChanged" });
        }
        return;
      }
      default: {
        const id = typeof p.threadId === "string" ? p.threadId : null;
        if (!id) return; // account, MCP, fs, … notifications: not about a thread
        const st = this.state(id);
        if (st.starting) {
          st.held.push({ method, params: p });
          return;
        }
        this.onThreadNotification(st, method, p);
      }
    }
  }

  private turnOf(st: ThreadState, p: Record<string, unknown>): TurnId {
    return (typeof p.turnId === "string" && p.turnId) || st.ownTurn || st.foreignTurn || "";
  }

  private onThreadNotification(st: ThreadState, method: string, p: Record<string, unknown>): void {
    const native = st.native;
    switch (method) {
      case "turn/started": {
        const turn = p.turn as Turn | undefined;
        if (!turn?.id) return;
        st.turnsSeen.add(turn.id);
        if (turn.id === st.ownTurn) return; // announced when turn/start returned
        st.foreignTurn = turn.id;
        st.foreignSeen = true;
        st.usageAtStart = st.lastUsage;
        st.plan = null;
        this.emit({ type: "turnStarted", thread: native, turnId: turn.id, origin: "external", at: new Date().toISOString() });
        this.publishControl(st);
        return;
      }
      case "turn/completed": {
        const turn = p.turn as Turn | undefined;
        if (!turn?.id) return;
        const whose = turn.id === st.ownTurn ? "own" : turn.id === st.foreignTurn ? "foreign" : null;
        if (!whose) return;
        this.finishTurn(st, turn, whose);
        return;
      }
      case "item/started":
      case "item/completed": {
        const item = p.item as ThreadItem | undefined;
        if (!item?.id) return;
        const turnId = this.turnOf(st, p);
        if (item.type === "userMessage" && this.isOwnInput(st, item, turnId, method === "item/completed")) return;
        const mapped = mapItem(item, turnId, method === "item/started" ? "inProgress" : "completed");
        this.emit({ type: method === "item/started" ? "itemStarted" : "itemCompleted", thread: native, item: mapped });
        return;
      }
      case "item/agentMessage/delta":
        return this.delta(st, p, "text");
      case "item/reasoning/textDelta":
      case "item/reasoning/summaryTextDelta":
        return this.delta(st, p, "reasoning");
      case "item/reasoning/summaryPartAdded":
        if (typeof p.summaryIndex === "number" && p.summaryIndex > 0 && typeof p.itemId === "string") {
          this.emit({ type: "itemDelta", thread: native, itemId: p.itemId, field: "reasoning", delta: "\n\n" });
        }
        return;
      case "item/commandExecution/outputDelta":
      case "item/fileChange/outputDelta":
        return this.delta(st, p, "output");
      case "turn/plan/updated": {
        const turnId = this.turnOf(st, p);
        const steps = (Array.isArray(p.plan) ? p.plan : []).map((s) => {
          const o = (s ?? {}) as Record<string, unknown>;
          return { text: String(o.step ?? ""), state: planState(o.status) };
        });
        const item: Item = { id: `plan:${turnId}`, turnId, status: "inProgress", kind: "plan", steps, explanation: typeof p.explanation === "string" ? p.explanation : null };
        this.emit({ type: st.plan ? "itemUpdated" : "itemStarted", thread: native, item });
        st.plan = item;
        return;
      }
      case "thread/tokenUsage/updated": {
        const usage = p.tokenUsage as ThreadTokenUsage | undefined;
        if (!usage) return;
        st.lastUsage = usage;
        this.emit({
          type: "usageUpdated",
          thread: native,
          tokens: tokenUsage(usage.total),
          context: { used: usage.last?.totalTokens ?? null, window: usage.modelContextWindow ?? null },
        });
        return;
      }
      case "model/rerouted": {
        const to = typeof p.toModel === "string" ? p.toModel : null;
        this.notice(st, p, "modelRerouted", `Codex switched this turn from ${String(p.fromModel ?? "the requested model")} to ${String(to ?? "another model")}.`, { reason: p.reason ?? null });
        if (to) {
          st.model = to;
          this.emit({ type: "settingsChanged", thread: native, applied: { model: to, effort: st.effort } });
        }
        return;
      }
      case "error": {
        const err = (p.error ?? {}) as Record<string, unknown>;
        const retry = p.willRetry === true;
        this.notice(st, p, retry ? "warning" : "error", String(err.message ?? "Codex reported an error"), { willRetry: retry, details: err.additionalDetails ?? null });
        return;
      }
      case "warning":
      case "guardianWarning":
      case "configWarning":
        if (typeof p.message === "string") this.notice(st, p, "warning", p.message, null);
        return;
      default:
        return; // hooks, diffs, moderation, goals, realtime, … : nothing the item model needs
    }
  }

  private isOwnInput(st: ThreadState, item: ThreadItem, turnId: TurnId, completed: boolean): boolean {
    const clientId = typeof item.clientId === "string" ? item.clientId : null;
    if (clientId) return st.sentClientIds.has(clientId);
    if (turnId !== st.ownTurn || st.ownInputsPending <= 0) return false;
    if (completed) st.ownInputsPending -= 1;
    return true;
  }

  private delta(st: ThreadState, p: Record<string, unknown>, field: "text" | "reasoning" | "output"): void {
    if (typeof p.itemId !== "string" || typeof p.delta !== "string" || p.delta.length === 0) return;
    this.emit({ type: "itemDelta", thread: st.native, itemId: p.itemId, field, delta: p.delta });
  }

  private notice(st: ThreadState, p: Record<string, unknown>, noticeKind: "error" | "warning" | "modelRerouted", text: string, details: unknown): void {
    const turnId = this.turnOf(st, p);
    st.notices += 1;
    const item: Item = { id: `notice:${turnId || "thread"}:${st.notices}`, turnId, status: "completed", kind: "notice", noticeKind, text, ...(details ? { details } : {}) };
    this.emit({ type: "itemStarted", thread: st.native, item });
    this.emit({ type: "itemCompleted", thread: st.native, item });
  }

  private finishTurn(st: ThreadState, turn: Turn, whose: "own" | "foreign"): void {
    const native = st.native;
    if (st.plan && st.plan.turnId === turn.id) {
      this.emit({ type: "itemCompleted", thread: native, item: { ...st.plan, status: "completed" } as Item });
      st.plan = null;
    }
    for (const live of [...this.requests.values()]) {
      if (live.mapped.threadId !== native || (live.mapped.turnId && live.mapped.turnId !== turn.id)) continue;
      if (live.answered) this.resolveRequest(live);
      else {
        this.requests.delete(live.requestId);
        this.emit({ type: "requestExpired", thread: native, requestId: live.requestId });
      }
    }
    const status = turnStatus(turn.status);
    const error = turn.error?.message ?? null;
    const usage = usageDelta(st.lastUsage, st.usageAtStart);
    if (whose === "own") {
      st.ownTurn = null;
      st.ownInputsPending = 0;
      st.sentClientIds.clear();
      delete this.persisted.ownTurns[native];
      this.save();
    } else {
      st.foreignTurn = null;
    }
    this.emit({ type: "turnCompleted", thread: native, turnId: turn.id, status, error, usage, at: new Date().toISOString() });
    if (whose === "foreign" && st.subscribed && this.conn?.isOpen) {
      // Someone else has this thread open. Let go of it, so that Codex can unload it once they
      // do too; until then it is loaded-but-not-ours, which reads as in use.
      st.subscribed = false;
      void this.conn.request("thread/unsubscribe", { threadId: native }).catch(() => {});
    }
    this.publishControl(st);
  }
}

// ── Helpers ────────────────────────────────────────────────────────────────────────────────

/** The server's own version, from its user agent (`<client>/<version> (…)`). */
export function serverVersion(userAgent: string | undefined): string | null {
  if (!userAgent) return null;
  const m = /^[^/\s]+\/(\S+)/.exec(userAgent);
  return m ? m[1]! : null;
}

/** The store identity key: the server's CODEX_HOME. Sign-in is not part of it (it can change). */
export function storeKey(codexHome: string): string {
  let real = codexHome;
  try {
    real = fs.realpathSync(codexHome);
  } catch {
    /* a remote or vanished path: use it as reported */
  }
  return `codex:${sha256Hex(real).slice(0, 16)}`;
}

interface ScanCursor {
  archived: boolean;
  cursor: string | null;
  count?: number;
}

function encodeCursor(c: ScanCursor): string {
  return JSON.stringify(c);
}

function decodeCursor(s: string | null | undefined): ScanCursor {
  if (!s) return { archived: false, cursor: null };
  try {
    const c = JSON.parse(s) as ScanCursor;
    return { archived: !!c.archived, cursor: typeof c.cursor === "string" ? c.cursor : null, count: typeof c.count === "number" ? c.count : 0 };
  } catch {
    return { archived: false, cursor: null };
  }
}

/** From "unknown variant `x`, expected one of `a`, `b`": the kinds the daemon accepts. */
export function acceptedKinds(e: unknown): string[] | null {
  const msg = e instanceof Error ? e.message : "";
  const m = /unknown variant `[^`]*`, expected one of (.*)$/s.exec(msg);
  if (!m) return null;
  const kinds = [...m[1]!.matchAll(/`([^`]+)`/g)].map((x) => x[1]!);
  return kinds.length > 0 ? kinds : null;
}
