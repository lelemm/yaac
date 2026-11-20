// Backend selection for a Codex instance (docs/04-providers.md#backend-selection):
// 1. use the daemon that is running; 2. otherwise start it with `codex app-server daemon start`
// (under the instance's CODEX_HOME), then use it. An explicit socket is connect-only.
//
// yaacd starts a Codex daemon; it never stops, restarts, or updates one. Nothing in this file
// runs `daemon stop`, `restart`, `update`, or `bootstrap`.

import { execFile } from "node:child_process";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import type { ProviderStatus } from "../../protocol/generated/types.js";
import { CodexConnection, ConnectError } from "./rpc.js";

export interface ExecResult {
  code: number | null;
  stdout: string;
  stderr: string;
  /** Set when the program could not be run at all (ENOENT, EACCES). */
  spawnError?: string | null;
}

/** Runs a program; injectable so tests never touch a real `codex`. */
export type Exec = (file: string, args: string[], env: NodeJS.ProcessEnv, timeoutMs: number) => Promise<ExecResult>;

export const realExec: Exec = (file, args, env, timeoutMs) =>
  new Promise((resolve) => {
    execFile(file, args, { env, timeout: timeoutMs, maxBuffer: 4 * 1024 * 1024 }, (err, stdout, stderr) => {
      const e = err as (NodeJS.ErrnoException & { code?: string | number }) | null;
      if (e && typeof e.code === "string" && (e.code === "ENOENT" || e.code === "EACCES")) {
        resolve({ code: null, stdout: String(stdout ?? ""), stderr: String(stderr ?? ""), spawnError: e.code });
        return;
      }
      const code = e ? (typeof e.code === "number" ? e.code : 1) : 0;
      resolve({ code, stdout: String(stdout ?? ""), stderr: String(stderr ?? "") });
    });
  });

export interface EndpointPlan {
  socket: string;
  /** Explicit socket in the instance configuration: connect-only. */
  explicit: boolean;
  /** Environment for `codex` (the instance's CODEX_HOME applied). */
  env: NodeJS.ProcessEnv;
  codexBin: string;
}

/** Where this instance's daemon listens: the configured socket, else `$CODEX_HOME/app-server-control/…`. */
export function planEndpoint(opts: { socket?: string | null; env?: Record<string, string>; baseEnv: NodeJS.ProcessEnv; codexBin?: string | null }): EndpointPlan {
  const env: NodeJS.ProcessEnv = { ...opts.baseEnv, ...(opts.env ?? {}) };
  const home = env.CODEX_HOME && env.CODEX_HOME.length > 0 ? env.CODEX_HOME : path.join(env.HOME ?? os.homedir(), ".codex");
  const socket = opts.socket && opts.socket.length > 0 ? opts.socket : path.join(home, "app-server-control", "app-server-control.sock");
  return { socket, explicit: !!opts.socket, env, codexBin: opts.codexBin ?? "codex" };
}

export interface BackendFailure {
  status: ProviderStatus;
  message: string;
}

export type BackendResult = { ok: true; connection: CodexConnection; started: boolean } | ({ ok: false } & BackendFailure);

export interface ConnectOptions {
  exec: Exec;
  /** May this call start a daemon (false while reconnecting from a drop: just wait for it). */
  mayStart: boolean;
  log: (line: string) => void;
  /** Polling for the socket after `daemon start`. */
  startWaitMs?: number;
}

async function tryOpen(socket: string): Promise<CodexConnection | ConnectError> {
  try {
    return await CodexConnection.open(socket);
  } catch (e) {
    return e instanceof ConnectError ? e : new ConnectError("other", (e as Error).message);
  }
}

/**
 * Connects to the instance's daemon, starting it if that is allowed and needed. Keeps the
 * failure reasons distinct: not installed, too old, could not start, access denied, unreachable.
 */
export async function connectBackend(plan: EndpointPlan, opts: ConnectOptions): Promise<BackendResult> {
  const first = await tryOpen(plan.socket);
  if (first instanceof CodexConnection) return { ok: true, connection: first, started: false };
  if (first.reason === "accessDenied") {
    return { ok: false, status: "accessDenied", message: `access to the Codex daemon socket was denied: ${first.message}` };
  }
  if (plan.explicit) {
    return { ok: false, status: "unreachable", message: `nothing is listening on the configured Codex socket ${plan.socket} (an instance with an explicit socket is connect-only)` };
  }
  if (first.reason !== "absent") {
    return { ok: false, status: "unreachable", message: `could not talk to the Codex daemon: ${first.message}` };
  }
  if (!opts.mayStart) return { ok: false, status: "unreachable", message: "the Codex daemon is not running" };

  // Is Codex installed, and new enough to have the daemon?
  const version = await opts.exec(plan.codexBin, ["--version"], plan.env, 20_000);
  if (version.spawnError === "ENOENT") return { ok: false, status: "notInstalled", message: "Codex is not installed on this target (no `codex` on PATH)" };
  if (version.spawnError) return { ok: false, status: "accessDenied", message: `the codex program could not be run: ${version.spawnError}` };
  const found = (version.stdout || version.stderr).trim().split("\n")[0] ?? "";
  const daemonHelp = await opts.exec(plan.codexBin, ["app-server", "daemon", "--help"], plan.env, 20_000);
  if (daemonHelp.code !== 0) {
    return { ok: false, status: "tooOld", message: `Codex is too old for YAAC — update Codex on this target (found ${found || "an unknown version"}, which has no app-server daemon)` };
  }

  opts.log("no Codex daemon is running; starting one with `codex app-server daemon start`");
  const start = await opts.exec(plan.codexBin, ["app-server", "daemon", "start"], plan.env, 120_000);
  if (start.code !== 0) {
    const why = lastLine(start.stderr) || lastLine(start.stdout) || `exit code ${String(start.code)}`;
    return { ok: false, status: "startFailed", message: `the Codex daemon could not be started: ${why}` };
  }
  const deadline = Date.now() + (opts.startWaitMs ?? 15_000);
  let last: ConnectError | null = null;
  while (Date.now() < deadline) {
    const c = await tryOpen(plan.socket);
    if (c instanceof CodexConnection) return { ok: true, connection: c, started: true };
    last = c;
    if (c.reason === "accessDenied") return { ok: false, status: "accessDenied", message: `access to the Codex daemon socket was denied: ${c.message}` };
    await new Promise((r) => setTimeout(r, 200));
  }
  return { ok: false, status: "startFailed", message: `the Codex daemon was started but its socket did not answer: ${last?.message ?? plan.socket}` };
}

function lastLine(s: string): string {
  const lines = s.split("\n").map((l) => l.trim()).filter(Boolean);
  return lines[lines.length - 1] ?? "";
}

/** True when the socket file exists (for diagnostics only; connecting is the real test). */
export function socketExists(p: string): boolean {
  try {
    return fs.statSync(p).isSocket();
  } catch {
    return false;
  }
}
