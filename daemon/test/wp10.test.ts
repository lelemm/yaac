// Projects, git status, thread flags, item/read, item timestamps, context usage, fork without
// a first message, and one attachment folder per thread.

import { test } from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { FAKE, RpcFailure, client, fakeOf, opId, startDaemon, startThread, text, tmpHome, until, waitTurnDone, readAll } from "./helpers.js";
import { matchProject, projectRefFor } from "../src/session/projects.js";
import { GitStatusStore, computeStatus, gitRunner, resolveRepo } from "../src/session/git.js";
import { Limiter } from "../src/util/exec.js";
import type { Project, RepoLocation } from "../src/protocol/generated/types.js";

// ── The matching rule: the examples of docs/05-threads.md, verbatim ─────────────────────────

function proj(name: string, folder: string, repo?: RepoLocation): Project {
  return { id: `pj_${name}`, name, folder, createdAt: "2026-01-01T00:00:00Z", ...(repo ? { repo } : {}) };
}

test("projects: the folder rule — deepest project containing the thread's folder", () => {
  const home = proj("~", "~");
  const abc = proj("abc", "~/projects/abc");
  const table: Array<[Project[], string, string | null]> = [
    [[home, abc], "~/projects/abc/123", "abc"],
    [[home, abc], "~/projects", "~"],
    [[abc], "~/projects/abc/123/src", "abc"],
    [[abc], "~/other", null],
    // Whole components only: abcdef is not inside abc.
    [[abc], "~/projects/abcdef", null],
  ];
  for (const [projects, folder, want] of table) {
    assert.equal(matchProject(projects, { folder })?.name ?? null, want, `${folder}`);
  }
});

test("projects: the repository rule — same repository, relative path inside, tried first", () => {
  const yaacRepo = "~/code/yaac/.git";
  const monoRepo = "~/code/mono/.git";
  const yaac = proj("yaac", "~/code/yaac", { repo: yaacRepo, relative: ".", root: "~/code/yaac" });
  const billing = proj("Billing", "~/code/mono/services/billing", { repo: monoRepo, relative: "services/billing", root: "~/code/mono" });
  const home = proj("Home", "~");
  const table: Array<[Project[], string, RepoLocation, string | null]> = [
    [[yaac], "~/code/yaac-wt/feature-x", { repo: yaacRepo, relative: ".", root: "~/code/yaac-wt/feature-x", worktreeName: "feature-x" }, "yaac"],
    [[yaac], "~/.codex/worktrees/a1b2/yaac/crates", { repo: yaacRepo, relative: "crates", root: "~/.codex/worktrees/a1b2/yaac", worktreeName: "yaac" }, "yaac"],
    [[billing], "~/wt/hotfix/services/billing/src", { repo: monoRepo, relative: "services/billing/src", root: "~/wt/hotfix", worktreeName: "hotfix" }, "Billing"],
    [[billing], "~/wt/hotfix/services/search", { repo: monoRepo, relative: "services/search", root: "~/wt/hotfix", worktreeName: "hotfix" }, null],
    [[home, yaac], "~/code/yaac-wt/feature-x", { repo: yaacRepo, relative: ".", root: "~/code/yaac-wt/feature-x", worktreeName: "feature-x" }, "yaac"],
  ];
  for (const [projects, folder, repo, want] of table) {
    assert.equal(matchProject(projects, { folder, repo })?.name ?? null, want, `${folder}`);
  }
  // No match: a temporary project named after the folder.
  assert.deepEqual(projectRefFor([billing], { folder: "~/other" }), { kind: "temporary", path: "~/other", displayName: "~/other" });
  const ref = projectRefFor([yaac], { folder: "~/code/yaac/src" });
  assert.equal(ref.kind, "user");
  assert.equal(ref.id, "pj_yaac");
  assert.equal(ref.displayName, "yaac");
});

// ── Git status against real repositories ────────────────────────────────────────────────────

const GIT_ENV: NodeJS.ProcessEnv = {
  ...process.env,
  GIT_CONFIG_GLOBAL: "/dev/null",
  GIT_CONFIG_NOSYSTEM: "1",
  GIT_AUTHOR_NAME: "t",
  GIT_AUTHOR_EMAIL: "t@example.invalid",
  GIT_COMMITTER_NAME: "t",
  GIT_COMMITTER_EMAIL: "t@example.invalid",
};

function g(cwd: string, ...args: string[]): string {
  return execFileSync("git", ["-C", cwd, ...args], { env: GIT_ENV, encoding: "utf8" });
}

function hasGit(): boolean {
  try {
    execFileSync("git", ["--version"], { env: GIT_ENV });
    return true;
  } catch {
    return false;
  }
}

/** A bare origin, a clone with one pushed commit on main, and a second clone. */
function repos(): { dir: string; origin: string; work: string; other: string } {
  const dir = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "yaac-git-")));
  const origin = path.join(dir, "origin.git");
  g(dir, "init", "--bare", "-b", "main", origin);
  const seed = path.join(dir, "seed");
  g(dir, "init", "-b", "main", seed);
  fs.writeFileSync(path.join(seed, "a.txt"), "a\n");
  fs.writeFileSync(path.join(seed, "b.txt"), "b\n");
  g(seed, "add", ".");
  g(seed, "commit", "-m", "one");
  g(seed, "remote", "add", "origin", origin);
  g(seed, "push", "-u", "origin", "main");
  const work = path.join(dir, "work");
  g(dir, "clone", origin, work);
  const other = path.join(dir, "other");
  g(dir, "clone", origin, other);
  return { dir, origin, work, other };
}

test("git status: clean, modified, added, deleted, ahead, behind, no upstream, not a repository, a second worktree", { skip: !hasGit() }, async () => {
  const git = gitRunner(GIT_ENV);
  const { dir, work, other } = repos();
  const st = async () => (await computeStatus(git, work))!;

  let s = await st();
  assert.equal(s.branch, "main");
  assert.equal(s.base, "origin/main");
  assert.deepEqual([s.added, s.modified, s.deleted, s.ahead, s.behind], [0, 0, 0, 0, 0], "clean");

  fs.writeFileSync(path.join(work, "a.txt"), "changed\n");
  s = await st();
  assert.deepEqual([s.added, s.modified, s.deleted], [0, 1, 0], "modified");

  fs.writeFileSync(path.join(work, "new.txt"), "new\n");
  s = await st();
  assert.deepEqual([s.added, s.modified, s.deleted], [1, 1, 0], "untracked counts as added");
  g(work, "add", "new.txt");
  s = await st();
  assert.deepEqual([s.added, s.modified, s.deleted], [1, 1, 0], "staged counts as added");

  fs.rmSync(path.join(work, "b.txt"));
  s = await st();
  assert.deepEqual([s.added, s.modified, s.deleted], [1, 1, 1], "deleted");

  g(work, "add", "-A");
  g(work, "commit", "-m", "local");
  s = await st();
  assert.deepEqual([s.added, s.modified, s.deleted, s.ahead, s.behind], [1, 1, 1, 1, 0], "committed, not pushed: still differs from origin");

  fs.writeFileSync(path.join(other, "c.txt"), "c\n");
  g(other, "add", ".");
  g(other, "commit", "-m", "theirs");
  g(other, "push", "origin", "main");
  g(work, "fetch", "origin");
  s = await st();
  assert.equal(s.behind, 1, "behind");
  assert.equal(s.ahead, 1);
  assert.deepEqual([s.added, s.modified, s.deleted], [1, 1, 1], "their change is not ours");

  g(work, "checkout", "-q", "-b", "topic");
  fs.writeFileSync(path.join(work, "t.txt"), "t\n");
  g(work, "add", ".");
  g(work, "commit", "-m", "topic");
  s = await st();
  assert.equal(s.branch, "topic");
  assert.equal(s.base, "origin/main", "no upstream: the remote's default branch");
  assert.equal(s.ahead, 2);
  assert.equal(s.added, 2);

  const plain = fs.mkdtempSync(path.join(os.tmpdir(), "yaac-plain-"));
  assert.equal(await computeStatus(git, plain), null, "not a repository");
  assert.equal(await resolveRepo(git, plain), null);

  const wt = path.join(dir, "wt-feature");
  g(work, "worktree", "add", "-q", "-b", "feature", wt);
  fs.mkdirSync(path.join(wt, "sub"));
  const main = await resolveRepo(git, work);
  const linked = await resolveRepo(git, path.join(wt, "sub"));
  assert.ok(main && linked);
  assert.equal(linked.repo, main.repo, "the same repository");
  assert.equal(main.worktreeName, undefined);
  assert.equal(linked.worktreeName, "wt-feature");
  assert.equal(linked.relative, "sub");
  assert.equal(linked.root, wt);
  const ws = (await computeStatus(git, wt))!;
  assert.equal(ws.branch, "feature");

  // Once per checkout, cached; a vanished checkout keeps its status, marked removed.
  const changed: string[] = [];
  const store = new GitStatusStore(path.join(dir, "git-status.json"), git, new Limiter(2), (root) => changed.push(root));
  const [x, y] = await Promise.all([store.refresh(wt), store.refresh(wt)]);
  assert.deepEqual(x, y);
  assert.deepEqual(changed, [wt], "two refreshes at once compute once");
  fs.rmSync(wt, { recursive: true, force: true });
  const gone = await store.refresh(wt);
  assert.equal(gone?.removed, true);
  assert.equal(gone?.branch, "feature");
  store.close();
  fs.rmSync(dir, { recursive: true, force: true });
  fs.rmSync(plain, { recursive: true, force: true });
});

test("git status: a target without git has no git status, and nothing fails", async () => {
  const empty = fs.mkdtempSync(path.join(os.tmpdir(), "yaac-nogit-"));
  const git = gitRunner({ PATH: empty });
  assert.equal(await resolveRepo(git, os.tmpdir()), null);
  assert.equal(await computeStatus(git, os.tmpdir()), null);
  fs.rmSync(empty, { recursive: true, force: true });
});

test("git status in summaries: once per checkout, refreshed when a turn ends there", { skip: !hasGit() }, async () => {
  const { dir, work } = repos();
  const home = tmpHome();
  const daemon = await startDaemon(home, {
    env: { YAAC_FAKE_PROVIDER: "1", YAAC_CODEX_PROVIDER: "0", YAAC_CLAUDE_PROVIDER: "0", PATH: process.env.PATH ?? "", GIT_CONFIG_GLOBAL: "/dev/null", GIT_CONFIG_NOSYSTEM: "1" },
  });
  const c = await client(daemon);
  const a: any = await startThread(c, "/fake say one", { cwd: work });
  const b: any = await startThread(c, "/fake say two", { cwd: work });
  await c.request("thread/subscribe", { threadId: a.threadId, epoch: 1, afterSeq: 0 });
  await waitTurnDone(c, a.threadId, a.turnId);
  const summary = async (id: string) => ((await c.request("thread/sync", {})) as any).changed.find((s: any) => s.id === id);
  const s1 = await until(async () => (await summary(a.threadId))?.git, "git status");
  assert.equal(s1.branch, "main");
  assert.equal(s1.modified, 0);
  await until(async () => (await summary(b.threadId))?.git, "git status of the second thread");
  daemon.core.gitStatus.save();
  const stored = JSON.parse(fs.readFileSync(path.join(home, "git-status.json"), "utf8"));
  assert.deepEqual(Object.keys(stored.checkouts), [work], "one entry per checkout, not per thread");
  assert.equal((await summary(a.threadId)).repo.root, work);
  fs.writeFileSync(path.join(work, "a.txt"), "changed\n");
  const t2: any = await c.request("turn/start", { opId: opId(), threadId: a.threadId, input: text("/fake say again") });
  await waitTurnDone(c, a.threadId, t2.turnId);
  await until(async () => (await summary(a.threadId))?.git?.modified === 1, "refresh after the turn");
  await until(async () => (await summary(b.threadId))?.git?.modified === 1, "the other thread in the checkout follows");
  await c.close();
  await daemon.stop("test");
  fs.rmSync(dir, { recursive: true, force: true });
});

// ── Thread flags and projects through the daemon ─────────────────────────────────────────────

test("mute, pin, and the agents policy are stored on the target and survive a restart", async () => {
  const home = tmpHome();
  let daemon = await startDaemon(home);
  let c = await client(daemon);
  const t: any = await startThread(c, "/fake say hi", { agents: "allow" });
  await c.request("thread/subscribe", { threadId: t.threadId, epoch: 1, afterSeq: 0 });
  await waitTurnDone(c, t.threadId, t.turnId);
  let s: any = ((await c.request("thread/sync", {})) as any).changed.find((x: any) => x.id === t.threadId);
  assert.deepEqual([s.muted, s.pinned, s.agents], [false, false, "allow"], "agents from thread/start");
  const before = c.notifications.length;
  const r: any = await c.request("thread/update", { opId: opId(), threadId: t.threadId, muted: true, pinned: true, agents: "ask" });
  assert.deepEqual([r.thread.muted, r.thread.pinned, r.thread.agents], [true, true, "ask"]);
  await until(() => c.notifications.slice(before).some((n) => n.method === "thread/updated" && n.params.thread.muted === true), "thread/updated");
  // A patch: only what is named changes.
  await c.request("thread/update", { opId: opId(), threadId: t.threadId, pinned: false });
  await assert.rejects(c.request("thread/update", { opId: opId(), threadId: t.threadId, muted: "yes" }), (e: unknown) => e instanceof RpcFailure && e.code === "invalid_params");
  await c.close();
  await daemon.stop("test");
  daemon = await startDaemon(home);
  c = await client(daemon);
  s = ((await c.request("thread/sync", {})) as any).changed.find((x: any) => x.id === t.threadId);
  assert.deepEqual([s.muted, s.pinned, s.agents], [true, false, "ask"]);
  assert.ok(typeof s.nativeId === "string" && s.nativeId.length > 0, "the harness's own id is on the summary");
  await c.close();
  await daemon.stop("test");
});

test("projects: create pulls threads in at once, rename and delete, thread/start in a project", async () => {
  const home = tmpHome();
  const root = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "yaac-proj-")));
  const inner = path.join(root, "abc", "123");
  fs.mkdirSync(inner, { recursive: true });
  const daemon = await startDaemon(home);
  const c = await client(daemon);
  const t: any = await startThread(c, "/fake say hi", { cwd: inner });
  await c.request("thread/subscribe", { threadId: t.threadId, epoch: 1, afterSeq: 0 });
  await waitTurnDone(c, t.threadId, t.turnId);
  const summary = async () => ((await c.request("thread/sync", {})) as any).changed.find((x: any) => x.id === t.threadId);
  assert.deepEqual((await summary()).project, { kind: "temporary", path: inner, displayName: inner });

  const before = c.notifications.length;
  const created: any = await c.request("project/create", { opId: opId(), folder: path.join(root, "abc"), name: "ABC" });
  assert.equal(created.project.name, "ABC");
  const p = (await summary()).project;
  assert.deepEqual([p.kind, p.id, p.displayName, p.path], ["user", created.project.id, "ABC", path.join(root, "abc")]);
  assert.ok(c.notifications.slice(before).some((n) => n.method === "project/updated" && n.params.projects.length === 1));
  assert.ok(c.notifications.slice(before).some((n) => n.method === "thread/updated" && n.params.thread.id === t.threadId && n.params.thread.project.kind === "user"));

  // A deeper project wins.
  const deeper: any = await c.request("project/create", { opId: opId(), folder: inner, name: "One-two-three" });
  assert.equal((await summary()).project.id, deeper.project.id);
  await c.request("project/rename", { opId: opId(), projectId: deeper.project.id, name: "Renamed" });
  assert.equal((await summary()).project.displayName, "Renamed");
  await c.request("project/delete", { opId: opId(), projectId: deeper.project.id });
  assert.equal((await summary()).project.id, created.project.id, "falls back to the outer project");
  assert.ok(fs.existsSync(inner), "deleting a project does not touch its folder");
  const list: any = await c.request("project/list", {});
  assert.deepEqual(list.projects.map((x: any) => x.name), ["ABC"]);

  await assert.rejects(c.request("project/create", { opId: opId(), folder: path.join(root, "missing"), name: "x" }), (e: unknown) => e instanceof RpcFailure && e.code === "invalid_params");

  // A thread started in a project starts in the root of its folder.
  const s2: any = await c.request("thread/start", { opId: opId(), instance: FAKE, cwd: "", project: created.project.id, input: text("/fake say in project") });
  await waitTurnDone(c, s2.threadId, s2.turnId).catch(() => undefined);
  const sum2 = ((await c.request("thread/sync", {})) as any).changed.find((x: any) => x.id === s2.threadId);
  assert.equal(sum2.cwd, path.join(root, "abc"));
  await assert.rejects(
    c.request("thread/start", { opId: opId(), instance: FAKE, cwd: inner, project: created.project.id, input: text("x") }),
    (e: unknown) => e instanceof RpcFailure && e.code === "invalid_params",
    "a cwd that is not the project's folder",
  );
  await assert.rejects(
    c.request("thread/start", { opId: opId(), instance: FAKE, cwd: "", project: "pj_nope", input: text("x") }),
    (e: unknown) => e instanceof RpcFailure && e.code === "invalid_params",
  );
  await c.close();
  await daemon.stop("test");
  fs.rmSync(root, { recursive: true, force: true });
});

// ── item/read, timestamps, context usage, fork without a first message ───────────────────────

test("item/read: byte ranges of a large output, never splitting a character", async () => {
  const home = tmpHome();
  const daemon = await startDaemon(home);
  const c = await client(daemon);
  const t: any = await startThread(c, "/fake big-output 100000");
  await c.request("thread/subscribe", { threadId: t.threadId, epoch: 1, afterSeq: 0 });
  await waitTurnDone(c, t.threadId, t.turnId, 20000);
  const tool = (await readAll(c, t.threadId)).flatMap((p) => p.items).find((i) => i.kind === "toolCall") as any;
  let offset = 0;
  let total = 0;
  for (let i = 0; i < 100; i++) {
    const r: any = await c.request("item/read", { threadId: t.threadId, itemId: tool.id, offset, maxBytes: 30000 });
    assert.equal(r.present, true);
    assert.equal(r.offset, offset);
    total += Buffer.byteLength(r.data);
    offset = r.nextOffset;
    if (!r.more) break;
  }
  assert.equal(total, 100000);
  assert.equal(offset, 100000);
  const gone: any = await c.request("item/read", { threadId: t.threadId, itemId: "no-such-item", maxBytes: 10 });
  assert.equal(gone.present, false);

  // Multi-byte text: written straight into the output file, read in tiny ranges.
  const file = path.join(home, "threads", (await import("../src/session/thread.js")).threadDirName(t.threadId), "outputs", `${createHash("sha256").update("item-utf8").digest("hex").slice(0, 32)}.out`);
  const textOut = "αβγ — 日本語 🙂 end";
  fs.writeFileSync(file, textOut);
  let got = "";
  offset = 0;
  for (let i = 0; i < 100; i++) {
    const r: any = await c.request("item/read", { threadId: t.threadId, itemId: "item-utf8", offset, maxBytes: 5 });
    got += r.data;
    assert.ok(!r.data.includes("�"), "no broken characters");
    offset = r.nextOffset;
    if (!r.more) break;
  }
  assert.equal(got, textOut);
  // An offset inside a character moves forward to the next one.
  const mid: any = await c.request("item/read", { threadId: t.threadId, itemId: "item-utf8", offset: 1, maxBytes: 100 });
  assert.equal(mid.offset, 2);
  assert.ok(textOut.endsWith(mid.data));
  await c.close();
  await daemon.stop("test");
});

test("items carry timestamps; subscribe and read carry context usage; fork without a first message", async () => {
  const home = tmpHome();
  const daemon = await startDaemon(home);
  const c = await client(daemon);
  const t: any = await startThread(c, "/fake reason thinking\n/fake usage\n/fake say done");
  await c.request("thread/subscribe", { threadId: t.threadId, epoch: 1, afterSeq: 0 });
  await waitTurnDone(c, t.threadId, t.turnId);
  const items = (await readAll(c, t.threadId)).flatMap((p) => p.items);
  for (const kind of ["userMessage", "reasoning", "agentMessage"]) {
    const it = items.find((i) => i.kind === kind) as any;
    assert.ok(it?.startedAt && it?.completedAt, `${kind} has both timestamps`);
    assert.ok(it.completedAt >= it.startedAt);
  }
  const first: any = await c.request("thread/read", { threadId: t.threadId });
  assert.deepEqual(first.context, { used: 150, window: 100000 });
  const sub: any = await c.request("thread/subscribe", { threadId: t.threadId });
  assert.deepEqual(sub.context, { used: 150, window: 100000 });

  const fork: any = await c.request("thread/forkOnly", { opId: opId(), source: t.threadId, instance: FAKE });
  assert.equal(fork.forkKind, "native");
  assert.equal(fork.turnId, undefined);
  const fakeProvider = fakeOf(daemon);
  const handed = fakeProvider.received.length;
  const turn: any = await c.request("turn/start", { opId: opId(), threadId: fork.threadId, input: text("/fake say first message of the fork") });
  await c.request("thread/subscribe", { threadId: fork.threadId, epoch: 1, afterSeq: 0 });
  await waitTurnDone(c, fork.threadId, turn.turnId);
  assert.equal(fakeProvider.received.length, handed + 1, "nothing was sent until turn/start");
  await c.close();
  await daemon.stop("test");
});

test("a thread has one attachment folder for its whole life, and parts carry the file's name", async () => {
  const home = tmpHome();
  const daemon = await startDaemon(home);
  const c = await client(daemon);
  const upload = async (name: string, body: string) => {
    const data = Buffer.from(body);
    const b: any = await c.request("upload/begin", { name, size: data.length, sha256: createHash("sha256").update(data).digest("hex") });
    if (!b.exists) {
      c.sendData(b.stream, data);
      await c.request("upload/end", { uploadId: b.uploadId });
    }
    return b.uploadId as string;
  };
  const u1 = await upload("first.txt", "one");
  const t: any = await startThread(c, "x", { input: [{ type: "text", text: "/fake say read it" }, { type: "file", uploadId: u1 }] });
  await c.request("thread/subscribe", { threadId: t.threadId, epoch: 1, afterSeq: 0 });
  await waitTurnDone(c, t.threadId, t.turnId);
  const u2 = await upload("second.txt", "two");
  const t2: any = await c.request("turn/start", { opId: opId(), threadId: t.threadId, input: [{ type: "text", text: "/fake say and this" }, { type: "file", uploadId: u2 }] });
  await waitTurnDone(c, t.threadId, t2.turnId);
  const files = fakeOf(daemon).received.filter((r) => r.input.files.length > 0).map((r) => r.input.files[0]!.path);
  assert.equal(files.length, 2);
  assert.equal(path.dirname(files[0]!), path.dirname(files[1]!), "the first message's attachments and later ones share one folder");
  const user = (await readAll(c, t.threadId)).flatMap((p) => p.items).filter((i) => i.kind === "userMessage") as any[];
  const names = user.flatMap((u) => u.parts).filter((p: any) => p.type === "file").map((p: any) => p.name).sort();
  assert.deepEqual(names, ["first.txt", "second.txt"]);
  // Deleting the thread removes that one folder.
  await c.close();
  await daemon.stop("test");
});
