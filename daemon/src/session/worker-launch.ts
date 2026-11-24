// The target user's optional adapter for the work between allocating a worker and
// sending its first prompt. This is a local subprocess interface, independent of
// the client and mesh wire protocols.

import * as fs from "node:fs";
import * as path from "node:path";
import { run } from "../util/exec.js";

const CONFIG_NAME = "worker-launch-hook.json";
const MAX_CONFIG_BYTES = 16 * 1024;
const MAX_OUTPUT_BYTES = 16 * 1024;
const TIMEOUT_MS = 15_000;
const ENV_KEYS = new Set([
  "HOME", "USERPROFILE", "LOCALAPPDATA", "APPDATA", "SYSTEMROOT", "WINDIR",
  "PATH", "PATHEXT", "TMPDIR", "TMP", "TEMP", "LANG", "LC_ALL",
]);

export interface WorkerLaunchIdentity {
  harness: string;
  instanceId: string;
  yaacThreadId: string;
  nativeThreadId: string;
  /** Omitted only when the provider established that it equals nativeThreadId. */
  sessionId?: string;
}

export interface WorkerLaunchEvent {
  event: "worker.beforeFirstPrompt";
  parent: WorkerLaunchIdentity;
  child: WorkerLaunchIdentity;
}

type LaunchHook = (event: WorkerLaunchEvent) => Promise<void>;

export function loadWorkerLaunchHook(home: string, daemonEnv: NodeJS.ProcessEnv): LaunchHook | null {
  const filename = path.join(home, CONFIG_NAME);
  let info: fs.Stats;
  try {
    info = fs.lstatSync(filename);
  } catch (e) {
    if ((e as NodeJS.ErrnoException).code === "ENOENT") return null;
    throw new Error(`Cannot read ${CONFIG_NAME}.`);
  }
  const uid = process.getuid?.();
  if (!info.isFile() || info.size > MAX_CONFIG_BYTES || (uid !== undefined && (info.uid !== uid || (info.mode & 0o022) !== 0))) {
    throw new Error(`${CONFIG_NAME} must be an ordinary file owned by the target user and writable only by that user.`);
  }
  let raw: unknown;
  try {
    raw = JSON.parse(fs.readFileSync(filename, "utf8"));
  } catch {
    throw new Error(`${CONFIG_NAME} must contain valid JSON.`);
  }
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) throw new Error(`Invalid ${CONFIG_NAME}.`);
  const { command, args = [] } = raw as { command?: unknown; args?: unknown };
  if (typeof command !== "string" || !path.isAbsolute(command) || command.includes("\0") ||
      !Array.isArray(args) || !args.every((arg): arg is string => typeof arg === "string" && !arg.includes("\0")) ||
      Object.keys(raw).some((key) => key !== "command" && key !== "args")) {
    throw new Error(`${CONFIG_NAME} requires an absolute executable path in command and accepts optional string args.`);
  }
  const env = Object.fromEntries(Object.entries(daemonEnv).filter(([key]) => ENV_KEYS.has(key.toUpperCase())));
  return async (event) => {
    const result = await run(command, args, {
      cwd: home, env, input: JSON.stringify(event) + "\n", timeoutMs: TIMEOUT_MS, maxBytes: MAX_OUTPUT_BYTES, killSignal: "SIGKILL",
    });
    if (result.code === 0) return;
    if (result.failure === "timeout") throw new Error("The worker launch hook did not complete within its time or output limit.");
    if (result.code === null) throw new Error("The configured worker launch hook could not complete. Check its executable and arguments.");
    let reason: unknown;
    try {
      reason = (JSON.parse(result.stdout) as { reason?: unknown } | null)?.reason;
    } catch {
      // Stderr and unstructured stdout may contain private diagnostics.
    }
    const detail = typeof reason === "string" && reason.length <= 500 && !/[\x00-\x1f\x7f]/.test(reason) ? reason.trim() : "";
    throw new Error(`The worker launch hook exited with status ${result.code}.${detail ? ` ${detail}` : ""}`);
  };
}
