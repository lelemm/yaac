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
    assert.deepEqual(names, ["find_thread", "list_models", "list_targets", "list_threads", "notify_when_done", "pr_checks", "pr_comments", "pr_create", "pr_get", "pr_list", "pr_reply", "pr_rerun_checks", "read_thread", "send_message", "spawn_thread", "stop_all", "stop_thread", "wait", "whoami"]);
    const who = await mcp.request("tools/call", { name: "whoami", arguments: {}, _meta: { threadId: native } });
    assert.equal(who.result.isError, undefined);
    assert.match(who.result.content[0].text, new RegExp(`Address: local/${threadId}`));
    const forged = await mcp.request("tools/call", { name: "whoami", arguments: {}, _meta: { threadId: "someone-else" } });
    assert.equal(forged.result.isError, true);
    assert.match(forged.result.content[0].text, /does not match/);
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
