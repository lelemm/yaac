// Translation from Codex app-server shapes to the YAAC item model and contract types.
// Pure functions: no I/O, no state. Nothing here names a model or an effort level; models,
// efforts, and modalities are copied from what the harness reports.

import type {
  ApprovalDecision,
  DecisionKind,
  EffortInfo,
  FileChange,
  InputPart,
  Item,
  ItemStatus,
  ModelCapability,
  ModelInfo,
  PlanStepState,
  Question,
  RequestResponse,
  SubagentStatus,
  TokenUsage,
  TurnStatus,
} from "../../protocol/generated/types.js";
import type { NativeThreadInfo, RaisedRequest } from "../types.js";
import { userItem } from "../markup.js";
import { CODEX_MARKUP, textWithElements } from "./markup.js";
import { agentCallTitle, codexAgentCall, meshAgentCall, MESH_SERVER } from "../agentcalls.js";
import { INTERACTIVE_SOURCES, type Model, type Thread, type ThreadItem, type ThreadTokenUsage, type TokenUsageBreakdown } from "./wire.js";

// ── Small helpers ──────────────────────────────────────────────────────────────────────────

const str = (v: unknown): string | null => (typeof v === "string" ? v : null);
const num = (v: unknown): number | null => (typeof v === "number" && Number.isFinite(v) ? v : null);
const arr = (v: unknown): unknown[] => (Array.isArray(v) ? v : []);
const obj = (v: unknown): Record<string, unknown> => (v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : {});

export function isoFromSeconds(s: unknown): string {
  const n = num(s);
  return new Date(n === null ? Date.now() : n * 1000).toISOString();
}

// ── Models ─────────────────────────────────────────────────────────────────────────────────

/** The id YAAC stores and sends back: the model slug Codex reports (`model`), else `id`. */
export function modelId(m: Model): string {
  return typeof m.model === "string" && m.model.length > 0 ? m.model : m.id;
}

/**
 * The service tier that is Codex's fast mode for this model (D125), from the model list: the
 * tier Codex names "Fast", or, by its id, the `fast` / `priority` tier. `null` when the model
 * lists tiers and none is fast, or lists none.
 */
export function fastTier(m: Model): string | null {
  const tiers = arr(m.serviceTiers).map((t) => obj(t)).filter((t) => typeof t.id === "string" && t.id.length > 0 && t.id !== "ultrafast");
  const named = tiers.find((t) => typeof t.name === "string" && t.name.trim().toLowerCase() === "fast");
  const byId = tiers.find((t) => t.id === "fast" || t.id === "priority");
  return ((named ?? byId)?.id as string | undefined) ?? null;
}

export function mapModel(m: Model): ModelInfo {
  const efforts: EffortInfo[] = arr(m.supportedReasoningEfforts)
    .map((e) => obj(e))
    .filter((e) => typeof e.reasoningEffort === "string")
    .map((e) => ({ id: e.reasoningEffort as string, ...(str(e.description) ? { description: e.description as string } : {}) }));
  const capabilities: ModelCapability[] = [];
  if (Array.isArray(m.inputModalities)) {
    const mods = new Set(m.inputModalities);
    capabilities.push({ capability: "imageInput", support: mods.has("image") ? "supported" : "unsupported" });
    capabilities.push({ capability: "audioInput", support: mods.has("audio") ? "supported" : "unsupported" });
  } else {
    // Codex gave no modality data for this model: that is "unknown", not "yes".
    capabilities.push({ capability: "imageInput", support: "unknown" });
  }
  // Fast mode: a fast service tier listed for the model. No tier list at all is "unknown"
  // (an older Codex), never "yes".
  if (Array.isArray(m.serviceTiers)) {
    capabilities.push({ capability: "fastMode", support: fastTier(m) ? "supported" : "unsupported" });
    capabilities.push({ capability: "ultrafastMode", support: m.serviceTiers.some((tier) => tier?.id === "ultrafast") ? "supported" : "unsupported" });
  } else {
    capabilities.push({ capability: "fastMode", support: "unknown" });
  }
  const info: ModelInfo = {
    id: modelId(m),
    displayName: m.displayName || modelId(m),
    description: m.description ?? null,
    isDefault: !!m.isDefault,
    hidden: !!m.hidden,
    capabilities,
    efforts,
    defaultEffort: typeof m.defaultReasoningEffort === "string" ? m.defaultReasoningEffort : null,
  };
  return info;
}

// ── Threads ────────────────────────────────────────────────────────────────────────────────

export function sourceKind(source: unknown): string {
  if (typeof source === "string") return source;
  const keys = Object.keys(obj(source));
  return keys[0] ?? "unknown";
}

export function originKindOf(source: unknown): "interactive" | "programmatic" {
  return INTERACTIVE_SOURCES.has(sourceKind(source)) ? "interactive" : "programmatic";
}

export function threadInfo(t: Thread, archived: boolean, status?: SubagentStatus | null): NativeThreadInfo {
  const info: NativeThreadInfo = {
    native: t.id,
    title: t.name ?? null,
    preview: t.preview ?? null,
    cwd: t.cwd ?? "",
    model: t.model ?? null,
    createdAt: isoFromSeconds(t.createdAt),
    updatedAt: isoFromSeconds(t.updatedAt ?? t.createdAt),
    originKind: originKindOf(t.source),
    archived,
  };
  if (t.forkedFromId) info.forkedFrom = t.forkedFromId;
  const spawn = spawnSource(t.source);
  if (spawn) {
    info.parent = spawn.parent;
    info.subagent = {
      agentId: t.id,
      ...(spawn.role ?? t.agentRole ? { agentType: (spawn.role ?? t.agentRole)! } : {}),
      status: status ?? subagentStatusOf(t.status),
      readOnly: CODEX_SUBAGENT_READ_ONLY,
    };
    if (!info.title) info.title = spawn.nickname ?? t.agentNickname ?? null;
  }
  return info;
}

/** Why YAAC does not send to a Codex sub-agent (D151, P192). */
export const CODEX_SUBAGENT_READ_ONLY = "YAAC does not send to a Codex sub-agent: the agent that spawned it is its only writer. Message that agent instead.";

/**
 * A sub-agent spawned by another thread (`source.subAgent.thread_spawn`, codex-cli 0.170.0):
 * its parent thread, nickname and role. Review and compaction sub-agents name no parent.
 */
export function spawnSource(source: unknown): { parent: string; nickname: string | null; role: string | null } | null {
  const sub = obj(obj(source).subAgent);
  const spawn = obj(sub.thread_spawn);
  const parent = str(spawn.parent_thread_id);
  if (!parent) return null;
  return { parent, nickname: str(spawn.agent_nickname) ?? null, role: str(spawn.agent_role) ?? null };
}

/** A sub-agent's status from its thread status: working while active, else finished. */
export function subagentStatusOf(status: unknown): SubagentStatus {
  const type = obj(status).type;
  if (type === "active") return "running";
  if (type === "systemError") return "failed";
  return "completed";
}

/** A collab tool call's last known state of an agent (`agentsStates`), as a sub-agent status. */
export function collabStatus(status: unknown): SubagentStatus | null {
  switch (status) {
    case "pendingInit":
    case "running":
      return "running";
    case "completed":
      return "completed";
    case "errored":
      return "failed";
    case "interrupted":
    case "shutdown":
      return "stopped";
    default:
      return null;
  }
}

// ── Items ──────────────────────────────────────────────────────────────────────────────────

function itemStatus(v: unknown, fallback: ItemStatus): ItemStatus {
  return v === "inProgress" || v === "completed" || v === "failed" || v === "declined" ? v : v === "interrupted" ? "failed" : fallback;
}

export function userInputToParts(content: unknown): InputPart[] {
  const parts: InputPart[] = [];
  for (const raw of arr(content)) {
    const c = obj(raw);
    if (c.type === "text" && typeof c.text === "string") parts.push(...textWithElements(c.text, c.text_elements ?? c.textElements));
    else if (c.type === "localImage" || c.type === "image") parts.push({ type: "text", text: `[image: ${str(c.path) ?? str(c.url) ?? "attached"}]` });
    else if (c.type === "localAudio" || c.type === "audio") parts.push({ type: "text", text: `[audio: ${str(c.path) ?? str(c.url) ?? "attached"}]` });
    else if (c.type === "skill" && str(c.name) && str(c.path)) parts.push({ type: "skill", name: str(c.name)!, path: str(c.path)! });
    else if (c.type === "skill" || c.type === "mention") parts.push({ type: "text", text: `[${String(c.type)}: ${str(c.name) ?? ""}]` });
    else if (typeof c.type === "string") parts.push({ type: "text", text: `[${c.type}]` });
  }
  return parts.length > 0 ? parts : [{ type: "text", text: "" }];
}

function mapChangeKind(kind: unknown): { change: FileChange["change"]; newPath: string | null } {
  const k = obj(kind);
  if (k.type === "add") return { change: "add", newPath: null };
  if (k.type === "delete") return { change: "delete", newPath: null };
  const move = str(k.move_path) ?? str(k.movePath);
  return move ? { change: "rename", newPath: move } : { change: "modify", newPath: null };
}

export function fileChanges(changes: unknown): FileChange[] {
  return arr(changes).map((raw) => {
    const c = obj(raw);
    const { change, newPath } = mapChangeKind(c.kind);
    const fc: FileChange = { path: str(c.path) ?? "", change };
    if (newPath) fc.newPath = newPath;
    if (typeof c.diff === "string") fc.diff = c.diff;
    return fc;
  });
}

function mcpOutput(result: unknown, error: unknown): string {
  const e = obj(error);
  if (typeof e.message === "string") return e.message;
  const r = obj(result);
  const texts = arr(r.content)
    .map((c) => obj(c))
    .map((c) => (typeof c.text === "string" ? c.text : JSON.stringify(c)));
  if (texts.length > 0) return texts.join("\n");
  if (r.structuredContent !== undefined && r.structuredContent !== null) return JSON.stringify(r.structuredContent);
  return "";
}

/** `/` and `\\` alike, no trailing slash, so a path compares the same on every target. */
export function skillPathKey(p: string): string {
  return p.replace(/\\/g, "/").replace(/\/+$/, "");
}

/**
 * The skill a finished command read the instructions of, or null. Conservative: only a
 * successful command whose parsed actions include a read of exactly a `SKILL.md` that `skills/list`
 * gave (`skills`: path → name). A command that merely mentions a skill, a failed or running
 * one, or a read of another file never counts.
 */
export function skillRead(it: Record<string, unknown>, status: ItemStatus, skills: ReadonlyMap<string, string>): string | null {
  if (status !== "completed" || num(it.exitCode) !== 0) return null;
  const cwd = str(it.cwd);
  for (const a of arr(it.commandActions)) {
    const act = obj(a);
    const p = act.type === "read" ? str(act.path) : null;
    if (!p) continue;
    const abs = /^(\/|[A-Za-z]:[\\/])/.test(p) || !cwd ? p : `${cwd}/${p}`;
    const name = skills.get(skillPathKey(abs));
    if (name) return name;
  }
  return null;
}

/**
 * One Codex thread item → one YAAC item. Never drops anything: what has no YAAC kind becomes
 * a `toolCall` of type `other` (or a `notice` where it is out-of-band information).
 * `final` is the status to assume when Codex gives none (completed for history).
 */
export function mapItem(raw: ThreadItem, turnId: string, final: ItemStatus, skills?: ReadonlyMap<string, string>): Item {
  const it = raw as Record<string, unknown>;
  const id = String(raw.id);
  const base = { id, turnId };
  switch (raw.type) {
    case "userMessage":
      return userItem(base, userInputToParts(it.content), CODEX_MARKUP);
    case "agentMessage":
      return { ...base, status: final, kind: "agentMessage", text: str(it.text) ?? "" };
    case "reasoning": {
      const content = arr(it.content).filter((x): x is string => typeof x === "string");
      const summary = arr(it.summary).filter((x): x is string => typeof x === "string");
      const useSummary = content.length === 0;
      return { ...base, status: final, kind: "reasoning", reasoning: (useSummary ? summary : content).join("\n\n"), ...(useSummary ? { summary: true } : {}) };
    }
    case "plan":
      return { ...base, status: final, kind: "plan", explanation: str(it.text), steps: [] };
    case "commandExecution": {
      const output = str(it.aggregatedOutput);
      const item: Item = {
        ...base,
        status: itemStatus(it.status, final),
        kind: "toolCall",
        tool: "command",
        title: str(it.command) ?? "command",
        input: { command: it.command, commandActions: it.commandActions ?? null },
        cwd: str(it.cwd),
        exitCode: num(it.exitCode),
      };
      if (output !== null) {
        (item as { output?: string }).output = output;
        (item as { outputBytes?: number }).outputBytes = Buffer.byteLength(output);
      }
      const skill = skills ? skillRead(it, item.status, skills) : null;
      if (skill) Object.assign(item, { title: "Read skill instructions", skill: { name: skill } });
      return item;
    }
    case "fileChange": {
      const changes = fileChanges(it.changes);
      const title = changes.length === 1 ? `Edit ${changes[0]!.path}` : `Edit ${changes.length} files`;
      return {
        ...base,
        status: itemStatus(it.status, final),
        kind: "toolCall",
        tool: "fileChange",
        title,
        input: { changes },
        output: changes.map((c) => c.diff ?? "").filter(Boolean).join("\n"),
      };
    }
    case "mcpToolCall": {
      const status = itemStatus(it.status, final);
      const output = mcpOutput(it.result, it.error);
      const item = {
        ...base,
        status,
        kind: "toolCall",
        tool: "mcp",
        title: `${str(it.server) ?? "mcp"}.${str(it.tool) ?? "tool"}`,
        input: { server: it.server, tool: it.tool, arguments: it.arguments ?? null },
        output,
      } as Item;
      // YAAC's own mesh tools (D150): read as communication with another agent.
      if (str(it.server) === MESH_SERVER) {
        const done = status !== "inProgress";
        const call = meshAgentCall(str(it.tool) ?? "", obj(it.arguments), done ? output : null, status === "failed" || obj(it.result).isError === true);
        if (call) return { ...item, agentCall: call, title: agentCallTitle(call, false) } as Item;
      }
      return item;
    }
    case "webSearch": {
      const action = obj(it.action);
      const query = str(it.query) ?? str(action.query) ?? str(action.url) ?? "";
      return { ...base, status: final, kind: "toolCall", tool: "webSearch", title: query ? `Search: ${query}` : "Web search", input: { query: it.query ?? null, action: it.action ?? null } };
    }
    case "collabAgentToolCall": {
      const call = codexAgentCall(it);
      return {
        ...base,
        status: itemStatus(it.status, final),
        kind: "toolCall",
        tool: "subagent",
        title: call ? agentCallTitle(call, true) : `Agent: ${String(it.tool ?? "collab")}`,
        input: { tool: it.tool, prompt: it.prompt ?? null, receiverThreadIds: it.receiverThreadIds ?? [], model: it.model ?? null, agentsStates: it.agentsStates ?? null },
        ...(call ? { agentCall: call } : {}),
      } as Item;
    }
    case "subAgentActivity":
      return { ...base, status: final, kind: "toolCall", tool: "subagent", title: `Sub-agent ${String(it.kind ?? "activity")}`, input: { agentThreadId: it.agentThreadId ?? null, agentPath: it.agentPath ?? null } };
    case "contextCompaction":
      return { ...base, status: final, kind: "notice", noticeKind: "compaction", text: final === "inProgress" ? "Compacting context…" : "Context compacted" };
    case "enteredReviewMode":
    case "exitedReviewMode":
      return { ...base, status: "completed", kind: "notice", noticeKind: "info", text: `${raw.type === "enteredReviewMode" ? "Entered" : "Left"} review mode${str(it.review) ? `: ${it.review}` : ""}` };
    case "hookPrompt":
    {
      const body = arr(it.fragments)
        .map((f) => str(obj(f).text) ?? "")
        .filter((t) => t.trim())
        .join("\n\n");
      return { ...base, status: "completed", kind: "notice", noticeKind: "info", text: "A hook added a prompt.", ...(body ? { body } : {}), details: { fragments: it.fragments ?? [] } };
    }
    default: {
      // dynamicToolCall, functionCallOutput, imageView, imageGeneration, sleep, and anything newer.
      const { id: _id, type, ...rest } = it;
      void _id;
      const name = str(it.tool) ?? str(it.name);
      const title = name ? `${String(type)}: ${name}` : String(type);
      return { ...base, status: itemStatus(it.status, final), kind: "toolCall", tool: "other", title, input: { type, ...rest } };
    }
  }
}

export function planState(v: unknown): PlanStepState {
  return v === "completed" ? "completed" : v === "inProgress" ? "inProgress" : "pending";
}

export function turnStatus(v: unknown): TurnStatus {
  return v === "interrupted" ? "interrupted" : v === "failed" ? "failed" : "completed";
}

export function tokenUsage(b: TokenUsageBreakdown | undefined): TokenUsage | null {
  if (!b) return null;
  return {
    input: b.inputTokens ?? null,
    cachedInput: b.cachedInputTokens ?? null,
    output: b.outputTokens ?? null,
    reasoning: b.reasoningOutputTokens ?? null,
    total: b.totalTokens ?? null,
  };
}

/** Usage of one turn: the thread's running total now minus the total when the turn began. */
export function usageDelta(now: ThreadTokenUsage | undefined, before: ThreadTokenUsage | undefined): TokenUsage | null {
  const a = now?.total;
  if (!a) return null;
  const b = before?.total ?? {};
  const d = (k: keyof TokenUsageBreakdown) => (typeof a[k] === "number" ? (a[k] as number) - ((b[k] as number | undefined) ?? 0) : null);
  return { input: d("inputTokens"), cachedInput: d("cachedInputTokens"), output: d("outputTokens"), reasoning: d("reasoningOutputTokens"), total: d("totalTokens") };
}

// ── Requests ───────────────────────────────────────────────────────────────────────────────

/** A raised request plus what is needed to turn a YAAC answer into the Codex response. */
export interface MappedRequest {
  request: RaisedRequest;
  /**
   * Stable across re-delivery (a reconnect or a new subscription re-sends pending requests
   * under new JSON-RPC ids). Null when Codex gives nothing stable to key on.
   */
  key: string | null;
  threadId: string;
  turnId: string;
  blocking: boolean;
  /** Turns a YAAC answer into the JSON the server request expects. */
  encode(answer: RequestResponse): unknown;
}

function decisionFor(raw: unknown): { id: string; kind: DecisionKind; label: string; description?: string } | null {
  if (typeof raw === "string") {
    switch (raw) {
      case "accept":
      case "approved":
        return { id: raw, kind: "allow", label: "Allow once" };
      case "acceptForSession":
      case "approved_for_session":
        return { id: raw, kind: "allowSession", label: "Allow for this session" };
      case "decline":
        return { id: raw, kind: "deny", label: "Deny" };
      case "denied":
        return { id: raw, kind: "deny", label: "Deny" };
      case "cancel":
      case "abort":
        return { id: raw, kind: "abort", label: "Deny and stop the turn" };
      case "approved_mcp_policy_amendment":
        return { id: raw, kind: "allowAlways", label: "Allow and remember" };
      default:
        return null; // e.g. "timed_out": not something a user chooses
    }
  }
  const o = obj(raw);
  const key = Object.keys(o)[0];
  if (!key) return null;
  if (key === "acceptWithExecpolicyAmendment" || key === "approved_execpolicy_amendment") {
    const inner = obj(o[key]);
    const rule = inner.execpolicy_amendment ?? inner.proposed_execpolicy_amendment;
    return { id: key, kind: "allowAlways", label: "Allow and don't ask again for this command", ...(Array.isArray(rule) ? { description: `Remembers: ${rule.join(" ")}` } : {}) };
  }
  if (key === "applyNetworkPolicyAmendment" || key === "network_policy_amendment") {
    const inner = obj(obj(o[key]).network_policy_amendment);
    const action = str(inner.action) ?? "";
    const host = str(inner.host) ?? "";
    const deny = action.toLowerCase().startsWith("deny");
    return { id: `${key}:${host}:${action}`, kind: deny ? "deny" : "allowAlways", label: deny ? `Deny network access to ${host} from now on` : `Allow network access to ${host} from now on` };
  }
  return { id: key, kind: "allow", label: key };
}

/** Builds offered decisions from what Codex lists; keeps the raw values to send back. */
function decisions(rawList: unknown[]): { decisions: ApprovalDecision[]; raw: Map<string, unknown> } {
  const out: ApprovalDecision[] = [];
  const raw = new Map<string, unknown>();
  for (const r of rawList) {
    const d = decisionFor(r);
    if (!d || raw.has(d.id)) continue;
    raw.set(d.id, r);
    out.push({ id: d.id, kind: d.kind, label: d.label, ...(d.description ? { description: d.description } : {}) });
  }
  return { decisions: out, raw };
}

function pickDecision(raw: Map<string, unknown>, answer: RequestResponse, method: string): unknown {
  if (answer.requestKind !== "approval") throw new Error(`${method}: expected an approval answer, got ${answer.requestKind}`);
  if (!raw.has(answer.decision)) throw new Error(`${method}: "${answer.decision}" is not one of the decisions Codex offered`);
  return raw.get(answer.decision);
}

/** Codex's stable decision sets, used when the server does not list `availableDecisions`. */
const COMMAND_DECISIONS = ["accept", "acceptForSession", "decline", "cancel"];
const FILE_DECISIONS = ["accept", "acceptForSession", "decline", "cancel"];

function questionsOf(rawQs: unknown): Question[] {
  return arr(rawQs).map((raw) => {
    const q = obj(raw);
    const options = arr(q.options).map((o) => obj(o)).map((o) => ({ label: str(o.label) ?? "", description: str(o.description) }));
    const isOther = q.isOther === true;
    return {
      id: str(q.id) ?? "",
      header: str(q.header),
      text: str(q.question) ?? "",
      options,
      multiSelect: false,
      allowOther: isOther || options.length === 0,
      secret: q.isSecret === true,
    };
  });
}

/**
 * A Codex server request → a YAAC request, or null when the method is not something the user
 * answers (the caller replies to those itself).
 */
export function mapServerRequest(method: string, params: unknown): MappedRequest | null {
  const p = obj(params);
  const threadId = str(p.threadId) ?? str(p.conversationId) ?? "";
  const turnId = str(p.turnId) ?? "";
  const itemId = str(p.itemId) ?? str(p.callId) ?? "";
  switch (method) {
    case "item/commandExecution/requestApproval": {
      const offered = Array.isArray(p.availableDecisions) && p.availableDecisions.length > 0 ? p.availableDecisions : COMMAND_DECISIONS;
      const { decisions: ds, raw } = decisions(offered);
      const command = str(p.command);
      const stdin = p.kind === "writeStdin";
      return {
        key: itemId ? `cmd:${itemId}:${str(p.approvalId) ?? ""}` : null,
        threadId,
        turnId,
        blocking: true,
        request: {
          requestKind: "approval",
          approvalKind: "command",
          title: stdin ? "Send input to a running command" : command ? `Run \`${command}\`` : "Run a command",
          command,
          cwd: str(p.cwd),
          reason: str(p.reason),
          decisions: ds,
          details: { kind: p.kind ?? "command", commandActions: p.commandActions ?? null, networkApprovalContext: p.networkApprovalContext ?? null, itemId },
        },
        encode: (a) => ({ decision: pickDecision(raw, a, method) }),
      };
    }
    case "item/fileChange/requestApproval": {
      const offered = Array.isArray(p.availableDecisions) && p.availableDecisions.length > 0 ? p.availableDecisions : FILE_DECISIONS;
      const { decisions: ds, raw } = decisions(offered);
      const grantRoot = str(p.grantRoot);
      return {
        key: itemId ? `file:${itemId}` : null,
        threadId,
        turnId,
        blocking: true,
        request: {
          requestKind: "approval",
          approvalKind: "fileChange",
          title: grantRoot ? `Allow writes under ${grantRoot}` : "Apply file changes",
          reason: str(p.reason),
          scope: grantRoot,
          decisions: ds,
          details: { itemId },
        },
        encode: (a) => ({ decision: pickDecision(raw, a, method) }),
      };
    }
    case "item/permissions/requestApproval": {
      const requested = obj(p.permissions);
      const granted: Record<string, unknown> = {};
      for (const [k, v] of Object.entries(requested)) if (v !== null && v !== undefined) granted[k] = v;
      return {
        key: itemId ? `perm:${itemId}` : null,
        threadId,
        turnId,
        blocking: true,
        request: {
          requestKind: "approval",
          approvalKind: "permission",
          title: "Grant additional permissions",
          cwd: str(p.cwd),
          reason: str(p.reason),
          scope: JSON.stringify(granted),
          decisions: [
            { id: "grantTurn", kind: "allow", label: "Allow for this turn" },
            { id: "grantSession", kind: "allowSession", label: "Allow for this session" },
            { id: "deny", kind: "deny", label: "Deny" },
          ],
          details: { permissions: requested, itemId },
        },
        encode: (a) => {
          if (a.requestKind !== "approval") throw new Error(`${method}: expected an approval answer`);
          if (a.decision === "grantTurn") return { permissions: granted, scope: "turn" };
          if (a.decision === "grantSession") return { permissions: granted, scope: "session" };
          if (a.decision === "deny") return { permissions: {}, scope: "turn" };
          throw new Error(`${method}: "${a.decision}" is not one of the offered decisions`);
        },
      };
    }
    case "item/tool/requestUserInput": {
      const questions = questionsOf(p.questions);
      return {
        key: itemId ? `input:${itemId}` : null,
        threadId,
        turnId,
        blocking: p.isBlocking !== false,
        request: { requestKind: "question", questions },
        encode: (a) => {
          if (a.requestKind !== "question") throw new Error(`${method}: expected a question answer`);
          const answers: Record<string, { answers: string[] }> = {};
          for (const q of questions) {
            const ans = a.answers[q.id];
            const list = [...(ans?.selected ?? [])];
            if (typeof ans?.other === "string" && ans.other.length > 0) list.push(ans.other);
            answers[q.id] = { answers: list };
          }
          return { answers };
        },
      };
    }
    case "mcpServer/elicitation/request": {
      const mode = str(p.mode) ?? "form";
      const message = str(p.message) ?? str(p.description) ?? str(p.title) ?? "An MCP server asks for input.";
      const schema = p.requestedSchema ?? (mode === "url" ? { url: p.url ?? null } : mode.endsWith("userVerification") ? { challenge: p.challenge ?? null } : null);
      return {
        key: str(p.elicitationId) ? `elicit:${str(p.serverName) ?? ""}:${str(p.elicitationId)}` : null,
        threadId,
        turnId,
        blocking: true,
        request: { requestKind: "form", message, schema, source: str(p.serverName) },
        encode: (a) => {
          if (a.requestKind !== "form") throw new Error(`${method}: expected a form answer`);
          return { action: "accept", content: a.values ?? {}, _meta: null };
        },
      };
    }
    case "execCommandApproval":
    case "applyPatchApproval": {
      const exec = method === "execCommandApproval";
      const legacy = ["approved", "approved_for_session", "denied", "abort"];
      const { decisions: ds, raw } = decisions(legacy);
      const cmd = Array.isArray(p.command) ? (p.command as unknown[]).map(String).join(" ") : null;
      const changes = Object.entries(obj(p.fileChanges)).map(([path]) => ({ path, change: "modify" as const }));
      return {
        key: itemId ? `legacy:${method}:${itemId}:${str(p.approvalId) ?? ""}` : null,
        threadId,
        turnId,
        blocking: true,
        request: exec
          ? { requestKind: "approval", approvalKind: "command", title: cmd ? `Run \`${cmd}\`` : "Run a command", command: cmd, cwd: str(p.cwd), reason: str(p.reason), decisions: ds }
          : { requestKind: "approval", approvalKind: "fileChange", title: "Apply file changes", changes, reason: str(p.reason), scope: str(p.grantRoot), decisions: ds },
        encode: (a) => {
          const d = pickDecision(raw, a, method);
          if (d === "denied") return { decision: { denied: { rejection: a.requestKind === "approval" ? a.reason ?? "" : "" } } };
          return { decision: d };
        },
      };
    }
    default:
      return null;
  }
}
