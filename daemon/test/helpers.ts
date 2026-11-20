// Test helpers: temporary homes, an in-process daemon, a raw protocol client, and a client
// model that applies journal events the way a host must.

import * as fs from "node:fs";
import * as net from "node:net";
import * as os from "node:os";
import * as path from "node:path";
import { FrameDecoder, encodeMessageFrame, encodeDataFrame, type Frame } from "../src/protocol/frame.js";
import { decodeMessage, encodeMessage, type Message } from "../src/protocol/rpc.js";
import type { ErrorObject, Item, ThreadEventParams } from "../src/protocol/generated/types.js";
import { Daemon, type DaemonConfig } from "../src/session/daemon.js";
import type { FakeProvider } from "../src/providers/fake/index.js";
import { applyDelta } from "../src/session/items.js";

export const FAKE = "pi_fake";

export function tmpHome(prefix = "yaacd-test-"): string {
  // Short paths: Unix socket paths are limited in length.
  return fs.mkdtempSync(path.join(os.tmpdir(), prefix));
}

export async function startDaemon(home: string, extra: Partial<DaemonConfig> = {}): Promise<Daemon> {
  return Daemon.start({
    home,
    build: "0.1.0+test",
    env: { YAAC_FAKE_PROVIDER: "1", ...(extra.env ?? {}) },
    idleTimeoutMs: 0,
    tempDir: path.join(home, "tmp"),
    coalesceMs: 5,
    ...extra,
  });
}

export function fakeOf(daemon: Daemon, id = FAKE): FakeProvider {
  return daemon.core.registry.get(id)!.provider as FakeProvider;
}

export class RpcFailure extends Error {
  constructor(readonly error: ErrorObject) {
    super(`${error.code}: ${error.message}`);
  }
  get code(): string {
    return this.error.code;
  }
}

export function sleep(ms: number): Promise<void> {
  return new Promise((r) => setTimeout(r, ms));
}

export async function until<T>(fn: () => T | Promise<T>, what = "condition", timeoutMs = 10000): Promise<NonNullable<T>> {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const v = await fn();
    if (v) return v as NonNullable<T>;
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}`);
    await sleep(10);
  }
}

/** A raw protocol client over a Unix socket (or any duplex). */
export class TestClient {
  readonly messages: Message[] = [];
  readonly notifications: Array<{ method: string; params: any }> = [];
  readonly rawIn: Buffer[] = [];
  private decoder = new FrameDecoder();
  private nextId = 1;
  private waiters = new Map<number | string, (m: Message) => void>();
  closed = false;
  private closeWaiters: Array<() => void> = [];

  constructor(readonly socket: net.Socket) {
    socket.on("data", (chunk: Buffer) => {
      this.rawIn.push(Buffer.from(chunk));
      for (const f of this.decoder.decode(chunk)) this.onFrame(f);
    });
    socket.on("close", () => {
      this.closed = true;
      for (const w of this.closeWaiters) w();
    });
    socket.on("error", () => {});
  }

  static connect(socketPath: string): Promise<TestClient> {
    return new Promise((resolve, reject) => {
      const s = net.connect(socketPath);
      s.once("connect", () => resolve(new TestClient(s)));
      s.once("error", reject);
    });
  }

  private onFrame(f: Frame): void {
    if (f.kind !== "message") return;
    const m = decodeMessage(f.payload);
    this.messages.push(m);
    if (m.type === "notification") this.notifications.push({ method: m.method, params: m.params });
    if ((m.type === "response" || m.type === "error") && m.id !== null) {
      const w = this.waiters.get(m.id);
      if (w) {
        this.waiters.delete(m.id);
        w(m);
      }
    }
  }

  sendRaw(bytes: Buffer): void {
    this.socket.write(bytes);
  }

  sendData(stream: number, bytes: Buffer): void {
    this.socket.write(encodeDataFrame(stream, bytes));
  }

  notify(method: string, params: unknown): void {
    this.socket.write(encodeMessageFrame(encodeMessage({ type: "notification", method, params })));
  }

  async call(method: string, params: unknown = {}): Promise<Message> {
    const id = this.nextId++;
    const p = new Promise<Message>((resolve) => this.waiters.set(id, resolve));
    this.socket.write(encodeMessageFrame(encodeMessage({ type: "request", id, method, params })));
    return p;
  }

  async request<T = any>(method: string, params: unknown = {}): Promise<T> {
    const m = await this.call(method, params);
    if (m.type === "error") throw new RpcFailure(m.error);
    return (m as { result: T }).result;
  }

  async initialize(id = "c_test", host = "test-host"): Promise<any> {
    return this.request("initialize", { protocol: { min: 1, max: 1 }, client: { name: "yaac-test", version: "0.0.0", id, host } });
  }

  events(threadId?: string): ThreadEventParams[] {
    return this.notifications
      .filter((n) => n.method === "thread/event" && (!threadId || n.params.threadId === threadId))
      .map((n) => n.params as ThreadEventParams);
  }

  async waitEvent(pred: (e: ThreadEventParams) => boolean, what = "event", timeoutMs = 10000): Promise<ThreadEventParams> {
    return until(() => this.events().find(pred), what, timeoutMs);
  }

  waitClosed(timeoutMs = 5000): Promise<void> {
    if (this.closed) return Promise.resolve();
    return new Promise((resolve, reject) => {
      const t = setTimeout(() => reject(new Error("connection did not close")), timeoutMs);
      this.closeWaiters.push(() => {
        clearTimeout(t);
        resolve();
      });
    });
  }

  close(): Promise<void> {
    this.socket.end();
    return this.waitClosed().catch(() => {
      this.socket.destroy();
    });
  }
}

export async function client(daemon: Daemon, id = "c_test", host = "test-host"): Promise<TestClient> {
  const c = await TestClient.connect(daemon.socketPath);
  await c.initialize(id, host);
  return c;
}

/** The host's view of a thread: items plus a position, updated by events. */
export class ClientModel {
  items = new Map<string, Item>();
  order: string[] = [];
  epoch = 0;
  seq = 0;
  applied = 0;
  duplicates = 0;

  /** Loads a snapshot (all pages, newest first per page). */
  loadSnapshot(pages: Array<{ items: Item[]; position: { epoch: number; seq: number } }>): void {
    this.items.clear();
    this.order = [];
    const all = pages.flatMap((p) => p.items).reverse();
    for (const i of all) this.upsert(structuredClone(i));
    this.epoch = pages[0]!.position.epoch;
    this.seq = pages[0]!.position.seq;
  }

  private upsert(item: Item): void {
    if (!this.items.has(item.id)) this.order.push(item.id);
    this.items.set(item.id, item);
  }

  apply(e: ThreadEventParams): void {
    if (e.epoch !== this.epoch) return;
    const last = (e.event as { throughSeq?: number | null }).throughSeq ?? e.seq;
    if (last <= this.seq) {
      this.duplicates++;
      return;
    }
    if (e.seq <= this.seq && e.event.type !== "itemUpdated") throw new Error(`event ${e.seq} overlaps position ${this.seq}`);
    if (e.seq > this.seq + 1) throw new Error(`gap: position ${this.seq}, next event ${e.seq}`);
    const ev = e.event as any;
    switch (e.event.type) {
      case "itemStarted":
      case "itemUpdated":
      case "itemCompleted":
        this.upsert(structuredClone(ev.item));
        break;
      case "itemDelta": {
        const it = this.items.get(ev.itemId);
        if (it) applyDelta(it, ev.field, ev.delta);
        break;
      }
      case "requestChanged": {
        const it = this.items.get(ev.itemId) as any;
        if (it) {
          it.state = ev.state;
          if (ev.resolution) it.resolution = ev.resolution;
          if (["resolved", "expired", "unknown"].includes(ev.state)) it.status = "completed";
        }
        break;
      }
    }
    this.seq = last;
    this.applied++;
  }

  list(): Item[] {
    return this.order.map((id) => this.items.get(id)!);
  }
}

/** Reads every page of a thread's snapshot. */
export async function readAll(c: TestClient, threadId: string, maxBytes?: number): Promise<Array<{ items: Item[]; position: { epoch: number; seq: number }; cursor?: string | null }>> {
  const pages = [];
  let cursor: string | null | undefined = null;
  for (;;) {
    const page: any = await c.request("thread/read", { threadId, cursor, ...(maxBytes ? { maxBytes } : {}) });
    pages.push(page);
    cursor = page.cursor;
    if (!cursor) break;
  }
  return pages;
}

export function text(input: string): Array<{ type: "text"; text: string }> {
  return [{ type: "text", text: input }];
}

let opCounter = 0;
export function opId(): string {
  return `op-${process.pid}-${Date.now()}-${++opCounter}`;
}

export async function startThread(c: TestClient, message: string, extra: Record<string, unknown> = {}): Promise<{ threadId: string; turnId: string }> {
  return c.request("thread/start", { opId: opId(), instance: FAKE, cwd: os.tmpdir(), input: text(message), ...extra });
}

export async function waitTurnDone(c: TestClient, threadId: string, turnId: string, timeoutMs = 10000): Promise<ThreadEventParams> {
  return c.waitEvent((e) => e.threadId === threadId && e.event.type === "turnCompleted" && (e.event as any).turnId === turnId, `turn ${turnId} to complete`, timeoutMs);
}

/** Comparable form of items (for equality across snapshot and replay). */
export function norm(items: Item[]): unknown[] {
  return items.map((i) => JSON.parse(JSON.stringify(i)));
}
