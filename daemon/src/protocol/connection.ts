// One host connection, server role: framing, the frozen core, `initialize`, the dialect, and
// priority scheduling. The session layer above sees only domain values: it never
// learns the negotiated version (docs/09-compatibility.md#the-three-rules).

import type { Duplex } from "node:stream";
import { performance } from "node:perf_hooks";
import type {
  ClientInfo,
  DaemonBusy,
  DaemonInfo,
  HandoverState,
  HandoverWhen,
  TargetFacts,
  ThreadEventParams,
} from "./generated/types.js";
import { FROZEN_CORE_METHODS, METHODS, METHOD_INFO, NOTIFICATIONS, NOTIFICATION_INFO } from "./generated/constants.js";
import type { MethodInfo } from "./generated/constants.js";
import { FrameDecoder, FrameError, MAX_FRAME_LEN, encodeMessageFrame, type Frame } from "./frame.js";
import { encodeMessageFrames, MessageChunks } from "./message-chunks.js";
import { decodeMessage, encodeMessage, MessageDecodeError, RpcError, type Message } from "./rpc.js";
import { Scheduler, type TrafficClass } from "./scheduler.js";
import { negotiate, NegotiationError, parseRange, select, supportedRange, type Negotiated } from "./negotiation.js";
import type { Dialect, FeatureSet } from "./dialect.js";

/** What the daemon provides to the frozen core and to `initialize`. */
export interface FrozenCoreHost {
  /** Native PTY probe result, absent on hosts without a terminal backend. */
  terminalSupport?(): { unavailableReason?: string };
  daemonInfo(): DaemonInfo;
  targetFacts(): TargetFacts;
  busy(): DaemonBusy;
  /** Idempotent. "exiting": the connection closes once the answer is written. */
  handover(when: HandoverWhen): HandoverState;
}

/** What a negotiated connection hands its requests to. */
export interface SessionHandler {
  /** A request (never a frozen-core one). Return the domain result or throw RpcError. */
  handle(method: string, params: unknown): Promise<unknown> | unknown;
  /** A data frame on a stream this side receives. */
  onData?(stream: number, bytes: Buffer): void;
  /** A notification from the host other than `stream/credit`. */
  onNotification?(method: string, params: unknown): void;
  /** The connection ended. */
  onClose?(): void;
}

export interface ConnectionView {
  readonly client: ClientInfo;
  /** Wire features of the negotiated version (fold capabilities through it). */
  readonly features: FeatureSet;
  /** Whether this daemon adapts to an older client. */
  readonly peerIsOlder: boolean;
  /** Diagnostics only. */
  readonly label: string;
  notify(method: string, params: unknown): void;
  sendEvent(params: ThreadEventParams): boolean;
  /** Bytes queued but not yet handed to the transport, plus the transport's own buffer. */
  backlogBytes(): number;
  /** Calls `listener` once, when the outbound backlog is below the low-water mark. */
  whenWritable(listener: () => void): void;
  openStream(stream: number, credit: number, interactive?: boolean): void;
  sendData(stream: number, bytes: Buffer, drained: () => void): void;
  cancelStream(stream: number): void;
  grantCredit(stream: number, bytes: number): void;
  /**
   * A request this daemon originates (the mesh: `mesh/relay`, `mesh/query`, `mesh/usage`).
   * Resolves with the host's result; rejects with an RpcError, or when the connection closes.
   */
  request(method: string, params: unknown, timeoutMs?: number): Promise<unknown>;
  close(): void;
}

const LOW_WATER = 64 * 1024;

function info(method: string): MethodInfo | undefined {
  return (METHOD_INFO as Record<string, MethodInfo>)[method];
}

export class ServerConnection implements ConnectionView {
  private readonly decoder = new FrameDecoder();
  private messageChunks: MessageChunks | null = null;
  private readonly scheduler = new Scheduler();
  private dialect: Dialect | null = null;
  private negotiated: Negotiated | null = null;
  private session: SessionHandler | null = null;
  private clientInfo: ClientInfo = { name: "unknown", version: "unknown" };
  private closed = false;
  private closing = false;
  private waitingDrain = false;
  private bulkInFlight = false;
  private writableListeners: Array<() => void> = [];
  private closeListeners: Array<() => void> = [];
  /** Requests this daemon originated, by id (their own id space). */
  private outgoing = new Map<number, { method: string; resolve: (v: unknown) => void; reject: (e: Error) => void; timer: NodeJS.Timeout | null }>();
  private nextOutgoingId = 1;

  constructor(
    private readonly transport: Duplex,
    private readonly host: FrozenCoreHost,
    private readonly openSession: (view: ConnectionView) => SessionHandler,
    private readonly log: (line: string) => void = () => {},
  ) {
    transport.on("data", (chunk: Buffer) => this.onBytes(chunk));
    transport.on("drain", () => {
      this.waitingDrain = false;
      this.flush();
    });
    transport.on("error", (e) => this.log(`connection error: ${e.message}`));
    transport.on("close", () => this.teardown());
    transport.on("end", () => this.teardown());
  }

  get client(): ClientInfo {
    return this.clientInfo;
  }

  get features(): FeatureSet {
    return this.dialect!.features();
  }

  get peerIsOlder(): boolean {
    return this.negotiated?.peerIsOlder() ?? false;
  }

  get label(): string {
    return this.negotiated?.diagnosticLabel() ?? "not negotiated";
  }

  get isClosed(): boolean {
    return this.closed;
  }

  onClose(listener: () => void): void {
    this.closeListeners.push(listener);
  }

  whenWritable(listener: () => void): void {
    this.writableListeners.push(listener);
    if (this.backlogBytes() < LOW_WATER) this.fireWritable();
  }

  private writableScheduled = false;

  private fireWritable(): void {
    if (this.writableScheduled || this.writableListeners.length === 0) return;
    this.writableScheduled = true;
    setImmediate(() => {
      this.writableScheduled = false;
      if (this.closed) return;
      const listeners = this.writableListeners;
      this.writableListeners = [];
      for (const l of listeners) l();
    });
  }

  // ── Inbound ────────────────────────────────────────────────────────────────────────────

  private onBytes(chunk: Buffer): void {
    if (this.closed) return;
    let frames: Frame[];
    try {
      frames = this.decoder.decode(chunk);
    } catch (e) {
      // Framing cannot resynchronise: close the stream.
      this.log(`fatal framing error: ${(e as FrameError).message}`);
      this.destroy();
      return;
    }
    for (const physical of frames) {
      if (this.closed) return;
      let frame: Frame | null;
      try {
        frame = this.messageChunks ? this.messageChunks.accept(physical) : physical;
      } catch (e) {
        this.log(`fatal message-fragment error: ${(e as Error).message}`);
        this.destroy();
        return;
      }
      if (!frame) continue;
      if (frame.kind === "data") {
        if (this.session?.onData) {
          try {
            this.session.onData(frame.stream, frame.bytes);
          } catch (e) {
            this.log(`fatal data-stream error: ${(e as Error).message}`);
            this.destroy();
            return;
          }
        }
        continue;
      }
      let message: Message;
      try {
        message = decodeMessage(frame.payload);
      } catch (e) {
        const err = e as MessageDecodeError;
        this.reply({ type: "error", id: err.id, error: { code: err.code, message: err.message } }, "control");
        continue;
      }
      this.onMessage(message);
    }
  }

  private onMessage(message: Message): void {
    switch (message.type) {
      case "request":
        void this.onRequest(message.id, message.method, message.params);
        return;
      case "notification":
        if (message.method === NOTIFICATIONS.STREAM_CREDIT) {
          const p = message.params as { stream?: unknown; bytes?: unknown };
          if (typeof p.stream === "number" && typeof p.bytes === "number") this.scheduler.grantCredit(p.stream, p.bytes);
          this.flush();
        } else if (this.session) {
          this.session.onNotification?.(message.method, this.dialect!.decodeParams(message.method, message.params));
        }
        return;
      case "response":
      case "error": {
        // An answer to a request this daemon originated (the mesh).
        const id = typeof message.id === "number" ? message.id : Number.NaN;
        const pending = this.outgoing.get(id);
        if (!pending) return;
        this.outgoing.delete(id);
        if (pending.timer) clearTimeout(pending.timer);
        if (message.type === "error") pending.reject(new RpcError(message.error.code, message.error.message, message.error.data));
        else pending.resolve(this.dialect ? this.dialect.decodeHostResult(pending.method, message.result) : message.result);
        return;
      }
      default:
        return;
    }
  }

  request(method: string, params: unknown, timeoutMs = 30000): Promise<unknown> {
    if (this.closed || this.closing || !this.dialect) return Promise.reject(new RpcError("not_initialized", "the host connection is not open"));
    const id = this.nextOutgoingId++;
    return new Promise<unknown>((resolve, reject) => {
      const timer =
        timeoutMs > 0
          ? setTimeout(() => {
              this.outgoing.delete(id);
              reject(new RpcError("timeout", `${method}: the host did not answer within ${Math.round(timeoutMs / 1000)} s`));
            }, timeoutMs)
          : null;
      timer?.unref?.();
      this.outgoing.set(id, { method, resolve, reject, timer });
      const cls = info(method)?.requestClass ?? "control";
      if (!this.reply({ type: "request", id, method, params: this.dialect!.encodeRequestParams(method, params ?? {}) }, cls)) {
        this.outgoing.delete(id);
        if (timer) clearTimeout(timer);
        reject(new RpcError("limit_exceeded", "request exceeds the connection's message limit"));
      }
    });
  }

  private responseClass(method: string): TrafficClass {
    return info(method)?.responseClass ?? "control";
  }

  private async onRequest(id: number | string, method: string, params: unknown): Promise<void> {
    if ((FROZEN_CORE_METHODS as readonly string[]).includes(method)) {
      this.frozenCore(id, method, params);
      return;
    }
    if (!this.session || !this.dialect) {
      this.reply(
        { type: "error", id, error: { code: "not_initialized", message: `${method} before a successful initialize` } },
        "control",
      );
      return;
    }
    try {
      const domain = this.dialect.decodeParams(method, params ?? {});
      const result = await this.session.handle(method, domain);
      if (this.closed) return;
      this.reply({ type: "response", id, result: this.dialect.encodeResult(method, result ?? {}) }, this.responseClass(method));
    } catch (e) {
      if (this.closed) return;
      const error =
        e instanceof RpcError
          ? e.toObject()
          : { code: "internal_error" as const, message: (e as Error)?.message ?? String(e) };
      if (!(e instanceof RpcError)) this.log(`internal error in ${method}: ${(e as Error)?.stack ?? e}`);
      this.reply({ type: "error", id, error }, "control");
    }
  }

  private frozenCore(id: number | string, method: string, params: unknown): void {
    switch (method) {
      case METHODS.INITIALIZE:
        this.initialize(id, params);
        return;
      case METHODS.DAEMON_STATUS: {
        const daemon = this.host.daemonInfo();
        this.reply(
          {
            type: "response",
            id,
            result: { build: daemon.build, protocol: supportedRange(), stateFormat: daemon.stateFormat, busy: this.host.busy() },
          },
          "control",
        );
        return;
      }
      case METHODS.DAEMON_HANDOVER: {
        const when = (params as { when?: unknown })?.when;
        if (when !== "idle" && when !== "now") {
          this.reply({ type: "error", id, error: { code: "invalid_params", message: "when must be \"idle\" or \"now\"" } }, "control");
          return;
        }
        const state = this.host.handover(when);
        this.reply({ type: "response", id, result: { state } }, "control");
        return;
      }
    }
  }

  private initialize(id: number | string, params: unknown): void {
    if (this.session) {
      this.reply({ type: "error", id, error: { code: "invalid_request", message: "already initialized" } }, "control");
      return;
    }
    const p = (params ?? {}) as { protocol?: unknown; client?: unknown; messageChunks?: unknown };
    const remote = parseRange(p.protocol);
    const client = p.client as ClientInfo | undefined;
    if (!remote || !client || typeof client.name !== "string" || typeof client.version !== "string") {
      this.reply(
        { type: "error", id, error: { code: "invalid_params", message: "initialize needs protocol { min, max } and client { name, version }" } },
        "control",
      );
      return;
    }
    let negotiated: Negotiated;
    try {
      negotiated = negotiate(remote);
    } catch (e) {
      if (e instanceof NegotiationError) {
        this.reply({ type: "error", id, error: e.toErrorObject() }, "control");
        return;
      }
      throw e;
    }
    this.negotiated = negotiated;
    this.dialect = select(negotiated);
    this.clientInfo = client;
    const terminal = this.host.terminalSupport?.();
    const result = {
      protocol: negotiated.answer(),
      ...(p.messageChunks === true ? { messageChunks: true } : {}),
      features: this.dialect.features().fold(terminal && !terminal.unavailableReason ? ["portForward", "fileView", "terminal", "budgetTimeLimit", "voiceInput"] : ["portForward", "fileView", "budgetTimeLimit", "voiceInput"]),
      terminalUnavailableReason: terminal?.unavailableReason,
      schemaHash: this.dialect.schemaHash(),
      daemon: this.host.daemonInfo(),
      target: this.host.targetFacts(),
    };
    this.reply({ type: "response", id, result }, "control");
    if (p.messageChunks === true) this.messageChunks = new MessageChunks();
    this.session = this.openSession(this);
  }

  // ── Outbound ───────────────────────────────────────────────────────────────────────────

  private reply(message: Message, cls: TrafficClass): boolean {
    if (this.closed) return false;
    let frames: Buffer[];
    try {
      const lane = this.messageChunks ? (cls === "control" ? 0 : 1) : null;
      frames = encodeMessageFrames(encodeMessage(message), lane);
    } catch (e) {
      if (!(e instanceof FrameError)) throw e;
      if (message.type !== "response" && message.type !== "error") {
        this.log(`cannot send ${message.method}: ${e.message}`);
        return false;
      }
      const error: Message = { type: "error", id: message.id, error: { code: "limit_exceeded", message: "result exceeds the connection's message limit" } };
      const payload = encodeMessage(error);
      if (Buffer.byteLength(payload) >= MAX_FRAME_LEN) {
        this.log("cannot send an error with an oversized request id");
        return false;
      }
      frames = [encodeMessageFrame(payload)];
    }
    for (const frame of frames) this.scheduler.pushFrame(cls, frame);
    this.flush();
    return true;
  }

  notify(method: string, params: unknown): boolean {
    if (!this.dialect || this.closed) return false;
    const cls = (NOTIFICATION_INFO as Record<string, { class: TrafficClass }>)[method]?.class ?? "control";
    return this.reply({ type: "notification", method, params: this.dialect.encodeNotification(method, params) }, cls);
  }

  sendEvent(params: ThreadEventParams): boolean {
    return this.notify(NOTIFICATIONS.THREAD_EVENT, params);
  }

  openStream(stream: number, credit: number, interactive = false): void {
    this.scheduler.openStream(stream, credit, interactive);
  }

  sendData(stream: number, bytes: Buffer, drained: () => void): void {
    this.scheduler.sendData(stream, bytes, drained);
    this.flush();
  }

  cancelStream(stream: number): void {
    this.scheduler.cancelStream(stream);
  }

  grantCredit(stream: number, bytes: number): void {
    this.notify(NOTIFICATIONS.STREAM_CREDIT, { stream, bytes });
  }

  backlogBytes(): number {
    return this.scheduler.pendingControlBytes() + this.scheduler.pendingBulkBytes() + (this.transport.writableLength ?? 0);
  }

  /** Writes frames while the transport takes them. Bulk frames go one at a time. */
  private flush(): void {
    if (this.closed) return;
    while (!this.waitingDrain && !this.bulkInFlight) {
      const next = this.scheduler.next();
      if (!next) break;
      if (next.class !== "control") {
        this.bulkInFlight = true;
        const started = performance.now();
        const len = next.bytes.length;
        const ok = this.transport.write(next.bytes, () => {
          this.bulkInFlight = false;
          if (next.class === "bulk") this.scheduler.onWritten(len, performance.now() - started);
          this.flush();
        });
        if (!ok) this.waitingDrain = true;
      } else if (!this.transport.write(next.bytes)) {
        this.waitingDrain = true;
      }
    }
    if (this.closing && this.scheduler.isIdle() && !this.bulkInFlight) {
      this.transport.end();
      return;
    }
    if (this.backlogBytes() < LOW_WATER) this.fireWritable();
  }

  /** Closes once everything queued is written. */
  close(): void {
    if (this.closed || this.closing) return;
    this.closing = true;
    this.flush();
  }

  destroy(): void {
    this.transport.destroy();
    this.teardown();
  }

  private teardown(): void {
    if (this.closed) return;
    this.closed = true;
    for (const p of this.outgoing.values()) {
      if (p.timer) clearTimeout(p.timer);
      p.reject(new RpcError("connection_closed", `${p.method}: the host connection closed`));
    }
    this.outgoing.clear();
    this.session?.onClose?.();
    for (const l of this.closeListeners) l();
  }
}
