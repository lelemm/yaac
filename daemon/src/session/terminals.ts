import { randomUUID } from "node:crypto";
import { createRequire } from "node:module";
import { homedir, release } from "node:os";
import { dirname, join, resolve } from "node:path";
import { existsSync, statSync } from "node:fs";
import type { IPty } from "@lydell/node-pty";
import type { ConnectionView } from "../protocol/connection.js";
import type { TerminalInfo } from "../protocol/generated/types.js";
import { ReceiveWindow } from "../protocol/scheduler.js";
import { RpcError } from "../protocol/rpc.js";
import { shellArgs, shells } from "./terminal-shells.js";

const require = createRequire(import.meta.url);
const VERSION = "1.2.0-beta.18";
function loadPty(): typeof import("@lydell/node-pty") {
  const platform = process.platform + "-" + process.arch;
  const windows = process.platform === "win32";
  if (windows && Number(release().split(".")[2]) < 17763) throw new Error("ConPTY requires Windows 10 build 17763 or later");
  const runtime = windows ? dirname(process.execPath) : join(process.env.YAAC_HOME || join(homedir(), ".yaac"), "runtime");
  const staged = join(runtime, "pty-" + VERSION, "lib", "index.js");
  let entry = staged;
  if (!existsSync(entry)) {
    // Source checkout: npm installed the matching optional dependency.
    try { entry = require.resolve("@lydell/node-pty-" + platform); }
    catch { throw new Error(`node-pty for ${platform} is missing at ${staged}`); }
  }
  // Load the actual binary first so ABI/loader failures reach initialize intact.
  // The upstream loader tries multiple layouts and reports only its last attempt.
  require(join(dirname(entry), "..", "prebuilds", platform, windows ? "conpty.node" : "pty.node"));
  return require(entry);
}

let native: ReturnType<typeof loadPty> | undefined;
export let terminalUnavailableReason: string | undefined;
try { native = loadPty(); } catch (error) { terminalUnavailableReason = String(error); }

const CREDIT = 1024 * 1024;
const HISTORY = 2 * CREDIT;
const BATCH = 16 * 1024;

/** Flush at the next 1 ms timer or after 16 KiB, whichever comes first. */
export class TerminalOutput {
  private parts: Buffer[] = [];
  private size = 0;
  private timer?: NodeJS.Timeout;
  constructor(private publish: (bytes: Buffer) => void) {}
  append(data: Buffer) {
    for (let offset = 0; offset < data.length;) {
      const n = Math.min(BATCH - this.size, data.length - offset);
      this.parts.push(data.subarray(offset, offset + n));
      this.size += n;
      offset += n;
      if (this.size === BATCH) this.flush();
    }
    if (this.size && !this.timer) this.timer = setTimeout(() => this.flush(), 1);
  }
  flush() {
    clearTimeout(this.timer);
    this.timer = undefined;
    if (this.size) this.publish(this.parts.length === 1 ? this.parts[0]! : Buffer.concat(this.parts, this.size));
    this.parts = [];
    this.size = 0;
  }
}

interface Attachment {
  view: ConnectionView;
  stream: number;
  window: ReceiveWindow;
  queue: Buffer[];
  queued: number;
  sending: boolean;
  ready: boolean;
}

interface Running {
  info: TerminalInfo;
  pty: IPty;
  attachments: Set<Attachment>;
  history: Buffer;
  written: number;
  batch: TerminalOutput;
  oscTail: string;
  exit?: number;
  paused: boolean;
  awaitingEcho: boolean;
}

function dimensions(p: Record<string, unknown>): [number, number] {
  const size = [p.cols, p.rows];
  if (!size.every(n => typeof n === "number" && Number.isInteger(n) && n > 0 && n <= 32767)) throw new RpcError("invalid_params", "invalid terminal dimensions");
  return [p.cols as number, p.rows as number];
}

export class Terminals {
  private running = new Map<string, Running>();
  private streams = new Map<number, [Running, Attachment]>();
  constructor(private allocate: () => number, private activity: () => void) {}
  list() { return [...this.running.values()].map(t => t.info); }

  open(view: ConnectionView, p: Record<string, unknown>) {
    if (!native) throw new RpcError("capability_missing", terminalUnavailableReason!);
    const [cols, rows] = dimensions(p);
    if (typeof p.cwd !== "string" || !p.cwd || !statSync(p.cwd, { throwIfNoEntry: false })?.isDirectory()) throw new RpcError("invalid_params", "worktree folder does not exist");
    const available = shells();
    const shell = p.shell === undefined ? available[0] : available.find(s => s.path === p.shell);
    if (!shell) throw new RpcError("invalid_params", "shell is unavailable");
    const cwd = resolve(p.cwd);
    const terminal = randomUUID();
    const pty = native.spawn(shell.path, shellArgs(shell.path), { cwd, cols, rows, name: "xterm-256color", encoding: null,
      env: { ...process.env, TERM: "xterm-256color", COLORTERM: "truecolor", TERM_PROGRAM: "YAAC" } });
    const info = { terminal, cwd, shell: shell.path, title: shell.name, ...(typeof p.threadId === "string" ? { threadId: p.threadId } : {}) };
    const batch = new TerminalOutput(data => this.output(t, data));
    const t: Running = { info, pty, attachments: new Set(), history: Buffer.allocUnsafe(HISTORY), written: 0, batch, oscTail: "", paused: false, awaitingEcho: false };
    this.running.set(terminal, t);
    pty.onData(data => {
      batch.append(Buffer.isBuffer(data) ? data : Buffer.from(data));
      if (t.awaitingEcho) {
        t.awaitingEcho = false;
        batch.flush();
      }
    });
    pty.onExit(({ exitCode, signal }) => {
      t.exit = signal ? 128 + signal : exitCode;
      this.running.delete(terminal);
      t.batch.flush();
      for (const a of t.attachments) this.pump(t, a);
      this.activity();
    });
    this.activity();
    return { terminal, ...this.subscribe(t, view) };
  }

  private get(id: unknown) {
    const t = typeof id === "string" && this.running.get(id);
    if (!t) throw new RpcError("invalid_params", "terminal has exited or does not exist");
    return t;
  }
  resize(p: Record<string, unknown>) { this.get(p.terminal).pty.resize(...dimensions(p)); }
  /** The user's name for a terminal (P343); absent or empty restores the automatic title. */
  rename(p: Record<string, unknown>) {
    const t = this.get(p.terminal);
    if (p.name !== undefined && p.name !== null && typeof p.name !== "string") throw new RpcError("invalid_params", "name must be a string");
    const name = typeof p.name === "string" ? p.name.trim().slice(0, 200) : "";
    if (name) t.info.name = name; else delete t.info.name;
    for (const view of new Set([...t.attachments].map(a => a.view))) {
      view.notify("terminal/renamed", { terminal: t.info.terminal, ...(name ? { name } : {}) });
    }
  }
  close(id: unknown) { this.get(id).pty.kill(); }
  shutdown() { for (const t of this.running.values()) t.pty.kill(); }
  attach(view: ConnectionView, p: Record<string, unknown>) {
    const t = this.get(p.terminal);
    t.pty.resize(...dimensions(p));
    t.batch.flush();
    return this.subscribe(t, view);
  }

  private subscribe(t: Running, view: ConnectionView) {
    const stream = this.allocate();
    const len = Math.min(HISTORY, t.written);
    const at = (t.written - len) % HISTORY;
    const replay = Buffer.concat([t.history.subarray(at, at + len), t.history.subarray(0, Math.max(0, at + len - HISTORY))]);
    const a: Attachment = { view, stream, window: new ReceiveWindow(stream, CREDIT), queue: len ? [replay] : [], queued: len, sending: false, ready: false };
    t.attachments.add(a);
    this.streams.set(stream, [t, a]);
    view.openStream(stream, 0, true);
    // Publish the stream id in the response before any bytes or exit notification.
    setImmediate(() => { a.ready = true; this.pump(t, a); });
    return { stream, initialCredit: CREDIT };
  }

  private output(t: Running, data: Buffer) {
    const at = t.written % HISTORY;
    data.copy(t.history, at);
    if (at + data.length > HISTORY) data.copy(t.history, 0, HISTORY - at);
    t.written += data.length;
    if (t.oscTail || data.includes(0x1b)) {
      const osc = t.oscTail + data.toString("latin1");
      for (const match of osc.matchAll(/\x1b\][02];([^\x07\x1b]{0,4096})(?:\x07|\x1b\\)/g)) {
        t.info.title = Buffer.from(match[1]!, "latin1").toString();
        for (const view of new Set([...t.attachments].map(a => a.view))) view.notify("terminal/title", { terminal: t.info.terminal, title: t.info.title });
      }
      t.oscTail = osc.match(/\x1b(?:\](?:[02](?:;[^\x07\x1b]{0,4096}\x1b?)?)?)?$/)?.[0] ?? "";
    }
    for (const a of t.attachments) {
      a.queue.push(data);
      a.queued += data.length;
      this.pump(t, a);
    }
    this.flow(t);
  }

  private flow(t: Running) {
    const congested = [...t.attachments].some(a => a.queued >= CREDIT);
    if (congested !== t.paused && t.exit === undefined) {
      t.paused = congested;
      if (congested) t.pty.pause(); else t.pty.resume();
    }
  }
  private pump(t: Running, a: Attachment) {
    if (!a.ready || a.sending || !t.attachments.has(a)) return;
    const data = a.queue.shift();
    if (data) {
      a.sending = true;
      a.view.sendData(a.stream, data, () => {
        a.queued -= data.length;
        a.sending = false;
        this.flow(t);
        this.pump(t, a);
      });
    } else if (t.exit !== undefined) {
      this.remove(t, a);
      if (![...t.attachments].some(other => other.view === a.view)) {
        a.view.notify("terminal/exited", { terminal: t.info.terminal, code: t.exit });
      }
    }
  }
  receive(view: ConnectionView, stream: number, bytes: Buffer): boolean {
    const pair = this.streams.get(stream);
    if (!pair || pair[1].view !== view) return false;
    const [t, a] = pair;
    a.window.onReceived(bytes.length);
    if (t.exit === undefined) {
      t.awaitingEcho = true;
      t.pty.write(bytes);
    }
    const grant = a.window.onConsumed(bytes.length);
    if (grant) view.grantCredit(stream, grant);
    return true;
  }
  private remove(t: Running, a: Attachment) {
    t.attachments.delete(a);
    this.streams.delete(a.stream);
    a.view.cancelStream(a.stream);
    this.flow(t);
  }
  detach(view: ConnectionView, stream: number) {
    const pair = this.streams.get(stream);
    if (pair && pair[1].view === view) this.remove(...pair);
  }
  drop(view: ConnectionView) {
    for (const [t, a] of this.streams.values()) if (a.view === view) this.remove(t, a);
  }
}
