// A fake Codex app-server for tests: a WebSocket server on a Unix socket, like the real
// daemon's control socket. Replays recorded steps and answers other requests from handlers.

import { createHash } from "node:crypto";
import * as fs from "node:fs";
import * as http from "node:http";
import * as path from "node:path";
import { fileURLToPath } from "node:url";
import { WebSocketServer, type WebSocket } from "ws";

export interface Msg {
  id?: number | string;
  method?: string;
  params?: any;
  result?: any;
  error?: any;
}

export type Step =
  | { expect: string; reply?: unknown; error?: unknown; synthetic?: boolean }
  | { expectNotify: string }
  | { expectReply: number | string; recordedReply?: unknown }
  | { push: Msg; synthetic?: boolean };

export type Handler = (params: any, msg: Msg) => unknown | { __error: { code: number; message: string } } | Promise<unknown>;

const here = path.dirname(fileURLToPath(import.meta.url));

/** Reads a fixture from the source tree (tests run from dist/test-build/…). */
export function fixture<T = any>(name: string): T {
  const candidates = [
    path.join(here, name),
    path.resolve(here, "../../../../../test/fixtures/codex", name),
    path.resolve(process.cwd(), "test/fixtures/codex", name),
  ];
  for (const c of candidates) if (fs.existsSync(c)) return JSON.parse(fs.readFileSync(c, "utf8")) as T;
  throw new Error(`fixture ${name} not found`);
}

/** A deep merge of plain objects, as Codex's `upsert` does. */
function merge(into: unknown, value: unknown): unknown {
  const plain = (v: unknown): v is Record<string, unknown> => !!v && typeof v === "object" && !Array.isArray(v);
  if (!plain(into) || !plain(value)) return structuredClone(value);
  const out: Record<string, unknown> = { ...into };
  for (const [k, v] of Object.entries(value)) out[k] = merge(out[k], v);
  return out;
}

export function rpcError(code: number, message: string): { __error: { code: number; message: string } } {
  return { __error: { code, message } };
}

export class FakeCodex {
  readonly received: Msg[] = [];
  readonly handlers = new Map<string, Handler>();
  /** Replies the client sent to server requests, by id. */
  readonly replies = new Map<number | string, unknown>();
  connections = 0;
  /** The user's Codex configuration (`config/read`, `config/value/write`). */
  userConfig: Record<string, any> = {};
  /** Successful `config/value/write` calls. */
  configWrites = 0;
  /** While a script runs, unhandled requests wait for it; otherwise they get "method not found". */
  private scripted = 0;
  private sockets = new Set<WebSocket>();
  private current: WebSocket | null = null;
  private queue: Msg[] = [];
  private waiters: Array<{ pred: (m: Msg) => boolean; resolve: (m: Msg) => void }> = [];
  private requestWaiters: Array<{ method: string; resolve: (m: Msg) => void }> = [];

  private constructor(
    readonly socket: string,
    private readonly server: http.Server,
    private readonly wss: WebSocketServer,
  ) {}

  static async start(socket: string, init?: unknown): Promise<FakeCodex> {
    fs.mkdirSync(path.dirname(socket), { recursive: true });
    fs.rmSync(socket, { force: true });
    const server = http.createServer((_req, res) => {
      res.writeHead(426);
      res.end();
    });
    const wss = new WebSocketServer({ server });
    const fake = new FakeCodex(socket, server, wss);
    fake.handlers.set("initialize", () => init ?? { userAgent: "yaacd/0.171.0 (redacted)", codexHome: "/home/user/.codex", platformFamily: "unix", platformOs: "linux" });
    // The user's config.toml, as codex-cli 0.170.0 answers for it: `config/read` gives the
    // effective config and, with `includeLayers`, the user layer's version; `config/value/write`
    // upserts (deep merge) or replaces at a dotted key path and refuses a stale
    // `expectedVersion` with `configVersionConflict`.
    fake.handlers.set("config/read", (p) => ({
      config: { model: null, projects: Object.keys(fake.userConfig.projects ?? {}).length > 0 ? structuredClone(fake.userConfig.projects) : null },
      origins: {},
      layers: p?.includeLayers ? [{ name: { type: "user", file: "/home/user/.codex/config.toml", profile: null }, version: fake.configVersion(), config: structuredClone(fake.userConfig), disabledReason: null }] : null,
    }));
    fake.handlers.set("config/value/write", (p) => {
      if (p.expectedVersion && p.expectedVersion !== fake.configVersion()) {
        return { __error: { code: -32600, message: "Configuration was modified since last read. Fetch latest version and retry.", data: { config_write_error_code: "configVersionConflict" } } };
      }
      const keys = String(p.keyPath).split(".");
      let at: any = fake.userConfig;
      for (const k of keys.slice(0, -1)) at = at[k] ??= {};
      const last = keys.at(-1)!;
      at[last] = p.mergeStrategy === "upsert" ? merge(at[last], p.value) : p.value;
      fake.configWrites += 1;
      return { status: "ok", version: fake.configVersion(), filePath: "/home/user/.codex/config.toml", overriddenMetadata: null };
    });
    wss.on("connection", (ws) => fake.onConnection(ws));
    await new Promise<void>((resolve) => server.listen(socket, resolve));
    return fake;
  }

  private onConnection(ws: WebSocket): void {
    this.connections += 1;
    this.sockets.add(ws);
    this.current = ws;
    ws.on("close", () => this.sockets.delete(ws));
    ws.on("message", (data) => {
      const msg = JSON.parse(data.toString()) as Msg;
      this.received.push(msg);
      if (msg.method === undefined && msg.id !== undefined) this.replies.set(msg.id, msg.result ?? { error: msg.error });
      for (const w of [...this.waiters]) {
        if (w.pred(msg)) {
          this.waiters.splice(this.waiters.indexOf(w), 1);
          w.resolve(msg);
        }
      }
      if (msg.method && msg.id !== undefined) void this.onRequest(ws, msg);
    });
  }

  private async onRequest(ws: WebSocket, msg: Msg): Promise<void> {
    const w = this.requestWaiters.find((x) => x.method === msg.method);
    if (w) {
      this.requestWaiters.splice(this.requestWaiters.indexOf(w), 1);
      w.resolve(msg);
      return;
    }
    const h = this.handlers.get(msg.method!);
    if (h) {
      const out = await h(msg.params, msg);
      if (out && typeof out === "object" && "__error" in (out as object)) this.sendTo(ws, { id: msg.id, error: (out as { __error: unknown }).__error });
      else this.sendTo(ws, { id: msg.id, result: out ?? {} });
      return;
    }
    if (this.scripted > 0) this.queue.push(msg);
    else this.sendTo(ws, { id: msg.id, error: { code: -32601, message: `fake: no handler for ${msg.method}` } });
  }

  private sendTo(ws: WebSocket, msg: Msg): void {
    if (ws.readyState === ws.OPEN) ws.send(JSON.stringify(msg));
  }

  /** A version of the user config that changes whenever its content does. */
  configVersion(): string {
    return `sha256:${createHash("sha256").update(JSON.stringify(this.userConfig)).digest("hex")}`;
  }

  push(msg: Msg): void {
    if (this.current) this.sendTo(this.current, msg);
  }

  /** Resolves with the next client message matching `pred` (including ones already received after `since`). */
  waitFor(pred: (m: Msg) => boolean, timeoutMs = 10_000): Promise<Msg> {
    return new Promise((resolve, reject) => {
      const t = setTimeout(() => reject(new Error("FakeCodex.waitFor timed out")), timeoutMs);
      this.waiters.push({
        pred,
        resolve: (m) => {
          clearTimeout(t);
          resolve(m);
        },
      });
    });
  }

  private nextRequest(method: string, timeoutMs = 10_000): Promise<Msg> {
    const i = this.queue.findIndex((m) => m.method === method);
    if (i >= 0) return Promise.resolve(this.queue.splice(i, 1)[0]!);
    return new Promise((resolve, reject) => {
      const t = setTimeout(() => reject(new Error(`FakeCodex: no ${method} request arrived`)), timeoutMs);
      this.requestWaiters.push({
        method,
        resolve: (m) => {
          clearTimeout(t);
          resolve(m);
        },
      });
    });
  }

  /** Runs recorded steps in order. Returns the requests matched to `expect` steps. */
  async run(steps: Step[]): Promise<Msg[]> {
    this.scripted += 1;
    try {
      return await this.runSteps(steps);
    } finally {
      this.scripted -= 1;
    }
  }

  private async runSteps(steps: Step[]): Promise<Msg[]> {
    const matched: Msg[] = [];
    for (const s of steps) {
      if ("expect" in s) {
        const req = await this.nextRequest(s.expect);
        matched.push(req);
        if (s.error) this.push({ id: req.id, error: s.error });
        else this.push({ id: req.id, result: s.reply ?? {} });
      } else if ("expectNotify" in s) {
        const name = s.expectNotify;
        if (!this.received.some((m) => m.method === name && m.id === undefined)) await this.waitFor((m) => m.method === name && m.id === undefined);
      } else if ("expectReply" in s) {
        const id = s.expectReply;
        if (!this.replies.has(id)) await this.waitFor((m) => m.method === undefined && m.id === id);
      } else {
        this.push(s.push);
        await new Promise((r) => setImmediate(r));
      }
    }
    return matched;
  }

  requests(method: string): Msg[] {
    return this.received.filter((m) => m.method === method && m.id !== undefined);
  }

  /** Drops every client connection (the daemon "restarts" its listener). */
  dropConnections(): void {
    for (const ws of this.sockets) ws.terminate();
  }

  async close(): Promise<void> {
    for (const ws of this.sockets) ws.terminate();
    await new Promise<void>((resolve) => this.wss.close(() => resolve()));
    await new Promise<void>((resolve) => this.server.close(() => resolve()));
    fs.rmSync(this.socket, { force: true });
  }
}
