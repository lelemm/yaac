// Tool calls that are communication with another agent → a structured `agentCall` on the
// tool call item (D150, docs/10-agent-mesh.md#agent-calls-in-the-transcript).
//
// Three families, verified against the installed tools (docs/research/harness-surfaces.md):
//   - YAAC's own mesh tools (the `yaac` MCP server), from either harness. Their results are
//     text `yaacd` itself wrote (src/mesh/mesh.ts); the phrases read here are that text's.
//   - Claude Code's sub-agent tools: `Agent` (formerly `Task`), `SendMessage`, `TaskStop` of
//     an agent, `SubagentHandback`.
//   - Codex's `collabAgentToolCall` items (spawnAgent, sendInput, sendMessage, followupTask,
//     resumeAgent, wait, closeAgent, interruptAgent, listAgents).
// The raw input and output stay on the item; this only adds a reading of them. Anything that
// does not parse leaves fields unset — never a guess.

import type { AgentCall, AgentCallKind, AgentRef } from "../protocol/generated/types.js";
import { SERVER_NAME } from "../mesh/tools.js";

type Json = Record<string, unknown>;
const str = (v: unknown): string | null => (typeof v === "string" && v.trim() ? v : null);
const obj = (v: unknown): Json => (v && typeof v === "object" && !Array.isArray(v) ? (v as Json) : {});
const arr = (v: unknown): unknown[] => (Array.isArray(v) ? v : []);

/** The name of YAAC's MCP server, as both harnesses see it (src/mesh/tools.ts). */
export const MESH_SERVER = SERVER_NAME;

function firstLine(text: string): string {
  return (text.split("\n").find((l) => l.trim()) ?? "").trim();
}

function clean<T extends AgentCall>(call: T): T {
  const c = call as unknown as Json;
  for (const k of Object.keys(c)) if (c[k] === undefined || c[k] === null || (Array.isArray(c[k]) && (c[k] as unknown[]).length === 0)) delete c[k];
  return call;
}

/** A short id for a line that has nothing better: the last part of an address, eight characters. */
export function shortId(id: string): string {
  const tail = id.includes("/") ? id.slice(id.lastIndexOf("/") + 1) : id;
  return tail.length > 8 ? tail.slice(0, 8) : tail;
}

function refLabel(r: AgentRef): string {
  if (r.parent) return "its parent";
  return r.name ?? (r.address ? shortId(r.address) : r.agentId ? shortId(r.agentId) : "an agent");
}

/** A readable one-line title, for a window that does not know `agentCall`. */
export function agentCallTitle(c: AgentCall, harnessOwn: boolean): string {
  const peers = (c.peers ?? []).map(refLabel).join(", ");
  const what = c.title ?? peers;
  switch (c.kind) {
    case "spawn":
      return `${harnessOwn ? "Subagent" : "Started"} ${what || "an agent"}`.trim();
    case "message":
      return `Message to ${peers || "an agent"}`;
    case "resume":
      return `Resumed ${peers || "an agent"}`;
    case "wait":
      return peers ? `Waiting for ${peers}` : "Waiting for messages";
    case "read":
      return `Read ${peers || "a thread"}`;
    case "stop":
      return peers ? `Stopped ${peers}` : "Stopped every worker";
    case "notify":
      return `Notify ${peers || "an agent"} when done`;
    case "list":
      return "Looked up agents";
    default:
      return peers || "Agent call";
  }
}

// ── YAAC's own mesh tools ────────────────────────────────────────────────────────────────

const MESH_KIND: Record<string, AgentCallKind> = {
  spawn_thread: "spawn",
  send_message: "message",
  wait: "wait",
  read_thread: "read",
  stop_thread: "stop",
  stop_all: "stop",
  notify_when_done: "notify",
  list_threads: "list",
  find_thread: "list",
};

/** The messages a `wait` handed over: `--- result from L (address A, id M):` then the text. */
function waitMessages(text: string): { from: string; label: string; id: string; kind: string; text: string }[] {
  const out: { from: string; label: string; id: string; kind: string; text: string }[] = [];
  const lines = text.split("\n");
  let cur: (typeof out)[number] | null = null;
  const body: string[] = [];
  const flush = () => {
    if (cur) out.push({ ...cur, text: body.join("\n").trim() });
    body.length = 0;
  };
  for (const l of lines) {
    const m = /^--- (result|message|notice from YAAC) from (.*) \(address (\S+), id (\S+)\):$/.exec(l);
    if (m) {
      flush();
      cur = { kind: m[1]!, label: m[2]!, from: m[3]!, id: m[4]!, text: "" };
      continue;
    }
    if (/^--- \(These came from/.test(l)) {
      flush();
      cur = null;
      continue;
    }
    if (cur) body.push(l);
  }
  flush();
  return out;
}

/**
 * A call of one of YAAC's own mesh tools. `result` is the text `yaacd` returned (null while
 * the call runs). Null for the tools that are not communication (`whoami`, `list_models`,
 * the pull-request tools).
 */
export function meshAgentCall(tool: string, args: Json, result: string | null, isError = false): AgentCall | null {
  const kind = MESH_KIND[tool];
  if (!kind) return null;
  const addr = (v: unknown): AgentRef[] => (str(v) ? [{ address: str(v)! }] : []);
  const call: AgentCall = { kind };
  switch (tool) {
    case "spawn_thread":
      call.text = str(args.text);
      call.title = str(args.title);
      call.model = str(args.model);
      call.effort = str(args.effort);
      call.target = str(args.target);
      break;
    case "send_message":
      call.peers = addr(args.to);
      call.text = str(args.text) ?? str(args.message);
      break;
    case "wait":
      call.peers = arr(args.threads).flatMap(addr);
      break;
    case "read_thread":
    case "stop_thread":
      call.peers = addr(args.thread);
      break;
    case "notify_when_done":
      call.peers = addr(args.to);
      break;
    case "list_threads":
    case "find_thread":
      call.text = str(args.query);
      break;
  }
  if (result === null) return clean(call);
  if (isError) {
    call.outcome = "failed";
    call.detail = firstLine(result);
    return clean(call);
  }
  switch (tool) {
    case "spawn_thread": {
      const a = /^Address: (\S+)$/m.exec(result);
      if (a) call.peers = [{ address: a[1]! }];
      call.outcome = "started";
      break;
    }
    case "send_message": {
      const id = /^Message id: (\S+)$/m.exec(result);
      if (id) call.messages = [id[1]!];
      const first = firstLine(result);
      const why = /^[^—]*— (.*?)(\. It is stored.*)?\.?$/.exec(first)?.[1];
      if (first.startsWith("Delivered")) call.outcome = "delivered";
      else if (first.startsWith("Queued")) call.outcome = "queued";
      else if (first.startsWith("Failed")) call.outcome = "failed";
      if (why) call.detail = why.replace(/\.$/, "");
      break;
    }
    case "wait": {
      const msgs = waitMessages(result);
      if (msgs.length > 0) {
        call.outcome = "returned";
        call.messages = msgs.map((m) => m.id);
        const senders: AgentRef[] = [];
        for (const m of msgs) if (!senders.some((s) => s.address === m.from)) senders.push({ address: m.from, name: m.label });
        call.peers = senders;
        call.result = msgs.map((m) => `**${m.kind === "result" ? "Result" : m.kind === "message" ? "Message" : "Notice"} from ${m.label}**\n\n${m.text}`).join("\n\n");
      } else if (/^Nothing yet after/.test(result) || /^The wait was cancelled/.test(result) || /^Another wait replaced/.test(result)) {
        call.outcome = "nothingYet";
        call.detail = firstLine(result);
      } else {
        const done = [...result.matchAll(/^(.*) \((\S+)\) (?:is|no longer) /gm)];
        if (done.length > 0) call.peers = done.map((m) => ({ address: m[2]!, name: m[1]! }));
        call.outcome = "returned";
        call.result = result.replace(/\nUse read_thread for its answer.*$/s, "").trim();
      }
      break;
    }
    case "read_thread":
    case "list_threads":
    case "find_thread":
      call.result = result;
      break;
    case "stop_thread":
    case "stop_all":
      call.outcome = /^You have no workers/.test(result) ? "notRunning" : "stopped";
      call.result = result;
      break;
    case "notify_when_done":
      call.outcome = "registered";
      break;
  }
  return clean(call);
}

/** `mcp__yaac__send_message` → `send_message`; null for any other tool. */
export function meshToolOf(claudeName: string): string | null {
  const prefix = `mcp__${MESH_SERVER}__`;
  return claudeName.startsWith(prefix) ? claudeName.slice(prefix.length) : null;
}

// ── Claude Code's sub-agent tools ────────────────────────────────────────────────────────

function parseJson(text: string | null): Json | null {
  if (!text) return null;
  try {
    const v = JSON.parse(text);
    return v && typeof v === "object" && !Array.isArray(v) ? (v as Json) : null;
  } catch {
    return null;
  }
}

/** An `Agent` tool's answer text, without the trailing bookkeeping Claude Code adds for the model. */
function agentAnswer(text: string): string {
  return text
    .replace(/<usage>[\s\S]*?<\/usage>/g, "")
    .replace(/\n*agentId: \S+[^\n]*(\n|$)/g, "\n")
    .trim();
}

/**
 * A call of one of Claude Code's own agent tools. `result` is the tool result's text (null
 * while it runs); `structured` is the SDK's `tool_use_result`, when it came with it.
 */
export function claudeAgentCall(name: string, input: Json, result: string | null, structured?: unknown, isError = false): AgentCall | null {
  const mesh = meshToolOf(name);
  if (mesh) return meshAgentCall(mesh, obj(input.arguments ?? input), result, isError);
  const s = obj(structured);
  switch (name) {
    case "Agent":
    case "Task": {
      const call: AgentCall = {
        kind: "spawn",
        title: str(input.description) ?? str(input.name) ?? str(input.subagent_type),
        text: str(input.prompt),
        model: str(input.model),
        effort: str(input.effort),
        background: input.run_in_background === true || input.run_in_background === "true" ? true : null,
      };
      const peer: AgentRef = { name: str(input.description) ?? str(input.name) ?? undefined };
      if (result !== null) {
        const agentId = str(s.agentId) ?? str(s.taskId) ?? /agentId: (\w+)/.exec(result)?.[1] ?? null;
        if (agentId) peer.agentId = agentId;
        if (!call.model && str(s.resolvedModel)) call.model = str(s.resolvedModel);
        if (isError) {
          call.outcome = "failed";
          call.detail = firstLine(result);
        } else if (s.status === "async_launched" || s.status === "remote_launched" || /^Async agent launched/.test(result)) {
          call.outcome = "started";
          call.background = true;
        } else {
          call.outcome = "completed";
          const content = arr(s.content).map((b) => str(obj(b).text) ?? "").filter(Boolean).join("\n\n");
          call.result = content || agentAnswer(result);
        }
      }
      call.peers = [peer];
      return clean(call);
    }
    case "SendMessage": {
      const to = str(input.to) ?? str(input.recipient);
      const call: AgentCall = { kind: "message", text: str(input.message) ?? str(input.content), title: str(input.summary) };
      const peer: AgentRef = to === "main" || input.recipient_kind === "main" ? { parent: true } : { agentId: to ?? undefined };
      const r = parseJson(result) ?? (Object.keys(s).length ? s : null);
      if (r) {
        const pin = obj(r.pin);
        if (!peer.parent) {
          peer.agentId = str(r.resumedAgentId) ?? str(pin.id) ?? peer.agentId;
          const pinName = str(pin.name);
          if (pinName && pinName !== peer.agentId) peer.name = pinName;
        }
        const message = str(r.message) ?? "";
        if (r.success === false || isError) {
          call.outcome = "failed";
          call.detail = firstLine(message || result || "");
        } else if (str(r.resumedAgentId) || /^Resuming agent/.test(message)) {
          call.outcome = "resumed";
        } else if (/^Message queued/.test(message)) {
          call.outcome = "queued";
          call.detail = /next tool round/.test(message) ? "it reads it at its next step" : /next turn/.test(message) ? "it reads it at its next turn" : null;
        } else call.outcome = "delivered";
      } else if (result !== null) {
        call.outcome = isError ? "failed" : "delivered";
        if (isError) call.detail = firstLine(result);
      }
      call.peers = [peer];
      return clean(call);
    }
    case "SubagentHandback": {
      const call: AgentCall = { kind: "message", text: str(input.message), peers: [{ parent: true }] };
      if (result !== null) {
        const r = parseJson(result);
        call.outcome = isError || r?.success === false ? "failed" : "delivered";
        if (call.outcome === "failed") call.detail = firstLine(str(r?.message) ?? result);
      }
      return clean(call);
    }
    case "TaskStop": {
      // A background shell is stopped with the same tool: only an agent's stop is communication.
      const r = parseJson(result) ?? s;
      if (r.task_type !== "local_agent") return null;
      const id = str(r.task_id) ?? str(input.task_id) ?? str(input.shell_id);
      return clean({ kind: "stop", peers: [{ agentId: id ?? undefined, name: str(r.command) ?? undefined }], outcome: isError ? "failed" : "stopped" });
    }
  }
  return null;
}

// ── Codex's collaboration tools ──────────────────────────────────────────────────────────

const CODEX_KIND: Record<string, AgentCallKind> = {
  spawnAgent: "spawn",
  sendInput: "message",
  sendMessage: "message",
  followupTask: "message",
  resumeAgent: "resume",
  wait: "wait",
  closeAgent: "stop",
  interruptAgent: "stop",
  listAgents: "list",
};

/** A Codex `collabAgentToolCall` item (app-server v2 `ThreadItem`). */
export function codexAgentCall(it: Json): AgentCall | null {
  const tool = str(it.tool) ?? "";
  const kind = CODEX_KIND[tool];
  if (!kind) return null;
  const ids = arr(it.receiverThreadIds).filter((x): x is string => typeof x === "string");
  const states = obj(it.agentsStates);
  const call: AgentCall = { kind, peers: ids.map((agentId) => ({ agentId })), text: str(it.prompt), model: str(it.model), effort: str(it.reasoningEffort) };
  const status = str(it.status);
  if (status === "failed") {
    call.outcome = "failed";
    const why = ids.map((id) => str(obj(states[id]).message)).find(Boolean);
    if (why) call.detail = firstLine(why);
    return clean(call);
  }
  if (status !== "completed") return clean(call);
  const st = ids.map((id) => ({ id, status: str(obj(states[id]).status), message: str(obj(states[id]).message) }));
  switch (kind) {
    case "spawn":
      call.outcome = st.some((x) => x.status === "errored" || x.status === "notFound") ? "failed" : "started";
      break;
    case "message":
      call.outcome = st.some((x) => x.status === "notFound") ? "failed" : "delivered";
      break;
    case "resume":
      call.outcome = "resumed";
      break;
    case "stop":
      call.outcome = "stopped";
      break;
    case "wait": {
      const finished = st.filter((x) => x.status === "completed" || x.status === "errored" || x.status === "shutdown" || x.status === "interrupted");
      call.outcome = finished.length > 0 ? "returned" : "nothingYet";
      const texts = finished.map((x) => x.message).filter((m): m is string => !!m);
      if (texts.length > 0) call.result = texts.join("\n\n");
      if (finished.length > 0) call.peers = finished.map((x) => ({ agentId: x.id }));
      break;
    }
  }
  return clean(call);
}
