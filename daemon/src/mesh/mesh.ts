// The agent mesh inside yaacd (docs/10-agent-mesh.md): the `yaac` tools, identity, the
// mailbox and outbox, delivery through the session layer's dispatcher, spawning, waiting,
// permission and budgets, and the daemon's side of the hub's relay.
//
// Everything a thread on this target does with another thread on this target happens here,
// with no hub (D73). Only work that involves another target goes through the hub, over the
// one connection the hub holds to this target.

import { PR_TOOL_NAMES } from "../forge/tools.js";
import { PrToolError, type PrTools } from "../forge/pr-tools.js";
import * as fs from "node:fs";
import * as path from "node:path";
import { randomBytes } from "node:crypto";
import type {
  Address,
  Item,
  MeshBudget,
  MeshBudgetResult,
  MeshBudgetUsage,
  MeshMessage,
  MeshPeer,
  MeshPermissionAction,
  MeshQuery,
  MeshQueryParams,
  MeshThreadFilter,
  MeshThreadInfo,
  ModeInfo,
  ProviderDescriptor,
  Receipt,
  ReceiptReason,
  RequestResponse,
  SpawnedThread,
  StopResult,
  ThreadHold,
  ThreadState,
  ThreadSummary,
  TranscriptEntry,
  TurnId,
  TurnOrigin,
} from "../protocol/generated/types.js";
import { RpcError } from "../protocol/rpc.js";
import { hasModelBestUse, listForAgents } from "../providers/model-best-use.js";
import { PeerInputDenied } from "../providers/types.js";
import type { MeshAttachment, MeshHooks, MeshInputMessage, MeshToolCall, MeshToolResult, NativeId } from "../providers/types.js";
import type { ClientSession, MethodHandler } from "../session/client.js";
import type { ClientLink, Core, CoreMeshHooks, DispatchHandle } from "../session/core.js";
import type { RequestRecord, ThreadRuntime } from "../session/thread.js";
import { artifactBytes } from "../session/files.js";
import { gitRunner } from "../session/git.js";
import { normaliseRemote } from "../session/fsinfo.js";
import { nowIso, randomId } from "../util/fsx.js";
import { clip, frameMessages } from "./format.js";
import { BROWSER_TOOLS, inspectBrowser } from "./browser.js";
import { createMeshServer, readable } from "./mcp.js";
import { MeshStore, NO_TIME_LIMIT, type MessageRecord, type RootState } from "./store.js";
import { chooseWorkerMode, ModeRefused, type ParentMode } from "./modes.js";
import { identityBlock, TOOL_TIMEOUT_SECS, WAIT_MAX_SECS } from "./tools.js";

/** How long an orchestration counts as live for discovery after a person last acted on it. */
const LIVE_WINDOW_MS = 12 * 3600 * 1000;

/** The target part of addresses before the hub has told this target its id. */
export const LOCAL_TARGET = "local";
const ACTIVE_WINDOW_MS = 60 * 60 * 1000;
const SNAPSHOT_TTL_MS = 5 * 60 * 1000;
const PAGE_DEFAULT = 20;
const PAGE_MAX = 50;
const FIND_PAGE = 10;

export interface MeshOptions {
  log: (line: string) => void;
  /** The daemon's socket, which `yaacd mcp` connects back to. */
  socketPath: string;
  home: string;
  env: NodeJS.ProcessEnv;
  /** How the stdio bridge is launched (default: this runtime, this bundle, `mcp`). */
  bridgeCommand?: { command: string; args: string[] };
  /** Longest `wait` (tests shorten it). */
  waitMaxMs?: number;
  /** How long a tool call waits for the user's answer to a permission prompt. */
  permissionWaitMs?: number;
  /** How often a `wait` on another target's thread asks for its state. */
  remotePollMs?: number;
  /** The housekeeping timer (undelivered notices, outbox retries). 0: off. */
  tickMs?: number;
  /** How often a parent asks for the state of its workers on other targets (live workers item). 0: off. */
  workerPollMs?: number;
  /** How long a stop waits for an interrupted turn to end before it reports "did not respond". */
  stopWaitMs?: number;
}

/** A tool failure the agent can read. */
class ToolError extends Error {}

interface Waiter {
  threadId: string;
  turnId: TurnId | null;
  forMessages: boolean;
  watch: Address[];
  itemId: string;
  done: boolean;
  finish: (text: string) => void;
}

interface Snapshot {
  key: string;
  rows: MeshThreadInfo[];
  at: number;
  header: string;
}

type Resolved = { kind: "local"; rt: ThreadRuntime; address: Address } | { kind: "remote"; address: Address; target: string };

const FINISHED: ThreadState[] = ["idle", "done", "failed", "needsAttention", "stopped"];

function isActive(s: ThreadSummary, now: number): boolean {
  if (s.archived) return false;
  if (s.state === "running" || s.state === "waiting" || s.state === "needsAttention") return true;
  if (s.originKind === "programmatic") return false;
  return now - Date.parse(s.updatedAt) <= ACTIVE_WINDOW_MS;
}

function words(s: string): string[] {
  return s.toLowerCase().split(/[^\p{L}\p{N}]+/u).filter(Boolean);
}

/** Levenshtein distance, on characters (the hub's `find::distance`). */
function distance(a: string[], b: string[]): number {
  let prev = Array.from({ length: b.length + 1 }, (_, i) => i);
  let cur = new Array<number>(b.length + 1).fill(0);
  for (let i = 0; i < a.length; i++) {
    cur[0] = i + 1;
    for (let j = 0; j < b.length; j++) cur[j + 1] = Math.min(prev[j]! + (a[i] === b[j] ? 0 : 1), prev[j + 1]! + 1, cur[j]! + 1);
    [prev, cur] = [cur, prev];
  }
  return prev[b.length]!;
}

/** One query word against one title word, 0–1 (the hub's `find::word_score`). */
function wordScore(q: string, t: string): number {
  if (q === t) return 1;
  const qc = [...q];
  const tc = [...t];
  if (t.startsWith(q)) return 0.8 + 0.15 * (qc.length / tc.length);
  if (qc.length >= 3 && t.includes(q)) return 0.7;
  const allowed = qc.length <= 3 ? 0 : qc.length <= 6 ? 1 : 2;
  if (allowed === 0) return 0;
  const whole = distance(qc, tc);
  const prefix = tc.length > qc.length ? distance(qc, tc.slice(0, qc.length)) : whole;
  const d = Math.min(whole, prefix);
  if (d > allowed) return 0;
  return (d === whole ? 0.65 : 0.55) - 0.1 * (d - 1);
}

/**
 * Fuzzy title score, 0–1; 0 when a query word matches nothing. The same matching as the hub's
 * `find::title_score`, so a search with the hub away ranks as one with it would.
 */
export function titleScore(query: string, title: string): number {
  const q = words(query);
  const t = words(title);
  if (q.length === 0 || t.length === 0) return 0;
  let total = 0;
  const used = new Array<boolean>(t.length).fill(false);
  for (const qw of q) {
    let best = 0;
    let at = -1;
    t.forEach((tw, i) => {
      const s = wordScore(qw, tw) - (used[i] ? 0.2 : 0);
      if (s > best) {
        best = s;
        at = i;
      }
    });
    if (best <= 0) return 0;
    if (at >= 0) used[at] = true;
    total += best;
  }
  let score = total / q.length;
  if (wordScore(q[0]!, t[0]!) > 0) score += 0.02;
  return Math.min(score, 1);
}

function looksLikeId(q: string): boolean {
  const s = q.trim();
  return s.length >= 4 && !/\s/.test(s) && /[\d_\-/:.]/.test(s);
}

export class Mesh implements CoreMeshHooks {
  readonly store: MeshStore;
  private hub: ClientSession | null = null;
  private readonly waiters = new Map<string, Waiter>();
  /** What each local root last said of its budget (`exhausted:held`), D164. */
  private readonly budgetReported = new Map<string, string>();
  private readonly dispatches = new Map<string, DispatchHandle>();
  private readonly kickScheduled = new Set<string>();
  private readonly pairLog = new Map<string, number[]>();
  private readonly lastText = new Map<string, { turnId: TurnId; text: string }>();
  private readonly permissionWaiters = new Map<string, Array<(answer: { decision: string } | null) => void>>();
  private readonly permissionRoots = new Map<string, Address>();
  private readonly pendingRoots = new Map<string, Address>();
  private readonly snapshots = new Map<string, Snapshot>();
  private readonly git;
  private timer: NodeJS.Timeout | null = null;
  private workerTimer: NodeJS.Timeout | null = null;
  private workerPolling = false;
  /** Threads a `now` delivery is being made to. */
  private readonly nowInFlight = new Set<string>();
  private readonly peerInFlight = new Set<string>();
  private readonly receiptsInFlight = new Set<string>();
  private readonly legacyReceiptHubs = new WeakSet<ClientSession>();
  /** `alwaysAsk`: the open prompt of each thread. */
  private readonly eachPrompt = new Map<string, string>();
  private readonly stopWaitMs: number;
  private closed = false;
  private readonly waitMaxMs: number;
  private readonly permissionWaitMs: number;
  private readonly remotePollMs: number;

  constructor(
    private readonly core: Core,
    private readonly opts: MeshOptions,
  ) {
    this.store = new MeshStore(opts.home);
    this.git = gitRunner(opts.env);
    this.waitMaxMs = Math.min(opts.waitMaxMs ?? WAIT_MAX_SECS * 1000, WAIT_MAX_SECS * 1000);
    this.permissionWaitMs = opts.permissionWaitMs ?? 40_000;
    this.remotePollMs = opts.remotePollMs ?? 5000;
    this.stopWaitMs = opts.stopWaitMs ?? 5000;
    core.mesh = this;
    core.registry.setMeshHooks((instanceId) => this.hooksFor(instanceId));
    core.heldAndIdle.on((threadId) => this.kick(threadId));
  }

  // ── Life cycle ───────────────────────────────────────────────────────────────────────

  /** After the core has started: reconcile what a crash may have left half-done. */
  start(): void {
    for (const r of this.store.all()) {
      if (!r.inbox) continue;
      if (r.inState === "dispatching" && !r.turnId) {
        // The intent was recorded and the turn id was not: yaacd cannot tell whether the harness
        // started a turn with it. It is shown, never re-sent (a retry could be a second paid turn).
        const rt = r.recipient ? this.core.thread(r.recipient) : undefined;
        if (rt && !rt.deleted) {
          this.noticeItem(rt, "warning", `yaacd stopped while handing over a message from ${this.labelOf(r.message.from)}. It may or may not have reached the agent; it will not be sent again automatically.`);
        }
        this.updateReceipt(r, { status: "outcomeUnknown", detail: "yaacd stopped during handoff. It may have reached the agent and will not be sent again automatically" }, { inState: "outcomeUnknown" });
      } else if (r.inState === "offered") {
        this.store.update(r.message.id, { inState: "pending", offered: undefined });
      }
    }
    for (const id of this.threadsWithMail()) this.kick(id);
    const tick = this.opts.tickMs ?? 30_000;
    if (tick > 0) {
      this.timer = setInterval(() => this.tick(), tick);
      this.timer.unref();
    }
    const poll = this.opts.workerPollMs ?? 10_000;
    if (poll > 0) {
      this.workerTimer = setInterval(() => void this.pollRemoteWorkers(), poll);
      this.workerTimer.unref();
    }
  }

  close(): void {
    this.closed = true;
    if (this.timer) clearInterval(this.timer);
    if (this.workerTimer) clearInterval(this.workerTimer);
    for (const w of this.waiters.values()) w.finish("yaacd is stopping; call wait again.");
    this.store.save();
    this.store.close();
  }

  // ── Identity: addresses and labels ─────────────────────────────────────────────────

  selfId(): string {
    return this.store.state.self?.id ?? LOCAL_TARGET;
  }

  selfName(): string {
    return this.store.state.self?.name ?? "this machine";
  }

  addressOf(threadId: string): Address {
    return `${this.selfId()}/${threadId}`;
  }

  private isSelfTarget(t: string): boolean {
    return t === LOCAL_TARGET || t === this.selfId() || (!!this.store.state.self && t === this.store.state.self.name);
  }

  private targetIdOf(nameOrId: string): string | null {
    if (this.isSelfTarget(nameOrId)) return this.selfId();
    const e = this.store.state.roster.find((r) => r.id === nameOrId || r.name.toLowerCase() === nameOrId.toLowerCase());
    return e ? e.id : null;
  }

  private targetNameOf(id: string): string {
    if (this.isSelfTarget(id)) return this.selfName();
    return this.store.state.roster.find((r) => r.id === id)?.name ?? id;
  }

  /** Splits an address; a bare thread id is on this target. */
  private split(address: string): { target: string; thread: string } | null {
    const a = address.trim();
    const i = a.indexOf("/");
    if (i < 0) return a.startsWith("t_") ? { target: this.selfId(), thread: a } : null;
    const target = a.slice(0, i);
    const thread = a.slice(i + 1);
    return target && thread ? { target, thread } : null;
  }

  private normalise(address: Address): Address {
    const s = this.split(address);
    if (!s) return address;
    if (this.isSelfTarget(s.target)) return this.addressOf(s.thread);
    const id = this.targetIdOf(s.target);
    return `${id ?? s.target}/${s.thread}`;
  }

  /** The local thread an address names, if it names one on this target. */
  private localOf(address: Address): ThreadRuntime | undefined {
    const s = this.split(address);
    if (!s || !this.isSelfTarget(s.target)) return undefined;
    const rt = this.core.thread(s.thread);
    return rt && !rt.deleted ? rt : undefined;
  }

  private isLocalAddress(address: Address): boolean {
    const s = this.split(address);
    return !!s && this.isSelfTarget(s.target);
  }

  /** What an agent wrote as an address: an address, a YAAC id, or a harness id on this target. */
  private resolve(input: unknown, what = "thread"): Resolved {
    if (typeof input !== "string" || !input.trim()) throw new ToolError(`Name the ${what} by its address.`);
    const raw = input.trim();
    const s = this.split(raw);
    if (s && this.isSelfTarget(s.target)) {
      const rt = this.core.thread(s.thread);
      if (rt && !rt.deleted) return { kind: "local", rt, address: this.addressOf(rt.id) };
      throw new ToolError(`There is no thread ${raw} on ${this.selfName()} (it may have been deleted). Use find_thread to look it up.`);
    }
    if (s) {
      const id = this.targetIdOf(s.target);
      if (!id && this.store.state.roster.length > 0) throw new ToolError(`There is no machine called ${s.target} in this mesh. list_targets shows the machines.`);
      return { kind: "remote", address: `${id ?? s.target}/${s.thread}`, target: id ?? s.target };
    }
    // A harness's own conversation id, on this target.
    for (const rt of this.core.allThreads()) {
      if (rt.record.native === raw || rt.id === raw) return { kind: "local", rt, address: this.addressOf(rt.id) };
    }
    throw new ToolError(`${raw} is not an address this machine knows. Use list_threads or find_thread to get the thread's address.`);
  }

  private providerName(instance: string): string {
    return this.core.registry.get(instance)?.descriptor.displayName ?? instance;
  }

  peerOf(rt: ThreadRuntime): MeshPeer {
    const s = this.core.summaryOf(rt);
    return {
      address: this.addressOf(rt.id),
      title: s.title,
      provider: this.providerName(rt.record.instance),
      ...(s.model ? { model: s.model } : {}),
      targetName: this.selfName(),
      reachable: true,
    };
  }

  private peerFor(address: Address): MeshPeer {
    const rt = this.localOf(address);
    if (rt) return this.peerOf(rt);
    const known = this.store.state.labels[this.normalise(address)];
    const s = this.split(address);
    const targetName = s ? this.targetNameOf(s.target) : null;
    const reachable = s ? this.store.state.roster.find((r) => r.id === s.target)?.reachable ?? null : null;
    const out: MeshPeer = { address: this.normalise(address), ...(targetName ? { targetName } : {}), ...(reachable !== null ? { reachable } : {}) };
    if (known?.title) out.title = known.title;
    if (known?.provider) out.provider = known.provider;
    if (known?.model) out.model = known.model;
    return out;
  }

  private remember(peer: MeshPeer, state?: ThreadState | null): void {
    if (this.isLocalAddress(peer.address)) return;
    const prev = this.store.state.labels[peer.address] ?? { address: peer.address };
    this.store.state.labels[peer.address] = { ...prev, ...peer, ...(state ? { state } : {}), seenAt: nowIso() };
  }

  /** `Codex agent "Orchestrator" on devbox`. */
  labelOf(address: Address): string {
    const p = this.peerFor(address);
    const who = p.provider ? `${p.provider} agent` : "agent";
    const title = p.title ? ` "${p.title}"` : ` ${p.address}`;
    return `${who}${title} on ${p.targetName ?? "another machine"}`;
  }

  private line(info: MeshThreadInfo, lastKnown = false): string {
    const p = info.peer;
    const model = p.model ? ` / ${p.model}` : "";
    return `- ${p.address} "${p.title ?? "Untitled"}" — ${info.state}${lastKnown ? " (last known)" : ""} · ${p.provider ?? "?"}${model} · ${p.targetName ?? "?"}`;
  }

  private infoOf(rt: ThreadRuntime): MeshThreadInfo {
    const s = this.core.summaryOf(rt);
    return { peer: this.peerOf(rt), state: s.state, ...(s.spawnedBy ? { spawnedBy: s.spawnedBy } : {}), cwd: s.cwd, ...(s.project ? { project: s.project } : {}) };
  }

  // ── Attachment: tickets and the provider hooks ─────────────────────────────────────

  private hooksFor(instanceId: string): MeshHooks {
    return { open: (req) => this.open(instanceId, req) };
  }

  /** The running daemon's own entry script (what `serve` is started with). */
  private daemonEntry(): string {
    return process.argv[1] ? path.resolve(process.argv[1]) : "";
  }

  private bridgeCommand(): { command: string; args: string[] } {
    if (this.opts.bridgeCommand) return this.opts.bridgeCommand;
    const script = this.daemonEntry();
    // The small bridge bundle beside the daemon's, when there is one (a built yaacd).
    const small = script ? path.join(path.dirname(script), "yaacd-mcp.mjs") : "";
    if (small && fs.existsSync(small)) return { command: process.execPath, args: [...process.execArgv, small] };
    return { command: process.execPath, args: [...process.execArgv, script, "mcp"] };
  }

  private open(instanceId: string, req: { native: NativeId | null; cwd: string | null; phase: "start" | "resume" | "fork" }): MeshAttachment | null {
    if (this.closed) return null;
    // One ticket per thread: a resume reuses the thread's ticket (a loaded Codex thread keeps
    // the bridge it was started with).
    let ticket: string | null = null;
    if (req.native && req.phase === "resume") {
      for (const [t, v] of Object.entries(this.store.state.tickets)) if (v.instance === instanceId && v.native === req.native) ticket = t;
    }
    if (!ticket) {
      ticket = randomBytes(32).toString("hex");
      this.store.state.tickets[ticket] = { instance: instanceId, native: req.native, createdAt: nowIso() };
      this.store.save();
    }
    const t = ticket;
    const rt = req.native ? this.core.threadByNative(instanceId, req.native) : undefined;
    const identity = identityBlock({
      address: rt ? this.addressOf(rt.id) : null,
      title: rt ? this.core.summaryOf(rt).title : null,
      provider: this.providerName(instanceId),
      target: this.selfName(),
    });
    const call = (c: MeshToolCall) => this.callWithTicket(t, c);
    return {
      ticket: t,
      identity,
      toolTimeoutSec: TOOL_TIMEOUT_SECS,
      bind: (native) => {
        const rec = this.store.state.tickets[t];
        if (!rec) return;
        if (rec.native && rec.native !== native) return; // a ticket is bound once
        rec.native = native;
        this.store.save();
      },
      call,
      inProcessServer: () => createMeshServer(call),
      stdio: () => {
        const cmd = this.bridgeCommand();
        return {
          command: cmd.command,
          args: cmd.args,
          env: {
            YAAC_MCP_TICKET: t,
            YAAC_SOCKET: this.opts.socketPath,
            YAAC_HOME: this.opts.home,
            YAAC_DAEMON_ENTRY: this.daemonEntry(),
          },
        };
      },
    };
  }

  /** A tool call from a bridge or an in-process server: the ticket says who is calling. */
  async callWithTicket(ticket: string, call: MeshToolCall): Promise<MeshToolResult> {
    try {
      const rec = this.store.state.tickets[ticket];
      if (!rec) throw new ToolError("This yaac tool attachment is not recognised by yaacd.");
      // A ticket is bound before the first turn; a call can only come from inside a turn. Allow
      // a moment for the thread's record to appear (the first turn may call at once).
      let rt: ThreadRuntime | undefined;
      for (let i = 0; i < 100; i++) {
        if (rec.native) rt = this.core.threadByNative(rec.instance, rec.native);
        if (rt) break;
        await new Promise((r) => setTimeout(r, 50));
      }
      if (!rec.native || !rt) throw new ToolError("This thread is not yet known to yaacd; try again in a moment.");
      // Where the harness states the calling thread, it must be the ticket's.
      const metaThread = call.meta && typeof call.meta.threadId === "string" ? call.meta.threadId : null;
      // Diagnostics: which call-metadata fields the harness sends (names only, never values).
      this.opts.log(`mesh: ${call.tool} via ${rec.instance}; call metadata: ${call.meta ? Object.keys(call.meta).join(",") || "(empty)" : "(none)"}${metaThread ? `; threadId ${metaThread === rec.native ? "matches" : "does not match"} the ticket` : ""}`);
      if (metaThread && metaThread !== rec.native) {
        this.opts.log(`mesh: a tool call's thread ${metaThread} does not match its ticket's ${rec.native}; refused`);
        throw new ToolError("This call's thread does not match the yaac attachment it came through; it was refused.");
      }
      return await this.callTool(rt, call);
    } catch (e) {
      return { text: e instanceof ToolError || e instanceof RpcError ? e.message : readable(e), isError: true };
    }
  }

  // ── The tools ───────────────────────────────────────────────────────────────────

  /** The pull-request tools (D121), when the forge is attached. */
  prTools: PrTools | null = null;

  async callTool(rt: ThreadRuntime, call: MeshToolCall): Promise<MeshToolResult> {
    if (rt.deleted) return { text: "This thread no longer exists in YAAC.", isError: true };
    // Any next call confirms what the previous `wait` handed over.
    this.confirmOffered(rt.id);
    const a = call.args ?? {};
    try {
      let text: string;
      switch (call.tool) {
        case "present_artifact": {
          if (typeof a.path !== "string") throw new ToolError("provide a relative file path");
          const name = path.basename(a.path);
          const title = a.title ?? name;
          if (typeof title !== "string" || !title.trim() || Buffer.byteLength(title) > 160) throw new ToolError("provide a title of 1–160 bytes");
          const bytes = await artifactBytes(rt.record.cwd, a.path);
          if (rt.deleted) throw new ToolError("this thread no longer exists");
          const previous = rt.record.artifacts ?? [];
          const snapshot = this.core.uploads.storeArtifact(bytes, name, id => {
            // Bound index metadata before writing any new blob.
            if (previous.length >= 32 && !previous.some(row => row.uploadId === id)) throw new ToolError("this thread has presented 32 artifacts");
          });
          if (!previous.some(row => row.uploadId === snapshot.uploadId)) {
            rt.record.artifacts = [...previous, { ...snapshot, title, size: bytes.length, publishedAt: nowIso() }];
            try { rt.saveRecord(); } catch (error) { rt.record.artifacts = previous; throw error; }
          }
          this.core.updateIndex(rt);
          text = `Presented ${snapshot.name} (${snapshot.uploadId}).`;
          break;
        }
        case "whoami":
          text = this.whoami(rt);
          break;
        case "list_targets":
          text = await this.listTargets(rt);
          break;
        case "list_models":
          text = await this.listModels(rt, a);
          break;
        case "list_threads":
          text = await this.listThreads(rt, a);
          break;
        case "find_thread":
          text = await this.findThread(rt, a);
          break;
        case "read_thread":
          text = await this.readThread(rt, a);
          break;
        case "spawn_thread":
          text = await this.spawnTool(rt, a);
          break;
        case "send_message":
          text = await this.sendTool(rt, a);
          break;
        case "wait":
          text = await this.waitTool(rt, a, call.signal);
          break;
        case "notify_when_done":
          text = await this.notifyTool(rt, a);
          break;
        case "stop_thread":
          text = await this.stopTool(rt, a);
          break;
        case "stop_all":
          text = await this.stopAllTool(rt);
          break;
        default:
          const browser = BROWSER_TOOLS.find(t => t.name === call.tool);
          if (browser) {
            const hub = this.hub;
            return await inspectBrowser(browser, a, rt.id,
              hub && !hub.isClosed ? params => hub.requestHost("browser/call", params, 30_000) : null,
              async () => (await this.core.computeEffective(rt)).imageInput === "supported");
          }
          // The pull-request tools (D121), served by the forge.
          if (this.prTools && PR_TOOL_NAMES.has(call.tool)) {
            try {
              text = await this.prTools.call(rt, call.tool, a);
            } catch (e) {
              throw e instanceof PrToolError ? new ToolError(e.message) : e;
            }
            break;
          }
          throw new ToolError(`There is no yaac tool named ${call.tool}.`);
      }
      this.store.save();
      return { text, isError: false };
    } catch (e) {
      this.store.save();
      if (e instanceof ToolError) return { text: e.message, isError: true };
      if (e instanceof RpcError) return { text: this.rpcText(e), isError: true };
      this.opts.log(`mesh: ${call.tool} failed: ${(e as Error)?.stack ?? e}`);
      return { text: `The ${call.tool} call failed: ${readable(e)}`, isError: true };
    }
  }

  private rpcText(e: RpcError): string {
    if (e.code === "target_offline") return `That machine is not reachable right now (${e.message}).`;
    if (e.code === "connection_closed" || e.code === "not_initialized") return "The YAAC hub is not reachable right now, so other machines cannot be reached. Threads on this machine still can.";
    if (e.code === "timeout") return `The other machine did not answer in time (${e.message}).`;
    return e.message;
  }

  private whoami(rt: ThreadRuntime): string {
    const s = this.core.summaryOf(rt);
    const tm = this.store.peekThread(rt.id);
    const lines = [
      `Address: ${this.addressOf(rt.id)}`,
      `Title: "${s.title}"`,
      `Provider: ${this.providerName(rt.record.instance)} (instance ${rt.record.instance})${s.model ? `, model ${s.model}` : ""}`,
      `Machine: ${this.selfName()}${this.store.state.self ? "" : " (the hub has not attached yet)"}`,
      `Working directory: ${rt.record.cwd}`,
      `Spawned by: ${tm?.spawnedBy ? `${tm.spawnedBy} (${this.labelOf(tm.spawnedBy)})` : "nobody (started by a person or outside YAAC)"}`,
      `Workers: ${tm && tm.children.length > 0 ? tm.children.map((c) => `${c} (${this.labelOf(c)})`).join("; ") : "none"}`,
      `Orchestration root: ${this.rootOf(rt)}`,
      `Agents setting of this chat: ${rt.record.agents ?? "ask"}`,
    ];
    return lines.join("\n");
  }

  private async hubQuery(caller: Address, target: string, query: MeshQuery): Promise<Record<string, unknown>> {
    const hub = this.hub;
    if (!hub || hub.isClosed) throw new RpcError("not_initialized", "the hub is not reachable");
    const params: MeshQueryParams = { caller: this.normalise(caller), target, query };
    return (await hub.requestHost("mesh/query", params, 30_000)) as Record<string, unknown>;
  }

  private async listTargets(rt: ThreadRuntime): Promise<string> {
    if (this.hub && !this.hub.isClosed) {
      try {
        const r = (await this.hubQuery(this.addressOf(rt.id), this.selfId(), { tool: "listTargets" })) as { targets?: Array<{ id: string; name: string; reachable: boolean; activeThreads: number }> };
        const rows = (r.targets ?? []).map((t) => `- ${t.name}${this.isSelfTarget(t.id) ? " (this machine)" : ""} — ${t.reachable ? "reachable" : "not reachable"} · ${t.activeThreads} active thread${t.activeThreads === 1 ? "" : "s"}`);
        return [`${rows.length} machine${rows.length === 1 ? "" : "s"} in this mesh:`, ...rows].join("\n");
      } catch (e) {
        if (!(e instanceof RpcError)) throw e;
      }
    }
    const active = this.core.allThreads().filter((t) => isActive(this.core.summaryOf(t), Date.now()) && ["running", "waiting", "needsAttention"].includes(this.core.summaryOf(t).state)).length;
    if (this.store.state.roster.length === 0) {
      return `Only this machine is known: the YAAC hub has not attached here yet.\n- ${this.selfName()} (this machine) — reachable · ${active} active`;
    }
    const rows = this.store.state.roster.map((t) => (this.isSelfTarget(t.id) ? `- ${t.name} (this machine) — reachable · ${active} active` : `- ${t.name} — not reachable through the hub now (last roster said ${t.reachable ? "reachable" : "not reachable"})`));
    return [`The YAAC hub is not reachable; this is the last list it sent (${this.store.state.rosterAt ?? "time unknown"}), possibly stale:`, ...rows].join("\n");
  }

  private describeModels(providers: ProviderDescriptor[], targetName: string): string {
    const lines = [`Providers on ${targetName}:`];
    for (const d of providers) {
      if (d.status !== "ready") {
        lines.push(`- instance ${d.instance} "${d.displayName}" — not usable now (${d.status}${d.statusMessage ? `: ${d.statusMessage}` : ""})`);
        continue;
      }
      lines.push(`- instance ${d.instance} "${d.displayName}"${d.modelsComplete ? "" : " (model list may be incomplete)"}`);
      const models = (d.models ?? []).filter((model) => listForAgents(d.kind, model));
      for (const m of models) {
        const efforts = (m.efforts ?? []).map((e) => e.id).join(", ");
        lines.push(`  - model ${m.id} "${m.displayName}"${m.isDefault ? " (default)" : ""}${efforts ? ` — efforts: ${efforts}` : ""}`);
        if (hasModelBestUse(d.kind)) lines.push(`    Best used as: ${m.bestUse?.summary ?? "No guidance yet"}`);

      }
      const modes = (d.modes ?? []).map((m) => `${m.id} "${m.displayName}"${m.isDefault ? " (default)" : ""}${m.skipsApprovals ? " (never asks for approval)" : ""}`);
      if (modes.length > 0) lines.push(`  - modes (for every model of this instance): ${modes.join(", ")}`);
    }
    if (providers.length === 0) lines.push("(no provider instances)");
    lines.push("Pass an instance id and a model id to spawn_thread, and a mode id only to run the worker in a mode less permissive than your own (it inherits yours by default).");
    return lines.join("\n");
  }

  private localDescriptors(): ProviderDescriptor[] {
    return this.core.registry.all().map((i) => i.descriptor);
  }

  private async listModels(rt: ThreadRuntime, a: Record<string, unknown>): Promise<string> {
    const target = typeof a.target === "string" && a.target.trim() ? a.target.trim() : null;
    if (!target || this.isSelfTarget(target)) {
      await this.core.registry.waitForModelBestUses();
      return this.describeModels(this.localDescriptors(), this.selfName());
    }
    const id = this.targetIdOf(target);
    if (!id) throw new ToolError(`There is no machine called ${target} in this mesh.`);
    const r = (await this.hubQuery(this.addressOf(rt.id), id, { tool: "listModels" })) as { providers?: ProviderDescriptor[] };
    return this.describeModels(r.providers ?? [], this.targetNameOf(id));
  }

  // ── Listing ────────────────────────────────────────────────────────────────────────

  /** The caller's orchestration: parent and ancestors on this target, workers and theirs, peers. */
  private relatedAddresses(rt: ThreadRuntime): Address[] {
    const out = new Set<Address>();
    const self = this.addressOf(rt.id);
    const visit = (address: Address, depth: number) => {
      if (depth > 10) return;
      const local = this.localOf(address);
      if (!local) return;
      const tm = this.store.peekThread(local.id);
      for (const c of tm?.children ?? []) {
        const n = this.normalise(c);
        if (!out.has(n) && n !== self) {
          out.add(n);
          visit(n, depth + 1);
        }
      }
    };
    let cur: ThreadRuntime | undefined = rt;
    for (let i = 0; cur && i < 10; i++) {
      const p = this.store.peekThread(cur.id)?.spawnedBy;
      if (!p) break;
      out.add(this.normalise(p));
      cur = this.localOf(p);
    }
    visit(self, 0);
    for (const p of this.store.peekThread(rt.id)?.peers ?? []) out.add(this.normalise(p));
    out.delete(self);
    return [...out];
  }

  /** Whether a thread is of the provider an agent named ("Codex", "Claude", an instance id). */
  private ofProvider(info: MeshThreadInfo, provider: string | null | undefined): boolean {
    if (!provider) return true;
    const p = provider.trim().toLowerCase();
    const local = this.localOf(info.peer.address);
    const instance = local?.record.instance.toLowerCase() ?? "";
    const kind = local?.record.kind.toLowerCase() ?? "";
    const name = (info.peer.provider ?? "").toLowerCase();
    return instance === p || kind === p || name === p || name.startsWith(`${p} `) || name.includes(p);
  }

  private filterRows(rows: Array<{ info: MeshThreadInfo; summary?: ThreadSummary; updatedAt: number }>, f: { query?: string | null; state?: string | null; includeInactive?: boolean; provider?: string | null }): MeshThreadInfo[] {
    const now = Date.now();
    return rows
      .filter((r) => {
        if (f.state && r.info.state !== f.state) return false;
        if (!this.ofProvider(r.info, f.provider)) return false;
        if (f.query && titleScore(f.query, r.info.peer.title ?? "") === 0) return false;
        if (!f.includeInactive && r.summary && !isActive(r.summary, now)) return false;
        return true;
      })
      .sort((x, y) => y.updatedAt - x.updatedAt)
      .map((r) => r.info);
  }

  private localRows(pred: (rt: ThreadRuntime, s: ThreadSummary) => boolean): Array<{ info: MeshThreadInfo; summary: ThreadSummary; updatedAt: number }> {
    const out = [];
    for (const rt of this.core.allThreads()) {
      const s = this.core.summaryOf(rt);
      if (!pred(rt, s)) continue;
      out.push({ info: this.infoOf(rt), summary: s, updatedAt: Date.parse(s.updatedAt) || 0 });
    }
    return out;
  }

  private page(rows: MeshThreadInfo[], key: string, header: string, limitIn: unknown, cursor: unknown): { rows: MeshThreadInfo[]; total: number; next: string | null; header: string } {
    const now = Date.now();
    for (const [k, s] of this.snapshots) if (now - s.at > SNAPSHOT_TTL_MS) this.snapshots.delete(k);
    const limit = Math.max(1, Math.min(typeof limitIn === "number" ? Math.floor(limitIn) : PAGE_DEFAULT, PAGE_MAX));
    let snap: Snapshot;
    let offset = 0;
    if (typeof cursor === "string" && cursor) {
      const [id, off] = cursor.split(".");
      const found = id ? this.snapshots.get(id) : undefined;
      if (!found || found.key !== key) throw new ToolError("That cursor has expired or belongs to a different listing; list again from the first page.");
      snap = found;
      offset = Number(off) || 0;
    } else {
      snap = { key, rows, at: now, header };
      const id = randomId("c", 6);
      this.snapshots.set(id, snap);
      (snap as Snapshot & { id?: string }).id = id;
    }
    const pageRows = snap.rows.slice(offset, offset + limit);
    const nextOffset = offset + pageRows.length;
    const id = (snap as Snapshot & { id?: string }).id ?? [...this.snapshots].find(([, v]) => v === snap)?.[0];
    return { rows: pageRows, total: snap.rows.length, next: nextOffset < snap.rows.length ? `${id}.${nextOffset}` : null, header: snap.header };
  }

  private renderPage(p: { rows: MeshThreadInfo[]; total: number; next: string | null; header: string }, where: string, lastKnown: Set<Address> = new Set()): string {
    if (p.total === 0) return `No threads match in ${where}.`;
    const lines = [`${p.rows.length} of ${p.total} ${where}:`, ...p.rows.map((r) => this.line(r, lastKnown.has(r.peer.address)))];
    if (p.next) lines.push(`More: pass cursor "${p.next}" for the next page — or narrow with query, state, or project first.`);
    return lines.join("\n");
  }

  private async listThreads(rt: ThreadRuntime, a: Record<string, unknown>): Promise<string> {
    const scope = typeof a.scope === "string" ? a.scope : "related";
    const query = typeof a.query === "string" && a.query.trim() ? a.query.trim() : null;
    const state = typeof a.state === "string" ? a.state : null;
    const includeInactive = a.includeInactive === true;
    const provider = typeof a.provider === "string" && a.provider.trim() ? a.provider.trim() : null;
    const key = JSON.stringify([rt.id, scope, a.target ?? null, query, state, includeInactive, provider]);
    if (scope === "related") {
      const lastKnown = new Set<Address>();
      const rows: Array<{ info: MeshThreadInfo; summary?: ThreadSummary; updatedAt: number }> = [];
      for (const address of this.relatedAddresses(rt)) {
        const local = this.localOf(address);
        if (local) {
          const s = this.core.summaryOf(local);
          rows.push({ info: this.infoOf(local), summary: s, updatedAt: Date.parse(s.updatedAt) || 0 });
        } else {
          const known = this.store.state.labels[address];
          lastKnown.add(address);
          rows.push({ info: { peer: this.peerFor(address), state: (known?.state as ThreadState) ?? "idle", cwd: "" }, updatedAt: known?.seenAt ? Date.parse(known.seenAt) : 0 });
        }
      }
      // The orchestration is small and named on purpose: inactive members are kept unless a state is asked for.
      const p = this.page(this.filterRows(rows, { query, state, includeInactive: true, provider }), key, "", a.limit, a.cursor);
      if (p.total === 0 && rows.length > 0) return "No thread in your orchestration matches. Try scope \"project\" for threads in your project, or find_thread.";
      if (p.total === 0) {
        // Nobody yet: show the caller's project instead (still one scoped, paged listing).
        const project = await this.listThreads(rt, { ...a, scope: "project", cursor: undefined });
        return `Nobody is in your orchestration yet (no parent, no workers, no one you have exchanged messages with). Threads in your project instead:\n${project}\nfind_thread finds any other thread by id or title.`;
      }
      return this.renderPage(p, "threads in your orchestration", lastKnown);
    }
    if (scope === "project") {
      const mine = this.core.summaryOf(rt).project;
      const sameProject = (s: ThreadSummary) => !!mine && !!s.project && (mine.id && s.project.id ? s.project.id === mine.id : s.project.path === mine.path);
      const rows = this.localRows((other, s) => other.id !== rt.id && sameProject(s));
      const p = this.page(this.filterRows(rows, { query, state, includeInactive, provider }), key, "", a.limit, a.cursor);
      return this.renderPage(p, `in your project (${mine?.path ?? rt.record.cwd}) on ${this.selfName()}`);
    }
    if (scope === "target") {
      const t = typeof a.target === "string" && a.target.trim() ? a.target.trim() : null;
      if (!t) throw new ToolError(`Scope "target" needs a target: the machine's name, from list_targets (this machine is "${this.selfName()}"). For threads you work with, use scope "related" or "project".`);
      if (this.isSelfTarget(t)) {
        const rows = this.localRows((other) => other.id !== rt.id);
        const p = this.page(this.filterRows(rows, { query, state, includeInactive, provider }), key, "", a.limit, a.cursor);
        return this.renderPage(p, `on ${this.selfName()}`);
      }
      const id = this.targetIdOf(t);
      if (!id) throw new ToolError(`There is no machine called ${t} in this mesh.`);
      const filter: MeshThreadFilter = {
        ...(query ? { text: query } : {}),
        ...(state ? { state: state as ThreadState } : {}),
        includeInactive,
        limit: Math.max(1, Math.min(typeof a.limit === "number" ? Math.floor(a.limit) : PAGE_DEFAULT, PAGE_MAX)),
        ...(typeof a.cursor === "string" ? { cursor: a.cursor } : {}),
      };
      const r = (await this.hubQuery(this.addressOf(rt.id), id, { tool: "listThreads", filter })) as { threads?: MeshThreadInfo[]; total?: number; cursor?: string | null };
      const threads = r.threads ?? [];
      for (const th of threads) this.remember(th.peer, th.state);
      const total = r.total ?? threads.length;
      if (total === 0) return `No threads match on ${this.targetNameOf(id)}.`;
      const lines = [`${threads.length} of ${total} on ${this.targetNameOf(id)}:`, ...threads.map((x) => this.line(x))];
      if (r.cursor) lines.push(`More: pass cursor "${r.cursor}" for the next page — or narrow with query or state first.`);
      return lines.join("\n");
    }
    throw new ToolError('scope must be "related" (your orchestration), "project" (threads in your project on this machine), or "target" (one named machine).');
  }

  /** `listThreads` for another target's caller (`mesh/query`). */
  private listForQuery(caller: Address, f: MeshThreadFilter): { threads: MeshThreadInfo[]; total: number; cursor?: string } {
    const related = (rt: ThreadRuntime) => {
      const tm = this.store.peekThread(rt.id);
      return tm?.spawnedBy === caller || (tm?.peers ?? []).includes(caller) || (tm?.children ?? []).includes(caller);
    };
    const rows = this.localRows((rt, s) => {
      if (f.mine && !related(rt)) return false;
      if (f.childrenOf && this.store.peekThread(rt.id)?.spawnedBy !== f.childrenOf) return false;
      if (f.running && s.state !== "running") return false;
      if (f.project && !(s.project?.crossTargetKey === f.project || s.project?.path === f.project)) return false;
      return true;
    });
    const filtered = this.filterRows(rows, { query: f.text ?? null, state: f.state ?? null, includeInactive: f.includeInactive ?? false });
    const p = this.page(filtered, JSON.stringify(["q", caller, f.text ?? null, f.state ?? null, f.includeInactive ?? false, f.mine ?? false, f.childrenOf ?? null, f.project ?? null]), "", f.limit ?? PAGE_DEFAULT, f.cursor ?? null);
    return { threads: p.rows, total: p.total, ...(p.next ? { cursor: p.next } : {}) };
  }

  private async findThread(rt: ThreadRuntime, a: Record<string, unknown>): Promise<string> {
    const query = typeof a.query === "string" ? a.query.trim() : "";
    if (!query) throw new ToolError("find_thread needs a query: an id, an id prefix of at least 4 characters, or words from the title.");
    const onlyTarget = typeof a.target === "string" && a.target.trim() ? this.targetIdOf(a.target.trim()) : null;
    if (typeof a.target === "string" && a.target.trim() && !onlyTarget) throw new ToolError(`There is no machine called ${a.target} in this mesh.`);
    const state = typeof a.state === "string" ? (a.state as ThreadState) : null;
    const provider = typeof a.provider === "string" && a.provider.trim() ? a.provider.trim() : null;
    if (this.hub && !this.hub.isClosed) {
      try {
        const r = (await this.hubQuery(this.addressOf(rt.id), this.selfId(), {
          tool: "findThreads",
          query,
          ...(onlyTarget ? { onlyTarget } : {}),
          ...(state ? { state } : {}),
          ...(typeof a.cursor === "string" ? { cursor: a.cursor } : {}),
        })) as { results?: Array<{ thread: MeshThreadInfo; matched: string; exact: boolean }>; total?: number; ambiguous?: boolean; cursor?: string | null };
        const self = this.addressOf(rt.id);
        const results = (r.results ?? []).filter((x) => this.ofProvider(x.thread, provider) && this.normalise(x.thread.peer.address) !== self);
        for (const x of results) this.remember(x.thread.peer, x.thread.state);
        const total = Math.max(results.length, (r.total ?? 0) - ((r.results ?? []).length - results.length));
        if (total === 0) return `No thread matches "${query}" on any machine.`;
        const lines = [`${results.length} of ${total} match${total === 1 ? "" : "es"} for "${query}" across all machines:`];
        if (r.ambiguous) lines.push("This id prefix matches several threads — ask the user which one is meant; do not guess.");
        for (const x of results) lines.push(`${this.line(x.thread)} (matched by ${x.matched}${x.exact ? ", exact" : ""})`);
        if (r.cursor) lines.push(`More: pass cursor "${r.cursor}" for the next ten.`);
        return lines.join("\n");
      } catch (e) {
        if (!(e instanceof RpcError) || !["not_initialized", "connection_closed", "timeout", "target_offline"].includes(e.code)) throw e;
      }
    }
    // The hub is unavailable: this target only, and say so.
    if (onlyTarget && !this.isSelfTarget(onlyTarget)) throw new ToolError(`The YAAC hub is not reachable, so ${a.target} cannot be searched; only this machine can.`);
    const found = this.findLocal(query, state);
    found.rows = found.rows.filter((r) => this.ofProvider(r, provider) && r.peer.address !== this.addressOf(rt.id));
    const key = JSON.stringify(["find", query, state, provider]);
    const p = this.page(found.rows, key, "", FIND_PAGE, a.cursor);
    const note = "(Only this machine was searched: the YAAC hub is not reachable.)";
    if (p.total === 0) return `No thread on ${this.selfName()} matches "${query}". ${note}`;
    const lines = [`${p.rows.length} of ${p.total} match${p.total === 1 ? "" : "es"} for "${query}" on ${this.selfName()} ${note}`];
    if (found.ambiguous) lines.push("This id prefix matches several threads — ask the user which one is meant; do not guess.");
    lines.push(...p.rows.map((r) => this.line(r)));
    if (p.next) lines.push(`More: pass cursor "${p.next}" for the next ten.`);
    return lines.join("\n");
  }

  /** The hub's matching, over this target's threads only. */
  findLocal(query: string, state: ThreadState | null): { rows: MeshThreadInfo[]; ambiguous: boolean } {
    const all = this.core.allThreads().filter((t) => !state || this.core.summaryOf(t).state === state);
    if (looksLikeId(query)) {
      const q = query.trim();
      const exact = all.filter((t) => t.id === q || t.record.native === q || this.addressOf(t.id) === this.normalise(q));
      if (exact.length > 0) return { rows: exact.map((t) => this.infoOf(t)), ambiguous: false };
      const thread = this.split(q)?.thread ?? q;
      const prefix = all.filter((t) => t.id.startsWith(thread) || t.record.native.startsWith(q) || this.addressOf(t.id).startsWith(q));
      if (prefix.length > 0) return { rows: prefix.map((t) => this.infoOf(t)), ambiguous: prefix.length > 1 };
    }
    const scored = all
      .map((t) => ({ t, s: titleScore(query, this.core.summaryOf(t).title) }))
      .filter((x) => x.s > 0)
      .sort((x, y) => y.s - x.s || (Date.parse(this.core.summaryOf(y.t).updatedAt) || 0) - (Date.parse(this.core.summaryOf(x.t).updatedAt) || 0));
    return { rows: scored.map((x) => this.infoOf(x.t)), ambiguous: false };
  }

  // ── Reading ────────────────────────────────────────────────────────────────────────

  async transcript(rt: ThreadRuntime, lastAnswerOnly: boolean, maxBytes: number): Promise<{ entries: TranscriptEntry[]; truncated: boolean }> {
    const projection = await this.core.ensureProjection(rt);
    const entries: TranscriptEntry[] = [];
    for (const item of projection.items()) {
      const i = item as Item & Record<string, unknown>;
      if (i.kind === "userMessage") {
        const text = ((i.parts as Array<{ type: string; text?: string }>) ?? []).filter((p) => p.type === "text").map((p) => p.text ?? "").join("\n");
        if (text) entries.push({ role: "user", text });
      } else if (i.kind === "agentMessage" && typeof i.text === "string" && i.text) {
        entries.push({ role: "agent", text: i.text });
      } else if (i.kind === "meshReceived") {
        const m = i.message as MeshMessage;
        entries.push({ role: m.kind === "notice" ? "notice" : "peer", from: m.from, text: m.text });
      } else if (i.kind === "meshSent") {
        const m = i.message as MeshMessage;
        entries.push({ role: "agent", text: `(sent to ${this.labelOf(m.to)}) ${m.text}` });
      }
    }
    if (lastAnswerOnly) {
      const last = [...entries].reverse().find((e) => e.role === "agent" && !e.text.startsWith("(sent to"));
      return { entries: last ? [{ role: "agent", text: clip(last.text, maxBytes) }] : [], truncated: false };
    }
    const out: TranscriptEntry[] = [];
    let bytes = 0;
    for (let k = entries.length - 1; k >= 0; k--) {
      const e = entries[k]!;
      const size = Buffer.byteLength(e.text) + 40;
      if (out.length > 0 && bytes + size > maxBytes) return { entries: out.reverse(), truncated: true };
      out.push(out.length === 0 ? { ...e, text: clip(e.text, maxBytes - 40) } : e);
      bytes += size;
    }
    return { entries: out.reverse(), truncated: false };
  }

  private renderTranscript(label: string, state: string, t: { entries: TranscriptEntry[]; truncated: boolean }): string {
    if (t.entries.length === 0) return `${label} (${state}) has said nothing yet.`;
    const lines = [`${label} — ${state}${t.truncated ? " (earlier messages left out)" : ""}:`];
    for (const e of t.entries) {
      const who = e.role === "peer" ? `message from ${e.from ? this.labelOf(e.from) : "an agent"}` : e.role;
      lines.push(`[${who}] ${e.text}`);
    }
    return lines.join("\n");
  }

  private async readThread(rt: ThreadRuntime, a: Record<string, unknown>): Promise<string> {
    const r = this.resolve(a.thread);
    const lastAnswerOnly = a.lastAnswerOnly === true;
    const maxBytes = Math.max(500, Math.min(typeof a.maxBytes === "number" ? a.maxBytes : 8000, 32000));
    const caller = this.addressOf(rt.id);
    if (r.kind === "local") {
      const t = await this.transcript(r.rt, lastAnswerOnly, maxBytes);
      return this.renderTranscript(this.labelOf(r.address), this.core.summaryOf(r.rt).state, t);
    }
    const reply = (await this.hubQuery(caller, r.target, { tool: "readThread", thread: r.address, lastAnswerOnly, maxBytes })) as { entries?: TranscriptEntry[]; truncated?: boolean };
    return this.renderTranscript(this.labelOf(r.address), "on another machine", { entries: reply.entries ?? [], truncated: !!reply.truncated });
  }

  // ── Roots, permission, budgets ─────────────────────────────────────────────────────

  /** The orchestration root of what a thread is doing now. */
  rootOf(rt: ThreadRuntime): Address {
    const tm = this.store.peekThread(rt.id);
    return this.normalise(tm?.currentRoot ?? tm?.spawnRoot ?? this.addressOf(rt.id));
  }

  private rootState(root: Address): RootState {
    const key = this.normalise(root);
    let rs = this.store.state.roots[key];
    if (!rs) {
      rs = { allowance: { ...this.store.state.policy.budget }, agentTurns: 0, threadsSpawned: 0, remote: {}, lastHumanAt: Date.now(), granted: false };
      this.store.state.roots[key] = rs;
    }
    return rs;
  }

  private usedOf(rs: RootState): MeshBudgetUsage {
    let turns = rs.agentTurns;
    let threads = rs.threadsSpawned;
    for (const r of Object.values(rs.remote)) {
      turns += r.agentTurns;
      threads += r.threadsSpawned;
    }
    return { agentTurns: turns, threadsSpawned: threads, wallClockSecs: Math.floor((Date.now() - rs.lastHumanAt) / 1000) };
  }

  /** Why a root's budget is used up, or null. */
  private exhaustedReason(root: Address | null | undefined): string | null {
    if (!root) return null;
    const key = this.normalise(root);
    if (this.isLocalAddress(key)) {
      const rs = this.store.state.roots[key];
      if (!rs) return null;
      const used = this.usedOf(rs);
      if (used.agentTurns >= rs.allowance.agentTurns) return `its ${rs.allowance.agentTurns} agent-started turns are used`;
      if (rs.allowance.wallClockSecs < NO_TIME_LIMIT && used.wallClockSecs >= rs.allowance.wallClockSecs) {
        const hours = Math.round(rs.allowance.wallClockSecs / 3600);
        return `more than ${hours} ${hours === 1 ? "hour has" : "hours have"} passed since a person last acted on it (its time limit)`;
      }
      return null;
    }
    return this.store.state.remoteRoots[key]?.paused ? "its owner's budget has run out" : null;
  }

  /** Shows the budget notice in a local root, once per exhaustion. */
  private budgetNotice(root: Address, reason: string): void {
    const rt = this.localOf(root);
    if (!rt) return;
    const rs = this.rootState(root);
    if (rs.exhaustedNoticed) return;
    rs.exhaustedNoticed = true;
    this.store.save();
    this.budgetChanged(root);
    this.noticeItem(rt, "budgetExhausted", `The agents working for this chat have used their budget (${reason}). Nothing was cancelled: turns in progress finish, and messages between the agents are kept but not delivered until you choose Continue.`);
  }

  /** Messages of a root stored but held back because its budget is used up (D164). */
  private heldFor(root: Address): number {
    const key = this.normalise(root);
    let n = 0;
    for (const r of this.store.all()) {
      if (r.inbox && r.inState === "pending" && r.receipt?.reason === "budgetExhausted" && r.message.root && this.normalise(r.message.root) === key) n++;
    }
    return n;
  }

  /** `mesh/budget`'s status of a local root. */
  private budgetStatus(root: Address): MeshBudgetResult {
    const key = this.normalise(root);
    const rs = this.rootState(key);
    const exhausted = !!this.exhaustedReason(key);
    return { root: key, allowance: rs.allowance, used: this.usedOf(rs), exhausted, held: exhausted ? this.heldFor(key) : 0, actedAt: new Date(rs.lastHumanAt).toISOString() };
  }

  /**
   * Tells every window when a local root's budget runs out, is freed, or starts or stops
   * holding messages back (`mesh/budget/changed`, D164). Freed: its notice can show again on
   * the next exhaustion, and what it held back is dispatched.
   */
  private budgetChanged(root: Address | null | undefined): void {
    if (!root) return;
    const key = this.normalise(root);
    if (!this.isLocalAddress(key) || !this.store.state.roots[key] || !this.localOf(key)) return;
    const status = this.budgetStatus(key);
    // What a window shows of it (P352): both limits, what is used, and the hour since a person
    // acted — so a person acting, an agent turn or a new time limit reaches every window.
    const sig = `${status.exhausted}:${status.held}:${status.used.agentTurns}/${status.allowance.agentTurns}:${Math.floor(status.used.wallClockSecs / 3600)}/${status.allowance.wallClockSecs}`;
    const before = this.budgetReported.get(key);
    if (before === sig) return;
    this.budgetReported.set(key, sig);
    const rs = this.rootState(key);
    if (!status.exhausted && rs.exhaustedNoticed) {
      rs.exhaustedNoticed = false;
      this.store.save();
    }
    // Every window that asked may hold an older answer: always said once, then on change.
    this.core.broadcast("mesh/budget/changed", status);
    if (before?.startsWith("true") && !status.exhausted) for (const id of this.threadsWithMail()) this.kick(id);
  }

  /**
   * A person gave a thread input (a message started, queued, sent now or steered, or an
   * answer to one of its requests, D164): its root's wall clock starts again.
   */
  humanInput(rt: ThreadRuntime): void {
    const root = this.rootOf(rt);
    const rs = this.isLocalAddress(root) ? this.store.state.roots[root] : undefined;
    if (!rs) return;
    rs.lastHumanAt = Date.now();
    this.store.save();
    this.budgetChanged(root);
  }

  private countTurn(root: Address): void {
    const key = this.normalise(root);
    if (this.isLocalAddress(key)) {
      const rs = this.rootState(key);
      rs.agentTurns++;
      const why = this.exhaustedReason(key);
      if (why) this.budgetNotice(key, why);
      this.budgetChanged(key);
    } else {
      const r = (this.store.state.remoteRoots[key] ??= { agentTurns: 0, threadsSpawned: 0, paused: false });
      r.agentTurns++;
      void this.reportUsage(key);
    }
    this.store.save();
  }

  private countSpawn(root: Address): void {
    const key = this.normalise(root);
    if (this.isLocalAddress(key)) {
      this.rootState(key).threadsSpawned++;
      this.budgetChanged(key);
    } else {
      const r = (this.store.state.remoteRoots[key] ??= { agentTurns: 0, threadsSpawned: 0, paused: false });
      r.threadsSpawned++;
      void this.reportUsage(key);
    }
    this.store.save();
  }

  private async reportUsage(root: Address): Promise<void> {
    const r = this.store.state.remoteRoots[root];
    const hub = this.hub;
    if (!r || !hub || hub.isClosed || !this.store.state.self) return;
    r.reportedAt = Date.now();
    try {
      const res = (await hub.requestHost("mesh/usage", { root, source: this.selfId(), agentTurns: r.agentTurns, threadsSpawned: r.threadsSpawned }, 15000)) as { exhausted?: boolean };
      const was = r.paused;
      r.paused = !!res.exhausted;
      this.store.save();
      if (was && !r.paused) for (const id of this.threadsWithMail()) this.kick(id);
    } catch {
      /* reported again later: the counts are cumulative */
    }
  }

  /**
   * The composer toggle (D51, N31): the first spawn or outgoing message of an orchestration in
   * an `ask` chat raises one prompt. Work under another thread's root is covered by that
   * root's answer.
   */
  /**
   * The per-chat toggle (D51) is the main control; the target's policy mode can only make it
   * stricter: `askFirst` asks once per orchestration even in a chat set to allow; `alwaysAsk`
   * asks for every action, in the thread that wants to act.
   */
  private async ensurePermission(rt: ThreadRuntime, action: MeshPermissionAction): Promise<void> {
    const root = this.rootOf(rt);
    const self = this.addressOf(rt.id);
    const mode = this.store.state.policy.mode;
    if (mode === "alwaysAsk") {
      const rs = this.isLocalAddress(root) ? this.rootState(root) : null;
      return this.promptFor(rt, action, root, rs, true);
    }
    if (root !== self) {
      if (this.isLocalAddress(root) && this.store.state.roots[root]?.denied) {
        throw new ToolError("The user did not allow the agents of this orchestration to work with other agents. Carry on alone.");
      }
      return;
    }
    const rs = this.rootState(root);
    if (rs.granted) return;
    if ((rt.record.agents ?? "ask") === "allow" && mode !== "askFirst") {
      rs.granted = true;
      this.store.save();
      return;
    }
    if (rs.denied) {
      throw new ToolError("The user did not allow this chat's agent to start or message other agents. Carry on alone, or ask the user to switch the chat's Agents setting to allow.");
    }
    return this.promptFor(rt, action, root, rs, false);
  }

  /** Raises (or reuses) a permission prompt in `rt` and waits for the user's answer. */
  private async promptFor(rt: ThreadRuntime, action: MeshPermissionAction, root: Address, rs: RootState | null, each: boolean): Promise<void> {
    let requestId = (each ? this.eachPrompt.get(rt.id) : rs?.pendingRequest) ?? null;
    if (!requestId || rt.requests.get(requestId)?.state !== "pending") {
      const budget = rs ? { ...rs.allowance } : { ...this.store.state.policy.budget };
      requestId = this.core.raiseMeshRequest(rt.id, { requestKind: "meshPermission", actions: [action], budget, root });
      if (each) this.eachPrompt.set(rt.id, requestId);
      else if (rs) rs.pendingRequest = requestId;
      this.permissionRoots.set(requestId, root);
      this.store.save();
    }
    const answer = await new Promise<{ decision: string } | null>((resolve) => {
      const list = this.permissionWaiters.get(requestId!) ?? [];
      const timer = setTimeout(() => resolve(null), this.permissionWaitMs);
      timer.unref();
      list.push((a) => {
        clearTimeout(timer);
        resolve(a);
      });
      this.permissionWaiters.set(requestId!, list);
    });
    if (!answer) {
      throw new ToolError("The user has not yet answered the prompt asking whether this chat's agent may work with other agents. The prompt stays open in the chat; try again after the user has answered.");
    }
    if (answer.decision !== "allow") {
      throw new ToolError("The user did not allow this chat's agent to start or message other agents. Carry on alone.");
    }
  }

  /** `request/respond` for a `meshPermission` (from `Core.requestRespond`). */
  async respond(rt: ThreadRuntime, rec: RequestRecord, response: RequestResponse, client: ClientLink): Promise<void> {
    if (this.eachPrompt.get(rt.id) === rec.requestId) this.eachPrompt.delete(rt.id);
    const r = response as RequestResponse & { decision?: string; budget?: MeshBudget | null; alwaysAllow?: boolean };
    const root = this.permissionRoots.get(rec.requestId) ?? this.addressOf(rt.id);
    const rs = this.rootState(root);
    rs.pendingRequest = null;
    rs.lastHumanAt = Date.now();
    if (r.decision === "allow") {
      rs.granted = true;
      rs.denied = false;
      if (r.budget && typeof r.budget.agentTurns === "number") {
        rs.allowance = { agentTurns: Math.max(0, r.budget.agentTurns), threadsSpawned: Math.max(0, r.budget.threadsSpawned), wallClockSecs: Math.max(0, r.budget.wallClockSecs) };
      }
      if (r.alwaysAllow) {
        rt.record.agents = "allow";
        rt.saveRecord();
      }
    } else {
      rs.denied = true;
    }
    this.store.save();
    this.core.resolveOwnRequest(rt, rec, response, client);
    const waiters = this.permissionWaiters.get(rec.requestId) ?? [];
    this.permissionWaiters.delete(rec.requestId);
    if (waiters.length > 0) for (const w of waiters) w({ decision: r.decision ?? "deny" });
    else {
      // The tool call gave up waiting: tell the agent at its next turn or wait (passive).
      this.notice(this.addressOf(rt.id), this.addressOf(rt.id), r.decision === "allow" ? "The user allowed this chat's agent to work with other agents. You can now try again what you asked for." : "The user did not allow this chat's agent to work with other agents.", root);
    }
    this.core.updateIndex(rt);
    this.budgetChanged(root);
    for (const id of this.threadsWithMail()) this.kick(id);
  }

  // ── Spawning ───────────────────────────────────────────────────────────────────────

  private resolveInstance(descriptors: ProviderDescriptor[], wanted: string | null, fallback: string | null): ProviderDescriptor {
    const ready = descriptors.filter((d) => d.status === "ready");
    if (!wanted) {
      const d = (fallback ? ready.find((x) => x.instance === fallback) : undefined) ?? (ready.length === 1 ? ready[0] : undefined);
      if (!d) throw new ToolError(`Say which provider instance to use: ${ready.map((x) => `${x.instance} ("${x.displayName}")`).join(", ") || "none is ready there"}.`);
      return d;
    }
    const w = wanted.toLowerCase();
    const exact = descriptors.find((d) => d.instance === wanted);
    const matches = exact ? [exact] : descriptors.filter((d) => d.displayName.toLowerCase() === w || d.kind.toLowerCase() === w || d.displayName.toLowerCase().includes(w));
    if (matches.length === 0) throw new ToolError(`No provider instance "${wanted}" there. Available: ${descriptors.map((x) => `${x.instance} ("${x.displayName}", ${x.status})`).join(", ") || "none"}.`);
    if (matches.length > 1) throw new ToolError(`"${wanted}" matches several provider instances: ${matches.map((x) => `${x.instance} ("${x.displayName}")`).join(", ")}. Pass the instance id.`);
    const d = matches[0]!;
    if (d.status !== "ready") throw new ToolError(`${d.displayName} cannot be used there now (${d.status}${d.statusMessage ? `: ${d.statusMessage}` : ""}).`);
    return d;
  }

  /** A model as the agent named it, against the live list: id, display name, or a unique part of either. */
  private resolveModel(d: ProviderDescriptor, wanted: string | null): string | null {
    if (!wanted) return null;
    const models = d.models ?? [];
    const instance = this.core.registry.require(d.instance);
    const refuseOff = (id: string): never => {
      throw new ToolError(`${id} is turned off for ${d.displayName} on ${this.selfName()}`);
    };
    if (this.core.registry.modelIsOff(instance, wanted)) refuseOff(wanted);
    const exact = models.find((m) => m.id === wanted);
    if (exact) return exact.id;
    const w = wanted.toLowerCase();
    const byName = models.filter((m) => m.displayName.toLowerCase() === w || m.id.toLowerCase() === w);
    const offByName = d.disabledModels?.find((m) => m.id.toLowerCase() === w || m.displayName.toLowerCase() === w);
    if (!byName.length && offByName) refuseOff(offByName.id);
    const partial = byName.length > 0 ? byName : models.filter((m) => m.displayName.toLowerCase().includes(w) || m.id.toLowerCase().includes(w));
    if (partial.length === 1) return partial[0]!.id;
    if (partial.length > 1) throw new ToolError(`"${wanted}" matches several models of ${d.displayName}: ${partial.map((m) => `${m.id} ("${m.displayName}")`).join(", ")}. Pass the model id.`);
    const off = d.disabledModels?.find((m) => m.id.toLowerCase().includes(w) || m.displayName.toLowerCase().includes(w));
    if (off) refuseOff(off.id);
    if (!d.modelsComplete) return wanted; // the list may be incomplete: let the harness decide
    throw new ToolError(`${d.displayName} does not offer "${wanted}" now. Its models: ${models.map((m) => `${m.id} ("${m.displayName}")`).join(", ") || "none listed"}.`);
  }

  /** Every checkout of a repository (by its normalised remote) known on this target. */
  private async checkoutsOf(key: string): Promise<string[]> {
    const found = new Set<string>();
    for (const p of this.core.projects.list()) if (p.crossTargetKey === key && fs.existsSync(p.folder)) found.add(p.repo?.root ?? p.folder);
    const roots = new Set<string>();
    for (const rt of this.core.allThreads()) if (rt.record.repo?.root) roots.add(rt.record.repo.root);
    let n = 0;
    for (const root of roots) {
      if (found.has(root) || n++ > 64 || !fs.existsSync(root)) continue;
      const r = await this.git(root, ["config", "--get", "remote.origin.url"]);
      if (r.ok && r.out.trim() && normaliseRemote(r.out.trim()) === key) found.add(root);
    }
    return [...found];
  }

  private async projectKeyOf(rt: ThreadRuntime): Promise<string | null> {
    const s = this.core.summaryOf(rt);
    if (s.project?.crossTargetKey) return s.project.crossTargetKey;
    const r = await this.git(rt.record.cwd, ["config", "--get", "remote.origin.url"]);
    return r.ok && r.out.trim() ? normaliseRemote(r.out.trim()) : null;
  }

  private spawnItem(parent: ThreadRuntime, turnId: TurnId | null, thread: SpawnedThread): void {
    const tm = this.store.thread(parent.id);
    const key = turnId ?? "";
    const items = (tm.spawnItems ??= {});
    let entry = items[key];
    const fresh = !entry;
    if (!entry) {
      entry = { itemId: `spawn:${key || randomId("", 6)}`, threads: [] };
      items[key] = entry;
    }
    const i = entry.threads.findIndex((t) => t.peer.address === thread.peer.address);
    if (i >= 0) entry.threads[i] = thread;
    else entry.threads.push(thread);
    const item = { id: entry.itemId, turnId: key, status: "completed", kind: "meshSpawn", threads: entry.threads } as Item;
    if (fresh) {
      this.core.injectItem(parent.id, "itemStarted", item);
      this.core.injectItem(parent.id, "itemCompleted", item);
    } else this.core.injectItem(parent.id, "itemUpdated", item);
  }

  /** A child's state changed: update its parent's live spawn item. */
  private updateSpawnItems(childAddress: Address, state: ThreadState, peer?: MeshPeer, keepStopped = false): void {
    const tmOfChild = this.localOf(childAddress) ? this.store.peekThread(this.localOf(childAddress)!.id) : undefined;
    const parentAddress = tmOfChild?.spawnedBy;
    const parents = parentAddress ? [this.localOf(parentAddress)].filter((x): x is ThreadRuntime => !!x) : this.core.allThreads().filter((t) => (this.store.peekThread(t.id)?.children ?? []).includes(childAddress));
    for (const parent of parents) {
      const items = this.store.peekThread(parent.id)?.spawnItems ?? {};
      for (const [turn, entry] of Object.entries(items)) {
        const t = entry.threads.find((x) => this.normalise(x.peer.address) === this.normalise(childAddress));
        if (!t || (t.state === state && !peer)) continue;
        // A stopped worker's interrupted turn still returns a result: it stays shown as stopped.
        if (keepStopped && t.state === "stopped") continue;
        this.spawnItem(parent, turn || null, { ...t, state, ...(peer ? { peer } : {}) });
      }
    }
  }

  private async spawnTool(rt: ThreadRuntime, a: Record<string, unknown>): Promise<string> {
    const text = typeof a.text === "string" ? a.text : "";
    if (!text.trim()) throw new ToolError("spawn_thread needs text: the first message, with the worker's task.");
    const limits = this.store.state.policy.limits;
    const tm = this.store.thread(rt.id);
    const caller = this.addressOf(rt.id);
    if (tm.depth + 1 > limits.spawnDepth) {
      this.limitNotice(rt, `spawning is limited to a depth of ${limits.spawnDepth}`);
      throw new ToolError(`Limit reached: workers can be nested at most ${limits.spawnDepth} deep, and this thread is already at depth ${tm.depth}. Do the work yourself or report back.`);
    }
    const targetIn = typeof a.target === "string" && a.target.trim() ? a.target.trim() : null;
    const targetId = targetIn ? this.targetIdOf(targetIn) : this.selfId();
    if (targetIn && !targetId) {
      if (this.store.state.roster.length === 0 || !this.hub || this.hub.isClosed) {
        throw new ToolError(`${targetIn} cannot be reached: the YAAC hub is not available, and a spawn on another machine cannot be queued. Threads on this machine can still be spawned.`);
      }
      throw new ToolError(`There is no machine called ${targetIn} in this mesh. list_targets shows the machines.`);
    }
    const local = !targetId || this.isSelfTarget(targetId);
    const hub = this.hub;
    if (!local && (!hub || hub.isClosed)) {
      throw new ToolError(`${this.targetNameOf(targetId!)} cannot be reached: the YAAC hub is not available, and a spawn on another machine cannot be queued. Threads on this machine can still be spawned.`);
    }
    const root = this.rootOf(rt);
    const instanceIn = typeof a.instance === "string" && a.instance.trim() ? a.instance.trim() : null;
    const modelIn = typeof a.model === "string" && a.model.trim() ? a.model.trim() : null;
    if (local && modelIn) {
      this.resolveModel(this.resolveInstance(this.localDescriptors(), instanceIn, rt.record.instance), modelIn);
    }
    const title = (typeof a.title === "string" && a.title.trim() ? a.title.trim() : text.split("\n").find((l) => l.trim() && !l.trim().startsWith("/fake"))?.trim() ?? "Worker").slice(0, 80);
    const modeIn = typeof a.mode === "string" && a.mode.trim() ? a.mode.trim() : null;
    const parentMode = await this.parentModeOf(rt);
    await this.ensurePermission(rt, {
      action: "startThreads",
      summary: `Start a thread on ${local ? this.selfName() : this.targetNameOf(targetId!)}${instanceIn ? ` (${instanceIn}${modelIn ? `, ${modelIn}` : ""})` : ""}: "${title}"`,
      threads: [{ count: 1, ...(instanceIn ? { instance: instanceIn } : {}), ...(modelIn ? { model: modelIn } : {}), ...(local ? {} : { target: targetId!, targetName: this.targetNameOf(targetId!) }) }],
    });
    const why = this.exhaustedReason(root);
    if (why) {
      this.budgetNotice(root, why);
      throw new ToolError(`This orchestration's budget is used up (${why}). Nothing was cancelled; the user can choose Continue to grant more.`);
    }
    if (local) {
      const reply = await this.spawnLocal({
        caller,
        callerRt: rt,
        instance: instanceIn,
        model: modelIn,
        effort: typeof a.effort === "string" ? a.effort : null,
        cwd: typeof a.cwd === "string" && a.cwd.trim() ? a.cwd.trim() : rt.record.cwd,
        projectKey: null,
        title,
        text,
        detached: a.detached === true,
        root,
        depth: tm.depth + 1,
        ancestors: [caller, ...this.ancestorsOf(rt)],
        mode: modeIn,
        parentMode,
      });
      return `Started ${this.labelOf(reply.thread.address)}.\nAddress: ${reply.thread.address}\n${reply.modeNote}\n${a.detached === true ? "It is detached: its answer will not come back to you." : "Its final answer to this task will come back to you once, as a result: call wait to receive it."}`;
    }
    const projectKey = typeof a.cwd === "string" && a.cwd.trim() ? null : await this.projectKeyOf(rt);
    const r = (await this.hubQuery(caller, targetId!, {
      tool: "spawnThread",
      instance: instanceIn ?? "",
      ...(modelIn ? { model: modelIn } : {}),
      ...(typeof a.effort === "string" ? { effort: a.effort } : {}),
      ...(typeof a.cwd === "string" && a.cwd.trim() ? { cwd: a.cwd.trim() } : {}),
      ...(projectKey ? { projectKey } : {}),
      title,
      text,
      detached: a.detached === true,
      root,
      depth: tm.depth + 1,
      ancestors: [caller, ...this.ancestorsOf(rt)],
      ...(modeIn ? { mode: modeIn } : {}),
      ...(parentMode.info ? { parentMode: parentMode.info } : {}),
    })) as { thread: MeshPeer; turnId: TurnId; modeNote?: string };
    this.remember(r.thread, "running");
    tm.children.push(r.thread.address);
    this.spawnItem(rt, rt.activeTurn, { peer: r.thread, state: "running" });
    // The first message is part of what agents said: record it, consumed by the child's first turn.
    this.store.save();
    const modeNote = r.modeNote ?? "Its mode was not reported (the yaacd there is older): it runs in its harness's default mode there.";
    return `Started ${this.labelOf(r.thread.address)}.\nAddress: ${r.thread.address}\n${modeNote}\n${a.detached === true ? "It is detached: its answer will not come back to you." : "Its final answer to this task will come back to you once, as a result: call wait to receive it."}`;
  }

  /** Starts a worker on this target (for a local caller, or one on another target via `mesh/query`). */
  private async spawnLocal(p: {
    caller: Address;
    callerRt: ThreadRuntime | null;
    instance: string | null;
    model: string | null;
    effort: string | null;
    cwd: string | null;
    projectKey: string | null;
    title: string;
    text: string;
    detached: boolean;
    root: Address;
    depth: number;
    /** Every thread above the new one, nearest first. */
    ancestors?: Address[];
    /** `spawn_thread`'s `mode`. */
    mode?: string | null;
    /** The caller's effective mode (D146). */
    parentMode: ParentMode;
  }): Promise<{ thread: MeshPeer; turnId: TurnId; modeNote: string }> {
    const limits = this.store.state.policy.limits;
    if (p.depth > limits.spawnDepth) throw new RpcError("limit_exceeded", `Limit reached: workers can be nested at most ${limits.spawnDepth} deep.`);
    const why = this.exhaustedReason(p.root);
    if (why) throw new RpcError("limit_exceeded", `The orchestration's budget is used up (${why}).`);
    const d = this.resolveInstance(this.localDescriptors(), p.instance, p.callerRt?.record.instance ?? null);
    const model = this.resolveModel(d, p.model);
    let cwd = p.cwd;
    if (!cwd) {
      const found = p.projectKey ? await this.checkoutsOf(p.projectKey) : [];
      if (found.length === 1) cwd = found[0]!;
      else if (found.length === 0) throw new RpcError("invalid_params", `No checkout of the caller's repository is known on ${this.selfName()}: name a working directory there (cwd).`);
      else throw new RpcError("invalid_params", `Several checkouts of the caller's repository are on ${this.selfName()} (${found.join(", ")}): name one as cwd.`);
    }
    cwd = path.resolve(cwd);
    let isDir = false;
    try {
      isDir = fs.statSync(cwd).isDirectory();
    } catch {
      isDir = false;
    }
    if (!isDir) throw new RpcError("invalid_params", `The working directory ${cwd} does not exist on ${this.selfName()}.`);
    if (p.effort) {
      const m = (d.models ?? []).find((x) => x.id === (model ?? (d.models ?? []).find((y) => y.isDefault)?.id));
      if (m && (m.efforts ?? []).length > 0 && !(m.efforts ?? []).some((e) => e.id === p.effort)) {
        throw new RpcError("invalid_params", `${m.displayName} has no effort "${p.effort}". Its efforts: ${(m.efforts ?? []).map((e) => e.id).join(", ")}.`);
      }
    }
    let choice;
    try {
      choice = chooseWorkerMode(p.parentMode, { instance: d.instance, displayName: d.displayName, modes: d.modes ?? [] }, p.mode ?? null);
    } catch (e) {
      if (e instanceof ModeRefused) throw new ToolError(e.message);
      throw e;
    }
    const message: MeshMessage = { id: randomId("m_", 12), from: this.normalise(p.caller), to: "", kind: "message", text: p.text, sentAt: nowIso(), seq: 1, root: this.normalise(p.root) };
    const input: MeshInputMessage = { id: message.id, kind: "message", from: message.from, fromLabel: this.labelOf(message.from), text: p.text, task: true };
    const received = { id: `recv:${message.id}`, turnId: "", status: "completed", kind: "meshReceived", from: this.peerFor(message.from), message } as unknown as Item;
    // Which mode it runs in, and why, is in its own transcript too (D146).
    const modeNotice = { id: `notice:mode:${message.id}`, turnId: "", status: "completed", kind: "notice", noticeKind: "info", text: `This worker ${choice.note.charAt(0).toLowerCase()}${choice.note.slice(1)}` } as unknown as Item;
    const started = await this.core.startThreadForAgent({
      instance: d.instance,
      cwd,
      model,
      effort: p.effort,
      mode: choice.mode,
      title: p.title,
      parts: [{ type: "text", text: frameMessages([input]) }],
      journalItems: [received, modeNotice],
      mesh: [input],
      spawnedBy: message.from,
      parent: p.callerRt,
    });
    const child = this.core.thread(started.threadId)!;
    const childAddress = this.addressOf(child.id);
    message.to = childAddress;
    this.store.put({ message, inbox: true, outbox: !!p.callerRt, inState: "consumed", outState: "local", turnId: started.turnId, recipient: child.id, ...(p.callerRt ? { sender: p.callerRt.id } : {}), storedAt: Date.now(), receipt: { status: "delivered" } });
    const ctm = this.store.thread(child.id);
    ctm.spawnedBy = message.from;
    ctm.spawnRoot = message.root ?? message.from;
    ctm.currentRoot = ctm.spawnRoot;
    ctm.depth = p.depth;
    ctm.ancestors = (p.ancestors && p.ancestors.length > 0 ? p.ancestors : [message.from]).map((a) => this.normalise(a));
    ctm.spawnTurn = started.turnId;
    ctm.detached = p.detached;
    ctm.introduced = true;
    this.countSpawn(p.root);
    this.countTurn(p.root);
    if (p.callerRt) {
      const ptm = this.store.thread(p.callerRt.id);
      ptm.children.push(childAddress);
      this.spawnItem(p.callerRt, p.callerRt.activeTurn, { peer: this.peerOf(child), state: this.core.summaryOf(child).state });
    }
    this.store.save();
    this.core.updateIndex(child);
    if (p.callerRt) this.core.updateIndex(p.callerRt);
    return { thread: this.peerOf(child), turnId: started.turnId, modeNote: choice.note };
  }

  /** A thread's effective mode as its harness reports it, with its provider's description (D146). */
  private async parentModeOf(rt: ThreadRuntime): Promise<ParentMode> {
    const eff = await this.core.computeEffective(rt);
    const id = eff.applied?.mode ?? null;
    const descriptor = this.core.registry.get(rt.record.instance)?.descriptor;
    const modes: ModeInfo[] = [...(eff.modes ?? []), ...(descriptor?.modes ?? [])];
    return { instance: rt.record.instance, id, info: id ? (modes.find((m) => m.id === id) ?? null) : null };
  }

  // ── Messages ───────────────────────────────────────────────────────────────────────

  private pairKey(a: Address, b: Address): string {
    const [x, y] = [this.normalise(a), this.normalise(b)].sort();
    return `${x}\u0000${y}`;
  }

  private checkPairRate(rt: ThreadRuntime, to: Address): void {
    const { pairMessages, pairWindowSecs } = this.store.state.policy.limits;
    const key = this.pairKey(this.addressOf(rt.id), to);
    const now = Date.now();
    const log = (this.pairLog.get(key) ?? []).filter((t) => now - t < pairWindowSecs * 1000);
    if (log.length >= pairMessages) {
      this.pairLog.set(key, log);
      this.limitNotice(rt, `at most ${pairMessages} messages between the same two threads per ${Math.round(pairWindowSecs / 60)} minutes`);
      throw new ToolError(`Limit reached: at most ${pairMessages} messages may pass between the same two threads in ${Math.round(pairWindowSecs / 60)} minutes. This looks like a loop; stop and report to the user instead.`);
    }
    log.push(now);
    this.pairLog.set(key, log);
  }

  /** Whether `to` has messaged `rt` (then answering it needs no permission). */
  private isReply(rt: ThreadRuntime, to: Address): boolean {
    const t = this.normalise(to);
    for (const r of this.store.all()) if (r.inbox && r.recipient === rt.id && r.message.kind !== "notice" && this.normalise(r.message.from) === t) return true;
    const tm = this.store.peekThread(rt.id);
    return !!tm?.spawnedBy && this.normalise(tm.spawnedBy) === t;
  }

  private async sendTool(rt: ThreadRuntime, a: Record<string, unknown>): Promise<string> {
    // `message` is accepted for `text`: Claude Code, whose MCP tool schemas are deferred, was
    // seen (live) to guess it before reading the schema.
    const text = typeof a.text === "string" ? a.text : typeof a.message === "string" ? a.message : "";
    if (!text.trim()) throw new ToolError("send_message needs text: the message to send (the parameter is called `text`).");
    const r = this.resolve(a.to, "recipient");
    if (r.address === this.addressOf(rt.id)) throw new ToolError("That is your own address; a message to yourself is not delivered.");
    if (a.delivery !== undefined && a.delivery !== null && a.delivery !== "queue" && a.delivery !== "now") throw new ToolError("delivery must be \"queue\" (the default) or \"now\".");
    const now = a.delivery === "now";
    if (!this.isReply(rt, r.address)) {
      await this.ensurePermission(rt, { action: "message", summary: `Message ${this.labelOf(r.address)}`, recipients: [this.peerFor(r.address)] });
    }
    this.checkPairRate(rt, r.address);
    const mode = await this.parentModeOf(rt).catch(() => null);
    const sent = this.send({ from: this.addressOf(rt.id), fromRt: rt, senderSkipsApprovals: mode?.info ? mode.info.skipsApprovals === true : undefined, to: r.address, kind: "message", text, inReplyTo: typeof a.inReplyTo === "string" ? a.inReplyTo : null, root: this.rootOf(rt), sentItem: true, now });
    // A `now` message is acted on at once: report what was done with it.
    if (now) {
      const recipient = this.localOf(r.address);
      if (recipient) await this.tryNow(recipient);
      else await this.untilRelayed(sent.id);
      const rec = this.store.get(sent.id);
      if (rec?.receipt) sent.receipt = rec.receipt;
    }
    return `${this.receiptText(sent.receipt, this.labelOf(r.address))}\nMessage id: ${sent.id}\nThis is not a reply: call wait to receive one.`;
  }

  private receiptText(receipt: Receipt, label: string): string {
    if (receipt.status === "delivered") return `Delivered to ${label}${receipt.detail ? ` — ${receipt.detail}` : ""}.`;
    if (receipt.status === "queued") return `Queued for ${label}${receipt.detail ? ` — ${receipt.detail}` : ""}. It is stored for delivery.`;
    if (receipt.status === "outcomeUnknown") return `Delivery to ${label} is uncertain — ${receipt.detail ?? "it may have reached the agent and will not be sent again automatically"}.`;
    return `Failed: ${label} — ${receipt.detail ?? "it could not be delivered"}.`;
  }

  /** The receipt a mailbox gives a message for a local recipient, as things stand. */
  private receiptFor(rt: ThreadRuntime, m: MeshMessage): Receipt {
    const q = (reason: ReceiptReason, detail: string): Receipt => ({ status: "queued", reason, detail });
    if (rt.external.external) return q("inUseElsewhere", `the thread is open in ${rt.external.holder ?? "another program"}; YAAC delivers it when that program lets go`);
    const hold = rt.record.hold;
    if (hold && !(m.kind === "message" && hold.by && this.normalise(hold.by) === this.normalise(m.from))) {
      return q("stopped", `the thread was stopped by ${hold.label}; it waits until ${hold.by ? "that agent" : "the user"} or the user sends it a message`);
    }
    const why = m.kind === "notice" ? null : this.exhaustedReason(m.root ?? null);
    if (why) return q("budgetExhausted", `the orchestration's budget is used up (${why}); it is delivered after the user chooses Continue`);
    if (rt.pendingRequestCount() > 0) return q("awaitingUser", "the thread is waiting for its user to answer a question or approval");
    if (this.waiters.get(rt.id)?.forMessages) return { status: "queued", detail: "the recipient is waiting for messages" };
    if (rt.activeTurn || rt.dispatching) return q("recipientBusy", "the recipient is running a turn; delivery is pending");
    return m.kind === "notice" ? { status: "delivered", detail: "it sees it at its next turn" } : { status: "delivered", detail: "waiting for its harness to start a turn" };
  }

  /** Stores and routes one message from a thread on this target (or about it, for notices). */
  private send(p: { from: Address; fromRt: ThreadRuntime | null; to: Address; kind: MeshMessage["kind"]; text: string; inReplyTo?: string | null; root?: Address | null; sentItem: boolean; now?: boolean; senderSkipsApprovals?: boolean }): { id: string; receipt: Receipt } {
    const from = this.normalise(p.from);
    const to = this.normalise(p.to);
    const effective = p.fromRt?.effective;
    const modeId = effective?.applied?.mode;
    const descriptor = p.fromRt && this.core.registry.get(p.fromRt.record.instance)?.descriptor;
    const mode = modeId && [...(effective?.modes ?? []), ...(descriptor?.modes ?? [])].find((m) => m.id === modeId);
    const senderSkipsApprovals = p.senderSkipsApprovals ?? (mode ? mode.skipsApprovals === true : undefined);
    const message: MeshMessage = { id: randomId("m_", 12), from, to, ...(senderSkipsApprovals !== undefined ? { senderSkipsApprovals } : {}), kind: p.kind, text: p.text, sentAt: nowIso(), seq: this.store.nextSeq(from, to), ...(p.inReplyTo ? { inReplyTo: p.inReplyTo } : {}), ...(p.root ? { root: this.normalise(p.root) } : {}), ...(p.now ? { delivery: "now" as const } : {}) };
    const recipient = this.localOf(to);
    const now = Date.now();
    let rec: MessageRecord;
    if (recipient) {
      rec = { message, inbox: true, outbox: !!p.fromRt, inState: "pending", outState: "local", receipt: this.receiptFor(recipient, message), recipient: recipient.id, ...(p.fromRt ? { sender: p.fromRt.id } : {}), storedAt: now };
    } else if (this.isLocalAddress(to)) {
      rec = { message, inbox: false, outbox: !!p.fromRt, outState: "done", receipt: { status: "failed", reason: "threadDeleted", detail: "that thread no longer exists" }, ...(p.fromRt ? { sender: p.fromRt.id } : {}), storedAt: now };
    } else {
      const online = !!this.hub && !this.hub.isClosed;
      const known = this.store.state.roster.length === 0 || this.store.state.roster.some((r) => r.id === this.split(to)?.target);
      rec = known
        ? { message, inbox: false, outbox: true, outState: "relayPending", receipt: online ? { status: "queued", reason: "targetOffline", detail: "on its way through the YAAC hub" } : { status: "queued", reason: "hubUnavailable", detail: "the hub is not reachable; it waits here and is sent when the hub returns" }, ...(p.fromRt ? { sender: p.fromRt.id } : {}), storedAt: now }
        : { message, inbox: false, outbox: true, outState: "done", receipt: { status: "failed", reason: "unknownAddress", detail: "no such machine in this mesh" }, ...(p.fromRt ? { sender: p.fromRt.id } : {}), storedAt: now };
    }
    if (p.fromRt && p.sentItem) (rec as MessageRecord & { sentTurn?: string }).sentTurn = p.fromRt.activeTurn ?? "";
    this.store.put(rec);
    if (rec.receipt?.reason === "budgetExhausted") this.budgetChanged(message.root);
    if (p.kind === "message") this.relate(from, to);
    if (p.fromRt && p.sentItem) this.sentItem(p.fromRt, rec, "itemStarted");
    if (recipient) {
      if (!p.now) this.kick(recipient.id);
    } else if (rec.outState === "relayPending") void this.relay(rec);
    return { id: message.id, receipt: rec.receipt! };
  }

  /** A passive notice from YAAC: never starts a turn; handed over with the next turn or wait. */
  private notice(about: Address, to: Address, text: string, root?: Address | null): void {
    this.send({ from: about, fromRt: null, to, kind: "notice", text, root: root ?? null, sentItem: false });
  }

  private relate(a: Address, b: Address): void {
    for (const [x, y] of [
      [a, b],
      [b, a],
    ] as const) {
      const rt = this.localOf(x);
      if (!rt) continue;
      const tm = this.store.thread(rt.id);
      const n = this.normalise(y);
      if (!tm.peers.includes(n) && !tm.children.includes(n) && tm.spawnedBy !== n) tm.peers.push(n);
    }
    this.store.save();
  }

  private sentItem(rt: ThreadRuntime, rec: MessageRecord, type: "itemStarted" | "itemUpdated"): void {
    if (rt.deleted) return;
    const turnId = (rec as MessageRecord & { sentTurn?: string }).sentTurn ?? "";
    const item = { id: `sent:${rec.message.id}`, turnId, status: "completed", kind: "meshSent", message: rec.message, receipt: rec.receipt, to: this.peerFor(rec.message.to) } as unknown as Item;
    if (type === "itemStarted") {
      this.core.injectItem(rt.id, "itemStarted", item);
      this.core.injectItem(rt.id, "itemCompleted", item);
    } else this.core.injectItem(rt.id, "itemUpdated", item);
  }

  private updateReceipt(rec: MessageRecord, receipt: Receipt, patch: Partial<MessageRecord> = {}): void {
    const changed = JSON.stringify(rec.receipt ?? null) !== JSON.stringify(receipt);
    if (!changed && Object.keys(patch).length === 0) return;
    const held = rec.receipt?.reason === "budgetExhausted" || receipt.reason === "budgetExhausted";
    const state = patch.inState ?? rec.inState;
    const terminal = state === "offered" || state === "consumed" || state === "failed" || state === "outcomeUnknown";
    const receiptPending = changed && terminal && receipt.status !== "queued" && rec.inbox && !rec.outbox && !this.isLocalAddress(rec.message.from) && rec.message.kind !== "notice";
    this.store.update(rec.message.id, { ...patch, receipt, ...(receiptPending ? { receiptPending: true } : {}) });
    if (!changed) return;
    void this.forwardReceipt(rec);
    if (held && rec.inbox) this.budgetChanged(rec.message.root);
    const sender = rec.sender ? this.core.thread(rec.sender) : undefined;
    if (sender && !sender.deleted && (rec as MessageRecord & { sentTurn?: string }).sentTurn !== undefined) this.sentItem(sender, rec, "itemUpdated");
  }

  private async forwardReceipt(rec: MessageRecord): Promise<void> {
    const hub = this.hub;
    const receipt = rec.receipt;
    const id = rec.message.id;
    if (!hub || hub.isClosed || this.legacyReceiptHubs.has(hub) || !receipt || !rec.receiptPending || this.receiptsInFlight.has(id)) return;
    this.receiptsInFlight.add(id);
    try {
      await hub.requestHost("mesh/deliveryReceipt", { messageId: id, receipt }, 30_000);
      if (!this.closed && rec.receipt === receipt) this.store.update(id, { receiptPending: false });
    } catch (e) {
      // An older hub only returns the original mailbox receipt. Other failures retry this
      // idempotent, durable receipt on the next tick or connection. Never resend the mail.
      if (e instanceof RpcError && e.code === "method_not_found") this.legacyReceiptHubs.add(hub);
    } finally {
      this.receiptsInFlight.delete(id);
    }
  }

  private async relay(rec: MessageRecord): Promise<void> {
    const hub = this.hub;
    if (!hub || hub.isClosed || !this.store.state.self) return;
    // An address made before the hub named this target is rewritten to its id now.
    const m = rec.message;
    const fixed: MeshMessage = { ...m, from: this.normalise(m.from), ...(m.root ? { root: this.normalise(m.root) } : {}) };
    if (fixed.from !== m.from || fixed.root !== m.root) this.store.update(m.id, { message: fixed });
    try {
      const r = (await hub.requestHost("mesh/relay", { message: fixed }, 30_000)) as { receipt?: Receipt };
      const current = this.store.get(m.id);
      if (!current || current.outState !== "relayPending") return;
      this.store.update(m.id, { outState: "relayed" });
      if (r.receipt) this.updateReceipt(current, r.receipt);
    } catch (e) {
      if (e instanceof RpcError && e.code === "policy_denied") {
        this.store.update(m.id, { outState: "done" });
        this.updateReceipt(rec, { status: "failed", reason: "policyDenied", detail: e.message });
      }
      // Otherwise it stays in the outbox and is sent again (the id makes a repeat harmless).
    }
  }

  private flushOutbox(): void {
    for (const r of this.store.all()) {
      if (r.outbox && r.outState === "relayPending") void this.relay(r);
      else void this.forwardReceipt(r);
    }
  }

  // ── Delivery ───────────────────────────────────────────────────────────────────────

  private threadsWithMail(): string[] {
    const ids = new Set<string>();
    for (const r of this.store.all()) if (r.inbox && r.inState === "pending" && r.recipient) ids.add(r.recipient);
    return [...ids];
  }

  kick(threadId: string): void {
    if (this.closed || this.kickScheduled.has(threadId)) return;
    this.kickScheduled.add(threadId);
    setImmediate(() => {
      this.kickScheduled.delete(threadId);
      try {
        this.deliver(threadId);
      } catch (e) {
        this.opts.log(`mesh: delivery to ${threadId} failed: ${(e as Error)?.stack ?? e}`);
      }
    });
  }

  private inputOf(r: MessageRecord): MeshInputMessage {
    const m = r.message;
    return { id: m.id, kind: m.kind, from: m.from, fromLabel: this.labelOf(m.from), text: m.text, senderSkipsApprovals: m.senderSkipsApprovals ?? undefined };
  }

  private receivedItem(r: MessageRecord, turnId: TurnId): Item {
    return { id: `recv:${r.message.id}`, turnId, status: "completed", kind: "meshReceived", from: this.peerFor(r.message.from), message: r.message } as unknown as Item;
  }

  /**
   * The table of recipient states (docs/10-agent-mesh.md#what-happens-to-a-message). Only the
   * session layer's dispatcher hands anything to a harness.
   */
  private deliver(threadId: string): void {
    const rt = this.core.thread(threadId);
    if (!rt || rt.deleted) return;
    const pending = this.store.inbox(threadId, ["pending"]);
    if (pending.length === 0) return;
    // A message from the agent that stopped a thread is what starts it again.
    const hold = rt.record.hold;
    if (hold?.by && pending.some((r) => r.message.kind === "message" && this.normalise(r.message.from) === this.normalise(hold.by!))) this.core.releaseHold(rt, false);
    if (this.nowInFlight.has(threadId) || this.peerInFlight.has(threadId)) return;
    if (pending.some((r) => r.message.delivery === "now" && !r.nowHandled) && (rt.activeTurn || rt.dispatching)) {
      void this.tryNow(rt);
      return;
    }
    const waiter = this.waiters.get(threadId);
    if (waiter && !waiter.done && waiter.forMessages) {
      const available = this.waitable(rt, pending);
      if (available.length > 0) {
        waiter.finish(this.offer(rt, available, waiter.turnId));
        return;
      }
    }
    if (this.dispatches.has(threadId)) return;
    // Notices are passive: they never start a turn.
    if (!pending.some((r) => r.message.kind !== "notice")) return;
    if (rt.activeTurn && !rt.dispatching && !rt.claimed && !rt.record.hold && !rt.external.external && rt.pendingRequestCount() === 0) {
      const first = pending.find((r) => r.message.kind !== "notice");
      if (first && !this.exhaustedReason(first.message.root ?? null)) {
        void this.deliverPeer(rt, first, rt.activeTurn);
        return;
      }
    }
    if (!this.core.dispatchableNow(rt)) {
      for (const r of pending) if (r.message.kind !== "notice") this.updateReceipt(r, this.receiptFor(rt, r.message));
      return;
    }
    const ready: MessageRecord[] = [];
    for (const r of pending) {
      const why = r.message.kind === "notice" ? null : this.exhaustedReason(r.message.root ?? null);
      if (why) {
        this.updateReceipt(r, this.receiptFor(rt, r.message));
        if (r.message.root) this.budgetNotice(r.message.root, why);
        if (r.message.root && !this.isLocalAddress(r.message.root)) {
          const rr = this.store.state.remoteRoots[this.normalise(r.message.root)];
          if (rr && Date.now() - (rr.reportedAt ?? 0) > 30_000) void this.reportUsage(this.normalise(r.message.root));
        }
      } else ready.push(r);
    }
    const waking = ready.filter((r) => r.message.kind !== "notice");
    if (waking.length === 0) return;
    this.dispatch(rt, ready, waking[0]!.message.root ?? waking[0]!.message.from);
  }

  private async deliverPeer(rt: ThreadRuntime, rec: MessageRecord, oldTurn: TurnId): Promise<void> {
    const input = this.inputOf(rec);
    const id = rec.message.id;
    const root = this.normalise(rec.message.root ?? rec.message.from);
    this.peerInFlight.add(rt.id);
    try {
      const turnId = await this.core.deliverPeer(rt, {
        input: [{ type: "text", text: frameMessages([input]) }],
        mesh: [input],
        journalItems: [this.receivedItem(rec, "")],
      }, () => {
        if (rec.inState !== "pending" || this.exhaustedReason(rec.message.root ?? null)) return false;
        this.store.update(id, { inState: "dispatching" });
        this.pendingRoots.set(rt.id, root);
        return true;
      }, () => {
        this.pendingRoots.set(rt.id, root);
        this.countTurn(root);
      });
      if (!turnId) {
        if (rec.inState === "dispatching") this.store.update(id, { inState: "pending" });
        const fallback = this.receiptFor(rt, rec.message);
        if (fallback.reason === "recipientBusy" && rt.activeTurn === oldTurn) {
          fallback.detail = "the harness could not take it mid-turn; it waits for the current turn to end";
        }
        this.updateReceipt(rec, fallback);
        return;
      }
      if (turnId !== oldTurn) this.countTurn(root);
      this.updateReceipt(rec, turnId === oldTurn
        ? { status: "delivered", detail: "its harness accepted it for the next boundary", handling: "nextBoundary" }
        : { status: "delivered", detail: "it started a new turn with it", handling: "delivered" }, { inState: "consumed", turnId });
      this.kick(rt.id);
    } catch (e) {
      if (rec.inState === "dispatching") {
        if (e instanceof PeerInputDenied) this.updateReceipt(rec, { status: "failed", reason: "policyDenied", detail: e.message }, { inState: "failed" });
        else this.unknownDelivery(rt, rec, e as Error);
      }
      this.opts.log(`mesh: peer delivery to ${rt.id}: ${(e as Error).message}`);
    } finally {
      this.pendingRoots.delete(rt.id);
      this.peerInFlight.delete(rt.id);
      if (rt.activeTurn !== oldTurn || rec.inState === "outcomeUnknown" || rec.inState === "failed") this.kick(rt.id);
    }
  }

  private unknownDelivery(rt: ThreadRuntime, rec: MessageRecord, error: Error): void {
    this.updateReceipt(rec, { status: "outcomeUnknown", detail: `the harness may have accepted it (${error.message}). It will not be sent again automatically` }, { inState: "outcomeUnknown" });
    this.noticeItem(rt, "warning", `A message from ${this.labelOf(rec.message.from)} may have reached this thread. YAAC will not send it again automatically.`);
  }

  private dispatch(rt: ThreadRuntime, recs: MessageRecord[], root: Address): void {
    const tm = this.store.thread(rt.id);
    const intro =
      !tm.introduced && rt.record.originKind !== "yaac"
        ? identityBlock({ address: this.addressOf(rt.id), title: this.core.summaryOf(rt).title, provider: this.providerName(rt.record.instance), target: this.selfName() })
        : null;
    const inputs = recs.map((r) => this.inputOf(r));
    const items = recs.map((r) => this.receivedItem(r, ""));
    const ids = recs.map((r) => r.message.id);
    let turn: TurnId | null = null;
    const handle = this.core.enqueueAgentInput(rt.id, {
      input: [{ type: "text", text: frameMessages(inputs, intro) }],
      mesh: inputs,
      journalItems: items,
      beforeStart: () => {
        // The intent, durably, before the harness is asked.
        for (const id of ids) this.store.update(id, { inState: "dispatching" });
        this.pendingRoots.set(rt.id, this.normalise(root));
      },
      onStarted: (turnId) => {
        turn = turnId;
        for (const r of recs) {
          const patch = { inState: "consumed" as const, turnId, ...(r.message.delivery === "now" ? { nowHandled: true } : {}) };
          if (r.message.kind === "notice") this.store.update(r.message.id, patch);
          else this.updateReceipt(r, { status: "delivered", detail: "it started a turn with it", handling: "delivered" }, patch);
        }
        if (intro) tm.introduced = true;
        this.countTurn(root);
      },
      onFailed: (error) => {
        this.pendingRoots.delete(rt.id);
        // Retry only a definite refusal. A missing response can mean the turn already ran.
        for (const id of ids) {
          const r = this.store.get(id);
          if (!r || r.inState !== "dispatching") continue;
          if (error instanceof RpcError && error.code === "outcome_unknown") {
            this.unknownDelivery(rt, r, error);
            continue;
          }
          if (error instanceof RpcError && error.code === "policy_denied") {
            this.updateReceipt(r, { status: "failed", reason: "policyDenied", detail: error.message }, { inState: "failed" });
            continue;
          }
          const n = (r.dispatchErrors ?? 0) + 1;
          if (n >= 3) {
            this.updateReceipt(r, { status: "failed", detail: `the harness refused it: ${error.message}` }, { inState: "failed", dispatchErrors: n });
          } else this.store.update(id, { inState: "pending", dispatchErrors: n });
        }
      },
    });
    this.dispatches.set(rt.id, handle);
    handle.started
      .catch(() => null)
      .finally(() => {
        if (this.dispatches.get(rt.id) === handle) this.dispatches.delete(rt.id);
        // Cancelled before it was handed over (a `wait` took the mail): nothing was consumed,
        // and the records are still pending.
        void turn;
        this.store.save();
        this.kick(rt.id);
      });
  }

  /** Whether a local thread has itself dealt with `from` (messaged it, spawned it, or was spawned under it). */
  private hasContacted(rt: ThreadRuntime, from: Address): boolean {
    const f = this.normalise(from);
    const tm = this.store.peekThread(rt.id);
    if (tm?.children.some((c) => this.normalise(c) === f)) return true;
    if (this.ancestorsOf(rt).includes(f)) return true;
    return this.store.all().some((r) => r.sender === rt.id && this.normalise(r.message.to) === f);
  }

  /**
   * `now` delivery (docs/10-agent-mesh.md#queued-or-now) to a thread that is mid-turn: steered
   * into its running turn where its harness can steer, otherwise its turn is interrupted and a
   * new one starts with the mail. Only from a thread above it; never while it waits on its
   * user; never to a held thread (unless from its stopper, which releases it). Otherwise the
   * message is delivered as `queue`, and its receipt says why.
   */
  async tryNow(rt: ThreadRuntime): Promise<void> {
    if (this.nowInFlight.has(rt.id) || this.peerInFlight.has(rt.id) || rt.deleted) return;
    const pending = this.store.inbox(rt.id, ["pending"]);
    const asked = pending.filter((r) => r.message.delivery === "now" && !r.nowHandled);
    if (asked.length === 0) return this.kick(rt.id);
    const hold = rt.record.hold;
    if (hold?.by && pending.some((r) => r.message.kind === "message" && this.normalise(r.message.from) === this.normalise(hold.by!))) this.core.releaseHold(rt, false);
    const degrade = (r: MessageRecord, reason: ReceiptReason, detail: string) => {
      this.store.update(r.message.id, { nowHandled: true });
      this.updateReceipt(r, { status: "queued", reason, detail, handling: "queued" });
    };
    const eligible: MessageRecord[] = [];
    for (const r of asked) {
      if (!this.isBeneath(rt, r.message.from)) degrade(r, "notBeneathSender", "`now` is only for threads beneath the sender: it was delivered as `queue`, and waits for the running turn to end");
      else if (!this.exhaustedReason(r.message.root ?? null)) eligible.push(r);
      else degrade(r, "budgetExhausted", "the orchestration’s budget is used up; it waits for the user to choose Continue");
    }
    if (eligible.length === 0) return this.kick(rt.id);
    if (rt.pendingRequestCount() > 0) {
      for (const r of eligible) degrade(r, "awaitingUser", "the thread is waiting on the user (a question or an approval); `now` never interrupts that: it is queued");
      return;
    }
    if (rt.record.hold) {
      for (const r of eligible) degrade(r, "stopped", `the thread was stopped by ${rt.record.hold.label}; it is queued`);
      return;
    }
    if (!rt.activeTurn && !rt.dispatching) return this.kick(rt.id); // idle: an ordinary delivery
    if (rt.external.external) {
      for (const r of eligible) degrade(r, "inUseElsewhere", `the thread is open in ${rt.external.holder ?? "another program"}`);
      return;
    }
    const recs = eligible;
    const ids = recs.map((r) => r.message.id);
    const inputs = recs.map((r) => this.inputOf(r));
    const items = recs.map((r) => this.receivedItem(r, ""));
    const root = this.normalise(eligible[0]!.message.root ?? eligible[0]!.message.from);
    const tm = this.store.thread(rt.id);
    const spawnTurn = tm.spawnTurn;
    const replacing = !!spawnTurn && spawnTurn === rt.activeTurn && !tm.resultSent;
    this.nowInFlight.add(rt.id);
    const existing = this.dispatches.get(rt.id);
    if (existing) existing.cancel();
    try {
      const steer = await this.core.canSteer(rt);
      // An interrupted spawn turn's answer is not the worker's result: the new turn's is.
      if (replacing && !steer) tm.spawnTurn = null;
      const result = await this.core.deliverNow(rt, [{ type: "text", text: frameMessages(inputs) }], {}, null, "mesh", {
        journalItems: items,
        mesh: inputs,
        beforeStart: () => {
          for (const id of ids) this.store.update(id, { inState: "dispatching" });
          this.pendingRoots.set(rt.id, root);
        },
        onStarted: (turnId) => {
          for (const id of ids) this.store.update(id, { inState: "consumed", turnId, nowHandled: true });
          if (replacing && !steer) tm.spawnTurn = turnId;
        },
      });
      if (result.outcome !== "steered") this.countTurn(root);
      const handling = result.outcome === "steered" ? "steered" : result.outcome === "interruptedAndStarted" ? "interruptedAndDelivered" : "delivered";
      const detail = handling === "steered" ? "steered into its running turn" : handling === "interruptedAndDelivered" ? "its running turn was interrupted and a new one started with it" : "it started a turn with it";
      for (const id of ids) {
        const r = this.store.get(id);
        if (r && r.message.kind !== "notice") this.updateReceipt(r, { status: "delivered", detail, handling });
      }
    } catch (e) {
      if (replacing && tm.spawnTurn === null) tm.spawnTurn = spawnTurn;
      for (const id of ids) {
        const r = this.store.get(id);
        if (r && r.inState === "dispatching") this.unknownDelivery(rt, r, e as Error);
      }
      for (const r of eligible) if (r.inState === "pending") degrade(r, "recipientBusy", `it could not be delivered at once (${(e as Error).message}); it is queued`);
    } finally {
      this.pendingRoots.delete(rt.id);
      this.nowInFlight.delete(rt.id);
      this.store.save();
      this.kick(rt.id);
    }
  }

  /** Waits (briefly) until a relayed message's receipt says what the other target did with it. */
  private async untilRelayed(id: string, timeoutMs = 10_000): Promise<void> {
    const deadline = Date.now() + timeoutMs;
    while (Date.now() < deadline) {
      const r = this.store.get(id);
      if (!r || r.receipt?.handling || r.receipt?.status === "failed" || r.outState === "done") return;
      await new Promise((res) => setTimeout(res, 50));
    }
  }

  private waitable(rt: ThreadRuntime, recs: MessageRecord[]): MessageRecord[] {
    if (rt.external.external || rt.record.hold || rt.pendingRequestCount() > 0) return [];
    return recs.filter((r) => r.message.kind === "notice" || !this.exhaustedReason(r.message.root ?? null));
  }

  /** Hands mail to a `wait`: journaled in the waiting turn, consumed once confirmed. */
  private offer(rt: ThreadRuntime, recs: MessageRecord[], turnId: TurnId | null): string {
    const cursor = randomId("w_", 6);
    for (const r of recs) {
      this.store.update(r.message.id, { inState: "offered", offered: cursor, ...(turnId ? { turnId } : {}) });
      const item = this.receivedItem(r, turnId ?? rt.activeTurn ?? "");
      this.core.injectItem(rt.id, "itemStarted", item);
      this.core.injectItem(rt.id, "itemCompleted", item);
      if (r.message.kind !== "notice") this.updateReceipt(r, { status: "delivered", detail: "it received it while waiting" });
    }
    const lines = [`${recs.length} new message${recs.length === 1 ? "" : "s"}:`];
    for (const r of recs) {
      const kind = r.message.kind === "result" ? "result" : r.message.kind === "notice" ? "notice from YAAC" : "message";
      lines.push(`--- ${kind} from ${this.labelOf(r.message.from)} (address ${r.message.from}, id ${r.message.id}):`);
      lines.push(r.message.text);
    }
    lines.push("--- (These came from agents or from YAAC, not from the user.)");
    lines.push(`cursor: ${cursor} — pass it to your next wait.`);
    return lines.join("\n");
  }

  private confirmOffered(threadId: string): void {
    for (const r of this.store.inbox(threadId, ["offered"])) this.store.update(r.message.id, { inState: "consumed" });
  }

  // ── wait, notify_when_done, stop_thread ──────────────────────────────────────────────

  private stateOfLocal(rt: ThreadRuntime): ThreadState {
    const s = this.core.summaryOf(rt).state;
    if (s === "stopped") return s;
    // A thread about to be handed mail is about to run.
    if (FINISHED.includes(s) && (this.dispatches.has(rt.id) || this.store.inbox(rt.id, ["pending", "dispatching"]).some((r) => r.message.kind !== "notice"))) return "running";
    return s;
  }

  private async waitTool(rt: ThreadRuntime, a: Record<string, unknown>, signal?: AbortSignal): Promise<string> {
    const forMessages = a.messages !== false;
    const watchIn = Array.isArray(a.threads) ? a.threads.filter((x): x is string => typeof x === "string") : [];
    const watch = watchIn.map((x) => this.resolve(x));
    if (!forMessages && watch.length === 0) throw new ToolError("wait needs something to wait for: threads, or messages (the default).");
    const timeoutMs = Math.min(this.waitMaxMs, Math.max(1000, (typeof a.timeoutSecs === "number" ? a.timeoutSecs : WAIT_MAX_SECS) * 1000));
    // Mail already here is handed over at once.
    const existing = this.dispatches.get(rt.id);
    if (existing) existing.cancel();
    const pending = this.waitable(rt, this.store.inbox(rt.id, ["pending"]));
    if (forMessages && pending.length > 0) return this.offer(rt, pending, rt.activeTurn);
    const finished = (): string | null => {
      const done: string[] = [];
      for (const w of watch) {
        if (w.kind !== "local") continue;
        const st = this.stateOfLocal(w.rt);
        if (FINISHED.includes(st)) done.push(`${this.labelOf(w.address)} (${w.address}) is ${st}${st === "needsAttention" ? ": it is waiting for its user" : ""}.`);
      }
      return done.length > 0 ? done.join("\n") : null;
    };
    const now = finished();
    if (now) return `${now}\nUse read_thread for its answer, if it has not come to you as a result.`;
    const itemId = `wait:${randomId("", 8)}`;
    const targets = () => watch.map((w) => ({ peer: this.peerFor(w.address), state: w.kind === "local" ? this.stateOfLocal(w.rt) : ((this.store.state.labels[w.address]?.state as ThreadState) ?? "running") }));
    const turnId = rt.activeTurn;
    const startItem = { id: itemId, turnId: turnId ?? "", status: "inProgress", kind: "meshWait", forMessages, targets: targets() } as unknown as Item;
    this.core.injectItem(rt.id, "itemStarted", startItem);
    const result = await new Promise<string>((resolve) => {
      let timer: NodeJS.Timeout | null = null;
      let poll: NodeJS.Timeout | null = null;
      const waiter: Waiter = {
        threadId: rt.id,
        turnId,
        forMessages,
        watch: watch.map((w) => w.address),
        itemId,
        done: false,
        finish: (text) => {
          if (waiter.done) return;
          waiter.done = true;
          if (timer) clearTimeout(timer);
          if (poll) clearInterval(poll);
          signal?.removeEventListener("abort", onAbort);
          if (this.waiters.get(rt.id) === waiter) this.waiters.delete(rt.id);
          resolve(text);
        },
      };
      const onAbort = () => waiter.finish("The wait was cancelled; nothing was consumed.");
      signal?.addEventListener("abort", onAbort, { once: true });
      this.waiters.get(rt.id)?.finish("Another wait replaced this one.");
      this.waiters.set(rt.id, waiter);
      timer = setTimeout(() => waiter.finish(`Nothing yet after ${Math.round(timeoutMs / 1000)} seconds. Call wait again to keep waiting.`), timeoutMs);
      timer.unref();
      const remote = watch.filter((w): w is Resolved & { kind: "remote" } => w.kind === "remote");
      if (remote.length > 0) {
        const check = async () => {
          const byTarget = new Map<string, Address[]>();
          for (const w of remote) byTarget.set(w.target, [...(byTarget.get(w.target) ?? []), w.address]);
          const done: string[] = [];
          for (const [target, threads] of byTarget) {
            try {
              const r = (await this.hubQuery(this.addressOf(rt.id), target, { tool: "threadStates", threads })) as { states?: Array<{ thread: Address; state?: ThreadState | null }> };
              for (const s of r.states ?? []) {
                const peer = this.peerFor(s.thread);
                if (s.state) this.remember(peer, s.state);
                if (!s.state) done.push(`${this.labelOf(s.thread)} no longer exists.`);
                else if (FINISHED.includes(s.state)) done.push(`${this.labelOf(s.thread)} (${s.thread}) is ${s.state}.`);
              }
            } catch {
              /* that machine is out of reach now: keep waiting */
            }
          }
          if (done.length > 0 && !waiter.done) {
            // Its result may already be on the way: give it a moment to arrive as mail.
            const mail = this.store.inbox(rt.id, ["pending"]);
            waiter.finish(mail.length > 0 && forMessages ? this.offer(rt, mail, turnId) : done.join("\n"));
          }
        };
        poll = setInterval(() => void check(), this.remotePollMs);
        poll.unref();
        void check();
      }
      // Mail that arrived between the check above and now.
      if (forMessages) this.kick(rt.id);
    });
    if (!rt.deleted) {
      const end = { id: itemId, turnId: turnId ?? "", status: "completed", kind: "meshWait", forMessages, targets: targets().map((t) => ({ ...t, finished: FINISHED.includes(t.state) })) } as unknown as Item;
      this.core.injectItem(rt.id, "itemCompleted", end);
    }
    return result;
  }

  private watchedFinished(address: Address, state: ThreadState): void {
    if (!FINISHED.includes(state)) return;
    // After the turn's own bookkeeping (its result is stored in the same tick), so a finished
    // worker's wait returns its result rather than only its state.
    setImmediate(() => this.watchedFinishedNow(address, state));
  }

  private watchedFinishedNow(address: Address, state: ThreadState): void {
    for (const w of this.waiters.values()) {
      if (w.done || !w.watch.map((x) => this.normalise(x)).includes(this.normalise(address))) continue;
      if (!FINISHED.includes(state)) continue;
      const rt = this.core.thread(w.threadId);
      // Prefer the thread's result, when it has already come as mail.
      const mail = rt ? this.store.inbox(rt.id, ["pending"]) : [];
      if (rt && w.forMessages && mail.length > 0) w.finish(this.offer(rt, mail, w.turnId));
      else w.finish(`${this.labelOf(address)} (${address}) is ${state}${state === "needsAttention" ? ": it is waiting for its user" : ""}.`);
    }
  }

  private async notifyTool(rt: ThreadRuntime, a: Record<string, unknown>): Promise<string> {
    const r = this.resolve(a.to);
    if (r.address === this.addressOf(rt.id)) throw new ToolError("That is your own address.");
    if (!rt.activeTurn) throw new ToolError("There is no running turn to register against.");
    if (!this.isReply(rt, r.address)) await this.ensurePermission(rt, { action: "message", summary: `Send its final answer to ${this.labelOf(r.address)} when its turn ends`, recipients: [this.peerFor(r.address)] });
    const tm = this.store.thread(rt.id);
    if (!tm.notify.some((n) => n.to === r.address && n.turnId === rt.activeTurn)) tm.notify.push({ to: r.address, turnId: rt.activeTurn });
    this.store.save();
    return `Registered: when this turn ends — however it ends — YAAC sends your final answer to ${this.labelOf(r.address)}.`;
  }

  /** Every thread above a local one, nearest first (its parent, the parent's parent, …). */
  ancestorsOf(rt: ThreadRuntime, depth = 0): Address[] {
    const tm = this.store.peekThread(rt.id);
    if (!tm?.spawnedBy) return [];
    if (tm.ancestors && tm.ancestors.length > 0) return tm.ancestors.map((a) => this.normalise(a));
    const parent = this.normalise(tm.spawnedBy);
    const prt = depth < 16 ? this.localOf(parent) : undefined;
    return [parent, ...(prt ? this.ancestorsOf(prt, depth + 1) : [])];
  }

  /** Whether a local thread is beneath `caller` (a worker of it, or of its workers, …). */
  isBeneath(rt: ThreadRuntime, caller: Address): boolean {
    return this.ancestorsOf(rt).includes(this.normalise(caller));
  }

  private outcomeText(o: string): string {
    return o === "stopped" ? "stopped" : o === "wasNotRunning" ? "was not running" : o === "didNotRespond" ? "did not respond" : o === "unreachable" ? "could not be reached" : o;
  }

  /** The parent's workers item shows what a stop did. */
  private noteStops(results: StopResult[]): void {
    for (const r of results) if (r.outcome !== "unreachable") this.updateSpawnItems(r.thread, "stopped");
  }

  private renderStops(results: StopResult[]): string {
    this.noteStops(results);
    const lines = results.map((r) => `- ${r.title ? `"${r.title}" ` : ""}(${r.thread}): ${this.outcomeText(r.outcome)}${r.detail ? ` — ${r.detail}` : ""}.`);
    const held = results.some((r) => r.outcome !== "unreachable");
    return [...lines, held ? "Each stopped thread is held: it does not start again by itself, and messages to it wait, until you or the user send it a message." : ""].filter(Boolean).join("\n");
  }

  /**
   * Stops a local thread and holds it, then (with `subtree`) everything beneath it, on any
   * target. `by` is the agent that asked: the hold names it, and its message releases it.
   */
  private async stopLocal(rt: ThreadRuntime, by: Address, subtree: boolean): Promise<StopResult[]> {
    const address = this.addressOf(rt.id);
    const hold: ThreadHold = { by: this.normalise(by), label: this.labelOf(by), at: nowIso() };
    const title = this.core.summaryOf(rt).title;
    const [own, below] = await Promise.all([
      this.core.stopAndHold(rt, hold, this.stopWaitMs),
      subtree ? this.stopChildren(rt, by) : Promise.resolve([] as StopResult[]),
    ]);
    // Mail already waiting: its senders are told the thread is stopped.
    for (const r of this.store.inbox(rt.id, ["pending"])) if (r.message.kind !== "notice") this.updateReceipt(r, this.receiptFor(rt, r.message));
    this.store.save();
    return [{ thread: address, outcome: own.outcome, title, ...(own.detail ? { detail: own.detail } : {}) }, ...below];
  }

  private async stopChildren(rt: ThreadRuntime, by: Address): Promise<StopResult[]> {
    const kids = (this.store.peekThread(rt.id)?.children ?? []).map((c) => this.normalise(c));
    const parts = await Promise.all(
      kids.map(async (child): Promise<StopResult[]> => {
        const local = this.localOf(child);
        if (local) return this.stopLocal(local, by, true);
        const target = this.split(child)?.target;
        if (!target) return [];
        try {
          // Asked as the child's own parent (this thread), which is what the hub can check.
          const reply = (await this.hubQuery(this.addressOf(rt.id), target, { tool: "stopThread", thread: child, subtree: true, by: this.normalise(by) })) as { results?: StopResult[] };
          return reply.results ?? [];
        } catch (e) {
          return [{ thread: child, outcome: "unreachable", title: this.labelOf(child), detail: (e as Error).message }];
        }
      }),
    );
    return parts.flat();
  }

  private async stopTool(rt: ThreadRuntime, a: Record<string, unknown>): Promise<string> {
    const r = this.resolve(a.thread);
    const self = this.addressOf(rt.id);
    const subtree = a.subtree !== false;
    if (r.kind === "local") {
      if (!this.isBeneath(r.rt, self)) throw new ToolError(`stop_thread works only on threads beneath you (your workers and theirs); ${this.labelOf(r.address)} is not one of them.`);
      return this.renderStops(await this.stopLocal(r.rt, self, subtree));
    }
    let reply: { results?: StopResult[] };
    try {
      reply = (await this.hubQuery(self, r.target, { tool: "stopThread", thread: r.address, subtree })) as { results?: StopResult[] };
    } catch (e) {
      if (e instanceof RpcError && e.code === "policy_denied") throw new ToolError(`stop_thread works only on threads beneath you (your workers and theirs); ${this.labelOf(r.address)} is not one of them.`);
      throw e;
    }
    return this.renderStops(reply.results ?? []);
  }

  private async stopAllTool(rt: ThreadRuntime): Promise<string> {
    const results = await this.stopChildren(rt, this.addressOf(rt.id));
    if (results.length === 0) return "You have no workers to stop.";
    return this.renderStops(results);
  }

  /** Asks other targets for the state of this target's threads' remote workers (the live workers item). */
  async pollRemoteWorkers(): Promise<void> {
    if (this.closed || this.workerPolling || !this.hub || this.hub.isClosed) return;
    this.workerPolling = true;
    try {
      for (const parent of this.core.allThreads()) {
        const items = this.store.peekThread(parent.id)?.spawnItems ?? {};
        const byTarget = new Map<string, Address[]>();
        for (const entry of Object.values(items)) {
          for (const t of entry.threads) {
            const a = this.normalise(t.peer.address);
            if (this.isLocalAddress(a) || t.state === "done" || t.state === "failed") continue;
            const target = this.split(a)?.target;
            if (target) byTarget.set(target, [...(byTarget.get(target) ?? []), a]);
          }
        }
        for (const [target, threads] of byTarget) {
          try {
            const reply = (await this.hubQuery(this.addressOf(parent.id), target, { tool: "threadStates", threads })) as { states?: Array<{ thread: Address; state?: ThreadState | null }> };
            for (const st of reply.states ?? []) if (st.state) this.updateSpawnItems(st.thread, st.state);
          } catch (e) {
            // That target is away: the next poll tries again.
            this.opts.log(`mesh: asking ${target} for its workers' states failed: ${(e as Error).message}`);
          }
        }
      }
    } finally {
      this.workerPolling = false;
    }
  }

  private limitNotice(rt: ThreadRuntime, what: string): void {
    this.noticeItem(rt, "meshLimit", `An agent limit was reached: ${what}. The agent was told.`);
  }

  private noticeItem(rt: ThreadRuntime, kind: string, text: string): void {
    if (rt.deleted) return;
    const item = { id: `notice:mesh:${randomId("", 8)}`, turnId: rt.activeTurn ?? "", status: "completed", kind: "notice", noticeKind: kind, text } as unknown as Item;
    this.core.injectItem(rt.id, "itemStarted", item);
    this.core.injectItem(rt.id, "itemCompleted", item);
  }

  // ── Session-core hooks ─────────────────────────────────────────────────────────────

  turnStarted(rt: ThreadRuntime, _turnId: TurnId, origin: TurnOrigin): void {
    this.kick(rt.id);
    if (origin === "mesh") {
      const root = this.pendingRoots.get(rt.id);
      this.pendingRoots.delete(rt.id);
      if (root) {
        this.store.thread(rt.id).currentRoot = root;
        this.store.save();
      }
      return;
    }
    // A person (here or in a terminal) acted: this thread is the root of what follows.
    const tm = this.store.peekThread(rt.id);
    const self = this.addressOf(rt.id);
    if (tm && tm.currentRoot !== self) {
      tm.currentRoot = self;
      this.store.save();
    }
    const rs = this.store.state.roots[self];
    if (rs) {
      rs.lastHumanAt = Date.now();
      if (rs.denied) rs.denied = false;
      this.store.save();
      this.budgetChanged(self);
    }
  }

  turnCompleted(rt: ThreadRuntime, turnId: TurnId, status: string, error: string | null): void {
    this.confirmOffered(rt.id);
    const w = this.waiters.get(rt.id);
    if (w && !w.done) w.finish("The turn ended.");
    const tm = this.store.peekThread(rt.id);
    const last = this.lastText.get(rt.id);
    const answer = last && last.turnId === turnId ? last.text : "";
    const suffix = status === "completed" ? "" : `\n\n(Its turn ${status === "failed" ? `failed${error ? `: ${error}` : ""}` : `ended: ${status}`}.)`;
    if (tm) {
      const due = tm.notify.filter((n) => n.turnId === turnId);
      if (due.length > 0) {
        tm.notify = tm.notify.filter((n) => n.turnId !== turnId);
        this.store.save();
        for (const n of due) {
          this.send({ from: this.addressOf(rt.id), fromRt: rt, to: n.to, kind: "message", text: `${answer || "(It gave no answer.)"}${suffix}`, root: this.rootOf(rt), sentItem: true });
        }
      }
      // A worker's answer to the task it was spawned with returns to its parent, once.
      if (tm.spawnTurn === turnId && !tm.resultSent && tm.spawnedBy) {
        tm.resultSent = true;
        this.store.save();
        if (!tm.detached) {
          this.send({ from: this.addressOf(rt.id), fromRt: rt, to: tm.spawnedBy, kind: "result", text: `${answer || "(It gave no answer.)"}${suffix}`, root: tm.spawnRoot ?? null, sentItem: true });
        }
      }
    }
    this.lastText.delete(rt.id);
    // A running slot may have freed up.
    for (const id of this.threadsWithMail()) if (id !== rt.id) this.kick(id);
  }

  itemCompleted(rt: ThreadRuntime, item: Item): void {
    if (item.kind === "agentMessage" && typeof (item as { text?: unknown }).text === "string" && item.turnId) {
      this.lastText.set(rt.id, { turnId: item.turnId, text: (item as { text: string }).text });
    }
  }

  requestRaised(rt: ThreadRuntime, rec: RequestRecord): void {
    if (rec.requestKind === "meshPermission") return;
    const tm = this.store.peekThread(rt.id);
    if (!tm?.spawnedBy) return;
    // A worker blocked on its user is seen by its parent (passively).
    this.notice(this.addressOf(rt.id), tm.spawnedBy, `Your worker ${this.labelOf(this.addressOf(rt.id))} is waiting for its user to answer ${rec.requestKind === "approval" ? "an approval" : "a question"}. It cannot continue until the user answers; you cannot answer for them.`, tm.spawnRoot ?? null);
  }

  decorate(rt: ThreadRuntime, s: ThreadSummary): void {
    const tm = this.store.peekThread(rt.id);
    if (!tm) return;
    if (tm.spawnedBy) {
      s.spawnedBy = this.normalise(tm.spawnedBy);
      s.reportsToParent = !tm.detached;
    }
    if (tm.children.length > 0) s.children = tm.children.length;
    if (tm.peers.length > 0) s.peers = tm.peers.length;
    const root = tm.currentRoot ?? tm.spawnRoot;
    if (root) s.root = this.normalise(root);
  }

  /** Part of a live orchestration: never cut by the discovery caps. */
  keeps(rt: ThreadRuntime): boolean {
    if (this.waiters.has(rt.id) || this.dispatches.has(rt.id) || this.nowInFlight.has(rt.id)) return true;
    if (this.store.inbox(rt.id, ["pending", "dispatching", "offered"]).length > 0) return true;
    const tm = this.store.peekThread(rt.id);
    if (!tm || (tm.children.length === 0 && tm.peers.length === 0 && !tm.spawnedBy)) return false;
    const root = tm.currentRoot ?? tm.spawnRoot;
    if (!root) return false;
    const rs = this.store.state.roots[this.normalise(root)];
    // How long an orchestration stays "live" for discovery: its time limit, but never more than
    // LIVE_WINDOW_MS — a budget without a time limit (P351) must not keep it live forever.
    const windowMs = Math.min(LIVE_WINDOW_MS, (rs?.allowance.wallClockSecs ?? this.store.state.policy.budget.wallClockSecs) * 1000);
    if (rs) return Date.now() - rs.lastHumanAt < windowMs;
    // A root on another target: live while messages of it moved recently.
    return this.store.all().some((r) => (r.recipient === rt.id || r.sender === rt.id) && Date.now() - r.storedAt < windowMs);
  }

  holdReleased(rt: ThreadRuntime): void {
    this.kick(rt.id);
  }

  /** How the mesh names a thread's machine and the thread (a handoff fork's preamble). */
  whereIs(rt: ThreadRuntime): { target: string; address: string } {
    return { target: this.selfName(), address: this.addressOf(rt.id) };
  }

  childrenOf(rt: ThreadRuntime): readonly Address[] {
    return (this.store.peekThread(rt.id)?.children ?? []).map(address => this.normalise(address));
  }

  stateChanged(rt: ThreadRuntime): void {
    const address = this.addressOf(rt.id);
    const state = this.core.summaryOf(rt).state;
    this.watchedFinished(address, this.stateOfLocal(rt));
    if (this.store.peekThread(rt.id)?.spawnedBy) this.updateSpawnItems(address, state);
  }

  threadRemoved(rt: ThreadRuntime): void {
    const address = this.addressOf(rt.id);
    this.waiters.get(rt.id)?.finish("This thread was deleted.");
    for (const r of this.store.inbox(rt.id, ["pending", "dispatching", "offered"])) {
      this.store.update(r.message.id, { inState: "failed" });
      if (r.message.kind === "notice") continue;
      this.updateReceipt(r, { status: "failed", reason: "threadDeleted", detail: "the thread was deleted before it received the message" });
      if (!r.sender || !this.core.thread(r.sender)) {
        if (!this.isLocalAddress(r.message.from)) this.notice(address, r.message.from, `Your message ${r.message.id} to ${address} was not delivered: that thread was deleted.`);
      } else {
        this.notice(address, r.message.from, `Your message to ${this.labelOf(address)} was not delivered: that thread was deleted.`);
      }
    }
    for (const [t, v] of Object.entries(this.store.state.tickets)) if (v.instance === rt.record.instance && v.native === rt.record.native) delete this.store.state.tickets[t];
    this.store.save();
  }

  // ── Housekeeping ───────────────────────────────────────────────────────────────────

  tick(): void {
    if (this.closed) return;
    const limit = this.store.state.policy.limits.undeliveredNoticeSecs * 1000;
    const now = Date.now();
    for (const r of this.store.all()) {
      if (!r.inbox || r.inState !== "pending" || r.noticed || r.message.kind === "notice") continue;
      if (now - r.storedAt < limit) continue;
      this.store.update(r.message.id, { noticed: true });
      const minutes = Math.round((now - r.storedAt) / 60000);
      this.notice(r.message.to, r.message.from, `Your message to ${this.labelOf(r.message.to)} has not been delivered after ${minutes} minutes (${r.receipt?.detail ?? "it is still queued"}). It has not been cancelled.`, r.message.root ?? null);
    }
    if (this.hub && !this.hub.isClosed) {
      this.flushOutbox();
      for (const [root, rr] of Object.entries(this.store.state.remoteRoots)) if (rr.paused || now - (rr.reportedAt ?? 0) > 5 * 60_000) void this.reportUsage(root);
    }
    for (const [k, s] of this.snapshots) if (now - s.at > SNAPSHOT_TTL_MS) this.snapshots.delete(k);
  }

  // ── Methods the hub (and the window) call ───────────────────────────────────────────

  methods(): Map<string, MethodHandler> {
    const m = new Map<string, MethodHandler>();
    m.set("mesh/roster", (client, p) => this.onRoster(client, p));
    m.set("mesh/deliver", (client, p) => this.onDeliver(client, p));
    m.set("mesh/receipt", (_c, p) => this.onReceipt(p));
    m.set("mesh/query", (_c, p) => this.onQuery(p as unknown as MeshQueryParams));
    m.set("mesh/usage", (_c, p) => this.onUsage(p));
    m.set("mesh/policy", (_c, p) => this.onPolicy(p));
    m.set("mesh/budget", (_c, p) => this.onBudget(p));
    return m;
  }

  private onRoster(client: ClientSession, p: Record<string, unknown>): unknown {
    const selfId = typeof p.selfId === "string" ? p.selfId : null;
    const selfName = typeof p.selfName === "string" ? p.selfName : null;
    if (!selfId || !selfName) throw new RpcError("invalid_params", "selfId and selfName are required");
    const prev = this.store.state.self;
    if (prev && prev.id !== selfId) this.opts.log(`mesh: the hub now calls this target ${selfId} (was ${prev.id})`);
    this.store.state.self = { id: selfId, name: selfName };
    this.store.state.roster = Array.isArray(p.targets) ? (p.targets as StateFileRoster) : [];
    this.store.state.rosterAt = nowIso();
    this.store.save();
    // Refresh parent addresses for harness sub-agents (D151) and workers spawned before the
    // hub attached, whose local parent was recorded using the provisional target id.
    if (prev?.id !== selfId) {
      this.core.refreshSubagents();
      for (const rt of this.core.allThreads()) if (rt.record.spawnedBy) this.core.updateIndex(rt);
    }
    if (this.hub !== client) {
      this.hub = client;
      client.onClosed(() => {
        if (this.hub === client) this.hub = null;
      });
    }
    this.flushOutbox();
    for (const root of Object.keys(this.store.state.remoteRoots)) void this.reportUsage(root);
    return {};
  }

  private async onDeliver(client: ClientSession, p: Record<string, unknown>): Promise<unknown> {
    if (this.hub !== client && this.hub === null) this.hub = client;
    const message = p.message as MeshMessage | undefined;
    if (!message || typeof message.id !== "string" || typeof message.to !== "string" || typeof message.from !== "string") throw new RpcError("invalid_params", "a message is required");
    const known = this.store.get(message.id);
    if (known) return { receipt: known.receipt ?? { status: "delivered" } };
    const rt = this.localOf(message.to);
    if (!rt) {
      const receipt: Receipt = { status: "failed", reason: this.isLocalAddress(message.to) ? "threadDeleted" : "unknownAddress", detail: "no such thread on that machine" };
      this.store.put({ message, inbox: false, outbox: false, outState: "done", receipt, storedAt: Date.now() });
      return { receipt };
    }
    if (this.isLocalAddress(message.from)) throw new RpcError("invalid_params", "a message relayed to this target cannot claim to come from it");
    // The target's own switch: agents elsewhere may not start conversations here. Results,
    // notices, and replies within what this thread itself started still arrive.
    if (!this.store.state.policy.acceptFromOtherTargets && message.kind === "message" && !this.hasContacted(rt, message.from)) {
      throw new RpcError("policy_denied", `${this.selfName()} does not accept messages from agents on other machines`);
    }
    const receipt = this.receiptFor(rt, message);
    // Stored (deduplicated in the same step) before the hub is answered.
    this.store.put({ message: { ...message, to: this.addressOf(rt.id) }, inbox: true, outbox: false, inState: "pending", recipient: rt.id, receipt, storedAt: Date.now() });
    if (receipt.reason === "budgetExhausted") this.budgetChanged(message.root);
    if (message.kind === "message") this.relate(message.from, message.to);
    // A worker on another target finished: its parent's live spawn item says so.
    if (message.kind === "result") this.updateSpawnItems(message.from, /\(Its turn failed/.test(message.text) ? "failed" : "done", undefined, true);
    if (message.delivery === "now") {
      // Acted on before the hub is answered, so the sender learns what was done.
      await this.tryNow(rt);
      return { receipt: this.store.get(message.id)?.receipt ?? receipt };
    }
    this.kick(rt.id);
    return { receipt };
  }

  private onReceipt(p: Record<string, unknown>): unknown {
    const id = typeof p.messageId === "string" ? p.messageId : null;
    const receipt = p.receipt as Receipt | undefined;
    if (!id || !receipt) throw new RpcError("invalid_params", "messageId and receipt are required");
    const rec = this.store.get(id);
    if (!rec) return {};
    // A terminal peer receipt can beat the initial mailbox acknowledgement through the hub.
    if (rec.receipt?.handling && receipt.status === "queued") return {};
    const before = rec.receipt?.status;
    if (rec.outState !== "done") this.store.update(id, { outState: receipt.status === "queued" ? "relayed" : "done" });
    this.updateReceipt(rec, receipt);
    if (before !== "failed" && receipt.status === "failed" && rec.sender && rec.message.kind !== "notice") {
      this.notice(rec.message.to, this.addressOf(rec.sender), `Your message to ${this.labelOf(rec.message.to)} could not be delivered: ${receipt.detail ?? receipt.reason ?? "it failed"}.`);
    }
    // A child's result arriving updates the parent's spawn item.
    return {};
  }

  private async onQuery(p: MeshQueryParams): Promise<unknown> {
    const caller = typeof p.caller === "string" ? p.caller : null;
    const q = p.query as MeshQuery & Record<string, unknown>;
    if (!caller || !q || typeof q.tool !== "string") throw new RpcError("invalid_params", "caller and query are required");
    if (this.isLocalAddress(caller)) throw new RpcError("policy_denied", "a query from another target cannot name a caller on this one");
    switch (q.tool) {
      case "listModels":
        await this.core.registry.waitForModelBestUses();
        return { tool: "listModels", providers: this.localDescriptors().map(({ disabledModels: _off, ...d }) => d) };
      case "listThreads":
        return { tool: "listThreads", ...this.listForQuery(caller, (q.filter as MeshThreadFilter) ?? {}) };
      case "threadStates":
        return {
          tool: "threadStates",
          states: ((q.threads as Address[]) ?? []).map((t) => {
            const rt = this.localOf(t);
            return rt ? { thread: t, state: this.stateOfLocal(rt) } : { thread: t };
          }),
        };
      case "readThread": {
        const rt = this.localOf(String(q.thread ?? ""));
        if (!rt) throw new RpcError("thread_not_found", `no thread ${String(q.thread)} on ${this.selfName()}`);
        const t = await this.transcript(rt, q.lastAnswerOnly === true, Math.max(500, Math.min(Number(q.maxBytes) || 8000, 32000)));
        return { tool: "readThread", ...t };
      }
      case "stopThread": {
        const rt = this.localOf(String(q.thread ?? ""));
        if (!rt) throw new RpcError("thread_not_found", `no thread ${String(q.thread)} on ${this.selfName()}`);
        // Checked here, by the thread's own daemon: only for a thread beneath the caller.
        if (!this.isBeneath(rt, caller)) throw new RpcError("policy_denied", "only a thread beneath the caller (its workers, and theirs) may be stopped by it");
        const by = typeof q.by === "string" && q.by ? q.by : caller;
        const results = await this.stopLocal(rt, by, q.subtree !== false);
        this.noteStops(results);
        return { tool: "stopThread", results };
      }
      case "spawnThread": {
        if (!this.store.state.policy.acceptFromOtherTargets) throw new RpcError("policy_denied", `${this.selfName()} does not accept work from agents on other machines`);
        const reply = await this.spawnLocal({
          caller,
          callerRt: null,
          instance: typeof q.instance === "string" && q.instance ? q.instance : null,
          model: typeof q.model === "string" ? q.model : null,
          effort: typeof q.effort === "string" ? q.effort : null,
          cwd: typeof q.cwd === "string" ? q.cwd : null,
          projectKey: typeof q.projectKey === "string" ? q.projectKey : null,
          title: typeof q.title === "string" && q.title ? q.title : "Worker",
          text: String(q.text ?? ""),
          detached: q.detached === true,
          root: typeof q.root === "string" ? q.root : caller,
          depth: typeof q.depth === "number" ? q.depth : 1,
          ancestors: Array.isArray(q.ancestors) && q.ancestors.length > 0 ? (q.ancestors as Address[]) : [caller],
          mode: typeof q.mode === "string" && q.mode ? q.mode : null,
          // Another target's instance is never this one's: modes are mapped by what they mean.
          parentMode: (() => {
            const info = q.parentMode && typeof q.parentMode === "object" ? (q.parentMode as ModeInfo) : null;
            return { instance: null, id: info?.id ?? null, info };
          })(),
        }).catch((e) => {
          if (e instanceof ToolError) throw new RpcError("invalid_params", e.message);
          throw e;
        });
        return { tool: "spawnThread", ...reply };
      }
      default:
        throw new RpcError("invalid_params", `this target does not answer ${q.tool}`);
    }
  }

  private onUsage(p: Record<string, unknown>): unknown {
    const root = typeof p.root === "string" ? this.normalise(p.root) : null;
    const source = typeof p.source === "string" ? p.source : null;
    if (!root || !source) throw new RpcError("invalid_params", "root and source are required");
    if (!this.localOf(root)) throw new RpcError("thread_not_found", `the root ${root} is not on this target`);
    const rs = this.rootState(root);
    const prev = rs.remote[source] ?? { agentTurns: 0, threadsSpawned: 0 };
    rs.remote[source] = { agentTurns: Math.max(prev.agentTurns, Number(p.agentTurns) || 0), threadsSpawned: Math.max(prev.threadsSpawned, Number(p.threadsSpawned) || 0) };
    const why = this.exhaustedReason(root);
    if (why) this.budgetNotice(root, why);
    this.store.save();
    this.budgetChanged(root);
    return { exhausted: !!why, used: this.usedOf(rs) };
  }

  private onPolicy(p: Record<string, unknown>): unknown {
    const update = p.update as { mode?: string | null; acceptFromOtherTargets?: boolean | null; budget?: Partial<MeshBudget> | null; limits?: Record<string, number | null> | null } | null | undefined;
    if (update?.mode !== undefined && update.mode !== null && !["allow", "askFirst", "alwaysAsk"].includes(update.mode)) throw new RpcError("invalid_params", "mode must be allow, askFirst, or alwaysAsk");
    if (update?.acceptFromOtherTargets !== undefined && update.acceptFromOtherTargets !== null && typeof update.acceptFromOtherTargets !== "boolean") throw new RpcError("invalid_params", "acceptFromOtherTargets must be true or false");
    const apply = () => {
      const pol = this.store.state.policy;
      if (typeof update?.mode === "string") pol.mode = update.mode;
      if (typeof update?.acceptFromOtherTargets === "boolean") pol.acceptFromOtherTargets = update.acceptFromOtherTargets;
      for (const [k, v] of Object.entries(update?.budget ?? {})) if (typeof v === "number" && v >= 0) (pol.budget as unknown as Record<string, number>)[k] = v;
      for (const [k, v] of Object.entries(update?.limits ?? {})) if (typeof v === "number" && v >= 0) (pol.limits as unknown as Record<string, number>)[k] = v;
      this.store.save();
      return this.policyResult();
    };
    if (!update) return this.policyResult();
    if (typeof p.opId !== "string") throw new RpcError("invalid_params", "opId is required with update");
    return this.core.ops.run("mesh/policy", p.opId, async () => apply());
  }

  private policyResult(): unknown {
    const pol = this.store.state.policy;
    return { policy: { mode: pol.mode, acceptFromOtherTargets: pol.acceptFromOtherTargets, budget: pol.budget, limits: pol.limits } };
  }

  private onBudget(p: Record<string, unknown>): unknown {
    const root = typeof p.root === "string" ? this.normalise(p.root) : null;
    if (!root) throw new RpcError("invalid_params", "root is required");
    if (!this.localOf(root)) throw new RpcError("thread_not_found", "the root is not a thread on this target");
    const status = () => this.budgetStatus(root);
    if (p.action === "setTimeLimit") return this.setTimeLimit(root, p, status);
    if (p.action !== "continue") return status();
    if (typeof p.opId !== "string") throw new RpcError("invalid_params", "opId is required");
    return this.core.ops.run("mesh/budget", p.opId, async () => {
      const rs = this.rootState(root);
      const used = this.usedOf(rs);
      const base = this.store.state.policy.budget;
      // Another allowance of turns on top of what is used, and a person has acted now: the
      // clock starts again. The root's time limit stays as it is — one a person set (P352)
      // is theirs to change, and the default has none (P351).
      rs.allowance = { ...rs.allowance, agentTurns: Math.max(rs.allowance.agentTurns, used.agentTurns + base.agentTurns) };
      rs.lastHumanAt = Date.now();
      rs.exhaustedNoticed = false;
      this.store.save();
      this.budgetChanged(root);
      for (const id of this.threadsWithMail()) this.kick(id);
      return status();
    });
  }

  /**
   * `setTimeLimit` (P352): a person sets, extends or removes the root's time limit. Only the
   * limit changes — it is not a person acting on the thread, so the clock runs on; a limit
   * that frees the root dispatches what it held back.
   */
  private setTimeLimit(root: Address, p: Record<string, unknown>, status: () => MeshBudgetResult): unknown {
    if (typeof p.opId !== "string") throw new RpcError("invalid_params", "opId is required");
    const secs = p.wallClockSecs;
    if (!(secs === null || (typeof secs === "number" && Number.isInteger(secs) && secs > 0))) {
      throw new RpcError("invalid_params", "wallClockSecs must be a positive whole number of seconds, or null for no time limit");
    }
    return this.core.ops.run("mesh/budget", p.opId, async () => {
      const rs = this.rootState(root);
      rs.allowance = { ...rs.allowance, wallClockSecs: secs === null ? NO_TIME_LIMIT : Math.min(secs, NO_TIME_LIMIT) };
      this.store.save();
      this.budgetChanged(root);
      return status();
    });
  }
}

type StateFileRoster = MeshStore["state"]["roster"];
