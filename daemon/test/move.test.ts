// Moving a thread to another project or worktree (D139): a placement yaacd keeps on the
// target, and — only under the provider's `threadMoveWorkingDirectory` — the folder the agent
// works in from its next turn, with a short fixed note at the start of that turn. Against the
// fake provider (with and without the capability), and the Codex provider against a fake
// app-server.

import { test } from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { FAKE, RpcFailure, client, fakeOf, opId, readAll, startDaemon, startThread, text, tmpHome, waitTurnDone, type TestClient } from "./helpers.js";
import { CodexProvider } from "../src/providers/codex/index.js";
import type { Exec } from "../src/providers/codex/backend.js";
import type { ProviderInput } from "../src/providers/types.js";
import { FakeCodex } from "./fixtures/codex/fake-codex.js";
import type { Item } from "../src/protocol/generated/types.js";

type D = Awaited<ReturnType<typeof startDaemon>>;

function folder(home: string, name: string): string {
  const d = path.join(home, "work", name);
  fs.mkdirSync(d, { recursive: true });
  return fs.realpathSync(d);
}

async function project(c: TestClient, dir: string, name: string): Promise<string> {
  const r: any = await c.request("project/create", { opId: opId(), folder: dir, name });
  return r.project.id;
}

async function items(c: TestClient, threadId: string): Promise<Item[]> {
  // Pages come newest first; oldest first here.
  return (await readAll(c, threadId)).flatMap((p) => p.items).reverse();
}

function notices(all: Item[]): string[] {
  return all.filter((i) => i.kind === "notice" && (i as any).noticeKind === "moved").map((i) => (i as any).text as string);
}

const code = (c: string) => (e: unknown) => e instanceof RpcFailure && e.code === c;

async function addFake(c: TestClient, name: string, env: Record<string, string>): Promise<string> {
  const r: any = await c.request("providerInstance/add", { opId: opId(), kind: "fake", name, enabled: true, backend: "auto", env: { FAKE_SEED: "0", ...env } });
  await c.request("provider/refresh", { instance: r.instance.id });
  return r.instance.id;
}

const summary = (d: D, id: string) => d.core.summaryOf(d.core.thread(id)!);

test("with threadMoveWorkingDirectory: the placement and the folder move, the next turn is told, a replay is the same, a restart keeps it", async () => {
  const home = tmpHome();
  let daemon = await startDaemon(home);
  let c = await client(daemon);
  const [a, b] = [folder(home, "api"), folder(home, "web")];
  const [pa, pb] = [await project(c, a, "api"), await project(c, b, "web")];
  // Both capabilities are offered: yaacd's own placement, and the fake harness's folder change.
  const list: any = await c.request("provider/list");
  const caps: string[] = list.providers.find((p: any) => p.instance === FAKE).capabilities;
  assert.ok(caps.includes("threadMove") && caps.includes("threadMoveWorkingDirectory"), caps.join(","));

  const { threadId, turnId } = await startThread(c, "/fake say hi", { cwd: a });
  await c.request("thread/subscribe", { threadId, epoch: 1, afterSeq: 0 });
  await waitTurnDone(c, threadId, turnId);
  assert.equal(summary(daemon, threadId).project?.id, pa);

  const op = opId();
  const moved: any = await c.request("thread/move", { opId: op, threadId, project: pb });
  assert.equal(moved.workingDirectory, b);
  assert.equal(moved.thread.project.id, pb);
  assert.equal(moved.thread.placement.project, pb);
  assert.equal(moved.thread.cwd, b);
  // The same operation id again: the recorded outcome, nothing done twice.
  const again: any = await c.request("thread/move", { opId: op, threadId, project: pb });
  assert.deepEqual(again, moved);
  assert.deepEqual(notices(await items(c, threadId)), [`Moved to web · working directory now ${b}`]);

  // The next turn runs in the new folder and begins with the fixed note; the transcript
  // shows the message as the user wrote it.
  const t2: any = await c.request("turn/start", { opId: opId(), threadId, input: text("/fake say where") });
  const sent = fakeOf(daemon).received.at(-1)!;
  assert.equal(sent.cwd, b);
  assert.ok(sent.input.text.startsWith(`[YAAC] Your working directory is now ${b}. It was ${a}; paths earlier in this conversation refer to that folder.\n\n---\n\n`), sent.input.text);
  assert.ok(sent.input.text.endsWith("/fake say where"));
  await waitTurnDone(c, threadId, t2.turnId);
  const user = (await items(c, threadId)).filter((i) => i.kind === "userMessage").at(-1) as any;
  assert.equal(user.parts[0].text, "/fake say where", "the note is the harness's only");
  // The turn after: the folder still, no note.
  const t3: any = await c.request("turn/start", { opId: opId(), threadId, input: text("/fake say again") });
  assert.equal(fakeOf(daemon).received.at(-1)!.cwd, b);
  assert.ok(!fakeOf(daemon).received.at(-1)!.input.text.includes("[YAAC]"));
  await waitTurnDone(c, threadId, t3.turnId);
  await c.close();
  await daemon.stop("test");

  // After a restart: the placement, the folder, and the replayed answer are all still there.
  daemon = await startDaemon(home);
  c = await client(daemon);
  assert.equal(summary(daemon, threadId).placement?.project, pb);
  assert.equal(summary(daemon, threadId).project?.id, pb);
  assert.equal(summary(daemon, threadId).cwd, b);
  const replay: any = await c.request("thread/move", { opId: op, threadId, project: pb });
  assert.equal(replay.workingDirectory, b);
  await c.request("thread/subscribe", { threadId, epoch: daemon.core.position(threadId).epoch, afterSeq: 0 });
  const t4: any = await c.request("turn/start", { opId: opId(), threadId, input: text("/fake say after restart") });
  assert.equal(fakeOf(daemon).received.at(-1)!.cwd, b);
  await waitTurnDone(c, threadId, t4.turnId);
  await c.close();
  await daemon.stop("test");
});

test("without the capability: only where YAAC lists it changes; the agent keeps its folder and is told nothing", async () => {
  const home = tmpHome();
  const daemon = await startDaemon(home);
  const c = await client(daemon);
  const [a, b] = [folder(home, "api"), folder(home, "web")];
  await project(c, a, "api");
  const pb = await project(c, b, "web");
  const instance = await addFake(c, "fixed", { FAKE_MOVE_CWD: "0" });
  const list: any = await c.request("provider/list");
  const caps: string[] = list.providers.find((p: any) => p.instance === instance).capabilities;
  assert.ok(caps.includes("threadMove") && !caps.includes("threadMoveWorkingDirectory"), caps.join(","));

  const { threadId, turnId } = await startThread(c, "/fake say hi", { cwd: a, instance });
  await c.request("thread/subscribe", { threadId, epoch: 1, afterSeq: 0 });
  await waitTurnDone(c, threadId, turnId);
  const moved: any = await c.request("thread/move", { opId: opId(), threadId, project: pb });
  assert.equal(moved.workingDirectory, undefined);
  assert.equal(moved.thread.project.id, pb, "listed under web");
  assert.equal(moved.thread.cwd, a, "still working in api");
  assert.deepEqual(notices(await items(c, threadId)), ["Moved to web in YAAC · working directory unchanged"]);
  const t2: any = await c.request("turn/start", { opId: opId(), threadId, input: text("/fake say where") });
  const sent = fakeOf(daemon, instance).received.at(-1)!;
  assert.equal(sent.cwd, undefined, "no folder handed over");
  assert.equal(sent.input.text, "/fake say where", "no note");
  await waitTurnDone(c, threadId, t2.turnId);

  // No project: the placement is forgotten and the folder rule applies again.
  const back: any = await c.request("thread/move", { opId: opId(), threadId });
  assert.equal(back.thread.placement, undefined);
  assert.equal(back.thread.project.displayName, "api");
  assert.equal(notices(await items(c, threadId)).at(-1), "Moved back to api in YAAC · working directory unchanged");

  // Deleting a project a thread was moved into puts it back where its folder says.
  await c.request("thread/move", { opId: opId(), threadId, project: pb });
  await c.request("project/delete", { opId: opId(), projectId: pb });
  assert.equal(summary(daemon, threadId).placement, undefined);
  assert.equal(summary(daemon, threadId).project?.displayName, "api");
  await c.close();
  await daemon.stop("test");
});

test("a running thread moves from its next turn; archived, unknown project, and in use elsewhere are refused", async () => {
  const home = tmpHome();
  const daemon = await startDaemon(home);
  const c = await client(daemon);
  const [a, b] = [folder(home, "api"), folder(home, "web")];
  await project(c, a, "api");
  const pb = await project(c, b, "web");
  const { threadId, turnId } = await startThread(c, "/fake stall", { cwd: a });
  await c.request("thread/subscribe", { threadId, epoch: 1, afterSeq: 0 });
  const moved: any = await c.request("thread/move", { opId: opId(), threadId, project: pb });
  assert.equal(moved.workingDirectory, b);
  assert.match(notices(await items(c, threadId))[0]!, /from its next turn/);
  await c.request("turn/interrupt", { threadId, turnId });
  await waitTurnDone(c, threadId, turnId);
  const t2: any = await c.request("turn/start", { opId: opId(), threadId, input: text("/fake say moved") });
  assert.equal(fakeOf(daemon).received.at(-1)!.cwd, b);
  await waitTurnDone(c, threadId, t2.turnId);

  await assert.rejects(c.request("thread/move", { opId: opId(), threadId, project: "pj_nope" }), code("invalid_params"));
  await assert.rejects(c.request("thread/move", { opId: opId(), threadId, worktree: b }), code("invalid_params"), "a worktree needs a project");
  await assert.rejects(c.request("thread/move", { opId: opId(), threadId, project: pb, worktree: b }), code("invalid_params"), "web is not in git");

  // In use elsewhere (a terminal holds it): read-only, refused.
  const t3: any = await c.request("turn/start", { opId: opId(), threadId, input: text("/fake in-use a-terminal") });
  await waitTurnDone(c, threadId, t3.turnId);
  await assert.rejects(c.request("thread/move", { opId: opId(), threadId, project: pb }), code("thread_in_use"));

  const other = await startThread(c, "/fake say hi", { cwd: a });
  await c.request("thread/subscribe", { threadId: other.threadId, epoch: 1, afterSeq: 0 });
  await waitTurnDone(c, other.threadId, other.turnId);
  await c.request("thread/archive", { opId: opId(), threadId: other.threadId });
  await assert.rejects(c.request("thread/move", { opId: opId(), threadId: other.threadId, project: pb }), code("invalid_params"));
  await c.close();
  await daemon.stop("test");
});

test("a worktree of the project's repository: the agent moves to the same place in that checkout", async () => {
  const home = tmpHome();
  const daemon = await startDaemon(home);
  const c = await client(daemon);
  const repo = folder(home, "mono");
  const git = (cwd: string, ...args: string[]) => execFileSync("git", ["-C", cwd, "-c", "user.name=t", "-c", "user.email=t@example.invalid", ...args], { stdio: "pipe" });
  git(repo, "init", "-q", "-b", "main");
  fs.mkdirSync(path.join(repo, "svc"));
  fs.writeFileSync(path.join(repo, "svc", "x.txt"), "x");
  git(repo, "add", ".");
  git(repo, "commit", "-q", "-m", "init");
  const wt = path.join(home, "work", "mono-hotfix");
  git(repo, "worktree", "add", "-q", "-b", "hotfix", wt);
  const svc = await project(c, path.join(repo, "svc"), "Service");
  const other = folder(home, "elsewhere");
  const { threadId, turnId } = await startThread(c, "/fake say hi", { cwd: other });
  await c.request("thread/subscribe", { threadId, epoch: 1, afterSeq: 0 });
  await waitTurnDone(c, threadId, turnId);
  const wtReal = fs.realpathSync(wt);
  const moved: any = await c.request("thread/move", { opId: opId(), threadId, project: svc, worktree: wtReal });
  assert.equal(moved.workingDirectory, path.join(wtReal, "svc"));
  assert.equal(moved.thread.placement.worktree, wtReal);
  assert.equal(moved.thread.placement.worktreeName, "mono-hotfix");
  assert.equal(moved.thread.worktree.path, wtReal);
  assert.equal(notices(await items(c, threadId))[0], `Moved to Service · mono-hotfix · working directory now ${path.join(wtReal, "svc")}`);
  // A folder that is not a checkout of that repository is refused.
  await assert.rejects(c.request("thread/move", { opId: opId(), threadId, project: svc, worktree: other }), code("invalid_params"));
  // Without a worktree: the project's own checkout, named by its branch.
  const main: any = await c.request("thread/move", { opId: opId(), threadId, project: svc });
  assert.equal(main.workingDirectory, path.join(fs.realpathSync(repo), "svc"));
  assert.equal(main.thread.placement.worktreeName, "main");
  await c.close();
  await daemon.stop("test");
});

// ── Codex: `cwd` on turn/start and thread/resume ──────────────────────────────────────────

const failExec: Exec = async (file, args) => {
  throw new Error(`unexpected exec: ${file} ${args.join(" ")}`);
};

function input(t: string): ProviderInput {
  return { parts: [{ type: "text", text: t }], text: t, images: [], files: [], origin: "user" };
}

test("codex: the moved folder goes on turn/start and on the resume before it; the capability is reported", async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "yaac-cx-move-"));
  const fake = await FakeCodex.start(path.join(dir, "s.sock"));
  const provider = new CodexProvider(
    { config: { id: "pi_codex_move", kind: "codex", enabled: true, backend: "auto" }, stateDir: path.join(dir, "state"), home: dir, env: {}, log: () => {} },
    { socket: fake.socket, exec: failExec, reconnectMinMs: 20, reconnectMaxMs: 50 },
  );
  try {
    fake.handlers.set("account/read", () => ({ account: { type: "apiKey" } }));
    fake.handlers.set("model/list", () => ({ data: [{ id: "m", model: "m", displayName: "M", isDefault: true }], nextCursor: null }));
    const probe = await provider.probe();
    assert.ok(probe.capabilities.includes("threadMoveWorkingDirectory"));
    const T = "01a1fa3e-0061-7e63-844f-8fa25836991c";
    fake.handlers.set("thread/read", (p) => ({ thread: { id: p.threadId, status: { type: "idle" }, cwd: "/old" } }));
    fake.handlers.set("thread/resume", (p) => ({ thread: { id: p.threadId, status: { type: "idle" }, cwd: p.cwd ?? "/old" }, model: "m" }));
    let n = 0;
    fake.handlers.set("turn/start", () => ({ turn: { id: `t${++n}`, status: "inProgress" } }));
    assert.ok(((await provider.effective(T)).capabilities ?? []).includes("threadMoveWorkingDirectory"));
    // A thread this connection has not subscribed: resumed in the new folder, then the turn.
    await provider.startTurn(T, input("go"), { cwd: "/new" });
    assert.equal(fake.requests("thread/resume")[0]!.params.cwd, "/new");
    assert.equal(fake.requests("turn/start")[0]!.params.cwd, "/new");
    fake.push({ method: "turn/completed", params: { threadId: T, turn: { id: "t1", status: "completed", items: [] } } });
    await new Promise((r) => setTimeout(r, 50));
    // A thread never moved: nothing about the folder is sent.
    const U = "01a1fa3e-0061-7e63-844f-8fa25836991d";
    await provider.startTurn(U, input("plain"), {});
    assert.equal(fake.requests("thread/resume")[1]!.params.cwd, undefined);
    assert.equal(fake.requests("turn/start")[1]!.params.cwd, undefined);
  } finally {
    await provider.dispose();
    await fake.close();
    fs.rmSync(dir, { recursive: true, force: true });
  }
});
