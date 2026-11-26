// Claude Code messages → the YAAC item model (docs/03-protocol.md#the-item-model).
//
// The same rules serve the live stream and stored history, so that an item has the same id
// either way (yaacd overlays history with its journal by item id):
//   - a turn's id is the uuid of the user message that began it (yaacd sets that uuid itself
//     on the messages it sends), and its user item is `user:<uuid>`;
//   - text and thinking are `<API message id>#<block index>`;
//   - a tool call (and a TodoWrite plan) is the tool_use id; an AskUserQuestion is the request
//     item `request:<tool_use id>`, because yaacd raises that question with the tool_use id as
//     its request id.
// Anything that does not map becomes a `toolCall` of type `other`, or a `notice`; nothing is
// dropped.

import type { AgentCall, AgentRef, Item, PlanStep, ToolKind, TurnId } from "../../protocol/generated/types.js";
import { isImageBlock, resultImages, type StoreToolImage } from "../tool-images.js";
import type { SessionMessage } from "@anthropic-ai/claude-agent-sdk";
import { ASK_TOOL, normaliseQuestions } from "./questions.js";
import { editDiff, editedPath } from "./diff.js";
import { cleanToolOutput, isInterruptedMarker, promptItem, unwrapToolError, type PromptFlags } from "./markup.js";
import { agentCallTitle, claudeAgentCall, meshToolOf } from "../agentcalls.js";
import { catalogSkillId } from "../../mesh/tools.js";

export { promptItem };

type Json = Record<string, unknown>;
type Block = Json & { type?: unknown };

const str = (v: unknown): string | null => (typeof v === "string" ? v : null);
const obj = (v: unknown): Json => (v && typeof v === "object" && !Array.isArray(v) ? (v as Json) : {});

export const TODO_TOOL = "TodoWrite";

export function describeTool(name: string, input: Json): { tool: ToolKind; title: string } {
  const p = str(input.file_path) ?? str(input.notebook_path) ?? str(input.path);
  switch (name) {
    case "Bash":
    case "PowerShell":
      return { tool: "command", title: str(input.command) ?? name };
    case "Edit":
    case "MultiEdit":
    case "NotebookEdit":
      return { tool: "fileChange", title: `Edit ${p ?? ""}`.trim() };
    case "Write":
      return { tool: "fileChange", title: `Write ${p ?? ""}`.trim() };
    case "Read":
      return { tool: "other", title: `Read ${p ?? ""}`.trim() };
    case "Glob":
    case "Grep":
      return { tool: "other", title: `${name} ${str(input.pattern) ?? ""}`.trim() };
    case "WebSearch":
      return { tool: "webSearch", title: str(input.query) ?? name };
    case "WebFetch":
      return { tool: "webSearch", title: str(input.url) ?? name };
    case "Skill":
      return { tool: "other", title: str(input.skill) ? `Skill ${str(input.skill)}` : "Skill" };
    case "Agent":
    case "Task":
      return { tool: "subagent", title: str(input.description) ?? str(input.subagent_type) ?? name };
  }
  if (name.startsWith("mcp__")) {
    const [, server, ...rest] = name.split("__");
    return { tool: "mcp", title: `${server ?? ""}: ${rest.join("__")}` };
  }
  return { tool: "other", title: name };
}

export function todoSteps(input: Json): PlanStep[] {
  const todos = Array.isArray(input.todos) ? (input.todos as Json[]) : [];
  return todos.map((t) => ({
    text: str(t.content) ?? str(t.activeForm) ?? "",
    state: t.status === "completed" ? "completed" : t.status === "in_progress" ? "inProgress" : "pending",
  }));
}

/** The text of a tool_result's content (a string, or text and image blocks). */
export function resultText(content: unknown): string {
  return cleanToolOutput(unwrapToolError(rawResultText(content)));
}

function rawResultText(content: unknown): string {
  if (typeof content === "string") return content;
  if (!Array.isArray(content)) return content == null ? "" : JSON.stringify(content);
  return (content as Block[])
    .map((b) => (b?.type === "text" ? (str(b.text) ?? "") : isImageBlock(b) ? "[image]" : JSON.stringify(b)))
    .join("\n");
}

export function toolCallItem(id: string, turnId: TurnId, name: string, input: Json, cwd: string | null): Item {
  const d = describeTool(name, input);
  const item = { id, turnId, status: "inProgress", kind: "toolCall", tool: d.tool, title: d.title, input: { name, ...input }, ...(cwd && d.tool === "command" ? { cwd } : {}) } as Item;
  return withAgentCall(item, name, input, null);
}

/** Communication with another agent (D150): the call read as an `agentCall`, and a readable title. */
function withAgentCall(item: Item, name: string, input: Json, result: string | null, structured?: unknown, failed = false): Item {
  const call = claudeAgentCall(name, input, result, structured, failed);
  const it = item as Item & { agentCall?: unknown; title?: string };
  if (!call) {
    delete it.agentCall;
    return item;
  }
  return { ...item, agentCall: call, title: agentCallTitle(call, !meshToolOf(name)) } as Item;
}

export function planItem(id: string, turnId: TurnId, input: Json): Item {
  return { id, turnId, status: "completed", kind: "plan", steps: todoSteps(input) };
}

/** An unknown content block, kept as a generic tool call. */
export function otherBlockItem(id: string, turnId: TurnId, block: Block): Item {
  return { id, turnId, status: "completed", kind: "toolCall", tool: "other", title: String(block.type ?? "content"), input: block };
}

/**
 * Completes a tool call with its result. A file change carries its change as a unified diff
 * (D86), from the tool's structured result when the SDK gives one (`tool_use_result`).
 */
export function withResult(item: Item, result: Block, structured?: unknown, storeImage?: StoreToolImage): Item {
  const failed = result.is_error === true;
  let done = { ...item, status: failed ? "failed" : "completed", output: resultText(result.content) } as Item & { tool?: string; input?: Json; title?: string; output?: string; agentCall?: unknown };
  const images = resultImages(storeImage, result.content);
  if (images.length) done = { ...done, images } as typeof done;
  if (done.input && (done.agentCall || str(done.input.name) === "TaskStop")) {
    const { name, ...input } = done.input;
    done = withAgentCall(done, str(name) ?? "", input, done.output ?? "", structured, failed) as typeof done;
  }
  // Confirmed by the tool's own successful result, never by the request alone.
  if (!failed && done.input) {
    const name = str(done.input.name) ?? "";
    const skill = name === "Skill" ? str(done.input.skill)
      : meshToolOf(name) === "load_skill" ? catalogSkillId(done.input.id) : null;
    if (skill) done = { ...done, title: "Skill loaded", skill: { name: skill } } as typeof done;
  }
  if (!failed && done.tool === "fileChange" && done.input) {
    const name = str(done.input.name) ?? "";
    const diff = editDiff(name, done.input, structured);
    if (diff) done.output = diff;
    const p = editedPath(done.input);
    if (p) {
      const created = name === "Write" && ((structured as Json | undefined)?.type === "create" || (diff?.startsWith("--- /dev/null") ?? false));
      done.title = `${created ? "Created" : "Edited"} ${p}`;
    }
  }
  return done as Item;
}

/** Claude Code's flags beside a stored or streamed user-role message. */
export function flagsOf(m: unknown): PromptFlags {
  const r = (m ?? {}) as Record<string, unknown>;
  return { meta: r.isMeta === true || r.is_meta === true || r.isSynthetic === true, compactSummary: r.isCompactSummary === true };
}

export function isToolResultMessage(content: unknown): boolean {
  return Array.isArray(content) && (content as Block[]).some((b) => b.type === "tool_result");
}

/** The AskUserQuestion request item, as history shows it. */
export function questionHistoryItem(toolUseId: string, turnId: TurnId, input: Json, answered: boolean): Item {
  return {
    id: `request:${toolUseId}`,
    turnId,
    status: "completed",
    kind: "request",
    requestId: toolUseId,
    requestKind: "question",
    state: answered ? "resolved" : "expired",
    blocking: true,
    ...(answered ? { resolution: { by: "elsewhere" } } : {}),
    questions: normaliseQuestions(input),
  } as Item;
}

// ── Stored history ───────────────────────────────────────────────────────────────────────

/** Maps a stored session, oldest first, to items. */
export function historyItems(
  messages: SessionMessage[],
  cwd: string | null,
  opts: { subagent?: boolean; storeImage?: StoreToolImage; imageRange?: { start: number; end: number } } = {},
): Item[] {
  const items: Item[] = [];
  const byId = new Map<string, number>();
  const put = (item: Item) => {
    const at = byId.get(item.id);
    if (at === undefined) {
      byId.set(item.id, items.length);
      items.push(item);
    } else items[at] = item;
  };
  const blockCount = new Map<string, number>();
  const asks = new Map<string, { turnId: TurnId; input: Json }>();
  let turnId: TurnId = "h:start";
  for (const m of messages) {
    // A subagent's own messages carry the Agent tool call they run under: kept only when reading
    // that subagent's transcript (D151).
    if (m.parent_tool_use_id && !opts.subagent) continue;
    const message = obj(m.message);
    const content = message.content;
    if (m.type === "user") {
      if (isToolResultMessage(content)) {
        // The structured result of a lone tool result, when the stored message kept it.
        const results = (content as Block[]).filter((b) => b.type === "tool_result");
        const raw = m as unknown as { toolUseResult?: unknown; tool_use_result?: unknown };
        const structured = results.length === 1 ? (raw.toolUseResult ?? raw.tool_use_result) : undefined;
        for (const b of content as Block[]) {
          if (b.type !== "tool_result") continue;
          const id = str(b.tool_use_id) ?? "";
          const ask = asks.get(id);
          if (ask) {
            put(questionHistoryItem(id, ask.turnId, ask.input, b.is_error !== true));
            continue;
          }
          const at = byId.get(id);
          if (at !== undefined && items[at]!.kind === "toolCall") {
            const range = opts.imageRange;
            const storeImage = !range || (at >= range.start && at < range.end) ? opts.storeImage : undefined;
            items[at] = withResult(items[at]!, b, structured, storeImage);
          }
        }
        continue;
      }
      const flags = flagsOf(m);
      // A message the harness wrote (a reminder, a caveat, the marker of a stopped turn) does
      // not begin a turn.
      if (!flags.meta && !isInterruptedMarker(content)) turnId = m.uuid;
      put(promptItem(m.uuid, turnId, content, (m as { origin?: unknown }).origin, flags));
      continue;
    }
    if (m.type !== "assistant") continue;
    const msgId = str(message.id) ?? m.uuid;
    const blocks = Array.isArray(content) ? (content as Block[]) : typeof content === "string" ? [{ type: "text", text: content }] : [];
    for (const b of blocks) {
      const k = blockCount.get(msgId) ?? 0;
      blockCount.set(msgId, k + 1);
      const id = `${msgId}#${k}`;
      switch (b.type) {
        case "text":
          put({ id, turnId, status: "completed", kind: "agentMessage", text: str(b.text) ?? "" });
          break;
        case "thinking":
          put({ id, turnId, status: "completed", kind: "reasoning", reasoning: str(b.thinking) ?? "" });
          break;
        case "redacted_thinking":
          put({ id, turnId, status: "completed", kind: "reasoning", reasoning: "[redacted reasoning]" });
          break;
        case "tool_use":
        case "server_tool_use": {
          const toolId = str(b.id) ?? id;
          const name = str(b.name) ?? "tool";
          const input = obj(b.input);
          if (name === ASK_TOOL) {
            asks.set(toolId, { turnId, input });
            put(questionHistoryItem(toolId, turnId, input, false));
          } else if (name === TODO_TOOL) put(planItem(toolId, turnId, input));
          else put(toolCallItem(toolId, turnId, name, input, cwd));
          break;
        }
        case "web_search_tool_result":
        case "web_fetch_tool_result": {
          const at = byId.get(str(b.tool_use_id) ?? "");
          if (at !== undefined) items[at] = withResult(items[at]!, { content: JSON.stringify(b.content) });
          else put(otherBlockItem(id, turnId, b));
          break;
        }
        default:
          put(otherBlockItem(id, turnId, b));
      }
    }
  }
  // A tool call left without a result: still running in the last turn, otherwise it ended
  // without one being stored.
  for (let i = 0; i < items.length; i++) {
    const it = items[i]!;
    if (it.kind === "toolCall" && it.status === "inProgress" && it.turnId !== turnId) items[i] = { ...it, status: "failed" } as Item;
  }
  return items;
}

// ── The live stream ──────────────────────────────────────────────────────────────────────

export type ItemEvent =
  | { type: "itemStarted"; item: Item }
  | { type: "itemDelta"; itemId: string; field: "text" | "reasoning"; delta: string }
  | { type: "itemUpdated"; item: Item }
  | { type: "itemCompleted"; item: Item };

interface OpenBlock {
  id: string;
  type: string;
  name?: string;
  text: string;
  json: string;
  /** The input the block started with, when it carried one. */
  input?: Json;
}

/**
 * Turns one live turn's stream (`includePartialMessages` stream events, then the complete
 * assistant and tool-result messages) into item events. Streamed blocks are emitted as they
 * arrive; the complete messages fill in anything the stream did not carry.
 */
export class LiveTranslator {
  private turnId: TurnId = "";
  private msgId: string | null = null;
  private open = new Map<number, OpenBlock>();
  private counts = new Map<string, number>();
  private done = new Set<string>();
  private started = new Set<string>();
  private tools = new Map<string, Item>();
  private skipTools = new Set<string>();
  private inputSeen = new Set<string>();

  constructor(
    private readonly emit: (ev: ItemEvent) => void,
    private readonly cwd: string | null,
    private readonly storeImage?: StoreToolImage,
  ) {}

  begin(turnId: TurnId): void {
    this.turnId = turnId;
    this.open.clear();
    this.msgId = null;
  }

  private start(item: Item): void {
    this.started.add(item.id);
    this.emit({ type: "itemStarted", item });
  }

  private complete(item: Item): void {
    this.done.add(item.id);
    this.emit({ type: "itemCompleted", item });
  }

  /** A Messages API streaming event of the main conversation. */
  stream(event: Json): void {
    const t = event.type;
    if (t === "message_start") {
      this.msgId = str(obj(event.message).id);
      this.open.clear();
      return;
    }
    const index = typeof event.index === "number" ? event.index : -1;
    if (!this.msgId || index < 0) return;
    if (t === "content_block_start") {
      const block = obj(event.content_block) as Block;
      const type = String(block.type ?? "");
      const id = type === "tool_use" || type === "server_tool_use" ? (str(block.id) ?? `${this.msgId}#${index}`) : `${this.msgId}#${index}`;
      const ob: OpenBlock = { id, type, text: "", json: "" };
      if (type === "tool_use" || type === "server_tool_use") {
        ob.name = str(block.name) ?? "tool";
        // The block may carry its input already (no deltas follow then).
        ob.input = obj(block.input);
        if (ob.name === ASK_TOOL || ob.name === TODO_TOOL) this.skipTools.add(id);
        else if (!this.started.has(id)) {
          // Never over what is known: its complete message may have come first.
          const item = toolCallItem(id, this.turnId, ob.name, ob.input, this.cwd);
          this.tools.set(id, item);
          this.start(item);
        }
      } else if (type === "text") {
        this.start({ id, turnId: this.turnId, status: "inProgress", kind: "agentMessage", text: "" });
      } else if (type === "thinking") {
        this.start({ id, turnId: this.turnId, status: "inProgress", kind: "reasoning", reasoning: "" });
      }
      this.open.set(index, ob);
      return;
    }
    const ob = this.open.get(index);
    if (!ob) return;
    if (t === "content_block_delta") {
      const d = obj(event.delta);
      if (d.type === "text_delta" && ob.type === "text") {
        const s = str(d.text) ?? "";
        ob.text += s;
        if (s) this.emit({ type: "itemDelta", itemId: ob.id, field: "text", delta: s });
      } else if (d.type === "thinking_delta" && ob.type === "thinking") {
        const s = str(d.thinking) ?? "";
        ob.text += s;
        if (s) this.emit({ type: "itemDelta", itemId: ob.id, field: "reasoning", delta: s });
      } else if (d.type === "input_json_delta") {
        ob.json += str(d.partial_json) ?? "";
      }
      return;
    }
    if (t === "content_block_stop") {
      this.open.delete(index);
      // The complete assistant message can arrive before the block's stop event.
      if (this.done.has(ob.id) || this.inputSeen.has(ob.id)) return;
      if (ob.type === "text") this.complete({ id: ob.id, turnId: this.turnId, status: "completed", kind: "agentMessage", text: ob.text });
      else if (ob.type === "thinking") this.complete({ id: ob.id, turnId: this.turnId, status: "completed", kind: "reasoning", reasoning: ob.text });
      else if (ob.name) {
        let input: Json | null = ob.input && Object.keys(ob.input).length > 0 ? ob.input : null;
        if (ob.json) {
          try {
            input = obj(JSON.parse(ob.json));
          } catch {
            /* the complete message carries the input */
          }
        }
        // Nothing read: the complete message carries the input. Never clear a call's input
        // (an Agent call's name for its subagent, D164).
        if (input) this.toolInput(ob.id, ob.name, input);
      }
    }
  }

  private toolInput(id: string, name: string, input: Json): void {
    if (name === ASK_TOOL) return;
    if (name === TODO_TOOL) {
      const plan = planItem(id, this.turnId, input);
      if (!this.started.has(id)) this.start({ ...plan, status: "inProgress" } as Item);
      this.complete(plan);
      return;
    }
    const prev = this.tools.get(id);
    const item = toolCallItem(id, prev?.turnId ?? this.turnId, name, input, this.cwd);
    this.tools.set(id, item);
    if (this.started.has(id)) this.emit({ type: "itemUpdated", item });
    else this.start(item);
  }

  /** A complete assistant message of the main conversation. */
  assistant(message: Json, uuid: string, error?: string | null): void {
    const msgId = str(message.id) ?? uuid;
    const content = message.content;
    const blocks = Array.isArray(content) ? (content as Block[]) : typeof content === "string" ? [{ type: "text", text: content }] : [];
    for (const b of blocks) {
      const k = this.counts.get(msgId) ?? 0;
      this.counts.set(msgId, k + 1);
      const id = `${msgId}#${k}`;
      switch (b.type) {
        case "text":
        case "thinking":
        case "redacted_thinking": {
          if (this.done.has(id)) break;
          const item: Item =
            b.type === "text"
              ? { id, turnId: this.turnId, status: "completed", kind: "agentMessage", text: str(b.text) ?? "" }
              : { id, turnId: this.turnId, status: "completed", kind: "reasoning", reasoning: b.type === "thinking" ? (str(b.thinking) ?? "") : "[redacted reasoning]" };
          if (!this.started.has(id)) this.start({ ...item, status: "inProgress" } as Item);
          this.complete(item);
          break;
        }
        case "tool_use":
        case "server_tool_use": {
          const toolId = str(b.id) ?? id;
          const name = str(b.name) ?? "tool";
          if (this.done.has(toolId)) break;
          this.inputSeen.add(toolId);
          this.toolInput(toolId, name, obj(b.input));
          break;
        }
        case "web_search_tool_result":
        case "web_fetch_tool_result":
          this.result({ tool_use_id: b.tool_use_id, content: JSON.stringify(b.content) });
          break;
        default:
          if (!this.done.has(id)) {
            const item = otherBlockItem(id, this.turnId, b);
            this.start({ ...item, status: "inProgress" } as Item);
            this.complete(item);
          }
      }
    }
    if (error) {
      const item: Item = { id: `${uuid}:error`, turnId: this.turnId, status: "completed", kind: "notice", noticeKind: "error", text: `Claude Code reported an error: ${error}` };
      this.start(item);
      this.complete(item);
    }
  }

  /**
   * Claude Code started the subagent of a running `Agent` call (`task_started`, D151): the
   * call's pill names it from now on — by the subagent's own id, so the window resolves it to
   * the subagent's conversation (its title), and by the task's description where the call's
   * input did not give one (D164).
   */
  subagentStarted(toolUseId: string, agentId: string, description: string | null, agentType: string | null): void {
    const item = this.tools.get(toolUseId) as (Item & { agentCall?: AgentCall }) | undefined;
    const call = item?.agentCall;
    if (!item || !call || call.kind !== "spawn") return;
    const peer: AgentRef = { ...(call.peers?.[0] ?? {}), agentId };
    const name = call.title ?? peer.name ?? description ?? agentType;
    if (name && !peer.name) peer.name = name;
    const next: AgentCall = { ...call, peers: [peer], ...(name && !call.title ? { title: name } : {}) };
    const updated = { ...item, agentCall: next, title: agentCallTitle(next, true) } as Item;
    this.tools.set(toolUseId, updated);
    if (this.started.has(toolUseId)) this.emit({ type: "itemUpdated", item: updated });
  }

  private result(b: Block, structured?: unknown): void {
    const id = str(b.tool_use_id) ?? "";
    if (this.skipTools.has(id)) return;
    const item = this.tools.get(id);
    if (!item) return;
    this.tools.delete(id);
    this.complete(withResult(item, b, structured, this.storeImage));
  }

  /**
   * A user message of the main conversation: tool results, or a prompt yaacd did not send.
   * `toolUseResult` is the SDK's structured result of the (single) tool it answers.
   */
  user(content: unknown, uuid: string | null, origin: unknown, toolUseResult?: unknown, flags: PromptFlags = {}): void {
    if (isToolResultMessage(content)) {
      const results = (content as Block[]).filter((b) => b.type === "tool_result");
      for (const b of results) this.result(b, results.length === 1 ? toolUseResult : undefined);
      return;
    }
    if (!uuid) return;
    const item = promptItem(uuid, this.turnId, content, origin, flags);
    this.start(item);
    this.complete(item);
  }

  /** The turn ended: close what is still open. Tool calls stay open only if the turn completed. */
  finish(completed: boolean): void {
    for (const ob of this.open.values()) {
      if (this.done.has(ob.id)) continue;
      if (ob.type === "text") this.complete({ id: ob.id, turnId: this.turnId, status: "completed", kind: "agentMessage", text: ob.text });
      else if (ob.type === "thinking") this.complete({ id: ob.id, turnId: this.turnId, status: "completed", kind: "reasoning", reasoning: ob.text });
    }
    this.open.clear();
    if (!completed) {
      for (const item of this.tools.values()) this.complete({ ...item, status: "failed" } as Item);
      this.tools.clear();
    }
  }
}
