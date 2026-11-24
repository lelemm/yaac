import { Socket, isIP } from "node:net";
import type { ConnectionView } from "../protocol/connection.js";
import { NOTIFICATIONS } from "../protocol/generated/constants.js";
import type { TunnelOpenResult } from "../protocol/generated/types.js";
import { ReceiveWindow } from "../protocol/scheduler.js";
import { RpcError } from "../protocol/rpc.js";

interface Tunnel {
  socket: Socket;
  receive: ReceiveWindow;
  connected: boolean;
  sending: boolean;
  eof: boolean;
}

/**
 * Any host name or address, resolved here on the target (P323): a tunnel reaches what the
 * target user's own shell reaches. Brackets around an IPv6 literal are dropped. Null if the
 * value cannot name a TCP host (empty, too long, a path, whitespace or control characters).
 */
export function destination(value: unknown): string | null {
  if (value === undefined) return "127.0.0.1";
  if (typeof value !== "string") return null;
  const host = /^\[[^\]]*\]$/.test(value) ? value.slice(1, -1) : value;
  if (!host || host.length > 253 || /[\s\x00-\x1f\x7f/\\@\[\]]/.test(host)) return null;
  if (host.includes(":") && isIP(host) !== 6) return null;
  return host;
}

/** One connection's sockets, including those waiting for connect(). */
export class Tunnels {
  private readonly tunnels = new Map<number, Tunnel>();

  constructor(private readonly view: ConnectionView, private readonly allocate: () => number) {}

  async open(params: Record<string, unknown>): Promise<TunnelOpenResult> {
    const { port, initialCredit } = params;
    const host = destination(params.host);
    if (typeof port !== "number" || !Number.isInteger(port) || port < 1 || port > 65535 ||
        typeof initialCredit !== "number" || !Number.isSafeInteger(initialCredit) || initialCredit < 0 || host === null) {
      throw new RpcError("invalid_params", "expected a host name or address, TCP port and nonnegative initialCredit");
    }
    if (this.tunnels.size >= 256) throw new RpcError("limit_exceeded", "at most 256 open tunnels per connection");
    const stream = this.allocate();
    const socket = new Socket();
    const receive = new ReceiveWindow(stream, 64 * 1024);
    const t: Tunnel = { socket, receive, sending: false, connected: false, eof: false };
    this.tunnels.set(stream, t);
    await new Promise<void>((resolve, reject) => {
      socket.on("error", (e: NodeJS.ErrnoException) => {
        const code = e.code === "ECONNREFUSED" ? "connection_refused" : "connection_failed";
        reject(new RpcError(code, e.code === "ENOTFOUND" || e.code === "EAI_AGAIN" ? `cannot resolve ${host} on the target` : e.message));
        this.finish(stream, e.message);
      });
      socket.on("close", () => {
        reject(new RpcError("connection_closed", "tunnel closed during connect"));
        t.eof = true;
        if (!t.sending) this.finish(stream);
      });
      // Pin localhost lookup to loopback, including IPv6-only servers.
      socket.connect({ host, port, ...(host === "localhost" ? {
        autoSelectFamily: true,
        lookup: (_name, options, callback) => {
          const addresses = [{ address: "127.0.0.1", family: 4 }, { address: "::1", family: 6 }];
          if (options.all) callback(null, addresses);
          else callback(null, "127.0.0.1", 4);
        },
      } : {}) }, resolve);
    });
    this.view.openStream(stream, initialCredit);
    t.connected = true;
    // Let the control response announce the stream before a server banner can be sent.
    setImmediate(() => {
      if (!this.tunnels.has(stream)) return;
      socket.on("data", (bytes: Buffer) => {
        socket.pause();
        t.sending = true;
        this.view.sendData(stream, bytes, () => {
          if (!this.tunnels.has(stream)) return;
          t.sending = false;
          if (t.eof) this.finish(stream);
          else socket.resume();
        });
      });
    });
    return { stream, initialCredit: receive.initialCredit() };
  }

  receive(stream: number, bytes: Buffer): boolean {
    const t = this.tunnels.get(stream);
    if (!t?.connected) return false;
    try {
      t.receive.onReceived(bytes.length);
    } catch (error) {
      this.finish(stream, (error as Error).message);
      return true;
    }
    t.socket.write(bytes, (error) => {
      if (error || !this.tunnels.has(stream)) return;
      const grant = t.receive.onConsumed(bytes.length);
      if (grant !== null) this.view.grantCredit(stream, grant);
    });
    return true;
  }

  private finish(stream: number, reason?: string): void {
    const t = this.tunnels.get(stream);
    if (!t) return;
    this.close(stream);
    if (t.connected) this.view.notify(NOTIFICATIONS.TUNNEL_CLOSE, { stream, reason });
  }

  close(stream: number): void {
    const t = this.tunnels.get(stream);
    if (!t) return;
    this.tunnels.delete(stream);
    this.view.cancelStream(stream);
    t.socket.destroy();
  }

  drop(): void {
    for (const stream of this.tunnels.keys()) this.close(stream);
  }
}
