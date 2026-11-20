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
} from "../protocol/generated/types.js";
import { canonical } from "../protocol/json.js";
import { RpcError } from "../protocol/rpc.js";
import type { ExternalControl, HarnessProvider, NativeId, NativeThreadInfo, ProviderEvent, ProviderFactory, ProviderInput, TurnOverrides } from "../providers/types.js";
import { ProviderRegistry, type Instance } from "../providers/registry.js";
import { Emitter } from "../util/emitter.js";
import { ensureDir, nowIso, randomId, readJson, sha256Hex } from "../util/fsx.js";
import { OUTPUT_HEAD_BYTES, Projection, applyDelta, clone, utf8Head } from "./items.js";
import { lastSeqOf, type JournalEntry, type JournalOptions } from "./journal.js";
import { NotActed, OpLog } from "./ops.js";
import { ThreadIndex, type Holder } from "./thread-index.js";
import { ThreadRuntime, threadDirName, type Driver, type RequestRecord, type ThreadRecord, type Work } from "./thread.js";
import { UploadStore } from "./uploads.js";
import { normaliseRemote } from "./fsinfo.js";
import { GitStatusStore, gitRunner, resolveRepo, type GitRunner } from "./git.js";
import { ProjectStore, projectRefFor } from "./projects.js";
import { Limiter } from "../util/exec.js";

export const PAGE_MAX_BYTES = 64 * 1024;
export const ITEM_READ_MAX = 512 * 1024;
const SNAPSHOT_TTL_MS = 10 * 60 * 1000;
const SNAPSHOT_MAX = 64;

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
  readonly clients = new Set<ClientLink>();
  /** Fires when running turns or pending requests change (idle tracking, hand-over). */
  readonly activity = new Emitter<void>();
  /** Fires with a thread id whenever a thread becomes held by yaacd and idle. */
  readonly heldAndIdle = new Emitter<string>();
  private threads = new Map<string, ThreadRuntime>();
  private byNative = new Map<string, string>();
  private snapshots = new Map<string, Snapshot>();
  private pendingStarts = new Map<string, number>();
  private buffered = new Map<string, ProviderEvent[]>();
  private readonly threadsDir: string;
  private readonly git: GitRunner;
  private readonly gitLimiter: Limiter;
  private readonly repoLookups = new Map<string, Promise<RepoLocation | null>>();
  private gitTimer: NodeJS.Timeout | null = null;

  constructor(readonly opts: CoreOptions) {
    this.threadsDir = path.join(opts.home, "threads");
    ensureDir(this.threadsDir);
    this.registry = new ProviderRegistry(opts.home, opts.env, opts.log);
    for (const f of opts.factories) this.registry.registerKind(f);
    this.index = new ThreadIndex(opts.home, opts.cleanStart);
    this.ops = new OpLog(path.join(opts.home, "ops.jsonl"));
    this.uploads = new UploadStore(opts.tempDir);
    this.projects = new ProjectStore(path.join(opts.home, "projects.json"));
    this.git = gitRunner(opts.env);
    this.gitLimiter = new Limiter(Math.max(1, opts.gitConcurrency ?? 2));
    this.gitStatus = new GitStatusStore(path.join(opts.home, "git-status.json"), this.git, this.gitLimiter, (root) => this.onGitChanged(root));
    this.registry.onEvent((instance, event) => this.onProviderEvent(instance, event));
    this.registry.onDescriptor((d) => this.broadcast("provider/updated", { provider: d }));
  }

  // ── Start-up ───────────────────────────────────────────────────────────────────────────

  async start(): Promise<void> {
    this.loadThreads();
    this.registry.load();
    await this.registry.probe();
    await this.reconcileAfterRestart();
    await this.scanAll();
    void this.refreshProjectRepos();
    // Threads whose repository was never looked up (older records): in the background.
    for (const rt of this.allThreads()) if (!rt.record.repoResolved) void this.resolveProject(rt);
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
      this.byNative.set(`${record.instance}\u0000${record.native}`, record.id);
    }
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
        else if (r.state === "pending" && !survived) this.setRequestState(rt, r, "expired", null);
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

  /** Builds the index from one instance's listing. A failed or partial scan removes nothing. */
  async scanInstance(instance: Instance): Promise<void> {
    const provider = instance.provider;
    if (!provider || instance.descriptor.status !== "ready") return;
    if (!(instance.descriptor.capabilities ?? []).includes("threadList")) return;
    let cursor: string | null | undefined = null;
    let complete = true;
    try {
      for (let pages = 0; pages < 10000; pages++) {
        const page = await provider.scanThreads({ cursor, limit: 100 });
        for (const info of page.threads) this.discovered(instance, info);
        if (page.complete === false) complete = false;
        cursor = page.cursor;
        if (!cursor) break;
      }
    } catch (e) {
      this.opts.log(`scan of ${instance.config.id} failed: ${(e as Error).message}`);
      complete = false;
    }
    this.index.setStale(instance.config.id, !complete);
  }

  // ── Threads ────────────────────────────────────────────────────────────────────────────

  thread(id: string): ThreadRuntime | undefined {
    return this.threads.get(id);
  }

  requireThread(id: unknown): ThreadRuntime {
    const rt = typeof id === "string" ? this.threads.get(id) : undefined;
    if (!rt || rt.deleted) throw rpc("thread_not_found", `no thread ${String(id)} on this target`);
    return rt;
  }

  allThreads(): ThreadRuntime[] {
    return [...this.threads.values()].filter((t) => !t.deleted);
  }

  private nativeKey(instance: string, native: string): string {
    return `${instance}\u0000${native}`;
  }

  private createThread(instance: Instance, native: NativeId, fields: Partial<ThreadRecord> & { cwd: string; originKind: ThreadRecord["originKind"] }, fresh: boolean): ThreadRuntime {
    const storeKey = instance.descriptor.store.key ?? null;
    const id = threadIdFor(instance.config.kind, storeKey, instance.config.id, native);
    const existing = this.threads.get(id);
    if (existing && !existing.deleted) {
      if (existing.record.instance !== instance.config.id) {
        const also = new Set(existing.record.alsoVisibleTo ?? []);
        also.add(instance.config.id);
        existing.record.alsoVisibleTo = [...also];
        existing.saveRecord();
      }
      this.byNative.set(this.nativeKey(instance.config.id, native), id);
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
    this.byNative.set(this.nativeKey(instance.config.id, native), id);
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
    }
    return p;
  }

  /** Resolves a thread's repository location once; a vanished folder keeps what was known. */
  private async resolveProject(rt: ThreadRuntime, urgent = false): Promise<void> {
    if (rt.record.repoResolved) return;
    if (!fs.existsSync(rt.record.cwd)) return;
    const repo = await this.repoOf(rt.record.cwd, urgent);
    if (rt.deleted) return;
    rt.record.repo = repo;
    rt.record.repoResolved = true;
    rt.saveRecord();
    this.updateIndex(rt);
    // The first status of a checkout is computed once, in the background.
    if (repo && !this.gitStatus.has(repo.root)) void this.gitStatus.refresh(repo.root, urgent);
  }

  private onGitChanged(root: string): void {
    for (const rt of this.threads.values()) if (!rt.deleted && rt.record.repo?.root === root) this.updateIndex(rt);
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
      this.projectsChanged();
      return {};
    });
  }

  /** `thread/update`: YAAC's own flags of a thread. Stored on the target; pushed as `thread/updated`. */
  async threadUpdate(client: ClientLink, p: ThreadUpdateParams): Promise<ThreadUpdateResult> {
    return this.ops.run("thread/update", p.opId, async () => {
      const rt = this.threads.get(p.threadId);
      if (!rt || rt.deleted) throw notActed("thread_not_found", `no thread ${String(p.threadId)} on this target`);
      for (const k of ["muted", "pinned"] as const) {
        if (p[k] !== undefined && p[k] !== null && typeof p[k] !== "boolean") throw notActed("invalid_params", `${k} must be true or false`);
      }
      if (p.agents !== undefined && p.agents !== null && typeof p.agents !== "string") throw notActed("invalid_params", "agents must be a string");
      if (typeof p.muted === "boolean") rt.record.muted = p.muted;
      if (typeof p.pinned === "boolean") rt.record.pinned = p.pinned;
      if (typeof p.agents === "string") rt.record.agents = p.agents;
      rt.saveRecord();
      this.updateIndex(rt);
      return { thread: this.renderSummary(this.summaryOf(rt), this.holderOf(rt), client.key) };
    });
  }

  private discovered(instance: Instance, info: NativeThreadInfo): ThreadRuntime {
    const known = this.byNative.get(this.nativeKey(instance.config.id, info.native));
    let rt = known ? this.threads.get(known) : undefined;
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
        },
        false,
      );
    } else {
      // The registry, not the harness, says what YAAC started.
      const r = rt.record;
      r.title = info.title ?? r.title ?? null;
      r.preview = info.preview ?? r.preview ?? null;
      r.model = info.model ?? r.model ?? null;
      r.archived = info.archived ?? r.archived ?? false;
      if (info.updatedAt > r.updatedAt) r.updatedAt = info.updatedAt;
      rt.saveRecord();
    }
    this.updateIndex(rt);
    return rt;
  }

  /** The sidebar summary of a thread, with `control: "free"` (applied per client). */
  summaryOf(rt: ThreadRuntime): ThreadSummary {
    const r = rt.record;
    const state: ThreadSummary["state"] =
      rt.pendingRequestCount() > 0
        ? "needsAttention"
        : rt.waiting.size > 0
          ? "waiting"
          : rt.activeTurn
            ? "running"
            : rt.lastStatus === "failed"
              ? "failed"
              : rt.lastStatus === "completed" && r.spawnedBy
                ? "done"
                : "idle";
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
      muted: r.muted ?? false,
      pinned: r.pinned ?? false,
      agents: r.agents ?? "ask",
    };
    if (r.model) s.model = r.model;
    if (r.forkedFrom) s.forkedFrom = r.forkedFrom;
    if (r.spawnedBy) s.spawnedBy = r.spawnedBy;
    s.project = projectRefFor(this.projects.list(), { folder: r.cwd, repo: r.repo ?? null });
    if (r.repo) {
      s.repo = r.repo;
      const git = this.gitStatus.get(r.repo.root);
      if (git) s.git = git;
      s.worktree = { path: r.repo.root, ...(git?.branch ? { branch: git.branch } : {}) };
      if (git?.branch) s.branch = git.branch;
    }
    return s;
  }

  holderOf(rt: ThreadRuntime): Holder {
    // Read-only only on positive evidence: `uncertain` without `external` is free.
    const ext = rt.external.external ? rt.external.holder ?? "another program" : null;
    return { driver: rt.driver?.key ?? null, driverHost: rt.driver?.host ?? null, external: ext };
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
    const summary = this.summaryOf(rt);
    const holder = this.holderOf(rt);
    if (!this.index.upsert(summary, holder)) return;
    const token = this.index.token;
    for (const c of this.clients) c.notify("thread/updated", { thread: this.renderSummary(summary, holder, c.key), token });
  }

  broadcast(method: string, params: unknown): void {
    for (const c of this.clients) c.notify(method, params);
  }

  busy(): { runningTurns: number; pendingRequests: number } {
    let runningTurns = 0;
    let pendingRequests = 0;
    for (const rt of this.threads.values()) {
      if (rt.deleted) continue;
      if (rt.activeTurn) runningTurns++;
      pendingRequests += rt.pendingRequestCount();
    }
    return { runningTurns, pendingRequests };
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
    const holder = rt.external.external ? rt.external.holder ?? "another program" : rt.driver?.host ?? null;
    if (holder) ev.holder = holder;
    this.append(rt, ev, { driver: rt.driver?.key ?? null, external: rt.external.external });
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
          if (provider && (inst.descriptor.capabilities ?? []).includes("threadRead")) {
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
      const end = i >= 0 && i + need > buf.length ? i : buf.length;
      buf = buf.subarray(0, Math.max(0, end));
    }
    const next = start + buf.length;
    return { present: true, data: buf.toString("utf8"), offset: start, nextOffset: next, totalBytes: total, more: next < total };
  }

  // ── Subscriptions ──────────────────────────────────────────────────────────────────────

  async subscribe(client: ClientLink, p: { threadId: string; epoch?: number | null; afterSeq?: number | null }, register: (cursor: number, mode: "replay" | "reset") => void): Promise<ThreadSubscribeResult> {
    const rt = this.requireThread(p.threadId);
    await this.refreshExternal(rt);
    const effective = await this.computeEffective(rt);
    this.flushDeltas(rt);
    const journal = this.journalOf(rt);
    journal.touch(client.key);
    const holder = this.holderOf(rt);
    const c = controlFor(holder, client.key);
    const common = {
      control: c.control,
      ...(c.controlHolder ? { controlHolder: c.controlHolder } : {}),
      ...(rt.activeTurn ? { activeTurn: rt.activeTurn } : {}),
      effective,
      ...(rt.context ? { context: rt.context } : {}),
      ...(rt.tokens ? { tokens: rt.tokens } : {}),
    };
    // Someone is watching this thread: make sure its checkout's git status is known.
    if (!rt.record.repoResolved) void this.resolveProject(rt, true);
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
    rt.effective = eff;
    return eff;
  }

  /** Asks the provider who holds the thread. Only a positive `external` makes it read-only. */
  async refreshExternal(rt: ThreadRuntime): Promise<ExternalControl> {
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

  private setExternal(rt: ThreadRuntime, ext: ExternalControl): void {
    const before = canonical(rt.external);
    rt.external = { external: !!ext.external, ...(ext.uncertain ? { uncertain: true } : {}), ...(ext.holder ? { holder: ext.holder } : {}) };
    if (canonical(rt.external) === before) return;
    this.appendControl(rt);
    this.updateIndex(rt);
    if (rt.canDispatch()) this.afterIdle(rt);
  }

  private checkWriter(rt: ThreadRuntime, client: ClientLink | null, wrap: boolean): void {
    const fail = (message: string, holder: string | null) => {
      const err = rpc("thread_in_use", message, holder ? { holder } : undefined);
      throw wrap ? new NotActed(err) : err;
    };
    if (rt.external.external) {
      fail(`this thread is in use in ${rt.external.holder ?? "another program"}; it can only be watched`, rt.external.holder ?? null);
    }
    if (rt.driver && (!client || rt.driver.key !== client.key)) {
      fail(`this thread is being driven from ${rt.driver.host ?? "another YAAC client"}; it can only be watched`, rt.driver.host);
    }
  }

  // ── Input ──────────────────────────────────────────────────────────────────────────────

  private provider(rt: ThreadRuntime): HarnessProvider {
    return this.registry.usable(rt.record.instance).provider;
  }

  private checkModel(descriptor: ProviderDescriptor, model: ModelId | null | undefined): void {
    if (!model) return;
    const models = descriptor.models ?? [];
    if (descriptor.modelsComplete && !models.some((m) => m.id === model)) {
      throw notActed("model_unavailable", `${model} is not offered by ${descriptor.displayName} now`);
    }
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
    return { parts, text, images, files, origin };
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
      const parts = this.validateParts(p.input, this.modelImageSupport(instance.descriptor, p.model));
      // The thread's id is not known until the harness has started it: its attachment
      // folder is keyed by this operation, for the thread's whole life.
      const uploadsKey = `start:${p.opId}`;
      const input = this.buildInput(uploadsKey, parts, "user");
      const key = instance.config.id;
      this.pendingStarts.set(key, (this.pendingStarts.get(key) ?? 0) + 1);
      let started: { thread: NativeId; turn: TurnId };
      try {
        started = await provider.startThread({ cwd, model: p.model ?? null, effort: p.effort ?? null, mode: p.mode ?? null }, input);
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
          ...(typeof p.agents === "string" ? { agents: p.agents } : {}),
        },
        true,
      );
      const driver: Driver = { key: client.key, host: client.host };
      this.turnBegan(rt, started.turn, "user", parts, driver);
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
  private turnBegan(rt: ThreadRuntime, turnId: TurnId, origin: TurnOrigin, parts: InputPart[], driver: Driver | null): void {
    if (rt.activeTurn === turnId) return;
    rt.expectTurn = null;
    rt.activeTurn = turnId;
    rt.turnOrigin = origin;
    rt.driver = driver;
    rt.lastStatus = null;
    rt.saveRuntime();
    this.append(rt, { type: "turnStarted", turnId, origin, at: nowIso() });
    const at = nowIso();
    const user: Item = { id: `user:${turnId}`, turnId, status: "completed", kind: "userMessage", parts, startedAt: at, completedAt: at };
    this.append(rt, { type: "itemStarted", item: user });
    this.append(rt, { type: "itemCompleted", item: user });
    if (driver) this.appendControl(rt);
    rt.record.updatedAt = nowIso();
    rt.saveRecord();
    this.updateIndex(rt);
    this.activity.emit();
  }

  /** `turn/start`: a client's message goes through the thread's queue, and only if it can go now. */
  async turnStart(client: ClientLink, p: TurnStartParams): Promise<TurnStartResult> {
    return this.ops.run("turn/start", p.opId, async () => {
      const rt = this.threads.get(p.threadId);
      if (!rt || rt.deleted) throw notActed("thread_not_found", `no thread ${String(p.threadId)} on this target`);
      let inst;
      try {
        inst = this.registry.usable(rt.record.instance).instance;
      } catch (e) {
        throw new NotActed(e as RpcError);
      }
      await this.refreshExternal(rt);
      this.checkWriter(rt, client, true);
      if (rt.activeTurn || rt.dispatching) {
        throw notActed("turn_in_progress", "a turn is already running in this thread", rt.activeTurn ? { activeTurn: rt.activeTurn } : undefined);
      }
      this.checkModel(inst.descriptor, p.model);
      const eff = rt.effective ?? (await this.computeEffective(rt));
      const imageSupport = p.model ? this.modelImageSupport(inst.descriptor, p.model) : eff.imageInput;
      const parts = this.validateParts(p.input, imageSupport);
      const turnId = await this.submit(rt, {
        origin: "user",
        parts,
        overrides: { model: p.model ?? null, effort: p.effort ?? null, mode: p.mode ?? null },
        driver: { key: client.key, host: client.host },
      }, "now");
      return { turnId };
    });
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
          reject(rt.activeTurn || rt.dispatching ? notActed("turn_in_progress", "a turn is already running in this thread") : notActed("thread_in_use", "this thread is in use elsewhere"));
          return;
        }
        rt.queue.unshift(full);
      } else {
        rt.queue.push(full);
      }
      this.pump(rt);
    });
  }

  private pump(rt: ThreadRuntime): void {
    if (rt.queue.length === 0 || !rt.canDispatch()) return;
    const work = rt.queue.shift()!;
    void this.execute(rt, work);
  }

  private async execute(rt: ThreadRuntime, work: Work): Promise<void> {
    rt.dispatching = true;
    try {
      const provider = this.provider(rt);
      // Ownership must be shown, not assumed: ask the harness right before handing input over.
      // Only positive evidence of another holder stops it; "unsure" is free.
      const ext = await provider.control(rt.record.native).catch((): ExternalControl => ({ external: false, uncertain: true }));
      if (ext.external) {
        rt.dispatching = false;
        this.setExternal(rt, ext);
        if (work.origin === "user") work.reject(notActed("thread_in_use", `this thread is in use in ${ext.holder ?? "another program"}; it can only be watched`));
        else rt.queue.unshift(work);
        return;
      }
      await work.beforeStart?.();
      const input = this.buildInput(rt.uploadsKey, work.parts, work.origin);
      rt.expectTurn = { origin: work.origin, parts: work.parts, driver: work.driver };
      let turnId: TurnId;
      try {
        turnId = await provider.startTurn(rt.record.native, input, work.overrides);
      } catch (e) {
        rt.expectTurn = null;
        throw rpc("provider_unavailable", `the harness did not start the turn: ${(e as Error).message}`);
      }
      this.turnBegan(rt, turnId, work.origin, work.parts, work.driver);
      rt.dispatching = false;
      try {
        await work.onStarted?.(turnId);
      } finally {
        work.resolve(turnId);
      }
    } catch (e) {
      rt.dispatching = false;
      const err = e instanceof NotActed ? e : e instanceof RpcError ? e : rpc("internal_error", (e as Error).message);
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

  // ── Turn control ───────────────────────────────────────────────────────────────────────

  async turnInterrupt(client: ClientLink, p: TurnInterruptParams): Promise<void> {
    const rt = this.requireThread(p.threadId);
    if (!rt.activeTurn || rt.activeTurn !== p.turnId) {
      throw rpc("turn_mismatch", `turn ${String(p.turnId)} is not the running turn`, rt.activeTurn ? { activeTurn: rt.activeTurn } : undefined);
    }
    this.checkWriter(rt, client, false);
    await this.provider(rt).interrupt(rt.record.native, p.turnId);
  }

  async turnSteer(client: ClientLink, p: TurnSteerParams): Promise<void> {
    await this.ops.run("turn/steer", p.opId, async () => {
      const rt = this.threads.get(p.threadId);
      if (!rt || rt.deleted) throw notActed("thread_not_found", `no thread ${String(p.threadId)} on this target`);
      await this.refreshExternal(rt);
      this.checkWriter(rt, client, true);
      const provider = this.registry.get(rt.record.instance)?.provider;
      const eff = rt.effective ?? (await this.computeEffective(rt));
      if (!provider?.steer || !(eff.capabilities ?? []).includes("steer")) throw notActed("capability_missing", "this thread cannot be steered");
      if (!rt.activeTurn || rt.activeTurn !== p.turnId) {
        throw notActed("turn_mismatch", `turn ${String(p.turnId)} is not the running turn`, rt.activeTurn ? { activeTurn: rt.activeTurn } : undefined);
      }
      const parts = this.validateParts(p.input, eff.imageInput);
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
  }

  async requestRespond(client: ClientLink, p: RequestRespondParams): Promise<void> {
    await this.ops.run("request/respond", p.opId, async () => {
      const rt = this.threads.get(p.threadId);
      if (!rt || rt.deleted) throw notActed("thread_not_found", `no thread ${String(p.threadId)} on this target`);
      const rec = rt.requests.get(p.requestId);
      if (!rec) throw notActed("request_not_found", `no request ${String(p.requestId)} in this thread`);
      await this.refreshExternal(rt);
      this.checkWriter(rt, client, true);
      if (rec.state !== "pending") throw notActed("request_not_pending", `the request is ${rec.state}`, { state: rec.state });
      if (!p.response || p.response.requestKind !== rec.requestKind) {
        throw notActed("invalid_params", `the answer must be a ${rec.requestKind} answer`);
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

  // ── Fork ───────────────────────────────────────────────────────────────────────────────

  private forkPlan(src: ThreadRuntime, instanceId: string): { kind: "native"; dest: Instance } | { kind: "handoff"; reason: string } {
    const dest = this.registry.get(instanceId);
    if (!dest) throw rpc("instance_not_found", `no provider instance ${instanceId} on this target`);
    const srcInst = this.registry.get(src.record.instance);
    const sameStore =
      !!srcInst &&
      dest.config.kind === srcInst.config.kind &&
      !!dest.descriptor.store.key &&
      dest.descriptor.store.key === (srcInst.descriptor.store.key ?? null) &&
      src.record.storeKey === dest.descriptor.store.key;
    const can = sameStore && typeof srcInst?.provider?.forkThread === "function" && (srcInst.descriptor.capabilities ?? []).includes("threadFork");
    if (can) return { kind: "native", dest };
    return { kind: "handoff", reason: "a handoff fork (through a transcript) is not available in this build of yaacd" };
  }

  private lastCompletedTurn(rt: ThreadRuntime): TurnId | null {
    const items = rt.projection?.items() ?? [];
    const turns = items.map((i) => i.turnId).filter((t) => t && t !== rt.activeTurn);
    return turns.at(-1) ?? null;
  }

  async forkPreview(p: ThreadForkPreviewParams): Promise<ThreadForkPreviewResult> {
    const src = this.requireThread(p.source);
    await this.ensureProjection(src);
    const plan = this.forkPlan(src, p.instance);
    if (plan.kind === "handoff") return { forkKind: "handoff", unavailableReason: plan.reason };
    const cutoff = p.atTurn ?? this.lastCompletedTurn(src);
    const out: ThreadForkPreviewResult = { forkKind: "native", cutoffTurn: cutoff };
    if (plan.dest.descriptor.store.account) out.destinationAccount = plan.dest.descriptor.store.account;
    return out;
  }

  async threadFork(client: ClientLink, p: ThreadForkParams): Promise<ThreadForkResult> {
    return this.ops.run("thread/fork", p.opId, async () => {
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

  /** `thread/forkOnly`: the fork, idle, with no first message. */
  async threadForkOnly(p: ThreadForkOnlyParams): Promise<ThreadForkOnlyResult> {
    return this.ops.run("thread/forkOnly", p.opId, async () => {
      const { rt } = await this.forkInto({ ...p, input: [] }, false);
      return { threadId: rt.id, forkKind: "native" as const };
    });
  }

  private async forkInto(p: ThreadForkParams, withInput: boolean): Promise<{ rt: ThreadRuntime; parts: InputPart[] | null }> {
    {
      const src = this.threads.get(p.source);
      if (!src || src.deleted) throw notActed("thread_not_found", `no thread ${String(p.source)} on this target`);
      let plan;
      try {
        plan = this.forkPlan(src, p.instance);
      } catch (e) {
        throw new NotActed(e as RpcError);
      }
      if (plan.kind === "handoff") throw notActed("capability_missing", plan.reason);
      const srcInst = this.registry.get(src.record.instance)!;
      if (p.atTurn && !(srcInst.descriptor.capabilities ?? []).includes("threadForkAtTurn")) {
        throw notActed("capability_missing", "this provider forks only from the last completed turn");
      }
      this.checkModel(plan.dest.descriptor, p.model);
      const parts = withInput ? this.validateParts(p.input, this.modelImageSupport(plan.dest.descriptor, p.model)) : null;
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
          title: `${src.record.titleOverride ?? src.record.title ?? "Thread"} (fork)`,
          model: p.model ?? src.record.model ?? null,
        },
        false,
      );
      const cutoff = p.atTurn ?? this.lastCompletedTurn(src);
      const handoff: Item = { id: `fork:${src.id}`, turnId: cutoff ?? "", status: "completed", kind: "handoff", forkKind: "native", forkedFrom: src.id, cutoffTurn: cutoff, startedAt: nowIso(), completedAt: nowIso() };
      this.append(rt, { type: "itemStarted", item: handoff });
      this.append(rt, { type: "itemCompleted", item: handoff });
      return { rt, parts };
    }
  }

  // ── Rename, archive, delete ────────────────────────────────────────────────────────────

  private async gated(rt: ThreadRuntime, cap: Capability, method: "rename" | "archive" | "remove"): Promise<HarnessProvider> {
    const inst = this.registry.get(rt.record.instance);
    const provider = inst?.provider;
    if (!provider || typeof provider[method] !== "function" || !(inst!.descriptor.capabilities ?? []).includes(cap)) {
      throw notActed("capability_missing", `this thread's provider cannot ${method === "remove" ? "delete" : method} threads`);
    }
    return provider;
  }

  async threadRename(p: { opId: string; threadId: string; title: string }): Promise<void> {
    await this.ops.run("thread/rename", p.opId, async () => {
      const rt = this.threads.get(p.threadId);
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
      const rt = this.threads.get(p.threadId);
      if (!rt || rt.deleted) throw notActed("thread_not_found", `no thread ${String(p.threadId)}`);
      const provider = await this.gated(rt, "threadArchive", "archive");
      await provider.archive!(rt.record.native);
      rt.record.archived = true;
      rt.saveRecord();
      this.updateIndex(rt);
      return {};
    });
  }

  async threadDelete(client: ClientLink, p: { opId: string; threadId: string }): Promise<void> {
    await this.ops.run("thread/delete", p.opId, async () => {
      const rt = this.threads.get(p.threadId);
      if (!rt || rt.deleted) throw notActed("thread_not_found", `no thread ${String(p.threadId)}`);
      await this.refreshExternal(rt);
      this.checkWriter(rt, client, true);
      if (rt.activeTurn) throw notActed("turn_in_progress", "stop the running turn before deleting the thread");
      const provider = await this.gated(rt, "threadDelete", "remove");
      await provider.remove!(rt.record.native);
      this.forget(rt);
      return {};
    });
  }

  private forget(rt: ThreadRuntime): void {
    rt.deleted = true;
    for (const w of rt.queue.splice(0)) w.reject(rpc("thread_not_found", "the thread was deleted"));
    if (rt.flushTimer) clearTimeout(rt.flushTimer);
    rt.journal?.destroy();
    fs.rmSync(rt.dir, { recursive: true, force: true });
    this.uploads.removeThread(rt.uploadsKey);
    this.threads.delete(rt.id);
    this.byNative.delete(this.nativeKey(rt.record.instance, rt.record.native));
    if (this.index.remove(rt.id)) this.broadcast("thread/removed", { threadId: rt.id, token: this.index.token });
    this.activity.emit();
  }

  // ── Provider events ────────────────────────────────────────────────────────────────────

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
    if (ev.type === "descriptorChanged") return;
    if (ev.type === "threadChanged") {
      if (this.pendingStarts.has(instanceId) && !this.byNative.has(this.nativeKey(instanceId, ev.info.native))) {
        this.bufferEvent(instanceId, ev);
        return;
      }
      this.discovered(instance, ev.info);
      return;
    }
    if (ev.type === "threadRemoved") {
      const id = this.byNative.get(this.nativeKey(instanceId, ev.thread));
      const rt = id ? this.threads.get(id) : undefined;
      if (rt) this.forget(rt);
      return;
    }
    const id = this.byNative.get(this.nativeKey(instanceId, ev.thread));
    let rt = id ? this.threads.get(id) : undefined;
    if (!rt) {
      if (this.pendingStarts.has(instanceId)) {
        this.bufferEvent(instanceId, ev);
        return;
      }
      // A conversation yaacd has not seen: someone else started it.
      rt = this.discovered(instance, { native: ev.thread, cwd: this.opts.home, createdAt: nowIso(), updatedAt: nowIso(), originKind: "interactive" });
    }
    if (rt.deleted) return;
    switch (ev.type) {
      case "turnStarted": {
        if (rt.activeTurn === ev.turnId) return;
        if (rt.expectTurn) {
          const x = rt.expectTurn;
          this.turnBegan(rt, ev.turnId, x.origin, x.parts, x.driver);
          return;
        }
        rt.activeTurn = ev.turnId;
        rt.turnOrigin = ev.origin ?? "external";
        rt.driver = null;
        rt.lastStatus = null;
        rt.saveRuntime();
        this.append(rt, { type: "turnStarted", turnId: ev.turnId, origin: rt.turnOrigin, at: ev.at ?? nowIso() });
        this.updateIndex(rt);
        this.activity.emit();
        return;
      }
      case "turnCompleted": {
        this.flushDeltas(rt);
        for (const r of rt.requests.values()) {
          if (r.turnId !== ev.turnId) continue;
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
        this.append(rt, { type: ev.type, item });
        if (wasWaiting !== rt.waiting.size > 0) this.updateIndex(rt);
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

  async close(): Promise<void> {
    if (this.gitTimer) clearInterval(this.gitTimer);
    this.gitTimer = null;
    await this.gitStatus.idle().catch(() => {});
    this.gitStatus.close();
    for (const rt of this.threads.values()) {
      this.flushDeltas(rt);
      rt.journal?.close();
    }
    this.index.save();
    this.ops.close();
    await this.registry.dispose();
  }

  /** Expose for tests: the last journal sequence of a thread. */
  position(threadId: string): JournalPosition {
    const j = this.journalOf(this.requireThread(threadId));
    return { epoch: j.epoch, seq: j.headSeq };
  }
}

export { lastSeqOf, applyDelta };
