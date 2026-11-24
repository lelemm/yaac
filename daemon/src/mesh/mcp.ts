// The `yaac` MCP server, and the link between the `yaacd mcp` bridge and the daemon
// (docs/10-agent-mesh.md#attaching-the-mcp-server).
//
// One server definition serves every harness:
// - the Claude Agent SDK hosts it in process (`type: "sdk"`), one instance per thread;
// - Codex launches `yaacd mcp` per thread, a stdio MCP server whose tool calls are forwarded
//   to the daemon over its own Unix socket, carrying the thread's attachment ticket.
//
// The bridge link on the socket: the bridge writes the 4-byte magic `YMCP` (never the start of
// a valid YAAC frame: as a length it exceeds the frame limit), then newline-delimited JSON:
//
//   bridge → daemon   {"t":"hello","ticket":"…"}
//                     {"t":"call","id":1,"tool":"send_message","args":{…},"meta":{…}}
//                     {"t":"cancel","id":1}
//   daemon → bridge   {"t":"result","id":1,"text":"…","isError":false}

import type { Socket } from "node:net";
import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { CallToolRequestSchema, ListToolsRequestSchema } from "@modelcontextprotocol/sdk/types.js";
import type { MeshToolCall, MeshToolResult } from "../providers/types.js";
import { SERVER_INSTRUCTIONS, SERVER_NAME, TOOLS, TOOL_NAMES } from "./tools.js";

import { BRIDGE_MAGIC, readable } from "./link.js";

export { BRIDGE_MAGIC, readable };

export type ToolCaller = (call: MeshToolCall) => Promise<MeshToolResult>;

/** A fresh `yaac` MCP server whose tool calls go to `call`. */
export function createMeshServer(call: ToolCaller, version = "1"): McpServer {
  const mcp = new McpServer({ name: SERVER_NAME, version }, { instructions: SERVER_INSTRUCTIONS });
  const server = mcp.server;
  server.registerCapabilities({ tools: {} });
  server.setRequestHandler(ListToolsRequestSchema, async () => ({ tools: TOOLS.map((t) => ({ name: t.name, description: t.description, inputSchema: t.inputSchema })) }));
  server.setRequestHandler(CallToolRequestSchema, async (req, extra) => {
    const name = req.params.name;
    if (!TOOL_NAMES.has(name)) return { content: [{ type: "text" as const, text: `There is no yaac tool named ${name}.` }], isError: true };
    const args = (req.params.arguments ?? {}) as Record<string, unknown>;
    const meta = (req.params._meta ?? null) as Record<string, unknown> | null;
    let result: MeshToolResult;
    try {
      result = await call({ tool: name, args, meta, signal: extra.signal });
    } catch (e) {
      result = { text: readable(e), isError: true };
    }
    return { content: result.image ? [result.image] : [{ type: "text" as const, text: result.text }], ...(result.isError ? { isError: true } : {}) };
  });
  return mcp;
}

// ── Daemon side of the bridge link ─────────────────────────────────────────────────────────

/** Reads a socket's first four bytes; `true` means it is a bridge (the bytes are consumed). */
export function sniffBridge(socket: Socket): Promise<boolean> {
  return new Promise((resolve) => {
    let got = Buffer.alloc(0);
    const onReadable = () => {
      let chunk: Buffer | null;
      while ((chunk = socket.read() as Buffer | null) !== null) {
        got = Buffer.concat([got, chunk]);
        if (got.length >= BRIDGE_MAGIC.length) break;
      }
      if (got.length < BRIDGE_MAGIC.length && !socket.readableEnded) return;
      socket.removeListener("readable", onReadable);
      socket.removeListener("end", onEnd);
      const isBridge = got.length >= BRIDGE_MAGIC.length && got.subarray(0, BRIDGE_MAGIC.length).equals(BRIDGE_MAGIC);
      const rest = isBridge ? got.subarray(BRIDGE_MAGIC.length) : got;
      if (rest.length > 0) socket.unshift(rest);
      resolve(isBridge);
    };
    const onEnd = () => onReadable();
    socket.on("readable", onReadable);
    socket.on("end", onEnd);
  });
}

/** Serves one bridge connection: `handle` runs each call as the ticket's thread. */
export function serveBridge(socket: Socket, handle: (ticket: string, call: MeshToolCall) => Promise<MeshToolResult>, log: (l: string) => void): void {
  let ticket: string | null = null;
  let buf = "";
  const inflight = new Map<number, AbortController>();
  const write = (o: unknown) => {
    if (!socket.destroyed) socket.write(`${JSON.stringify(o)}\n`);
  };
  socket.setEncoding("utf8");
  socket.on("data", (chunk: string) => {
    buf += chunk;
    if (buf.length > 4 * 1024 * 1024) {
      log("mcp bridge: oversized line; closing");
      socket.destroy();
      return;
    }
    let nl: number;
    while ((nl = buf.indexOf("\n")) >= 0) {
      const line = buf.slice(0, nl);
      buf = buf.slice(nl + 1);
      if (!line.trim()) continue;
      let msg: { t?: string; id?: number; ticket?: string; tool?: string; args?: Record<string, unknown>; meta?: Record<string, unknown> | null };
      try {
        msg = JSON.parse(line);
      } catch {
        socket.destroy();
        return;
      }
      if (msg.t === "hello" && typeof msg.ticket === "string") {
        ticket = msg.ticket;
        continue;
      }
      if (msg.t === "cancel" && typeof msg.id === "number") {
        inflight.get(msg.id)?.abort();
        continue;
      }
      if (msg.t === "call" && typeof msg.id === "number" && typeof msg.tool === "string") {
        const id = msg.id;
        if (!ticket) {
          write({ t: "result", id, text: "This yaac tool connection has no attachment ticket.", isError: true });
          continue;
        }
        const abort = new AbortController();
        inflight.set(id, abort);
        void handle(ticket, { tool: msg.tool, args: msg.args ?? {}, meta: msg.meta ?? null, signal: abort.signal })
          .catch((e): MeshToolResult => ({ text: readable(e), isError: true }))
          .then((r) => {
            inflight.delete(id);
            write({ t: "result", id, ...r });
          });
      }
    }
  });
  socket.on("close", () => {
    for (const a of inflight.values()) a.abort();
    inflight.clear();
  });
  socket.on("error", () => {});
}
