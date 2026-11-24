// The `yaacd mcp` stdio bridge, end to end (docs/10-agent-mesh.md#attaching-the-mcp-server):
// the real bundle is launched as Codex would launch it — by absolute path, with the thread's
// attachment ticket in its environment — and spoken to in MCP over standard I/O. Its tool
// calls reach the daemon over the daemon's own socket and act as the ticket's thread.

import { test } from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as path from "node:path";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { client, fakeOf, startDaemon, startThread, tmpHome, until } from "./helpers.js";
import { encodeMessage } from "../src/protocol/rpc.js";
import { encodeMessageFrame } from "../src/protocol/frame.js";
import { untilIdle } from "./mesh-helpers.js";

const BUNDLE = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../yaacd.mjs");
/** The bridge's own minimal bundle (no SDK, no providers), which yaacd launches when it exists. */
const SMALL = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../yaacd-mcp.mjs");

class McpProcess {
  private buf = "";
  private nextId = 1;
  private waiters = new Map<number, (m: any) => void>();
  readonly child;
  constructor(env: NodeJS.ProcessEnv, args: string[] = [BUNDLE, "mcp"]) {
    this.child = spawn(process.execPath, args, { env, stdio: ["pipe", "pipe", "pipe"] });
    this.child.stdout.setEncoding("utf8");
    this.child.stdout.on("data", (chunk: string) => {
      this.buf += chunk;
      let nl: number;
      while ((nl = this.buf.indexOf("\n")) >= 0) {
        const line = this.buf.slice(0, nl);
        this.buf = this.buf.slice(nl + 1);
        if (!line.trim()) continue;
        const m = JSON.parse(line);
        if (typeof m.id === "number" && this.waiters.has(m.id)) {
          this.waiters.get(m.id)!(m);
          this.waiters.delete(m.id);
        }
      }
    });
  }
  request(method: string, params: unknown): Promise<any> {
    const id = this.nextId++;
    return new Promise((resolve) => {
      this.waiters.set(id, resolve);
      this.child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", id, method, params })}\n`);
    });
  }
  notify(method: string, params: unknown = {}): void {
    this.child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", method, params })}\n`);
  }
  async close(): Promise<void> {
    this.child.stdin.end();
    await new Promise<void>((r) => {
      if (this.child.exitCode !== null) return r();
      this.child.once("exit", () => r());
      setTimeout(() => {
        this.child.kill("SIGKILL");
        r();
      }, 3000).unref();
    });
  }
}

for (const [label, args] of [
  ["yaacd mcp", () => [BUNDLE, "mcp"]],
  ["the small bridge bundle", () => [SMALL]],
] as Array<[string, () => string[]]>) {
test(`the stdio bridge (${label}) serves the yaac tools as the ticket's thread, and checks the caller's thread id`, async (t) => {
  if (!fs.existsSync(BUNDLE) || !fs.existsSync(SMALL)) return t.skip("dist/yaacd.mjs is not built");
  const home = tmpHome("ymb-");
  const d = await startDaemon(home, { mesh: { tickMs: 0 } });
  const c = await client(d);
  let mcp: McpProcess | null = null;
  try {
    const { threadId } = await startThread(c, "/fake say hi", { agents: "allow" });
    await untilIdle(d, threadId);
    const native = d.core.thread(threadId)!.record.native;
    const att = fakeOf(d).meshAttachment(native)!;
    const stdio = att.stdio();
    assert.ok(path.isAbsolute(stdio.args.find((a) => a.endsWith(".js") || a.endsWith(".mjs")) ?? stdio.command), "launched by absolute path");
    assert.equal(stdio.env.YAAC_MCP_TICKET, att.ticket);
    mcp = new McpProcess({ PATH: process.env.PATH, YAAC_MCP_TICKET: att.ticket, YAAC_SOCKET: d.socketPath, YAAC_HOME: home }, args());
    const init = await mcp.request("initialize", { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "test", version: "0" } });
    assert.equal(init.result.serverInfo.name, "yaac");
    assert.match(init.result.instructions, /running inside YAAC/);
    mcp.notify("notifications/initialized");
    const ping = await mcp.request("ping", {});
    assert.deepEqual(ping.result, {});
    const unknown = await mcp.request("resources/list", {});
    assert.equal(unknown.error.code, -32601);
    const list = await mcp.request("tools/list", {});
    const names = list.result.tools.map((x: any) => x.name).sort();
    assert.deepEqual(names, ["browser_click", "browser_close", "browser_console", "browser_eval", "browser_list", "browser_navigate", "browser_network", "browser_open", "browser_press", "browser_screenshot", "browser_snapshot", "browser_type", "find_thread", "list_models", "list_targets", "list_threads", "notify_when_done", "pr_checks", "pr_comments", "pr_create", "pr_get", "pr_list", "pr_reply", "pr_rerun_checks", "read_thread", "send_message", "spawn_thread", "stop_all", "stop_thread", "wait", "whoami"]);
    const who = await mcp.request("tools/call", { name: "whoami", arguments: {}, _meta: { threadId: native } });
    assert.equal(who.result.isError, undefined);
    assert.match(who.result.content[0].text, new RegExp(`Address: local/${threadId}`));
    const forged = await mcp.request("tools/call", { name: "whoami", arguments: {}, _meta: { threadId: "someone-else" } });
    assert.equal(forged.result.isError, true);
    assert.match(forged.result.content[0].text, /does not match/);
    const browser = (name: string, args = {}) => mcp!.request("tools/call", { name, arguments: args });
    const offline = await browser("browser_list");
    assert.equal(offline.result.isError, true);
    assert.match(offline.result.content[0].text, /No YAAC window is open/);
    await c.request("mesh/roster", { selfId: "target", selfName: "test", targets: [] });
    const spoof = await browser("browser_list", { threadId: "other" });
    assert.equal(spoof.result.isError, true);
    const png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jQp8AAAAASUVORK5CYII=";
    for (const [kind, fields, result] of [
      ["list", {}, { tabs: [] }],
      ["snapshot", { tab: "a" }, { tree: "document" }],
      ["screenshot", { tab: "a", selector: "main" }, { data: png }],
      ["console", { tab: "a", cursor: 2 }, { entries: [], cursor: 2 }],
      ["network", { tab: "a" }, { entries: [], cursor: 0 }],
      ["open", { url: "5173" }, { tab: "t1", url: "http://localhost:5173/", title: "App", loading: false }],
      ["close", { tab: "a" }, {}],
      ["navigate", { tab: "a", history: "back" }, { tab: "a", url: "http://localhost:5173/", title: "", loading: true }],
      ["navigate", { tab: "a", url: "http://example.com/" }, { tab: "a", url: "http://example.com/", title: "", loading: false }],
      ["click", { tab: "a", ref: "12" }, {}],
      ["click", { tab: "a", x: 10, y: 20 }, {}],
      ["type", { tab: "a", text: "hi", selector: "#q", clear: true }, {}],
      ["press", { tab: "a", key: "Enter" }, {}],
      ["eval", { tab: "a", expression: "1+1" }, { value: 2 }],
    ] as const) {
      const start = c.messages.length;
      const pending = browser("browser_" + kind, fields);
      const req = await until(() => c.messages.slice(start).find(m => m.type === "request" && m.method === "browser/call"));
      assert.ok(req.type === "request");
      assert.deepEqual(req.params, { threadId, action: { kind, ...fields } });
      c.sendRaw(encodeMessageFrame(encodeMessage({ type: "response", id: req.id, result: { kind, ...result } })));
      const response = (await pending).result;
      if (kind === "screenshot") assert.deepEqual(response.content, [{ type: "image", mimeType: "image/png", data: png }]);
      else if (kind === "snapshot") assert.equal(response.content[0].text, result.tree);
      else if (kind === "eval") assert.equal(response.content[0].text, "2");
      else if (kind === "close" || kind === "click" || kind === "type" || kind === "press") assert.equal(response.content[0].text, "Done.");
      else assert.deepEqual(JSON.parse(response.content[0].text), { kind, ...result });
    }
    // Malformed actions never reach the window, and a tool cannot name another thread.
    for (const [name, args] of [
      ["browser_open", {}], ["browser_navigate", { tab: "a" }], ["browser_navigate", { tab: "a", url: "x", history: "back" }],
      ["browser_navigate", { tab: "a", history: "sideways" }], ["browser_click", { tab: "a" }], ["browser_click", { tab: "a", x: 1 }],
      ["browser_click", { tab: "a", ref: "1", selector: "b" }], ["browser_type", { tab: "a" }], ["browser_press", { tab: "a", key: "" }],
      ["browser_eval", { tab: "a", expression: "1", threadId: "other" }],
    ] as const) {
      const start = c.messages.length;
      const r = await browser(name, args);
      assert.equal(r.result.isError, true, name + JSON.stringify(args));
      assert.ok(!c.messages.slice(start).some(m => m.type === "request" && m.method === "browser/call"));
    }
    // A wait cut off by the harness consumes nothing.
    const w = mcp.request("tools/call", { name: "wait", arguments: { timeoutSecs: 1 }, _meta: { threadId: native } });
    const r = await w;
    assert.match(r.result.content[0].text, /Nothing yet|no running turn|The turn ended|Nothing/);
  } finally {
    await mcp?.close();
    await c.close();
    await d.stop("test");
    await until(() => true);
    fs.rmSync(home, { recursive: true, force: true });
  }
});
}
