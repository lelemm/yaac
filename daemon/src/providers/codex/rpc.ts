// A JSON-RPC client for the Codex app-server, over a WebSocket on the daemon's Unix control
// socket (an HTTP Upgrade on the socket, answered with 101). One instance is one connection;
// reconnecting is the provider's job.

import * as net from "node:net";
import WebSocket from "ws";
import { Emitter } from "../../util/emitter.js";
import type { InitializeResponse, RpcId, RpcMessage } from "./wire.js";

export class CodexRpcError extends Error {
  constructor(
    readonly method: string,
    readonly code: number | undefined,
    message: string,
    readonly data?: unknown,
  ) {
    super(message);
  }
}

/** Why a connection could not be opened. */
export type ConnectFailure = "absent" | "accessDenied" | "other";

export class ConnectError extends Error {
  constructor(
    readonly reason: ConnectFailure,
    message: string,
  ) {
    super(message);
  }
}

export interface ServerRequest {
  id: RpcId;
  method: string;
  params: unknown;
}

export interface ClientInfo {
  name: string;
  title: string | null;
  version: string;
}

export class CodexConnection {
  readonly notifications = new Emitter<{ method: string; params: unknown }>();
  readonly requests = new Emitter<ServerRequest>();
  readonly closed = new Emitter<Error | null>();
  private nextId = 1;
  private pending = new Map<RpcId, { method: string; resolve: (v: unknown) => void; reject: (e: Error) => void }>();
  private isClosed = false;

  private constructor(private readonly ws: WebSocket) {
    ws.on("message", (data) => this.onMessage(data.toString()));
    ws.on("close", () => this.onClose(null));
    ws.on("error", (e) => this.onClose(e));
  }

  /** Opens a WebSocket on a Unix socket. */
  static open(socketPath: string, timeoutMs = 5000): Promise<CodexConnection> {
    return new Promise((resolve, reject) => {
      let settled = false;
      const ws = new WebSocket("ws://localhost/", {
        createConnection: () => net.connect(socketPath),
        handshakeTimeout: timeoutMs,
        maxPayload: 256 * 1024 * 1024,
      } as WebSocket.ClientOptions);
      const fail = (e: NodeJS.ErrnoException) => {
        if (settled) return;
        settled = true;
        ws.removeAllListeners();
        ws.on("error", () => {});
        try {
          ws.terminate();
        } catch {
          /* already gone */
        }
        const code = e.code ?? "";
        const reason: ConnectFailure =
          code === "ENOENT" || code === "ECONNREFUSED" || code === "ENOTSOCK" ? "absent" : code === "EACCES" || code === "EPERM" ? "accessDenied" : "other";
        reject(new ConnectError(reason, `${socketPath}: ${e.message}`));
      };
      ws.once("error", fail);
      ws.once("unexpected-response", (_req, res) => fail(Object.assign(new Error(`the socket answered HTTP ${res.statusCode}, not a WebSocket upgrade`), { code: "EPROTO" })));
      ws.once("open", () => {
        if (settled) return;
        settled = true;
        ws.removeAllListeners();
        resolve(new CodexConnection(ws));
      });
    });
  }

  get isOpen(): boolean {
    return !this.isClosed;
  }

  /** `initialize`, then the `initialized` notification. */
  async initialize(client: ClientInfo, capabilities: Record<string, unknown>): Promise<InitializeResponse> {
    const reply = (await this.request("initialize", { clientInfo: client, capabilities })) as InitializeResponse;
    this.notify("initialized");
    return reply ?? {};
  }

  request(method: string, params?: unknown, timeoutMs = 120_000): Promise<unknown> {
    if (this.isClosed) return Promise.reject(new CodexRpcError(method, undefined, "the Codex connection is closed"));
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        if (this.pending.delete(id)) reject(new CodexRpcError(method, undefined, `${method}: no reply from Codex within ${timeoutMs} ms`));
      }, timeoutMs);
      timer.unref?.();
      this.pending.set(id, {
        method,
        resolve: (v) => {
          clearTimeout(timer);
          resolve(v);
        },
        reject: (e) => {
          clearTimeout(timer);
          reject(e);
        },
      });
      this.send({ id, method, ...(params === undefined ? {} : { params }) });
    });
  }

  notify(method: string, params?: unknown): void {
    this.send({ method, ...(params === undefined ? {} : { params }) });
  }

  /** Answers a server request. */
  reply(id: RpcId, result: unknown): void {
    this.send({ id, result });
  }

  replyError(id: RpcId, code: number, message: string): void {
    this.send({ id, error: { code, message } });
  }

  close(): void {
    if (this.isClosed) return;
    try {
      this.ws.close();
    } catch {
      /* ignore */
    }
    this.onClose(null);
  }

  private send(msg: RpcMessage): void {
    if (this.isClosed) return;
    this.ws.send(JSON.stringify(msg));
  }

  private onMessage(text: string): void {
    let msg: RpcMessage;
    try {
      msg = JSON.parse(text) as RpcMessage;
    } catch {
      return; // not JSON: ignore rather than fail
    }
    if (!msg || typeof msg !== "object") return;
    if (typeof msg.method === "string") {
      if (msg.id !== undefined && msg.id !== null) this.requests.emit({ id: msg.id, method: msg.method, params: msg.params });
      else this.notifications.emit({ method: msg.method, params: msg.params });
      return;
    }
    if (msg.id === undefined || msg.id === null) return;
    const p = this.pending.get(msg.id);
    if (!p) return;
    this.pending.delete(msg.id);
    if (msg.error) p.reject(new CodexRpcError(p.method, msg.error.code, msg.error.message ?? `${p.method} failed`, msg.error.data));
    else p.resolve(msg.result);
  }

  private onClose(err: Error | null): void {
    if (this.isClosed) return;
    this.isClosed = true;
    for (const [, p] of this.pending) p.reject(new CodexRpcError(p.method, undefined, "the Codex connection closed"));
    this.pending.clear();
    try {
      this.ws.terminate();
    } catch {
      /* ignore */
    }
    this.closed.emit(err);
  }
}
