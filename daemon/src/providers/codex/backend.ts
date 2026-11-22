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
  /** An explicit program; null means find it when it is needed (`findCodexExecutable`). */
  codexBin: string | null;
}

/** Where this instance's daemon listens: the configured socket, else `$CODEX_HOME/app-server-control/…`. */
export function planEndpoint(opts: { socket?: string | null; env?: Record<string, string>; baseEnv: NodeJS.ProcessEnv; codexBin?: string | null }): EndpointPlan {
  const env: NodeJS.ProcessEnv = { ...opts.baseEnv, ...(opts.env ?? {}) };
  const home = env.CODEX_HOME && env.CODEX_HOME.length > 0 ? env.CODEX_HOME : path.join(env.HOME ?? os.homedir(), ".codex");
  const socket = opts.socket && opts.socket.length > 0 ? opts.socket : path.join(home, "app-server-control", "app-server-control.sock");
  return { socket, explicit: !!opts.socket, env, codexBin: opts.codexBin ?? null };
}

/** The name of the instance-configuration (or process) variable that names the program. */
export const CODEX_EXECUTABLE_VAR = "YAAC_CODEX_EXECUTABLE";

function isExecutableFile(p: string): boolean {
  try {
    if (!fs.statSync(p).isFile()) return false;
    if (process.platform !== "win32") fs.accessSync(p, fs.constants.X_OK);
    return true;
  } catch {
    return false;
  }
}

/**
 * Finds the installed `codex`: an explicit override, then `PATH`, then where Codex's own
 * installer puts it (`~/.local/bin`, and the standalone package under `CODEX_HOME`). `yaacd`
 * is often started by a non-login shell (`wsl.exe`, `ssh host cmd`) whose `PATH` leaves out
 * `~/.local/bin`, so `PATH` alone is not enough (P130). Returns the path as found, not its real
 * path: the launcher symlink is what Codex's updater repoints. Null when there is none.
 */
export function findCodexExecutable(env: NodeJS.ProcessEnv): string | null {
  const explicit = env[CODEX_EXECUTABLE_VAR];
  if (explicit) return isExecutableFile(explicit) ? explicit : null;
  const names = process.platform === "win32" ? ["codex.exe", "codex.cmd", "codex"] : ["codex"];
  const dirs = (env.PATH ?? env.Path ?? "").split(path.delimiter).filter(Boolean);
  // Only the given environment's home: an environment without one (a test's) finds nothing there.
  const home = env.HOME ?? env.USERPROFILE;
  if (home) dirs.push(path.join(home, ".local", "bin"));
  const codexHome = env.CODEX_HOME && env.CODEX_HOME.length > 0 ? env.CODEX_HOME : home ? path.join(home, ".codex") : null;
  if (codexHome) dirs.push(path.join(codexHome, "packages", "standalone", "current", "bin"));
  for (const dir of dirs) {
    for (const name of names) {
      const candidate = path.join(dir, name);
      if (isExecutableFile(candidate)) return candidate;
    }
  }
  return null;
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
  const bin = plan.codexBin ?? findCodexExecutable(plan.env);
  const notInstalled = { ok: false as const, status: "notInstalled" as const, message: "Codex is not installed on this target (no `codex` on PATH, in ~/.local/bin, or in Codex's own package folder)" };
  if (!bin) return notInstalled;
  const env = withBinOnPath(plan.env, bin);
  const version = await opts.exec(bin, ["--version"], env, 20_000);
  if (version.spawnError === "ENOENT") return notInstalled;
  if (version.spawnError) return { ok: false, status: "accessDenied", message: `the codex program could not be run: ${version.spawnError}` };
  const found = (version.stdout || version.stderr).trim().split("\n")[0] ?? "";
  const daemonHelp = await opts.exec(bin, ["app-server", "daemon", "--help"], env, 20_000);
  if (daemonHelp.code !== 0) {
    return { ok: false, status: "tooOld", message: `Codex is too old for YAAC · update Codex on this target (found ${found || "an unknown version"}, which has no app-server daemon)` };
  }

  opts.log("no Codex daemon is running; starting one with `codex app-server daemon start`");
  const start = await opts.exec(bin, ["app-server", "daemon", "start"], env, 120_000);
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

/**
 * The environment for running `bin`, with its folder on `PATH` when it was found off `PATH`: the
 * daemon it starts, and the commands it runs for agents, then find `codex` as a login shell would.
 */
function withBinOnPath(env: NodeJS.ProcessEnv, bin: string): NodeJS.ProcessEnv {
  if (!path.isAbsolute(bin)) return env;
  const dir = path.dirname(bin);
  const key = env.PATH === undefined && env.Path !== undefined ? "Path" : "PATH";
  const dirs = (env[key] ?? "").split(path.delimiter).filter(Boolean);
  if (dirs.includes(dir)) return env;
  return { ...env, [key]: [...dirs, dir].join(path.delimiter) };
}

function lastLine(s: string): string {
  const lines = s.split("\n").map((l) => l.trim()).filter(Boolean);
  return lines[lines.length - 1] ?? "";
}
