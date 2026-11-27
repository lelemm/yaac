// The Claude Code processes a yaacd started, kept on disk (P385).
//
// yaacd runs one Claude Code process per live thread (through the Agent SDK). If yaacd dies
// without closing them (a crash, a SIGKILL), a process that was mid-turn keeps running — its
// parent is gone, but it finishes its turn and writes its session before it sees its input end.
// The next yaacd must know that process is its own: `claude agents` lists it as a live session
// holding the thread, and without this file it would be taken for another program (the thread
// read-only, Take over refused while it is busy). The file names each process by pid *and*
// start time, so a pid the system reused for something else is never taken as ours.

import * as fs from "node:fs";
import * as path from "node:path";
import { execFileSync } from "node:child_process";

/** One Claude Code process yaacd started. */
export interface ChildEntry {
  pid: number;
  /** When the process started, as the system tells it (`/proc/<pid>/stat` field 22 on Linux). */
  start: string;
  /** The thread (Claude Code session id) it runs. */
  native: string;
  /** The turn it was running, or null while idle. */
  turnId: string | null;
  /** When yaacd recorded it (ISO). */
  at: string;
}

/** How yaacd looks at and signals processes. Tests pass a fake process table. */
export interface ProcessTable {
  /** The process's start time, or null when no such process is running. */
  startTime(pid: number): string | null;
  kill(pid: number, signal: NodeJS.Signals): void;
}

/** The start time of a running process: `/proc` on Linux, `ps` elsewhere; null when it is gone. */
export function systemStartTime(pid: number): string | null {
  if (!Number.isInteger(pid) || pid <= 0) return null;
  if (process.platform === "linux") {
    try {
      const stat = fs.readFileSync(`/proc/${pid}/stat`, "utf8");
      // The command name (field 2) is in parentheses and may contain spaces: count after it.
      const rest = stat.slice(stat.lastIndexOf(")") + 2).split(" ");
      // rest[0] is field 3 (state); field 22 (starttime) is rest[19]. A zombie has ended.
      if (rest[0] === "Z") return null;
      return rest[19] ?? null;
    } catch {
      return null;
    }
  }
  try {
    const out = execFileSync("ps", ["-o", "lstart=", "-p", String(pid)], { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"], timeout: 2000 }).trim();
    return out || null;
  } catch {
    return null;
  }
}

export const systemProcesses: ProcessTable = {
  startTime: systemStartTime,
  kill: (pid, signal) => process.kill(pid, signal),
};

export class ChildrenFile {
  private entries = new Map<number, ChildEntry>();

  constructor(
    readonly file: string,
    private readonly procs: ProcessTable,
    private readonly log: (line: string) => void = () => {},
  ) {}

  /**
   * Reads what a previous yaacd left. Entries whose process still runs with the same start
   * time are returned (its orphans) and kept; the rest are dropped.
   */
  load(): ChildEntry[] {
    let raw: unknown;
    try {
      raw = JSON.parse(fs.readFileSync(this.file, "utf8"));
    } catch {
      raw = null;
    }
    const list = raw && typeof raw === "object" && Array.isArray((raw as { children?: unknown }).children) ? ((raw as { children: unknown[] }).children) : [];
    const alive: ChildEntry[] = [];
    for (const e of list) {
      if (!isEntry(e)) continue;
      const start = this.procs.startTime(e.pid);
      if (start === null || start !== e.start) continue; // gone, or the pid is someone else's now
      this.entries.set(e.pid, e);
      alive.push(e);
    }
    this.save();
    return alive;
  }

  /** Records a process yaacd just started. False when its start time cannot be read (it already ended). */
  add(pid: number, native: string, turnId: string | null): boolean {
    const start = this.procs.startTime(pid);
    if (start === null) return false;
    this.entries.set(pid, { pid, start, native, turnId, at: new Date().toISOString() });
    this.save();
    return true;
  }

  setTurn(pid: number, turnId: string | null): void {
    const e = this.entries.get(pid);
    if (!e || e.turnId === turnId) return;
    e.turnId = turnId;
    this.save();
  }

  remove(pid: number): void {
    if (this.entries.delete(pid)) this.save();
  }

  /** Whether the recorded process still runs (same pid, same start time). */
  alive(pid: number): boolean {
    const e = this.entries.get(pid);
    return !!e && this.procs.startTime(pid) === e.start;
  }

  get(pid: number): ChildEntry | undefined {
    return this.entries.get(pid);
  }

  private save(): void {
    try {
      fs.mkdirSync(path.dirname(this.file), { recursive: true, mode: 0o700 });
      const tmp = `${this.file}.${process.pid}.tmp`;
      fs.writeFileSync(tmp, JSON.stringify({ children: [...this.entries.values()] }, null, 2), { mode: 0o600 });
      fs.renameSync(tmp, this.file);
    } catch (e) {
      this.log(`could not write ${this.file}: ${(e as Error).message}`);
    }
  }
}

function isEntry(e: unknown): e is ChildEntry {
  if (!e || typeof e !== "object") return false;
  const r = e as Record<string, unknown>;
  return typeof r.pid === "number" && typeof r.start === "string" && typeof r.native === "string" && (r.turnId === null || typeof r.turnId === "string");
}
