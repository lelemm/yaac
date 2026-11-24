// The app-server's terminal inventory is the liveness source. Item notifications provide
// output and the originating turn when this connection receives them.
import { randomUUID } from "node:crypto";
import type { CommandLogsResult, RunningCommand } from "../../protocol/generated/types.js";
import type { ProviderEvent } from "../types.js";
import { CodexRpcError, type CodexConnection } from "./rpc.js";

interface Terminal {
  itemId: string;
  processId: string;
  command: string;
}

interface RunningTerminal extends Terminal {
  view: RunningCommand;
}

interface Execution {
  thread: string;
  turn: string;
  output?: string;
  truncated: boolean;
  announced: boolean;
  startedHere: boolean;
  turnSettled: boolean;
}

const LIST = "thread/backgroundTerminals/list";
const STOP = "thread/backgroundTerminals/terminate";
const POLL_MS = 2_000;
const DISCOVER_MS = 8_000;
const OUTPUT_LIMIT = 8 * 1024;

export class CodexCommands {
  private conn: CodexConnection | null = null;
  private timer?: NodeJS.Timeout;
  private discoverAt = 0;
  private canStop = true;
  private dirty = new Set<string>();
  private polls = new Map<string, symbol>();
  private running = new Map<string, RunningTerminal[]>();
  private executions = new Map<string, Execution>();

  constructor(private readonly emit: (event: ProviderEvent) => void) {}

  connect(conn: CodexConnection) {
    this.disconnect();
    this.conn = conn;
    this.canStop = true;
    this.schedule(conn);
  }

  disconnect() {
    this.conn = null;
    clearTimeout(this.timer);
    this.timer = undefined;
    this.discoverAt = 0;
    this.dirty.clear();
    this.polls.clear();
    for (const thread of this.running.keys()) this.publish(thread, []);
    this.executions.clear();
  }

  /** Track output even before a process first appears in the background list. */
  item(thread: string, id: string, turn: string, started: boolean, aggregate?: string | null) {
    const entry = this.execution(thread, id);
    entry.turn ||= turn;
    entry.startedHere ||= started;
    if (aggregate && entry.output === undefined) this.append(entry, aggregate);
    this.dirty.add(thread);
  }

  turnCompleted(thread: string, turn: string) {
    for (const execution of this.executions.values()) {
      if (execution.thread === thread && execution.turn === turn) execution.turnSettled = true;
    }
    this.dirty.add(thread);
  }

  delta(thread: string, id: string, delta: string) {
    this.append(this.execution(thread, id), delta);
  }

  private append(entry: Execution, delta: string) {
    const text = (entry.output ?? "") + delta;
    entry.output = text.slice(-OUTPUT_LIMIT);
    entry.truncated ||= text.length > OUTPUT_LIMIT;
  }

  private key(thread: string, id: string) { return JSON.stringify([thread, id]); }

  private execution(thread: string, id: string): Execution {
    const key = this.key(thread, id);
    const prev = this.executions.get(key);
    if (prev) return prev;
    if (this.executions.size >= 256) {
      // Never drop a live terminal's log to make room for unrelated foreground activity.
      const live = new Set([...this.running].flatMap(([owner, commands]) => commands.map((c) => this.key(owner, c.itemId))));
      const oldest = [...this.executions.keys()].find((candidate) => !live.has(candidate));
      if (oldest) this.executions.delete(oldest);
    }
    const entry = { thread, turn: "", announced: false, truncated: false, startedHere: false, turnSettled: false };
    this.executions.set(key, entry);
    return entry;
  }

  logs(thread: string, id: string): CommandLogsResult {
    const terminal = this.running.get(thread)?.find((c) => c.view.id === id);
    const entry = terminal && this.executions.get(this.key(thread, terminal.itemId));
    return entry ? { output: entry.output, truncated: entry.truncated } : { truncated: false };
  }

  async stop(thread: string, id: string): Promise<boolean> {
    const conn = this.conn;
    const terminal = this.running.get(thread)?.find((c) => c.view.id === id);
    if (!conn || !terminal || !this.canStop) return false;
    // Re-confirm both harness ids so a stale row cannot stop a reused process id.
    const live = await this.pages<Terminal>(conn, LIST, { threadId: thread }).catch(() => []);
    if (this.conn !== conn || !this.canStop || !live.some((c) => c?.itemId === terminal.itemId && c.processId === terminal.processId)) return false;
    try {
      const reply = await conn.request(STOP, { threadId: thread, processId: terminal.processId }, 5_000) as { terminated?: unknown };
      if (this.conn === conn) {
        this.dirty.add(thread);
        if (reply.terminated === true) {
          // A list already in flight could still report the process that just stopped.
          this.polls.delete(thread);
          const commands = this.running.get(thread) ?? [];
          this.publish(thread, commands.filter((command) => command.view.id !== terminal.view.id));
        }
      }
      return reply.terminated === true;
    } catch (error) {
      if (!(error instanceof CodexRpcError) || error.code !== -32601) throw error;
      if (this.conn === conn) {
        this.canStop = false;
        for (const [owner, commands] of this.running) {
          this.publish(owner, commands.map((c) => ({ ...c, view: { ...c.view, canStop: false } })));
        }
      }
      return false;
    }
  }

  private schedule(conn: CodexConnection) {
    if (this.conn !== conn) return;
    this.timer = setTimeout(() => {
      this.timer = undefined;
      void this.poll(conn).then((supported) => { if (supported) this.schedule(conn); });
    }, POLL_MS);
    this.timer.unref();
  }

  private async pages<T>(conn: CodexConnection, method: string, params: object): Promise<T[]> {
    const result: T[] = [];
    let cursor: string | undefined;
    const seen = new Set<string>();
    do {
      const reply = await conn.request(method, { ...params, ...(cursor && { cursor }) }, 4_000) as { data?: T[]; nextCursor?: string | null };
      if (!Array.isArray(reply?.data)) throw new Error(`${method} returned no data`);
      result.push(...reply.data);
      cursor = reply.nextCursor || undefined;
      if (cursor && seen.has(cursor)) throw new Error(`${method} repeated a cursor`);
      if (cursor) seen.add(cursor);
    } while (cursor && this.conn === conn);
    return result;
  }

  private async poll(conn: CodexConnection): Promise<boolean> {
    const watch = new Set([...this.running.keys(), ...this.dirty]);
    this.dirty.clear();
    if (Date.now() >= this.discoverAt) {
      this.discoverAt = Date.now() + DISCOVER_MS;
      try {
        const loaded = await this.pages<string>(conn, "thread/loaded/list", {});
        if (this.conn !== conn) return false;
        for (const thread of loaded) if (typeof thread === "string") watch.add(thread);
      } catch {
        // An inventory failure leaves the individually tracked threads eligible for checks.
      }
    }
    const threads = [...watch];
    for (let at = 0; at < threads.length && this.conn === conn; at += 8) {
      const results = await Promise.all(threads.slice(at, at + 8).map(async (thread) => {
        const poll = Symbol();
        this.polls.set(thread, poll);
        try {
          // Only a list requested after the turn completed can confirm it outlived the turn.
          const settled = new Set([...this.executions.values()].filter((e) => e.thread === thread && e.turnSettled));
          const listed = await this.pages<Terminal>(conn, LIST, { threadId: thread });
          if (this.conn === conn && this.polls.get(thread) === poll) this.replace(thread, listed, settled);
        } catch (error) {
          if (this.conn === conn && this.polls.get(thread) === poll) this.publish(thread, []);
          if (error instanceof CodexRpcError && error.code === -32601) return false;
        } finally {
          if (this.polls.get(thread) === poll) this.polls.delete(thread);
        }
        return true;
      }));
      if (results.includes(false)) return false;
    }
    return this.conn === conn;
  }

  private replace(thread: string, listed: Terminal[], settled: Set<Execution>) {
    const previous = this.running.get(thread) ?? [];
    const commands = listed.filter((c) => typeof c?.itemId === "string" && typeof c.processId === "string" && typeof c.command === "string")
      .map(({ itemId, processId, command }) => {
        const entry = this.executions.get(this.key(thread, itemId));
        const known = previous.find((p) => p.processId === processId && p.itemId === itemId);
        const view: RunningCommand = {
          id: known?.view.id ?? randomUUID(), command, observedAt: known?.view.observedAt ?? new Date().toISOString(), canStop: this.canStop,
        };
        if (entry?.startedHere && settled.has(entry) && !entry.announced) {
          entry.announced = true;
          this.emit({ type: "itemCompleted", thread, item: {
            id: `notice:command:${view.id}`, turnId: entry.turn, status: "completed", kind: "notice", noticeKind: "info", text: `Started background command: ${command}`,
          } });
        }
        return { itemId, processId, command, view };
      });
    this.publish(thread, commands);
  }

  private publish(thread: string, commands: RunningTerminal[]) {
    const prev = this.running.get(thread);
    if (commands.length) this.running.set(thread, commands);
    else this.running.delete(thread);
    if (!prev && !commands.length) return;
    if (JSON.stringify(prev) !== JSON.stringify(commands)) {
      this.emit({ type: "runningCommands", thread, commands: commands.map((c) => c.view) });
    }
  }
}
