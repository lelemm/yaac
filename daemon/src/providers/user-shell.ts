// Running a `!command` from the composer in yaacd itself (P344), for a harness that has no
// programmatic `!` of its own (Claude Code: docs/research/user-shell-commands-claude.md). The
// command runs on the target, in the thread's folder, with the target user's shell, outside
// any sandbox — as the harness's own `!` does.

import { spawn } from "node:child_process";

/** Claude Code's Bash tool default (`BASH_DEFAULT_TIMEOUT_MS`). */
export const USER_SHELL_TIMEOUT_MS = 120_000;
/** Claude Code's default `bashOutputMaxChars`: what the model is given of each stream. */
export const USER_SHELL_MAX_CHARS = 30_000;

export interface UserShellResult {
  stdout: string;
  stderr: string;
  /** The exit status; 124 after the timeout, 130 when stopped, 128 + n for another signal. */
  exitCode: number;
  durationMs: number;
  timedOut: boolean;
  stopped: boolean;
  /** Characters of each stream beyond what was kept. */
  dropped: { stdout: number; stderr: number };
}

export interface UserShellOptions {
  command: string;
  cwd: string;
  env: NodeJS.ProcessEnv;
  timeoutMs?: number;
  /** Live output, in order, both streams. */
  onOutput?: (chunk: string) => void;
  /** Stop: ends the command (its whole process group). */
  signal?: AbortSignal;
  /** What is kept of each stream for the record (default USER_SHELL_MAX_CHARS). */
  keepChars?: number;
}

/** The target user's shell and how to give it one command line. */
export function userShell(env: NodeJS.ProcessEnv, platform: NodeJS.Platform = process.platform): { file: string; args: (command: string) => string[] } {
  if (platform === "win32") {
    const file = env.ComSpec || env.COMSPEC || "cmd.exe";
    return { file, args: (c) => ["/d", "/s", "/c", c] };
  }
  const file = env.SHELL && env.SHELL.startsWith("/") ? env.SHELL : "/bin/sh";
  return { file, args: (c) => ["-c", c] };
}

const SIGNALS: Record<string, number> = { SIGHUP: 1, SIGINT: 2, SIGQUIT: 3, SIGKILL: 9, SIGTERM: 15 };

/** Runs one command line to its end (or its timeout, or Stop). Never throws for the command's own failure. */
export function runUserShell(o: UserShellOptions): Promise<UserShellResult> {
  const keep = o.keepChars ?? USER_SHELL_MAX_CHARS;
  const shell = userShell(o.env);
  const started = Date.now();
  return new Promise<UserShellResult>((resolve) => {
    const out = { stdout: "", stderr: "" };
    const dropped = { stdout: 0, stderr: 0 };
    let timedOut = false;
    let stopped = false;
    let settled = false;
    const child = spawn(shell.file, shell.args(o.command), {
      cwd: o.cwd,
      env: o.env,
      stdio: ["ignore", "pipe", "pipe"],
      detached: process.platform !== "win32",
      windowsHide: true,
    });
    const kill = () => {
      try {
        if (child.pid && process.platform !== "win32") process.kill(-child.pid, "SIGKILL");
        else child.kill("SIGKILL");
      } catch {
        /* already gone */
      }
    };
    const timer = setTimeout(() => {
      timedOut = true;
      kill();
    }, o.timeoutMs ?? USER_SHELL_TIMEOUT_MS);
    timer.unref();
    const onAbort = () => {
      stopped = true;
      kill();
    };
    if (o.signal?.aborted) onAbort();
    else o.signal?.addEventListener("abort", onAbort, { once: true });
    const take = (stream: "stdout" | "stderr") => (data: Buffer) => {
      const text = data.toString("utf8");
      o.onOutput?.(text);
      const room = Math.max(0, keep - out[stream].length);
      out[stream] += text.slice(0, room);
      dropped[stream] += Math.max(0, text.length - room);
    };
    child.stdout!.on("data", take("stdout"));
    child.stderr!.on("data", take("stderr"));
    const finish = (code: number | null, signal: NodeJS.Signals | null, error?: Error) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      o.signal?.removeEventListener("abort", onAbort);
      if (error) out.stderr += `${out.stderr && !out.stderr.endsWith("\n") ? "\n" : ""}${error.message}`;
      const exitCode = timedOut ? 124 : stopped ? 130 : code ?? (signal ? 128 + (SIGNALS[signal] ?? 0) : error ? 127 : -1);
      resolve({ stdout: out.stdout, stderr: out.stderr, exitCode, durationMs: Date.now() - started, timedOut, stopped, dropped });
    };
    child.on("error", (e) => finish(null, null, e));
    child.on("close", (code, signal) => finish(code, signal));
  });
}

/** Removes ANSI escape sequences and resolves carriage-return overwrites (progress bars). */
export function stripAnsi(text: string): string {
  const plain = text
    // CSI … final byte; OSC … BEL or ST; two-byte escapes.
    .replace(/\u001b\[[0-?]*[ -/]*[@-~]/g, "")
    .replace(/\u001b\][^\u0007\u001b]*(?:\u0007|\u001b\\)/g, "")
    .replace(/\u001b[@-Z\\-_]/g, "")
    .replace(/\r\n/g, "\n");
  return plain
    .split("\n")
    .map((line) => {
      const i = line.lastIndexOf("\r");
      return i < 0 ? line : line.slice(i + 1);
    })
    .join("\n");
}

/** XML-escapes command output as Claude Code does (`&`, `<`, `>`). */
export function escapeXmlText(text: string): string {
  return text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

/**
 * The two text blocks Claude Code's own `!` writes (docs/research/user-shell-commands-claude.md):
 * `<bash-input>` with the command as typed (not escaped), then the escaped output of each
 * stream and the exit code. Output is ANSI-stripped and capped at `USER_SHELL_MAX_CHARS` per
 * stream, with the truncation note Claude Code's own headless runner uses.
 */
export function bashMarkup(command: string, r: Pick<UserShellResult, "stdout" | "stderr" | "exitCode" | "dropped" | "timedOut">): [string, string] {
  const stream = (text: string, dropped: number) => {
    let t = stripAnsi(text);
    let more = dropped;
    if (t.length > USER_SHELL_MAX_CHARS) {
      more += t.length - USER_SHELL_MAX_CHARS;
      t = t.slice(0, USER_SHELL_MAX_CHARS);
    }
    return escapeXmlText(more > 0 ? `${t}\n[output truncated: ${more} more characters not shown]` : t);
  };
  let stderr = r.stderr;
  if (r.timedOut) stderr += `${stderr && !stderr.endsWith("\n") ? "\n" : ""}Command timed out after ${USER_SHELL_TIMEOUT_MS / 1000} s and was stopped.`;
  return [
    `<bash-input>${command}</bash-input>`,
    `<bash-stdout>${stream(r.stdout, r.dropped.stdout)}</bash-stdout><bash-stderr>${stream(stderr, r.dropped.stderr)}</bash-stderr><bash-exit-code>${r.exitCode}</bash-exit-code>`,
  ];
}
