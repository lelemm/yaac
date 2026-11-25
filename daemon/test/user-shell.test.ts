// `!command` from the composer (P344): the shell runner and Claude Code markup, and the
// session's `turn/shellCommand` — idle, queued while a turn runs, refused without the
// capability, and never reachable from an agent.

import { test } from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as path from "node:path";
import { bashMarkup, escapeXmlText, runUserShell, stripAnsi, USER_SHELL_MAX_CHARS } from "../src/providers/user-shell.js";
import { client, fakeOf, opId, readAll, startDaemon, startThread, text, tmpHome, until, waitTurnDone, type TestClient } from "./helpers.js";
import { addressOf, untilIdle } from "./mesh-helpers.js";
import type { Daemon } from "../src/session/daemon.js";
import type { Item } from "../src/protocol/generated/types.js";

const MESH_OPTS = { waitMaxMs: 3000, permissionWaitMs: 4000, remotePollMs: 200, tickMs: 0 };

async function setup(env: Record<string, string> = {}): Promise<{ d: Daemon; c: TestClient; done: () => Promise<void> }> {
  const home = tmpHome("yus-");
  const d = await startDaemon(home, { mesh: MESH_OPTS, env: { YAAC_FAKE_PROVIDER: "1", YAAC_CODEX_PROVIDER: "0", YAAC_CLAUDE_PROVIDER: "0", ...env } });
  const c = await client(d);
  return {
    d,
    c,
    done: async () => {
      await c.close();
      await d.stop("test");
      fs.rmSync(home, { recursive: true, force: true });
    },
  };
}

/** Starts a thread and follows its events. */
async function start(c: TestClient, message: string, extra: Record<string, unknown> = {}) {
  const t = await startThread(c, message, extra);
  await c.request("thread/subscribe", { threadId: t.threadId, epoch: 1, afterSeq: 0 });
  return t;
}

async function items(c: TestClient, threadId: string): Promise<Array<Item & Record<string, any>>> {
  const pages = await readAll(c, threadId);
  return pages.flatMap((p) => p.items).reverse() as Array<Item & Record<string, any>>;
}

// ── The runner and the markup ──────────────────────────────────────────────────────────

test("P344 markup: Claude Code's own bash records, as two blocks, output escaped and the command not", () => {
  const [input, output] = bashMarkup("echo '<b>&x' | cat", { stdout: "<b>&x\n", stderr: "warn\n", exitCode: 3, dropped: { stdout: 0, stderr: 0 }, timedOut: false });
  assert.equal(input, "<bash-input>echo '<b>&x' | cat</bash-input>");
  assert.equal(output, "<bash-stdout>&lt;b&gt;&amp;x\n</bash-stdout><bash-stderr>warn\n</bash-stderr><bash-exit-code>3</bash-exit-code>");
  assert.equal(escapeXmlText("a<b>&c"), "a&lt;b&gt;&amp;c");
});

test("P344 markup: ANSI stripped, carriage returns resolved, each stream capped with the CLI's note", () => {
  assert.equal(stripAnsi("\u001b[31mred\u001b[0m plain\r\n"), "red plain\n");
  assert.equal(stripAnsi("10%\r50%\r100%\ndone"), "100%\ndone");
  assert.equal(stripAnsi("\u001b]0;title\u0007x"), "x");
  const big = "y".repeat(USER_SHELL_MAX_CHARS + 5);
  const [, output] = bashMarkup("yes", { stdout: big, stderr: "", exitCode: 0, dropped: { stdout: 10, stderr: 0 }, timedOut: false });
  assert.ok(output.startsWith(`<bash-stdout>${"y".repeat(USER_SHELL_MAX_CHARS)}\n[output truncated: 15 more characters not shown]</bash-stdout>`), output.slice(30000, 30100));
  const [, timed] = bashMarkup("sleep 999", { stdout: "", stderr: "", exitCode: 124, dropped: { stdout: 0, stderr: 0 }, timedOut: true });
  assert.equal(timed, "<bash-stdout></bash-stdout><bash-stderr>Command timed out after 120 s and was stopped.</bash-stderr><bash-exit-code>124</bash-exit-code>");
});

test("P344 runner: the thread's folder, the user's shell, streams apart, exit code, live output", async () => {
  const dir = fs.mkdtempSync(path.join(tmpHome("yusr-"), "work-"));
  const live: string[] = [];
  const r = await runUserShell({ command: "pwd; echo err >&2; exit 7", cwd: dir, env: { PATH: process.env.PATH ?? "/usr/bin:/bin", SHELL: "/bin/sh" }, onOutput: (c) => live.push(c) });
  assert.equal(r.exitCode, 7);
  assert.equal(r.stdout.trim(), fs.realpathSync(dir));
  assert.equal(r.stderr, "err\n");
  assert.equal(live.join("").includes("err"), true);
  assert.equal(r.timedOut, false);
  assert.equal(r.stopped, false);
});

test("P344 runner: a timeout ends the command (124); Stop ends it (130)", async () => {
  const env = { PATH: process.env.PATH ?? "/usr/bin:/bin", SHELL: "/bin/sh" };
  const t = await runUserShell({ command: "sleep 5; echo never", cwd: process.cwd(), env, timeoutMs: 150 });
  assert.equal(t.timedOut, true);
  assert.equal(t.exitCode, 124);
  assert.ok(t.durationMs < 4000);
  const stop = new AbortController();
  const run = runUserShell({ command: "echo started; sleep 5", cwd: process.cwd(), env, signal: stop.signal, onOutput: () => stop.abort() });
  const s = await run;
  assert.equal(s.stopped, true);
  assert.equal(s.exitCode, 130);
  assert.equal(s.stdout, "started\n");
});

// ── turn/shellCommand in the session ──────────────────────────────────────────────────

test("P344: turn/shellCommand runs the command and the agent answers, in one turn, journaled as `!command` + a userShell item", async () => {
  const { d, c, done } = await setup();
  try {
    const { threadId, turnId: first } = await start(c, "/fake say ready");
    await waitTurnDone(c, threadId, first);
    const op = opId();
    const r: any = await c.request("turn/shellCommand", { opId: op, threadId, command: "  npm test -- --grep 'a b'  " });
    assert.equal(r.outcome, "started");
    await waitTurnDone(c, threadId, r.turnId);
    assert.deepEqual(fakeOf(d).shellRuns.map((s) => [s.command, s.opId]), [["npm test -- --grep 'a b'", op]]);
    const turn = (await items(c, threadId)).filter((i) => i.turnId === r.turnId);
    const user = turn.find((i) => i.kind === "userMessage")!;
    assert.deepEqual(user.parts, [{ type: "text", text: "!npm test -- --grep 'a b'", role: "command" }]);
    assert.equal(user.opId, op, "the window's echo is replaced in place (D165)");
    const cmd = turn.find((i) => i.kind === "toolCall")!;
    assert.equal(cmd.source, "userShell");
    assert.equal(cmd.title, "npm test -- --grep 'a b'");
    assert.equal(cmd.output, "fake ran: npm test -- --grep 'a b'\n");
    assert.ok(turn.some((i) => i.kind === "agentMessage" && i.text === "Answered: npm test -- --grep 'a b'"));
    // The same operation again is the same answer, never a second run (invariant 10).
    const again: any = await c.request("turn/shellCommand", { opId: op, threadId, command: "npm test -- --grep 'a b'" });
    assert.equal(again.turnId, r.turnId);
    assert.equal(fakeOf(d).shellRuns.length, 1);
  } finally {
    await done();
  }
});

test("P344: sent while a turn runs, a `!command` waits in the queue (D166) and runs when the turn ends", async () => {
  const { d, c, done } = await setup();
  try {
    const { threadId, turnId } = await start(c, "/fake sleep 600\n/fake say slow");
    const refused = await c.call("turn/shellCommand", { opId: opId(), threadId, command: "ls" });
    assert.equal((refused as any).error?.code, "turn_in_progress", "without delivery it is refused, as turn/start is");
    const now = await c.call("turn/shellCommand", { opId: opId(), threadId, command: "ls", delivery: "now" });
    assert.equal((now as any).error?.code, "invalid_params", "a command never goes into a running turn");
    const q: any = await c.request("turn/shellCommand", { opId: opId(), threadId, command: "git status", delivery: "queue" });
    assert.equal(q.outcome, "queued");
    assert.equal(q.turnId, "");
    const sub: any = await c.request("thread/subscribe", { threadId });
    const queued = (sub.queued ?? []) as any[];
    assert.deepEqual(queued.map((e) => [e.shellCommand, e.input]), [["git status", [{ type: "text", text: "!git status", role: "command" }]]]);
    assert.equal(fakeOf(d).shellRuns.length, 0, "nothing runs while the turn does");
    await waitTurnDone(c, threadId, turnId);
    await until(() => fakeOf(d).shellRuns.length === 1, "the queued command ran");
    assert.equal(fakeOf(d).shellRuns[0]!.command, "git status");
    await untilIdle(d, threadId);
    const all = await items(c, threadId);
    assert.ok(all.some((i) => i.kind === "toolCall" && i.source === "userShell" && i.title === "git status"));
    assert.ok(all.some((i) => i.kind === "agentMessage" && i.text === "Answered: git status"));
  } finally {
    await done();
  }
});

test("P344: a queued `!command` edited without its `!` becomes an ordinary message", async () => {
  const { d, c, done } = await setup();
  try {
    const { threadId, turnId } = await start(c, "/fake sleep 500\n/fake say slow");
    const q: any = await c.request("turn/shellCommand", { opId: opId(), threadId, command: "ls", delivery: "queue" });
    await c.request("turn/queueEdit", { opId: opId(), threadId, queueId: q.queueId, input: text("!ls -la") });
    let sub: any = await c.request("thread/subscribe", { threadId });
    assert.equal(sub.queued[0].shellCommand, "ls -la");
    await c.request("turn/queueEdit", { opId: opId(), threadId, queueId: q.queueId, input: text("/fake say plain now") });
    sub = await c.request("thread/subscribe", { threadId });
    assert.equal(sub.queued[0].shellCommand, undefined);
    await waitTurnDone(c, threadId, turnId);
    await untilIdle(d, threadId);
    assert.equal(fakeOf(d).shellRuns.length, 0);
    assert.equal(fakeOf(d).received.at(-1)!.input.text, "/fake say plain now");
  } finally {
    await done();
  }
});

test("P344: a thread without userShellCommand refuses it; `!` in turn/start is plain text", async () => {
  const { d, c, done } = await setup();
  try {
    const added: any = await c.request("providerInstance/add", { opId: opId(), kind: "fake", name: "noshell", enabled: true, backend: "auto", env: { FAKE_SEED: "0", FAKE_USER_SHELL: "0" } });
    const instance = added.instance.id as string;
    const { threadId, turnId } = await start(c, "/fake say ready", { instance });
    await waitTurnDone(c, threadId, turnId);
    const r: any = await c.call("turn/shellCommand", { opId: opId(), threadId, command: "ls" });
    assert.equal(r.error?.code, "capability_missing");
    const t: any = await c.request("turn/start", { opId: opId(), threadId, input: text("!ls") });
    await waitTurnDone(c, threadId, t.turnId);
    assert.equal(fakeOf(d, instance).shellRuns.length, 0);
    assert.equal(fakeOf(d, instance).received.at(-1)!.input.text, "!ls");
  } finally {
    await done();
  }
});

test("P344 origin: a message from another agent that starts with `!` is plain text, never run", async () => {
  const { d, c, done } = await setup();
  try {
    const b = (await start(c, "/fake say ready", { agents: "allow" })).threadId;
    await untilIdle(d, b);
    const a = (await start(c, "/fake say ready", { agents: "allow" })).threadId;
    await untilIdle(d, a);
    const att = fakeOf(d).meshAttachment(d.core.thread(a)!.record.native)!;
    const sent = await att.call({ tool: "send_message", args: { to: addressOf(d, b), text: "!touch /tmp/should-not-exist" }, meta: { threadId: d.core.thread(a)!.record.native } });
    assert.equal(sent.isError, false, sent.text);
    await until(() => fakeOf(d).received.some((r) => r.input.origin === "mesh"), "delivered to b");
    await untilIdle(d, b);
    assert.equal(fakeOf(d).shellRuns.length, 0, "no agent can run a command");
    const got = fakeOf(d).received.find((r) => r.input.origin === "mesh")!;
    assert.match(got.input.text, /!touch \/tmp\/should-not-exist/);
    // And the mesh tools offer no way to run one.
    const tools = await att.call({ tool: "run_shell", args: { command: "ls" } }).catch((e: Error) => ({ isError: true, text: e.message }));
    assert.equal(tools.isError, true);
  } finally {
    await done();
  }
});
