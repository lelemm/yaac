// Read-only filesystem helpers for pickers: `fs/list` and `project/resolve`.

import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { execFile } from "node:child_process";
import type { FsEntry, FsListParams, FsListResult, ProjectResolveResult } from "../protocol/generated/types.js";
import { RpcError } from "../protocol/rpc.js";

export const FS_LIST_MAX = 2000;

export function fsList(params: FsListParams): FsListResult {
  const dir = path.resolve(params.path ?? os.homedir());
  let names: fs.Dirent[];
  try {
    names = fs.readdirSync(dir, { withFileTypes: true });
  } catch (e) {
    const code = (e as NodeJS.ErrnoException).code;
    throw new RpcError("invalid_params", `cannot list ${dir}: ${code ?? (e as Error).message}`);
  }
  const entries: FsEntry[] = [];
  for (const d of names) {
    if (!params.includeHidden && d.name.startsWith(".")) continue;
    const kind = d.isDirectory() ? "directory" : d.isFile() ? "file" : d.isSymbolicLink() ? "symlink" : "other";
    entries.push({ name: d.name, path: path.join(dir, d.name), kind });
  }
  entries.sort((a, b) => (a.kind === "directory") === (b.kind === "directory") ? a.name.localeCompare(b.name) : a.kind === "directory" ? -1 : 1);
  const parent = path.dirname(dir);
  const result: FsListResult = { path: dir, entries: entries.slice(0, FS_LIST_MAX) };
  if (parent !== dir) result.parent = parent;
  if (entries.length > FS_LIST_MAX) result.truncated = true;
  return result;
}

function git(cwd: string, args: string[]): Promise<string | null> {
  return new Promise((resolve) => {
    execFile("git", ["-C", cwd, ...args], { timeout: 5000, windowsHide: true, env: { ...process.env, GIT_TERMINAL_PROMPT: "0" } }, (err, stdout) => {
      resolve(err ? null : stdout.trim());
    });
  });
}

/** `git@host:a/b.git`, `https://user@host/a/b` → `host/a/b`: equal for the same repository. */
export function normaliseRemote(url: string): string {
  let u = url.trim();
  const scp = /^([^@/]+@)?([^:/]+):(?!\/)(.*)$/.exec(u);
  if (scp && !/^[a-z]+:\/\//i.test(u)) u = `${scp[2]}/${scp[3]}`;
  else u = u.replace(/^[a-z+]+:\/\//i, "").replace(/^[^@/]*@/, "");
  u = u.replace(/\.git\/?$/, "").replace(/\/+$/, "");
  const slash = u.indexOf("/");
  const host = (slash < 0 ? u : u.slice(0, slash)).toLowerCase().replace(/:\d+$/, "");
  return slash < 0 ? host : `${host}${u.slice(slash)}`;
}

export async function projectResolve(dir: string): Promise<ProjectResolveResult> {
  if (typeof dir !== "string" || dir.length === 0) throw new RpcError("invalid_params", "path is required");
  const abs = path.resolve(dir);
  if (!fs.existsSync(abs)) throw new RpcError("invalid_params", `${abs} does not exist`);
  const top = await git(abs, ["rev-parse", "--show-toplevel"]);
  if (!top) return { project: { path: abs, displayName: path.basename(abs) || abs } };
  const common = await git(abs, ["rev-parse", "--path-format=absolute", "--git-common-dir"]);
  const branchRaw = await git(abs, ["symbolic-ref", "--quiet", "--short", "HEAD"]);
  const origin = await git(abs, ["config", "--get", "remote.origin.url"]);
  const commonDir = common ? path.resolve(abs, common) : path.join(top, ".git");
  const repoDir = path.basename(commonDir) === ".git" ? path.dirname(commonDir) : commonDir.replace(/\.git$/, "");
  const result: ProjectResolveResult = {
    gitRoot: top,
    commonDir,
    branch: branchRaw || null,
    originUrl: origin || null,
    worktree: { path: top, branch: branchRaw || null },
    project: { path: commonDir, displayName: path.basename(repoDir) || top },
  };
  if (origin) result.project.crossTargetKey = normaliseRemote(origin);
  return result;
}
