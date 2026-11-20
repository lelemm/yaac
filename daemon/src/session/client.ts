// One attached client: request handling and its live event stream.
//
// Events are pulled from each subscribed thread's journal as the connection can take them,
// so a slow client costs no memory beyond the journal itself. When a client falls behind, a
// run of deltas for one item is collapsed into one `itemUpdated` snapshot whose `throughSeq`
// names the last event it replaces (docs/03-protocol.md#backpressure).

import type { JournalPosition, ThreadEventParams } from "../protocol/generated/types.js";
import { METHODS } from "../protocol/generated/constants.js";
import { RpcError } from "../protocol/rpc.js";
import type { ConnectionView, SessionHandler } from "../protocol/connection.js";
import type { ClientLink, Core } from "./core.js";
import { eventItemId } from "./items.js";
import { lastSeqOf, type JournalEntry } from "./journal.js";
import { fsList, projectResolve } from "./fsinfo.js";

export interface ClientOptions {
  /** Stop handing events to the connection above this many queued bytes. */
  highWaterBytes: number;
  /** Collapse delta runs when this many events behind the journal head. */
  collapseBehind: number;
}

export const DEFAULT_CLIENT_OPTIONS: ClientOptions = { highWaterBytes: 256 * 1024, collapseBehind: 64 };

/** Extra request handlers (the mesh package registers its methods here). */
export type MethodHandler = (client: ClientSession, params: Record<string, unknown>) => Promise<unknown> | unknown;

export interface DaemonHooks {
  diagnostics(): Array<{ title: string; lines: string[] }>;
  extraMethods: Map<string, MethodHandler>;
  log(line: string): void;
}

interface Subscription {
  cursor: number;
  epoch: number;
  mode: "replay" | "reset";
  snapshotSeen: boolean;
}

function obj(params: unknown): Record<string, unknown> {
  if (typeof params !== "object" || params === null || Array.isArray(params)) {
    throw new RpcError("invalid_params", "params must be an object");
  }
  return params as Record<string, unknown>;
}

function str(p: Record<string, unknown>, key: string): string {
  const v = p[key];
  if (typeof v !== "string" || v.length === 0) throw new RpcError("invalid_params", `${key} is required`);
  return v;
}

const MESH_PREFIX = "mesh/";

export class ClientSession implements SessionHandler, ClientLink {
  readonly key: string;
  readonly host: string | null;
  private subs = new Map<string, Subscription>();
  private pumping = false;
  private pumpScheduled = false;
  private waitingWritable = false;
  private closed = false;
  private closeListeners: Array<() => void> = [];

  constructor(
    private readonly core: Core,
    readonly view: ConnectionView,
    connectionNumber: number,
    private readonly hooks: DaemonHooks,
    private readonly opts: ClientOptions = DEFAULT_CLIENT_OPTIONS,
  ) {
    this.key = view.client.id ?? `conn-${connectionNumber}`;
    this.host = view.client.host ?? null;
    core.clients.add(this);
  }

  // ── Requests ───────────────────────────────────────────────────────────────────────────

  async handle(method: string, raw: unknown): Promise<unknown> {
    const p = obj(raw);
    const core = this.core;
    switch (method) {
      case METHODS.PING:
        return {};
      case METHODS.DIAGNOSTICS_GET:
        return core.diagnostics([
          ...this.hooks.diagnostics(),
          { title: "connection", lines: [this.view.label, `client: ${this.view.client.name} ${this.view.client.version}`, `adapting to an older client: ${this.view.peerIsOlder ? "yes" : "no"}`] },
        ]);
      case METHODS.PROVIDER_LIST:
        return { providers: core.registry.all().map((i) => i.descriptor) };
      case METHODS.PROVIDER_REFRESH: {
        const id = typeof p.instance === "string" ? p.instance : undefined;
        await core.registry.probe(id);
        const targets = id ? [core.registry.require(id)] : core.registry.all();
        await Promise.all(targets.map((i) => core.scanInstance(i)));
        return {};
      }
      case METHODS.PROVIDER_INSTANCE_LIST:
        return { instances: core.registry.configs() };
      case METHODS.PROVIDER_INSTANCE_ADD:
        return core.ops.run(method, p.opId, async () => ({ instance: await core.registry.add(p as never) }));
      case METHODS.PROVIDER_INSTANCE_UPDATE:
        return core.ops.run(method, p.opId, async () => ({ instance: await core.registry.update(p as never) }));
      case METHODS.PROVIDER_INSTANCE_REMOVE:
        return core.ops.run(method, p.opId, async () => {
          await core.registry.remove(str(p, "id"));
          return {};
        });
      case METHODS.THREAD_SYNC:
        return core.index.sync(typeof p.token === "string" ? p.token : undefined, typeof p.maxBytes === "number" ? p.maxBytes : null, (s, h) =>
          core.renderSummary(s, h, this.key),
        );
      case METHODS.THREAD_START:
        return core.threadStart(this, p as never);
      case METHODS.THREAD_READ:
        return core.read(this, { threadId: str(p, "threadId"), cursor: (p.cursor as string | null) ?? null, maxBytes: (p.maxBytes as number | null) ?? null });
      case METHODS.THREAD_SUBSCRIBE: {
        const threadId = str(p, "threadId");
        const result = await core.subscribe(
          this,
          { threadId, epoch: p.epoch as number | null, afterSeq: p.afterSeq as number | null },
          (cursor, mode) => {
            this.subs.set(threadId, { cursor, epoch: core.position(threadId).epoch, mode, snapshotSeen: false });
          },
        );
        // Replayed events follow the response (they are queued behind it).
        setImmediate(() => this.wake(threadId));
        return result;
      }
      case METHODS.THREAD_UNSUBSCRIBE:
        this.subs.delete(str(p, "threadId"));
        return {};
      case METHODS.THREAD_ACK: {
        const threadId = str(p, "threadId");
        if (typeof p.epoch !== "number" || typeof p.seq !== "number") throw new RpcError("invalid_params", "epoch and seq are required");
        core.ack(this, threadId, p.epoch, p.seq);
        return {};
      }
      case METHODS.THREAD_FORK:
        return core.threadFork(this, p as never);
      case METHODS.THREAD_FORK_ONLY:
        return core.threadForkOnly(p as never);
      case METHODS.THREAD_UPDATE:
        return core.threadUpdate(this, p as never);
      case METHODS.ITEM_READ:
        return core.itemRead({ threadId: str(p, "threadId"), itemId: str(p, "itemId"), offset: typeof p.offset === "number" ? p.offset : 0, maxBytes: p.maxBytes as number });
      case METHODS.PROJECT_LIST:
        return core.listProjects();
      case METHODS.PROJECT_CREATE:
        return core.projectCreate(p as never);
      case METHODS.PROJECT_RENAME:
        return core.projectRename(p as never);
      case METHODS.PROJECT_DELETE:
        await core.projectDelete(p as never);
        return {};
      case METHODS.THREAD_FORK_PREVIEW:
        return core.forkPreview(p as never);
      case METHODS.THREAD_RENAME:
        await core.threadRename(p as never);
        return {};
      case METHODS.THREAD_ARCHIVE:
        await core.threadArchive(p as never);
        return {};
      case METHODS.THREAD_DELETE:
        await core.threadDelete(this, p as never);
        return {};
      case METHODS.TURN_START:
        return core.turnStart(this, p as never);
      case METHODS.TURN_INTERRUPT:
        await core.turnInterrupt(this, { threadId: str(p, "threadId"), turnId: str(p, "turnId") });
        return {};
      case METHODS.TURN_STEER:
        await core.turnSteer(this, p as never);
        return {};
      case METHODS.REQUEST_RESPOND:
        await core.requestRespond(this, p as never);
        return {};
      case METHODS.UPLOAD_BEGIN:
        return core.uploads.begin(p as never, this);
      case METHODS.UPLOAD_END:
        return core.uploads.end(str(p, "uploadId"), this);
      case METHODS.UPLOAD_CANCEL:
        core.uploads.cancel(str(p, "uploadId"), this);
        return {};
      case METHODS.UPLOAD_READ: {
        if (typeof p.maxBytes !== "number") throw new RpcError("invalid_params", "maxBytes is required");
        return core.uploads.read(str(p, "uploadId"), typeof p.offset === "number" ? p.offset : 0, p.maxBytes);
      }
      case METHODS.FS_LIST:
        return fsList(p as never);
      case METHODS.PROJECT_RESOLVE:
        return projectResolve(str(p, "path"));
      default: {
        const extra = this.hooks.extraMethods.get(method);
        if (extra) return extra(this, p);
        throw new RpcError(
          "method_not_found",
          method.startsWith(MESH_PREFIX) ? `${method} is not available in this build of yaacd` : `unknown method ${method}`,
        );
      }
    }
  }

  onData(stream: number, bytes: Buffer): void {
    this.core.uploads.onData(stream, bytes, this, (s, n) => this.view.grantCredit(s, n));
  }

  /** Mesh (WP7): called once when this client goes away. */
  onClosed(listener: () => void): void {
    if (this.closed) listener();
    else this.closeListeners.push(listener);
  }

  /** Mesh (WP7): a request this daemon originates to this host (`mesh/relay`, `mesh/query`, `mesh/usage`). */
  requestHost(method: string, params: unknown, timeoutMs?: number): Promise<unknown> {
    return this.view.request(method, params, timeoutMs);
  }

  get isClosed(): boolean {
    return this.closed;
  }

  onClose(): void {
    for (const l of this.closeListeners.splice(0)) {
      try {
        l();
      } catch {
        /* a listener must not break teardown */
      }
    }
    this.closed = true;
    this.core.clients.delete(this);
    this.core.uploads.dropOwner(this);
    this.subs.clear();
  }

  notify(method: string, params: unknown): void {
    if (!this.closed) this.view.notify(method, params);
  }

  onSnapshot(threadId: string, position: JournalPosition): void {
    const sub = this.subs.get(threadId);
    // After a reset, events strictly after the snapshot's position follow it.
    if (sub && sub.mode === "reset" && !sub.snapshotSeen && position.epoch === sub.epoch) {
      sub.snapshotSeen = true;
      if (position.seq > sub.cursor) sub.cursor = position.seq;
    }
  }

  isSubscribed(threadId: string): boolean {
    return this.subs.has(threadId);
  }

  // ── Live events ────────────────────────────────────────────────────────────────────────

  wake(threadId: string): void {
    if (this.closed || !this.subs.has(threadId) || this.pumpScheduled || this.waitingWritable) return;
    this.pumpScheduled = true;
    queueMicrotask(() => {
      this.pumpScheduled = false;
      this.pump();
    });
  }

  private pump(): void {
    if (this.pumping || this.closed) return;
    this.pumping = true;
    try {
      let progressed = true;
      while (progressed) {
        progressed = false;
        for (const [threadId, sub] of this.subs) {
          if (this.view.backlogBytes() >= this.opts.highWaterBytes) {
            this.waitingWritable = true;
            this.view.whenWritable(() => {
              this.waitingWritable = false;
              this.pump();
            });
            return;
          }
          if (this.sendNext(threadId, sub)) progressed = true;
        }
      }
    } finally {
      this.pumping = false;
    }
  }

  /** Sends the next event (or collapsed run) of one thread. Returns whether it sent one. */
  private sendNext(threadId: string, sub: Subscription): boolean {
    const thread = this.core.thread(threadId);
    if (!thread || thread.deleted) {
      this.subs.delete(threadId);
      return false;
    }
    const { entries, start, epoch, headSeq } = this.core.entriesAfter(threadId, sub.cursor);
    if (epoch !== sub.epoch || start >= entries.length) return false;
    const first = entries[start]!;
    const behind = headSeq - sub.cursor > this.opts.collapseBehind || this.view.backlogBytes() >= this.opts.highWaterBytes / 2;
    if (behind && (first.event.type === "itemDelta" || (first.event.type === "itemUpdated" && first.through === undefined))) {
      const id = eventItemId(first.event);
      const foldable = (e: JournalEntry) => (e.event.type === "itemDelta" || e.event.type === "itemUpdated") && eventItemId(e.event) === id;
      let j = start + 1;
      while (j < entries.length && foldable(entries[j]!)) j++;
      if (id && j - start >= 2) {
        const through = lastSeqOf(entries[j - 1]!);
        const item = this.core.itemStateAt(threadId, id, through);
        if (item) {
          this.send({ threadId, epoch, seq: first.seq, event: { type: "itemUpdated", item, throughSeq: through } });
          sub.cursor = through;
          return true;
        }
      }
    }
    this.send({ threadId, epoch, seq: first.seq, event: this.core.renderEvent(first, this.key) });
    sub.cursor = lastSeqOf(first);
    return true;
  }

  private send(params: ThreadEventParams): void {
    this.view.sendEvent(params);
  }
}
