// Per-thread runtime state and its on-disk record.

import * as path from "node:path";
import type {
  AgentsPolicy,
  ContextUsage,
  HarnessSubagent,
  InputPart,
  Item,
  RepoLocation,
  TokenUsage,
  OriginKind,
  ProjectRef,
  RequestResolution,
  RequestState,
  QueuedMessage,
  ThreadEffective,
  ThreadHold,
  TurnId,
  TurnOrigin,
  WorktreeRef,
} from "../protocol/generated/types.js";
import type { ExternalControl, MeshInputMessage, NativeId, TurnOverrides } from "../providers/types.js";
import { Journal, type JournalOptions } from "./journal.js";
import type { Projection } from "./items.js";
import { readJson, sha256Hex, writeJsonAtomic } from "../util/fsx.js";

/** The immutable binding plus what YAAC itself knows about a thread (`thread.json`). */
export interface ThreadRecord {
  id: string;
  instance: string;
  /** The instance's store identity when the binding was made. */
  storeKey: string | null;
  kind: string;
  native: NativeId;
  cwd: string;
  createdAt: string;
  updatedAt: string;
  originKind: OriginKind;
  forkedFrom?: string | null;
  /**
   * A handoff fork's transcript file (in `~/.yaac/handoffs/`), pinned to this thread: kept
   * for its whole life, removed with it.
   */
  handoffTranscript?: string | null;
  title?: string | null;
  titleOverride?: string | null;
  preview?: string | null;
  model?: string | null;
  archived?: boolean;
  project?: ProjectRef | null;
  worktree?: WorktreeRef | null;
  branch?: string | null;
  /** Other instances that see the same conversation (same kind and store). */
  alsoVisibleTo?: string[];
  /** The chat's agents policy (`ask` when absent). */
  agents?: AgentsPolicy;
  /** Where the working directory sits in a repository (null: not in one). */
  repo?: RepoLocation | null;
  /** Whether `repo` has been looked up. */
  repoResolved?: boolean;
  /** The agent that started this thread (set by the mesh). */
  spawnedBy?: string | null;
  /**
   * One of the harness's own sub-agents (D151): the native id of its parent conversation on
   * the same instance, and what the harness last said of it. Never a mesh relation.
   */
  nativeParent?: NativeId | null;
  subagent?: HarnessSubagent | null;
  /**
   * The key of the thread's attachment folder, fixed for its whole life. A thread started
   * with `thread/start` links its first message's attachments before its id exists, so its
   * folder is keyed by that operation; every other thread's by its id.
   */
  uploadsKey?: string | null;
  /** Archived from YAAC (the user touched it: never cut by the discovery caps). */
  userArchived?: boolean;
  /** Outside the discovery caps at the last scan: not listed unless something keeps it. */
  capped?: boolean;
  /** Brought in by a lookup by id or an older-threads page; dropped again at a scan once nothing keeps it. */
  broughtIn?: boolean;
  /** Stopped and held (docs/10-agent-mesh.md#stopping-workers). */
  hold?: ThreadHold | null;
  /**
   * Taken over from another program (D140): while every holder the harness reports is one of
   * `holderIds`, that evidence does not make the thread read-only. Cleared when a different
   * holder appears or the harness reports nobody else.
   */
  takenOver?: TakenOver | null;
  /**
   * Fast mode (D125): the setting the last turn started with, and the next turns' unless a
   * `turn/start` changes it. Only ever true while the model has `fastMode`.
   */
  fast?: boolean;
  /**
   * Where the user moved the thread in YAAC (`thread/move`, D144): its project and, when the
   * project is in git, the checkout. Apart from `cwd`; absent: the folder rule decides.
   */
  placement?: PlacementRecord | null;
  /**
   * A `thread/move` changed the working directory (`threadMoveWorkingDirectory`): every turn
   * from then on hands the harness `cwd` explicitly.
   */
  cwdMoved?: boolean;
  /**
   * The working directory changed since the last turn started: the next turn's input begins
   * with a short fixed note saying so (`from` is where the agent last worked).
   */
  cwdNote?: { from: string; to: string } | null;
}

/** A thread's placement as `thread.json` keeps it. */
export interface PlacementRecord {
  projectId: string;
  /** The checkout's root, when the project is in git. */
  worktree?: string | null;
  /** A linked worktree's name (absent for the main checkout, which is named by its branch). */
  worktreeName?: string | null;
  /** The checkout's branch when the move was made (for the main checkout's label until git status is known). */
  branch?: string | null;
}

/** A takeover (D140), kept with the thread record so it survives a yaacd restart. */
export interface TakenOver {
  /** Where it was taken from ("another Claude Code session"). */
  from: string;
  /** The holders it was taken from (`ExternalControl.holderIds`); empty: not named. */
  holderIds: string[];
  at: string;
}

/** A user message waiting for the running turn to end (`queue.json`). */
export interface QueuedEntry extends QueuedMessage {
  driver: Driver | null;
  /** The `turn/start` that queued it: stamped on the user message when it is delivered (D165). */
  opId?: string;
  /** Set (durably) just before it is handed to the harness. */
  dispatching?: boolean;
}

/** What must survive a restart to reconcile a turn that was running (`runtime.json`). */
export interface RuntimeFile {
  activeTurn: TurnId | null;
  driver: Driver | null;
  turnOrigin: TurnOrigin | null;
  lastStatus: string | null;
  /** Last known context-window and token usage. */
  context?: ContextUsage | null;
  tokens?: TokenUsage | null;
}

export interface Driver {
  key: string;
  host: string | null;
}

/** A request's state, kept by yaacd (`requests.json`). Answers to secret questions are never kept. */
export interface RequestRecord {
  requestId: string;
  itemId: string;
  turnId: TurnId;
  requestKind: string;
  state: RequestState;
  secretQuestions: string[];
  submittedBy?: Driver | null;
  resolution?: RequestResolution | null;
}

/** Something that can start work in a thread: the dispatcher's unit. */
export interface Work {
  origin: TurnOrigin;
  parts: InputPart[];
  overrides: TurnOverrides;
  driver: Driver | null;
  beforeStart?: () => void | Promise<void>;
  onStarted?: (turnId: TurnId) => void | Promise<void>;
  onFailed?: (error: Error) => void;
  /** Mesh (WP7): items journaled at the turn's start instead of a user message (`meshReceived`). */
  journalItems?: Item[];
  /** Mesh (WP7): the agent messages the turn carries, sender and body apart. */
  mesh?: MeshInputMessage[];
  /** The client's operation that sent it, stamped on the user message (D165). */
  opId?: string;
  resolve: (turnId: TurnId) => void;
  reject: (error: Error) => void;
}

export interface OutputTrack {
  file: string;
  bytes: number;
  headBytes: number;
  truncated: boolean;
}

export function threadDirName(id: string): string {
  return sha256Hex(id).slice(0, 32);
}

export class ThreadRuntime {
  journal: Journal | null = null;
  projection: Projection | null = null;
  projectionLoading: Promise<Projection> | null = null;
  activeTurn: TurnId | null = null;
  turnOrigin: TurnOrigin | null = null;
  driver: Driver | null = null;
  external: ExternalControl = { external: false };
  lastStatus: string | null = null;
  requests = new Map<string, RequestRecord>();
  queue: Work[] = [];
  dispatching = false;
  /** Set while yaacd waits for the harness to start a turn it asked for. */
  expectTurn: { origin: TurnOrigin; parts: InputPart[]; driver: Driver | null; items?: Item[]; opId?: string } | null = null;
  pendingDeltas = new Map<string, { itemId: string; field: "text" | "reasoning" | "output"; delta: string }>();
  pendingDeltaBytes = 0;
  flushTimer: NodeJS.Timeout | null = null;
  outputs = new Map<string, OutputTrack>();
  interruptReason: "update" | null = null;
  effective: ThreadEffective | null = null;
  deleted = false;
  /** `meshWait` items in progress: the thread is blocked on other agents. */
  waiting = new Set<string>();
  /** When in-progress items appeared, so their completion carries `startedAt`. */
  itemStarts = new Map<string, string>();
  context: ContextUsage | null = null;
  tokens: TokenUsage | null = null;
  /** Queued user messages, in order (durable: `queue.json`). */
  userQueue: QueuedEntry[] = [];
  /** A dispatch of a queued user message failed: the queue waits for the user. */
  queuePaused = false;
  /** Reserved for a `now` delivery waiting for the running turn to end: nothing else dispatches. */
  claimed = false;

  constructor(
    public record: ThreadRecord,
    readonly dir: string,
  ) {}

  get id(): string {
    return this.record.id;
  }

  openJournal(opts: JournalOptions, fresh: boolean): Journal {
    if (!this.journal) this.journal = Journal.open(path.join(this.dir, "journal"), opts, fresh);
    return this.journal;
  }

  saveRecord(): void {
    if (!this.deleted) writeJsonAtomic(path.join(this.dir, "thread.json"), this.record);
  }

  saveRuntime(): void {
    if (this.deleted) return;
    const r: RuntimeFile = { activeTurn: this.activeTurn, driver: this.driver, turnOrigin: this.turnOrigin, lastStatus: this.lastStatus, context: this.context, tokens: this.tokens };
    writeJsonAtomic(path.join(this.dir, "runtime.json"), r);
  }

  loadRuntime(): RuntimeFile | undefined {
    try {
      return readJson<RuntimeFile>(path.join(this.dir, "runtime.json"));
    } catch {
      return undefined;
    }
  }

  saveRequests(): void {
    if (!this.deleted) writeJsonAtomic(path.join(this.dir, "requests.json"), [...this.requests.values()]);
  }

  loadRequests(): void {
    let list: RequestRecord[] | undefined;
    try {
      list = readJson<RequestRecord[]>(path.join(this.dir, "requests.json"));
    } catch {
      list = undefined;
    }
    for (const r of list ?? []) this.requests.set(r.requestId, r);
  }

  saveQueue(): void {
    if (!this.deleted) writeJsonAtomic(path.join(this.dir, "queue.json"), this.userQueue);
  }

  loadQueue(): void {
    try {
      this.userQueue = readJson<QueuedEntry[]>(path.join(this.dir, "queue.json")) ?? [];
    } catch {
      this.userQueue = [];
    }
  }

  pendingRequestCount(): number {
    let n = 0;
    for (const r of this.requests.values()) if (r.state === "pending") n++;
    return n;
  }

  /** The key of the thread's attachment folder. */
  get uploadsKey(): string {
    return this.record.uploadsKey ?? this.record.id;
  }

  /** Held by yaacd and idle: the two conditions for dispatching (docs/10-agent-mesh.md). */
  canDispatch(): boolean {
    // Only positive evidence of another holder blocks dispatch; "uncertain" is free.
    // A held (stopped) thread starts nothing by itself; a claimed one waits for its `now` input.
    return !this.deleted && !this.activeTurn && !this.dispatching && !this.external.external && !this.record.hold && !this.claimed;
  }
}
