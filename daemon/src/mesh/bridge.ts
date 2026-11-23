// `yaacd mcp`: the stdio MCP server Codex launches for one thread
// (docs/10-agent-mesh.md#attaching-the-mcp-server).
//
// It speaks MCP on standard I/O and forwards every tool call to the serving `yaacd` over its
// user-only Unix socket, with the thread's attachment ticket from `YAAC_MCP_TICKET`. It holds
// no state of its own: the MCP session survives a daemon restart, and a call made while the
// daemon is away reconnects (starting the daemon, as `yaacd connect` does, if none serves).
//
// Never logs to standard output (that is the MCP channel).

import type * as net from "node:net";
import { spawn } from "node:child_process";
import type { MeshToolCall, MeshToolResult } from "../providers/types.js";
import { connectDaemonSocket } from "../session/server.js";
import { BRIDGE_MAGIC } from "./link.js";
import { serveStdioMcp } from "./stdio-mcp.js";
import { SERVER_INSTRUCTIONS, SERVER_NAME, TOOLS } from "./tools.js";

interface Pending {
  resolve: (r: MeshToolResult) => void;
}

class DaemonLink {
  private socket: net.Socket | null = null;
  private connecting: Promise<net.Socket> | null = null;
  private nextId = 1;
  private pending = new Map<number, Pending>();
  private buf = "";

  constructor(
    private readonly socketPath: string,
    private readonly ticket: string,
    private readonly startDaemon: (() => void) | null,
  ) {}

  private async open(): Promise<net.Socket> {
    let s = await connectDaemonSocket(this.socketPath);
    if (!s && this.startDaemon) {
      this.startDaemon();
      const deadline = Date.now() + 15000;
      while (!s && Date.now() < deadline) {
        await new Promise((r) => setTimeout(r, 100));
        s = await connectDaemonSocket(this.socketPath);
      }
    }
    if (!s) throw new Error("yaacd is not running on this machine, so the yaac tools are unavailable right now");
    s.setEncoding("utf8");
    s.write(BRIDGE_MAGIC);
    s.write(`${JSON.stringify({ t: "hello", ticket: this.ticket })}\n`);
    s.on("data", (chunk: string) => this.onData(chunk));
    s.on("close", () => {
      if (this.socket === s) this.socket = null;
      for (const p of this.pending.values()) p.resolve({ text: "The connection to yaacd closed during this call; its outcome is unknown. Check before repeating it.", isError: true });
      this.pending.clear();
    });
    s.on("error", () => {});
    this.socket = s;
    return s;
  }

  private ensure(): Promise<net.Socket> {
    if (this.socket && !this.socket.destroyed) return Promise.resolve(this.socket);
    if (!this.connecting) this.connecting = this.open().finally(() => (this.connecting = null));
    return this.connecting;
  }

  private onData(chunk: string): void {
    this.buf += chunk;
    let nl: number;
    while ((nl = this.buf.indexOf("\n")) >= 0) {
      const line = this.buf.slice(0, nl);
      this.buf = this.buf.slice(nl + 1);
      try {
        const m = JSON.parse(line) as { t?: string; id?: number; text?: string; isError?: boolean };
        if (m.t === "result" && typeof m.id === "number") {
          const p = this.pending.get(m.id);
          this.pending.delete(m.id);
          p?.resolve({ text: String(m.text ?? ""), isError: !!m.isError });
        }
      } catch {
        /* ignore a garbled line */
      }
    }
  }

  async call(call: MeshToolCall): Promise<MeshToolResult> {
    const s = await this.ensure();
    const id = this.nextId++;
    return new Promise<MeshToolResult>((resolve) => {
      this.pending.set(id, { resolve });
      const onAbort = () => {
        if (this.pending.has(id) && !s.destroyed) s.write(`${JSON.stringify({ t: "cancel", id })}\n`);
      };
      call.signal?.addEventListener("abort", onAbort, { once: true });
      s.write(`${JSON.stringify({ t: "call", id, tool: call.tool, args: call.args, meta: call.meta ?? null })}\n`);
    });
  }
}

export async function runBridge(env: NodeJS.ProcessEnv, selfArgs: string[]): Promise<void> {
  const ticket = env.YAAC_MCP_TICKET;
  const socketPath = env.YAAC_SOCKET;
  if (!ticket || !socketPath) {
    process.stderr.write("yaacd mcp: YAAC_MCP_TICKET and YAAC_SOCKET must be set (yaacd sets them when it attaches this server)\n");
    process.exit(2);
  }
  // The daemon's own entry: this process may be the small bridge bundle, which cannot serve.
  const daemonArgs = env.YAAC_DAEMON_ENTRY ? [env.YAAC_DAEMON_ENTRY] : selfArgs;
  const startDaemon = () => {
    const child = spawn(process.execPath, [...process.execArgv, ...daemonArgs, "serve"], { detached: true, windowsHide: true, stdio: "ignore", env });
    child.unref();
  };
  const link = new DaemonLink(socketPath, ticket, startDaemon);
  serveStdioMcp({
    input: process.stdin,
    output: process.stdout,
    name: SERVER_NAME,
    version: "1",
    instructions: SERVER_INSTRUCTIONS,
    tools: TOOLS,
    call: (call) => link.call(call),
    onClose: () => process.exit(0),
  });
}
