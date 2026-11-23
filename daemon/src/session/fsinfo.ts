// Read-only filesystem helpers for pickers: `fs/list` and `project/resolve`.

import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { execFile } from "node:child_process";
import type { FsEntry, FsListParams, FsListResult, FsSearchParams, FsSearchResult, ProjectResolveResult } from "../protocol/generated/types.js";
import { gitRunner } from "./git.js";
import { Limiter } from "../util/exec.js";
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

const fileSearches = new Limiter(4);
const MATCH_LIMIT = 50;
const WALK_LIMIT = 20_000;
const searchGit = gitRunner(process.env, 3_000);

/** Search the target's working directory, including untracked but unignored git files. */
export function fsSearch({ cwd, query }: FsSearchParams): Promise<FsSearchResult> {
  if (typeof cwd !== "string" || !path.isAbsolute(cwd) || typeof query !== "string" || query.length > 512) {
    throw new RpcError("invalid_params", "an absolute cwd and a query of up to 512 characters are required");
  }
  if (fileSearches.pending >= 16) throw new RpcError("invalid_params", "file search is busy; try again");
  return fileSearches.run(async () => {
    if (!(await fs.promises.stat(cwd).catch(() => null))?.isDirectory()) {
      throw new RpcError("invalid_params", `cannot search ${cwd}`);
    }
    const terms = query.toLowerCase().replaceAll("\\", "/").split(/\s+/).filter(Boolean);
    const matches = (name: string) => terms.every((term) => name.toLowerCase().includes(term));
    const entries: FsEntry[] = [];
    const add = (name: string) => entries.push({ name, path: path.join(cwd, name), kind: "file" });
    const listed = await searchGit(cwd, ["ls-files", "--cached", "--others", "--exclude-standard", "--deduplicate", "-z", "--", "."]);
    const listedNames = listed.out.split("\0");
    listedNames.pop(); // A timed-out or capped command may end in a partial path.
    if (listed.ok || listedNames.length > 0) {
      const names = [...new Set(listedNames.filter(matches))];
      for (let i = 0; i < names.length; i += MATCH_LIMIT) {
        const checked = await Promise.all(names.slice(i, i + MATCH_LIMIT).map(async (name) =>
          (await fs.promises.stat(path.join(cwd, name)).catch(() => null))?.isFile() ? name : null));
        for (const name of checked) if (name) add(name);
        if (entries.length > MATCH_LIMIT) return { entries: entries.slice(0, MATCH_LIMIT), truncated: true };
      }
      return { entries, truncated: !listed.ok };
    }

    // Outside git, walk real directories only and keep remote searches bounded.
    const dirs = [""];
    let visited = 0;
    const deadline = Date.now() + 3_000;
    for (let i = 0; i < dirs.length; i++) {
      const dir = dirs[i]!;
      const stream = await fs.promises.opendir(path.join(cwd, dir)).catch(() => null);
      if (!stream) continue;
      for await (const entry of stream) {
        if (++visited > WALK_LIMIT || Date.now() > deadline || entries.length > MATCH_LIMIT) {
          return { entries: entries.slice(0, MATCH_LIMIT), truncated: true };
        }
        if (entry.name === ".git") continue;
        const name = dir ? `${dir}/${entry.name}` : entry.name;
        if (entry.isDirectory()) dirs.push(name);
        else if (entry.isFile() && matches(name)) add(name);
      }
    }
    return { entries: entries.slice(0, MATCH_LIMIT), truncated: entries.length > MATCH_LIMIT };
  });
}
