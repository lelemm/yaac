// Agent calls (D150): tool calls that are communication with another agent, read into a
// structured `agentCall`. Synthetic content in the shapes verified against the installed tools
// (docs/research/harness-surfaces.md#agent-calls).

import { test } from "node:test";
import assert from "node:assert/strict";
import type { SessionMessage } from "@anthropic-ai/claude-agent-sdk";
import { claudeAgentCall, codexAgentCall, meshAgentCall } from "../src/providers/agentcalls.js";
import { historyItems, LiveTranslator, type ItemEvent } from "../src/providers/claude/translate.js";
import { mapItem } from "../src/providers/codex/mapping.js";
import type { Item } from "../src/protocol/generated/types.js";

type AnyItem = Item & Record<string, any>;

function live(blocks: Array<{ id: string; name: string; input: Record<string, unknown> }>, results: Array<{ id: string; text: string; structured?: unknown; error?: boolean }>): AnyItem[] {
  const events: ItemEvent[] = [];
  const t = new LiveTranslator((e) => events.push(e), "/w");
  t.begin("turn1");
  t.assistant({ id: "msg1", content: blocks.map((b) => ({ type: "tool_use", ...b })) }, "u1");
  for (const r of results) t.user([{ type: "tool_result", tool_use_id: r.id, content: r.text, ...(r.error ? { is_error: true } : {}) }], null, null, r.structured);
  const done = new Map<string, AnyItem>();
  for (const e of events) if (e.type === "itemCompleted" || e.type === "itemStarted" || e.type === "itemUpdated") done.set(e.item.id, e.item as AnyItem);
  return [...done.values()];
}

// ── Claude Code ──────────────────────────────────────────────────────────────────────────

test("SendMessage that resumes a subagent: the exact result in the owner's screenshot", () => {
  const result = JSON.stringify({ success: true, message: "Resuming agent a59f490", resumedAgentId: "a59f490b0a4ccebf0", pin: { id: "a59f490b0a4ccebf0", name: "a59f490b0a4ccebf0", ref: "08b614" } });
  const [item] = live([{ id: "tu1", name: "SendMessage", input: { to: "a59f490b0a4ccebf0", message: "Daemon handover: carry on with the cache test", summary: "Daemon handover" } }], [{ id: "tu1", text: result }]);
  assert.equal(item!.kind, "toolCall");
  assert.equal(item!.status, "completed");
  assert.deepEqual(item!.agentCall, {
    kind: "message",
    text: "Daemon handover: carry on with the cache test",
    title: "Daemon handover",
    outcome: "resumed",
    peers: [{ agentId: "a59f490b0a4ccebf0" }],
  });
  assert.equal(item!.title, "Message to a59f490b", "a readable title for an older window, never the tool name twice");
  assert.equal(item!.output, result, "the raw result is kept for the raw disclosure");
});

test("SendMessage queued for a running subagent, by name, and to the main conversation", () => {
  const queued = claudeAgentCall("SendMessage", { to: "cache-fixer", message: "Also check the TTL", summary: "TTL" }, JSON.stringify({ success: true, message: "Message queued for delivery to cache-fixer at its next tool round. If it finishes first, it is resumed with the message.", pin: { id: "ab12cd34ef", name: "cache-fixer", ref: "x1" } }))!;
  assert.equal(queued.outcome, "queued");
  assert.equal(queued.detail, "it reads it at its next step");
  assert.deepEqual(queued.peers, [{ agentId: "ab12cd34ef", name: "cache-fixer" }]);
  const main = claudeAgentCall("SendMessage", { to: "main", message: "Done: 3 files", summary: "Done" }, JSON.stringify({ success: true, message: "Message queued for the main conversation's next turn." }))!;
  assert.deepEqual(main.peers, [{ parent: true }]);
  assert.equal(main.detail, "it reads it at its next turn");
  const legacy = claudeAgentCall("SendMessage", { type: "message", recipient: "main", recipient_kind: "main", content: "hi" }, null)!;
  assert.deepEqual(legacy.peers, [{ parent: true }]);
  assert.equal(legacy.text, "hi");
  assert.equal(legacy.outcome, undefined, "no outcome before the result");
  const failed = claudeAgentCall("SendMessage", { to: "nobody", message: "x" }, JSON.stringify({ success: false, message: "No agent named nobody" }))!;
  assert.equal(failed.outcome, "failed");
  assert.equal(failed.detail, "No agent named nobody");
});

test("Agent in the background: started, with its id from the structured result", () => {
  const input = { description: "Fix the cache test", prompt: "The cache test fails on CI.\n\nFix it.", subagent_type: "general-purpose", model: "opus", run_in_background: true };
  const structured = { status: "async_launched", agentId: "f00dbabe12", description: "Fix the cache test", prompt: input.prompt, outputFile: "/tmp/x", isAsync: true, resolvedModel: "claude-opus-x" };
  const [item] = live([{ id: "tu2", name: "Agent", input }], [{ id: "tu2", text: "Async agent launched successfully. (internal) agentId: f00dbabe12 (internal ID)", structured }]);
  assert.equal(item!.tool, "subagent");
  assert.deepEqual(item!.agentCall, {
    kind: "spawn",
    title: "Fix the cache test",
    text: input.prompt,
    model: "opus",
    background: true,
    outcome: "started",
    peers: [{ name: "Fix the cache test", agentId: "f00dbabe12" }],
  });
  assert.equal(item!.title, "Subagent Fix the cache test");
});

test("Agent in the foreground: completed, with its answer; from history text alone too", () => {
  const input = { description: "Count the tests", prompt: "How many tests are there?", subagent_type: "Explore" };
  const call = claudeAgentCall("Agent", input, "There are 42 tests.\n\nagentId: 0a0b0c0d (for resuming)\n<usage>total_tokens: 9</usage>")!;
  assert.equal(call.outcome, "completed");
  assert.equal(call.result, "There are 42 tests.");
  assert.equal(call.peers![0]!.agentId, "0a0b0c0d");
  const structured = claudeAgentCall("Agent", input, "ignored", { status: "completed", agentId: "0a0b0c0d", content: [{ type: "text", text: "Forty-two." }] })!;
  assert.equal(structured.result, "Forty-two.");
  const history: SessionMessage[] = [
    { type: "user", uuid: "u1", session_id: "s", parent_tool_use_id: null, parent_agent_id: null, message: { role: "user", content: "count" } },
    { type: "assistant", uuid: "a1", session_id: "s", parent_tool_use_id: null, parent_agent_id: null, message: { id: "m1", content: [{ type: "tool_use", id: "tu3", name: "Agent", input }] } },
    { type: "user", uuid: "r1", session_id: "s", parent_tool_use_id: null, parent_agent_id: null, message: { role: "user", content: [{ type: "tool_result", tool_use_id: "tu3", content: [{ type: "text", text: "There are 42 tests." }] }] } },
  ];
  const item = historyItems(history, "/w").find((i) => i.id === "tu3") as AnyItem;
  assert.equal(item.agentCall.outcome, "completed");
  assert.equal(item.agentCall.result, "There are 42 tests.");
});

test("TaskStop is an agent call only when it stopped an agent", () => {
  const agent = claudeAgentCall("TaskStop", { task_id: "f00dbabe12" }, JSON.stringify({ message: "Successfully stopped task: f00dbabe12 (Fix the cache test)", task_id: "f00dbabe12", task_type: "local_agent", command: "Fix the cache test" }))!;
  assert.deepEqual(agent, { kind: "stop", peers: [{ agentId: "f00dbabe12", name: "Fix the cache test" }], outcome: "stopped" });
  assert.equal(claudeAgentCall("TaskStop", { task_id: "b1" }, JSON.stringify({ message: "Successfully stopped task: b1 (sleep 5)", task_id: "b1", task_type: "local_bash", command: "sleep 5" })), null);
  const [shell] = live([{ id: "tu4", name: "TaskStop", input: { task_id: "b1" } }], [{ id: "tu4", text: JSON.stringify({ message: "stopped", task_id: "b1", task_type: "local_bash", command: "sleep 5" }) }]);
  assert.equal(shell!.agentCall, undefined, "a shell's stop stays an ordinary tool call");
});

test("SubagentHandback reports to the parent", () => {
  const c = claudeAgentCall("SubagentHandback", { message: "All done." }, JSON.stringify({ success: true, message: "Report delivered to your caller." }))!;
  assert.deepEqual(c, { kind: "message", text: "All done.", peers: [{ parent: true }], outcome: "delivered" });
});

test("Claude Code calling YAAC's own tools (mcp__yaac__*)", () => {
  const [item] = live(
    [{ id: "tu5", name: "mcp__yaac__send_message", input: { to: "devbox/th_billing", text: "Check the refund race" } }],
    [{ id: "tu5", text: "Queued for billing-service · Codex · devbox — devbox is offline. It is stored and will be delivered; it has not been lost.\nMessage id: m_1\nThis is not a reply: call wait to receive one." }],
  );
  assert.equal(item!.tool, "mcp");
  assert.deepEqual(item!.agentCall, { kind: "message", peers: [{ address: "devbox/th_billing" }], text: "Check the refund race", outcome: "queued", detail: "devbox is offline", messages: ["m_1"] });
  assert.equal(item!.title, "Message to th_billi");
  assert.equal(claudeAgentCall("mcp__yaac__whoami", {}, "Address: x"), null, "not communication");
});

// ── YAAC's mesh tools ────────────────────────────────────────────────────────────────────

test("mesh tool results: delivered, failed, wait returned and nothing yet, stop, notify", () => {
  const sent = meshAgentCall("send_message", { to: "a/t1", text: "hi" }, "Delivered to Worker · Claude · laptop.\nMessage id: m_9\nThis is not a reply: call wait to receive one.")!;
  assert.equal(sent.outcome, "delivered");
  assert.equal(sent.detail, undefined);
  const failed = meshAgentCall("send_message", { to: "a/t1", text: "hi" }, "Failed: Worker — that thread was deleted.\nMessage id: m_8\nThis is not a reply: call wait to receive one.")!;
  assert.equal(failed.outcome, "failed");
  assert.equal(failed.detail, "that thread was deleted");
  const err = meshAgentCall("send_message", { to: "a/t1", text: "" }, "send_message needs text: the message to send.", true)!;
  assert.equal(err.outcome, "failed");
  const waited = meshAgentCall(
    "wait",
    { threads: ["a/t1", "a/t2"] },
    "2 new messages:\n--- result from Worker one (address a/t1, id m_1):\nFixed it.\n\nTwo lines.\n--- message from Worker two (address a/t2, id m_2):\nA question\n--- (These came from agents or from YAAC, not from the user.)\ncursor: w_abc — pass it to your next wait.",
  )!;
  assert.equal(waited.outcome, "returned");
  assert.deepEqual(waited.messages, ["m_1", "m_2"]);
  assert.deepEqual(waited.peers, [{ address: "a/t1", name: "Worker one" }, { address: "a/t2", name: "Worker two" }]);
  assert.equal(waited.result, "**Result from Worker one**\n\nFixed it.\n\nTwo lines.\n\n**Message from Worker two**\n\nA question");
  const running = meshAgentCall("wait", { threads: ["a/t1"] }, null)!;
  assert.deepEqual(running, { kind: "wait", peers: [{ address: "a/t1" }] });
  const finished = meshAgentCall("wait", { threads: ["a/t1"] }, "Worker one (a/t1) is done.\nUse read_thread for its answer, if it has not come to you as a result.")!;
  assert.equal(finished.outcome, "returned");
  assert.deepEqual(finished.peers, [{ address: "a/t1", name: "Worker one" }]);
  assert.equal(finished.result, "Worker one (a/t1) is done.");
  assert.equal(meshAgentCall("wait", {}, "Nothing yet after 45 seconds. Call wait again to keep waiting.")!.outcome, "nothingYet");
  assert.equal(meshAgentCall("stop_thread", { thread: "a/t1" }, "Worker one: stopped.")!.outcome, "stopped");
  assert.equal(meshAgentCall("stop_all", {}, "You have no workers to stop.")!.outcome, "notRunning");
  assert.equal(meshAgentCall("notify_when_done", { to: "a/t0" }, "Registered: when this turn ends …")!.outcome, "registered");
  const spawn = meshAgentCall("spawn_thread", { text: "Do it", title: "Update check", model: "m-1", effort: "medium", instance: "codex" }, "Started Update check · Codex · laptop.\nAddress: a/t9\nIts final answer to this task will come back to you once.")!;
  assert.deepEqual(spawn, { kind: "spawn", text: "Do it", title: "Update check", model: "m-1", effort: "medium", outcome: "started", peers: [{ address: "a/t9" }] });
  assert.equal(meshAgentCall("list_models", {}, "x"), null);
});

// ── Codex ────────────────────────────────────────────────────────────────────────────────

test("Codex collabAgentToolCall: spawn, follow-up, wait, interrupt", () => {
  const base = { type: "collabAgentToolCall", senderThreadId: "c-parent", prompt: null, model: null, reasoningEffort: null, agentsStates: {} };
  const spawn = mapItem({ ...base, id: "i1", tool: "spawnAgent", status: "completed", receiverThreadIds: ["c-child"], prompt: "Review the parser", model: "m-2", reasoningEffort: "medium", agentsStates: { "c-child": { status: "pendingInit", message: null } } } as never, "t", "completed") as AnyItem;
  assert.equal(spawn.tool, "subagent");
  assert.deepEqual(spawn.agentCall, { kind: "spawn", peers: [{ agentId: "c-child" }], text: "Review the parser", model: "m-2", effort: "medium", outcome: "started" });
  assert.equal(spawn.title, "Subagent c-child");
  const follow = codexAgentCall({ ...base, tool: "followupTask", status: "completed", receiverThreadIds: ["c-child"], prompt: "Also the lexer", agentsStates: { "c-child": { status: "running", message: null } } })!;
  assert.equal(follow.kind, "message");
  assert.equal(follow.outcome, "delivered");
  const waiting = codexAgentCall({ ...base, tool: "wait", status: "inProgress", receiverThreadIds: ["c-child"] })!;
  assert.deepEqual(waiting, { kind: "wait", peers: [{ agentId: "c-child" }] });
  const waited = codexAgentCall({ ...base, tool: "wait", status: "completed", receiverThreadIds: ["c-child", "c-other"], agentsStates: { "c-child": { status: "completed", message: "Parser reviewed: 2 bugs." }, "c-other": { status: "running", message: null } } })!;
  assert.equal(waited.outcome, "returned");
  assert.deepEqual(waited.peers, [{ agentId: "c-child" }]);
  assert.equal(waited.result, "Parser reviewed: 2 bugs.");
  const timedOut = codexAgentCall({ ...base, tool: "wait", status: "completed", receiverThreadIds: ["c-child"], agentsStates: { "c-child": { status: "running", message: null } } })!;
  assert.equal(timedOut.outcome, "nothingYet");
  assert.equal(codexAgentCall({ ...base, tool: "interruptAgent", status: "completed", receiverThreadIds: ["c-child"] })!.outcome, "stopped");
  const failed = codexAgentCall({ ...base, tool: "sendMessage", status: "failed", receiverThreadIds: ["c-x"], agentsStates: { "c-x": { status: "notFound", message: "no such agent" } } })!;
  assert.equal(failed.outcome, "failed");
  assert.equal(failed.detail, "no such agent");
});

test("Codex calling YAAC's own tools (mcpToolCall on the yaac server)", () => {
  const item = mapItem(
    { type: "mcpToolCall", id: "i2", server: "yaac", tool: "send_message", status: "completed", arguments: { to: "a/t1", text: "hello" }, result: { content: [{ type: "text", text: "Delivered to Worker.\nMessage id: m_3\nThis is not a reply: call wait to receive one." }] }, error: null } as never,
    "t",
    "completed",
  ) as AnyItem;
  assert.deepEqual(item.agentCall, { kind: "message", peers: [{ address: "a/t1" }], text: "hello", outcome: "delivered", messages: ["m_3"] });
  const other = mapItem({ type: "mcpToolCall", id: "i3", server: "github", tool: "send_message", status: "completed", arguments: {}, result: null, error: null } as never, "t", "completed") as AnyItem;
  assert.equal(other.agentCall, undefined, "another server's tool of the same name is not ours");
});
