// Running short helper programs (git) without ever blocking the event loop: always
// asynchronous, always with a timeout and an output cap, and a missing program is an ordinary
// failure, never an exception.

import { execFile } from "node:child_process";

export interface RunResult {
  /** Exit code; null when the program could not be started, timed out, or was killed. */
  code: number | null;
  stdout: string;
  stderr: string;
  /** Why it did not run to completion (ENOENT, timeout, …), when it did not. */
  failure?: string;
}

export interface RunOptions {
  cwd?: string;
  env: NodeJS.ProcessEnv;
  timeoutMs: number;
  maxBytes?: number;
  input?: string;
  killSignal?: NodeJS.Signals;
}

export function run(program: string, args: string[], opts: RunOptions): Promise<RunResult> {
  return new Promise((resolve) => {
    try {
      const child = execFile(
        program,
        args,
        { cwd: opts.cwd, env: opts.env, timeout: opts.timeoutMs, maxBuffer: opts.maxBytes ?? 4 * 1024 * 1024, killSignal: opts.killSignal ?? "SIGTERM", windowsHide: true, encoding: "utf8" },
        (err, stdout, stderr) => {
          if (!err) {
            resolve({ code: 0, stdout, stderr });
            return;
          }
          const e = err as NodeJS.ErrnoException & { code?: number | string; killed?: boolean; signal?: string | null };
          if (typeof e.code === "number") resolve({ code: e.code, stdout: stdout ?? "", stderr: stderr ?? "" });
          else resolve({ code: null, stdout: stdout ?? "", stderr: stderr ?? "", failure: e.killed ? "timeout" : String(e.code ?? e.signal ?? e.message) });
        },
      );
      if (opts.input !== undefined && child.stdin) {
        // The helper can close stdin early; its exit status remains authoritative.
        child.stdin.on("error", () => {});
        child.stdin.end(opts.input, "utf8");
      }
    } catch (e) {
      resolve({ code: null, stdout: "", stderr: "", failure: (e as Error).message });
    }
  });
}

/** Bounded concurrency with two priorities: urgent work goes ahead of background work. */
export class Limiter {
  private running = 0;
  private urgent: Array<() => void> = [];
  private background: Array<() => void> = [];

  constructor(private readonly max: number) {}

  run<T>(fn: () => Promise<T>, urgent = false): Promise<T> {
    return new Promise<T>((resolve, reject) => {
      const start = () => {
        this.running++;
        fn().then(resolve, reject).finally(() => {
          this.running--;
          this.next();
        });
      };
      (urgent ? this.urgent : this.background).push(start);
      this.next();
    });
  }

  private next(): void {
    while (this.running < this.max) {
      const job = this.urgent.shift() ?? this.background.shift();
      if (!job) return;
      job();
    }
  }

  get pending(): number {
    return this.urgent.length + this.background.length + this.running;
  }
}
