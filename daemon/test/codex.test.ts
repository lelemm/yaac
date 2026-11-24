// The Codex provider against a fake app-server that replays recorded traffic
// (test/fixtures/codex/*.json). No real Codex is touched: every instance here has an explicit
// socket (connect-only) or a fake `codex` program.

import { test } from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { CodexProvider, type CodexOptions } from "../src/providers/codex/index.js";
import { ThreadInUseError } from "../src/providers/codex/provider.js";
import { acceptedKinds, serverVersion } from "../src/providers/codex/provider.js";
import { mapItem } from "../src/providers/codex/mapping.js";
import type { Exec } from "../src/providers/codex/backend.js";
import { InputOutcomeUnknown, type ProviderEvent, type ProviderInput } from "../src/providers/types.js";
import { FakeCodex, fixture, rpcError, type Step } from "./fixtures/codex/fake-codex.js";

function tmp(): string {
  return fs.mkdtempSync(path.join(os.tmpdir(), "yaac-cx-"));
}

function input(text: string): ProviderInput {
  return { parts: [{ type: "text", text }], text, images: [], files: [], origin: "user" };
}

interface Rig {
  dir: string;
  fake: FakeCodex;
  provider: CodexProvider;
  events: ProviderEvent[];
  logs: string[];
  done(): Promise<void>;
}

async function rig(opts: Partial<CodexOptions> = {}, init?: unknown): Promise<Rig> {
  const dir = tmp();
  const fake = await FakeCodex.start(path.join(dir, "s.sock"), init);
  const logs: string[] = [];
  const provider = new CodexProvider(
    {
      config: { id: "pi_codex_test", kind: "codex", enabled: true, backend: "auto" },
      stateDir: path.join(dir, "state"),
      home: dir,
      env: {},
      log: (l) => logs.push(l),
    },
    { socket: fake.socket, exec: failExec, reconnectMinMs: 20, reconnectMaxMs: 50, ...opts },
  );
  fake.handlers.set("account/read", () => ({ account: { type: "apiKey" }, requiresOpenaiAuth: true }));
  fake.handlers.set("model/list", () => ({ data: [], nextCursor: null }));
  const events: ProviderEvent[] = [];
  provider.events.on((e) => events.push(e));
  return {
    dir,
    fake,
    provider,
    events,
    logs,
    async done() {
      await provider.dispose();
      await fake.close();
      fs.rmSync(dir, { recursive: true, force: true });
    },
  };
}

/** A `codex` that must never be run (explicit sockets are connect-only). */
const failExec: Exec = async (file, args) => {
  throw new Error(`unexpected exec: ${file} ${args.join(" ")}`);
};

async function until(pred: () => boolean, ms = 5000): Promise<void> {
  const end = Date.now() + ms;
  while (!pred()) {
    if (Date.now() > end) throw new Error("condition not met in time");
    await new Promise((r) => setTimeout(r, 5));
  }
}

const THREAD_ID = "01a1fa3e-0061-7e63-844f-8fa25836991b";
const TURN_ID = "01a1fa3e-0086-76e0-b64c-b654bc53bcb8";

/** Minimal handlers for a thread that exists and is idle. */
function idleThread(fake: FakeCodex, id = THREAD_ID, status: unknown = { type: "idle" }): void {
  fake.handlers.set("thread/start", () => ({ thread: { id, status, cwd: "/work/fixture", source: "vscode" }, model: "model-1", reasoningEffort: null }));
  fake.handlers.set("thread/read", (p) => ({ thread: { id: p.threadId, status, cwd: "/work/fixture", source: "vscode" } }));
  fake.handlers.set("thread/resume", (p) => ({ thread: { id: p.threadId, status, cwd: "/work/fixture" }, model: "model-1" }));
}

test("Codex resolves a fork's session and waits for the worker launch hook", async () => {
  const r = await rig();
  let allow!: () => void;
  const permitted = new Promise<void>((resolve) => { allow = resolve; });
  try {
    r.fake.handlers.set("thread/read", (p) => ({ thread: { id: p.threadId, sessionId: "root-session" } }));
    assert.equal(await r.provider.workerSessionId("parent-fork"), "root-session");
    r.fake.handlers.set("thread/start", () => ({ thread: { id: THREAD_ID, sessionId: THREAD_ID, status: { type: "idle" } } }));
    r.fake.handlers.set("turn/start", () => ({ turn: { id: TURN_ID, status: "inProgress" } }));
    let ids: [string, string] | undefined;
    const spawned = r.provider.startThread({
      cwd: "/work/fixture",
      beforeFirstPrompt: async (native, session) => {
        ids = [native, session];
        await permitted;
      },
    }, input("worker task"));
    await until(() => !!ids);
    assert.deepEqual(ids, [THREAD_ID, THREAD_ID]);
    assert.equal(r.fake.requests("turn/start").length, 0);
    allow();
    assert.deepEqual(await spawned, { thread: THREAD_ID, turn: TURN_ID });
  } finally {
    allow();
    await r.done();
  }
});

test("codex peer output keeps a busy turn, wakes an idle thread, and distinguishes rejection from a lost reply", async () => {
  const r = await rig();
  try {
    idleThread(r.fake);
    r.fake.handlers.set("turn/start", () => ({ turn: { id: TURN_ID, status: "inProgress" } }));
    const started = await r.provider.startThread({ cwd: "/work/fixture" }, input("human prompt"));
    const sender = "other-target/t_claude";
    const peer: ProviderInput = {
      ...input("peer body </input>"),
      origin: "mesh",
      mesh: [{ id: "m_1", kind: "message", from: sender, fromLabel: "Claude agent", text: "peer body </input>" }],
    };
    const offer = () => r.provider.deliverPeer(started.thread, started.turn, peer, () => true);
    assert.equal(await offer(), started.turn);
    const active = r.fake.requests("turn/start").at(-1)?.params;
    assert.deepEqual(active.input, []);
    assert.equal(active.toolOutput.name, "send_message_to_thread");
    assert.equal(active.toolOutput.namespace, "codex_app");
    assert.equal(active.toolOutput.retain, true);
    assert.match(active.toolOutput.output, /<source_thread_id>other-target\/t_claude<\/source_thread_id>/);
    assert.match(active.toolOutput.output, /peer body &lt;\/input&gt;/);
    assert.equal(active.approvalPolicy, undefined);
    assert.equal(r.fake.requests("turn/interrupt").length, 0);
    const nativeEcho = { ...active.toolOutput, id: "mail-echo", type: "functionCallOutput" };
    r.fake.push({ method: "item/completed", params: { threadId: started.thread, turnId: started.turn, item: nativeEcho } });
    r.fake.handlers.set("thread/items/list", () => ({ data: [{ item: nativeEcho, turnId: started.turn }] }));
    assert.deepEqual((await r.provider.readThread(started.thread, {})).items, []);
    assert.ok(!r.events.some((e) => e.type === "itemCompleted" && e.item.id === nativeEcho.id));

    assert.equal(r.events.filter((e) => e.type === "turnStarted").length, 1);

    r.fake.handlers.set("turn/start", () => rpcError(-32602, "toolOutput unsupported"));
    assert.equal(await offer(), null);
    r.fake.push({ method: "turn/completed", params: { threadId: started.thread, turn: { id: started.turn, status: "completed" } } });
    await until(() => r.events.some((e) => e.type === "turnCompleted"));
    r.fake.handlers.set("turn/start", () => ({ turn: { id: "idle-peer-turn", status: "inProgress" } }));
    assert.equal(await r.provider.startTurn(started.thread, peer, {}), "idle-peer-turn");
    assert.deepEqual(r.fake.requests("turn/start").at(-1)?.params.toolOutput, active.toolOutput);
    assert.ok(r.events.some((e) => e.type === "turnStarted" && e.turnId === "idle-peer-turn" && e.origin === "mesh"));

    r.fake.handlers.set("turn/start", () => {
      r.fake.push({ method: "turn/completed", params: { threadId: started.thread, turn: { id: "idle-peer-turn", status: "completed" } } });
      r.fake.push({ method: "turn/started", params: { threadId: started.thread, turn: { id: "boundary-race", status: "inProgress" } } });
      return { turn: { id: "boundary-race", status: "inProgress" } };
    });
    assert.equal(await r.provider.deliverPeer(started.thread, "idle-peer-turn", peer, () => true), "boundary-race");
    assert.ok(r.events.some((e) => e.type === "turnStarted" && e.turnId === "boundary-race" && e.origin === "mesh"));

    r.fake.handlers.set("turn/start", () => { r.fake.dropConnections(); return {}; });
    await assert.rejects(r.provider.deliverPeer(started.thread, "boundary-race", peer, () => true), InputOutcomeUnknown);
    assert.equal(r.fake.requests("turn/start").length, 6);
  } finally {
    await r.done();
  }
});

test("codex: a recorded turn — reasoning, a command behind an approval, input from another client, usage", async () => {
  const r = await rig();
  try {
    const fx = fixture<{ steps: Step[] }>("turn-approval.json");
    const script = r.fake.run(fx.steps);
    const started = await r.provider.startThread({ cwd: "/work/fixture" }, input("Run the shell command `echo yaac-fixture` and then reply with just the word done."));
    assert.equal(started.thread, THREAD_ID);
    assert.equal(started.turn, TURN_ID);

    await until(() => r.events.some((e) => e.type === "requestRaised"));
    const raised = r.events.find((e) => e.type === "requestRaised") as Extract<ProviderEvent, { type: "requestRaised" }>;
    assert.equal(raised.thread, THREAD_ID);
    assert.equal(raised.turnId, TURN_ID);
    assert.equal(raised.request.requestKind, "approval");
    const req = raised.request as any;
    assert.equal(req.approvalKind, "command");
    assert.equal(req.command, "/bin/bash -lc 'echo yaac-fixture'");
    // Exactly the decisions Codex offered, in its order.
    assert.deepEqual(req.decisions.map((d: any) => d.id), ["accept", "acceptWithExecpolicyAmendment", "cancel"]);
    assert.deepEqual(req.decisions.map((d: any) => d.kind), ["allow", "allowAlways", "abort"]);
    // Not answered by the provider on its own.
    assert.equal(r.fake.replies.size, 0);
    await r.provider.respond(THREAD_ID, raised.requestId, { requestKind: "approval", decision: "accept" });

    await script;
    await until(() => r.events.some((e) => e.type === "turnCompleted"));
    assert.deepEqual(r.fake.replies.get(0), { decision: "accept" });

    const evs = r.events.filter((e) => "thread" in e && (e as { thread: string }).thread === THREAD_ID);
    // turnStarted first, with the id startThread returned.
    const firstTurnEvent = evs.findIndex((e) => e.type === "turnStarted");
    assert.ok(firstTurnEvent >= 0);
    assert.equal((evs[firstTurnEvent] as any).turnId, TURN_ID);
    assert.ok(evs.slice(0, firstTurnEvent).every((e) => e.type !== "itemStarted" && e.type !== "itemDelta"));
    assert.equal(r.events.filter((e) => e.type === "turnStarted").length, 1);

    // The input yaacd sent is not echoed; the one another client joined into the turn is.
    const users = evs.filter((e) => e.type === "itemStarted" && e.item.kind === "userMessage").map((e: any) => e.item.parts[0].text);
    assert.deepEqual(users, ["(second input: no extra work needed)"]);

    // Reasoning streamed as deltas, then completed with the full summary.
    const rsDeltas = evs.filter((e) => e.type === "itemDelta" && e.field === "reasoning").map((e: any) => e.delta).join("");
    assert.equal(rsDeltas, "Checking how to run the command.");
    const rs = evs.find((e) => e.type === "itemCompleted" && e.item.kind === "reasoning") as any;
    assert.equal(rs.item.reasoning, "Checking how to run the command.");
    assert.equal(rs.item.summary, true);

    // Agent text streamed and completed.
    const msgDeltas = evs.filter((e) => e.type === "itemDelta" && e.field === "text").map((e: any) => e.delta).join("");
    assert.ok(msgDeltas.startsWith("Running the command."));
    const msgs = evs.filter((e) => e.type === "itemCompleted" && e.item.kind === "agentMessage").map((e: any) => e.item.text);
    assert.deepEqual(msgs, ["Running the command.", "done"]);

    // The command: started in progress, output streamed, completed with exit code and output.
    const cmdStart = evs.find((e) => e.type === "itemStarted" && e.item.kind === "toolCall") as any;
    assert.equal(cmdStart.item.tool, "command");
    assert.equal(cmdStart.item.status, "inProgress");
    const out = evs.filter((e) => e.type === "itemDelta" && e.field === "output").map((e: any) => e.delta).join("");
    assert.equal(out, "yaac-fixture\n");
    const cmdDone = evs.find((e) => e.type === "itemCompleted" && e.item.kind === "toolCall") as any;
    assert.equal(cmdDone.item.status, "completed");
    assert.equal(cmdDone.item.exitCode, 0);
    assert.equal(cmdDone.item.output, "yaac-fixture\n");
    assert.equal(cmdDone.item.cwd, "/work/fixture");

    // The approval is confirmed by Codex (serverRequest/resolved), as answered by this client.
    const resolved = evs.find((e) => e.type === "requestResolved") as any;
    assert.equal(resolved.requestId, raised.requestId);
    assert.equal(resolved.by, "client");

    // Usage and the end of the turn.
    const usage = evs.filter((e) => e.type === "usageUpdated") as any[];
    assert.ok(usage.length > 0);
    assert.equal(typeof usage.at(-1).tokens.total, "number");
    assert.equal(typeof usage.at(-1).context.window, "number");
    const done = evs.find((e) => e.type === "turnCompleted") as any;
    assert.equal(done.turnId, TURN_ID);
    assert.equal(done.status, "completed");
    assert.equal(done.usage.total, usage.at(-1).tokens.total);

    // What yaacd sent: thread/start with the cwd; turn/start with a client message id, no model.
    const ts = r.fake.requests("thread/start")[0]!;
    assert.equal(ts.params.cwd, "/work/fixture");
    const tu = r.fake.requests("turn/start")[0]!;
    assert.equal(tu.params.threadId, THREAD_ID);
    assert.equal(typeof tu.params.clientUserMessageId, "string");
    assert.equal(tu.params.model, undefined);
    assert.equal(tu.params.input[0].type, "text");
    // The experimental API is requested (for availableDecisions and requestUserInput).
    assert.equal(r.fake.requests("initialize")[0]!.params.capabilities.experimentalApi, true);
    assert.ok(r.fake.received.some((m) => m.method === "initialized" && m.id === undefined));
    r.fake.handlers.set("thread/read", (p) => ({ thread: { id: p.threadId, status: { type: "idle" } } }));
    assert.equal(await r.provider.runningTurn(THREAD_ID), null);
  } finally {
    await r.done();
  }
});

test("codex: requestUserInput becomes a question (options, isOther, isSecret) and the answer is converted", async () => {
  const r = await rig();
  try {
    const fx = fixture<any>("user-input.json");
    idleThread(r.fake);
    r.fake.handlers.set("turn/start", () => ({ turn: { id: fx.turn, status: "inProgress", items: [] } }));
    await r.provider.startThread({ cwd: "/work/fixture" }, input("go"));
    r.fake.push(fx.request);
    await until(() => r.events.some((e) => e.type === "requestRaised"));
    const raised = r.events.find((e) => e.type === "requestRaised") as any;
    assert.equal(raised.request.requestKind, "question");
    assert.equal(raised.blocking, true);
    assert.equal(raised.turnId, fx.turn);
    const [q1, q2] = raised.request.questions;
    assert.equal(q1.id, "approach");
    assert.equal(q1.header, "Approach");
    assert.equal(q1.text, "Which fix do you want?");
    assert.deepEqual(q1.options.map((o: any) => o.label), ["Patch the decoder", "Rewrite the framing"]);
    assert.equal(q1.allowOther, true);
    assert.equal(q1.secret, false);
    assert.equal(q2.id, "token");
    assert.equal(q2.secret, true);
    assert.equal(q2.allowOther, true, "a question with no options takes free text");
    await r.provider.respond(THREAD_ID, raised.requestId, {
      requestKind: "question",
      answers: { approach: { selected: ["Patch the decoder"] }, token: { other: "s3cret-value" } },
    });
    await until(() => r.fake.replies.has(fx.request.id));
    assert.deepEqual(r.fake.replies.get(fx.request.id), fx.expectedResponse);
    assert.ok(!r.events.some((e) => e.type === "requestResolved"), "confirmed only once Codex says so");
    r.fake.push(fx.resolved);
    await until(() => r.events.some((e) => e.type === "requestResolved"));
    const res = r.events.find((e) => e.type === "requestResolved") as any;
    assert.equal(res.by, "client");
  } finally {
    await r.done();
  }
});

test("codex: a request answered by another client resolves elsewhere; an unanswered one expires with its turn", async () => {
  const r = await rig();
  try {
    idleThread(r.fake);
    r.fake.handlers.set("turn/start", () => ({ turn: { id: TURN_ID, status: "inProgress", items: [] } }));
    await r.provider.startThread({ cwd: "/work/fixture" }, input("go"));
    const p = (itemId: string) => ({ threadId: THREAD_ID, turnId: TURN_ID, itemId, startedAtMs: 0, reason: null });
    r.fake.push({ id: 7, method: "item/fileChange/requestApproval", params: { ...p("patch-1"), grantRoot: null } });
    r.fake.push({ id: 8, method: "item/permissions/requestApproval", params: { ...p("perm-1"), cwd: "/work/fixture", environmentId: null, permissions: { network: { enabled: true }, fileSystem: null } } });
    await until(() => r.events.filter((e) => e.type === "requestRaised").length === 2);
    const [file, perm] = r.events.filter((e) => e.type === "requestRaised") as any[];
    // Without availableDecisions, Codex's stable set for file changes.
    assert.deepEqual(file.request.decisions.map((d: any) => d.id), ["accept", "acceptForSession", "decline", "cancel"]);
    assert.equal(perm.request.approvalKind, "permission");
    r.fake.push({ method: "serverRequest/resolved", params: { threadId: THREAD_ID, requestId: 7 } });
    await until(() => r.events.some((e) => e.type === "requestResolved"));
    assert.equal((r.events.find((e) => e.type === "requestResolved") as any).by, "elsewhere");
    r.fake.push({ method: "turn/completed", params: { threadId: THREAD_ID, turn: { id: TURN_ID, status: "interrupted", items: [], error: null } } });
    await until(() => r.events.some((e) => e.type === "turnCompleted"));
    assert.ok(r.events.some((e) => e.type === "requestExpired" && e.requestId === perm.requestId));
    assert.equal((r.events.find((e) => e.type === "turnCompleted") as any).status, "interrupted");
    // A permission grant is sent with the requested profile and scope.
    r.fake.push({ id: 9, method: "item/permissions/requestApproval", params: { ...p("perm-2"), cwd: "/w", environmentId: null, permissions: { network: { enabled: true }, fileSystem: null } } });
    await until(() => r.events.filter((e) => e.type === "requestRaised").length === 3);
    const perm2 = r.events.filter((e) => e.type === "requestRaised").at(-1) as any;
    await r.provider.respond(THREAD_ID, perm2.requestId, { requestKind: "approval", decision: "grantSession" });
    await until(() => r.fake.replies.has(9));
    assert.deepEqual(r.fake.replies.get(9), { permissions: { network: { enabled: true } }, scope: "session" });
    // A decision Codex did not offer is refused, not guessed.
    r.fake.push({ id: 10, method: "item/fileChange/requestApproval", params: { ...p("patch-2"), grantRoot: null } });
    await until(() => r.events.filter((e) => e.type === "requestRaised").length === 4);
    const file2 = r.events.filter((e) => e.type === "requestRaised").at(-1) as any;
    await assert.rejects(r.provider.respond(THREAD_ID, file2.requestId, { requestKind: "approval", decision: "allowAlways" }));
  } finally {
    await r.done();
  }
});

test("codex: a request re-delivered after a new subscription is the same request", async () => {
  const r = await rig();
  try {
    idleThread(r.fake);
    r.fake.handlers.set("turn/start", () => ({ turn: { id: TURN_ID, status: "inProgress", items: [] } }));
    await r.provider.startThread({ cwd: "/work/fixture" }, input("go"));
    const params = { kind: "command", threadId: THREAD_ID, turnId: TURN_ID, itemId: "exec-1", startedAtMs: 0, environmentId: null, command: "ls" };
    r.fake.push({ id: 0, method: "item/commandExecution/requestApproval", params });
    r.fake.push({ id: 5, method: "item/commandExecution/requestApproval", params });
    await until(() => r.fake.received.length > 0);
    await new Promise((res) => setTimeout(res, 50));
    const raised = r.events.filter((e) => e.type === "requestRaised") as any[];
    assert.equal(raised.length, 1);
    await r.provider.respond(THREAD_ID, raised[0].requestId, { requestKind: "approval", decision: "accept" });
    await until(() => r.fake.replies.has(5));
    assert.ok(!r.fake.replies.has(0), "answered under the id Codex re-sent it with");
  } finally {
    await r.done();
  }
});

test("codex: native fork, with the mesh hook's config and developer instructions passed through", async () => {
  const r = await rig();
  try {
    const fx = fixture<any>("fork.json");
    const src = fx.fork.thread.forkedFromId as string;
    r.fake.handlers.set("thread/turns/list", () => fx.sourceTurns);
    r.fake.handlers.set("thread/fork", () => fx.fork);
    const seen: any[] = [];
    r.provider.setThreadSetupHook((req) => {
      seen.push(req);
      return { config: { "mcp_servers.yaac": { command: "/opt/yaacd", args: ["mcp"], env: { YAAC_TICKET: "t-1" } } }, developerInstructions: "You are agent A." };
    });
    const forked = await r.provider.forkThread(src, { cwd: "/work/fixture" });
    assert.equal(forked, fx.fork.thread.id);
    const req = r.fake.requests("thread/fork")[0]!;
    assert.equal(req.params.threadId, src);
    assert.equal(req.params.lastTurnId, undefined, "no running turn: fork everything");
    assert.equal(req.params.excludeTurns, true);
    assert.deepEqual(req.params.config, { "mcp_servers.yaac": { command: "/opt/yaacd", args: ["mcp"], env: { YAAC_TICKET: "t-1" } } });
    assert.equal(req.params.developerInstructions, "You are agent A.");
    assert.deepEqual(seen, [{ phase: "fork", thread: src, cwd: "/work/fixture" }]);
    r.fake.push(fx.forkStarted);
    await until(() => r.events.some((e) => e.type === "threadChanged"));
    const changed = r.events.find((e) => e.type === "threadChanged") as any;
    assert.equal(changed.info.native, forked);
    assert.equal(changed.info.forkedFrom, src);
    // The fork is held by this connection: free.
    r.fake.handlers.set("thread/read", (p) => ({ thread: { id: p.threadId, status: { type: "idle" } } }));
    assert.deepEqual(await r.provider.control(forked), { external: false });

    // Fork of a thread mid-turn: from the last completed turn.
    r.fake.handlers.set("thread/turns/list", () => ({ data: [{ id: "t-running", status: "inProgress" }, { id: "t-done", status: "completed" }], nextCursor: null }));
    await r.provider.forkThread(src, { cwd: "/work/fixture" });
    assert.equal(r.fake.requests("thread/fork")[1]!.params.lastTurnId, "t-done");
  } finally {
    await r.done();
  }
});

test("codex: the mesh hook reaches thread/start and thread/resume", async () => {
  const r = await rig();
  try {
    idleThread(r.fake, THREAD_ID, { type: "notLoaded" });
    r.fake.handlers.set("thread/start", () => ({ thread: { id: THREAD_ID, status: { type: "idle" } } }));
    r.fake.handlers.set("turn/start", () => ({ turn: { id: TURN_ID, status: "inProgress" } }));
    r.provider.setThreadSetupHook((req) => ({ config: { "mcp_servers.yaac": { command: "/x" } }, developerInstructions: `identity for ${req.phase}` }));
    await r.provider.startThread({ cwd: "/work/fixture" }, input("go"));
    const st = r.fake.requests("thread/start")[0]!.params;
    assert.deepEqual(st.config, { "mcp_servers.yaac": { command: "/x" } });
    assert.equal(st.developerInstructions, "identity for start");
    await r.provider.dispose();

    const r2 = await rig();
    try {
      idleThread(r2.fake, "other-thread", { type: "notLoaded" });
      r2.fake.handlers.set("turn/start", () => ({ turn: { id: "t2", status: "inProgress" } }));
      r2.provider.setThreadSetupHook((req) => ({ developerInstructions: `identity for ${req.phase} of ${req.thread}` }));
      await r2.provider.startTurn("other-thread", input("hi"), {});
      const rs = r2.fake.requests("thread/resume")[0]!.params;
      assert.equal(rs.developerInstructions, "identity for resume of other-thread");
      assert.equal(rs.config, undefined);
    } finally {
      await r2.done();
    }
  } finally {
    await r.done();
  }
});

test("codex: probe lists every model page, efforts and modalities as reported, and the store identity", async () => {
  const fx = fixture<any>("models.json");
  const r = await rig({}, fx.initialize);
  try {
    assert.ok(fx.pages.length > 1, "the fixture spans several pages");
    r.fake.handlers.set("account/read", () => ({ account: { type: "chatgpt", email: "user@example.com", planType: "redacted" }, requiresOpenaiAuth: true }));
    r.fake.handlers.set("model/list", (p) => {
      const i = p.cursor ? fx.pages.findIndex((_pg: any, k: number) => k > 0 && fx.pages[k - 1].nextCursor === p.cursor) : 0;
      return fx.pages[i];
    });
    const probe = await r.provider.probe();
    assert.equal(probe.status, "ready");
    assert.equal(probe.backend, "daemon");
    assert.equal(probe.harnessVersion, serverVersion(fx.initialize.userAgent));
    assert.equal(probe.store.home, fx.initialize.codexHome);
    assert.match(probe.store.key ?? "", /^codex:[0-9a-f]{16}$/);
    assert.equal(probe.store.account, "user@example.com");
    assert.equal(probe.modelsComplete, true);
    const all = fx.pages.flatMap((p: any) => p.data);
    assert.equal(probe.models.length, all.length);
    for (const m of all) {
      const got = probe.models.find((x) => x.id === m.model)!;
      assert.ok(got, `model ${m.model} is listed`);
      assert.equal(got.displayName, m.displayName);
      assert.equal(got.hidden, m.hidden);
      assert.equal(got.isDefault, m.isDefault);
      assert.deepEqual(got.efforts!.map((e) => e.id), m.supportedReasoningEfforts.map((e: any) => e.reasoningEffort));
      assert.equal(got.defaultEffort, m.defaultReasoningEffort);
      const img = got.capabilities!.find((c) => c.capability === "imageInput")!.support;
      assert.equal(img, m.inputModalities.includes("image") ? "supported" : "unsupported");
    }
    for (const req of r.fake.requests("model/list")) assert.equal(req.params.includeHidden, true);
    assert.equal(r.fake.requests("model/list").length, fx.pages.length);
    assert.ok(probe.capabilities.includes("approvals") && probe.capabilities.includes("userQuestions") && probe.capabilities.includes("threadFork"));
    assert.ok(!probe.capabilities.includes("meshTools"), "not until the MCP server is attached (WP7)");

    // A listing that fails part-way is partial — models are never declared gone after it.
    r.fake.handlers.set("model/list", (p) => (p.cursor ? rpcError(-32603, "boom") : fx.pages[0]));
    const partial = await r.provider.probe();
    assert.equal(partial.modelsComplete, false);
    assert.equal(partial.models.length, fx.pages[0].data.length);

    // Not signed in.
    r.fake.handlers.set("account/read", () => ({ account: null, requiresOpenaiAuth: true }));
    assert.equal((await r.provider.probe()).status, "needsAuth");
  } finally {
    await r.done();
  }
});

test("codex: thread listing asks for every source kind, then archived threads; origin from the source", async () => {
  const fx = fixture<any>("threads.json");
  const r = await rig();
  try {
    r.fake.handlers.set("thread/list", (p) => (p.archived ? fx.archived : fx.active));
    const all: any[] = [];
    let cursor: string | null | undefined = null;
    for (let i = 0; i < 10; i++) {
      const page = await r.provider.scanThreads({ cursor, limit: 50 });
      assert.notEqual(page.complete, false);
      all.push(...page.threads);
      cursor = page.cursor;
      if (!cursor) break;
    }
    const reqs = r.fake.requests("thread/list");
    assert.equal(reqs.length, 2);
    assert.equal(reqs[0]!.params.archived, false);
    assert.equal(reqs[1]!.params.archived, true);
    for (const q of reqs) assert.ok(Array.isArray(q.params.sourceKinds) && q.params.sourceKinds.length >= 2, "never the default (interactive-only) filter");
    assert.equal(all.length, fx.active.data.length + fx.archived.data.length);
    for (const t of fx.active.data) {
      const got = all.find((x) => x.native === t.id && !x.archived);
      assert.ok(got);
      const kind = typeof t.source === "string" ? t.source : Object.keys(t.source)[0];
      assert.equal(got.originKind, kind === "cli" || kind === "vscode" ? "interactive" : "programmatic");
      assert.equal(got.cwd, t.cwd);
      assert.equal(got.title, t.name ?? null);
      assert.equal(got.createdAt, new Date(t.createdAt * 1000).toISOString());
    }
    assert.ok(all.some((x) => x.originKind === "programmatic") && all.some((x) => x.originKind === "interactive"));
    assert.ok(all.filter((x) => x.archived).length === fx.archived.data.length);

    // An older daemon that rejects a kind it does not know: ask again with the kinds it has.
    let calls = 0;
    r.fake.handlers.set("thread/list", (p) => {
      calls++;
      if (p.sourceKinds.includes("subAgentOther")) return rpcError(-32600, "Invalid request: unknown variant `subAgentOther`, expected one of `cli`, `vscode`, `exec`");
      return fx.active;
    });
    const page = await r.provider.scanThreads({ cursor: null });
    assert.equal(calls, 2);
    assert.equal(page.threads.length, fx.active.data.length);
    assert.deepEqual(r.fake.requests("thread/list").at(-1)!.params.sourceKinds, ["cli", "vscode", "exec"]);
    assert.deepEqual(acceptedKinds(new Error("x")), null);
  } finally {
    await r.done();
  }
});

test("codex: live index changes from broadcast notifications", async () => {
  const fx = fixture<any>("threads.json");
  const r = await rig();
  try {
    const t = fx.active.data[1];
    r.fake.handlers.set("thread/read", () => ({ thread: { ...t, name: "Renamed" } }));
    await r.provider.probe().catch(() => {});
    r.fake.push({ method: "thread/started", params: { thread: t } });
    r.fake.push({ method: "thread/name/updated", params: { threadId: t.id, threadName: "Renamed" } });
    r.fake.push({ method: "thread/archived", params: { threadId: t.id } });
    r.fake.push({ method: "thread/deleted", params: { threadId: t.id } });
    r.fake.push({ method: "some/futureNotification", params: { threadId: t.id, whatever: 1 } });
    await until(() => r.events.filter((e) => e.type === "threadChanged").length === 3 && r.events.some((e) => e.type === "threadRemoved"));
    const changed = r.events.filter((e) => e.type === "threadChanged") as any[];
    assert.equal(changed[0].info.native, t.id);
    assert.ok(changed.some((c) => c.info.title === "Renamed" && !c.info.archived));
    assert.ok(changed.some((c) => c.info.archived === true));
  } finally {
    await r.done();
  }
});

test("codex: history from thread/items/list, oldest first, paged; unknown items are kept", async () => {
  const fx = fixture<any>("fork.json");
  const r = await rig();
  try {
    const items = fx.sourceTurns.data[0].items as any[];
    const entries = [...items, { type: "someFutureItem", id: "f-1", payload: { x: 1 } }].map((item) => ({ turnId: fx.sourceTurns.data[0].id, item }));
    r.fake.handlers.set("thread/items/list", (p) => {
      const start = p.cursor ? Number(p.cursor) : 0;
      const slice = entries.slice(start, start + 2);
      return { data: slice, nextCursor: start + 2 < entries.length ? String(start + 2) : null };
    });
    const got: any[] = [];
    let cursor: string | null | undefined = null;
    do {
      const page = await r.provider.readThread("src", { cursor, limit: 2 });
      got.push(...page.items);
      cursor = page.cursor;
    } while (cursor);
    assert.equal(got.length, entries.length);
    assert.deepEqual(got.map((i) => i.id), entries.map((e) => e.item.id));
    assert.deepEqual(got.map((i) => i.kind), ["userMessage", "agentMessage", "userMessage", "toolCall", "agentMessage", "toolCall"]);
    assert.equal(got.at(-1).tool, "other");
    assert.deepEqual(got.at(-1).input, { type: "someFutureItem", payload: { x: 1 } });
    assert.equal(r.fake.requests("thread/items/list")[0]!.params.sortDirection, "asc");
  } finally {
    await r.done();
  }
});

test("codex: item mapping covers every documented kind without dropping any", () => {
  const t = "turn-1";
  const cases: Array<[any, string, string?]> = [
    [{ type: "fileChange", id: "a", status: "completed", changes: [{ path: "/w/x.ts", kind: { type: "update", move_path: null }, diff: "@@" }] }, "toolCall", "fileChange"],
    [{ type: "mcpToolCall", id: "b", server: "s", tool: "t", status: "failed", arguments: {}, result: null, error: { message: "no" } }, "toolCall", "mcp"],
    [{ type: "webSearch", id: "c", query: "q", action: null, results: null }, "toolCall", "webSearch"],
    [{ type: "collabAgentToolCall", id: "d", tool: "spawnAgent", status: "completed", senderThreadId: "x", receiverThreadIds: [], prompt: "p", model: null, reasoningEffort: null, agentsStates: {} }, "toolCall", "subagent"],
    [{ type: "plan", id: "e", text: "1. do it" }, "plan"],
    [{ type: "contextCompaction", id: "f" }, "notice"],
    [{ type: "dynamicToolCall", id: "g", tool: "x", status: "completed" }, "toolCall", "other"],
    [{ type: "reasoning", id: "h", summary: ["s"], content: ["full"] }, "reasoning"],
  ];
  for (const [raw, kind, tool] of cases) {
    const item = mapItem(raw, t, "completed") as any;
    assert.equal(item.kind, kind, raw.type);
    if (tool) assert.equal(item.tool, tool, raw.type);
    assert.equal(item.id, raw.id);
    assert.equal(item.turnId, t);
  }
  const fc = mapItem(cases[0]![0], t, "completed") as any;
  assert.deepEqual(fc.input.changes, [{ path: "/w/x.ts", change: "modify", diff: "@@" }]);
  assert.equal((mapItem(cases[7]![0], t, "completed") as any).reasoning, "full");
});

test("codex: control — not loaded is free; loaded elsewhere is in use; a foreign turn is external", async () => {
  const r = await rig();
  try {
    let status: any = { type: "notLoaded" };
    r.fake.handlers.set("thread/read", (p) => ({ thread: { id: p.threadId, status } }));
    assert.deepEqual(await r.provider.control("t1"), { external: false });
    status = { type: "idle" };
    const loaded = await r.provider.control("t1");
    assert.equal(loaded.uncertain, true, "loaded on the daemon, not by yaacd: cannot tell, so in use");
    status = { type: "active", activeFlags: [] };
    const busy = await r.provider.control("t1");
    assert.equal(busy.external, true);
    // Status changes are broadcast: the provider reports them without being asked.
    r.fake.push({ method: "thread/status/changed", params: { threadId: "t1", status: { type: "notLoaded" } } });
    await until(() => r.events.some((e) => e.type === "controlChanged" && !e.control.external && !e.control.uncertain));

    // A thread yaacd holds; another client runs a turn in it.
    idleThread(r.fake, THREAD_ID);
    status = { type: "idle" };
    r.fake.handlers.set("thread/read", (p) => ({ thread: { id: p.threadId, status } }));
    r.fake.handlers.set("turn/start", () => ({ turn: { id: TURN_ID, status: "inProgress" } }));
    await r.provider.startThread({ cwd: "/work/fixture" }, input("go"));
    r.fake.push({ method: "turn/completed", params: { threadId: THREAD_ID, turn: { id: TURN_ID, status: "completed", items: [] } } });
    await until(() => r.events.some((e) => e.type === "turnCompleted"));
    assert.deepEqual(await r.provider.control(THREAD_ID), { external: false });
    r.fake.push({ method: "turn/started", params: { threadId: THREAD_ID, turn: { id: "foreign-1", status: "inProgress", items: [] } } });
    r.fake.push({ method: "item/started", params: { threadId: THREAD_ID, turnId: "foreign-1", item: { type: "userMessage", id: "u-f", clientId: null, content: [{ type: "text", text: "typed in a terminal" }] } } });
    await until(() => r.events.some((e) => e.type === "turnStarted" && e.turnId === "foreign-1"));
    assert.equal((r.events.find((e) => e.type === "turnStarted" && e.turnId === "foreign-1") as any).origin, "external");
    await until(() => r.events.some((e) => e.type === "itemStarted" && e.item.id === "u-f"));
    await until(() => r.events.some((e) => e.type === "controlChanged" && e.thread === THREAD_ID && e.control.external));
    // While the other client's turn runs, the thread cannot be taken over (D140): input would join it.
    status = { type: "active", activeFlags: [] };
    const running = await r.provider.control(THREAD_ID);
    assert.deepEqual(running.holderIds, ["turn:foreign-1"]);
    assert.match(running.takeOverBlocked ?? "", /running a turn/);
    await assert.rejects(r.provider.startTurn(THREAD_ID, input("no"), {}));
    r.fake.push({ method: "turn/completed", params: { threadId: THREAD_ID, turn: { id: "foreign-1", status: "completed", items: [] } } });
    await until(() => r.fake.requests("thread/unsubscribe").length === 1);
    status = { type: "idle" };
    const after = await r.provider.control(THREAD_ID);
    assert.notDeepEqual(after, { external: false }, "still open in the other client");
    // Once it ends, the holder is named by that turn and the thread can be taken over.
    assert.deepEqual(after, { external: true, holder: "another Codex client", holderIds: ["turn:foreign-1"] });
    // After a takeover yaacd sends: Codex lets a second client rejoin a thread another client
    // has loaded and start a turn in it (verified against codex app-server 0.170.0).
    const resumes = r.fake.requests("thread/resume").length;
    r.fake.handlers.set("thread/resume", (p) => ({ thread: { id: p.threadId, status: { type: "idle" }, cwd: "/work/fixture" }, model: "model-1" }));
    r.fake.handlers.set("turn/start", () => ({ turn: { id: "mine-2", status: "inProgress" } }));
    assert.equal(await r.provider.startTurn(THREAD_ID, input("taken over"), {}), "mine-2");
    assert.equal(r.fake.requests("thread/resume").length, resumes + 1, "it rejoins the thread first");
    r.fake.push({ method: "turn/completed", params: { threadId: THREAD_ID, turn: { id: "mine-2", status: "completed", items: [] } } });
    await until(() => r.events.some((e) => e.type === "turnCompleted" && e.turnId === "mine-2"));
    status = { type: "notLoaded" };
    assert.deepEqual(await r.provider.control(THREAD_ID), { external: false }, "Codex unloaded it: nobody has it");
  } finally {
    await r.done();
  }
});

test("codex: a busy thread (turn events or a broadcast status) is refused before turn/start is sent", async () => {
  const r = await rig();
  try {
    idleThread(r.fake, THREAD_ID);
    let n = 0;
    r.fake.handlers.set("turn/start", () => ({ turn: { id: n++ === 0 ? TURN_ID : "other-turn", status: "inProgress" } }));
    await r.provider.startThread({ cwd: "/work/fixture" }, input("one"));
    r.fake.push({ method: "turn/completed", params: { threadId: THREAD_ID, turn: { id: TURN_ID, status: "completed", items: [] } } });
    await until(() => r.events.some((e) => e.type === "turnCompleted"));
    // Another client starts a turn; yaacd's turn/start (racing it) is joined into it.
    r.fake.push({ method: "turn/started", params: { threadId: THREAD_ID, turn: { id: "other-turn", status: "inProgress" } } });
    await until(() => r.events.some((e) => e.type === "turnStarted" && e.turnId === "other-turn"));
    await assert.rejects(r.provider.startTurn(THREAD_ID, input("two"), {}), (e: unknown) => e instanceof Error);
    // Directly: the busy check refuses before sending anything.
    assert.equal(r.fake.requests("turn/start").length, 1);

    // Busy by a broadcast status alone (no turn events seen): refused before sending.
    r.fake.push({ method: "turn/completed", params: { threadId: THREAD_ID, turn: { id: "other-turn", status: "completed", items: [] } } });
    await until(() => r.events.filter((e) => e.type === "turnCompleted").length === 2);
    r.fake.push({ method: "thread/status/changed", params: { threadId: THREAD_ID, status: { type: "active", activeFlags: [] } } });
    await until(() => r.events.some((e) => e.type === "controlChanged" && e.thread === THREAD_ID && e.control.external));
    r.fake.handlers.set("thread/resume", (p) => ({ thread: { id: p.threadId, status: { type: "active", activeFlags: [] } } }));
    await assert.rejects(r.provider.startTurn(THREAD_ID, input("three"), {}), ThreadInUseError);
    assert.equal(r.fake.requests("turn/start").length, 1);
  } finally {
    await r.done();
  }
});

test("codex: steer and interrupt carry the turn id", async () => {
  const r = await rig();
  try {
    idleThread(r.fake, THREAD_ID);
    r.fake.handlers.set("turn/start", () => ({ turn: { id: TURN_ID, status: "inProgress" } }));
    r.fake.handlers.set("turn/steer", () => ({ turnId: TURN_ID }));
    r.fake.handlers.set("turn/interrupt", () => ({}));
    await r.provider.startThread({ cwd: "/w" }, input("go"));
    await r.provider.steer(THREAD_ID, TURN_ID, input("also this"));
    const s = r.fake.requests("turn/steer")[0]!.params;
    assert.equal(s.expectedTurnId, TURN_ID);
    // The steered input is not echoed back as a user message.
    r.fake.push({ method: "item/started", params: { threadId: THREAD_ID, turnId: TURN_ID, item: { type: "userMessage", id: "u1", clientId: s.clientUserMessageId, content: [] } } });
    await r.provider.interrupt(THREAD_ID, TURN_ID);
    assert.deepEqual(r.fake.requests("turn/interrupt")[0]!.params, { threadId: THREAD_ID, turnId: TURN_ID });
    await assert.rejects(r.provider.steer(THREAD_ID, "not-running", input("x")));
    await new Promise((res) => setTimeout(res, 30));
    assert.ok(!r.events.some((e) => e.type === "itemStarted" && e.item.kind === "userMessage"));
  } finally {
    await r.done();
  }
});

test("codex: per-turn model and effort, and effective capabilities from the reported model", async () => {
  const fx = fixture<any>("models.json");
  const r = await rig({}, fx.initialize);
  try {
    const all = fx.pages.flatMap((p: any) => p.data);
    const withImage = all.find((m: any) => m.inputModalities.includes("image") && m.supportedReasoningEfforts.length > 1);
    r.fake.handlers.set("account/read", () => ({ account: { type: "apiKey" } }));
    r.fake.handlers.set("model/list", () => ({ data: all, nextCursor: null }));
    await r.provider.probe();
    idleThread(r.fake, THREAD_ID);
    r.fake.handlers.set("thread/start", (p) => ({ thread: { id: THREAD_ID, status: { type: "idle" } }, model: p.model }));
    r.fake.handlers.set("turn/start", () => ({ turn: { id: TURN_ID, status: "inProgress" } }));
    const effort = withImage.supportedReasoningEfforts.at(-1).reasoningEffort;
    await r.provider.startThread({ cwd: "/w", model: withImage.model, effort }, input("go"));
    const ts = r.fake.requests("turn/start")[0]!.params;
    assert.equal(ts.effort, effort);
    assert.equal(r.fake.requests("thread/start")[0]!.params.model, withImage.model);
    r.fake.push({ method: "turn/completed", params: { threadId: THREAD_ID, turn: { id: TURN_ID, status: "completed", items: [] } } });
    await until(() => r.events.some((e) => e.type === "turnCompleted"));
    const eff = await r.provider.effective(THREAD_ID);
    assert.equal(eff.imageInput, "supported");
    assert.ok(eff.capabilities!.includes("imageInput"));
    assert.deepEqual(eff.efforts!.map((e) => e.id), withImage.supportedReasoningEfforts.map((e: any) => e.reasoningEffort));
    assert.equal(eff.applied!.effort, effort);

    // Switch to a model without image input, mid-thread.
    const noImage = all.find((m: any) => !m.inputModalities.includes("image"));
    if (noImage) {
      r.fake.handlers.set("turn/start", () => ({ turn: { id: "t2", status: "inProgress" } }));
      await r.provider.startTurn(THREAD_ID, input("next"), { model: noImage.model });
      assert.equal(r.fake.requests("turn/start")[1]!.params.model, noImage.model);
      assert.equal((await r.provider.effective(THREAD_ID)).imageInput, "unsupported");
    }
    // A model Codex reported nothing about: unknown.
    r.fake.handlers.set("thread/read", (p) => ({ thread: { id: p.threadId, status: { type: "notLoaded" }, model: "a-model-not-listed" } }));
    assert.equal((await r.provider.effective("unlisted-thread")).imageInput, "unknown");
  } finally {
    await r.done();
  }
});

test("codex: reconnects when the socket drops and rejoins the threads it held", async () => {
  const r = await rig();
  try {
    idleThread(r.fake, THREAD_ID);
    r.fake.handlers.set("turn/start", () => ({ turn: { id: TURN_ID, status: "inProgress" } }));
    await r.provider.startThread({ cwd: "/w" }, input("go"));
    assert.equal(r.fake.connections, 1);
    r.fake.dropConnections();
    await until(() => r.events.some((e) => e.type === "controlChanged" && e.thread === THREAD_ID && !!e.control.uncertain));
    await until(() => r.fake.connections === 2 && r.fake.requests("thread/resume").length === 1);
    assert.equal(r.fake.requests("initialize").length, 2);
    assert.equal(r.fake.requests("thread/resume")[0]!.params.threadId, THREAD_ID);
    await until(() => r.events.some((e) => e.type === "controlChanged" && e.thread === THREAD_ID && !e.control.uncertain && !e.control.external));
  } finally {
    await r.done();
  }
});

// ── Backend selection ──────────────────────────────────────────────────────────────────────

type ExecLog = Array<{ file?: string; args: string[]; env: NodeJS.ProcessEnv }>;

function scriptedExec(answers: Record<string, Awaited<ReturnType<Exec>>>, log: ExecLog, onStart?: () => Promise<void>): Exec {
  return async (file, args, env) => {
    log.push({ file, args, env });
    const key = args.join(" ");
    if (key === "app-server daemon start" && onStart) await onStart();
    const a = answers[key];
    if (!a) throw new Error(`unexpected exec ${key}`);
    return a;
  };
}

function homeProvider(dir: string, exec: Exec, socket?: string, env: Record<string, string> = {}): CodexProvider {
  return new CodexProvider(
    { config: { id: "pi_x", kind: "codex", enabled: true, backend: "auto" }, stateDir: path.join(dir, "state"), home: dir, env: process.env, log: () => {} },
    { exec, env: { CODEX_HOME: path.join(dir, "ch"), ...env }, baseEnv: { PATH: "/nonexistent", HOME: dir }, ...(socket ? { socket } : {}), reconnectMinMs: 10_000 },
  );
}

/** A stand-in `codex` program (never run: the tests' Exec answers instead). */
function fakeCodexBin(dir: string): string {
  fs.mkdirSync(dir, { recursive: true });
  const bin = path.join(dir, "codex");
  fs.writeFileSync(bin, "#!/bin/sh\nexit 1\n", { mode: 0o755 });
  return bin;
}

test("codex backend: codex is found off PATH, in ~/.local/bin, as a non-login wsl.exe or ssh shell leaves it (P130)", async () => {
  const dir = tmp();
  const ok = { code: 0, stdout: "codex-cli 9.9.9\n", stderr: "" };
  const tooOld = { code: 2, stdout: "", stderr: "unrecognized subcommand" };
  try {
    // Nowhere: not installed, and nothing is run.
    const p0 = homeProvider(dir, failExec);
    assert.equal((await p0.probe()).status, "notInstalled");
    await p0.dispose();

    // Only in ~/.local/bin (PATH is /nonexistent): found, run by its path, its folder added to PATH.
    const local = fakeCodexBin(path.join(dir, ".local", "bin"));
    const log: ExecLog = [];
    const p1 = homeProvider(dir, scriptedExec({ "--version": ok, "app-server daemon --help": ok, "app-server daemon start": { code: 1, stdout: "", stderr: "no" } }, log));
    assert.equal((await p1.probe()).status, "startFailed");
    await p1.dispose();
    assert.ok(log.length === 3 && log.every((l) => l.file === local));
    assert.deepEqual(log[2]!.env.PATH!.split(path.delimiter), ["/nonexistent", path.dirname(local)]);
    fs.rmSync(local);

    // Codex's standalone package under CODEX_HOME.
    const standalone = fakeCodexBin(path.join(dir, "ch", "packages", "standalone", "current", "bin"));
    const log2: ExecLog = [];
    const p2 = homeProvider(dir, scriptedExec({ "--version": ok, "app-server daemon --help": tooOld }, log2));
    assert.equal((await p2.probe()).status, "tooOld");
    await p2.dispose();
    assert.equal(log2[0]!.file, standalone);

    // An explicit program in the instance's environment wins.
    const explicit = fakeCodexBin(path.join(dir, "elsewhere"));
    const log3: ExecLog = [];
    const p3 = homeProvider(dir, scriptedExec({ "--version": ok, "app-server daemon --help": tooOld }, log3), undefined, { YAAC_CODEX_EXECUTABLE: explicit });
    await p3.probe();
    await p3.dispose();
    assert.equal(log3[0]!.file, explicit);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test("codex backend: an explicit socket is connect-only; distinct failure reasons; start only when absent", async () => {
  const dir = tmp();
  fakeCodexBin(path.join(dir, ".local", "bin"));
  const ok = { code: 0, stdout: "codex-cli 9.9.9\n", stderr: "" };
  try {
    // Explicit socket, nothing listening: unreachable, and codex is never run.
    const p0 = homeProvider(dir, failExec, path.join(dir, "nothing.sock"));
    const r0 = await p0.probe();
    assert.equal(r0.status, "unreachable");
    await p0.dispose();

    const cases: Array<[Record<string, Awaited<ReturnType<Exec>>>, string]> = [
      [{ "--version": { code: null, stdout: "", stderr: "", spawnError: "ENOENT" } }, "notInstalled"],
      [{ "--version": ok, "app-server daemon --help": { code: 2, stdout: "", stderr: "unrecognized subcommand" } }, "tooOld"],
      [{ "--version": ok, "app-server daemon --help": ok, "app-server daemon start": { code: 1, stdout: "", stderr: "Error: could not bind" } }, "startFailed"],
    ];
    for (const [answers, status] of cases) {
      const log: Array<{ args: string[]; env: NodeJS.ProcessEnv }> = [];
      const p = homeProvider(dir, scriptedExec(answers, log));
      const res = await p.probe();
      assert.equal(res.status, status, JSON.stringify(res.statusMessage));
      if (status === "tooOld") assert.match(res.statusMessage ?? "", /9\.9\.9/);
      await p.dispose();
      for (const l of log) assert.ok(!/stop|restart|update|bootstrap/.test(l.args.join(" ")), "never stop, restart, or update a daemon");
    }

    // Absent → `codex app-server daemon start` under the instance's CODEX_HOME → connect.
    const sock = path.join(dir, "ch", "app-server-control", "app-server-control.sock");
    let fake: FakeCodex | null = null;
    const log: Array<{ args: string[]; env: NodeJS.ProcessEnv }> = [];
    const exec = scriptedExec({ "--version": ok, "app-server daemon --help": ok, "app-server daemon start": ok }, log, async () => {
      fake = await FakeCodex.start(sock);
      fake.handlers.set("account/read", () => ({ account: { type: "apiKey" } }));
      fake.handlers.set("model/list", () => ({ data: [], nextCursor: null }));
    });
    const p = homeProvider(dir, exec);
    const res = await p.probe();
    assert.equal(res.status, "ready", JSON.stringify(res.statusMessage));
    const start = log.find((l) => l.args.join(" ") === "app-server daemon start")!;
    assert.equal(start.env.CODEX_HOME, path.join(dir, "ch"));
    // Running now: a second probe does not start anything.
    const before = log.length;
    await p.probe();
    assert.equal(log.length, before);
    await p.dispose();
    await (fake as FakeCodex | null)?.close();

    // Access denied on the socket.
    if (process.getuid?.() !== 0) {
      const locked = path.join(dir, "locked");
      const f2 = await FakeCodex.start(path.join(locked, "s.sock"));
      fs.chmodSync(locked, 0o000);
      try {
        const p2 = homeProvider(dir, failExec, path.join(locked, "s.sock"));
        assert.equal((await p2.probe()).status, "accessDenied");
        await p2.dispose();
      } finally {
        fs.chmodSync(locked, 0o700);
        await f2.close();
      }
    }
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test("codex: the server version comes from its user agent; the key is Thread.id", () => {
  assert.equal(serverVersion("yaacd/0.171.0 (Ubuntu 24.4.0; x86_64) WindowsTerminal (yaacd; 0.1.0)"), "0.171.0");
  assert.equal(serverVersion(undefined), null);
});

test("codex modes: offered, only never-ask-with-full-access skips approvals, never applied unless chosen", async () => {
  const { CODEX_MODES } = await import("../src/providers/codex/index.js");
  const r = await rig();
  try {
    const probe = await r.provider.probe();
    assert.ok(probe.capabilities.includes("modes"));
    assert.deepEqual(probe.modes.map((m) => m.id), CODEX_MODES.map((m) => m.id));
    for (const m of CODEX_MODES) {
      const offered = probe.modes.find((x) => x.id === m.id)!;
      // Skips approvals exactly when Codex never asks and nothing confines it.
      const never = m.approvalPolicy === "never" && m.sandbox === "danger-full-access";
      assert.equal(offered.skipsApprovals === true, never, m.id);
      assert.ok(!offered.isDefault, "no mode is preselected");
      assert.equal((offered as any).approvalPolicy, undefined, "harness settings stay internal");
    }
    assert.equal(probe.modes.filter((m) => m.skipsApprovals).length, 1, "the bypass mode is offered, not hidden");
    const bypass = probe.modes.find((m) => m.skipsApprovals)!;
    const safe = probe.modes.find((m) => !m.skipsApprovals)!;

    // No mode chosen: nothing sent, the thread follows the user's own Codex configuration.
    idleThread(r.fake, THREAD_ID);
    r.fake.handlers.set("turn/start", () => ({ turn: { id: TURN_ID, status: "inProgress" } }));
    await r.provider.startThread({ cwd: "/w" }, input("go"));
    const st0 = r.fake.requests("thread/start")[0]!.params;
    assert.equal(st0.approvalPolicy, undefined);
    assert.equal(st0.sandbox, undefined);
    assert.equal(r.fake.requests("turn/start")[0]!.params.approvalPolicy, undefined);
    assert.equal((await r.provider.effective(THREAD_ID)).applied!.mode, null);
    r.fake.push({ method: "turn/completed", params: { threadId: THREAD_ID, turn: { id: TURN_ID, status: "completed", items: [] } } });
    await until(() => r.events.some((e) => e.type === "turnCompleted"));

    // Chosen per turn: approval policy and sandbox policy go with that turn.
    r.fake.handlers.set("turn/start", () => ({ turn: { id: "t2", status: "inProgress" } }));
    await r.provider.startTurn(THREAD_ID, input("next"), { mode: bypass.id });
    const tu = r.fake.requests("turn/start")[1]!.params;
    assert.equal(tu.approvalPolicy, "never");
    assert.deepEqual(tu.sandboxPolicy, { type: "dangerFullAccess" });
    assert.equal((await r.provider.effective(THREAD_ID)).applied!.mode, bypass.id);
    assert.ok(r.events.some((e) => e.type === "settingsChanged" && e.applied.mode === bypass.id));

    // Chosen at start, and on a fork.
    r.fake.handlers.set("thread/start", () => ({ thread: { id: "t-safe", status: { type: "idle" } } }));
    r.fake.handlers.set("turn/start", () => ({ turn: { id: "t3", status: "inProgress" } }));
    await r.provider.startThread({ cwd: "/w", mode: safe.id }, input("go"));
    const st1 = r.fake.requests("thread/start")[1]!.params;
    const safeDef = CODEX_MODES.find((m) => m.id === safe.id)!;
    assert.equal(st1.approvalPolicy, safeDef.approvalPolicy);
    assert.equal(st1.sandbox, safeDef.sandbox);
    r.fake.handlers.set("thread/turns/list", () => ({ data: [], nextCursor: null }));
    r.fake.handlers.set("thread/fork", () => ({ thread: { id: "t-fork", status: { type: "idle" } } }));
    await r.provider.forkThread(THREAD_ID, { cwd: "/w", mode: bypass.id });
    assert.equal(r.fake.requests("thread/fork")[0]!.params.approvalPolicy, "never");
    // An unknown mode is refused, never guessed.
    await assert.rejects(r.provider.startThread({ cwd: "/w", mode: "codex:nope" }, input("x")));
  } finally {
    await r.done();
  }
});

test("codex modes: each says how much it allows unasked, and a thread shows the preset Codex reports it in (D146)", async () => {
  const { CODEX_MODES, reportedMode } = await import("../src/providers/codex/provider.js");
  const r = await rig();
  try {
    const probe = await r.provider.probe();
    const autonomy = Object.fromEntries(probe.modes.map((m) => [m.id, m.autonomy]));
    for (const m of CODEX_MODES) {
      const expected = m.sandbox === "read-only" ? "ask" : m.sandbox === "workspace-write" ? "workspace" : "full";
      assert.equal(autonomy[m.id], expected, m.id);
    }
    // The settings Codex reports on thread/start (the user's own configuration, no mode sent).
    const full = { approvalPolicy: "never", approvalsReviewer: "user", sandbox: { type: "dangerFullAccess" } };
    idleThread(r.fake, THREAD_ID);
    r.fake.handlers.set("thread/start", () => ({ thread: { id: THREAD_ID, status: { type: "idle" } }, model: "model-1", ...full }));
    r.fake.handlers.set("turn/start", () => ({ turn: { id: TURN_ID, status: "inProgress" } }));
    await r.provider.startThread({ cwd: "/w" }, input("go"));
    assert.equal(r.fake.requests("thread/start")[0]!.params.approvalPolicy, undefined, "still nothing sent");
    const bypass = CODEX_MODES.find((m) => m.skipsApprovals)!;
    assert.equal((await r.provider.effective(THREAD_ID)).applied!.mode, bypass.id);
    // What no preset describes is unknown; an older Codex that reports nothing changes nothing.
    assert.equal(reportedMode({ ...full, approvalsReviewer: "auto_review" }), null);
    assert.equal(reportedMode({ approvalPolicy: "untrusted", sandbox: { type: "workspaceWrite", networkAccess: false } }), null);
    assert.equal(reportedMode({ approvalPolicy: "on-request", sandbox: { type: "workspaceWrite", networkAccess: true } }), null);
    assert.equal(reportedMode({ approvalPolicy: "on-request", sandbox: { type: "readOnly", networkAccess: false } }), CODEX_MODES.find((m) => m.sandbox === "read-only")!.id);
    assert.equal(reportedMode({}), undefined);
  } finally {
    await r.done();
  }
});

// ── Sub-agents (D151) ────────────────────────────────────────────────────────────────────

const PARENT_ID = "01a1fa3e-0100-7000-8000-000000000001";
const CHILD_ID = "01a1fa3e-0100-7000-8000-000000000002";

/** A sub-agent thread as codex-cli 0.170.0 reports it (`SessionSource.subAgent.thread_spawn`). */
function subAgentThread(status: unknown = { type: "idle" }): Record<string, unknown> {
  return {
    id: CHILD_ID,
    preview: "Look at the parser",
    createdAt: 1763700000,
    updatedAt: 1763700100,
    status,
    cwd: "/work/fixture",
    source: { subAgent: { thread_spawn: { parent_thread_id: PARENT_ID, depth: 1, agent_path: null, agent_nickname: "Kepler", agent_role: "explorer" } } },
    agentNickname: "Kepler",
    agentRole: "explorer",
    name: null,
  };
}

test("codex: sub-agent threads are listed with their parent, outside the max", async () => {
  const r = await rig();
  try {
    const parent = { id: PARENT_ID, preview: "Orchestrate", createdAt: 1763700000, updatedAt: 1763700050, status: { type: "idle" }, cwd: "/work/fixture", source: "vscode", name: "Orchestrate" };
    r.fake.handlers.set("thread/list", (p) => (p.archived ? { data: [], nextCursor: null } : { data: [subAgentThread(), parent], nextCursor: null }));
    const page = await r.provider.scanThreads({ cursor: null, max: 1 });
    const kid = page.threads.find((t) => t.native === CHILD_ID)!;
    const top = page.threads.find((t) => t.native === PARENT_ID)!;
    assert.ok(kid && top, "the sub-agent does not use up max: its parent is still listed");
    assert.equal(kid.parent, PARENT_ID);
    assert.equal(kid.title, "Kepler", "the nickname, when it has no name");
    assert.equal(kid.subagent!.agentType, "explorer");
    assert.equal(kid.subagent!.agentId, CHILD_ID, "what the parent's collab calls name it by");
    assert.equal(kid.subagent!.status, "completed");
    assert.match(kid.subagent!.readOnly!, /only writer/);
    assert.equal(top.parent, undefined);
    assert.equal(top.subagent, undefined);
    assert.ok((r.fake.requests("thread/list")[0]!.params.sourceKinds as string[]).includes("subAgentThreadSpawn"));
  } finally {
    await r.done();
  }
});

test("codex: a sub-agent appears when it starts, and its parent's collab calls update its state", async () => {
  const r = await rig();
  try {
    let status: unknown = { type: "active", activeFlags: [] };
    r.fake.handlers.set("thread/read", () => ({ thread: subAgentThread(status) }));
    await r.provider.probe().catch(() => {});
    r.fake.push({ method: "thread/started", params: { thread: subAgentThread(status) } });
    const isKid = (e: ProviderEvent) => e.type === "threadChanged" && e.info.native === CHILD_ID;
    await until(() => r.events.some(isKid));
    const started = r.events.find(isKid) as any;
    assert.equal(started.info.parent, PARENT_ID);
    assert.equal(started.info.subagent.status, "running");

    // The parent waits on it; the call reports it completed.
    status = { type: "idle" };
    const mark = r.events.length;
    r.fake.push({
      method: "item/completed",
      params: {
        threadId: PARENT_ID,
        turnId: "turn-p",
        item: { type: "collabAgentToolCall", id: "call-1", tool: "wait", status: "completed", senderThreadId: PARENT_ID, receiverThreadIds: [CHILD_ID], prompt: null, model: null, reasoningEffort: null, agentsStates: { [CHILD_ID]: { status: "completed", message: "done" } } },
      },
    });
    await until(() => r.events.slice(mark).some(isKid));
    assert.equal((r.events.slice(mark).find(isKid) as any).info.subagent.status, "completed");

    // A status change of a known sub-agent is reported too: an error is a failure.
    const mark2 = r.events.length;
    r.fake.push({ method: "thread/status/changed", params: { threadId: CHILD_ID, status: { type: "systemError" } } });
    await until(() => r.events.slice(mark2).some(isKid));
    assert.equal((r.events.slice(mark2).find(isKid) as any).info.subagent.status, "failed");
  } finally {
    await r.done();
  }
});

test("codex: compaction is confirmed by the native item and completes without a user prompt", async () => {
  const r = await rig();
  try {
    idleThread(r.fake);
    r.fake.handlers.set("thread/compact/start", (params) => {
      assert.deepEqual(params, { threadId: THREAD_ID });
      return {};
    });
    let resolved = false;
    const run = r.provider.compact(THREAD_ID).then(() => { resolved = true; });
    await until(() => r.fake.received.some((msg) => msg.method === "thread/compact/start"));
    const params = { threadId: THREAD_ID, turnId: TURN_ID, item: { id: "native-compaction", type: "contextCompaction" } };
    r.fake.push({ method: "turn/started", params: { threadId: THREAD_ID, turn: { id: TURN_ID, status: "inProgress" } } });
    r.fake.push({ method: "item/started", params });
    await until(() => r.events.some((e) => e.type === "turnStarted" && e.turnId === TURN_ID));
    assert.equal(resolved, false, "the request reply and start event do not confirm compaction");
    r.fake.push({ method: "item/completed", params });
    r.fake.push({ method: "turn/completed", params: { threadId: THREAD_ID, turn: { id: TURN_ID, status: "completed" } } });
    await run;
    assert.ok(r.events.some((e) => e.type === "itemCompleted" && e.item.kind === "notice" && e.item.text === "Context compacted"));
    assert.ok(r.events.some((e) => e.type === "turnStarted" && e.origin === "user" && e.turnId === TURN_ID));
    assert.equal(r.fake.received.filter((msg) => msg.method === "turn/start").length, 0);
  } finally {
    await r.done();
  }
});

test("codex: a racing external user turn is not owned by the compaction request", async () => {
  const r = await rig();
  try {
    idleThread(r.fake);
    r.fake.handlers.set("thread/compact/start", () => ({}));
    const run = assert.rejects(r.provider.compact(THREAD_ID), ThreadInUseError);
    await until(() => r.fake.received.some((msg) => msg.method === "thread/compact/start"));
    r.fake.push({ method: "turn/started", params: { threadId: THREAD_ID, turn: { id: TURN_ID, status: "inProgress" } } });
    r.fake.push({ method: "item/completed", params: {
      threadId: THREAD_ID, turnId: TURN_ID, item: { id: "outside", type: "userMessage", content: [{ type: "text", text: "A different client" }] },
    } });
    await run;
    assert.ok(r.events.some((e) => e.type === "turnStarted" && e.turnId === TURN_ID && e.origin === "external"));
    assert.equal((await r.provider.control(THREAD_ID)).external, true);
  } finally {
    await r.done();
  }
});

test("codex: a compaction item in a failed turn does not produce a success notice", async () => {
  const r = await rig();
  try {
    idleThread(r.fake);
    r.fake.handlers.set("thread/compact/start", () => ({}));
    const run = assert.rejects(r.provider.compact(THREAD_ID), /compaction failed/);
    await until(() => r.fake.received.some((msg) => msg.method === "thread/compact/start"));
    const params = { threadId: THREAD_ID, turnId: TURN_ID, item: { id: "failed-compaction", type: "contextCompaction" } };
    r.fake.push({ method: "turn/started", params: { threadId: THREAD_ID, turn: { id: TURN_ID, status: "inProgress" } } });
    r.fake.push({ method: "item/completed", params });
    r.fake.push({ method: "turn/completed", params: { threadId: THREAD_ID, turn: {
      id: TURN_ID, status: "failed", error: { message: "compaction failed" },
    } } });
    await run;
    assert.ok(!r.events.some((e) => e.type === "itemCompleted" && e.item.id === "failed-compaction"));
  } finally {
    await r.done();
  }
});

test("Codex tracks process lifetime and late output separately from the turn", async () => {
  const r = await rig();
  try {
    idleThread(r.fake);
    r.fake.handlers.set("turn/start", () => ({ turn: { id: TURN_ID, status: "inProgress" } }));
    r.fake.handlers.set("thread/loaded/list", () => ({ data: [THREAD_ID], nextCursor: null }));
    const terminal = { itemId: "producer", processId: "473", command: "npm run preview", cwd: "/work/fixture" };
    let running = true;
    r.fake.handlers.set("thread/backgroundTerminals/list", (p) => {
      assert.equal(p.threadId, THREAD_ID);
      return { data: running ? [terminal] : [], nextCursor: null };
    });
    const stops: unknown[] = [];
    r.fake.handlers.set("thread/backgroundTerminals/terminate", (p) => {
      stops.push(p);
      running = false;
      return { terminated: true };
    });
    await r.provider.startThread({ cwd: "/work/fixture" }, input("launch preview"));
    r.fake.push({ method: "item/started", params: {
      threadId: THREAD_ID, turnId: TURN_ID, item: { id: terminal.itemId, type: "commandExecution", command: terminal.command },
    } });
    const last = () => r.events.findLast((e) => e.type === "runningCommands");
    await until(() => last()?.type === "runningCommands");
    const active = last();
    assert.equal(active?.type, "runningCommands");
    if (active?.type !== "runningCommands") return;
    const cmd = active.commands[0]!;
    assert.equal(cmd.command, terminal.command);
    const started = () => r.events.filter((e) => e.type === "itemCompleted" && e.item.kind === "notice" && e.item.text === `Started background command: ${terminal.command}`);
    assert.equal(started().length, 0, "the running foreground phase is not yet a confirmed background launch");
    r.fake.push({ method: "turn/completed", params: { threadId: THREAD_ID, turn: { id: TURN_ID, status: "completed" } } });
    r.fake.push({ method: "item/commandExecution/outputDelta", params: { threadId: THREAD_ID, turnId: TURN_ID, itemId: terminal.itemId, delta: "server ready" } });
    await until(() => started().length === 1);
    assert.deepEqual(await r.provider.commandLogs(THREAD_ID, cmd.id), { output: "server ready", truncated: false });
    assert.equal(await r.provider.stopCommand("another-owner", cmd.id), false);
    assert.equal(await r.provider.stopCommand(THREAD_ID, cmd.id), true);
    assert.deepEqual(stops, [{ threadId: THREAD_ID, processId: terminal.processId }]);
    const latest = last();
    assert.equal(latest?.type, "runningCommands");
    if (latest?.type === "runningCommands") assert.deepEqual(latest.commands, []);
    assert.equal(await r.provider.stopCommand(THREAD_ID, cmd.id), false);
    assert.equal(started().length, 1);
  } finally {
    await r.done();
  }
});
