// The session core: threads, journals, requests, control, the dispatcher, and the index —
// everything between the providers and the connected clients. It knows no protocol version
// and no wire encoding; client.ts turns its results into responses.

import * as fs from "node:fs";
import * as path from "node:path";
import type {
  ControlChangedEvent,
  DiagnosticsGetResult,
  InputPart,
  Item,
  JournalPosition,
  ModelId,
  ProviderDescriptor,
  RequestResolution,
  RequestResponse,
  ThreadEffective,
  ThreadEvent,
  ThreadForkParams,
  ThreadForkPreviewParams,
  ThreadForkPreviewResult,
  ThreadForkResult,
  ThreadReadParams,
  ThreadReadResult,
  ThreadStartParams,
  ThreadCompactParams,
  ThreadStartResult,
  ThreadSubscribeResult,
  ThreadSummary,
  TurnId,
  TurnOrigin,
  TurnStartParams,
  TurnStartResult,
  RequestRespondParams,
  TurnSteerParams,
  TurnInterruptParams,
  UploadPresence,
  Capability,
  ItemReadResult,
  Project,
  RepoLocation,
  ThreadForkOnlyParams,
  ThreadForkOnlyResult,
  ThreadUpdateParams,
  ThreadUpdateResult,
  ThreadMoveParams,
  ThreadMoveResult,
  ThreadPlacement,
  DiscoveryCaps,
  DiscoveryCapsParams,
  DiscoveryCapsResult,
  ThreadHold,
  ThreadLookupParams,
  ThreadLookupResult,
  ThreadOlderParams,
  ThreadOlderResult,
  QueuedMessage,
  QueueChange,
  TurnStartOutcome,
} from "../protocol/generated/types.js";
import { canonical } from "../protocol/json.js";
import { RpcError } from "../protocol/rpc.js";
import { InputOutcomeUnknown, PeerInputDenied } from "../providers/types.js";
import type { ExternalControl, HarnessProvider, MeshInputMessage, RaisedRequest, NativeId, NativeThreadInfo, ProviderEvent, ProviderFactory, ProviderInput, StartThreadParams, TurnOverrides } from "../providers/types.js";
import { CompactionUnconfirmed } from "../providers/types.js";
import { ProviderRegistry, type Instance } from "../providers/registry.js";
import { Emitter } from "../util/emitter.js";
import { ensureDir, nowIso, randomId, readJson, sha256Hex, writeJsonAtomic } from "../util/fsx.js";
import { OUTPUT_HEAD_BYTES, Projection, applyDelta, clone, utf8Head } from "./items.js";
import { lastSeqOf, type JournalEntry, type JournalOptions } from "./journal.js";
import { NotActed, OpLog } from "./ops.js";
import { ThreadIndex, type Holder } from "./thread-index.js";
import { ThreadRuntime, threadDirName, type Driver, type PlacementRecord, type QueuedEntry, type RequestRecord, type ThreadRecord, type Work } from "./thread.js";
import { UploadStore } from "./uploads.js";
import { HANDOFF_ITEM_BYTES, boundTranscript, cutoffTurn, itemsThrough, preamble, previewOf, renderTranscript, type AttachmentLookup, type RenderedTranscript } from "./handoff.js";
import { normaliseRemote } from "./fsinfo.js";
import { GitStatusStore, gitRunner, resolveRepo, type GitRunner } from "./git.js";
import { ProjectStore, matchProject, projectRefFor, userProjectRef } from "./projects.js";
import { FolderTrust } from "./trust.js";
import { Limiter } from "../util/exec.js";
import { loadWorkerLaunchHook, type WorkerLaunchIdentity } from "./worker-launch.js";

export const PAGE_MAX_BYTES = 64 * 1024;
export const ITEM_READ_MAX = 512 * 1024;
const SNAPSHOT_TTL_MS = 10 * 60 * 1000;
/** The longest a stop waits for git status computations in flight. */
const CLOSE_GIT_WAIT_MS = 2000;
/** The longest a stop waits for the providers to close. */
const CLOSE_DISPOSE_WAIT_MS = 5000;

/** Whether `p` settled within `ms` (it is not cancelled either way). */
async function within(p: Promise<unknown>, ms: number): Promise<boolean> {
  let timer: NodeJS.Timeout | undefined;
  const late = new Promise<false>((resolve) => {
    // Not unref'd: the stop itself keeps the process alive until it has finished.
    timer = setTimeout(() => resolve(false), ms);
  });
  try {
    return await Promise.race([p.then(() => true as const), late]);
  } finally {
    clearTimeout(timer);
  }
}
const SNAPSHOT_MAX = 64;
/** How long a `now` delivery waits for an interrupted turn to end before giving up. */
const NOW_INTERRUPT_WAIT_MS = 5000;
/** Summaries of older conversations handed out by `thread/older`, so their ids work. */
const OLDER_MEMO_MAX = 5000;
const DEFAULT_CAPS: DiscoveryCaps = { autoDiscover: true, enabled: true, maxAgeDays: 30, maxThreads: 50 };

export interface CoreOptions {
  home: string;
  env: NodeJS.ProcessEnv;
  factories: ProviderFactory[];
  log: (line: string) => void;
  /** Delta coalescing: flush after this long… */
  coalesceMs: number;
  /** …or once this many bytes are pending. */
  coalesceBytes: number;
  journal: JournalOptions;
  /** The previous daemon stopped cleanly. */
  cleanStart: boolean;
  tempDir?: string;
  /** The slow git-status refresh for watched checkouts; 0 turns it off. */
  gitRefreshMs?: number;
  /** At most this many git computations at once. */
  gitConcurrency?: number;
}

export const DEFAULT_GIT_REFRESH_MS = 120_000;

/** What the core needs from a connected client. */
export interface ClientLink {
  readonly key: string;
  readonly host: string | null;
  notify(method: string, params: unknown): void;
  /** New journal entries exist for a thread. */
  wake(threadId: string): void;
  /** A snapshot of `threadId` valid at `position` was handed to this client. */
  onSnapshot(threadId: string, position: JournalPosition): void;
  /** Whether this client is subscribed to a thread (git status is refreshed only for those). */
  isSubscribed?(threadId: string): boolean;
}

interface Snapshot {
  id: string;
  threadId: string;
  items: Item[];
  position: JournalPosition;
  at: number;
}

// ── Agent mesh wiring (WP7) ──────────────────────────────────────────────────────────────
/**
 * What the agent mesh (`../mesh/`) hears from the session core. Every call site is guarded
 * (`this.mesh?.…`): the core works the same without a mesh.
 */
export interface CoreMeshHooks {
  /** A turn began (journaled already). */
  turnStarted(rt: ThreadRuntime, turnId: TurnId, origin: TurnOrigin): void;
  /**
   * A person gave the thread input — a message (started, queued, sent now, steered) or an
   * answer to one of its requests — before it is acted on (D164).
   */
  humanInput?(rt: ThreadRuntime): void;
  /** A turn ended (journaled already), before queued work is dispatched. */
  turnCompleted(rt: ThreadRuntime, turnId: TurnId, status: string, error: string | null): void;
  /** An item reached its final state. */
  itemCompleted(rt: ThreadRuntime, item: Item): void;
  /** A request was raised and journaled. */
  requestRaised(rt: ThreadRuntime, rec: RequestRecord): void;
  /** Adds the mesh's relationships to a summary. */
  decorate(rt: ThreadRuntime, summary: ThreadSummary): void;
  /** Answers a request the mesh raised (`meshPermission`). */
  respond(rt: ThreadRuntime, rec: RequestRecord, response: RequestResponse, client: ClientLink): Promise<void>;
  /** The thread's summary changed. */
  stateChanged(rt: ThreadRuntime): void;
  /** The thread is about to be forgotten (deleted). */
  threadRemoved(rt: ThreadRuntime): void;
  /** Whether the thread is part of a live orchestration (never cut by the discovery caps). */
  keeps?(rt: ThreadRuntime): boolean;
  /** The thread's hold was released (a message from its stopper, or the user). */
  holdReleased?(rt: ThreadRuntime): void;
  /** How the mesh names the thread's machine and the thread itself (a handoff's preamble). */
  whereIs?(rt: ThreadRuntime): { target: string; address: string };
  /** The mesh address of a thread on this target (a harness sub-agent's `spawnedBy`, D151). */
  addressOf?(threadId: string): string;
  /** The source's direct mesh children, including children on other targets. */
  childrenOf?(rt: ThreadRuntime): readonly string[];
}

/**
 * Capabilities a harness's own sub-agent (D151) never has in YAAC while it is read-only:
 * everything that would make YAAC a second writer, or change it in the harness.
 */
const SUBAGENT_WRITES: ReadonlySet<Capability> = new Set<Capability>([
  "threadResume",
  "threadFork",
  "threadForkAtTurn",
  "threadRename",
  "threadArchive",
  "threadDelete",
  "threadTakeOver",
  "threadMove",
  "threadMoveWorkingDirectory",
  "interrupt",
  "steer",
  "compact",
  "rewindConversation",
  "rewindFiles",
  "modes",
  "modelSwitch",
  "effortSwitch",
  "fastMode",
  "ultrafastMode",
  "meshTools",
]);

const METADATA_PENDING = "Codex thread metadata is not available yet.";

/** The thread state a sub-agent's harness status stands for while YAAC runs no turn of it. */
function subagentState(status: string | undefined): ThreadSummary["state"] | null {
  if (status === "running") return "running";
  if (status === "completed") return "done";
  if (status === "failed") return "failed";
  return null;
}

/** Agent-originated input for the dispatcher (the mesh's entry point). */
export interface AgentDispatch {
  /** The input the turn starts with (e.g. a framed agent message). */
  input: InputPart[];
  overrides?: TurnOverrides;
  /** Durably record the intent before the harness is asked. */
  beforeStart?: () => void | Promise<void>;
  /** The harness accepted the turn: record its id. */
  onStarted?: (turnId: TurnId) => void | Promise<void>;
  /** The harness refused or failed. The dispatch is not retried automatically. */
  onFailed?: (error: Error) => void;
  /** Items journaled at the turn's start instead of a user message (`meshReceived`). */
  journalItems?: Item[];
  /** The agent messages, sender and body apart, for the provider. */
  mesh?: MeshInputMessage[];
}

export interface DispatchHandle {
  /** Resolves with the turn id once dispatched; rejects if the harness refused. */
  readonly started: Promise<TurnId>;
  /** Removes it from the queue if it has not been dispatched yet. */
  cancel(): boolean;
}

function controlFor(holder: Holder, clientKey: string): { control: ThreadSummary["control"]; controlHolder: string | null } {
  if (holder.external) return { control: "external", controlHolder: holder.external };
  if (holder.driver) {
    return holder.driver === clientKey
      ? { control: "mine", controlHolder: null }
      : { control: "otherClient", controlHolder: holder.driverHost ?? "another YAAC client" };
  }
  return { control: "free", controlHolder: null };
}

const NOW_WHILE_ASKING = "the agent is waiting for an answer to its question or approval; a message can only be queued now (answering the card is what moves it on)";

function sleepMs(ms: number): Promise<void> {
  return new Promise((r) => setTimeout(r, ms));
}

/**
 * The fixed note at the start of the first turn after a `thread/move` changed the working
 * directory (D144). Only the harness receives it; the transcript shows the message as sent.
 */
export function movedNote(from: string, to: string): string {
  return `[YAAC] Your working directory is now ${to}. It was ${from}; paths earlier in this conversation refer to that folder.\n\n---\n\n`;
}

/**
 * Threads by their harness id within an instance. Two stores of one instance (its home or
 * socket changed, P10/P300) may use the same harness id; the thread of the store the instance
 * leads to now wins, then one bound before its store was known.
 */
class NativeIndex {
  private map = new Map<string, Array<{ store: string | null; id: string }>>();

  private key(instance: string, native: string): string {
    return `${instance}\u0000${native}`;
  }

  set(instance: string, store: string | null, native: string, id: string): void {
    const k = this.key(instance, native);
    const list = (this.map.get(k) ?? []).filter((e) => e.id !== id && e.store !== store);
    list.push({ store, id });
    this.map.set(k, list);
  }

  /** With the instance's store unknown (`now` null), any; else only one of that store, or unbound. */
  get(instance: string, native: string, now: string | null): string | undefined {
    const list = this.map.get(this.key(instance, native)) ?? [];
    if (now === null) return list.at(-1)?.id;
    return (list.find((e) => e.store === now) ?? list.find((e) => e.store === null))?.id;
  }

  has(instance: string, native: string, now: string | null): boolean {
    return this.get(instance, native, now) !== undefined;
  }

  delete(instance: string, native: string, id: string): void {
    const k = this.key(instance, native);
    const list = (this.map.get(k) ?? []).filter((e) => e.id !== id);
    if (list.length > 0) this.map.set(k, list);
    else this.map.delete(k);
  }
}

export function threadIdFor(kind: string, storeKey: string | null, instance: string, native: string): string {
  const basis = storeKey ? ["s", kind, storeKey, native] : ["i", instance, native];
  return `t_${sha256Hex(JSON.stringify(basis)).slice(0, 32)}`;
}

function rpc(code: string, message: string, data?: unknown): RpcError {
  return new RpcError(code, message, data);
}

function notActed(code: string, message: string, data?: unknown): NotActed {
  return new NotActed(new RpcError(code, message, data));
}

export class Core {
  readonly registry: ProviderRegistry;
  readonly index: ThreadIndex;
  readonly ops: OpLog;
  readonly uploads: UploadStore;
  readonly projects: ProjectStore;
  readonly gitStatus: GitStatusStore;
  /** Folder trust in the harnesses (D147). */
  readonly folderTrust: FolderTrust;
  readonly clients = new Set<ClientLink>();
  /** Fires when running turns or pending requests change (idle tracking, hand-over). */
  readonly activity = new Emitter<void>();
  /** Fires with a thread id whenever a thread becomes held by yaacd and idle. */
  readonly heldAndIdle = new Emitter<string>();
  /** Fires with a checkout root whenever its git status changed (a commit, a push; the forge refreshes on it). */
  readonly gitChanged = new Emitter<string>();
  /** The agent mesh, when one is attached (WP7). */
  mesh: CoreMeshHooks | null = null;
  private threads = new Map<string, ThreadRuntime>();
  private byNative = new NativeIndex();
  /** Harness sub-agents (D151) of each parent, by `nativeKey(instance, parent native)`: thread ids. */
  private subagentKids = new Map<string, Set<string>>();
  private snapshots = new Map<string, Snapshot>();
  private pendingStarts = new Map<string, number>();
  private buffered = new Map<string, ProviderEvent[]>();
  private metadataReads = new WeakMap<ThreadRuntime, { pending: Promise<void> | null; failedAt?: number }>();
  private readonly threadsDir: string;
  private readonly git: GitRunner;
  private readonly gitLimiter: Limiter;
  private readonly repoLookups = new Map<string, Promise<RepoLocation | null>>();
  private gitTimer: NodeJS.Timeout | null = null;
  /** `thread/older` results, by thread id: what is needed to bring one in when it is opened. */
  private olderMemo = new Map<string, { instance: string; info: NativeThreadInfo }>();
  private caps: DiscoveryCaps;
  private readonly capsFile: string;
  /** Harness sessions the user deleted from YAAC only (D110): discovery leaves them out. */
  private readonly forgottenFile: string;
  private forgotten: Set<string>;
  /** Threads the host keeps (`thread/keep`): never cut by the discovery caps. */
  private kept = new Set<string>();
  private readonly keepFile: string;

  constructor(readonly opts: CoreOptions) {
    this.threadsDir = path.join(opts.home, "threads");
    ensureDir(this.threadsDir);
    this.uploads = new UploadStore(opts.tempDir);
    this.registry = new ProviderRegistry(opts.home, opts.env, opts.log, (id) => this.clearInstanceCommands(id),
      (bytes, name) => this.uploads.storeToolImage(bytes, name));
    for (const f of opts.factories) this.registry.registerKind(f);
    this.index = new ThreadIndex(opts.home, opts.cleanStart);
    this.ops = new OpLog(path.join(opts.home, "ops.jsonl"));
    this.projects = new ProjectStore(path.join(opts.home, "projects.json"));
    this.folderTrust = new FolderTrust(path.join(opts.home, "folder-trust.json"), opts.log);
    this.git = gitRunner(opts.env);
    this.gitLimiter = new Limiter(Math.max(1, opts.gitConcurrency ?? 2));
    this.gitStatus = new GitStatusStore(path.join(opts.home, "git-status.json"), this.git, this.gitLimiter, (root) => this.onGitChanged(root));
    this.registry.onEvent((instance, event) => this.onProviderEvent(instance, event));
    this.registry.onDescriptor((d) => this.broadcast("provider/updated", { provider: d }));
    this.capsFile = path.join(opts.home, "discovery.json");
    let stored: Partial<DiscoveryCaps> | undefined;
    try {
      stored = readJson<{ caps?: Partial<DiscoveryCaps> }>(this.capsFile)?.caps;
    } catch {
      stored = undefined;
    }
    this.caps = { ...DEFAULT_CAPS, ...(stored ?? {}) };
    this.forgottenFile = path.join(opts.home, "forgotten.json");
    try {
      this.forgotten = new Set(readJson<{ natives?: string[] }>(this.forgottenFile)?.natives ?? []);
    } catch {
      this.forgotten = new Set();
    }
    this.keepFile = path.join(opts.home, "keep.json");
    try {
      this.kept = new Set(readJson<{ threadIds?: string[] }>(this.keepFile)?.threadIds ?? []);
    } catch {
      this.kept = new Set();
    }
  }

  // ── Start-up ───────────────────────────────────────────────────────────────────────────

  async start(): Promise<void> {
    this.loadThreads();
    this.registry.load();
    await this.registry.probe();
    await this.reconcileAfterRestart();
    await this.scanAll();
    // Queued user messages are delivered with no window attached.
    for (const rt of this.allThreads()) if (rt.userQueue.length > 0) this.pump(rt);
    void this.refreshProjectRepos();
    // Resolve older records and retry previous misses in the background.
    for (const rt of this.allThreads()) if (!rt.record.repoResolved || !rt.record.repo) void this.resolveProject(rt);
    const every = this.opts.gitRefreshMs ?? DEFAULT_GIT_REFRESH_MS;
    if (every > 0) {
      this.gitTimer = setInterval(() => this.refreshWatchedGit(), every);
      this.gitTimer.unref();
    }
  }

  private loadThreads(): void {
    for (const name of fs.readdirSync(this.threadsDir)) {
      const dir = path.join(this.threadsDir, name);
      let record: ThreadRecord | undefined;
      try {
        record = readJson<ThreadRecord>(path.join(dir, "thread.json"));
      } catch {
        record = undefined;
      }
      if (!record) continue;
      const rt = new ThreadRuntime(record, dir);
      rt.loadRequests();
      rt.loadQueue();
      const runtime = rt.loadRuntime();
      if (runtime) {
        rt.activeTurn = runtime.activeTurn;
        rt.driver = runtime.driver;
        rt.turnOrigin = runtime.turnOrigin;
        rt.lastStatus = runtime.lastStatus;
        rt.context = runtime.context ?? null;
        rt.tokens = runtime.tokens ?? null;
      }
      this.threads.set(record.id, rt);
      this.byNative.set(record.instance, record.storeKey, record.native, record.id);
      this.linkSubagent(rt);
    }
  }

  // ── Harness sub-agents (D151) ──────────────────────────────────────────────────────────

  /** Files a harness sub-agent under its parent conversation. */
  private linkSubagent(rt: ThreadRuntime): void {
    const parent = rt.record.nativeParent;
    if (!parent) return;
    const key = this.nativeKey(rt.record.instance, parent);
    let kids = this.subagentKids.get(key);
    if (!kids) this.subagentKids.set(key, (kids = new Set()));
    kids.add(rt.id);
    // Read-only from the start: listed as held by its parent, not only once it is opened.
    const readOnly = rt.record.subagent?.readOnly;
    if (readOnly) rt.external = { external: true, holder: "the agent that started it", takeOverBlocked: readOnly };
  }

  /** The parent conversation of a harness sub-agent, when YAAC knows it. */
  private subagentParent(rt: ThreadRuntime): ThreadRuntime | undefined {
    const parent = rt.record.nativeParent;
    if (!parent) return undefined;
    const p = this.threadByNative(rt.record.instance, parent);
    return p && !p.record.metadataPending ? p : undefined;
  }

  /** How many listed harness sub-agents a thread has. */
  private subagentCount(rt: ThreadRuntime): number {
    const kids = this.subagentKids.get(this.nativeKey(rt.record.instance, rt.record.native));
    if (!kids) return 0;
    let n = 0;
    for (const id of kids) {
      const k = this.threads.get(id);
      if (k && !k.deleted && this.index.get(id)) n++;
    }
    return n;
  }

  /**
   * The read-only reason of a harness sub-agent, as the provider stated it. Every sub-agent
   * is read-only unless its provider says it can be continued from YAAC (no harness can yet).
   */
  private subagentReadOnly(rt: ThreadRuntime): string | null {
    const sub = rt.record.subagent;
    if (!sub) return null;
    return sub.readOnly ?? null;
  }

  /** The mesh address of a thread here; without a mesh, the local form. */
  private addressHere(threadId: string): string {
    return this.mesh?.addressOf?.(threadId) ?? `local/${threadId}`;
  }

  /** Re-publishes every harness sub-agent's summary (the target's mesh address changed). */
  refreshSubagents(): void {
    for (const rt of this.threads.values()) if (!rt.deleted && rt.record.nativeParent) this.updateIndex(rt);
  }

  /** Turns and requests that were live when the previous daemon stopped. */
  private async reconcileAfterRestart(): Promise<void> {
    for (const rt of this.threads.values()) {
      const inst = this.registry.get(rt.record.instance);
      const provider = inst?.provider ?? null;
      let survived = false;
      if (rt.activeTurn && provider?.runningTurn) {
        survived = (await provider.runningTurn(rt.record.native).catch(() => null)) === rt.activeTurn;
      }
      const journal = rt.openJournal(this.opts.journal, false);
      for (const r of rt.requests.values()) {
        // An answer handed over but not confirmed: yaacd cannot tell whether it arrived.
        if (r.state === "submitted") this.setRequestState(rt, r, "unknown", null);
        else if (r.state === "pending" && !survived && r.requestKind !== "meshPermission") this.setRequestState(rt, r, "expired", null);
      }
      // A queued message handed over when the previous daemon stopped: yaacd cannot tell
      // whether the harness took it. It is shown, never re-sent.
      const unsure = rt.userQueue.filter((q) => q.dispatching);
      if (unsure.length > 0) {
        rt.userQueue = rt.userQueue.filter((q) => !q.dispatching);
        rt.saveQueue();
        for (const q of unsure) {
          this.queueEvent(rt, "removed", q);
          this.noticeItem(rt, "warning", "yaacd stopped while handing a queued message to the agent. It may or may not have arrived; it will not be sent again automatically.");
        }
      }
      if (rt.activeTurn && !survived) {
        journal.append({ type: "turnCompleted", turnId: rt.activeTurn, status: "interrupted", error: "yaacd stopped while the turn was running", at: nowIso() });
        rt.activeTurn = null;
        rt.turnOrigin = null;
        if (rt.driver) {
          rt.driver = null;
          this.appendControl(rt);
        }
        rt.saveRuntime();
      }
      this.updateIndex(rt);
    }
  }

  async scanAll(): Promise<void> {
    await Promise.all(this.registry.all().map((i) => this.scanInstance(i)));
  }

  /**
   * Builds the index from one instance's listing, under the discovery caps
   * (docs/05-threads.md#discovery-is-capped-by-age-and-by-number): the harness is asked newest
   * first and stops early; a conversation YAAC did not create is indexed only within both caps,
   * unless something keeps it. An index that holds more than the caps is trimmed by the same
   * rules. A failed or partial scan removes and trims nothing.
   */
  async scanInstance(instance: Instance): Promise<void> {
    const provider = instance.provider;
    if (this.caps.autoDiscover === false || !provider || instance.descriptor.status !== "ready") return;
    if (!(instance.descriptor.capabilities ?? []).includes("threadList")) return;
    const id = instance.config.id;
    const caps = this.caps;
    const ageCap = caps.enabled && caps.maxAgeDays > 0 ? new Date(Date.now() - caps.maxAgeDays * 86_400_000).toISOString() : null;
    const numCap = caps.enabled && caps.maxThreads > 0 ? caps.maxThreads : null;
    // Threads the caps never cut do not use up the number cap, so the harness is asked for
    // that many more: the top `numCap` capped ones are then always within what it lists.
    const kept = this.allThreads().filter((t) => t.record.instance === id && this.neverCut(t)).length;
    let cursor: string | null | undefined = null;
    let complete = true;
    const seen: NativeThreadInfo[] = [];
    try {
      for (let pages = 0; pages < 10000; pages++) {
        const page = await provider.scanThreads({ cursor, limit: 100, ...(ageCap ? { activeSince: ageCap } : {}), ...(numCap ? { max: numCap + kept } : {}) });
        if (this.caps !== caps) return;
        seen.push(...page.threads);
        if (page.complete === false) complete = false;
        cursor = page.cursor;
        if (!cursor) break;
      }
    } catch (e) {
      this.opts.log(`scan of ${id} failed: ${(e as Error).message}`);
      complete = false;
    }
    if (this.caps !== caps) return;
    // Which of the listed conversations are within the caps (yaacd applies them itself too).
    const keptNow = (info: NativeThreadInfo) => {
      const rt = this.threadByNative(id, info.native);
      return !!rt && this.neverCut(rt);
    };
    // A harness's own sub-agents (D151) do not use up the number cap: each is listed when its
    // parent is (within the caps or kept) and it is itself recent enough — at most `numCap`
    // of them per parent, newest first.
    const newestFirst = (a: NativeThreadInfo, b: NativeThreadInfo) => (a.updatedAt < b.updatedAt ? 1 : a.updatedAt > b.updatedAt ? -1 : 0);
    const candidates = seen
      .filter((info) => !info.parent)
      .filter((info) => !keptNow(info))
      .filter((info) => !ageCap || info.updatedAt >= ageCap)
      .sort(newestFirst);
    const within = new Set((numCap ? candidates.slice(0, numCap) : candidates).map((i) => i.native));
    const parentListed = (native: NativeId) => {
      if (within.has(native)) return true;
      const p = this.threadByNative(id, native);
      return !!p && !p.deleted && (this.neverCut(p) || this.listed(p));
    };
    const perParent = new Map<string, number>();
    for (const info of seen.filter((i) => !!i.parent && !keptNow(i)).sort(newestFirst)) {
      if (!parentListed(info.parent!)) continue;
      if (ageCap && info.updatedAt < ageCap) continue;
      const n = perParent.get(info.parent!) ?? 0;
      if (numCap && n >= numCap) continue;
      perParent.set(info.parent!, n + 1);
      within.add(info.native);
    }
    // Parents first, so that a sub-agent's parent is known when it is filed.
    seen.sort((a, b) => Number(!!a.parent) - Number(!!b.parent));
    for (const info of seen) {
      if (this.isForgotten(id, info.native)) continue;
      if (keptNow(info) || within.has(info.native)) {
        const rt = this.discovered(instance, info, false);
        if (rt.record.capped) this.setCapped(rt, false);
      } else {
        // Repair existing records even if this scan would not bring them in.
        const rt = this.threadByNative(id, info.native);
        if (rt && this.needsMetadata(rt)) this.discovered(instance, info, rt.record.capped);
      }
    }
    if (complete) {
      const listedNatives = new Set(seen.map((i) => i.native));
      for (const rt of this.allThreads()) {
        if (rt.record.instance !== id) continue;
        // A thread of another store (the instance's home or socket changed, P10/P300) is not
        // in this listing for that reason alone: it is kept, and shown unavailable.
        if (this.storeAway(rt)) continue;
        if (!listedNatives.has(rt.record.native) && this.needsMetadata(rt)) void this.recoverMetadata(instance, rt);
        const inCaps = within.has(rt.record.native) || (listedNatives.has(rt.record.native) && keptNow({ native: rt.record.native } as NativeThreadInfo));
        if (!inCaps && !this.neverCut(rt)) {
          if (!rt.record.capped) this.setCapped(rt, true);
          // Brought in by id or an older page: kept only while open.
          if (rt.record.broughtIn && !this.isOpen(rt)) {
            rt.record.broughtIn = false;
            rt.saveRecord();
          }
          this.updateIndex(rt);
        }
      }
    }
    this.index.setStale(id, !complete);
  }

  private setCapped(rt: ThreadRuntime, capped: boolean): void {
    rt.record.capped = capped;
    rt.saveRecord();
    this.updateIndex(rt);
  }

  /**
   * Never cut by the discovery caps: started by YAAC or an agent; kept by the host
   * (`thread/keep`: pinned or muted there) or archived from YAAC; active now; part of a live
   * orchestration.
   */
  neverCut(rt: ThreadRuntime): boolean {
    const r = rt.record;
    return (
      r.originKind === "yaac" ||
      !!r.spawnedBy ||
      this.kept.has(rt.id) ||
      !!r.userArchived ||
      !!r.hold ||
      !!rt.activeTurn ||
      rt.runningCommands.length > 0 ||
      rt.dispatching ||
      rt.pendingRequestCount() > 0 ||
      rt.waiting.size > 0 ||
      rt.queue.length > 0 ||
      rt.userQueue.length > 0 ||
      r.subagent?.status === "running" ||
      !!this.mesh?.keeps?.(rt)
    );
  }

  private isOpen(rt: ThreadRuntime): boolean {
    for (const c of this.clients) if (c.isSubscribed?.(rt.id)) return true;
    return false;
  }

  /** Whether a thread is in the index (listed to clients). */
  private listed(rt: ThreadRuntime): boolean {
    if (rt.record.metadataPending) return this.metadataFailed(rt) && rt.pendingRequestCount() > 0;
    return !rt.record.capped || !!rt.record.broughtIn || this.neverCut(rt) || this.isOpen(rt);
  }

  // ── Discovery caps, lookup by id, older threads ────────────────────────────────────────

  discoveryCaps(p: DiscoveryCapsParams): Promise<DiscoveryCapsResult> | DiscoveryCapsResult {
    if (!p.update) return { caps: { ...this.caps } };
    if (typeof p.opId !== "string") throw rpc("invalid_params", "opId is required with update");
    return this.ops.run("discovery/caps", p.opId, async () => {
      const u = p.update!;
      for (const k of ["maxAgeDays", "maxThreads"] as const) {
        const v = u[k];
        if (v !== undefined && v !== null && (typeof v !== "number" || !Number.isInteger(v) || v < 0)) throw notActed("invalid_params", `${k} must be a whole number, 0 for no cap`);
      }
      if (u.enabled !== undefined && u.enabled !== null && typeof u.enabled !== "boolean") throw notActed("invalid_params", "enabled must be true or false");
      if (u.autoDiscover != null && typeof u.autoDiscover !== "boolean") throw notActed("invalid_params", "autoDiscover must be true or false");
      const next = {
        autoDiscover: typeof u.autoDiscover === "boolean" ? u.autoDiscover : this.caps.autoDiscover,
        enabled: typeof u.enabled === "boolean" ? u.enabled : this.caps.enabled,
        maxAgeDays: typeof u.maxAgeDays === "number" ? u.maxAgeDays : this.caps.maxAgeDays,
        maxThreads: typeof u.maxThreads === "number" ? u.maxThreads : this.caps.maxThreads,
      };
      writeJsonAtomic(this.capsFile, { caps: next });
      this.caps = next;
      this.broadcast("discovery/updated", { caps: { ...this.caps } });
      // Raised or switched off: more comes in; lowered: the index is trimmed.
      void this.scanAll();
      return { caps: { ...this.caps } };
    });
  }

  /** A thread by id, bringing in one handed out by `thread/older` when it is used. */
  findThread(id: unknown): ThreadRuntime | undefined {
    if (typeof id !== "string") return undefined;
    const rt = this.threads.get(id);
    if (rt && !rt.deleted && (!rt.record.metadataPending || this.metadataFailed(rt))) {
      if (!this.index.get(rt.id) && rt.record.capped) this.bringIn(rt);
      return rt;
    }
    const memo = this.olderMemo.get(id);
    const inst = memo ? this.registry.get(memo.instance) : undefined;
    if (!memo || !inst) return undefined;
    const brought = this.discovered(inst, memo.info, true);
    this.bringIn(brought);
    return brought;
  }

  private bringIn(rt: ThreadRuntime): void {
    if (!rt.record.broughtIn) {
      rt.record.broughtIn = true;
      rt.saveRecord();
    }
    this.updateIndex(rt);
  }

  /**
   * `thread/lookup`: by YAAC thread id or the harness's own id. Misses in the index ask each
   * harness directly, and what is found is brought into the index.
   */
  async lookup(client: ClientLink, p: ThreadLookupParams): Promise<ThreadLookupResult> {
    const id = typeof p.id === "string" ? p.id.trim() : "";
    if (!id) throw rpc("invalid_params", "id is required");
    const out: ThreadRuntime[] = [];
    const byId = this.findThread(id);
    if (byId && (!p.instance || byId.record.instance === p.instance)) out.push(byId);
    const instances = this.registry.all().filter((i) => (!p.instance || i.config.id === p.instance) && i.provider && i.descriptor.status === "ready");
    for (const inst of instances) {
      const known = this.threadByNative(inst.config.id, id);
      if (known) {
        if (this.needsMetadata(known)) await this.recoverMetadata(inst, known, true);
        if (known.deleted || (known.record.metadataPending && !this.listed(known))) continue;
        if (!out.includes(known)) out.push(known);
        if (known.record.capped) this.bringIn(known);
        continue;
      }
      if (out.length > 0 || typeof inst.provider!.lookupThread !== "function") continue;
      let info: NativeThreadInfo | null = null;
      try {
        info = await inst.provider!.lookupThread(id);
      } catch (e) {
        this.opts.log(`lookup in ${inst.config.id} failed: ${(e as Error).message}`);
      }
      if (!info) continue;
      this.remember(inst.config.id, info.native);
      const rt = this.discovered(inst, info, true);
      this.bringIn(rt);
      out.push(rt);
    }
    return { threads: out.map((rt) => this.renderSummary(this.summaryOf(rt), this.holderOf(rt), client.key)) };
  }

  /**
   * `thread/older`: conversations not in the index (beyond the caps), newest first. They are
   * not indexed; using one's id (reading, subscribing, …) brings it in.
   */
  async older(client: ClientLink, p: ThreadOlderParams): Promise<ThreadOlderResult> {
    const limit = Math.max(1, Math.min(typeof p.limit === "number" ? p.limit : 20, 100));
    const instances = this.registry
      .all()
      .filter((i) => (!p.instance || i.config.id === p.instance) && i.provider && i.descriptor.status === "ready" && (i.descriptor.capabilities ?? []).includes("threadList"));
    let pos = { i: 0, c: null as string | null, s: 0 };
    if (p.cursor) {
      try {
        pos = { ...pos, ...(JSON.parse(Buffer.from(String(p.cursor), "base64url").toString("utf8")) as typeof pos) };
      } catch {
        throw rpc("invalid_params", "this cursor is not valid; ask for the first page again");
      }
    }
    const query = typeof p.query === "string" ? p.query.trim().toLowerCase() : "";
    const folder = typeof p.folder === "string" && p.folder ? p.folder : null;
    const out: ThreadSummary[] = [];
    let next: typeof pos | null = null;
    outer: for (let i = pos.i; i < instances.length; i++) {
      const inst = instances[i]!;
      let cursor: string | null = i === pos.i ? pos.c : null;
      let skip = i === pos.i ? pos.s : 0;
      for (let pages = 0; pages < 10000; pages++) {
        const page = await inst.provider!.scanThreads({ cursor, limit: 100 });
        const list = page.threads;
        for (let k = skip; k < list.length; k++) {
          const info = list[k]!;
          const known = this.threadByNative(inst.config.id, info.native);
          if (known && this.index.get(known.id)) continue;
          if (query && !(info.title ?? "").toLowerCase().includes(query) && !info.native.toLowerCase().startsWith(query)) continue;
          const rt = known ?? this.transientThread(inst, info);
          if (!known && !rt.record.repoResolved && fs.existsSync(info.cwd)) {
            rt.record.repo = await this.repoOf(info.cwd);
            rt.record.repoResolved = true;
          }
          const summary = this.summaryOf(rt);
          if (folder && summary.project?.path !== folder) continue;
          if (out.length >= limit) {
            next = { i, c: cursor, s: k };
            break outer;
          }
          out.push(this.renderSummary(summary, this.holderOf(rt), client.key));
          if (!known) this.rememberOlder(rt.id, inst.config.id, info);
        }
        skip = 0;
        if (!page.cursor) break;
        cursor = page.cursor;
      }
    }
    const result: ThreadOlderResult = { threads: out };
    if (next) result.cursor = Buffer.from(JSON.stringify(next), "utf8").toString("base64url");
    return result;
  }

  private rememberOlder(id: string, instance: string, info: NativeThreadInfo): void {
    this.olderMemo.delete(id);
    this.olderMemo.set(id, { instance, info });
    while (this.olderMemo.size > OLDER_MEMO_MAX) this.olderMemo.delete(this.olderMemo.keys().next().value!);
  }

  /** A summary-only runtime for a conversation that is not in the index (never saved). */
  private transientThread(instance: Instance, info: NativeThreadInfo): ThreadRuntime {
    const storeKey = instance.descriptor.store.key ?? null;
    const id = threadIdFor(instance.config.kind, storeKey, instance.config.id, info.native);
    const record: ThreadRecord = {
      id,
      instance: instance.config.id,
      storeKey,
      kind: instance.config.kind,
      native: info.native,
      cwd: info.cwd,
      originKind: info.originKind === "yaac" ? "programmatic" : info.originKind,
      title: info.title ?? null,
      preview: info.preview ?? null,
      model: info.model ?? null,
      archived: info.archived ?? false,
      createdAt: info.createdAt,
      updatedAt: info.updatedAt,
      capped: true,
    };
    const rt = new ThreadRuntime(record, path.join(this.threadsDir, threadDirName(id)));
    rt.deleted = true; // never saved, never journaled
    return rt;
  }

  /**
   * `thread/keep`: the threads the host keeps (pinned or muted there), never cut by the caps.
   * Replaces the previous set. A thread now kept that the caps had cut is listed again; one no
   * longer kept is trimmed at the next scan.
   */
  keep(p: { threadIds?: unknown }): Record<string, never> {
    const ids = Array.isArray(p.threadIds) ? p.threadIds.filter((x): x is string => typeof x === "string") : null;
    if (!ids) throw rpc("invalid_params", "threadIds must be a list of thread ids");
    const before = this.kept;
    this.kept = new Set(ids);
    writeJsonAtomic(this.keepFile, { threadIds: [...this.kept] });
    for (const id of new Set([...before, ...this.kept])) {
      const rt = this.threads.get(id);
      if (rt && !rt.deleted && before.has(id) !== this.kept.has(id)) this.updateIndex(rt);
    }
    return {};
  }

  // ── Threads ────────────────────────────────────────────────────────────────────────────

  thread(id: string): ThreadRuntime | undefined {
    return this.threads.get(id);
  }

  requireThread(id: unknown): ThreadRuntime {
    const rt = this.findThread(id);
    if (!rt || rt.deleted) throw rpc("thread_not_found", `no thread ${String(id)} on this target`);
    return rt;
  }

  allThreads(): ThreadRuntime[] {
    return [...this.threads.values()].filter((t) => !t.deleted);
  }

  /** The store an instance leads to now, when known. */
  private storeNow(instance: string): string | null {
    return this.registry.get(instance)?.descriptor.store.key ?? null;
  }

  private nativeKey(instance: string, native: string): string {
    return `${instance}\u0000${native}`;
  }

  private createThread(instance: Instance, native: NativeId, fields: Partial<ThreadRecord> & { cwd: string; originKind: ThreadRecord["originKind"] }, fresh: boolean, storeKey = instance.descriptor.store.key ?? null): ThreadRuntime {
    const id = threadIdFor(instance.config.kind, storeKey, instance.config.id, native);
    const existing = this.threads.get(id);
    if (existing && !existing.deleted) {
      if (existing.record.instance !== instance.config.id) {
        const also = new Set(existing.record.alsoVisibleTo ?? []);
        also.add(instance.config.id);
        existing.record.alsoVisibleTo = [...also];
        existing.saveRecord();
      }
      this.byNative.set(instance.config.id, existing.record.storeKey, native, id);
      return existing;
    }
    const now = nowIso();
    const record: ThreadRecord = {
      id,
      instance: instance.config.id,
      storeKey,
      kind: instance.config.kind,
      native,
      createdAt: now,
      updatedAt: now,
      ...fields,
    };
    const dir = path.join(this.threadsDir, threadDirName(id));
    ensureDir(dir);
    if (fresh) fs.rmSync(path.join(dir, "journal"), { recursive: true, force: true });
    const rt = new ThreadRuntime(record, dir);
    rt.saveRecord();
    rt.openJournal(this.opts.journal, fresh);
    this.threads.set(id, rt);
    this.byNative.set(instance.config.id, storeKey, native, id);
    void this.resolveProject(rt);
    return rt;
  }

  // ── Repositories, git status, projects ───────────────────────────────────────────────

  /** Where a folder sits in a repository: one git query per distinct folder, cached. */
  private repoOf(folder: string, urgent = false): Promise<RepoLocation | null> {
    let p = this.repoLookups.get(folder);
    if (!p) {
      p = this.gitLimiter.run(() => resolveRepo(this.git, folder).catch(() => null), urgent);
      this.repoLookups.set(folder, p);
      const cached = p;
      void cached.then((repo) => {
        if (repo) return;
        const timer = setTimeout(() => {
          if (this.repoLookups.get(folder) === cached) this.repoLookups.delete(folder);
        }, 60_000);
        timer.unref();
      });
    }
    return p;
  }

  /** Resolves a thread's repository location once; a vanished folder keeps what was known. */
  private async resolveProject(rt: ThreadRuntime, urgent = false): Promise<void> {
    if (rt.record.repoResolved && rt.record.repo) return;
    const cwd = rt.record.cwd;
    if (!fs.existsSync(cwd)) return;
    const repo = await this.repoOf(cwd, urgent);
    if (rt.deleted || rt.record.cwd !== cwd) return;
    if (!repo) return;
    rt.record.repo = repo;
    rt.record.repoResolved = true;
    rt.saveRecord();
    this.updateIndex(rt);
    // The first status of a checkout is computed once, in the background.
    if (repo && !this.gitStatus.has(repo.root)) void this.gitStatus.refresh(repo.root, urgent);
  }

  private onGitChanged(root: string): void {
    this.gitChanged.emit(root);
    for (const rt of this.threads.values()) if (!rt.deleted && (rt.record.repo?.root === root || rt.record.placement?.worktree === root)) this.updateIndex(rt);
  }

  /** The slow timer: only checkouts of threads some client is subscribed to, never the whole index. */
  refreshWatchedGit(): void {
    const roots = new Set<string>();
    for (const rt of this.threads.values()) {
      const root = rt.record.repo?.root;
      if (!root || rt.deleted) continue;
      for (const c of this.clients) if (c.isSubscribed?.(rt.id)) roots.add(root);
    }
    for (const root of roots) void this.gitStatus.refresh(root);
  }

  private async refreshProjectRepos(): Promise<void> {
    let changed = false;
    for (const p of this.projects.list()) {
      if (!fs.existsSync(p.folder)) continue;
      const repo = await this.repoOf(p.folder);
      if (this.projects.setRepo(p.id, repo)) changed = true;
    }
    if (changed) this.projectsChanged();
  }

  /** Every project changed: tell the clients, and move threads between projects. */
  private projectsChanged(): void {
    this.broadcast("project/updated", { projects: this.projects.list() });
    for (const rt of this.threads.values()) this.updateIndex(rt);
  }

  listProjects(): { projects: Project[] } {
    return { projects: this.projects.list() };
  }

  async projectCreate(p: { opId: string; folder: string; name: string }): Promise<{ project: Project }> {
    return this.ops.run("project/create", p.opId, async () => {
      if (typeof p.folder !== "string" || !path.isAbsolute(p.folder)) throw notActed("invalid_params", "folder must be an absolute path on this target");
      if (typeof p.name !== "string" || !p.name.trim()) throw notActed("invalid_params", "name is required");
      const folder = path.resolve(p.folder);
      let isDir = false;
      try {
        isDir = fs.statSync(folder).isDirectory();
      } catch {
        isDir = false;
      }
      if (!isDir) throw notActed("invalid_params", `${folder} is not a folder on this target`);
      const repo = await this.repoOf(folder, true);
      let key: string | null = null;
      if (repo) {
        const origin = await this.git(folder, ["config", "--get", "remote.origin.url"]);
        if (origin.ok && origin.out.trim()) key = normaliseRemote(origin.out.trim());
      }
      const project = this.projects.create(folder, p.name.trim(), repo, key);
      this.projectsChanged();
      return { project };
    });
  }

  async projectRename(p: { opId: string; projectId: string; name: string }): Promise<{ project: Project }> {
    return this.ops.run("project/rename", p.opId, async () => {
      if (typeof p.name !== "string" || !p.name.trim()) throw notActed("invalid_params", "name is required");
      if (!this.projects.get(p.projectId)) throw notActed("invalid_params", `no project ${String(p.projectId)} on this target`);
      const project = this.projects.rename(p.projectId, p.name.trim());
      this.projectsChanged();
      return { project };
    });
  }

  async projectDelete(p: { opId: string; projectId: string }): Promise<void> {
    await this.ops.run("project/delete", p.opId, async () => {
      if (!this.projects.get(p.projectId)) throw notActed("invalid_params", `no project ${String(p.projectId)} on this target`);
      this.projects.delete(p.projectId);
      // Threads moved into it (D144) go back to where their folders put them.
      for (const rt of this.threads.values()) {
        if (rt.record.placement?.projectId !== p.projectId) continue;
        delete rt.record.placement;
        rt.saveRecord();
      }
      this.projectsChanged();
      return {};
    });
  }

  /** `thread/update`: YAAC's own flags of a thread. Stored on the target; pushed as `thread/updated`. */
  async threadUpdate(client: ClientLink, p: ThreadUpdateParams): Promise<ThreadUpdateResult> {
    return this.ops.run("thread/update", p.opId, async () => {
      const rt = this.findThread(p.threadId);
      if (!rt || rt.deleted) throw notActed("thread_not_found", `no thread ${String(p.threadId)} on this target`);
      if (p.agents !== undefined && p.agents !== null && typeof p.agents !== "string") throw notActed("invalid_params", "agents must be a string");
      if (p.held !== undefined && p.held !== null && typeof p.held !== "boolean") throw notActed("invalid_params", "held must be true or false");
      if (p.held === false && rt.record.hold) this.releaseHold(rt);
      if (p.held === true && !rt.record.hold) this.setHold(rt, { label: "the user", at: nowIso() });
      if (typeof p.agents === "string") rt.record.agents = p.agents;
      rt.saveRecord();
      this.updateIndex(rt);
      return { thread: this.renderSummary(this.summaryOf(rt), this.holderOf(rt), client.key) };
    });
  }

  /**
   * `thread/move` (D144): where YAAC lists a thread — a project on this target and, when it is
   * in git, one checkout of its repository — and, under the provider's
   * `threadMoveWorkingDirectory`, the folder the agent works in from its next turn: the
   * project's folder, or the same place in the chosen checkout. Without the capability only
   * the placement changes. A running thread may be moved; its turn goes on where it is.
   */
  async threadMove(client: ClientLink, p: ThreadMoveParams): Promise<ThreadMoveResult> {
    return this.ops.run("thread/move", p.opId, async () => {
      const rt = this.findThread(p.threadId);
      if (!rt || rt.deleted) throw notActed("thread_not_found", `no thread ${String(p.threadId)} on this target`);
      if (rt.record.archived) throw notActed("invalid_params", "an archived thread cannot be moved; unarchive it first");
      if (p.project !== undefined && p.project !== null && typeof p.project !== "string") throw notActed("invalid_params", "project must be a project id");
      if (p.worktree !== undefined && p.worktree !== null && (typeof p.worktree !== "string" || !path.isAbsolute(p.worktree))) {
        throw notActed("invalid_params", "worktree must be an absolute path on this target");
      }
      await this.refreshExternal(rt);
      this.checkWriter(rt, client, true);
      if (p.project === undefined || p.project === null) {
        if (p.worktree) throw notActed("invalid_params", "a worktree needs a project");
        if (rt.record.placement) {
          delete rt.record.placement;
          rt.saveRecord();
          this.updateIndex(rt);
          const back = this.summaryOf(rt).project?.displayName ?? rt.record.cwd;
          this.noticeItem(rt, "moved", `Moved back to ${back} in YAAC · working directory unchanged`);
        }
        return { thread: this.renderSummary(this.summaryOf(rt), this.holderOf(rt), client.key) };
      }
      const project = this.projects.get(p.project);
      if (!project) throw notActed("invalid_params", `no project ${String(p.project)} on this target`);
      // The checkout: the one asked for, else the project's own (when it is in git).
      const placement: PlacementRecord = { projectId: project.id };
      let cwd = project.folder;
      if (p.worktree) {
        if (!project.repo) throw notActed("invalid_params", `${project.name} is not in a git repository, so it has no worktrees`);
        const loc = await this.repoOf(path.resolve(p.worktree), true);
        if (!loc || loc.repo !== project.repo.repo) throw notActed("invalid_params", `${p.worktree} is not a checkout of ${project.name}'s repository on this target`);
        placement.worktree = loc.root;
        if (loc.worktreeName) placement.worktreeName = loc.worktreeName;
        cwd = project.repo.relative === "." ? loc.root : path.join(loc.root, ...project.repo.relative.split("/"));
      } else if (project.repo) {
        placement.worktree = project.repo.root;
        if (project.repo.worktreeName) placement.worktreeName = project.repo.worktreeName;
      }
      if (placement.worktree && !placement.worktreeName) {
        const head = await this.git(placement.worktree, ["rev-parse", "--abbrev-ref", "HEAD"]);
        const branch = head.ok ? head.out.trim() : "";
        if (branch && branch !== "HEAD") placement.branch = branch;
        if (!this.gitStatus.has(placement.worktree)) void this.gitStatus.refresh(placement.worktree, true);
      }
      const eff = await this.computeEffective(rt);
      const moveCwd = (eff.capabilities ?? []).includes("threadMoveWorkingDirectory");
      let workingDirectory: string | null = null;
      if (moveCwd && path.resolve(cwd) !== path.resolve(rt.record.cwd)) {
        let isDir = false;
        try {
          isDir = fs.statSync(cwd).isDirectory();
        } catch {
          isDir = false;
        }
        if (!isDir) throw notActed("invalid_params", `the folder ${cwd} does not exist on this target`);
        // Two moves before a turn: the note still names where the agent last worked.
        const from = rt.record.cwdNote?.from ?? rt.record.cwd;
        rt.record.cwd = cwd;
        rt.record.cwdMoved = true;
        if (path.resolve(from) === path.resolve(cwd)) delete rt.record.cwdNote;
        else rt.record.cwdNote = { from, to: cwd };
        rt.record.repo = null;
        rt.record.repoResolved = false;
        workingDirectory = cwd;
      }
      rt.record.placement = placement;
      rt.saveRecord();
      if (workingDirectory) await this.resolveProject(rt, true);
      this.updateIndex(rt);
      const where = placement.worktree && p.worktree ? `${project.name} · ${placement.worktreeName ?? placement.branch ?? placement.worktree}` : project.name;
      const running = rt.activeTurn ? " (from its next turn; the running turn goes on where it is)" : "";
      this.noticeItem(
        rt,
        "moved",
        workingDirectory ? `Moved to ${where} · working directory now ${workingDirectory}${running}` : `Moved to ${where} in YAAC · working directory unchanged`,
      );
      // The agent works in a project's folder from its next turn: that folder, like a new thread's.
      const inst = this.registry.get(rt.record.instance);
      if (workingDirectory && inst) this.trustNotice(rt, await this.trustProjectFolder(inst, workingDirectory));
      const thread = this.renderSummary(this.summaryOf(rt), this.holderOf(rt), client.key);
      return workingDirectory ? { thread, workingDirectory } : { thread };
    });
  }

  private discovered(instance: Instance, info: NativeThreadInfo, capped?: boolean): ThreadRuntime {
    const known = this.byNative.get(instance.config.id, info.native, this.storeNow(instance.config.id));
    let rt = known ? this.threads.get(known) : undefined;
    const metadataWasPending = !!rt?.record.metadataPending;
    const parentWasUnknown = !rt || rt.deleted || metadataWasPending;
    // Seen outside a scan (a harness event): only the age cap can be checked here.
    const ageCap = this.caps.enabled && this.caps.maxAgeDays > 0 ? new Date(Date.now() - this.caps.maxAgeDays * 86_400_000).toISOString() : null;
    const isCapped = capped ?? (!!ageCap && info.updatedAt < ageCap);
    if (!rt || rt.deleted) {
      rt = this.createThread(
        instance,
        info.native,
        {
          cwd: info.cwd,
          originKind: info.originKind === "yaac" ? "programmatic" : info.originKind,
          title: info.title ?? null,
          preview: info.preview ?? null,
          model: info.model ?? null,
          archived: info.archived ?? false,
          createdAt: info.createdAt,
          updatedAt: info.updatedAt,
          ...(instance.config.kind === "codex" ? { metadataPending: false } : {}),
          ...(isCapped ? { capped: true } : {}),
        },
        false,
      );
    } else {
      // Keep YAAC's working directory for threads it started or explicitly moved.
      const r = rt.record;
      if (this.needsMetadata(rt)) {
        r.metadataPending = false;
        r.createdAt = info.createdAt;
        if (metadataWasPending) r.capped = isCapped;
        if (r.originKind !== "yaac") r.originKind = info.originKind === "yaac" ? "programmatic" : info.originKind;
      }
      if (r.originKind !== "yaac" && info.cwd && !r.cwdMoved && !r.placement && info.cwd !== r.cwd) {
        r.cwd = info.cwd;
        r.repo = null;
        r.repoResolved = false;
      }
      if (!r.repoResolved) void this.resolveProject(rt);
      r.title = info.title ?? r.title ?? null;
      r.preview = info.preview ?? r.preview ?? null;
      r.model = info.model ?? r.model ?? null;
      r.archived = info.archived ?? r.archived ?? false;
      if (info.updatedAt > r.updatedAt) r.updatedAt = info.updatedAt;
      rt.saveRecord();
    }
    // A harness sub-agent (D151): filed under its parent, which now counts it.
    if (info.parent && info.subagent) {
      const r = rt.record;
      const changed = r.nativeParent !== info.parent || canonical(r.subagent ?? null) !== canonical(info.subagent);
      if (changed) {
        r.nativeParent = info.parent;
        r.subagent = { ...info.subagent };
        if (r.originKind !== "yaac") r.originKind = info.originKind === "yaac" ? "programmatic" : info.originKind;
        rt.saveRecord();
        this.linkSubagent(rt);
      }
    }
    if (metadataWasPending) this.appendControl(rt);
    this.updateIndex(rt);
    const parent = this.subagentParent(rt);
    if (parent) this.updateIndex(parent);
    // Sub-agents filed before their parent was known name it now.
    if (parentWasUnknown) for (const kid of this.subagentKids.get(this.nativeKey(instance.config.id, info.native)) ?? []) {
      const k = this.threads.get(kid);
      if (k && !k.deleted) this.updateIndex(k);
    }
    return rt;
  }

  private commandController(rt: ThreadRuntime): ThreadRuntime | undefined {
    if (rt.commandController) {
      const id = this.byNative.get(rt.record.instance, rt.commandController, this.storeNow(rt.record.instance));
      return id ? this.threads.get(id) : undefined;
    }
    return this.subagentReadOnly(rt) ? this.subagentParent(rt) : rt;
  }

  /**
   * What a thread is doing (`ThreadSummary.state`). A thread with no turn of its own whose
   * harness sub-agents still work (D151) — an orchestrator that ended its turn while its
   * background agents run — is waiting on them, as on mesh workers (D188), until they end
   * and the harness resumes it.
   */
  private stateOf(rt: ThreadRuntime, depth = 0): ThreadSummary["state"] {
    const r = rt.record;
    if (rt.pendingRequestCount() > 0) return "needsAttention";
    if (r.hold) return "stopped";
    if (rt.waiting.size > 0) return "waiting";
    if (rt.activeTurn) return "running";
    if (this.subagentsWorking(rt, depth)) return "waiting";
    if (rt.lastStatus === "failed") return "failed";
    return (r.subagent ? subagentState(r.subagent.status) : null) ??
      (rt.lastStatus === "completed" && (r.spawnedBy || r.subagent) ? "done" : "idle");
  }

  /** Whether any of a thread's harness sub-agents is running, waiting, or needs the user. */
  private subagentsWorking(rt: ThreadRuntime, depth: number): boolean {
    if (depth > 8 || !rt.record.native) return false;
    const kids = this.subagentKids.get(this.nativeKey(rt.record.instance, rt.record.native));
    if (!kids) return false;
    for (const id of kids) {
      const k = this.threads.get(id);
      if (!k || k.deleted || k === rt) continue;
      const st = this.stateOf(k, depth + 1);
      if (st === "running" || st === "waiting" || st === "needsAttention") return true;
    }
    return false;
  }

  /** The sidebar summary of a thread, with `control: "free"` (applied per client). */
  summaryOf(rt: ThreadRuntime): ThreadSummary {
    const r = rt.record;
    const state = this.stateOf(rt);
    const s: ThreadSummary = {
      id: r.id,
      instance: r.instance,
      nativeId: r.native,
      title: r.titleOverride ?? r.title ?? "Untitled",
      preview: r.preview ?? "",
      cwd: r.cwd,
      state,
      control: "free",
      originKind: r.originKind,
      createdAt: r.createdAt,
      updatedAt: r.updatedAt,
      archived: r.archived ?? false,
      agents: r.agents ?? "ask",
    };
    const away = this.storeAway(rt) ?? (r.metadataPending ? METADATA_PENDING : null);
    if (away) s.unavailable = away;
    else if (rt.runningCommands.length) {
      const controller = this.commandController(rt);
      s.runningCommands = rt.runningCommands.map((c) => ({
        ...c, ...(controller && controller !== rt && { controller: controller.id }), canStop: c.canStop && !!controller,
      }));
    }
    if (r.hold) s.hold = r.hold;
    if (r.fast) s.fast = true;
    if (r.ultrafast) s.ultrafast = true;
    if (r.model) s.model = r.model;
    if (r.forkedFrom) s.forkedFrom = r.forkedFrom;
    if (r.linkedChildren?.length) s.linkedChildren = r.linkedChildren;
    if (r.spawnedBy) s.spawnedBy = r.spawnedBy;
    this.mesh?.decorate(rt, s);
    // A harness's own sub-agent (D151) is listed under its parent like a worker; a parent
    // counts its sub-agents among its children.
    if (r.subagent) {
      s.subagent = { ...r.subagent };
      const parent = this.subagentParent(rt);
      if (parent && !s.spawnedBy) s.spawnedBy = this.addressHere(parent.id);
    }
    const subagents = this.subagentCount(rt);
    if (subagents > 0) s.children = (s.children ?? 0) + subagents;
    // Where the user moved it (D144) wins over the folder rule, while its project exists.
    const placed = r.placement ? this.projects.get(r.placement.projectId) : undefined;
    s.project = placed ? userProjectRef(placed) : projectRefFor(this.projects.list(), { folder: r.cwd, repo: r.repo ?? null });
    if (placed && r.placement) {
      const pl: ThreadPlacement = { project: placed.id };
      const root = r.placement.worktree;
      if (root) {
        const branch = this.gitStatus.get(root)?.branch ?? r.placement.branch ?? null;
        pl.worktree = root;
        const name = r.placement.worktreeName ?? branch;
        if (name) pl.worktreeName = name;
        s.worktree = { path: root, ...(branch ? { branch } : {}) };
      }
      s.placement = pl;
    }
    if (r.repo) {
      s.repo = r.repo;
      const git = this.gitStatus.get(r.repo.root);
      if (git) s.git = git;
      if (!s.placement?.worktree) s.worktree = { path: r.repo.root, ...(git?.branch ? { branch: git.branch } : {}) };
      if (git?.branch) s.branch = git.branch;
    }
    return s;
  }

  /**
   * Why the thread cannot be used while its instance leads to another store than the one it
   * is bound to (P10, P300), or null. Unknown keys on either side are not evidence.
   */
  storeAway(rt: ThreadRuntime): string | null {
    const bound = rt.record.storeKey;
    const inst = this.registry.get(rt.record.instance);
    const now = inst?.descriptor.store.key ?? null;
    if (!bound || !now || bound === now) return null;
    return `${inst!.descriptor.displayName} now uses another store${inst!.descriptor.store.home ? ` (${inst!.descriptor.store.home})` : ""}. This thread belongs to the one it used before; change the instance's harness home back in Settings → Providers to use it.`;
  }

  /** Refuses to drive a thread of another store than its instance's now (P10, P300). */
  private checkStore(rt: ThreadRuntime): void {
    const away = this.storeAway(rt);
    if (away) throw rpc("provider_unavailable", away);
  }

  private clearInstanceCommands(id: string): void {
    for (const rt of this.allThreads()) {
      if (rt.record.instance !== id || !rt.runningCommands.length) continue;
      rt.runningCommands = [];
      rt.commandController = undefined;
      this.updateIndex(rt);
    }
    this.activity.emit();
  }

  /** After an instance's configuration changed: its threads' availability, then its listing. */
  async instanceChanged(id: string): Promise<void> {
    const inst = this.registry.get(id);
    if (!inst) return;
    for (const rt of this.allThreads()) if (rt.record.instance === id) this.updateIndex(rt);
    await this.scanInstance(inst);
  }

  holderOf(rt: ThreadRuntime): Holder {
    // Read-only ownership requires positive evidence of an external writer.
    const ext = rt.external.external ? rt.external.holder ?? "another program" : null;
    const driver = rt.driver ?? rt.compactDriver;
    return { driver: driver?.key ?? null, driverHost: driver?.host ?? null, external: ext };
  }

  renderSummary(summary: ThreadSummary, holder: Holder, clientKey: string): ThreadSummary {
    const c = controlFor(holder, clientKey);
    const out: ThreadSummary = { ...summary, control: c.control };
    if (c.controlHolder) out.controlHolder = c.controlHolder;
    else delete out.controlHolder;
    return out;
  }

  updateIndex(rt: ThreadRuntime): void {
    if (rt.deleted) return;
    // Active again: it is among the most recent now.
    if (rt.record.capped && rt.activeTurn) {
      rt.record.capped = false;
      rt.saveRecord();
    }
    if (!this.listed(rt)) {
      if (this.index.remove(rt.id)) {
        this.broadcast("thread/removed", { threadId: rt.id, token: this.index.token });
        // Its parent counts one sub-agent fewer (D151).
        const parent = this.subagentParent(rt);
        if (parent) this.updateIndex(parent);
      }
      return;
    }
    const summary = this.summaryOf(rt);
    const holder = this.holderOf(rt);
    if (!this.index.upsert(summary, holder)) return;
    const token = this.index.token;
    for (const c of this.clients) c.notify("thread/updated", { thread: this.renderSummary(summary, holder, c.key), token });
    this.mesh?.stateChanged(rt);
    // A sub-agent's state decides whether its parent waits on it (D188).
    if (rt.record.subagent) {
      const parent = this.subagentParent(rt);
      if (parent && parent !== rt) this.updateIndex(parent);
    }
  }

  broadcast(method: string, params: unknown): void {
    for (const c of this.clients) c.notify(method, params);
  }

  busy(): { runningTurns: number; pendingRequests: number; runningCommands: number } {
    let runningTurns = 0;
    let pendingRequests = 0;
    let runningCommands = 0;
    for (const rt of this.threads.values()) {
      if (rt.deleted) continue;
      if (rt.activeTurn || rt.compactDriver) runningTurns++;
      pendingRequests += rt.pendingRequestCount();
      runningCommands += rt.runningCommands.length;
    }
    return { runningTurns, pendingRequests, runningCommands };
  }

  // ── Journal ────────────────────────────────────────────────────────────────────────────

  private journalOf(rt: ThreadRuntime) {
    return rt.openJournal(this.opts.journal, false);
  }

  private append(rt: ThreadRuntime, event: ThreadEvent, meta?: JournalEntry["meta"], durable = false): JournalEntry {
    if (event.type !== "itemDelta") this.flushDeltas(rt);
    const entry = this.journalOf(rt).append(event, meta, durable);
    rt.projection?.apply(event);
    for (const c of this.clients) c.wake(rt.id);
    return entry;
  }

  private appendControl(rt: ThreadRuntime): void {
    const ev: ControlChangedEvent & { type: "controlChanged" } = { type: "controlChanged", control: "free" };
    if (rt.activeTurn) ev.activeTurn = rt.activeTurn;
    const driver = rt.driver ?? rt.compactDriver;
    const external = this.holderOf(rt).external;
    const holder = external ?? driver?.host ?? null;
    if (holder) ev.holder = holder;
    const blocked = rt.record.metadataPending ? METADATA_PENDING : rt.external.takeOverBlocked;
    if (external && blocked) ev.takeOverBlocked = blocked;
    this.append(rt, ev, { driver: driver?.key ?? null, external: !!external });
  }

  /** Renders a journal entry's event for one client (control is relative to the receiver). */
  renderEvent(entry: JournalEntry, clientKey: string): ThreadEvent {
    if (entry.event.type !== "controlChanged" || !entry.meta) return entry.event;
    const c = controlFor({ driver: entry.meta.driver ?? null, external: entry.meta.external ? entry.event.holder ?? "another program" : null }, clientKey);
    const ev: ThreadEvent = { ...entry.event, control: c.control };
    return ev;
  }

  /** Coalesces a delta; sequence numbers are assigned only when it is flushed. */
  private delta(rt: ThreadRuntime, itemId: string, field: "text" | "reasoning" | "output", delta: string): void {
    if (delta.length === 0) return;
    const key = `${itemId}\u0000${field}`;
    const prev = rt.pendingDeltas.get(key);
    if (prev) prev.delta += delta;
    else rt.pendingDeltas.set(key, { itemId, field, delta });
    rt.pendingDeltaBytes += delta.length;
    if (rt.pendingDeltaBytes >= this.opts.coalesceBytes) this.flushDeltas(rt);
    else if (!rt.flushTimer) rt.flushTimer = setTimeout(() => this.flushDeltas(rt), this.opts.coalesceMs);
  }

  flushDeltas(rt: ThreadRuntime): void {
    if (rt.flushTimer) {
      clearTimeout(rt.flushTimer);
      rt.flushTimer = null;
    }
    if (rt.pendingDeltas.size === 0) return;
    const pending = [...rt.pendingDeltas.values()];
    rt.pendingDeltas.clear();
    rt.pendingDeltaBytes = 0;
    const journal = this.journalOf(rt);
    for (const d of pending) {
      const ev: ThreadEvent = { type: "itemDelta", itemId: d.itemId, field: d.field, delta: d.delta };
      journal.append(ev);
      rt.projection?.apply(ev);
    }
    for (const c of this.clients) c.wake(rt.id);
  }

  // ── Projection and snapshots ───────────────────────────────────────────────────────────

  /** Items of a thread: the harness's stored history overlaid with the journal. */
  async ensureProjection(rt: ThreadRuntime): Promise<Projection> {
    if (rt.projection) return rt.projection;
    if (!rt.projectionLoading) {
      rt.projectionLoading = (async () => {
        const journal = this.journalOf(rt);
        let base: Item[] = [];
        if (!journal.complete) {
          const inst = this.registry.get(rt.record.instance);
          const provider = inst?.provider;
          // Not from another store than the thread's (P10, P300): what the journal has is shown.
          if (provider && (inst.descriptor.capabilities ?? []).includes("threadRead") && !this.storeAway(rt)) {
            let cursor: string | null | undefined = null;
            try {
              for (let i = 0; i < 100000; i++) {
                const page = await provider.readThread(rt.record.native, { cursor });
                base.push(...page.items);
                cursor = page.cursor;
                if (!cursor) break;
              }
            } catch (e) {
              this.opts.log(`reading history of ${rt.id} failed: ${(e as Error).message}`);
            }
          }
        }
        this.flushDeltas(rt);
        const p = new Projection();
        const inJournal = new Set<string>();
        for (const e of journal.entries()) {
          const ev = e.event as unknown as { item?: Item; itemId?: string };
          if (ev.item?.id) inJournal.add(ev.item.id);
        }
        for (const item of base) if (!inJournal.has(item.id)) p.upsert(this.boundItem(rt, clone(item), false));
        for (const e of journal.entries()) p.apply(e.event);
        // Requests: yaacd's own records are the authority on their state.
        for (const r of rt.requests.values()) {
          const item = p.get(r.itemId) as (Item & { state?: string; resolution?: unknown }) | undefined;
          if (item && item.kind === "request") {
            item.state = r.state;
            if (r.resolution) item.resolution = r.resolution;
          }
        }
        rt.projection = p;
        return p;
      })().finally(() => {
        rt.projectionLoading = null;
      });
    }
    return rt.projectionLoading;
  }

  /** `thread/read`: newest first, bounded by bytes, from a snapshot bound to its position. */
  async read(client: ClientLink, p: ThreadReadParams): Promise<ThreadReadResult> {
    const rt = this.requireThread(p.threadId);
    const limit = Math.max(1024, Math.min(p.maxBytes ?? PAGE_MAX_BYTES, PAGE_MAX_BYTES));
    const maxItems = typeof p.maxItems === "number" && Number.isFinite(p.maxItems) ? Math.max(1, Math.min(200, Math.floor(p.maxItems))) : Infinity;
    const cursor = p.cursor ? String(p.cursor) : null;
    const newerPrefix = "history-newer:";
    const olderPrefix = "history:";
    const forward = p.around != null || cursor?.startsWith(newerPrefix);
    if (forward || cursor?.startsWith(olderPrefix) || (!cursor && maxItems !== Infinity)) {
      const projection = await this.ensureProjection(rt);
      this.flushDeltas(rt);
      const journal = this.journalOf(rt);
      const position = { epoch: journal.epoch, seq: journal.headSeq };
      const anchor = p.around ?? cursor?.slice(forward ? newerPrefix.length : olderPrefix.length);
      const offset = anchor == null ? projection.length : projection.offset(anchor);
      if (offset === undefined) {
        throw rpc("invalid_params", "this message is no longer in the thread");
      }
      const start = p.wholeTurns && p.around != null
        ? projection.turnStart(offset)
        : offset + (forward && p.around == null ? 1 : 0);
      const source = forward ? projection.historyFrom(start) : projection.historyBefore(anchor ?? undefined);
      const items: Item[] = [];
      let bytes = 0;
      let continuesTurn = false;
      for (const item of source) {
        const sameTurn = p.wholeTurns && items.at(-1)?.turnId === item.turnId;
        if (items.length >= maxItems && !sameTurn) break;
        const size = Buffer.byteLength(canonical(item));
        if (items.length && bytes + size > limit) {
          continuesTurn = sameTurn === true;
          break;
        }
        items.push(item);
        bytes += size;
      }
      if (forward) items.reverse();
      const result: ThreadReadResult = { items, position, ...(continuesTurn ? { continuesTurn } : {}) };
      const first = forward ? start : start - items.length;
      const end = forward ? start + items.length : start;
      if (first > 0 && items.length) result.cursor = `${olderPrefix}${items.at(-1)!.id}`;
      if (end < projection.length && items.length) result.newerCursor = `${newerPrefix}${items[0]!.id}`;
      if (!cursor) {
        if (rt.context) result.context = rt.context;
        if (p.around == null) result.prompts = projection.timeline(!!rt.record.spawnedBy);
        const included = new Set(items.map((item) => item.id));
        const required = [...rt.requests.values()]
          .filter((r) => (r.state === "pending" || r.state === "submitted") && !included.has(r.itemId))
          .flatMap((r) => projection.get(r.itemId) ?? []);
        if (required.length) result.requiredItems = required;
        client.onSnapshot(rt.id, position);
      }
      const uploads = this.uploadPresence(items);
      if (uploads.length) result.uploads = uploads;
      return result;
    }
    let snap: Snapshot;
    let offset = 0;
    this.expireSnapshots();
    if (p.cursor) {
      const [id, off] = String(p.cursor).split(".");
      const found = id ? this.snapshots.get(id) : undefined;
      if (!found || found.threadId !== rt.id) {
        throw rpc("invalid_params", "this page cursor has expired; read the thread again from the first page");
      }
      snap = found;
      offset = Number(off) || 0;
    } else {
      const projection = await this.ensureProjection(rt);
      // Synchronous from here: the snapshot and its position belong together.
      this.flushDeltas(rt);
      const journal = this.journalOf(rt);
      snap = {
        id: randomId("s", 8),
        threadId: rt.id,
        items: projection.snapshot(),
        position: { epoch: journal.epoch, seq: journal.headSeq },
        at: Date.now(),
      };
      this.snapshots.set(snap.id, snap);
      client.onSnapshot(rt.id, snap.position);
    }
    const items: Item[] = [];
    let bytes = 0;
    let i = snap.items.length - 1 - offset;
    for (; i >= 0; i--) {
      const item = snap.items[i]!;
      const size = Buffer.byteLength(canonical(item));
      if (items.length > 0 && bytes + size > limit) break;
      items.push(item);
      bytes += size;
    }
    const nextOffset = offset + items.length;
    const result: ThreadReadResult = { items, position: snap.position };
    if (!p.cursor && rt.context) result.context = rt.context;
    if (nextOffset < snap.items.length) result.cursor = `${snap.id}.${nextOffset}`;
    const uploads = this.uploadPresence(items);
    if (uploads.length > 0) result.uploads = uploads;
    return result;
  }

  private expireSnapshots(): void {
    const now = Date.now();
    for (const [k, s] of this.snapshots) if (now - s.at > SNAPSHOT_TTL_MS) this.snapshots.delete(k);
    while (this.snapshots.size > SNAPSHOT_MAX) this.snapshots.delete(this.snapshots.keys().next().value!);
  }

  private uploadPresence(items: Item[]): UploadPresence[] {
    const ids = new Set<string>();
    for (const item of items) {
      if (item.kind === "toolCall") {
        for (const image of item.images ?? []) ids.add(image.uploadId);
      }
      if (item.kind !== "userMessage") continue;
      for (const part of item.parts ?? []) {
        const u = (part as { uploadId?: unknown }).uploadId;
        if (typeof u === "string") ids.add(u);
      }
    }
    return [...ids].map((uploadId) => ({ uploadId, present: this.uploads.present(uploadId) }));
  }

  /**
   * `item/read`: a byte range of one item's full output — from the file kept for a truncated
   * output, else from the item itself. A range never splits a UTF-8 character: its start moves
   * forward and its end back to a character boundary.
   */
  async itemRead(p: { threadId: string; itemId: string; offset?: number | null; maxBytes: number }): Promise<ItemReadResult> {
    const rt = this.requireThread(p.threadId);
    if (typeof p.itemId !== "string" || !p.itemId) throw rpc("invalid_params", "itemId is required");
    if (typeof p.maxBytes !== "number" || p.maxBytes < 0) throw rpc("invalid_params", "maxBytes is required");
    let whole: Buffer | null = null;
    const file = this.outputFile(rt, p.itemId);
    let total = 0;
    if (fs.existsSync(file)) {
      total = fs.statSync(file).size;
    } else {
      const projection = await this.ensureProjection(rt);
      const item = projection.get(p.itemId) as Item | undefined;
      if (!item || item.kind !== "toolCall") return { present: false };
      whole = Buffer.from(item.output ?? "", "utf8");
      total = whole.length;
    }
    const readAt = (start: number, n: number): Buffer => {
      if (whole) return whole.subarray(start, start + n);
      const buf = Buffer.alloc(n);
      const fd = fs.openSync(file, "r");
      try {
        const got = fs.readSync(fd, buf, 0, n, start);
        return buf.subarray(0, got);
      } finally {
        fs.closeSync(fd);
      }
    };
    let start = Math.max(0, Math.min(Math.floor(p.offset ?? 0), total));
    // Move a start inside a character forward to the next character (at most 3 bytes).
    const lead = readAt(start, Math.min(4, total - start));
    let skip = 0;
    while (skip < lead.length && (lead[skip]! & 0xc0) === 0x80) skip++;
    start += skip;
    const n = Math.max(0, Math.min(Math.floor(p.maxBytes), ITEM_READ_MAX, total - start));
    let buf = readAt(start, n);
    if (start + buf.length < total) {
      // Cut back to the start of the last character if it is incomplete.
      let i = buf.length - 1;
      while (i > 0 && (buf[i]! & 0xc0) === 0x80 && buf.length - i < 4) i--;
      const leadByte = buf[i] ?? 0;
      const need = leadByte >= 0xf0 ? 4 : leadByte >= 0xe0 ? 3 : leadByte >= 0xc0 ? 2 : 1;
      let end = i >= 0 && i + need > buf.length ? i : buf.length;
      // A range smaller than one character still makes progress: it returns that character.
      if (end === 0 && buf.length > 0) {
        buf = readAt(start, Math.min(need, total - start));
        end = buf.length;
      }
      buf = buf.subarray(0, Math.max(0, end));
    }
    const next = start + buf.length;
    return { present: true, data: buf.toString("utf8"), offset: start, nextOffset: next, totalBytes: total, more: next < total };
  }

  // ── Subscriptions ──────────────────────────────────────────────────────────────────────

  async subscribe(client: ClientLink, p: { threadId: string; epoch?: number | null; afterSeq?: number | null }, register: (cursor: number, mode: "replay" | "reset") => void): Promise<ThreadSubscribeResult> {
    const rt = this.requireThread(p.threadId);
    await this.refreshExternal(rt);
    // A harness sub-agent (D151) YAAC is not streaming is read again from the harness when it
    // is opened, as its parent may have moved it on since (a session in a terminal).
    if (rt.record.subagent && !rt.activeTurn && !rt.projectionLoading) rt.projection = null;
    const effective = await this.computeEffective(rt);
    this.flushDeltas(rt);
    const journal = this.journalOf(rt);
    journal.touch(client.key);
    const holder = this.holderOf(rt);
    const c = controlFor(holder, client.key);
    const blocked = rt.record.metadataPending ? METADATA_PENDING : rt.external.takeOverBlocked;
    const common = {
      control: c.control,
      ...(c.controlHolder ? { controlHolder: c.controlHolder } : {}),
      ...(c.control === "external" && blocked ? { takeOverBlocked: blocked } : {}),
      ...(rt.activeTurn ? { activeTurn: rt.activeTurn } : {}),
      effective,
      ...(rt.context ? { context: rt.context } : {}),
      ...(rt.tokens ? { tokens: rt.tokens } : {}),
      ...(rt.userQueue.length > 0 ? { queued: this.queuedOf(rt) } : {}),
      ...(rt.record.hold ? { hold: rt.record.hold } : {}),
    };
    // Someone is watching this thread: make sure its checkout's git status is known.
    if (!rt.record.repoResolved || !rt.record.repo) void this.resolveProject(rt, true);
    else if (rt.record.repo && !this.gitStatus.has(rt.record.repo.root)) void this.gitStatus.refresh(rt.record.repo.root, true);
    if (typeof p.epoch === "number" && typeof p.afterSeq === "number" && journal.canReplay(p.epoch, p.afterSeq)) {
      register(p.afterSeq, "replay");
      return { ...common, mode: "replay", epoch: journal.epoch, fromSeq: p.afterSeq + 1, headSeq: journal.headSeq };
    }
    register(journal.headSeq, "reset");
    return { ...common, mode: "reset", epoch: journal.epoch, headSeq: journal.headSeq };
  }

  /** Journal entries after `seq`, starting at index `from` (see Journal.indexAfter). */
  entriesAfter(threadId: string, seq: number): { entries: readonly JournalEntry[]; start: number; epoch: number; headSeq: number } {
    const rt = this.requireThread(threadId);
    const j = this.journalOf(rt);
    return { entries: j.entries(), start: j.indexAfter(seq), epoch: j.epoch, headSeq: j.headSeq };
  }

  itemStateAt(threadId: string, itemId: string, seq: number): Item | null {
    return this.journalOf(this.requireThread(threadId)).itemStateAt(itemId, seq);
  }

  ack(client: ClientLink, threadId: string, epoch: number, seq: number): void {
    const rt = this.requireThread(threadId);
    this.journalOf(rt).ack(client.key, epoch, seq);
  }

  // ── Effective capabilities and control ─────────────────────────────────────────────────

  async computeEffective(rt: ThreadRuntime): Promise<ThreadEffective> {
    const inst = this.registry.get(rt.record.instance);
    const provider = inst?.provider;
    if (!inst || !provider) return { imageInput: "unknown", capabilities: [] };
    let eff: ThreadEffective;
    try {
      eff = await provider.effective(rt.record.native);
    } catch {
      eff = { imageInput: "unknown", capabilities: [] };
    }
    const declared = new Set(inst.descriptor.capabilities ?? []);
    const needs: Partial<Record<Capability, keyof HarnessProvider>> = {
      threadFork: "forkThread",
      steer: "steer",
      threadRename: "rename",
      threadArchive: "archive",
      threadDelete: "remove",
    };
    eff.capabilities = (eff.capabilities ?? []).filter((c) => {
      if (declared.size > 0 && !declared.has(c)) return false;
      const method = needs[c];
      return !method || typeof provider[method] === "function";
    });
    // `threadMove` is yaacd's own (D144): any thread can be placed under another project.
    if (!eff.capabilities.includes("threadMove")) eff.capabilities.push("threadMove");
    // Fast mode (D125) is a model-level capability: present when the thread's model has it.
    eff.capabilities = eff.capabilities.filter((c) => c !== "fastMode" && c !== "ultrafastMode");
    for (const cap of ["fastMode", "ultrafastMode"] as const) {
      if (this.modelSupportsSpeed(inst.descriptor, eff.applied?.model ?? this.modelOf(rt), cap)) eff.capabilities.push(cap);
    }
    // A read-only harness sub-agent (D151) can be read and watched, nothing more.
    if (rt.record.metadataPending || this.subagentReadOnly(rt)) eff.capabilities = eff.capabilities.filter((c) => !SUBAGENT_WRITES.has(c));
    rt.effective = eff;
    return eff;
  }

  /** Asks the provider who holds the thread. Only a positive `external` makes it read-only. */
  async refreshExternal(rt: ThreadRuntime): Promise<ExternalControl> {
    // A read-only harness sub-agent (D151) is held by the agent that started it: positive
    // evidence of another writer, and nothing YAAC can take over.
    const readOnly = this.subagentReadOnly(rt);
    if (readOnly) {
      const sub: ExternalControl = { external: true, holder: "the agent that started it", takeOverBlocked: readOnly };
      this.setExternal(rt, sub);
      return sub;
    }
    const provider = this.registry.get(rt.record.instance)?.provider;
    if (!provider) return rt.external;
    let ext: ExternalControl;
    try {
      ext = await provider.control(rt.record.native);
    } catch {
      ext = { external: false, uncertain: true, holder: "unknown (the harness did not say)" };
    }
    this.setExternal(rt, ext);
    return ext;
  }

  /**
   * What the provider reports, with a takeover (D140) applied: the holders the thread was
   * taken from no longer make it read-only. A holder it was not taken from ends the takeover
   * (and says so in the transcript); so does a harness that positively reports nobody else.
   * "Unsure" keeps it.
   */
  private applyTakeOver(rt: ThreadRuntime, ext: ExternalControl): ExternalControl {
    const t = rt.record.takenOver;
    if (!t) return ext;
    if (!ext.external) {
      if (!ext.uncertain) this.endTakeOver(rt, null);
      return ext;
    }
    const fresh = (ext.holderIds ?? []).some((id) => !t.holderIds.includes(id));
    if (fresh) {
      this.endTakeOver(rt, `${ext.holder ?? "Another program"} opened this thread after it was taken over; it is read-only here again.`);
      return ext;
    }
    return { external: false };
  }

  private endTakeOver(rt: ThreadRuntime, notice: string | null): void {
    if (!rt.record.takenOver) return;
    rt.record.takenOver = null;
    rt.saveRecord();
    if (notice) this.noticeItem(rt, "info", notice);
  }

  /**
   * `thread/takeOver` (D140): continue here a thread another program holds, without stopping
   * that program. Refused when the harness will not let go (`takeOverBlocked`).
   */
  async threadTakeOver(p: { opId: string; threadId: string }): Promise<Record<string, never>> {
    return this.ops.run("thread/takeOver", p.opId, async () => {
      const rt = this.findThread(p.threadId);
      if (!rt || rt.deleted) throw notActed("thread_not_found", `no thread ${String(p.threadId)}`);
      const eff = await this.computeEffective(rt);
      if (!(eff.capabilities ?? []).includes("threadTakeOver")) throw notActed("capability_missing", "this thread's harness cannot hand over a thread another program holds");
      const provider = this.provider(rt);
      let raw: ExternalControl;
      try {
        raw = await provider.control(rt.record.native);
      } catch (e) {
        throw notActed("provider_unavailable", `the harness did not say who holds the thread: ${(e as Error).message}`);
      }
      if (!raw.external) {
        // Nobody else holds it: there is nothing to take.
        this.setExternal(rt, raw);
        return {};
      }
      if (raw.takeOverBlocked) {
        this.setExternal(rt, raw);
        throw notActed("thread_in_use", raw.takeOverBlocked);
      }
      const from = raw.holder ?? "another program";
      rt.record.takenOver = { from, holderIds: [...(raw.holderIds ?? [])], at: nowIso() };
      rt.saveRecord();
      this.noticeItem(rt, "takenOver", `Taken over from ${from} · the other program is still open and will not see these messages`);
      this.setExternal(rt, raw);
      return {};
    });
  }

  private setExternal(rt: ThreadRuntime, raw: ExternalControl): void {
    const readOnly = this.subagentReadOnly(rt);
    if (readOnly) raw = { external: true, holder: "the agent that started it", takeOverBlocked: readOnly };
    const ext = this.applyTakeOver(rt, raw);
    const before = canonical(rt.external);
    rt.external = {
      external: !!ext.external,
      ...(ext.uncertain ? { uncertain: true } : {}),
      ...(ext.holder ? { holder: ext.holder } : {}),
      ...(ext.external && ext.takeOverBlocked ? { takeOverBlocked: ext.takeOverBlocked } : {}),
    };
    if (canonical(rt.external) === before) return;
    this.appendControl(rt);
    this.updateIndex(rt);
    if (rt.canDispatch()) this.afterIdle(rt);
  }

  private checkWriter(rt: ThreadRuntime, client: ClientLink | null, wrap: boolean, answeringRequest = false): void {
    const fail = (message: string, holder: string | null) => {
      const err = rpc("thread_in_use", message, holder ? { holder } : undefined);
      throw wrap ? new NotActed(err) : err;
    };
    if (rt.external.external) {
      fail(`this thread is in use in ${rt.external.holder ?? "another program"}; it can only be watched`, rt.external.holder ?? null);
    }
    if (rt.record.metadataPending && !answeringRequest) fail(METADATA_PENDING, null);
    const driver = rt.driver ?? rt.compactDriver;
    if (driver && (!client || driver.key !== client.key)) {
      fail(`this thread is being driven from ${driver.host ?? "another YAAC client"}; it can only be watched`, driver.host);
    }
  }

  // ── Input ──────────────────────────────────────────────────────────────────────────────

  private provider(rt: ThreadRuntime): HarnessProvider {
    this.checkStore(rt);
    return this.registry.usable(rt.record.instance).provider;
  }

  private checkModel(descriptor: ProviderDescriptor, model: ModelId | null | undefined): void {
    if (!model) return;
    const models = descriptor.models ?? [];
    if (descriptor.modelsComplete && !models.some((m) => m.id === model)) {
      throw notActed("model_unavailable", `${model} is not offered by ${descriptor.displayName} now`);
    }
  }

  /** The model a thread's turns run on, as far as yaacd knows (null: the harness default). */
  private modelOf(rt: ThreadRuntime): ModelId | null {
    return rt.effective?.applied?.model ?? rt.record.model ?? null;
  }

  /**
   * Whether a model offers fast mode (D125): a definite `supported` on its descriptor (the
   * default model when none is named). Unknown is not yes.
   */
  modelSupportsSpeed(descriptor: ProviderDescriptor, model: ModelId | null | undefined, speed: "fastMode" | "ultrafastMode"): boolean {
    const models = descriptor.models ?? [];
    const m = model ? models.find((x) => x.id === model) : models.find((x) => x.isDefault) ?? models[0];
    return m?.capabilities?.some((c) => c.capability === speed && c.support === "supported") ?? false;
  }

  private checkSpeed(descriptor: ProviderDescriptor, model: ModelId | null | undefined, speed: "fast" | "ultrafast", value: unknown): boolean {
    if (value === undefined || value === null) return false;
    if (typeof value !== "boolean") throw notActed("invalid_params", `${speed} must be true or false`);
    if (value && !this.modelSupportsSpeed(descriptor, model, speed === "fast" ? "fastMode" : "ultrafastMode")) {
      throw notActed("invalid_params", `${model ?? "the default model"} has no ${speed} mode on ${descriptor.displayName}`);
    }
    return value;
  }

  private modelImageSupport(descriptor: ProviderDescriptor, model: ModelId | null | undefined): "supported" | "unsupported" | "unknown" {
    const models = descriptor.models ?? [];
    const m = model ? models.find((x) => x.id === model) : models.find((x) => x.isDefault) ?? models[0];
    const s = m?.capabilities?.find((c) => c.capability === "imageInput")?.support;
    return s === "supported" || s === "unsupported" ? s : "unknown";
  }

  private validateParts(parts: unknown, imageSupport: string): InputPart[] {
    if (!Array.isArray(parts) || parts.length === 0) throw notActed("invalid_params", "input must be a non-empty list of parts");
    for (const p of parts as InputPart[]) {
      if (p?.type === "text") {
        if (typeof p.text !== "string") throw notActed("invalid_params", "a text part needs text");
      } else if (p?.type === "image" || p?.type === "file") {
        if (typeof p.uploadId !== "string") throw notActed("invalid_params", `an ${p.type} part needs an uploadId`);
        if (!this.uploads.present(p.uploadId)) throw notActed("upload_not_found", `upload ${p.uploadId} is not on this target`);
        if (p.type === "image" && imageSupport === "unsupported") {
          throw notActed("capability_missing", "the selected model cannot see images; send the image as a file instead");
        }
      } else if (p?.type === "skill") {
        // A provider without `inlineSkills` ignores it: the `$name` text still says it all.
        if (typeof p.name !== "string" || p.name === "" || typeof p.path !== "string" || p.path === "") throw notActed("invalid_params", "a skill part needs a name and a path");
      } else {
        throw notActed("invalid_params", `unsupported input part type ${String((p as { type?: unknown })?.type)}`);
      }
    }
    // The journaled parts carry the file's name and media type, so a transcript can show them.
    return (parts as InputPart[]).map((p) => {
      if (p.type !== "image" && p.type !== "file") return p;
      const meta = this.uploads.meta(p.uploadId);
      if (!meta) return p;
      const out = { ...p, name: p.name ?? meta.name };
      if (meta.mediaType && !p.mediaType) out.mediaType = meta.mediaType;
      return out;
    });
  }

  private buildInput(threadKey: string, parts: InputPart[], origin: TurnOrigin): ProviderInput {
    const texts: string[] = [];
    const images: ProviderInput["images"] = [];
    const files: ProviderInput["files"] = [];
    for (const p of parts) {
      if (p.type === "text") texts.push(p.text);
      else if (p.type === "image") {
        const l = this.uploads.linkForThread(threadKey, p.uploadId);
        images.push({ uploadId: p.uploadId, path: l.path, mediaType: l.meta.mediaType });
      } else if (p.type === "file") {
        const l = this.uploads.linkForThread(threadKey, p.uploadId);
        files.push({ uploadId: p.uploadId, path: l.path, name: l.meta.name });
      }
    }
    let text = texts.join("\n\n");
    if (files.length > 0) {
      text += `${text ? "\n\n" : ""}Attached files (on this machine):\n${files.map((f) => `- ${f.path}`).join("\n")}`;
    }
    return { parts, text, images, files, origin, uploadsDir: this.uploads.threadDir(threadKey) };
  }

  // ── Starting work: thread/start and the per-thread dispatcher ─────────────────────────

  async threadStart(client: ClientLink, p: ThreadStartParams): Promise<ThreadStartResult> {
    return this.ops.run("thread/start", p.opId, async () => {
      if (typeof p.instance !== "string") throw notActed("invalid_params", "instance is required");
      let usable;
      try {
        usable = this.registry.usable(p.instance);
      } catch (e) {
        throw new NotActed(e as RpcError);
      }
      const { instance, provider } = usable;
      // A thread started from YAAC starts in the root of its project's folder (D65).
      let cwd = p.cwd;
      if (p.project !== undefined && p.project !== null) {
        const project = this.projects.get(p.project);
        if (!project) throw notActed("invalid_params", `no project ${String(p.project)} on this target`);
        if (typeof cwd === "string" && cwd !== "" && path.resolve(cwd) !== project.folder) {
          throw notActed("invalid_params", `cwd must be empty or the project's folder (${project.folder})`);
        }
        cwd = project.folder;
      }
      if (typeof cwd !== "string" || !path.isAbsolute(cwd) || !fs.existsSync(cwd) || !fs.statSync(cwd).isDirectory()) {
        throw notActed("invalid_params", `the working directory ${String(cwd)} does not exist on this target`);
      }
      if (p.agents !== undefined && p.agents !== null && typeof p.agents !== "string") throw notActed("invalid_params", "agents must be a string");
      this.checkModel(instance.descriptor, p.model);
      const ultrafast = this.checkSpeed(instance.descriptor, p.model, "ultrafast", p.ultrafast);
      const fast = this.checkSpeed(instance.descriptor, p.model, "fast", p.fast) && !ultrafast;
      const parts = this.validateParts(p.input, this.modelImageSupport(instance.descriptor, p.model));
      // The thread's id is not known until the harness has started it: its attachment
      // folder is keyed by this operation, for the thread's whole life.
      const uploadsKey = `start:${p.opId}`;
      const input = this.buildInput(uploadsKey, parts, "user");
      const trusted = await this.trustProjectFolder(instance, cwd);
      const key = instance.config.id;
      this.pendingStarts.set(key, (this.pendingStarts.get(key) ?? 0) + 1);
      let started: { thread: NativeId; turn: TurnId };
      try {
        started = await provider.startThread({ cwd, model: p.model ?? null, effort: p.effort ?? null, mode: p.mode ?? null, fast, ultrafast }, input);
      } catch (e) {
        this.finishPendingStart(key);
        throw rpc("provider_unavailable", `${instance.descriptor.displayName} could not start the thread: ${(e as Error).message}`);
      }
      const firstText = parts.find((x) => x.type === "text") as { text?: string } | undefined;
      const rt = this.createThread(
        instance,
        started.thread,
        {
          cwd,
          originKind: "yaac",
          title: (firstText?.text ?? "").split("\n")[0]!.slice(0, 80) || "New thread",
          preview: (firstText?.text ?? "").slice(0, 120),
          model: p.model ?? null,
          uploadsKey,
          ...(fast ? { fast: true } : {}),
          ...(ultrafast ? { ultrafast: true } : {}),
          ...(typeof p.agents === "string" ? { agents: p.agents } : {}),
        },
        true,
      );
      const driver: Driver = { key: client.key, host: client.host };
      this.turnBegan(rt, started.turn, "user", parts, driver, undefined, p.opId);
      this.trustNotice(rt, trusted);
      this.finishPendingStart(key);
      return { threadId: rt.id, turnId: started.turn };
    });
  }

  private finishPendingStart(instance: string): void {
    const n = (this.pendingStarts.get(instance) ?? 1) - 1;
    if (n > 0) this.pendingStarts.set(instance, n);
    else this.pendingStarts.delete(instance);
    const events = this.buffered.get(instance) ?? [];
    this.buffered.delete(instance);
    for (const ev of events) this.onProviderEvent(instance, ev);
  }

  /** Journals the start of a turn yaacd asked for: `turnStarted`, then the user's message. */
  private turnBegan(rt: ThreadRuntime, turnId: TurnId, origin: TurnOrigin, parts: InputPart[], driver: Driver | null, items?: Item[], opId?: string): void {
    if (rt.activeTurn === turnId) return;
    rt.expectTurn = null;
    rt.activeTurn = turnId;
    rt.turnOrigin = origin;
    rt.driver = driver;
    rt.lastStatus = null;
    rt.saveRuntime();
    this.append(rt, { type: "turnStarted", turnId, origin, at: nowIso() });
    const at = nowIso();
    if (items && items.length > 0) {
      // Mesh input is journaled as what it is (`meshReceived`), never as a user message.
      for (const raw of items) {
        const item = { ...raw, turnId, startedAt: raw.startedAt ?? at, completedAt: raw.completedAt ?? at } as Item;
        this.append(rt, { type: "itemStarted", item });
        this.append(rt, { type: "itemCompleted", item });
      }
    } else {
      // The op id lets the window put this in place of its "Sending…" echo (D165).
      const user: Item = { id: `user:${turnId}`, turnId, status: "completed", kind: "userMessage", parts, ...(opId ? { opId } : {}), startedAt: at, completedAt: at };
      this.append(rt, { type: "itemStarted", item: user });
      this.append(rt, { type: "itemCompleted", item: user });
    }
    if (driver) this.appendControl(rt);
    rt.record.updatedAt = nowIso();
    rt.saveRecord();
    this.updateIndex(rt);
    this.activity.emit();
    this.mesh?.turnStarted(rt, turnId, origin);
  }

  /** Compact in place, using the same writer and dispatch checks as a message. */
  async threadCompact(client: ClientLink, p: ThreadCompactParams): Promise<Record<string, never>> {
    return this.ops.run("thread/compact", p.opId, async () => {
      const rt = this.findThread(p.threadId);
      if (!rt || rt.deleted) throw notActed("thread_not_found", "the thread was not found");
      const away = this.storeAway(rt);
      if (away) throw notActed("provider_unavailable", away);
      const provider = this.registry.get(rt.record.instance)?.provider;
      const effective = rt.effective ?? (await this.computeEffective(rt));
      if (!provider?.compact || !effective.capabilities?.includes("compact")) {
        throw notActed("capability_missing", "this thread does not support context compaction");
      }
      await this.refreshExternal(rt);
      this.checkWriter(rt, client, true);
      if (rt.pendingRequestCount()) throw notActed("awaiting_user", "answer the pending request before compacting context");
      if (rt.record.hold) throw notActed("thread_held", "the thread is stopped and held");
      if (!rt.canDispatch()) throw notActed("turn_in_progress", "wait for the current turn to finish before compacting context");
      rt.dispatching = true;
      rt.compactDriver = { key: client.key, host: client.host };
      this.appendControl(rt);
      this.updateIndex(rt);
      this.activity.emit();
      try {
        await provider.compact(rt.record.native);
        return {};
      } catch (e) {
        throw rpc(e instanceof CompactionUnconfirmed ? "outcome_unknown" : "provider_unavailable", (e as Error).message);
      } finally {
        rt.compactDriver = null;
        rt.dispatching = false;
        this.appendControl(rt);
        this.updateIndex(rt);
        this.activity.emit();
        if (rt.canDispatch()) this.afterIdle(rt);
      }
    });
  }

  /**
   * `turn/start`: a client's message. With no turn running it starts one. With one running:
   * refused (no `delivery`, as before), queued (`queue`), or delivered now (`now`).
   */
  async turnStart(client: ClientLink, p: TurnStartParams): Promise<TurnStartResult> {
    return this.ops.run("turn/start", p.opId, async () => {
      const rt = this.findThread(p.threadId);
      if (!rt || rt.deleted) throw notActed("thread_not_found", `no thread ${String(p.threadId)} on this target`);
      let inst;
      try {
        this.checkStore(rt);
        inst = this.registry.usable(rt.record.instance).instance;
      } catch (e) {
        throw new NotActed(e as RpcError);
      }
      const delivery = p.delivery ?? null;
      if (delivery !== null && delivery !== "queue" && delivery !== "now") throw notActed("invalid_params", "delivery must be queue or now");
      await this.refreshExternal(rt);
      this.checkWriter(rt, client, true);
      if (rt.compactDriver && delivery !== "queue") throw notActed("turn_in_progress", "context is being compacted");
      this.checkModel(inst.descriptor, p.model);
      const eff = rt.effective ?? (await this.computeEffective(rt));
      this.checkSpeed(inst.descriptor, p.model ?? this.modelOf(rt), "fast", p.fast);
      this.checkSpeed(inst.descriptor, p.model ?? this.modelOf(rt), "ultrafast", p.ultrafast);
      const imageSupport = p.model ? this.modelImageSupport(inst.descriptor, p.model) : eff.imageInput;
      const parts = this.validateParts(p.input, imageSupport);
      const overrides = { model: p.model ?? null, effort: p.effort ?? null, mode: p.mode ?? null, fast: p.fast ?? null, ultrafast: p.ultrafast ?? null };
      const driver: Driver = { key: client.key, host: client.host };
      const busy = !!rt.activeTurn || rt.dispatching || rt.claimed;
      if (!busy || delivery !== null) this.mesh?.humanInput?.(rt);
      if (!busy) {
        // The user sending a message is what starts a stopped thread again.
        if (rt.record.hold) this.releaseHold(rt, false);
        rt.queuePaused = false;
        const turnId = await this.submit(rt, { origin: "user", parts, overrides, driver, opId: p.opId }, "now");
        return { turnId, outcome: "started" as TurnStartOutcome };
      }
      if (delivery === null) {
        throw notActed("turn_in_progress", "a turn is already running in this thread", rt.activeTurn ? { activeTurn: rt.activeTurn } : undefined);
      }
      if (delivery === "queue") {
        const entry = this.enqueueUser(rt, parts, overrides, driver, p.opId);
        return { turnId: "", outcome: "queued" as TurnStartOutcome, queueId: entry.queueId };
      }
      if (rt.pendingRequestCount() > 0) throw notActed("awaiting_user", NOW_WHILE_ASKING);
      return this.deliverNow(rt, parts, overrides, driver, "user", { opId: p.opId });
    });
  }

  // ── Queued user messages, and `now` ────────────────────────────────────────────────────

  private publicEntry(e: QueuedEntry): QueuedMessage {
    const { driver: _d, dispatching: _x, opId: _o, ...pub } = e;
    return clone(pub) as QueuedMessage;
  }

  private queueEvent(rt: ThreadRuntime, change: QueueChange, entry: QueuedEntry, turnId?: TurnId): void {
    const ev = { type: "queueChanged", change, entry: this.publicEntry(entry), ...(turnId ? { turnId } : {}) } as ThreadEvent;
    this.append(rt, ev, undefined, true);
  }

  /** Queues a user message durably, ahead of queued agent input. */
  private enqueueUser(rt: ThreadRuntime, parts: InputPart[], overrides: TurnOverrides, driver: Driver | null, opId?: string): QueuedEntry {
    const entry: QueuedEntry = {
      queueId: randomId("q_", 10),
      input: parts,
      queuedAt: nowIso(),
      ...(overrides.model ? { model: overrides.model } : {}),
      ...(overrides.effort ? { effort: overrides.effort } : {}),
      ...(overrides.mode ? { mode: overrides.mode } : {}),
      ...(typeof overrides.fast === "boolean" ? { fast: overrides.fast } : {}),
      ...(typeof overrides.ultrafast === "boolean" ? { ultrafast: overrides.ultrafast } : {}),
      driver,
      ...(opId ? { opId } : {}),
    };
    rt.userQueue.push(entry);
    rt.saveQueue();
    this.queueEvent(rt, "added", entry);
    this.pump(rt);
    return entry;
  }

  /** The thread's queued user messages, in order. */
  queuedOf(rt: ThreadRuntime): QueuedMessage[] {
    return rt.userQueue.map((e) => this.publicEntry(e));
  }

  private queuedEntry(rt: ThreadRuntime, queueId: unknown): QueuedEntry {
    const e = rt.userQueue.find((q) => q.queueId === queueId);
    if (!e) throw notActed("queue_entry_not_found", `no queued message ${String(queueId)} in this thread (it may have been delivered)`);
    if (e.dispatching) throw notActed("queue_entry_not_found", "that message is being delivered now");
    return e;
  }

  async queueEdit(client: ClientLink, p: { opId: string; threadId: string; queueId: string; input: InputPart[] }): Promise<void> {
    await this.ops.run("turn/queueEdit", p.opId, async () => {
      const rt = this.findThread(p.threadId);
      if (!rt || rt.deleted) throw notActed("thread_not_found", `no thread ${String(p.threadId)} on this target`);
      this.checkWriter(rt, client, true);
      const e = this.queuedEntry(rt, p.queueId);
      const eff = rt.effective ?? (await this.computeEffective(rt));
      e.input = this.validateParts(p.input, eff.imageInput);
      rt.saveQueue();
      this.queueEvent(rt, "updated", e);
      rt.queuePaused = false;
      this.pump(rt);
      return {};
    });
  }

  async commandLogs(threadId: string, commandId: string) {
    const rt = this.findThread(threadId);
    if (!rt || rt.deleted) throw rpc("thread_not_found", "The command's thread is no longer available.");
    const provider = this.provider(rt);
    if (!provider.commandLogs) return { truncated: false };
    return provider.commandLogs(rt.record.native, commandId);
  }

  async commandStop(client: ClientLink, opId: string, threadId: string, commandId: string): Promise<void> {
    await this.ops.run("command/stop", opId, async () => {
      const rt = this.findThread(threadId);
      if (!rt || rt.deleted || !rt.runningCommands.some((c) => c.id === commandId && c.canStop)) {
        throw notActed("command_unavailable", "This command no longer has a Stop action.");
      }
      const controller = this.commandController(rt);
      if (!controller || controller.deleted) throw notActed("command_unavailable", "The command's controlling thread is unavailable.");
      this.checkWriter(controller, client, true);
      const provider = this.provider(rt);
      if (!provider.stopCommand || !(await provider.stopCommand(rt.record.native, commandId))) {
        throw notActed("command_unavailable", "The harness could not stop this command.");
      }
      return {};
    });
  }

  async queueCancel(client: ClientLink, p: { opId: string; threadId: string; queueId: string }): Promise<void> {
    await this.ops.run("turn/queueCancel", p.opId, async () => {
      const rt = this.findThread(p.threadId);
      if (!rt || rt.deleted) throw notActed("thread_not_found", `no thread ${String(p.threadId)} on this target`);
      this.checkWriter(rt, client, true);
      const e = this.queuedEntry(rt, p.queueId);
      rt.userQueue = rt.userQueue.filter((q) => q !== e);
      rt.saveQueue();
      this.queueEvent(rt, "removed", e);
      rt.queuePaused = false;
      this.pump(rt);
      return {};
    });
  }

  /** "Send now" on a queued message: out of the queue, with the `now` mechanics. */
  async queuePromote(client: ClientLink, p: { opId: string; threadId: string; queueId: string }): Promise<TurnStartResult> {
    return this.ops.run("turn/queuePromote", p.opId, async () => {
      const rt = this.findThread(p.threadId);
      if (!rt || rt.deleted) throw notActed("thread_not_found", `no thread ${String(p.threadId)} on this target`);
      await this.refreshExternal(rt);
      this.checkWriter(rt, client, true);
      const e = this.queuedEntry(rt, p.queueId);
      if (rt.compactDriver) throw notActed("turn_in_progress", "context is being compacted");
      if (rt.pendingRequestCount() > 0) throw notActed("awaiting_user", NOW_WHILE_ASKING);
      this.mesh?.humanInput?.(rt);
      rt.userQueue = rt.userQueue.filter((q) => q !== e);
      rt.saveQueue();
      this.queueEvent(rt, "removed", e);
      rt.queuePaused = false;
      if (rt.record.hold) this.releaseHold(rt, false);
      const overrides = { model: e.model ?? null, effort: e.effort ?? null, mode: e.mode ?? null, fast: e.fast ?? null, ultrafast: e.ultrafast ?? null };
      const driver: Driver = { key: client.key, host: client.host };
      if (!rt.activeTurn && !rt.dispatching && !rt.claimed) {
        const turnId = await this.submit(rt, { origin: "user", parts: e.input, overrides, driver, ...(e.opId ? { opId: e.opId } : {}) }, "now");
        return { turnId, outcome: "started" as TurnStartOutcome };
      }
      return this.deliverNow(rt, e.input, overrides, driver, "user", e.opId ? { opId: e.opId } : {});
    });
  }

  /** Whether `now` input for this thread is steered (else it interrupts the running turn). */
  async canSteer(rt: ThreadRuntime): Promise<boolean> {
    const provider = this.registry.get(rt.record.instance)?.provider;
    const eff = rt.effective ?? (await this.computeEffective(rt));
    return !!provider && typeof provider.steer === "function" && (eff.capabilities ?? []).includes("steer");
  }

  /**
   * Delivers input to a thread that is running a turn, at once: steered into the running turn
   * where the harness can steer (with the turn id as the precondition), otherwise the running
   * turn is interrupted and a new one starts with it. Chosen by capability, never by harness.
   */
  async deliverNow(
    rt: ThreadRuntime,
    parts: InputPart[],
    overrides: TurnOverrides,
    driver: Driver | null,
    origin: TurnOrigin,
    extra: { journalItems?: Item[]; mesh?: MeshInputMessage[]; onStarted?: (turnId: TurnId) => void | Promise<void>; beforeStart?: () => void | Promise<void>; opId?: string } = {},
  ): Promise<TurnStartResult> {
    if (rt.claimed) throw notActed("turn_in_progress", "another message is being handed to this thread");
    if (rt.compactDriver) throw notActed("turn_in_progress", "context is being compacted");
    const provider = this.provider(rt);
    const turn = rt.activeTurn;
    if (turn && (await this.canSteer(rt))) {
      if (rt.claimed || rt.activeTurn !== turn || rt.pendingRequestCount() > 0) throw notActed("turn_in_progress", "the recipient changed before the message could be delivered");
      const input = this.buildInput(rt.uploadsKey, parts, origin);
      if (extra.mesh) input.mesh = extra.mesh;
      await extra.beforeStart?.();
      try {
        await provider.steer!(rt.record.native, turn, input);
      } catch (e) {
        throw rpc("provider_unavailable", `the harness did not take the message into the running turn: ${(e as Error).message}`);
      }
      // What was added is in the transcript, as what it is.
      const at = nowIso();
      const items: Item[] = extra.journalItems ?? [{ id: `user:steer:${randomId("", 8)}`, turnId: turn, status: "completed", kind: "userMessage", parts, ...(extra.opId ? { opId: extra.opId } : {}) } as Item];
      for (const raw of items) {
        const item = { ...raw, turnId: turn, startedAt: raw.startedAt ?? at, completedAt: raw.completedAt ?? at } as Item;
        this.append(rt, { type: "itemStarted", item });
        this.append(rt, { type: "itemCompleted", item });
      }
      await extra.onStarted?.(turn);
      return { turnId: turn, outcome: "steered" as TurnStartOutcome };
    }
    let interrupted = false;
    if (rt.activeTurn || rt.dispatching) {
      rt.claimed = true;
      try {
        if (rt.activeTurn) {
          interrupted = true;
          const t = rt.activeTurn;
          await Promise.race([provider.interrupt(rt.record.native, t).catch(() => {}), sleepMs(NOW_INTERRUPT_WAIT_MS)]);
        }
        const deadline = Date.now() + NOW_INTERRUPT_WAIT_MS;
        while ((rt.activeTurn || rt.dispatching) && Date.now() < deadline) await sleepMs(20);
      } finally {
        rt.claimed = false;
      }
      if (rt.activeTurn || rt.dispatching) {
        if (rt.canDispatch()) this.afterIdle(rt);
        throw rpc("turn_in_progress", "the running turn was asked to stop and has not stopped yet; the message was not sent");
      }
    }
    const turnId = await this.submit(rt, { origin, parts, overrides, driver, ...(extra.opId ? { opId: extra.opId } : {}), ...(extra.journalItems ? { journalItems: extra.journalItems } : {}), ...(extra.mesh ? { mesh: extra.mesh } : {}), ...(extra.onStarted ? { onStarted: extra.onStarted } : {}), ...(extra.beforeStart ? { beforeStart: extra.beforeStart } : {}) }, "now");
    return { turnId, outcome: (interrupted ? "interruptedAndStarted" : "started") as TurnStartOutcome };
  }

  /** Offer mail only to the same owned turn, with the ownership and approval checks repeated at handoff. */
  async deliverPeer(rt: ThreadRuntime, dispatch: AgentDispatch, beforeSend: () => boolean, beforeFollowOn: () => void): Promise<TurnId | null> {
    const provider = this.provider(rt);
    const turn = rt.activeTurn;
    const blocked = () => rt.activeTurn !== turn || rt.deleted || rt.dispatching || rt.compactDriver || rt.record.hold || rt.external.external || rt.pendingRequestCount() > 0;
    if (!provider.deliverPeer || !turn || rt.claimed || blocked()) return null;
    rt.claimed = true;
    try {
      const raw = await provider.control(rt.record.native).catch((): ExternalControl => ({ external: false, uncertain: true }));
      this.setExternal(rt, raw);
      if (blocked()) return null;
      const input = this.buildInput(rt.uploadsKey, dispatch.input, "mesh");
      input.mesh = dispatch.mesh;
      const accepted = await provider.deliverPeer(rt.record.native, turn, input, () => !blocked() && beforeSend(), beforeFollowOn);
      if (accepted) {
        for (const rawItem of dispatch.journalItems ?? []) {
          const at = nowIso();
          const item = { ...rawItem, turnId: accepted, startedAt: at, completedAt: at } as Item;
          this.append(rt, { type: "itemStarted", item });
          this.append(rt, { type: "itemCompleted", item });
        }
      }
      return accepted;
    } finally {
      rt.claimed = false;
      if (rt.canDispatch()) this.afterIdle(rt);
    }
  }

  /** Dispatches the first queued user message (the queue goes ahead of agent input). */
  private dispatchQueued(rt: ThreadRuntime): void {
    const entry = rt.userQueue[0]!;
    entry.dispatching = true;
    rt.saveQueue();
    const settle = () => {
      entry.dispatching = false;
      rt.saveQueue();
    };
    const work: Work = {
      origin: "user",
      parts: entry.input,
      overrides: { model: entry.model ?? null, effort: entry.effort ?? null, mode: entry.mode ?? null, fast: entry.fast ?? null, ultrafast: entry.ultrafast ?? null },
      driver: entry.driver ?? null,
      ...(entry.opId ? { opId: entry.opId } : {}),
      onStarted: (turnId) => {
        rt.userQueue = rt.userQueue.filter((q) => q !== entry);
        rt.saveQueue();
        this.queueEvent(rt, "delivered", entry, turnId);
      },
      onFailed: (e) => {
        // The harness refused it: never re-sent blindly; the queue waits for the user.
        settle();
        rt.queuePaused = true;
        this.noticeItem(rt, "warning", `A queued message could not be delivered: ${e.message}. It is still queued; edit, send, or cancel it.`);
      },
      resolve: () => {},
      reject: () => {
        if (rt.userQueue.includes(entry) && entry.dispatching) settle();
      },
    };
    void this.execute(rt, work);
  }

  /**
   * Folder trust (D147): before YAAC starts a conversation in `cwd`, marks that folder trusted
   * in the instance's harness — only when the folder belongs to a project the user created (by
   * the same rule that lists threads under projects), and once per folder (`FolderTrust`).
   * Resolves to the notice for the new thread, or null. Never throws: the thread starts anyway.
   */
  private async trustProjectFolder(instance: Instance, cwd: string): Promise<string | null> {
    if (!FolderTrust.applies(instance)) return null;
    try {
      const folder = path.resolve(cwd);
      if (!matchProject(this.projects.list(), { folder, repo: await this.repoOf(folder, true) })) return null;
      return await this.folderTrust.ensure(instance, folder);
    } catch (e) {
      this.opts.log(`folder trust skipped: ${(e as Error).message}`);
      return null;
    }
  }

  private trustNotice(rt: ThreadRuntime, text: string | null): void {
    if (!text) return;
    this.noticeItem(rt, text.startsWith("Marked ") ? "folderTrust" : "warning", text);
  }

  /** A notice item in a thread's transcript (out of band). */
  noticeItem(rt: ThreadRuntime, kind: string, text: string): void {
    if (rt.deleted) return;
    const item = { id: `notice:${kind}:${randomId("", 8)}`, turnId: rt.activeTurn ?? "", status: "completed", kind: "notice", noticeKind: kind, text } as unknown as Item;
    this.append(rt, { type: "itemStarted", item });
    this.append(rt, { type: "itemCompleted", item });
  }

  // ── Stop and hold (docs/10-agent-mesh.md#stopping-workers) ─────────────────────────────

  setHold(rt: ThreadRuntime, hold: ThreadHold): void {
    rt.record.hold = hold;
    rt.saveRecord();
    this.append(rt, { type: "holdChanged", hold } as ThreadEvent, undefined, true);
    this.updateIndex(rt);
    this.activity.emit();
  }

  /** Releases a hold; `dispatch`: and lets queued input go now. */
  releaseHold(rt: ThreadRuntime, dispatch = true): void {
    if (!rt.record.hold) return;
    rt.record.hold = null;
    rt.saveRecord();
    this.append(rt, { type: "holdChanged" } as ThreadEvent, undefined, true);
    this.updateIndex(rt);
    this.mesh?.holdReleased?.(rt);
    if (dispatch && rt.canDispatch()) this.afterIdle(rt);
  }

  /**
   * Stops a thread and holds it. The outcome is real: `stopped` (its turn ended),
   * `wasNotRunning`, or `didNotRespond` (the turn had not ended after `waitMs`; then, where
   * yaacd owns the harness process, that process is ended).
   */
  async stopAndHold(rt: ThreadRuntime, hold: ThreadHold, waitMs: number): Promise<{ outcome: "stopped" | "wasNotRunning" | "didNotRespond"; detail?: string }> {
    this.setHold(rt, hold);
    this.noticeItem(rt, "stopped", `Stopped by ${hold.label}.`);
    const turn = rt.activeTurn;
    if (!turn) return { outcome: "wasNotRunning" };
    const provider = this.registry.get(rt.record.instance)?.provider;
    if (!provider) return { outcome: "didNotRespond", detail: "its provider is not available" };
    const deadline = Date.now() + waitMs;
    await Promise.race([provider.interrupt(rt.record.native, turn).catch(() => {}), sleepMs(waitMs)]);
    while (rt.activeTurn === turn && Date.now() < deadline) await sleepMs(20);
    if (rt.activeTurn !== turn) return { outcome: "stopped" };
    if (typeof provider.forceStop === "function") {
      await Promise.race([provider.forceStop(rt.record.native).catch(() => {}), sleepMs(waitMs)]);
      const until = Date.now() + waitMs;
      while (rt.activeTurn === turn && Date.now() < until) await sleepMs(20);
      return { outcome: "didNotRespond", detail: rt.activeTurn !== turn ? "its harness process was ended" : "ending its harness process did not end the turn either" };
    }
    return { outcome: "didNotRespond", detail: "YAAC does not own this harness's process (it is shared), so it could not end it; the thread is held and nothing more is sent to it" };
  }

  /**
   * Everything that can start work passes through here, one at a time per thread. "now": the
   * work must be dispatchable at once (a client's message), or it is rejected. "queue": it
   * waits until the thread is held by yaacd and idle (agent-originated input).
   */
  private submit(rt: ThreadRuntime, work: Omit<Work, "resolve" | "reject">, mode: "now" | "queue"): Promise<TurnId> {
    return new Promise<TurnId>((resolve, reject) => {
      const full: Work = { ...work, resolve, reject };
      if (mode === "now") {
        if (!rt.canDispatch()) {
          reject(rt.activeTurn || rt.dispatching || rt.claimed ? notActed("turn_in_progress", "a turn is already running in this thread") : rt.record.hold ? notActed("thread_held", "this thread is stopped and held") : notActed("thread_in_use", "this thread is in use elsewhere"));
          return;
        }
        // Straight to the harness: queued input (the user's or agents') waits behind it.
        void this.execute(rt, full);
        return;
      } else {
        rt.queue.push(full);
      }
      this.pump(rt);
    });
  }

  private pump(rt: ThreadRuntime): void {
    if (!rt.canDispatch()) return;
    if (rt.userQueue.length > 0 && !rt.queuePaused) {
      this.dispatchQueued(rt);
      return;
    }
    if (rt.queue.length === 0) return;
    const work = rt.queue.shift()!;
    void this.execute(rt, work);
  }

  private async execute(rt: ThreadRuntime, work: Work): Promise<void> {
    rt.dispatching = true;
    let handedOver = false;
    try {
      const provider = this.provider(rt);
      // Ownership must be shown, not assumed: ask the harness right before handing input over.
      // Only positive evidence of another holder stops it; "unsure" is free.
      const raw = await provider.control(rt.record.native).catch((): ExternalControl => ({ external: false, uncertain: true }));
      const ext = this.applyTakeOver(rt, raw);
      if (ext.external) {
        rt.dispatching = false;
        this.setExternal(rt, raw);
        if (work.origin === "user") work.reject(notActed("thread_in_use", `this thread is in use in ${ext.holder ?? "another program"}; it can only be watched`));
        else rt.queue.unshift(work);
        return;
      }
      if (work.origin === "mesh") {
        if (rt.pendingRequestCount() || rt.activeTurn || rt.record.hold || rt.claimed || rt.deleted) {
          throw notActed("thread_in_use", "the recipient became unavailable before handoff");
        }
      }
      await work.beforeStart?.();
      const input = this.buildInput(rt.uploadsKey, work.parts, work.origin);
      if (work.mesh) input.mesh = work.mesh;
      rt.expectTurn = { origin: work.origin, parts: work.parts, driver: work.driver, ...(work.journalItems ? { items: work.journalItems } : {}), ...(work.opId ? { opId: work.opId } : {}) };
      const expected = rt.expectTurn;
      // A queued turn keeps its explicit choice; otherwise use the session's current speed.
      const descriptor = this.registry.get(rt.record.instance)?.descriptor;
      const model = work.overrides.model ?? this.modelOf(rt);
      const ultrafast = (work.overrides.ultrafast ?? (work.overrides.fast == null && rt.record.ultrafast)) === true
        && !!descriptor && this.modelSupportsSpeed(descriptor, model, "ultrafastMode");
      const fast = !ultrafast && (work.overrides.fast ?? rt.record.fast) === true
        && !!descriptor && this.modelSupportsSpeed(descriptor, model, "fastMode");
      // A thread a `thread/move` moved (D144): every turn names its folder, and the first one
      // after the move begins with a short fixed note saying so. Only the harness sees it.
      const overrides: TurnOverrides = { ...work.overrides, fast, ultrafast };
      if (rt.record.cwdMoved) overrides.cwd = rt.record.cwd;
      const note = rt.record.cwdNote;
      if (note) input.text = movedNote(note.from, note.to) + input.text;
      let turnId: TurnId;
      try {
        turnId = await provider.startTurn(rt.record.native, input, overrides);
        handedOver = true;
      } catch (e) {
        rt.expectTurn = null;
        if (e instanceof InputOutcomeUnknown) throw rpc("outcome_unknown", e.message);
        if (e instanceof PeerInputDenied) throw rpc("policy_denied", e.message);
        throw rpc("provider_unavailable", `the harness did not start the turn: ${(e as Error).message}`);
      }
      if (note) delete rt.record.cwdNote;
      const speedChanged = (rt.record.fast ?? false) !== fast || (rt.record.ultrafast ?? false) !== ultrafast;
      if (speedChanged) {
        if (fast) rt.record.fast = true;
        else delete rt.record.fast;
        if (ultrafast) rt.record.ultrafast = true;
        else delete rt.record.ultrafast;
        rt.saveRecord();
      }
      // A provider can emit both start and completion before returning the turn id.
      if (rt.expectTurn === expected) this.turnBegan(rt, turnId, work.origin, work.parts, work.driver, work.journalItems, work.opId);
      else if (speedChanged) this.updateIndex(rt);
      rt.dispatching = false;
      try {
        await work.onStarted?.(turnId);
      } finally {
        work.resolve(turnId);
      }
      if (rt.canDispatch()) this.afterIdle(rt);
    } catch (e) {
      rt.dispatching = false;
      const err = handedOver && work.origin === "mesh"
        ? rpc("outcome_unknown", (e as Error).message)
        : e instanceof NotActed || e instanceof RpcError ? e : rpc("internal_error", (e as Error).message);
      work.onFailed?.(err instanceof NotActed ? err.error : err);
      work.reject(err);
      if (rt.canDispatch()) this.afterIdle(rt);
    }
  }

  /** The thread became held and idle: dispatch what is queued, and tell the mesh. */
  private afterIdle(rt: ThreadRuntime): void {
    this.pump(rt);
    if (rt.canDispatch()) this.heldAndIdle.emit(rt.id);
  }

  // ── The dispatcher API (for the agent mesh) ────────────────────────────────────────────

  /**
   * Queues agent-originated input for a thread. It is dispatched as a new turn (origin
   * `mesh`, never presented as the user) once the thread is idle and held by yaacd — never
   * spliced into a running turn, never handed to a thread another program has open.
   */
  enqueueAgentInput(threadId: string, dispatch: AgentDispatch): DispatchHandle {
    const rt = this.requireThread(threadId);
    let work: Work | null = null;
    const started = new Promise<TurnId>((resolve, reject) => {
      work = {
        origin: "mesh",
        parts: dispatch.input,
        overrides: dispatch.overrides ?? {},
        driver: null,
        resolve,
        reject,
        ...(dispatch.beforeStart ? { beforeStart: dispatch.beforeStart } : {}),
        ...(dispatch.onStarted ? { onStarted: dispatch.onStarted } : {}),
        ...(dispatch.onFailed ? { onFailed: dispatch.onFailed } : {}),
        ...(dispatch.journalItems ? { journalItems: dispatch.journalItems } : {}),
        ...(dispatch.mesh ? { mesh: dispatch.mesh } : {}),
      };
    });
    rt.queue.push(work!);
    this.pump(rt);
    return {
      started,
      cancel: () => {
        const i = rt.queue.indexOf(work!);
        if (i < 0) return false;
        rt.queue.splice(i, 1);
        work!.reject(new Error("cancelled"));
        return true;
      },
    };
  }

  /** Whether the thread is idle and provably held by yaacd (asks the harness now). */
  async isHeldAndIdle(threadId: string): Promise<boolean> {
    const rt = this.requireThread(threadId);
    await this.refreshExternal(rt);
    return rt.canDispatch() && rt.pendingRequestCount() === 0;
  }

  /** Agent input waiting in a thread's queue. */
  queuedInputs(threadId: string): number {
    return this.requireThread(threadId).queue.length;
  }

  // ── Agent mesh API (WP7) ───────────────────────────────────────────────────────────────

  /** The thread a provider instance knows by this native id. */
  threadByNative(instance: string, native: NativeId): ThreadRuntime | undefined {
    const id = this.byNative.get(instance, native, this.storeNow(instance));
    const rt = id ? this.threads.get(id) : undefined;
    return rt && !rt.deleted ? rt : undefined;
  }

  /** Journals an item yaacd itself produced (mesh items), through the provider-item path. */
  injectItem(threadId: string, type: "itemStarted" | "itemUpdated" | "itemCompleted", item: Item): void {
    const rt = this.requireThread(threadId);
    this.handleProviderEvent(rt.record.instance, { type, thread: rt.record.native, item } as ProviderEvent);
  }

  /** Raises yaacd's own request (`meshPermission`): journaled durably, listed like any request. */
  raiseMeshRequest(threadId: string, request: RaisedRequest): string {
    const rt = this.requireThread(threadId);
    const requestId = randomId("mp-", 12);
    this.handleProviderEvent(rt.record.instance, { type: "requestRaised", thread: rt.record.native, turnId: rt.activeTurn ?? "", requestId, request, blocking: true });
    return requestId;
  }

  /** Records the answer to a request yaacd raised itself. */
  resolveOwnRequest(rt: ThreadRuntime, rec: RequestRecord, answer: RequestResponse, client: ClientLink | null): void {
    const resolution: RequestResolution = { by: client ? "client" : "elsewhere", at: nowIso(), answer };
    if (client) {
      resolution.clientId = client.key;
      if (client.host) resolution.clientHost = client.host;
    }
    this.setRequestState(rt, rec, "resolved", resolution);
  }

  /**
   * Starts a thread for an agent (`spawn_thread`): no client drives it, its first turn has
   * origin `mesh`, and it starts in the mode the mesh chose from its parent's (D146), or in the
   * harness's default mode when none is given (no mode is sent).
   */
  async startThreadForAgent(p: {
    instance: string;
    cwd: string;
    model?: string | null;
    effort?: string | null;
    /** The mode to start in (D146: the mesh's choice); null: the harness's default. */
    mode?: string | null;
    title: string;
    parts: InputPart[];
    journalItems: Item[];
    mesh: MeshInputMessage[];
    spawnedBy: string;
    /** Resolved by the local mesh attachment. A caller on another target has no local runtime. */
    parent?: ThreadRuntime | null;
    agents?: ThreadRecord["agents"] | null;
  }): Promise<{ threadId: string; turnId: TurnId }> {
    const { instance, provider } = this.registry.usable(p.instance);
    if (!fs.existsSync(p.cwd) || !fs.statSync(p.cwd).isDirectory()) throw rpc("invalid_params", `the working directory ${p.cwd} does not exist on this target`);
    this.checkModel(instance.descriptor, p.model);
    const uploadsKey = `spawn:${randomId("s", 12)}`;
    const input = this.buildInput(uploadsKey, p.parts, "mesh");
    input.mesh = p.mesh;
    // Only a folder of a user's project: an agent's arbitrary path keeps the harness's behaviour.
    const trusted = await this.trustProjectFolder(instance, p.cwd);
    const key = instance.config.id;
    this.pendingStarts.set(key, (this.pendingStarts.get(key) ?? 0) + 1);
    let started: { thread: NativeId; turn: TurnId };
    let launchStoreKey: string | null | undefined;
    try {
      launchStoreKey = instance.descriptor.store.key ?? null;
      const beforeFirstPrompt = p.parent
        ? await this.workerLaunchCallback(p.parent, instance, provider, launchStoreKey)
        : undefined;
      if (!beforeFirstPrompt) launchStoreKey = undefined;
      started = await provider.startThread({
        cwd: p.cwd, model: p.model ?? null, effort: p.effort ?? null, mode: p.mode ?? null,
        ...(beforeFirstPrompt ? { beforeFirstPrompt } : {}),
      }, input);
    } catch (e) {
      this.finishPendingStart(key);
      if (e instanceof InputOutcomeUnknown) {
        throw rpc("outcome_unknown", `${e.message} Check whether the worker started before trying again.`);
      }
      throw rpc("provider_unavailable", `${instance.descriptor.displayName} could not start the thread: ${(e as Error).message}`);
    }
    const rt = this.createThread(
      instance,
      started.thread,
      {
        cwd: p.cwd,
        originKind: "yaac",
        title: p.title,
        preview: "",
        model: p.model ?? null,
        uploadsKey,
        spawnedBy: p.spawnedBy,
        ...(p.agents ? { agents: p.agents } : {}),
      },
      true,
      launchStoreKey,
    );
    this.turnBegan(rt, started.turn, "mesh", p.parts, null, p.journalItems);
    this.trustNotice(rt, trusted);
    this.finishPendingStart(key);
    return { threadId: rt.id, turnId: started.turn };
  }

  private async workerLaunchCallback(
    parent: ThreadRuntime, instance: Instance, provider: HarnessProvider, storeKey: string | null,
  ): Promise<StartThreadParams["beforeFirstPrompt"]> {
    const hook = loadWorkerLaunchHook(this.opts.home, this.opts.env);
    if (!hook) return undefined;
    const source = this.provider(parent);
    if (!source.workerSessionId || !provider.workerSessionId) {
      throw new Error("The configured worker launch hook needs session identities from both providers.");
    }
    const parentSession = await source.workerSessionId(parent.record.native);
    const sourceRef: WorkerLaunchIdentity = {
      harness: parent.record.kind, instanceId: parent.record.instance,
      yaacThreadId: parent.id, nativeThreadId: parent.record.native,
      ...(parentSession === parent.record.native ? {} : { sessionId: parentSession }),
    };
    return (nativeThreadId, sessionId) => hook({
      event: "worker.beforeFirstPrompt",
      parent: sourceRef,
      child: {
        harness: instance.config.kind, instanceId: instance.config.id,
        yaacThreadId: threadIdFor(instance.config.kind, storeKey, instance.config.id, nativeThreadId),
        nativeThreadId,
        ...(sessionId === nativeThreadId ? {} : { sessionId }),
      },
    });
  }

  /** Interrupts a thread's running turn on an agent's behalf (`stop_thread`, own workers only). */
  async interruptForAgent(threadId: string): Promise<boolean> {
    const rt = this.requireThread(threadId);
    if (!rt.activeTurn) return false;
    await this.provider(rt).interrupt(rt.record.native, rt.activeTurn);
    return true;
  }

  /** Whether the thread is idle and not known to be held elsewhere (no harness round trip). */
  dispatchableNow(rt: ThreadRuntime): boolean {
    return rt.canDispatch() && rt.queue.length === 0 && rt.userQueue.length === 0 && rt.pendingRequestCount() === 0;
  }

  /** Asks the provider who holds the thread (public for the mesh). */
  async checkExternal(threadId: string): Promise<ExternalControl> {
    return this.refreshExternal(this.requireThread(threadId));
  }

  // ── Turn control ───────────────────────────────────────────────────────────────────────

  async turnInterrupt(client: ClientLink, p: TurnInterruptParams): Promise<void> {
    const rt = this.requireThread(p.threadId);
    if (!rt.activeTurn || rt.activeTurn !== p.turnId) {
      throw rpc("turn_mismatch", `turn ${String(p.turnId)} is not the running turn`, rt.activeTurn ? { activeTurn: rt.activeTurn } : undefined);
    }
    this.checkWriter(rt, client, false);
    if (p.hold === true && !rt.record.hold) this.setHold(rt, { label: "the user", at: nowIso() });
    await this.provider(rt).interrupt(rt.record.native, p.turnId);
  }

  async turnSteer(client: ClientLink, p: TurnSteerParams): Promise<void> {
    await this.ops.run("turn/steer", p.opId, async () => {
      const rt = this.findThread(p.threadId);
      if (!rt || rt.deleted) throw notActed("thread_not_found", `no thread ${String(p.threadId)} on this target`);
      await this.refreshExternal(rt);
      this.checkWriter(rt, client, true);
      if (rt.compactDriver) throw notActed("turn_in_progress", "context is being compacted");
      const away = this.storeAway(rt);
      if (away) throw notActed("provider_unavailable", away);
      const provider = this.registry.get(rt.record.instance)?.provider;
      const eff = rt.effective ?? (await this.computeEffective(rt));
      if (!provider?.steer || !(eff.capabilities ?? []).includes("steer")) throw notActed("capability_missing", "this thread cannot be steered");
      if (!rt.activeTurn || rt.activeTurn !== p.turnId) {
        throw notActed("turn_mismatch", `turn ${String(p.turnId)} is not the running turn`, rt.activeTurn ? { activeTurn: rt.activeTurn } : undefined);
      }
      const parts = this.validateParts(p.input, eff.imageInput);
      this.mesh?.humanInput?.(rt);
      await provider.steer(rt.record.native, p.turnId, this.buildInput(rt.uploadsKey, parts, "user"));
      return {};
    });
  }

  // ── Requests ───────────────────────────────────────────────────────────────────────────

  private redact(rec: RequestRecord, response: RequestResponse | null | undefined): RequestResponse | null {
    if (!response) return null;
    if (response.requestKind !== "question" || rec.secretQuestions.length === 0) return clone(response);
    const out = clone(response);
    for (const q of rec.secretQuestions) if (out.answers[q]) out.answers[q] = { redacted: true };
    return out;
  }

  private setRequestState(rt: ThreadRuntime, rec: RequestRecord, state: RequestRecord["state"], resolution: RequestResolution | null): void {
    rec.state = state;
    if (resolution) rec.resolution = resolution;
    rt.saveRequests();
    const ev: ThreadEvent = { type: "requestChanged", itemId: rec.itemId, requestId: rec.requestId, state };
    if (resolution) ev.resolution = resolution;
    this.append(rt, ev, undefined, true);
    this.updateIndex(rt);
    this.activity.emit();
    if (rt.pendingRequestCount() === 0) this.mesh?.holdReleased?.(rt);
  }

  async requestRespond(client: ClientLink, p: RequestRespondParams): Promise<void> {
    await this.ops.run("request/respond", p.opId, async () => {
      const rt = this.findThread(p.threadId);
      if (!rt || rt.deleted) throw notActed("thread_not_found", `no thread ${String(p.threadId)} on this target`);
      const rec = rt.requests.get(p.requestId);
      if (!rec) throw notActed("request_not_found", `no request ${String(p.requestId)} in this thread`);
      // A read-only harness sub-agent's request (D162) is answered by whoever may write to its
      // parent: the sub-agent is held by the agent that started it, which is not the one asking
      // for its approval — the user is.
      const writer = this.subagentReadOnly(rt) ? this.subagentParent(rt) : rt;
      if (writer) {
        await this.refreshExternal(writer);
        this.checkWriter(writer, client, true, true);
      }
      if (rec.state !== "pending") throw notActed("request_not_pending", `the request is ${rec.state}`, { state: rec.state });
      if (!p.response || p.response.requestKind !== rec.requestKind) {
        throw notActed("invalid_params", `the answer must be a ${rec.requestKind} answer`);
      }
      // A sub-agent's answer is its parent's orchestration acted on.
      this.mesh?.humanInput?.(writer ?? rt);
      if (rec.requestKind === "meshPermission") {
        // yaacd's own request: answered by the mesh, never by the harness.
        if (!this.mesh) throw notActed("capability_missing", "the agent mesh is not available");
        await this.mesh.respond(rt, rec, p.response, client);
        return {};
      }
      let provider: HarnessProvider;
      try {
        provider = this.provider(rt);
      } catch (e) {
        throw new NotActed(e as RpcError);
      }
      rec.submittedBy = { key: client.key, host: client.host };
      // Journaled (without the answer) before it is handed over.
      this.setRequestState(rt, rec, "submitted", null);
      try {
        await provider.respond(rt.record.native, rec.requestId, p.response);
      } catch (e) {
        if ((rec.state as string) === "submitted") this.setRequestState(rt, rec, "unknown", null);
        throw rpc("provider_unavailable", `the harness did not take the answer: ${(e as Error).message}`);
      }
      return {};
    });
  }

  // ── Fork (docs/05-threads.md#fork) ───────────────────────────────────────────────────

  /**
   * Native or handoff, from the source and destination pair: native when the destination is
   * the same kind, shares the source's conversation store, and the source can fork natively.
   * Everything else is a handoff, which needs only that the source can read its history.
   */
  private forkPlan(src: ThreadRuntime, instanceId: string): { kind: "native"; dest: Instance } | { kind: "handoff"; dest: Instance; unavailable: string | null } {
    const dest = this.registry.get(instanceId);
    if (!dest) throw rpc("instance_not_found", `no provider instance ${instanceId} on this target`);
    if (src.record.metadataPending) return { kind: "handoff", dest, unavailable: METADATA_PENDING };
    const srcInst = this.registry.get(src.record.instance);
    const sameStore =
      !!srcInst &&
      dest.config.kind === srcInst.config.kind &&
      !!dest.descriptor.store.key &&
      dest.descriptor.store.key === (srcInst.descriptor.store.key ?? null) &&
      src.record.storeKey === dest.descriptor.store.key;
    const can = sameStore && typeof srcInst?.provider?.forkThread === "function" && (srcInst.descriptor.capabilities ?? []).includes("threadFork");
    if (can) return { kind: "native", dest };
    let unavailable: string | null = this.storeAway(src);
    if (unavailable) {
      // Its history is in a store the instance does not lead to now (P10, P300).
    } else if (!srcInst?.provider || !(srcInst.descriptor.capabilities ?? []).includes("threadRead")) {
      unavailable = "this thread's provider cannot read its history back, so YAAC will not fork it";
    } else if (!fs.existsSync(src.record.cwd)) {
      unavailable = `the working directory ${src.record.cwd} no longer exists on this target`;
    } else {
      try {
        this.registry.usable(instanceId);
      } catch (e) {
        unavailable = (e as Error).message;
      }
    }
    return { kind: "handoff", dest, unavailable };
  }

  private lastCompletedTurn(rt: ThreadRuntime): TurnId | null {
    const items = rt.projection?.items() ?? [];
    const turns = items.map((i) => i.turnId).filter((t) => t && t !== rt.activeTurn);
    return turns.at(-1) ?? null;
  }

  /**
   * The source's history from the harness's own store, cut at the last completed turn (or
   * `atTurn`), rendered to a transcript. `uploadsKey` (a fork, not a preview) links the
   * attachments still on the target into the new thread's folder so its agent can read them.
   */
  private async renderHandoff(src: ThreadRuntime, atTurn: TurnId | null, uploadsKey: string | null): Promise<{ cutoff: TurnId | null; rendered: RenderedTranscript; readable: string[] }> {
    const inst = this.registry.get(src.record.instance)!;
    const provider = inst.provider!;
    const items: Item[] = [];
    let cursor: string | null | undefined = null;
    try {
      for (let i = 0; i < 100000; i++) {
        const page = await provider.readThread(src.record.native, { cursor });
        items.push(...page.items);
        cursor = page.cursor;
        if (!cursor) break;
      }
    } catch (e) {
      throw rpc("provider_unavailable", `${inst.descriptor.displayName} could not read the thread's history: ${(e as Error).message}`);
    }
    const running = [src.activeTurn, await provider.runningTurn?.(src.record.native).catch(() => null)];
    const cutoff = cutoffTurn(items, running, atTurn);
    if (atTurn && !cutoff) throw notActed("invalid_params", `turn ${atTurn} is not in the thread's history`);
    const journaled = (await this.ensureProjection(src)).items();
    const readable = new Set<string>();
    const look: AttachmentLookup = {
      upload: (id) => {
        if (!this.uploads.present(id)) return null;
        const meta = this.uploads.meta(id)!;
        if (!uploadsKey) return { path: meta.name, name: meta.name };
        const l = this.uploads.linkForThread(uploadsKey, id);
        readable.add(path.dirname(l.path));
        return { path: l.path, name: meta.name };
      },
      exists: (p) => fs.existsSync(p),
    };
    const where = this.mesh?.whereIs?.(src);
    const rendered = renderTranscript(
      itemsThrough(items, cutoff),
      { title: this.summaryOf(src).title, provider: inst.descriptor.displayName, target: where?.target ?? "this machine", threadId: src.id },
      look,
      journaled,
    );
    return { cutoff, rendered, readable: [...readable] };
  }

  async forkPreview(p: ThreadForkPreviewParams): Promise<ThreadForkPreviewResult> {
    const src = this.requireThread(p.source);
    await this.ensureProjection(src);
    const plan = this.forkPlan(src, p.instance);
    const account = plan.dest.descriptor.store.account ?? null;
    if (plan.kind === "native") {
      const out: ThreadForkPreviewResult = { forkKind: "native", cutoffTurn: p.atTurn ?? this.lastCompletedTurn(src) };
      if (account) out.destinationAccount = account;
      return out;
    }
    if (plan.unavailable) return { forkKind: "handoff", unavailableReason: plan.unavailable };
    const { cutoff, rendered } = await this.renderHandoff(src, p.atTurn ?? null, null);
    const max = Math.max(256, Math.min(Number(p.maxPreviewBytes ?? 4096) || 4096, PAGE_MAX_BYTES));
    const out: ThreadForkPreviewResult = {
      forkKind: "handoff",
      transcriptBytes: Buffer.byteLength(rendered.markdown),
      transcriptPreview: previewOf(rendered.markdown, max),
      omittedAttachments: rendered.omitted,
    };
    if (cutoff) out.cutoffTurn = cutoff;
    else out.unavailableReason = "the thread has no completed turn to fork from yet";
    if (account) out.destinationAccount = account;
    return out;
  }

  async threadFork(client: ClientLink, p: ThreadForkParams): Promise<ThreadForkResult> {
    return this.ops.run("thread/fork", p.opId, async () => {
      const src = this.findThread(p.source);
      if (!src || src.deleted) throw notActed("thread_not_found", `no thread ${String(p.source)} on this target`);
      let plan;
      try {
        plan = this.forkPlan(src, p.instance);
      } catch (e) {
        throw new NotActed(e as RpcError);
      }
      if (plan.kind === "handoff") return this.handoffFork(client, p, src, plan.dest, plan.unavailable);
      const { rt, parts } = await this.forkInto(p, true);
      const turnId = await this.submit(rt, {
        origin: "user",
        parts: parts!,
        overrides: { model: p.model ?? null, effort: p.effort ?? null, mode: p.mode ?? null },
        driver: { key: client.key, host: client.host },
      }, "now");
      return { threadId: rt.id, turnId, forkKind: "native" as const };
    });
  }

  /**
   * `thread/forkOnly`: the fork, idle, with no first message. Native only: a handoff creates
   * the destination conversation with its first message (the preamble goes in front of it),
   * so a handoff is `thread/fork`.
   */
  async threadForkOnly(p: ThreadForkOnlyParams): Promise<ThreadForkOnlyResult> {
    return this.ops.run("thread/forkOnly", p.opId, async () => {
      const { rt } = await this.forkInto({ ...p, input: [] }, false);
      return { threadId: rt.id, forkKind: "native" as const };
    });
  }

  private retainedForkChildren(src: ThreadRuntime, requested: unknown): string[] | undefined {
    if (requested == null || requested === false) return undefined;
    if (requested !== true) throw notActed("invalid_params", "keepChildren must be a boolean");
    const parent = this.addressHere(src.id);
    const linked = new Set([...(src.record.linkedChildren ?? []), ...(this.mesh?.childrenOf?.(src) ?? [])]);
    for (const thread of this.threads.values()) {
      if (thread.deleted || thread === src) continue;
      if (thread.record.spawnedBy === parent || this.subagentParent(thread) === src) {
        linked.add(this.addressHere(thread.id));
      }
    }
    linked.delete(parent);
    return linked.size > 0 ? [...linked] : undefined;
  }

  private async forkInto(p: ThreadForkParams, withInput: boolean): Promise<{ rt: ThreadRuntime; parts: InputPart[] | null }> {
    const src = this.findThread(p.source);
    if (!src || src.deleted) throw notActed("thread_not_found", `no thread ${String(p.source)} on this target`);
    let plan;
    try {
      plan = this.forkPlan(src, p.instance);
    } catch (e) {
      throw new NotActed(e as RpcError);
    }
    if (plan.kind === "handoff") {
      throw notActed("capability_missing", "a handoff fork starts with its first message: send it with thread/fork");
    }
    const srcInst = this.registry.get(src.record.instance)!;
    if (p.atTurn && !(srcInst.descriptor.capabilities ?? []).includes("threadForkAtTurn")) {
      throw notActed("capability_missing", "this provider forks only from the last completed turn");
    }
    this.checkModel(plan.dest.descriptor, p.model);
    const parts = withInput ? this.validateParts(p.input, this.modelImageSupport(plan.dest.descriptor, p.model)) : null;
    const linkedChildren = this.retainedForkChildren(src, p.keepChildren);
    const trusted = await this.trustProjectFolder(plan.dest, src.record.cwd);
    let native: NativeId;
    try {
      native = await srcInst.provider!.forkThread!(src.record.native, {
        atTurn: p.atTurn ?? null,
        cwd: src.record.cwd,
        model: p.model ?? null,
        effort: p.effort ?? null,
        mode: p.mode ?? null,
      });
    } catch (e) {
      throw rpc("provider_unavailable", `the harness did not fork the thread: ${(e as Error).message}`);
    }
    const rt = this.createThread(
      plan.dest,
      native,
      {
        cwd: src.record.cwd,
        originKind: "yaac",
        forkedFrom: src.id,
        ...(linkedChildren && { linkedChildren }),
        title: `${src.record.titleOverride ?? src.record.title ?? "Thread"} (fork)`,
        model: p.model ?? src.record.model ?? null,
      },
      false,
    );
    const cutoff = p.atTurn ?? this.lastCompletedTurn(src);
    const handoff: Item = { id: `fork:${src.id}`, turnId: cutoff ?? "", status: "completed", kind: "handoff", forkKind: "native", forkedFrom: src.id, cutoffTurn: cutoff, startedAt: nowIso(), completedAt: nowIso() };
    this.append(rt, { type: "itemStarted", item: handoff });
    this.append(rt, { type: "itemCompleted", item: handoff });
    this.trustNotice(rt, trusted);
    return { rt, parts };
  }

  /**
   * A handoff fork: the transcript is rendered from the harness's store and written to
   * `~/.yaac/handoffs/` before anything is sent; the destination conversation is created with
   * the fixed preamble in front of the user's first message; the new thread journals the
   * `handoff` item (the frozen transcript) before its first turn.
   */
  private async handoffFork(client: ClientLink, p: ThreadForkParams, src: ThreadRuntime, dest: Instance, unavailable: string | null): Promise<ThreadForkResult> {
    if (unavailable) throw notActed("capability_missing", unavailable);
    const { provider } = this.registry.usable(dest.config.id);
    this.checkModel(dest.descriptor, p.model);
    const parts = this.validateParts(p.input, this.modelImageSupport(dest.descriptor, p.model));
    const linkedChildren = this.retainedForkChildren(src, p.keepChildren);
    // The new thread's attachment folder is keyed by this operation, for its whole life.
    const uploadsKey = `fork:${p.opId}`;
    const { cutoff, rendered, readable } = await this.renderHandoff(src, p.atTurn ?? null, uploadsKey);
    if (!cutoff) throw notActed("invalid_params", "the thread has no completed turn to fork from yet");
    const file = path.join(this.opts.home, "handoffs", sha256Hex(uploadsKey).slice(0, 24), "transcript.md");
    ensureDir(path.dirname(file));
    fs.writeFileSync(file, rendered.markdown, { mode: 0o600 });
    const where = this.mesh?.whereIs?.(src);
    const meshTools = (dest.descriptor.capabilities ?? []).includes("meshTools");
    const input = this.buildInput(uploadsKey, parts, "user");
    input.text = preamble(file, { target: where?.target ?? "this machine", threadId: src.id, address: where?.address ?? null }, meshTools) + input.text;
    input.readable = [path.dirname(file), ...readable];
    const trusted = await this.trustProjectFolder(dest, src.record.cwd);
    const key = dest.config.id;
    this.pendingStarts.set(key, (this.pendingStarts.get(key) ?? 0) + 1);
    let started: { thread: NativeId; turn: TurnId };
    try {
      started = await provider.startThread({ cwd: src.record.cwd, model: p.model ?? null, effort: p.effort ?? null, mode: p.mode ?? null }, input);
    } catch (e) {
      this.finishPendingStart(key);
      throw rpc("provider_unavailable", `${dest.descriptor.displayName} could not start the thread: ${(e as Error).message}`);
    }
    const title = `${this.summaryOf(src).title} (fork)`;
    const firstText = parts.find((x) => x.type === "text") as { text?: string } | undefined;
    const rt = this.createThread(
      dest,
      started.thread,
      {
        cwd: src.record.cwd,
        originKind: "yaac",
        forkedFrom: src.id,
        ...(linkedChildren && { linkedChildren }),
        handoffTranscript: file,
        title,
        // The harness titles the conversation from its first message, which starts with the
        // preamble: YAAC names it after its source instead.
        titleOverride: title,
        preview: (firstText?.text ?? "").slice(0, 120),
        model: p.model ?? null,
        uploadsKey,
      },
      true,
    );
    const at = nowIso();
    const handoff: Item = {
      id: `fork:${src.id}`,
      turnId: "",
      status: "completed",
      kind: "handoff",
      forkKind: "handoff",
      forkedFrom: src.id,
      cutoffTurn: cutoff,
      transcript: boundTranscript(rendered.markdown, HANDOFF_ITEM_BYTES, file),
      transcriptPath: file,
      startedAt: at,
      completedAt: at,
    };
    this.append(rt, { type: "itemStarted", item: handoff });
    this.append(rt, { type: "itemCompleted", item: handoff });
    this.turnBegan(rt, started.turn, "user", parts, { key: client.key, host: client.host }, undefined, p.opId);
    this.trustNotice(rt, trusted);
    this.finishPendingStart(key);
    if (typeof provider.rename === "function" && (dest.descriptor.capabilities ?? []).includes("threadRename")) {
      void provider.rename(started.thread, title).catch((e: Error) => this.opts.log(`naming the fork ${rt.id} failed: ${e.message}`));
    }
    return { threadId: rt.id, turnId: started.turn, forkKind: "handoff" as const };
  }

  // ── Rename, archive, delete ────────────────────────────────────────────────────────────

  private async gated(rt: ThreadRuntime, cap: Capability, method: "rename" | "archive" | "remove"): Promise<HarnessProvider> {
    const readOnly = rt.record.metadataPending ? METADATA_PENDING : this.subagentReadOnly(rt);
    if (readOnly) throw notActed("thread_in_use", readOnly);
    const away = this.storeAway(rt);
    if (away) throw notActed("provider_unavailable", away);
    const inst = this.registry.get(rt.record.instance);
    const provider = inst?.provider;
    if (!provider || typeof provider[method] !== "function" || !(inst!.descriptor.capabilities ?? []).includes(cap)) {
      throw notActed("capability_missing", `this thread's provider cannot ${method === "remove" ? "delete" : method} threads`);
    }
    return provider;
  }

  async threadRename(p: { opId: string; threadId: string; title: string }): Promise<void> {
    await this.ops.run("thread/rename", p.opId, async () => {
      const rt = this.findThread(p.threadId);
      if (!rt || rt.deleted) throw notActed("thread_not_found", `no thread ${String(p.threadId)}`);
      if (typeof p.title !== "string" || !p.title.trim()) throw notActed("invalid_params", "title is required");
      const provider = await this.gated(rt, "threadRename", "rename");
      await provider.rename!(rt.record.native, p.title);
      rt.record.titleOverride = p.title;
      rt.record.updatedAt = nowIso();
      rt.saveRecord();
      this.updateIndex(rt);
      return {};
    });
  }

  async threadArchive(p: { opId: string; threadId: string }): Promise<void> {
    await this.ops.run("thread/archive", p.opId, async () => {
      const rt = this.findThread(p.threadId);
      if (!rt || rt.deleted) throw notActed("thread_not_found", `no thread ${String(p.threadId)}`);
      const provider = await this.gated(rt, "threadArchive", "archive");
      await provider.archive!(rt.record.native);
      rt.record.archived = true;
      rt.record.userArchived = true;
      rt.saveRecord();
      this.updateIndex(rt);
      return {};
    });
  }

  /** `thread/unarchive`, with the harness's own unarchive (under `threadArchive`). */
  async threadUnarchive(p: { opId: string; threadId: string }): Promise<void> {
    await this.ops.run("thread/unarchive", p.opId, async () => {
      const rt = this.findThread(p.threadId);
      if (!rt || rt.deleted) throw notActed("thread_not_found", `no thread ${String(p.threadId)}`);
      const away = this.storeAway(rt);
      if (away) throw notActed("provider_unavailable", away);
      const inst = this.registry.get(rt.record.instance);
      const provider = inst?.provider;
      if (!provider || typeof provider.unarchive !== "function" || !(inst!.descriptor.capabilities ?? []).includes("threadArchive")) {
        throw notActed("capability_missing", "this thread's provider cannot unarchive threads");
      }
      await provider.unarchive(rt.record.native);
      rt.record.archived = false;
      rt.record.userArchived = false;
      rt.saveRecord();
      this.updateIndex(rt);
      return {};
    });
  }

  /**
   * `thread/delete` (D110). YAAC's own record and journal always go. With `alsoInHarness`
   * (under `threadDelete`) the harness is asked to delete its session first; if that fails
   * nothing is removed. Without it the harness keeps the session, and discovery leaves it
   * out from then on (an explicit `thread/lookup` by id brings it back).
   */
  async threadDelete(client: ClientLink, p: { opId: string; threadId: string; alsoInHarness?: boolean }): Promise<void> {
    await this.ops.run("thread/delete", p.opId, async () => {
      const rt = this.findThread(p.threadId);
      if (!rt || rt.deleted) throw notActed("thread_not_found", `no thread ${String(p.threadId)}`);
      await this.refreshExternal(rt);
      this.checkWriter(rt, client, true);
      if (rt.activeTurn || rt.compactDriver) throw notActed("turn_in_progress", "stop the running turn before deleting the thread");
      if (p.alsoInHarness === true) {
        const provider = await this.gated(rt, "threadDelete", "remove");
        await provider.remove!(rt.record.native);
      } else {
        this.forgotten.add(this.nativeKey(rt.record.instance, rt.record.native));
        writeJsonAtomic(this.forgottenFile, { natives: [...this.forgotten] });
      }
      this.forget(rt);
      return {};
    });
  }

  private isForgotten(instanceId: string, native: string): boolean {
    return this.forgotten.has(this.nativeKey(instanceId, native));
  }

  /** An explicit lookup found a forgotten session: it is wanted again. */
  private remember(instanceId: string, native: string): void {
    if (this.forgotten.delete(this.nativeKey(instanceId, native))) writeJsonAtomic(this.forgottenFile, { natives: [...this.forgotten] });
  }

  private forget(rt: ThreadRuntime): void {
    this.mesh?.threadRemoved(rt);
    rt.deleted = true;
    for (const w of rt.queue.splice(0)) w.reject(rpc("thread_not_found", "the thread was deleted"));
    if (rt.flushTimer) clearTimeout(rt.flushTimer);
    rt.journal?.destroy();
    fs.rmSync(rt.dir, { recursive: true, force: true });
    this.uploads.removeThread(rt.uploadsKey);
    // A handoff transcript is pinned to its thread: it goes with it.
    if (rt.record.handoffTranscript) fs.rmSync(path.dirname(rt.record.handoffTranscript), { recursive: true, force: true });
    this.threads.delete(rt.id);
    this.byNative.delete(rt.record.instance, rt.record.native, rt.id);
    if (this.index.remove(rt.id)) this.broadcast("thread/removed", { threadId: rt.id, token: this.index.token });
    this.activity.emit();
  }

  // ── Provider events ────────────────────────────────────────────────────────────────────

  private needsMetadata(rt: ThreadRuntime): boolean {
    const r = rt.record;
    // The old event fallback used the daemon home and no title for an interactive thread.
    return r.metadataPending ?? (r.kind === "codex" && r.originKind === "interactive" && r.cwd === this.opts.home && !r.title);
  }

  private metadataFailed(rt: ThreadRuntime): boolean {
    return this.metadataReads.get(rt)?.failedAt !== undefined;
  }

  private recoverMetadata(instance: Instance, rt: ThreadRuntime, force = false): Promise<void> | undefined {
    const provider = instance.provider;
    if (!provider?.lookupThread) return;
    const state = this.metadataReads.get(rt) ?? { pending: null };
    if (state.pending) return state.pending;
    // A notification can precede the initial rollout write. A later observation can retry a
    // failed read without issuing one RPC for every item or delta in the meantime.
    if (!force && state.failedAt !== undefined && Date.now() - state.failedAt < 1000) return;
    const store = instance.descriptor.store.key;
    const native = rt.record.native;
    const stillPending = () => !rt.deleted && instance.provider === provider && instance.descriptor.store.key === store && this.needsMetadata(rt);
    state.pending = Promise.resolve().then(async () => {
      if (!stillPending()) return;
      const info = await provider.lookupThread!(native);
      if (!stillPending()) return;
      if (info?.native === native && info.cwd) this.discovered(instance, info);
      else {
        state.failedAt = Date.now();
        this.updateIndex(rt);
      }
    }).catch((e) => {
      this.opts.log(`metadata for ${native} from ${instance.config.id} failed: ${(e as Error).message}`);
      if (stillPending()) {
        state.failedAt = Date.now();
        this.updateIndex(rt);
      }
    }).finally(() => {
      state.pending = null;
    });
    this.metadataReads.set(rt, state);
    return state.pending;
  }

  private onProviderEvent(instanceId: string, ev: ProviderEvent): void {
    try {
      this.handleProviderEvent(instanceId, ev);
    } catch (e) {
      this.opts.log(`provider event ${ev.type} from ${instanceId} failed: ${(e as Error).stack ?? e}`);
    }
  }

  private handleProviderEvent(instanceId: string, ev: ProviderEvent): void {
    const instance = this.registry.get(instanceId);
    if (!instance) return;
    // Not about a thread: the registry's listeners (re-probe; rate limits, session/ratelimits.ts) take these.
    if (ev.type === "descriptorChanged" || ev.type === "rateLimitsChanged") return;
    if (ev.type === "threadChanged") {
      if (this.pendingStarts.has(instanceId) && !this.byNative.has(instanceId, ev.info.native, this.storeNow(instanceId))) {
        this.bufferEvent(instanceId, ev);
        return;
      }
      const known = this.threadByNative(instanceId, ev.info.native);
      if (!known && this.isForgotten(instanceId, ev.info.native)) return;
      if (!known && this.caps.autoDiscover === false
        && !(ev.info.parent && this.threadByNative(instanceId, ev.info.parent))) return;
      this.discovered(instance, ev.info);
      return;
    }
    if (ev.type === "threadRemoved") {
      const id = this.byNative.get(instanceId, ev.thread, this.storeNow(instanceId));
      const rt = id ? this.threads.get(id) : undefined;
      if (rt) this.forget(rt);
      return;
    }
    const id = this.byNative.get(instanceId, ev.thread, this.storeNow(instanceId));
    let rt = id ? this.threads.get(id) : undefined;
    if (!rt) {
      if (this.pendingStarts.has(instanceId)) {
        this.bufferEvent(instanceId, ev);
        return;
      }
      // An observation alone does not import new external sessions when discovery is off.
      if (ev.type === "threadObserved" && this.caps.autoDiscover === false) return;
      // A conversation the user deleted from YAAC only stays out.
      if (this.isForgotten(instanceId, ev.thread)) return;
      if (instance.config.kind === "codex") {
        // Events and approvals must reach the journal even if we do not yet know how to file
        // the conversation. In particular an externally spawned sub-agent can arrive here.
        rt = this.createThread(instance, ev.thread, { cwd: "", originKind: "programmatic", metadataPending: true }, false);
      } else {
        rt = this.discovered(instance, { native: ev.thread, cwd: this.opts.home, createdAt: nowIso(), updatedAt: nowIso(), originKind: "interactive" });
      }
    }
    if (rt.deleted) return;
    if (this.needsMetadata(rt)) void this.recoverMetadata(instance, rt);
    switch (ev.type) {
      case "threadObserved":
        return;
      case "runningCommands": {
        const changed = !!rt.runningCommands.length !== !!ev.commands.length;
        rt.runningCommands = ev.commands;
        rt.commandController = ev.controller;
        this.updateIndex(rt);
        if (changed) this.activity.emit();
        return;
      }
      case "turnStarted": {
        if (rt.activeTurn === ev.turnId) return;
        if (rt.expectTurn && (!ev.origin || ev.origin === rt.expectTurn.origin)) {
          const x = rt.expectTurn;
          this.turnBegan(rt, ev.turnId, x.origin, x.parts, x.driver, x.items, x.opId);
          return;
        }
        rt.activeTurn = ev.turnId;
        rt.turnOrigin = ev.origin ?? "external";
        rt.driver = ev.origin === "user" ? rt.compactDriver : null;
        rt.lastStatus = null;
        rt.saveRuntime();
        this.append(rt, { type: "turnStarted", turnId: ev.turnId, origin: rt.turnOrigin, at: ev.at ?? nowIso() });
        if (rt.driver) this.appendControl(rt);
        this.updateIndex(rt);
        this.activity.emit();
        this.mesh?.turnStarted(rt, ev.turnId, rt.turnOrigin);
        return;
      }
      case "turnCompleted": {
        this.flushDeltas(rt);
        for (const r of rt.requests.values()) {
          if (r.turnId !== ev.turnId) continue;
          // A mesh permission prompt is yaacd's own, not the harness's: it outlives the turn.
          if (r.requestKind === "meshPermission") continue;
          if (r.state === "pending") this.setRequestState(rt, r, "expired", null);
          else if (r.state === "submitted") this.setRequestState(rt, r, "unknown", null);
        }
        const status = rt.interruptReason === "update" && ev.status === "interrupted" ? "interruptedByUpdate" : ev.status;
        if (status === "interruptedByUpdate") {
          const notice: Item = { id: `notice:update:${ev.turnId}`, turnId: ev.turnId, status: "completed", kind: "notice", noticeKind: "interruptedByUpdate", text: "This turn was interrupted because yaacd was updated." };
          this.append(rt, { type: "itemStarted", item: notice });
          this.append(rt, { type: "itemCompleted", item: notice });
        }
        const done: ThreadEvent = { type: "turnCompleted", turnId: ev.turnId, status, at: ev.at ?? nowIso() };
        if (ev.error) done.error = ev.error;
        if (ev.usage) done.usage = ev.usage;
        this.append(rt, done);
        const hadDriver = !!rt.driver;
        if (rt.activeTurn === ev.turnId || rt.activeTurn) {
          rt.activeTurn = null;
          rt.turnOrigin = null;
          rt.driver = null;
        }
        rt.interruptReason = null;
        rt.lastStatus = status;
        rt.waiting.clear();
        rt.saveRuntime();
        if (hadDriver) this.appendControl(rt);
        this.journalOf(rt).compact();
        rt.record.updatedAt = nowIso();
        rt.saveRecord();
        this.updateIndex(rt);
        this.activity.emit();
        this.mesh?.turnCompleted(rt, ev.turnId, status, ev.error ?? null);
        if (rt.canDispatch()) this.afterIdle(rt);
        // A turn ended in this checkout: its git status may have changed.
        if (rt.record.repo) void this.gitStatus.refresh(rt.record.repo.root, true);
        return;
      }
      case "itemStarted":
      case "itemUpdated":
      case "itemCompleted": {
        const item = this.boundItem(rt, clone(ev.item), ev.type === "itemCompleted");
        if (item.kind === "agentMessage" && ev.type === "itemCompleted" && item.text) {
          rt.record.preview = item.text.slice(0, 120);
        }
        this.stampItem(rt, item, ev.type === "itemCompleted");
        const wasWaiting = rt.waiting.size > 0;
        if (item.kind === "meshWait" && item.status === "inProgress" && ev.type !== "itemCompleted") rt.waiting.add(item.id);
        else rt.waiting.delete(item.id);
        this.append(rt, { type: ev.type, item }, undefined, item.kind === "notice" && item.id.startsWith("notice:command:"));
        if (wasWaiting !== rt.waiting.size > 0) this.updateIndex(rt);
        if (ev.type === "itemCompleted") this.mesh?.itemCompleted(rt, item);
        return;
      }
      case "itemDelta": {
        if (ev.field === "output") {
          const kept = this.trackOutput(rt, ev.itemId, ev.delta);
          if (kept) this.delta(rt, ev.itemId, "output", kept);
          return;
        }
        if (ev.field === "text" || ev.field === "reasoning") this.delta(rt, ev.itemId, ev.field, ev.delta);
        return;
      }
      case "requestRaised": {
        const req = ev.request;
        const itemId = `request:${ev.requestId}`;
        const secret = req.requestKind === "question" ? (req.questions ?? []).filter((q) => q.secret).map((q) => q.id) : [];
        const rec: RequestRecord = { requestId: ev.requestId, itemId, turnId: ev.turnId, requestKind: req.requestKind, state: "pending", secretQuestions: secret };
        rt.requests.set(ev.requestId, rec);
        // Journaled — durably — before anything else happens to it.
        rt.saveRequests();
        const item = { id: itemId, turnId: ev.turnId, status: "inProgress", startedAt: nowIso(), kind: "request", requestId: ev.requestId, state: "pending", blocking: ev.blocking ?? true, ...req } as Item;
        this.append(rt, { type: "itemStarted", item }, undefined, true);
        this.updateIndex(rt);
        this.activity.emit();
        this.mesh?.requestRaised(rt, rec);
        return;
      }
      case "requestResolved": {
        const rec = rt.requests.get(ev.requestId);
        if (!rec || rec.state === "resolved" || rec.state === "expired") return;
        const resolution: RequestResolution = { by: "elsewhere", at: nowIso() };
        if (ev.by === "client" && rec.submittedBy) {
          resolution.by = "client";
          resolution.clientId = rec.submittedBy.key;
          if (rec.submittedBy.host) resolution.clientHost = rec.submittedBy.host;
        }
        const answer = this.redact(rec, ev.answer);
        if (answer) resolution.answer = answer;
        this.setRequestState(rt, rec, "resolved", resolution);
        return;
      }
      case "requestExpired": {
        const rec = rt.requests.get(ev.requestId);
        if (rec && (rec.state === "pending" || rec.state === "submitted")) this.setRequestState(rt, rec, "expired", null);
        return;
      }
      case "controlChanged":
        this.setExternal(rt, ev.control);
        return;
      case "fastRefused": {
        // The harness would not run it fast: the setting goes off, so the toggle shows off
        // and the next turns are not asked for it again (D133). The notice says why.
        if (rt.record.fast) {
          delete rt.record.fast;
          rt.saveRecord();
          this.updateIndex(rt);
        }
        return;
      }
      case "settingsChanged": {
        if (ev.applied.model) rt.record.model = ev.applied.model;
        void this.computeEffective(rt).then((effective) => {
          if (!rt.deleted) this.append(rt, { type: "settingsChanged", effective: { ...effective, applied: { ...effective.applied, ...ev.applied } } });
        });
        return;
      }
      case "usageUpdated": {
        const u: ThreadEvent = { type: "usageUpdated" };
        if (ev.tokens) u.tokens = rt.tokens = ev.tokens;
        if (ev.context) u.context = rt.context = ev.context;
        this.append(rt, u);
        return;
      }
    }
  }

  /** Item timestamps: when it appeared, and when it reached a final state ("Thought for 4s"). */
  private stampItem(rt: ThreadRuntime, item: Item, final: boolean): void {
    const now = nowIso();
    const known = rt.itemStarts.get(item.id) ?? (rt.projection?.get(item.id) as Item | undefined)?.startedAt ?? null;
    if (!item.startedAt) {
      if (known) item.startedAt = known;
      else if (!final) item.startedAt = now;
    }
    const finished = final || item.status === "completed" || item.status === "failed" || item.status === "declined";
    if (finished) {
      if (!item.completedAt) item.completedAt = now;
      rt.itemStarts.delete(item.id);
    } else if (item.startedAt) {
      rt.itemStarts.set(item.id, item.startedAt);
    }
  }

  private bufferEvent(instanceId: string, ev: ProviderEvent): void {
    const list = this.buffered.get(instanceId) ?? [];
    list.push(ev);
    this.buffered.set(instanceId, list);
  }

  // ── Bounded tool output ────────────────────────────────────────────────────────────────

  private outputFile(rt: ThreadRuntime, itemId: string): string {
    return path.join(rt.dir, "outputs", `${sha256Hex(itemId).slice(0, 32)}.out`);
  }

  /** Records streamed output; returns the part that still fits the inline head. */
  private trackOutput(rt: ThreadRuntime, itemId: string, delta: string): string {
    let t = rt.outputs.get(itemId);
    if (!t) {
      const file = this.outputFile(rt, itemId);
      ensureDir(path.dirname(file));
      fs.writeFileSync(file, "", { mode: 0o600 });
      t = { file, bytes: 0, headBytes: 0, truncated: false };
      rt.outputs.set(itemId, t);
    }
    fs.appendFileSync(t.file, delta);
    t.bytes += Buffer.byteLength(delta);
    if (t.headBytes >= OUTPUT_HEAD_BYTES) {
      t.truncated = true;
      return "";
    }
    const kept = utf8Head(delta, OUTPUT_HEAD_BYTES - t.headBytes);
    t.headBytes += Buffer.byteLength(kept);
    if (kept.length < delta.length) t.truncated = true;
    return kept;
  }

  /** Bounds a tool call's output: a head, its true size, and the rest kept for reading on demand. */
  private boundItem(rt: ThreadRuntime, item: Item, final: boolean): Item {
    if (item.kind !== "toolCall") return item;
    const track = rt.outputs.get(item.id);
    const full = typeof item.output === "string" ? item.output : null;
    const size = full !== null ? Buffer.byteLength(full) : track?.bytes ?? 0;
    if (full !== null && size > OUTPUT_HEAD_BYTES) {
      const file = this.outputFile(rt, item.id);
      ensureDir(path.dirname(file));
      fs.writeFileSync(file, full, { mode: 0o600 });
      item.output = utf8Head(full, OUTPUT_HEAD_BYTES);
      item.outputTruncated = true;
      item.outputBytes = size;
    } else if (full === null && track) {
      const head = fs.readFileSync(track.file, "utf8");
      item.output = utf8Head(head, OUTPUT_HEAD_BYTES);
      item.outputTruncated = track.truncated;
      item.outputBytes = track.bytes;
    } else if (full !== null && track && track.truncated) {
      // The final item carries less than was streamed: keep what was streamed.
      item.outputTruncated = true;
      item.outputBytes = Math.max(size, track.bytes);
    }
    if (final) {
      rt.outputs.delete(item.id);
      if (!item.outputTruncated) fs.rmSync(this.outputFile(rt, item.id), { force: true });
    }
    return item;
  }

  // ── Hand-over and shutdown ─────────────────────────────────────────────────────────────

  /** "Update now": interrupt every running turn, marked as interrupted by an update. */
  async interruptAllForUpdate(timeoutMs = 2000): Promise<void> {
    const running = this.allThreads().filter((rt) => rt.activeTurn);
    await Promise.all(
      running.map(async (rt) => {
        const turn = rt.activeTurn!;
        rt.interruptReason = "update";
        const provider = this.registry.get(rt.record.instance)?.provider;
        await Promise.race([provider?.interrupt(rt.record.native, turn).catch(() => {}), new Promise((r) => setTimeout(r, timeoutMs))]);
        if (rt.activeTurn === turn) {
          this.handleProviderEvent(rt.record.instance, { type: "turnCompleted", thread: rt.record.native, turnId: turn, status: "interrupted" });
        }
      }),
    );
  }

  diagnostics(extra: Array<{ title: string; lines: string[] }>): DiagnosticsGetResult {
    const sections = [...extra];
    for (const inst of this.registry.all()) {
      const d = inst.descriptor;
      const threads = this.allThreads().filter((t) => t.record.instance === inst.config.id).length;
      sections.push({
        title: d.displayName,
        lines: [
          `instance: ${d.instance} (${d.kind})`,
          `status: ${d.status}${d.statusMessage ? ` — ${d.statusMessage}` : ""}`,
          `backend: ${d.backend}`,
          `harness version: ${d.harnessVersion ?? "unknown"}`,
          `models: ${(d.models ?? []).length}${d.modelsComplete ? "" : " (incomplete listing)"}`,
          `threads: ${threads}${this.index.staleInstances().includes(d.instance) ? " (last scan incomplete)" : ""}`,
          `revision: ${d.revision}`,
        ],
      });
    }
    return { sections };
  }

  /**
   * Stops: journals flushed and closed, the index saved, providers disposed. Bounded: nothing
   * here may keep the process from exiting (idle exit, a signal). What must not be lost is on
   * disk before the waits that can be cut short — git status is only a cache, and a provider
   * that does not finish disposing has nothing yaacd still needs.
   */
  async close(): Promise<void> {
    if (this.gitTimer) clearInterval(this.gitTimer);
    this.gitTimer = null;
    if (!(await within(this.gitStatus.idle().catch(() => {}), CLOSE_GIT_WAIT_MS))) {
      this.opts.log(`stop: git status still computing after ${CLOSE_GIT_WAIT_MS} ms; not waiting for it`);
    }
    this.gitStatus.close();
    for (const rt of this.threads.values()) {
      this.flushDeltas(rt);
      rt.journal?.close();
    }
    this.index.save();
    this.ops.close();
    if (!(await within(this.registry.dispose().catch(() => {}), CLOSE_DISPOSE_WAIT_MS))) {
      this.opts.log(`stop: a provider had not finished closing after ${CLOSE_DISPOSE_WAIT_MS} ms; not waiting for it`);
    }
  }

  /** Expose for tests: the last journal sequence of a thread. */
  position(threadId: string): JournalPosition {
    const j = this.journalOf(this.requireThread(threadId));
    return { epoch: j.epoch, seq: j.headSeq };
  }
}

export { lastSeqOf, applyDelta };
