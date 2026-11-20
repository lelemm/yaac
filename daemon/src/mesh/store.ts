// The mesh's durable state on the target (docs/10-agent-mesh.md#durability-hop-by-hop).
//
//   <home>/mesh/messages.jsonl   every message this target holds — mailbox and outbox — as an
//                                append-only log of records and their changes, fsynced before
//                                any hop is acknowledged. Kept apart from the UI journal, so
//                                trimming a journal never erases what agents said.
//   <home>/mesh/state.json       relationships, roots and budgets, tickets, sequence numbers,
//                                the last roster. Written atomically.
//
// Deduplication by message id happens in the same synchronous step that stores a message:
// `put` refuses an id it already has, and yaacd is single-threaded, so there is no window
// between the check and the write.

import * as fs from "node:fs";
import * as path from "node:path";
import type { Address, MeshBudget, MeshLimits, MeshMessage, MeshPeer, Receipt, RosterEntry, SpawnedThread, ThreadState, TurnId } from "../protocol/generated/types.js";
import { ensureDir, readJson, writeJsonAtomic } from "../util/fsx.js";

/** A message's life on this target. */
export interface MessageRecord {
  message: MeshMessage;
  /** The recipient is a thread on this target: the message is in its mailbox. */
  inbox: boolean;
  /** The sender is a thread on this target (its `meshSent` item lives here). */
  outbox: boolean;
  /** Mailbox: pending → (dispatching →) consumed; or offered (handed to a `wait`) → consumed. */
  inState?: "pending" | "dispatching" | "offered" | "consumed" | "outcomeUnknown" | "failed";
  /** Outbox, other target: relayPending → relayed → done (a final receipt came back). */
  outState?: "local" | "relayPending" | "relayed" | "done";
  /** The sender's receipt, as far as this target knows. */
  receipt?: Receipt;
  /** The local recipient thread id. */
  recipient?: string;
  /** The local sender thread id. */
  sender?: string;
  /** The cursor of the `wait` it was offered to. */
  offered?: string;
  /** The turn that consumed it. */
  turnId?: TurnId;
  /** When it was stored here (ms). */
  storedAt: number;
  /** The sender has been told it is overdue. */
  noticed?: boolean;
  /** Failed dispatch attempts (a refusal by the harness, not an uncertain outcome). */
  dispatchErrors?: number;
}

/** What the mesh knows about one local thread. */
export interface ThreadMesh {
  spawnedBy?: Address | null;
  /** The orchestration root it was spawned under (its own address when a person started it). */
  spawnRoot?: Address | null;
  depth: number;
  children: Address[];
  peers: Address[];
  /** Its first turn: the one whose answer returns to the parent once. */
  spawnTurn?: TurnId | null;
  detached?: boolean;
  resultSent?: boolean;
  /** The root of the turn now running (or last run). */
  currentRoot?: Address | null;
  notify: Array<{ to: Address; turnId: TurnId }>;
  /** Given the one-time YAAC introduction. */
  introduced?: boolean;
  /** `meshSpawn` items, by the turn that spawned. */
  spawnItems?: Record<string, { itemId: string; threads: SpawnedThread[] }>;
}

/** An orchestration root owned by this target. */
export interface RootState {
  allowance: MeshBudget;
  agentTurns: number;
  threadsSpawned: number;
  /** Spending reported by other targets (cumulative, the highest seen per source). */
  remote: Record<string, { agentTurns: number; threadsSpawned: number }>;
  lastHumanAt: number;
  granted: boolean;
  denied?: boolean;
  pendingRequest?: string | null;
  /** A budget notice is showing (until Continue). */
  exhaustedNoticed?: boolean;
}

/** A root that lives on another target: what this target spent against it. */
export interface RemoteRootState {
  agentTurns: number;
  threadsSpawned: number;
  /** Its target said it has run out. */
  paused: boolean;
  reportedAt?: number;
}

export interface Ticket {
  instance: string;
  native: string | null;
  createdAt: string;
}

export interface MeshPolicyState {
  budget: MeshBudget;
  limits: MeshLimits;
}

export interface StateFile {
  self: { id: string; name: string } | null;
  roster: RosterEntry[];
  rosterAt?: string | null;
  policy: MeshPolicyState;
  threads: Record<string, ThreadMesh>;
  roots: Record<string, RootState>;
  remoteRoots: Record<string, RemoteRootState>;
  tickets: Record<string, Ticket>;
  /** Last sequence number per sender→recipient pair. */
  seqs: Record<string, number>;
  /** Labels of threads on other targets, as last seen. */
  labels: Record<string, MeshPeer & { state?: ThreadState | null; seenAt?: string }>;
}

export const DEFAULT_POLICY: MeshPolicyState = {
  budget: { agentTurns: 100, threadsSpawned: 20, wallClockSecs: 12 * 3600 },
  limits: { workersPerThread: 8, spawnDepth: 3, runningThreads: 6, pairMessages: 30, pairWindowSecs: 600, undeliveredNoticeSecs: 900 },
};

type LogLine = { op: "put"; rec: MessageRecord } | { op: "upd"; id: string; patch: Partial<MessageRecord> };

export class MeshStore {
  readonly dir: string;
  state: StateFile;
  private readonly records = new Map<string, MessageRecord>();
  private readonly logFile: string;
  private readonly stateFile: string;
  private fd: number;
  private lines = 0;

  constructor(home: string) {
    this.dir = path.join(home, "mesh");
    ensureDir(this.dir);
    try {
      fs.chmodSync(this.dir, 0o700);
    } catch {
      /* best effort */
    }
    this.stateFile = path.join(this.dir, "state.json");
    this.logFile = path.join(this.dir, "messages.jsonl");
    const loaded = readJson<Partial<StateFile>>(this.stateFile) ?? {};
    this.state = {
      self: loaded.self ?? null,
      roster: loaded.roster ?? [],
      rosterAt: loaded.rosterAt ?? null,
      policy: {
        budget: { ...DEFAULT_POLICY.budget, ...(loaded.policy?.budget ?? {}) },
        limits: { ...DEFAULT_POLICY.limits, ...(loaded.policy?.limits ?? {}) },
      },
      threads: loaded.threads ?? {},
      roots: loaded.roots ?? {},
      remoteRoots: loaded.remoteRoots ?? {},
      tickets: loaded.tickets ?? {},
      seqs: loaded.seqs ?? {},
      labels: loaded.labels ?? {},
    };
    this.load();
    this.fd = fs.openSync(this.logFile, "a", 0o600);
    if (this.lines > 2 * this.records.size + 1000) this.compact();
  }

  private load(): void {
    let text = "";
    try {
      text = fs.readFileSync(this.logFile, "utf8");
    } catch {
      return;
    }
    for (const raw of text.split("\n")) {
      if (!raw.trim()) continue;
      let line: LogLine;
      try {
        line = JSON.parse(raw) as LogLine;
      } catch {
        continue; // a torn last line: that write was never acknowledged
      }
      this.lines++;
      if (line.op === "put") {
        if (!this.records.has(line.rec.message.id)) this.records.set(line.rec.message.id, line.rec);
      } else {
        const r = this.records.get(line.id);
        if (r) Object.assign(r, line.patch);
      }
    }
  }

  private append(line: LogLine): void {
    fs.writeSync(this.fd, `${JSON.stringify(line)}\n`);
    fs.fsyncSync(this.fd);
    this.lines++;
  }

  /** Rewrites the log with one line per record. */
  compact(): void {
    const tmp = `${this.logFile}.tmp`;
    const body = [...this.records.values()].map((rec) => JSON.stringify({ op: "put", rec } satisfies LogLine)).join("\n");
    fs.writeFileSync(tmp, body ? `${body}\n` : "", { mode: 0o600 });
    const fd = fs.openSync(tmp, "r");
    fs.fsyncSync(fd);
    fs.closeSync(fd);
    fs.closeSync(this.fd);
    fs.renameSync(tmp, this.logFile);
    this.fd = fs.openSync(this.logFile, "a", 0o600);
    this.lines = this.records.size;
  }

  get(id: string): MessageRecord | undefined {
    return this.records.get(id);
  }

  /** Stores a new message durably. Returns false (and changes nothing) when the id is known. */
  put(rec: MessageRecord): boolean {
    if (this.records.has(rec.message.id)) return false;
    this.append({ op: "put", rec });
    this.records.set(rec.message.id, rec);
    return true;
  }

  update(id: string, patch: Partial<MessageRecord>): MessageRecord | undefined {
    const r = this.records.get(id);
    if (!r) return undefined;
    this.append({ op: "upd", id, patch });
    Object.assign(r, patch);
    return r;
  }

  all(): MessageRecord[] {
    return [...this.records.values()];
  }

  /** A thread's mailbox entries in a given state, in arrival order. */
  inbox(threadId: string, states: Array<NonNullable<MessageRecord["inState"]>>): MessageRecord[] {
    const out: MessageRecord[] = [];
    for (const r of this.records.values()) if (r.inbox && r.recipient === threadId && r.inState && states.includes(r.inState)) out.push(r);
    // Arrival order, and within a sender-recipient pair, sequence order.
    out.sort((a, b) => (a.message.from === b.message.from ? a.message.seq - b.message.seq : a.storedAt - b.storedAt));
    return out;
  }

  save(): void {
    writeJsonAtomic(path.join(this.dir, "state.json"), this.state);
  }

  thread(id: string): ThreadMesh {
    let t = this.state.threads[id];
    if (!t) {
      t = { depth: 0, children: [], peers: [], notify: [] };
      this.state.threads[id] = t;
    }
    return t;
  }

  peekThread(id: string): ThreadMesh | undefined {
    return this.state.threads[id];
  }

  nextSeq(from: Address, to: Address): number {
    const key = `${from}\u0000${to}`;
    const n = (this.state.seqs[key] ?? 0) + 1;
    this.state.seqs[key] = n;
    return n;
  }

  close(): void {
    try {
      fs.closeSync(this.fd);
    } catch {
      /* already closed */
    }
  }
}
