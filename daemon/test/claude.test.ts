// The Claude provider against a stubbed SDK: recorded and scripted `SDKMessage` streams are
// replayed into it, and `canUseTool` is driven by hand. No Claude Code process is started.

import { after, test } from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { fileURLToPath } from "node:url";
import type { ModelInfo as SdkModelInfo, Options, PermissionResult, PermissionUpdate, SDKMessage, SDKSessionInfo, SDKUserMessage, SessionMessage } from "@anthropic-ai/claude-agent-sdk";
import { ClaudeProvider, claudeProviderFactory, type ClaudeProviderOptions } from "../src/providers/claude/index.js";
import type { AgentEntry, ClaudeCli, ClaudeSdk, SdkQuery } from "../src/providers/claude/sdk.js";
import { historyItems } from "../src/providers/claude/translate.js";
import type { ProviderEvent, ProviderInput } from "../src/providers/types.js";
import type { Item } from "../src/protocol/generated/types.js";

const here = path.dirname(fileURLToPath(import.meta.url));
// Compiled to dist/test-build/test/; the fixtures stay in the source tree.
const FIXTURES = path.resolve(here, "../../../test/fixtures/claude");

// ── The stub SDK ─────────────────────────────────────────────────────────────────────────

let nextPid = 40000;

class StubQuery implements SdkQuery {
  readonly calls: Array<[string, unknown]> = [];
  readonly users: SDKUserMessage[] = [];
  private out: SDKMessage[] = [];
  private outWaiters: Array<() => void> = [];
  private userWaiters: Array<() => void> = [];
  private ended = false;
  private failure: Error | null = null;
  closed = false;
  failEffort = false;
  model: string | null;
  effort: string | null;
  readonly pid = nextPid++;

  constructor(
    readonly options: Options,
    prompt: AsyncIterable<SDKUserMessage>,
    private readonly models: SdkModelInfo[],
  ) {
    this.model = options.model ?? null;
    this.effort = (options.effort as string | undefined) ?? null;
    void (async () => {
      for await (const u of prompt) {
        this.users.push(u);
        for (const w of this.userWaiters.splice(0)) w();
      }
    })();
  }

  get canUseTool(): NonNullable<Options["canUseTool"]> {
    return this.options.canUseTool!;
  }

  push(...messages: unknown[]): void {
    this.out.push(...(messages as SDKMessage[]));
    for (const w of this.outWaiters.splice(0)) w();
  }

  end(): void {
    this.ended = true;
    for (const w of this.outWaiters.splice(0)) w();
  }

  crash(message: string): void {
    this.failure = new Error(message);
    this.end();
  }

  async nextUser(n = 1): Promise<SDKUserMessage> {
    while (this.users.length < n) await new Promise<void>((r) => this.userWaiters.push(r));
    return this.users[n - 1]!;
  }

  async *[Symbol.asyncIterator](): AsyncIterator<SDKMessage> {
    for (;;) {
      const m = this.out.shift();
      if (m) {
        yield m;
        continue;
      }
      if (this.ended) {
        if (this.failure) throw this.failure;
        return;
      }
      await new Promise<void>((r) => this.outWaiters.push(r));
    }
  }

  async interrupt() {
    this.calls.push(["interrupt", null]);
    return undefined;
  }
  async setModel(model?: string) {
    this.calls.push(["setModel", model]);
    this.model = model ?? null;
    this.effort = null;
  }
  async setPermissionMode(mode: string) {
    this.calls.push(["setPermissionMode", mode]);
  }
  async applyFlagSettings(settings: Record<string, unknown>) {
    this.calls.push(["applyFlagSettings", settings]);
    if (this.failEffort) throw new Error("flag settings rejected");
    if ("effortLevel" in settings) this.effort = settings.effortLevel as string;
  }
  async supportedModels() {
    return this.models;
  }
  async accountInfo() {
    return { email: "someone@example.invalid" };
  }
  async initializationResult() {
    return { pid: this.pid, claude_code_version: "stub-cc" };
  }
  async getSettings() {
    return { applied: { model: this.model ?? "stub-default-resolved", effort: this.effort ?? "stub-mid" } };
  }
  close(): void {
    this.closed = true;
    this.calls.push(["close", null]);
    this.end();
  }
}

const MODELS: SdkModelInfo[] = [
  { value: "stub-alias", resolvedModel: "stub-default-resolved", displayName: "Stub Alias", description: "An invented model", supportsEffort: true, supportedEffortLevels: ["stub-lo" as never, "stub-mid" as never, "stub-hi" as never], supportsFastMode: true },
  { value: "stub-plain", displayName: "Stub Plain", description: "No efforts" },
];

class StubSdk implements ClaudeSdk {
  readonly queries: StubQuery[] = [];
  sessions: SDKSessionInfo[] = [];
  programmatic = new Set<string>();
  messages: Record<string, SessionMessage[]> = {};
  readonly calls: Array<[string, unknown[]]> = [];
  query(params: { prompt: AsyncIterable<SDKUserMessage>; options: Options }): SdkQuery {
    const q = new StubQuery(params.options, params.prompt, MODELS);
    this.queries.push(q);
    return q;
  }
  async listSessions(o?: { includeProgrammatic?: boolean }) {
    return this.sessions.filter((s) => o?.includeProgrammatic !== false || !this.programmatic.has(s.sessionId));
  }
  async getSessionInfo(id: string) {
    return this.sessions.find((s) => s.sessionId === id);
  }
  async getSessionMessages(id: string) {
    return this.messages[id] ?? [];
  }
  async forkSession(id: string, o?: unknown) {
    this.calls.push(["forkSession", [id, o]]);
    return { sessionId: "forked-session" };
  }
  async renameSession(id: string, t: string) {
    this.calls.push(["renameSession", [id, t]]);
  }
  async archiveSession(id: string) {
    this.calls.push(["archiveSession", [id]]);
  }
  async unarchiveSession(id: string) {
    this.calls.push(["unarchiveSession", [id]]);
  }
  async deleteSession(id: string) {
    this.calls.push(["deleteSession", [id]]);
  }
}

class StubCli implements ClaudeCli {
  agentsList: AgentEntry[] | Error = [];
  loggedIn = true;
  async version() {
    return "stub-version";
  }
  async authStatus() {
    return { loggedIn: this.loggedIn, configDirectory: "/home/user/.claude" };
  }
  async agents() {
    if (this.agentsList instanceof Error) throw this.agentsList;
    return this.agentsList;
  }
}

interface Rig {
  provider: ClaudeProvider;
  sdk: StubSdk;
  cli: StubCli;
  events: ProviderEvent[];
  dir: string;
}

const tmpDirs: string[] = [];
after(() => {
  for (const d of tmpDirs) fs.rmSync(d, { recursive: true, force: true });
});

function tmp(prefix: string): string {
  const d = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), prefix)));
  tmpDirs.push(d);
  return d;
}

function rig(opts: Partial<ClaudeProviderOptions> = {}): Rig {
  const dir = tmp("yaac-claude-test-");
  const sdk = new StubSdk();
  const cli = new StubCli();
  const provider = new ClaudeProvider({ sdk, cli, executable: "/nonexistent/claude", env: {}, stateDir: path.join(dir, "state"), idleMs: 0, controlCacheMs: 0, ...opts });
  const events: ProviderEvent[] = [];
  provider.events.on((e) => events.push(e));
  return { provider, sdk, cli, events, dir };
}

function input(text: string, extra: Partial<ProviderInput> = {}): ProviderInput {
  return { parts: [{ type: "text", text }], text, images: [], files: [], origin: "user", ...extra };
}

async function until<T>(fn: () => T | undefined | null | false, what: string, ms = 3000): Promise<T> {
  const end = Date.now() + ms;
  for (;;) {
    const v = fn();
    if (v) return v;
    if (Date.now() > end) throw new Error(`timed out waiting for ${what}`);
    await new Promise((r) => setTimeout(r, 5));
  }
}

const tick = () => new Promise((r) => setTimeout(r, 10));

function of<T extends ProviderEvent["type"]>(events: ProviderEvent[], type: T): Array<Extract<ProviderEvent, { type: T }>> {
  return events.filter((e) => e.type === type) as Array<Extract<ProviderEvent, { type: T }>>;
}

function completedItems(events: ProviderEvent[]): Item[] {
  return of(events, "itemCompleted").map((e) => e.item);
}

const result = (extra: Record<string, unknown> = {}) => ({
  type: "result",
  subtype: "success",
  is_error: false,
  result: "",
  usage: { input_tokens: 10, output_tokens: 5, cache_read_input_tokens: 100, cache_creation_input_tokens: 0 },
  uuid: "r",
  session_id: "s",
  ...extra,
});

function signal(): AbortSignal {
  return new AbortController().signal;
}

async function startedThread(r: Rig, text = "hello") {
  const started = await r.provider.startThread({ cwd: r.dir }, input(text));
  const q = r.sdk.queries.at(-1)!;
  await q.nextUser();
  return { ...started, q };
}

// ── Tests ────────────────────────────────────────────────────────────────────────────────

test("probe reports models and their efforts exactly as listed, and closes its query", async () => {
  const r = rig();
  const p = await r.provider.probe();
  assert.equal(p.status, "ready");
  assert.equal(p.backend, "sdk");
  assert.equal(p.harnessVersion, "stub-cc");
  assert.equal(p.modelsComplete, true);
  assert.deepEqual(
    p.models.map((m) => [m.id, m.displayName, (m.efforts ?? []).map((e) => e.id), m.isDefault]),
    [
      ["stub-alias", "Stub Alias", ["stub-lo", "stub-mid", "stub-hi"], true],
      ["stub-plain", "Stub Plain", [], false],
    ],
  );
  assert.equal(p.models[0]!.defaultEffort, "stub-mid");
  // Image input for every model; fast mode as Claude Code reports it per model (D125): a
  // model that says nothing is unknown, never "yes".
  assert.deepEqual(p.models.map((m) => m.capabilities), [
    [{ capability: "imageInput", support: "supported" }, { capability: "fastMode", support: "supported" }],
    [{ capability: "imageInput", support: "supported" }, { capability: "fastMode", support: "unknown" }],
  ]);
  assert.ok(p.capabilities.includes("approvals") && p.capabilities.includes("userQuestions") && p.capabilities.includes("reasoning"));
  assert.ok(p.modes.some((m) => m.id === "default" && m.isDefault));
  assert.ok(p.modes.some((m) => m.id === "bypassPermissions" && m.skipsApprovals && !m.isDefault));
  // What the agent mesh maps modes by (D146): every mode says how much it allows unasked.
  const autonomy = Object.fromEntries(p.modes.map((m) => [m.id, m.autonomy]));
  assert.equal(autonomy.plan, "readOnly");
  assert.equal(autonomy.default, "ask");
  assert.equal(autonomy.dontAsk, "ask");
  assert.equal(autonomy.acceptEdits, "edits");
  assert.equal(autonomy.bypassPermissions, "full");
  if ("auto" in autonomy) assert.equal(autonomy.auto, "judged");
  assert.equal(p.store.account, "someone@example.invalid");
  assert.ok(p.store.key?.startsWith("claude:"));
  const probeQuery = r.sdk.queries[0]!;
  assert.equal(probeQuery.closed, true);
  assert.equal(probeQuery.options.persistSession, false);

  r.cli.loggedIn = false;
  assert.equal((await r.provider.probe()).status, "needsAuth");
  const none = new ClaudeProvider({ sdk: r.sdk, executable: null, env: {}, stateDir: path.join(r.dir, "none") });
  assert.equal((await none.probe()).status, "notInstalled");
});

test("the factory finds no implicit instance in an environment without claude", () => {
  assert.equal(claudeProviderFactory.kind, "claude");
  assert.equal(claudeProviderFactory.implicit({ YAAC_FAKE_PROVIDER: "1" }), false);
  assert.equal(claudeProviderFactory.implicit({ PATH: path.join(os.tmpdir(), "no-such-dir") }), false);
});

test("a streamed turn with thinking and a tool call becomes reasoning, text, and a command item", async () => {
  const r = rig();
  const { thread, turn, q } = await startedThread(r, "list files");
  // The session id is chosen up front; the first user message carries the turn id.
  assert.equal(q.options.sessionId, thread);
  assert.equal(q.options.resume, undefined);
  assert.equal(q.options.includePartialMessages, true);
  assert.equal(q.options.permissionMode, "default");
  assert.equal(q.options.cwd, r.dir);
  assert.equal(q.options.pathToClaudeCodeExecutable, "/nonexistent/claude");
  const user = await q.nextUser();
  assert.equal(user.uuid, turn);
  assert.equal(user.message.content, "list files");
  assert.deepEqual(of(r.events, "turnStarted").map((e) => [e.turnId, e.origin]), [[turn, "user"]]);

  const se = (event: unknown) => ({ type: "stream_event", event, parent_tool_use_id: null, uuid: "u", session_id: thread });
  q.push(
    se({ type: "message_start", message: { id: "msg_A" } }),
    se({ type: "content_block_start", index: 0, content_block: { type: "thinking", thinking: "" } }),
    se({ type: "content_block_delta", index: 0, delta: { type: "thinking_delta", thinking: "Let me " } }),
    se({ type: "content_block_delta", index: 0, delta: { type: "thinking_delta", thinking: "look." } }),
    se({ type: "content_block_stop", index: 0 }),
    se({ type: "content_block_start", index: 1, content_block: { type: "text", text: "" } }),
    se({ type: "content_block_delta", index: 1, delta: { type: "text_delta", text: "Listing." } }),
    { type: "assistant", message: { id: "msg_A", content: [{ type: "thinking", thinking: "Let me look." }] }, parent_tool_use_id: null, uuid: "a1", session_id: thread },
    { type: "assistant", message: { id: "msg_A", content: [{ type: "text", text: "Listing." }] }, parent_tool_use_id: null, uuid: "a2", session_id: thread },
    se({ type: "content_block_stop", index: 1 }),
    se({ type: "content_block_start", index: 2, content_block: { type: "tool_use", id: "toolu_1", name: "Bash", input: {} } }),
    se({ type: "content_block_delta", index: 2, delta: { type: "input_json_delta", partial_json: '{"command":' } }),
    se({ type: "content_block_delta", index: 2, delta: { type: "input_json_delta", partial_json: '"ls"}' } }),
    se({ type: "content_block_stop", index: 2 }),
    { type: "assistant", message: { id: "msg_A", content: [{ type: "tool_use", id: "toolu_1", name: "Bash", input: { command: "ls" } }] }, parent_tool_use_id: null, uuid: "a3", session_id: thread },
    { type: "user", message: { role: "user", content: [{ type: "tool_result", tool_use_id: "toolu_1", content: "a.txt\nb.txt", is_error: false }] }, parent_tool_use_id: null, uuid: "u1", session_id: thread },
    // A subagent's own message is not part of the main conversation.
    { type: "assistant", message: { id: "msg_sub", content: [{ type: "text", text: "inside a subagent" }] }, parent_tool_use_id: "toolu_x", uuid: "a4", session_id: thread },
    result(),
  );
  const done = await until(() => of(r.events, "turnCompleted")[0], "turnCompleted");
  assert.equal(done.turnId, turn);
  assert.equal(done.status, "completed");
  assert.deepEqual(done.usage, { input: 110, cachedInput: 100, output: 5, total: 115 });

  const deltas = of(r.events, "itemDelta");
  assert.deepEqual(deltas.map((d) => [d.itemId, d.field, d.delta]), [
    ["msg_A#0", "reasoning", "Let me "],
    ["msg_A#0", "reasoning", "look."],
    ["msg_A#1", "text", "Listing."],
  ]);
  const items = completedItems(r.events);
  assert.deepEqual(items.map((i) => [i.id, i.kind, i.status]), [
    ["msg_A#0", "reasoning", "completed"],
    ["msg_A#1", "agentMessage", "completed"],
    ["toolu_1", "toolCall", "completed"],
  ]);
  const tool = items[2] as Item & { tool: string; title: string; output: string; cwd?: string };
  assert.equal(tool.tool, "command");
  assert.equal(tool.title, "ls");
  assert.equal(tool.output, "a.txt\nb.txt");
  assert.equal(tool.cwd, r.dir);
  for (const i of items) assert.equal(i.turnId, turn);
  // yaacd journals the user's own message; the provider never echoes it.
  assert.ok(!r.events.some((e) => "item" in e && (e.item as Item).kind === "userMessage"));
  assert.ok(!items.some((i) => JSON.stringify(i).includes("inside a subagent")));
  await r.provider.dispose();
});

test("a recorded turn (Bash, then AskUserQuestion) replays into items whose ids match its stored history", async () => {
  const fx = JSON.parse(fs.readFileSync(path.join(FIXTURES, "turn-bash-question.json"), "utf8")) as {
    stream: unknown[];
    permissions: Array<{ name: string; input: Record<string, unknown>; toolUseID: string }>;
    history: SessionMessage[];
    turnUuid: string;
  };
  const r = rig();
  const { thread, q } = await startedThread(r, "recorded");
  const ask = fx.permissions.find((p) => p.name === "AskUserQuestion")!;
  const askAt = fx.stream.findIndex((m) => (m as { type: string; message?: { content?: Array<{ name?: string }> } }).type === "assistant" && (m as { message: { content: Array<{ name?: string }> } }).message.content.some((b) => b.name === "AskUserQuestion"));
  q.push(...fx.stream.slice(0, askAt + 1));
  // The CLI asks through canUseTool.
  let answered: PermissionResult | null = null;
  void q.canUseTool(ask.name, ask.input, { signal: signal(), toolUseID: ask.toolUseID, requestId: "ctl-1" }).then((res) => (answered = res));
  const raised = await until(() => of(r.events, "requestRaised")[0], "requestRaised");
  assert.equal(raised.requestId, ask.toolUseID);
  assert.equal(raised.request.requestKind, "question");
  const qs = (raised.request as { questions: Array<{ id: string; header: string; text: string; options: Array<{ label: string; description: string }>; multiSelect: boolean; allowOther: boolean }> }).questions;
  const recordedQ = (ask.input.questions as Array<{ question: string; header: string; options: Array<{ label: string }> }>)[0]!;
  assert.equal(qs[0]!.text, recordedQ.question);
  assert.equal(qs[0]!.header, recordedQ.header);
  assert.deepEqual(qs[0]!.options.map((o) => o.label), recordedQ.options.map((o) => o.label));
  assert.equal(qs[0]!.allowOther, true);
  await tick();
  assert.equal(answered, null, "the question waits for the user");
  const label = recordedQ.options[0]!.label;
  await r.provider.respond(thread, ask.toolUseID, { requestKind: "question", answers: { q1: { selected: [label] } } });
  await until(() => answered, "the answer");
  assert.deepEqual(answered, { behavior: "allow", updatedInput: { ...ask.input, answers: { [recordedQ.question]: label } } });
  assert.deepEqual(of(r.events, "requestResolved").map((e) => [e.requestId, e.by]), [[ask.toolUseID, "client"]]);
  q.push(...fx.stream.slice(askAt + 1));
  const done = await until(() => of(r.events, "turnCompleted")[0], "turnCompleted");
  assert.equal(done.status, "completed");

  const live = completedItems(r.events);
  const bash = live.find((i) => i.kind === "toolCall") as Item & { title: string; output: string; tool: string };
  assert.equal(bash.tool, "command");
  assert.equal(bash.title, "echo hi");
  assert.equal(bash.output, "hi");
  const text = live.find((i) => i.kind === "agentMessage") as Item & { text: string };
  assert.ok(text.text.length > 0);
  assert.ok(!live.some((i) => i.kind === "toolCall" && (i as { title?: string }).title === "AskUserQuestion"), "the question is a request, not a tool call");

  // History of the same conversation: the same ids, and the turn id is the uuid yaacd set.
  const hist = historyItems(fx.history, null);
  assert.equal(hist[0]!.id, `user:${fx.turnUuid}`);
  assert.equal(hist[0]!.kind, "userMessage");
  for (const i of hist) assert.equal(i.turnId, fx.turnUuid);
  const ids = new Set(hist.map((i) => i.id));
  for (const i of live) assert.ok(ids.has(i.id), `history has live item ${i.id}`);
  const req = hist.find((i) => i.kind === "request") as Item & { requestId: string; state: string };
  assert.equal(req.id, `request:${ask.toolUseID}`);
  assert.equal(req.state, "resolved");
  await r.provider.dispose();
});

test("an approval offers the decisions Claude Code makes available and returns the chosen one", async () => {
  const r = rig();
  const { thread, turn, q } = await startedThread(r);
  const suggestion: PermissionUpdate = { type: "addRules", rules: [{ toolName: "Bash", ruleContent: "npm test" }], behavior: "allow", destination: "localSettings" };
  const pending = q.canUseTool("Bash", { command: "npm test" }, { signal: signal(), toolUseID: "toolu_b", requestId: "c1", suggestions: [suggestion], decisionReason: "not allowed yet" });
  const raised = await until(() => of(r.events, "requestRaised")[0], "requestRaised");
  assert.equal(raised.turnId, turn);
  assert.equal(raised.blocking, true);
  const req = raised.request as { requestKind: string; approvalKind: string; command: string; reason: string; decisions: Array<{ id: string; kind: string }> };
  assert.equal(req.requestKind, "approval");
  assert.equal(req.approvalKind, "command");
  assert.equal(req.command, "npm test");
  assert.equal(req.reason, "not allowed yet");
  assert.deepEqual(req.decisions.map((d) => d.kind), ["allow", "allowSession", "allowAlways", "deny", "abort"]);
  await r.provider.respond(thread, "toolu_b", { requestKind: "approval", decision: "allowSession" });
  assert.deepEqual(await pending, { behavior: "allow", updatedInput: { command: "npm test" }, updatedPermissions: [{ ...suggestion, destination: "session" }] });

  // Without suggestions only allow / deny / abort are offered; a denial carries the reason.
  const second = q.canUseTool("Write", { file_path: "/x/y.txt", content: "z" }, { signal: signal(), toolUseID: "toolu_w", requestId: "c2" });
  const raised2 = await until(() => of(r.events, "requestRaised")[1], "second requestRaised");
  const req2 = raised2.request as { approvalKind: string; decisions: Array<{ kind: string }>; changes: Array<{ path: string; change: string }> };
  assert.equal(req2.approvalKind, "fileChange");
  assert.deepEqual(req2.changes, [{ path: "/x/y.txt", change: "add", diff: null }]);
  assert.deepEqual(req2.decisions.map((d) => d.kind), ["allow", "deny", "abort"]);
  await r.provider.respond(thread, "toolu_w", { requestKind: "approval", decision: "deny", reason: "not that file" });
  assert.deepEqual(await second, { behavior: "deny", message: "not that file" });
  await assert.rejects(r.provider.respond(thread, "toolu_w", { requestKind: "approval", decision: "allow" }), /not pending/);
  await r.provider.dispose();
});

test("AskUserQuestion: multi-select, previews, and a free-text answer round trip", async () => {
  const r = rig();
  const { thread, q } = await startedThread(r);
  const askInput = {
    questions: [
      { question: "Which parts?", header: "Parts", multiSelect: true, options: [{ label: "A", description: "first", preview: "aaa" }, { label: "B", description: "second" }, { label: "C", description: "third" }] },
      { question: "Name?", header: "Name", multiSelect: false, options: [{ label: "X", description: "x" }, { label: "Y", description: "y" }] },
    ],
  };
  const pending = q.canUseTool("AskUserQuestion", askInput, { signal: signal(), toolUseID: "toolu_q", requestId: "c" });
  const raised = await until(() => of(r.events, "requestRaised")[0], "requestRaised");
  assert.deepEqual(raised.request, {
    requestKind: "question",
    questions: [
      { id: "q1", header: "Parts", text: "Which parts?", options: [{ label: "A", description: "first", preview: "aaa" }, { label: "B", description: "second", preview: null }, { label: "C", description: "third", preview: null }], multiSelect: true, allowOther: true, secret: false },
      { id: "q2", header: "Name", text: "Name?", options: [{ label: "X", description: "x", preview: null }, { label: "Y", description: "y", preview: null }], multiSelect: false, allowOther: true, secret: false },
    ],
  });
  await r.provider.respond(thread, "toolu_q", { requestKind: "question", answers: { q1: { selected: ["A", "C"] }, q2: { other: "Zed" } } });
  const res = (await pending) as { behavior: string; updatedInput: { answers: Record<string, string> } };
  assert.equal(res.behavior, "allow");
  assert.deepEqual(res.updatedInput.answers, { "Which parts?": "A, C", "Name?": "Zed" });
  await r.provider.dispose();
});

test("Claude Code never reports threadMoveWorkingDirectory: a move is YAAC's placement only (D144)", async () => {
  const r = rig();
  const p = await r.provider.probe();
  assert.ok(!p.capabilities.includes("threadMoveWorkingDirectory"), "not on the descriptor");
  const { thread, q } = await startedThread(r);
  assert.ok(!((await r.provider.effective(thread)).capabilities ?? []).includes("threadMoveWorkingDirectory"), "not on a thread");
  q.push(result());
  await until(() => of(r.events, "turnCompleted")[0], "first turn");
  await r.provider.dispose();
});

test("a model switch uses setModel, effort uses applyFlagSettings, and the applied settings are reported", async () => {
  const r = rig();
  await r.provider.probe();
  const { thread, q } = await startedThread(r);
  q.push(result());
  await until(() => of(r.events, "turnCompleted")[0], "first turn");
  await r.provider.startTurn(thread, input("again"), { model: "stub-alias", effort: "stub-hi", mode: "acceptEdits" });
  assert.deepEqual(q.calls.filter(([c]) => c !== "close"), [
    ["setModel", "stub-alias"],
    ["applyFlagSettings", { effortLevel: "stub-hi" }],
    ["setPermissionMode", "acceptEdits"],
  ]);
  const changed = await until(() => of(r.events, "settingsChanged").at(-1), "settingsChanged");
  assert.deepEqual(changed.applied, { model: "stub-alias", effort: "stub-hi", mode: "acceptEdits" });
  const eff = await r.provider.effective(thread);
  assert.deepEqual((eff.efforts ?? []).map((e) => e.id), ["stub-lo", "stub-mid", "stub-hi"]);
  assert.deepEqual(eff.applied, { model: "stub-alias", effort: "stub-hi", mode: "acceptEdits" });
  assert.equal(eff.imageInput, "supported");
  q.push(result());
  await until(() => of(r.events, "turnCompleted")[1], "second turn");

  // An effort the live query refuses: the query is re-opened with resume, invisibly.
  q.failEffort = true;
  await r.provider.startTurn(thread, input("third"), { effort: "stub-lo" });
  assert.equal(r.sdk.queries.length, 3); // probe, first, re-opened
  const reopened = r.sdk.queries.at(-1)!;
  assert.equal(q.closed, true);
  assert.equal(reopened.options.resume, thread);
  assert.equal(reopened.options.sessionId, undefined);
  assert.equal(reopened.options.effort, "stub-lo");
  assert.equal(reopened.options.model, "stub-alias");
  assert.equal((await reopened.nextUser()).message.content, "third");
  await r.provider.dispose();
});

test("bypass mode is offered, marked skipsApprovals, and is the only mode so marked", async () => {
  const r = rig();
  const p = await r.provider.probe();
  const marked = p.modes.filter((m) => m.skipsApprovals);
  assert.deepEqual(marked.map((m) => m.id), ["bypassPermissions"]);
  assert.deepEqual(p.modes.filter((m) => m.isDefault).map((m) => m.id), ["default"]);
  // A new thread does not start in it, and its query is opened without the dangerous option.
  const { thread, q } = await startedThread(r);
  assert.equal(q.options.permissionMode, "default");
  assert.equal(q.options.allowDangerouslySkipPermissions, undefined);
  assert.equal(q.options.hooks, undefined);
  assert.equal((await r.provider.effective(thread)).applied?.mode, "default");
  await r.provider.dispose();
});

test("switching into bypass re-opens the query with allowDangerouslySkipPermissions; leaving drops it", async () => {
  const r = rig();
  await r.provider.probe();
  const { thread, q } = await startedThread(r);
  q.push(result());
  await until(() => of(r.events, "turnCompleted")[0], "first turn");

  await r.provider.startTurn(thread, input("go"), { mode: "bypassPermissions" });
  assert.equal(q.closed, true);
  assert.ok(!q.calls.some(([c]) => c === "setPermissionMode"), "bypass is not entered with setPermissionMode");
  const qb = r.sdk.queries.at(-1)!;
  assert.notEqual(qb, q);
  assert.equal(qb.options.resume, thread);
  assert.equal(qb.options.permissionMode, "bypassPermissions");
  assert.equal(qb.options.allowDangerouslySkipPermissions, true);
  assert.equal((await qb.nextUser()).message.content, "go");
  assert.equal(of(r.events, "settingsChanged").at(-1)!.applied.mode, "bypassPermissions");
  assert.equal((await r.provider.effective(thread)).applied?.mode, "bypassPermissions");
  qb.push(result());
  await until(() => of(r.events, "turnCompleted")[1], "bypass turn");

  // A turn with no mode override stays in bypass on the same query.
  await r.provider.startTurn(thread, input("more"), {});
  assert.equal(r.sdk.queries.at(-1), qb);
  qb.push(result());
  await until(() => of(r.events, "turnCompleted")[2], "third turn");

  await r.provider.startTurn(thread, input("careful now"), { mode: "acceptEdits" });
  assert.equal(qb.closed, true);
  const qa = r.sdk.queries.at(-1)!;
  assert.notEqual(qa, qb);
  assert.equal(qa.options.resume, thread);
  assert.equal(qa.options.permissionMode, "acceptEdits");
  assert.equal(qa.options.allowDangerouslySkipPermissions, undefined);
  assert.equal(qa.options.hooks, undefined);
  assert.equal((await qa.nextUser()).message.content, "careful now");

  // A resume after idle eviction keeps the thread's mode, and the option goes with it.
  const r2 = rig({ idleMs: 30 });
  const t2 = await r2.provider.startThread({ cwd: r2.dir, mode: "bypassPermissions" }, input("start in bypass, as asked"));
  const q2 = r2.sdk.queries.at(-1)!;
  assert.equal(q2.options.allowDangerouslySkipPermissions, true);
  q2.push(result());
  await until(() => q2.closed, "idle close");
  await r2.provider.startTurn(t2.thread, input("again"), {});
  const q3 = r2.sdk.queries.at(-1)!;
  assert.equal(q3.options.resume, t2.thread);
  assert.equal(q3.options.permissionMode, "bypassPermissions");
  assert.equal(q3.options.allowDangerouslySkipPermissions, true);
  await r.provider.dispose();
  await r2.provider.dispose();
});

test("in bypass mode AskUserQuestion still reaches the user, once, through the hook or canUseTool", async () => {
  const r = rig();
  const started = await r.provider.startThread({ cwd: r.dir, mode: "bypassPermissions" }, input("ask me"));
  const q = r.sdk.queries.at(-1)!;
  await q.nextUser();
  const matchers = q.options.hooks?.PreToolUse ?? [];
  assert.equal(matchers.length, 1);
  assert.equal(matchers[0]!.matcher, "AskUserQuestion");
  const hook = matchers[0]!.hooks[0]!;
  const askInput = { questions: [{ question: "Continue?", header: "Go", multiSelect: false, options: [{ label: "Yes", description: "" }, { label: "No", description: "" }] }] };
  const hookInput = { hook_event_name: "PreToolUse", tool_name: "AskUserQuestion", tool_input: askInput, tool_use_id: "toolu_ask", session_id: started.thread, transcript_path: "", cwd: r.dir } as never;

  // The hook asks; canUseTool, if Claude Code also calls it, waits for the same answer.
  const fromHook = hook(hookInput, "toolu_ask", { signal: signal() });
  const fromPerm = q.canUseTool("AskUserQuestion", askInput, { signal: signal(), toolUseID: "toolu_ask", requestId: "c" });
  const raised = await until(() => of(r.events, "requestRaised")[0], "requestRaised");
  assert.equal(raised.requestId, "toolu_ask");
  assert.equal(raised.request.requestKind, "question");
  await tick();
  assert.equal(of(r.events, "requestRaised").length, 1, "raised once");
  await r.provider.respond(started.thread, "toolu_ask", { requestKind: "question", answers: { q1: { selected: ["No"] } } });
  const answered = { ...askInput, answers: { "Continue?": "No" } };
  assert.deepEqual(await fromHook, { hookSpecificOutput: { hookEventName: "PreToolUse", permissionDecision: "allow", updatedInput: answered } });
  assert.deepEqual(await fromPerm, { behavior: "allow", updatedInput: answered });

  // canUseTool alone (no hook call) also raises the question.
  const alone = q.canUseTool("AskUserQuestion", askInput, { signal: signal(), toolUseID: "toolu_ask2", requestId: "c2" });
  await until(() => of(r.events, "requestRaised")[1], "second question");
  await r.provider.respond(started.thread, "toolu_ask2", { requestKind: "question", answers: { q1: { other: "later" } } });
  assert.deepEqual(await alone, { behavior: "allow", updatedInput: { ...askInput, answers: { "Continue?": "later" } } });

  // Other tools pass the hook untouched.
  assert.deepEqual(await hook({ ...(hookInput as object), tool_name: "Bash" } as never, "toolu_b", { signal: signal() }), {});
  await r.provider.dispose();
});

test("an idle query is closed and the thread resumes transparently on the next message", async () => {
  const r = rig({ idleMs: 40 });
  const { thread, q } = await startedThread(r);
  q.push(result());
  await until(() => of(r.events, "turnCompleted")[0], "turn");
  await until(() => q.closed, "idle close");
  assert.deepEqual(r.provider.liveThreads(), []);
  const turn2 = await r.provider.startTurn(thread, input("later"), {});
  const q2 = r.sdk.queries.at(-1)!;
  assert.notEqual(q2, q);
  assert.equal(q2.options.resume, thread);
  const user = await q2.nextUser();
  assert.equal(user.uuid, turn2);
  assert.equal(user.message.content, "later");
  // A query with an open request is never closed for being idle.
  void q2.canUseTool("Bash", { command: "x" }, { signal: signal(), toolUseID: "t", requestId: "c" });
  await until(() => of(r.events, "requestRaised")[0], "request");
  await new Promise((res) => setTimeout(res, 100));
  assert.equal(q2.closed, false);
  await r.provider.dispose();
});

test("at most maxLive queries stay open: the least recently used idle one is closed first, never a busy one", async () => {
  const r = rig({ maxLive: 2 });
  const a = await startedThread(r, "a");
  a.q.push(result());
  await until(() => of(r.events, "turnCompleted").length === 1, "a idle");
  const b = await startedThread(r, "b"); // b keeps a running turn
  const c = await startedThread(r, "c");
  assert.equal(a.q.closed, true, "a was idle and least recently used");
  assert.equal(b.q.closed, false);
  assert.deepEqual(new Set(r.provider.liveThreads()), new Set([b.thread, c.thread]));
  // Both live ones are busy (b runs a turn, c has an open request): a fourth exceeds the cap rather than close them.
  void c.q.canUseTool("Bash", { command: "x" }, { signal: signal(), toolUseID: "t1", requestId: "c" });
  await until(() => of(r.events, "requestRaised").length === 1, "request");
  const d = await startedThread(r, "d");
  assert.equal(b.q.closed, false);
  assert.equal(c.q.closed, false);
  assert.equal(d.q.closed, false);
  await r.provider.dispose();
});

test("a process that dies with a request open expires the request and fails the turn", async () => {
  const r = rig();
  const { turn, q } = await startedThread(r);
  const pending = q.canUseTool("Bash", { command: "rm -rf build" }, { signal: signal(), toolUseID: "toolu_d", requestId: "c" });
  await until(() => of(r.events, "requestRaised")[0], "request");
  q.crash("Claude Code process exited with code 1");
  const done = await until(() => of(r.events, "turnCompleted")[0], "turn end");
  assert.equal(done.turnId, turn);
  assert.equal(done.status, "failed");
  assert.match(done.error ?? "", /process ended/);
  assert.deepEqual(of(r.events, "requestExpired").map((e) => e.requestId), ["toolu_d"]);
  assert.equal((await pending)?.behavior, "deny");
  // The expiry comes before the end of the turn.
  const order = r.events.map((e) => e.type);
  assert.ok(order.indexOf("requestExpired") < order.indexOf("turnCompleted"));
  assert.deepEqual(r.provider.liveThreads(), []);
  await r.provider.dispose();
});

test("interrupt stops the turn and expires its open requests", async () => {
  const r = rig();
  const { thread, turn, q } = await startedThread(r);
  void q.canUseTool("Bash", { command: "sleep 100" }, { signal: signal(), toolUseID: "toolu_i", requestId: "c" });
  await until(() => of(r.events, "requestRaised")[0], "request");
  const stopping = r.provider.interrupt(thread, turn);
  await until(() => q.calls.some(([c]) => c === "interrupt"), "interrupt call");
  q.push(result({ subtype: "error_during_execution", is_error: true, errors: ["aborted"] }));
  await stopping;
  assert.equal(of(r.events, "turnCompleted")[0]!.status, "interrupted");
  assert.deepEqual(of(r.events, "requestExpired").map((e) => e.requestId), ["toolu_i"]);
  await r.provider.dispose();
});

test("in use elsewhere comes from claude agents --json; yaacd's own process does not count", async () => {
  const r = rig();
  const { thread, q } = await startedThread(r);
  r.cli.agentsList = [{ pid: q.pid, sessionId: thread, kind: "interactive", status: "idle" }];
  assert.deepEqual(await r.provider.control(thread), { external: false });
  r.cli.agentsList = [{ pid: q.pid, sessionId: thread, kind: "interactive" }, { pid: 1, sessionId: thread, kind: "interactive", status: "idle" }];
  // Take over (D140): an idle interactive holder can be taken over, and is named by its process.
  assert.deepEqual(await r.provider.control(thread), { external: true, holder: "another Claude Code session", holderIds: ["pid:1"] });
  r.cli.agentsList = [{ pid: 1, sessionId: thread, kind: "interactive", status: "busy" }];
  assert.deepEqual(await r.provider.control(thread), {
    external: true,
    holder: "another Claude Code session",
    holderIds: ["pid:1"],
    takeOverBlocked: "the other Claude Code session is running a turn; it can be taken over when that turn ends",
  });
  // A background holder refuses a resume (verified: `session_held_by_background`), so it cannot
  // be taken over; it is named by its id, not the shared background host's pid.
  r.cli.agentsList = [{ id: "bg1", sessionId: "other", kind: "background", state: "blocked" }, { pid: 99, id: "bg2", sessionId: thread, kind: "background", state: "done", status: "idle" }];
  assert.deepEqual(await r.provider.control(thread), {
    external: true,
    holder: "a Claude Code background session",
    holderIds: ["bg:bg2"],
    takeOverBlocked: "Claude Code runs it as a background session; it cannot be continued elsewhere until that session is stopped there (claude stop bg2)",
  });
  r.cli.agentsList = [{ id: "bg1", sessionId: "other", kind: "background" }];
  assert.deepEqual(await r.provider.control(thread), { external: false });
  r.cli.agentsList = [{ pid: 7, kind: "interactive" }];
  assert.deepEqual(await r.provider.control(thread), { external: false, uncertain: true });
  r.cli.agentsList = new Error("claude agents failed");
  assert.deepEqual(await r.provider.control(thread), { external: false, uncertain: true });
  await r.provider.dispose();
});

test("the mesh hook's MCP servers and system-prompt text reach the SDK options, again on resume", async () => {
  const r = rig({ idleMs: 30 });
  const seen: Array<{ native: string; resumed: boolean }> = [];
  const server = { type: "sdk" as const, name: "yaac", instance: {} as never };
  r.provider.setThreadExtras((t) => {
    seen.push({ native: t.native, resumed: t.resumed });
    return { mcpServers: { yaac: server }, appendSystemPrompt: `You are thread ${t.native}.` };
  });
  const { thread, q } = await startedThread(r);
  assert.equal(q.options.mcpServers?.yaac, server);
  assert.deepEqual(q.options.systemPrompt, { type: "preset", preset: "claude_code", append: `You are thread ${thread}.` });
  q.push(result());
  await until(() => q.closed, "idle close");
  await r.provider.startTurn(thread, input("again"), {});
  const q2 = r.sdk.queries.at(-1)!;
  assert.equal(q2.options.mcpServers?.yaac, server);
  assert.deepEqual(seen, [{ native: thread, resumed: false }, { native: thread, resumed: true }]);
  await r.provider.dispose();
});

test("images become image content blocks; the thread's attachment folder is readable to Claude Code", async () => {
  const r = rig();
  const uploads = tmp("yaac-claude-up-");
  const png = Buffer.concat([Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]), Buffer.from("rest")]);
  fs.writeFileSync(path.join(uploads, "1-shot.png"), png);
  const { thread, q } = await (async () => {
    const started = await r.provider.startThread(
      { cwd: r.dir },
      input("look", { images: [{ uploadId: "u1", path: path.join(uploads, "1-shot.png"), mediaType: null }] }),
    );
    const q = r.sdk.queries.at(-1)!;
    await q.nextUser();
    return { ...started, q };
  })();
  assert.deepEqual(q.options.additionalDirectories, [uploads]);
  const content = (await q.nextUser()).message.content as unknown as Array<Record<string, unknown>>;
  assert.deepEqual(content, [
    { type: "text", text: "look" },
    { type: "image", source: { type: "base64", media_type: "image/png", data: png.toString("base64") } },
  ]);
  q.push(result());
  await until(() => of(r.events, "turnCompleted")[0], "turn");
  // A file from another folder: the query is re-opened with both folders.
  const other = tmp("yaac-claude-up2-");
  await r.provider.startTurn(thread, input("read it", { files: [{ uploadId: "u2", path: path.join(other, "1-a.txt"), name: "a.txt" }] }), {});
  const q2 = r.sdk.queries.at(-1)!;
  assert.notEqual(q2, q);
  assert.deepEqual(new Set(q2.options.additionalDirectories), new Set([uploads, other]));
  await r.provider.dispose();
});

test("the thread index tells interactive from programmatic sessions; fork, rename, archive, delete use the session functions", async () => {
  const r = rig();
  r.sdk.sessions = [
    { sessionId: "s-term", summary: "From a terminal", lastModified: 2000, createdAt: 1000, cwd: "/w/a", firstPrompt: "fix it" },
    { sessionId: "s-sdk", summary: "From a script", customTitle: "Nightly", lastModified: 3000, cwd: "/w/b", archived: true },
  ];
  r.sdk.programmatic.add("s-sdk");
  const page1 = await r.provider.scanThreads({ limit: 1 });
  const page2 = await r.provider.scanThreads({ cursor: page1.cursor!, limit: 1 });
  assert.equal(page2.cursor, null);
  const all = [...page1.threads, ...page2.threads];
  assert.deepEqual(all.map((t) => [t.native, t.originKind, t.title, t.cwd, t.archived]), [
    ["s-term", "interactive", "From a terminal", "/w/a", false],
    ["s-sdk", "programmatic", "Nightly", "/w/b", true],
  ]);

  // Native fork at a turn: up to the last message of that turn.
  r.sdk.messages["s-term"] = [
    { type: "user", uuid: "t1", session_id: "s-term", message: { role: "user", content: "one" }, parent_tool_use_id: null, parent_agent_id: null },
    { type: "assistant", uuid: "a1", session_id: "s-term", message: { id: "m1", content: [{ type: "text", text: "1" }] }, parent_tool_use_id: null, parent_agent_id: null },
    { type: "user", uuid: "t2", session_id: "s-term", message: { role: "user", content: "two" }, parent_tool_use_id: null, parent_agent_id: null },
    { type: "assistant", uuid: "a2", session_id: "s-term", message: { id: "m2", content: [{ type: "text", text: "2" }] }, parent_tool_use_id: null, parent_agent_id: null },
  ];
  const forked = await r.provider.forkThread("s-term", { cwd: "/w/a", atTurn: "t1" });
  assert.equal(forked, "forked-session");
  assert.deepEqual(r.sdk.calls.find(([c]) => c === "forkSession"), ["forkSession", ["s-term", { upToMessageId: "a1" }]]);
  const page = await r.provider.readThread("s-term", {});
  assert.deepEqual(page.items.map((i) => [i.id, i.turnId, i.kind]), [
    ["user:t1", "t1", "userMessage"],
    ["m1#0", "t1", "agentMessage"],
    ["user:t2", "t2", "userMessage"],
    ["m2#0", "t2", "agentMessage"],
  ]);
  await r.provider.rename("s-term", "New title");
  await r.provider.archive("s-term");
  await r.provider.remove("s-term");
  assert.deepEqual(r.sdk.calls.filter(([c]) => c !== "forkSession").map(([c]) => c), ["renameSession", "archiveSession", "deleteSession"]);
  await r.provider.dispose();
});

test("history: unknown blocks are kept as generic tool calls, TodoWrite is a plan, agent prompts are notices", () => {
  const msg = (type: "user" | "assistant", uuid: string, content: unknown, extra: Record<string, unknown> = {}): SessionMessage =>
    ({ type, uuid, session_id: "s", message: { id: `m-${uuid}`, role: type, content }, parent_tool_use_id: null, parent_agent_id: null, ...extra }) as SessionMessage;
  const items = historyItems(
    [
      msg("user", "t1", [{ type: "text", text: "plan it" }, { type: "image", source: {} }]),
      msg("assistant", "a1", [{ type: "tool_use", id: "todo1", name: "TodoWrite", input: { todos: [{ content: "one", status: "completed", activeForm: "" }, { content: "two", status: "in_progress", activeForm: "" }] } }]),
      msg("assistant", "a2", [{ type: "tool_use", id: "mcp1", name: "mcp__srv__do_it", input: { x: 1 } }]),
      msg("user", "r2", [{ type: "tool_result", tool_use_id: "mcp1", content: [{ type: "text", text: "done" }], is_error: true }]),
      msg("assistant", "a3", [{ type: "mystery_block", data: 1 }]),
      msg("user", "t2", "<task-notification>finished</task-notification>", { origin: { kind: "task-notification" } }),
    ],
    null,
  );
  assert.deepEqual(items.map((i) => [i.id, i.kind, i.status]), [
    ["user:t1", "userMessage", "completed"],
    ["todo1", "plan", "completed"],
    ["mcp1", "toolCall", "failed"],
    ["m-a3#0", "toolCall", "completed"],
    ["user:t2", "notice", "completed"],
  ]);
  assert.deepEqual((items[0] as { parts: unknown }).parts, [{ type: "text", text: "plan it" }, { type: "text", text: "[image]" }]);
  assert.deepEqual((items[1] as { steps: unknown }).steps, [{ text: "one", state: "completed" }, { text: "two", state: "inProgress" }]);
  assert.deepEqual([(items[2] as { tool: string }).tool, (items[2] as { title: string }).title, (items[2] as { output: string }).output], ["mcp", "srv: do_it", "done"]);
  assert.equal((items[3] as { tool: string }).tool, "other");
});

// ── File changes as unified diffs (D86) ──────────────────────────────────────────────────

test("file changes carry unified diffs: an edit (structured patch, live), a multi-edit and a new file (history)", async () => {
  const { LiveTranslator } = await import("../src/providers/claude/translate.js");
  const events: Array<{ type: string; item: any }> = [];
  const tr = new LiveTranslator((e) => events.push(e as never), "/w");
  tr.begin("turn-1");
  tr.assistant({ id: "m1", content: [{ type: "tool_use", id: "toolu_e", name: "Edit", input: { file_path: "src/frame.rs", old_string: "let n = 1;", new_string: "let n = 2;" } }] }, "a1", null);
  tr.user(
    [{ type: "tool_result", tool_use_id: "toolu_e", content: "The file src/frame.rs has been updated.", is_error: false }],
    "u1",
    null,
    {
      filePath: "src/frame.rs",
      oldString: "let n = 1;",
      newString: "let n = 2;",
      originalFile: "fn f() {\n    let n = 1;\n    n\n}\n",
      structuredPatch: [{ oldStart: 1, oldLines: 4, newStart: 1, newLines: 4, lines: [" fn f() {", "-    let n = 1;", "+    let n = 2;", "     n", " }"] }],
      userModified: false,
      replaceAll: false,
    },
  );
  const edit = events.find((e) => e.type === "itemCompleted" && e.item.id === "toolu_e")!.item;
  assert.equal(edit.tool, "fileChange");
  assert.equal(edit.title, "Edited src/frame.rs");
  assert.equal(edit.output, "--- a/src/frame.rs\n+++ b/src/frame.rs\n@@ -1,4 +1,4 @@\n fn f() {\n-    let n = 1;\n+    let n = 2;\n     n\n }\n");

  const msg = (type: "user" | "assistant", uuid: string, content: unknown): SessionMessage =>
    ({ type, uuid, session_id: "s", message: { id: `m-${uuid}`, role: type, content }, parent_tool_use_id: null, parent_agent_id: null }) as SessionMessage;
  const items = historyItems(
    [
      msg("user", "t1", "change things"),
      msg("assistant", "a1", [
        { type: "tool_use", id: "me", name: "MultiEdit", input: { file_path: "a.txt", edits: [{ old_string: "one\ntwo", new_string: "one\n2" }, { old_string: "x", new_string: "y\nz" }] } },
        { type: "tool_use", id: "w", name: "Write", input: { file_path: "new.md", content: "# Title\n\nbody\n" } },
      ]),
      msg("user", "r1", [
        { type: "tool_result", tool_use_id: "me", content: "ok", is_error: false },
        { type: "tool_result", tool_use_id: "w", content: "File created", is_error: false },
      ]),
    ],
    null,
  ) as any[];
  const multi = items.find((i) => i.id === "me");
  assert.equal(multi.title, "Edited a.txt");
  assert.equal(multi.output, "--- a/a.txt\n+++ b/a.txt\n@@ -1,2 +1,2 @@\n one\n-two\n+2\n@@ -1 +1,2 @@\n-x\n+y\n+z\n");
  const created = items.find((i) => i.id === "w");
  assert.equal(created.title, "Created new.md");
  assert.equal(created.output, "--- /dev/null\n+++ b/new.md\n@@ -0,0 +1,3 @@\n+# Title\n+\n+body\n");
});

test("a line diff keeps three lines of context and splits distant changes into hunks", async () => {
  const { lineHunks } = await import("../src/providers/claude/diff.js");
  const before = Array.from({ length: 20 }, (_, i) => `l${i + 1}`).join("\n");
  const after = before.replace("l2", "L2").replace("l18", "L18");
  const hunks = lineHunks(before, after);
  assert.equal(hunks.length, 2);
  assert.deepEqual([hunks[0]!.oldStart, hunks[0]!.oldLines, hunks[1]!.oldStart, hunks[1]!.oldLines], [1, 5, 15, 6]);
  assert.deepEqual(hunks[0]!.lines, [" l1", "-l2", "+L2", " l3", " l4", " l5"]);
});

test("fast mode is Claude Code's fastMode setting: at open, then applyFlagSettings on the live query (D125)", async () => {
  const r = rig();
  await r.provider.probe();
  // Started in fast mode: the query opens with the flag setting (the SDK's opt-in too).
  const started = await r.provider.startThread({ cwd: r.dir, model: "stub-alias", fast: true }, input("go fast"));
  const q = r.sdk.queries.at(-1)!;
  await q.nextUser();
  assert.deepEqual(q.options.settings, { fastMode: true });
  // Claude Code reports the turn did not get fast mode: a notice in the transcript says why.
  q.push(result({ fast_mode_state: "off", fast_mode_disabled_reason: "extra_usage_disabled" }));
  await until(() => of(r.events, "turnCompleted")[0], "first turn");
  const notice = completedItems(r.events).find((i) => i.kind === "notice") as any;
  assert.equal(notice?.text, "Claude Code did not run this turn in fast mode (extra_usage_disabled). Fast mode is now off for this thread.");
  // A refusal turns the thread's fast setting off in yaacd (D133).
  assert.equal(of(r.events, "fastRefused").length, 1);
  // The same setting again: nothing to change.
  await r.provider.startTurn(started.thread, input("still fast"), { fast: true });
  assert.ok(!q.calls.some(([c]) => c === "applyFlagSettings"));
  await q.nextUser();
  q.push(result({ fast_mode_state: "on" }));
  await until(() => of(r.events, "turnCompleted")[1], "second turn");
  assert.equal(completedItems(r.events).filter((i) => i.kind === "notice").length, 1, "nothing to say when it ran fast");
  // Off: the live query's flag layer changes; no re-open.
  await r.provider.startTurn(started.thread, input("normal speed"), { fast: false });
  assert.deepEqual(q.calls.filter(([c]) => c === "applyFlagSettings"), [["applyFlagSettings", { fastMode: false }]]);
  assert.equal(r.sdk.queries.length, 2, "probe and the thread's query");
  await q.nextUser();
  q.push(result());
  await until(() => of(r.events, "turnCompleted")[2], "third turn");
  // A query opened later (here: after a refused change) opens without the setting.
  q.failEffort = true;
  await r.provider.startTurn(started.thread, input("fast again"), { fast: true });
  const reopened = r.sdk.queries.at(-1)!;
  assert.equal(reopened.options.resume, started.thread);
  assert.deepEqual(reopened.options.settings, { fastMode: true });
  // A thread that never asked for fast mode never sends the setting.
  const plain = await r.provider.startThread({ cwd: r.dir }, input("plain"));
  const pq = r.sdk.queries.at(-1)!;
  await pq.nextUser();
  assert.equal(pq.options.settings, undefined);
  void plain;
  await r.provider.dispose();
});
