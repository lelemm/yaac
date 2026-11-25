// Read-only worktree browser and diff pages (docs/12-files.md).
import { execFile } from "node:child_process";
import { createHash } from "node:crypto";
import { constants, type BigIntStats, type Stats } from "node:fs";
import * as fs from "node:fs/promises";
import * as path from "node:path";
import { promisify } from "node:util";
import { MAX_FRAME_LEN } from "../protocol/generated/constants.js";
import type {
  FsReadParams, FsReadResult, FsTreeEntry, FsTreeParams, FsTreeResult,
  GitFileDiffParams, GitFileDiffResult, GitFileStatus,
} from "../protocol/generated/types.js";
import { RpcError } from "../protocol/rpc.js";
import { Limiter } from "../util/exec.js";
import { GIT_TIMEOUT_MS } from "./git.js";

const FILE_LIMIT = 4 * 1024 * 1024;
const SAMPLE_BYTES = 8 * 1024;
// Allow for the envelope, cursor, old path and revision in a 64 KiB response.
const TEXT_BUDGET = 30 * 1024;
const TREE_LIMIT = 5_000;
const exec = promisify(execFile);
const jobs = new Limiter(4);
const windows = process.platform === "win32";
const gitEnv: NodeJS.ProcessEnv = {
  ...process.env, GIT_TERMINAL_PROMPT: "0", GIT_OPTIONAL_LOCKS: "0",
  GIT_NO_LAZY_FETCH: "1", LC_ALL: "C",
};
// Resolve the requested worktree even if yaacd inherited a caller's Git context.
for (const name of ["GIT_DIR", "GIT_WORK_TREE", "GIT_COMMON_DIR", "GIT_INDEX_FILE",
  "GIT_OBJECT_DIRECTORY", "GIT_ALTERNATE_OBJECT_DIRECTORIES", "GIT_PREFIX"]) delete gitEnv[name];

function schedule<T>(work: () => Promise<T>): Promise<T> {
  if (jobs.pending >= 32) throw new RpcError("files_busy", "too many pending file views");
  return jobs.run(work);
}

function invalid(message: string): RpcError {
  return new RpcError("invalid_params", message);
}

function missing(error: unknown): boolean {
  return (error as NodeJS.ErrnoException).code === "ENOENT";
}

function contains(root: string, candidate: string): boolean {
  const rel = path.relative(root, candidate);
  return rel !== ".." && !rel.startsWith(`..${path.sep}`) && !path.isAbsolute(rel);
}

function gitPath(root: string, candidate: string): string {
  return path.relative(root, candidate).split(path.sep).join("/");
}

interface WorktreePath {
  root: string;
  entry: string;
  real: string;
  stat?: Stats;
}

async function locate(rootArg: string, relative: string): Promise<WorktreePath> {
  if (typeof rootArg !== "string" || !path.isAbsolute(rootArg) || rootArg.includes("\0")) {
    throw invalid("an absolute worktree root is required");
  }
  if (typeof relative !== "string" || path.isAbsolute(relative) || relative.includes("\0")
    || (windows && relative.includes(":"))) {
    throw invalid("a relative worktree path is required");
  }
  try {
    const root = await fs.realpath(rootArg);
    let stat: Stats | undefined = await fs.stat(root);
    if (!stat.isDirectory()) throw invalid("the worktree root must be a directory");
    let real = root;
    let entry = root;
    const parts = (windows ? relative.split(/[\\/]/) : relative.split("/")).filter(p => p !== "" && p !== ".");
    for (const part of parts) {
      if (stat && !stat.isDirectory()) throw invalid("a path component is not a directory");
      entry = path.join(real, part);
      if ((part === ".." && real === root) || !contains(root, entry)) {
        throw invalid("the path leaves the worktree");
      }
      stat = await fs.lstat(entry).catch(error => {
        if (missing(error)) return undefined;
        throw error;
      });
      real = entry;
      if (stat?.isSymbolicLink()) {
        real = await fs.realpath(entry);
        if (!contains(root, real)) throw invalid("a symbolic link leaves the worktree");
        stat = await fs.stat(real);
      }
    }
    return { root, real, entry, ...(stat && { stat }) };
  } catch (error) {
    if (error instanceof RpcError) throw error;
    throw invalid(`cannot resolve the worktree path (${(error as NodeJS.ErrnoException).code ?? "error"})`);
  }
}

async function git(cwd: string, args: string[], maxBuffer = 16 * 1024 * 1024): Promise<Buffer> {
  const { stdout } = await exec("git", ["--no-pager", "-C", cwd, "-c", "core.fsmonitor=false", ...args], {
    env: gitEnv, timeout: GIT_TIMEOUT_MS, killSignal: "SIGKILL", maxBuffer, windowsHide: true, encoding: "buffer",
  });
  return stdout;
}

async function repoRoot(root: string): Promise<string | undefined> {
  try {
    const result = await git(root, ["rev-parse", "--show-toplevel"]);
    const top = await fs.realpath(result.toString("utf8").replace(windows ? /\r?\n$/ : /\n$/, ""));
    return contains(top, root) ? top : undefined;
  } catch (error) {
    const e = error as NodeJS.ErrnoException & { stderr?: Buffer };
    if (missing(error) || e.stderr?.toString().includes("not a git repository")) return undefined;
    throw new RpcError("git_status_failed", "could not find the worktree's git repository");
  }
}

interface Mark {
  git?: GitFileStatus;
  ignored?: boolean;
  oldPath?: string;
}

async function gitMarks(repo: string): Promise<Map<string, Mark>> {
  const marks = new Map<string, Mark>();
  let output: Buffer;
  try {
    output = await git(repo, [
      "status", "--porcelain=v2", "--ignored=matching", "--untracked-files=all", "--renames", "-z",
    ]);
  } catch {
    throw new RpcError("git_status_failed", "could not read git status");
  }
  const records = output.toString("utf8").split("\0");
  for (let i = 0; i < records.length - 1; i++) {
    const record = records[i]!;
    const kind = record[0];
    if (kind === "?" || kind === "!") {
      const name = record.slice(2).replace(/\/$/, "");
      const tracked = marks.get(name)?.git;
      if (kind === "!") {
        if (!tracked) marks.set(name, { ignored: true });
      } else {
        // A staged deletion can be followed by an untracked working copy of the same file.
        marks.set(name, { git: tracked === "D" ? "M" : "U" });
      }
      continue;
    }
    const fields = kind === "1" ? 8 : kind === "2" ? 9 : kind === "u" ? 10 : 0;
    if (!fields) continue;
    let end = -1;
    for (let field = 0; field < fields; field++) end = record.indexOf(" ", end + 1);
    const name = record.slice(end + 1);
    const xy = record.slice(2, 4);
    const oldPath = kind === "2" ? records[++i] : undefined;
    const status = kind === "u" ? "!" : xy.includes("R") ? "R"
      : xy.includes("D") ? "D" : xy.includes("A") || xy.includes("C") ? "A" : "M";
    marks.set(name, { git: status, ...(status === "R" && oldPath && { oldPath }) });
  }
  return marks;
}

export function fsTree(params: FsTreeParams): Promise<FsTreeResult> {
  return schedule(async () => {
    const loc = await locate(params.root, params.path);
    if (loc.stat && !loc.stat.isDirectory()) throw invalid("path is not a directory");
    const repo = await repoRoot(loc.root);
    const marks = repo ? await gitMarks(repo) : new Map<string, Mark>();
    if (!loc.stat) return treeEntries(loc, repo, marks);
    if (process.platform !== "linux") return treeEntries(loc, repo, marks, loc.real);
    // Read names and metadata through the verified directory descriptor on Linux.
    const { file } = await openConfined(loc, constants.O_DIRECTORY);
    try {
      return await treeEntries(loc, repo, marks, `/proc/self/fd/${file.fd}`);
    } finally {
      await file.close();
    }
  });
}

async function treeEntries(loc: WorktreePath, repo: string | undefined, marks: Map<string, Mark>, listPath?: string): Promise<FsTreeResult> {
  const directory = repo ? gitPath(repo, loc.real) : "";
  const prefix = directory ? `${directory}/` : "";
  const entries = new Map<string, FsTreeEntry>();
  // When an entire directory matches an ignore rule, git reports only the directory itself.
  let ignored = false;
  for (let parent = directory; parent; parent = parent.slice(0, Math.max(0, parent.lastIndexOf("/")))) {
    if (marks.get(parent)?.ignored) {
      ignored = true;
      break;
    }
  }
  const actual = listPath ? await fs.readdir(listPath, { withFileTypes: true }) : [];
  for (const child of actual) {
    const kind = child.isDirectory() ? "dir" : child.isFile() ? "file" : child.isSymbolicLink() ? "symlink" : undefined;
    if (kind) entries.set(child.name, { name: child.name, kind, ignored, changed: false });
  }
  for (const [file, mark] of marks) {
    if (!file.startsWith(prefix) || file === directory) continue;
    const [name, descendant] = file.slice(prefix.length).split("/", 2);
    if (!name) continue;
    let entry = entries.get(name);
    if (!entry) {
      // Keep missing deleted children, including removed folders needed to reach them.
      if (mark.git !== "D" && mark.git !== "R") continue;
      entry = { name, kind: descendant ? "dir" : "file", ignored: false, changed: false };
      entries.set(name, entry);
    }
    if (descendant) {
      if (mark.git && entry.kind === "dir") entry.changed = true;
    } else {
      if (mark.git) entry.git = mark.git;
      entry.ignored = mark.ignored === true;
    }
  }
  // A rename out of a folder also changes that folder even if the destination is elsewhere.
  for (const { oldPath } of marks.values()) {
    if (!oldPath?.startsWith(prefix)) continue;
    const [name, descendant] = oldPath.slice(prefix.length).split("/", 2);
    const entry = name ? entries.get(name) : undefined;
    if (descendant && entry?.kind === "dir") entry.changed = true;
  }
  if (!loc.stat && entries.size === 0) throw invalid("directory does not exist");
  const sorted = [...entries.values()].sort((a, b) => {
    if ((a.kind === "dir") !== (b.kind === "dir")) return a.kind === "dir" ? -1 : 1;
    return a.name < b.name ? -1 : a.name > b.name ? 1 : 0;
  });
  const result: FsTreeResult = { entries: [], repo: !!repo, truncated: false };
  let capacity = MAX_FRAME_LEN - 4096;
  const onDisk = new Set(actual.map(child => child.name));
  for (let i = 0; i < Math.min(sorted.length, TREE_LIMIT); i += 64) {
    const batch = sorted.slice(i, Math.min(i + 64, TREE_LIMIT));
    await Promise.all(batch.map(async entry => {
      if (!listPath || entry.kind !== "file" || !onDisk.has(entry.name)) return;
      const stat = await fs.lstat(path.join(listPath, entry.name)).catch(() => undefined);
      if (stat?.isFile()) {
        entry.size = stat.size;
        if (stat.mtimeMs >= 0) entry.mtime = Math.floor(stat.mtimeMs);
      }
    }));
    for (const entry of batch) {
      capacity -= Buffer.byteLength(JSON.stringify(entry)) + 1;
      if (capacity < 0) return { ...result, truncated: true };
      result.entries.push(entry);
    }
  }
  result.truncated = sorted.length > TREE_LIMIT;
  return result;
}

interface FileBytes {
  bytes: Buffer;
  size: number;
  revision: string;
}

function identity(stat: BigIntStats): string {
  return [stat.dev, stat.ino, stat.size, stat.mtimeNs, stat.ctimeNs].join(":");
}

async function openConfined(loc: WorktreePath, flags: number) {
  const file = await fs.open(loc.real, flags | constants.O_RDONLY | (constants.O_NOFOLLOW ?? 0) | (constants.O_NONBLOCK ?? 0));
  try {
    const before = await file.stat({ bigint: true });
    const actual = await fs.realpath(process.platform === "linux" ? `/proc/self/fd/${file.fd}` : loc.real);
    if (!contains(loc.root, actual)) throw invalid("the opened path leaves the worktree");
    const current = await fs.stat(actual, { bigint: true });
    if (current.dev !== before.dev || current.ino !== before.ino) {
      throw invalid("the path changed while opening it");
    }
    return { file, before };
  } catch (error) {
    await file.close();
    throw error;
  }
}

async function workingBytes(loc: WorktreePath, limit: number): Promise<FileBytes> {
  if (!loc.stat?.isFile()) throw invalid("path is not a regular file");
  const { file, before } = await openConfined(loc, 0);
  try {
    if (!before.isFile()) throw invalid("path is not a regular file");
    const size = Number(before.size);
    const bytes = Buffer.alloc(size > limit ? Math.min(size, SAMPLE_BYTES) : size);
    let offset = 0;
    while (offset < bytes.length) {
      const { bytesRead } = await file.read(bytes, offset, bytes.length - offset, offset);
      if (bytesRead === 0) break;
      offset += bytesRead;
    }
    if (offset !== bytes.length || identity(await file.stat({ bigint: true })) !== identity(before)) {
      throw new RpcError("file_changed", "the file changed while it was being read");
    }
    const revision = size > limit ? identity(before) : createHash("sha256").update(bytes).digest("hex");
    return { bytes, size, revision };
  } finally {
    await file.close();
  }
}

/** Snapshot bytes under the authenticated caller's worktree using the ordinary confined reader. */
export function artifactBytes(root: string, relative: string): Promise<Buffer> {
  return schedule(async () => {
    const limit = 64 * 1024 * 1024;
    const { size, bytes } = await workingBytes(await locate(root, relative), limit);
    if (!size || size > limit) throw invalid("present a nonempty artifact of at most 64 MiB");
    return bytes;
  });
}

function isBinary(bytes: Buffer): boolean {
  return bytes.subarray(0, SAMPLE_BYTES).includes(0);
}

function byteOffset(value: number | null | undefined): number | undefined {
  if (value == null) return undefined;
  if (!Number.isSafeInteger(value) || value < 0) throw invalid("invalid byte offset");
  return value;
}

// Keep a partially read UTF-8 sequence for the next page, including the valid prefix
// of a malformed sequence, so concatenated pages equal one lossy UTF-8 decode.
function utf8Boundary(bytes: Buffer, start: number, end: number): number {
  if (end === bytes.length) return end;
  let lead = end - 1;
  while (lead >= start && end - lead <= 3 && (bytes[lead]! & 0xc0) === 0x80) lead--;
  if (lead < start || end - lead > 3) return end;
  const first = bytes[lead]!;
  const width = first >= 0xc2 && first <= 0xdf ? 2 : first >= 0xe0 && first <= 0xef ? 3 : first >= 0xf0 && first <= 0xf4 ? 4 : 1;
  if (end - lead >= width) return end;
  if (lead + 1 < end) {
    const second = bytes[lead + 1]!;
    if ((first === 0xe0 && second < 0xa0) || (first === 0xed && second >= 0xa0)
      || (first === 0xf0 && second < 0x90) || (first === 0xf4 && second >= 0x90)) return end;
  }
  return lead;
}

function textPage(bytes: Buffer, start: number): { text: string; next?: number } {
  if (start > bytes.length) throw invalid("byte offset exceeds file size");
  let end = Math.min(bytes.length, start + TEXT_BUDGET);
  for (;;) {
    end = utf8Boundary(bytes, start, end);
    let text = bytes.toString("utf8", start, end);
    if (start === 0) text = text.replace(/^\uFEFF/, "");
    if (Buffer.byteLength(JSON.stringify(text)) <= TEXT_BUDGET) {
      return { text, ...(end < bytes.length && { next: end }) };
    }
    end = start + Math.floor((end - start) / 2);
  }
}

export function fsRead(params: FsReadParams): Promise<FsReadResult> {
  return schedule(async () => {
    if (!Number.isSafeInteger(params.maxBytes) || params.maxBytes < 0) throw invalid("invalid maxBytes");
    const offset = byteOffset(params.offset) ?? 0;
    const loc = await locate(params.root, params.path);
    const limit = Math.min(FILE_LIMIT, params.maxBytes);
    const { bytes, size, revision } = await workingBytes(loc, limit);
    const binary = isBinary(bytes);
    const truncated = size > limit;
    const result: FsReadResult = { size, truncated, etag: revision };
    if (binary) result.binary = true;
    if (!binary && !truncated) {
      const page = textPage(bytes, offset);
      result.text = page.text;
      if (page.next !== undefined) result.nextOffset = page.next;
    }
    return result;
  });
}

async function headBytes(repo: string, file: string): Promise<FileBytes | undefined> {
  let revision: string;
  try {
    const result = await git(repo, ["rev-parse", "--verify", `HEAD:${file}`]);
    revision = result.toString("utf8").trim();
  } catch (error) {
    if ((error as { code?: unknown }).code === 128) return undefined;
    throw new RpcError("git_diff_failed", "could not resolve the HEAD file");
  }
  if (!/^[0-9a-f]{40,64}$/.test(revision)) throw new RpcError("git_diff_failed", "invalid HEAD object id");
  try {
    const type = (await git(repo, ["cat-file", "-t", revision])).toString().trim();
    if (type !== "blob") return undefined;
    const size = Number((await git(repo, ["cat-file", "-s", revision])).toString().trim());
    if (size > FILE_LIMIT) throw new RpcError("file_too_large", `the HEAD file is ${size} bytes and exceeds 4 MiB`, { size });
    // Read the exact object resolved from HEAD above, even if HEAD advances in the meantime.
    const bytes = await git(repo, ["show", "--no-ext-diff", "--no-textconv", revision], FILE_LIMIT);
    return { bytes, size, revision };
  } catch (error) {
    if (error instanceof RpcError) throw error;
    throw new RpcError("git_diff_failed", "could not read the HEAD file");
  }
}

async function diffWorkingBytes(loc: WorktreePath): Promise<FileBytes> {
  if (loc.entry !== loc.real) {
    // HEAD stores a symlink's target string, not its referent.
    const bytes = await fs.readlink(loc.entry, { encoding: "buffer" });
    return { bytes, size: bytes.length, revision: createHash("sha256").update(bytes).digest("hex") };
  }
  return workingBytes(loc, FILE_LIMIT);
}

export function gitFileDiff(params: GitFileDiffParams): Promise<GitFileDiffResult> {
  return schedule(async () => {
    let oldOffset = byteOffset(params.oldOffset);
    let newOffset = byteOffset(params.newOffset);
    if (oldOffset === undefined && newOffset === undefined) oldOffset = newOffset = 0;
    const loc = await locate(params.root, params.path);
    const repo = await repoRoot(loc.root);
    if (!repo || !contains(repo, loc.entry) || loc.root === loc.entry) throw invalid("path has no git diff");
    const file = gitPath(repo, loc.entry);
    // Renames can cross any directory, so use the same full-repo status as the tree.
    const mark = (await gitMarks(repo)).get(file);
    if (!mark?.git) throw invalid("path has no git changes");
    const source = mark.oldPath ?? file;
    const inside = contains(loc.root, path.join(repo, source));
    const [old, working] = await Promise.all([
      mark.git === "U" || !inside ? undefined : headBytes(repo, source),
      loc.stat && mark.git !== "D" ? diffWorkingBytes(loc) : undefined,
    ]);
    if (working && working.size > FILE_LIMIT) {
      throw new RpcError("file_too_large", `the working file is ${working.size} bytes and exceeds 4 MiB`, { size: working.size });
    }
    const binary = !!((old && isBinary(old.bytes)) || (working && isBinary(working.bytes)));
    const etag = createHash("sha256")
      .update(JSON.stringify([file, source, mark.git, old?.revision, working?.revision])).digest("hex");
    const result: GitFileDiffResult = { status: mark.git, binary, etag };
    if (mark.oldPath && inside) result.oldPath = gitPath(loc.root, path.join(repo, source));
    if (!binary && old && oldOffset !== undefined) {
      const page = textPage(old.bytes, oldOffset);
      result.old = page.text;
      if (page.next !== undefined) result.nextOldOffset = page.next;
    }
    if (!binary && working && newOffset !== undefined) {
      const page = textPage(working.bytes, newOffset);
      result.new = page.text;
      if (page.next !== undefined) result.nextNewOffset = page.next;
    }
    return result;
  });
}
