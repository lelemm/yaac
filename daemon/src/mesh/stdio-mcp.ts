// A minimal MCP server on standard I/O, written by hand for the `yaacd mcp` bridge: the bridge
// runs once per live Codex thread, so it carries no SDK (docs/10-agent-mesh.md#attaching-the-mcp-server).
//
// The surface the `yaac` server needs is small: `initialize`, `ping`, `tools/list`,
// `tools/call`, and the `notifications/initialized` and `notifications/cancelled`
// notifications. Messages are newline-delimited JSON-RPC 2.0. Never writes anything but
// protocol messages to the output.

import type { Readable, Writable } from "node:stream";
import type { MeshToolCall, MeshToolResult } from "../providers/types.js";
import type { ToolDef } from "./tools.js";
import { readable } from "./link.js";

/** The MCP protocol revisions this server speaks; a client asking for another gets the newest. */
export const MCP_VERSIONS = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05", "2024-10-07"];

export interface StdioMcpOptions {
  input: Readable;
  output: Writable;
  name: string;
  version: string;
  instructions: string;
  tools: ToolDef[];
  call: (call: MeshToolCall) => Promise<MeshToolResult>;
  /** The input ended (the harness closed the server). */
  onClose?: () => void;
}

type Id = number | string;
interface Rpc {
  jsonrpc?: string;
  id?: Id | null;
  method?: string;
  params?: Record<string, unknown>;
}

export function serveStdioMcp(o: StdioMcpOptions): void {
  const names = new Set(o.tools.map((t) => t.name));
  const inflight = new Map<Id, AbortController>();
  let buf = "";
  const send = (m: unknown) => {
    o.output.write(`${JSON.stringify(m)}\n`);
  };
  const reply = (id: Id, result: unknown) => send({ jsonrpc: "2.0", id, result });
  const fail = (id: Id | null, code: number, message: string) => send({ jsonrpc: "2.0", id, error: { code, message } });

  const handle = (m: Rpc) => {
    if (!m || typeof m !== "object" || typeof m.method !== "string") {
      if (m && (m as { id?: unknown }).id !== undefined && ("result" in m || "error" in m)) return; // a response to nothing we asked
      fail(null, -32600, "invalid request");
      return;
    }
    const isRequest = m.id !== undefined && m.id !== null;
    const p = (m.params ?? {}) as Record<string, unknown>;
    switch (m.method) {
      case "initialize": {
        if (!isRequest) return;
        const asked = typeof p.protocolVersion === "string" ? p.protocolVersion : "";
        reply(m.id!, {
          protocolVersion: MCP_VERSIONS.includes(asked) ? asked : MCP_VERSIONS[0],
          capabilities: { tools: { listChanged: false } },
          serverInfo: { name: o.name, version: o.version },
          instructions: o.instructions,
        });
        return;
      }
      case "ping":
        if (isRequest) reply(m.id!, {});
        return;
      case "tools/list":
        if (isRequest) reply(m.id!, { tools: o.tools.map((t) => ({ name: t.name, description: t.description, inputSchema: t.inputSchema })) });
        return;
      case "tools/call": {
        if (!isRequest) return;
        const id = m.id!;
        const name = typeof p.name === "string" ? p.name : "";
        if (!names.has(name)) {
          reply(id, { content: [{ type: "text", text: `There is no yaac tool named ${name}.` }], isError: true });
          return;
        }
        const args = p.arguments && typeof p.arguments === "object" ? (p.arguments as Record<string, unknown>) : {};
        const meta = p._meta && typeof p._meta === "object" ? (p._meta as Record<string, unknown>) : null;
        const abort = new AbortController();
        inflight.set(id, abort);
        void o
          .call({ tool: name, args, meta, signal: abort.signal })
          .catch((e): MeshToolResult => ({ text: readable(e), isError: true }))
          .then((r) => {
            inflight.delete(id);
            // A cancelled request gets no response (MCP: the receiver should not answer it).
            if (abort.signal.aborted) return;
            reply(id, { content: r.image ? [r.image] : [{ type: "text", text: r.text }], ...(r.isError ? { isError: true } : {}) });
          });
        return;
      }
      case "notifications/cancelled": {
        const rid = p.requestId as Id | undefined;
        if (rid !== undefined) inflight.get(rid)?.abort();
        return;
      }
      default:
        if (isRequest) fail(m.id!, -32601, `method not found: ${m.method}`);
    }
  };

  o.input.setEncoding("utf8");
  o.input.on("data", (chunk: string) => {
    buf += chunk;
    let nl: number;
    while ((nl = buf.indexOf("\n")) >= 0) {
      const line = buf.slice(0, nl).replace(/\r$/, "");
      buf = buf.slice(nl + 1);
      if (!line.trim()) continue;
      let parsed: unknown;
      try {
        parsed = JSON.parse(line);
      } catch {
        fail(null, -32700, "parse error");
        continue;
      }
      for (const m of Array.isArray(parsed) ? parsed : [parsed]) handle(m as Rpc);
    }
  });
  o.input.on("end", () => {
    for (const a of inflight.values()) a.abort();
    o.onClose?.();
  });
}
