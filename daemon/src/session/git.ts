// Git, as the sidebar needs it (docs/07-ui.md#sidebar, docs/05-threads.md#projects).
//
// Two questions, both answered by running `git` on the target — asynchronously, with bounded
// concurrency and timeouts, and tolerating a target without git:
//
// - Where does a folder sit in a repository? (`resolveRepo`) — which repository (its common
//   git directory), the checkout's root, and the folder's path relative to that root.
// - How does a checkout differ from origin? (`GitStatusStore`) — computed once per checkout,
//   never per thread, cached on disk, and refreshed only when asked: when a turn ends in that
//   checkout, and on a slow timer for checkouts some client is watching.

import * as fs from "node:fs";
import * as path from "node:path";
import type { GitStatus, RepoLocation } from "../protocol/generated/types.js";
import { Limiter, run } from "../util/exec.js";
import { nowIso, readJson, writeJsonAtomic } from "../util/fsx.js";

export const GIT_TIMEOUT_MS = 10_000;
/** The empty tree: what a repository without commits is compared with. */
const EMPTY_TREE = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

export interface GitRunner {
  (cwd: string, args: string[]): Promise<{ ok: boolean; out: string; missing?: boolean }>;
}

/** Runs git with no prompts, no optional locks (never takes the index lock), plain paths. */
export function gitRunner(env: NodeJS.ProcessEnv, timeoutMs = GIT_TIMEOUT_MS): GitRunner {
  const childEnv: NodeJS.ProcessEnv = { ...env, GIT_TERMINAL_PROMPT: "0", GIT_OPTIONAL_LOCKS: "0", LC_ALL: "C" };
  const once = async (cwd: string, args: string[]) => {
    const r = await run("git", ["-C", cwd, "-c", "core.quotepath=off", ...args], { env: childEnv, timeoutMs });
    return { ok: r.code === 0, out: r.stdout, missing: r.failure === "ENOENT" };
  };
  return once;
}

function slash(p: string): string {
  return p.split(path.sep).join("/");
}

/** Where `dir` sits in a git repository; null outside git, when git is absent, or on any failure. */
export async function resolveRepo(git: GitRunner, dir: string): Promise<RepoLocation | null> {
  if (!fs.existsSync(dir)) return null;
  const top = await git(dir, ["rev-parse", "--show-toplevel"]);
  if (!top.ok) return null;
  const root = top.out.trim();
  const common = await git(dir, ["rev-parse", "--path-format=absolute", "--git-common-dir"]);
  const gitDir = await git(dir, ["rev-parse", "--path-format=absolute", "--git-dir"]);
  if (!common.ok || !root) return null;
  const repo = path.resolve(dir, common.out.trim());
  let real = dir;
  try {
    real = fs.realpathSync(dir);
  } catch {
    /* keep it as given */
  }
  let realRoot = root;
  try {
    realRoot = fs.realpathSync(root);
  } catch {
    /* keep it as given */
  }
  const rel = slash(path.relative(realRoot, real)) || ".";
  const loc: RepoLocation = { repo, relative: rel.startsWith("..") ? "." : rel, root };
  const linked = gitDir.ok && path.resolve(dir, gitDir.out.trim()) !== repo;
  if (linked) loc.worktreeName = path.basename(root);
  return loc;
}

/** Counts `git diff --name-status -z` output by kind. */
export function countNameStatus(out: string): { added: number; modified: number; deleted: number } {
  const f = out.split("\0").filter((x) => x.length > 0);
  const c = { added: 0, modified: 0, deleted: 0 };
  for (let i = 0; i < f.length; ) {
    const status = f[i]!;
    const letter = status[0];
    // Renames and copies carry two paths (they do not occur with --no-renames, but be safe).
    i += letter === "R" || letter === "C" ? 3 : 2;
    if (letter === "A" || letter === "C") c.added++;
    else if (letter === "D") c.deleted++;
    else if (letter === "R") {
      c.added++;
      c.deleted++;
    } else c.modified++;
  }
  return c;
}

/**
 * The status of one checkout: branch; files added / modified / deleted against the base,
 * uncommitted changes and untracked files included; commits ahead / behind. The base is the
 * branch's upstream, else the remote's default branch; the files are counted against the
 * merge base, so they are what this checkout changed, not what others pushed since.
 */
export async function computeStatus(git: GitRunner, root: string): Promise<GitStatus | null> {
  const head = await git(root, ["rev-parse", "--verify", "--quiet", "HEAD"]);
  const hasCommits = head.ok;
  const branchOut = await git(root, ["symbolic-ref", "--quiet", "--short", "HEAD"]);
  if (!branchOut.ok && !hasCommits) {
    // Not a repository at all (or git is missing).
    const probe = await git(root, ["rev-parse", "--git-dir"]);
    if (!probe.ok) return null;
  }
  const status: GitStatus = { added: 0, modified: 0, deleted: 0, ahead: 0, behind: 0, at: nowIso() };
  const branch = branchOut.ok ? branchOut.out.trim() : "";
  if (branch) status.branch = branch;
  let base: string | null = null;
  if (hasCommits) {
    const up = await git(root, ["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}"]);
    if (up.ok && up.out.trim()) base = up.out.trim();
    if (!base) {
      const remotes = await git(root, ["remote"]);
      const names = remotes.ok ? remotes.out.split("\n").map((s) => s.trim()).filter(Boolean) : [];
      const remote = names.includes("origin") ? "origin" : names[0];
      if (remote) {
        const def = await git(root, ["symbolic-ref", "--quiet", "--short", `refs/remotes/${remote}/HEAD`]);
        if (def.ok && def.out.trim()) base = def.out.trim();
      }
    }
  }
  let compareWith = hasCommits ? "HEAD" : EMPTY_TREE;
  if (base) {
    status.base = base;
    const counts = await git(root, ["rev-list", "--left-right", "--count", `${base}...HEAD`]);
    const m = counts.ok ? /^(\d+)\s+(\d+)/.exec(counts.out.trim()) : null;
    if (m) {
      status.behind = Number(m[1]);
      status.ahead = Number(m[2]);
    }
    const mb = await git(root, ["merge-base", base, "HEAD"]);
    if (mb.ok && mb.out.trim()) compareWith = mb.out.trim();
  }
  let added = 0;
  let modified = 0;
  let deleted = 0;
  const diff = await git(root, ["diff", "--name-status", "--no-renames", "-z", compareWith, "--"]);
  if (diff.ok) {
    const c = countNameStatus(diff.out);
    added += c.added;
    modified += c.modified;
    deleted += c.deleted;
  }
  const untracked = await git(root, ["ls-files", "--others", "--exclude-standard", "-z"]);
  if (untracked.ok) added += untracked.out.split("\0").filter((x) => x.length > 0).length;
  status.added = added;
  status.modified = modified;
  status.deleted = deleted;
  return status;
}

interface StoreFile {
  checkouts: Record<string, GitStatus>;
}

/**
 * Git status per checkout root, cached on disk. `refresh` coalesces: a checkout already being
 * computed is not computed twice, and its waiters share the answer.
 */
export class GitStatusStore {
  private data: StoreFile;
  private inFlight = new Map<string, Promise<GitStatus | null>>();
  private saveTimer: NodeJS.Timeout | null = null;
  private closed = false;

  constructor(
    private readonly file: string,
    private readonly git: GitRunner,
    private readonly limiter: Limiter,
    private readonly onChange: (root: string) => void,
  ) {
    let loaded: StoreFile | undefined;
    try {
      loaded = readJson<StoreFile>(file);
    } catch {
      loaded = undefined;
    }
    this.data = loaded && typeof loaded.checkouts === "object" ? loaded : { checkouts: {} };
  }

  get(root: string): GitStatus | null {
    return this.data.checkouts[root] ?? null;
  }

  has(root: string): boolean {
    return root in this.data.checkouts;
  }

  /** Recomputes one checkout's status. A vanished checkout keeps its last status, marked removed. */
  refresh(root: string, urgent = false): Promise<GitStatus | null> {
    const running = this.inFlight.get(root);
    if (running) return running;
    const p = this.limiter.run(async () => {
      let next: GitStatus | null;
      if (!fs.existsSync(root)) {
        const prev = this.data.checkouts[root];
        next = prev ? { ...prev, removed: true } : null;
      } else {
        next = await computeStatus(this.git, root).catch(() => null);
      }
      if (this.closed) return next;
      const before = JSON.stringify(this.stable(this.data.checkouts[root]));
      if (next) this.data.checkouts[root] = next;
      else delete this.data.checkouts[root];
      if (JSON.stringify(this.stable(next ?? undefined)) !== before) {
        this.scheduleSave();
        this.onChange(root);
      }
      return next;
    }, urgent).finally(() => this.inFlight.delete(root));
    this.inFlight.set(root, p);
    return p;
  }

  /** The status without its timestamp, to tell a real change from a re-computation. */
  private stable(s: GitStatus | undefined): unknown {
    if (!s) return null;
    const { at: _at, ...rest } = s;
    return rest;
  }

  private scheduleSave(): void {
    if (this.saveTimer || this.closed) return;
    this.saveTimer = setTimeout(() => {
      this.saveTimer = null;
      this.save();
    }, 200);
  }

  save(): void {
    if (this.saveTimer) {
      clearTimeout(this.saveTimer);
      this.saveTimer = null;
    }
    writeJsonAtomic(this.file, this.data);
  }

  /** Resolves once nothing is being computed (tests, shutdown). */
  async idle(): Promise<void> {
    while (this.inFlight.size > 0) await Promise.all([...this.inFlight.values()]);
  }

  close(): void {
    this.save();
    this.closed = true;
  }
}
