// Asking the user: `canUseTool` permission prompts become approval requests with the decisions
// the harness actually offers; the `AskUserQuestion` tool becomes a question request.

import type {
  ApprovalDecision,
  ApprovalKind,
  FileChange,
  Question,
  RequestResponse,
} from "../../protocol/generated/types.js";
import type { PermissionResult, PermissionUpdate } from "@anthropic-ai/claude-agent-sdk";
import type { RaisedRequest } from "../types.js";

/** The tool through which Claude Code asks structured questions. */
export const ASK_TOOL = "AskUserQuestion";

interface AskOption {
  label?: unknown;
  description?: unknown;
  preview?: unknown;
}
interface AskQuestion {
  question?: unknown;
  header?: unknown;
  options?: unknown;
  multiSelect?: unknown;
}

const str = (v: unknown): string | null => (typeof v === "string" ? v : null);

/** The questions of an `AskUserQuestion` input, normalised (question ids are `q1`, `q2`, …). */
export function normaliseQuestions(input: Record<string, unknown>): Question[] {
  const list = Array.isArray(input.questions) ? (input.questions as AskQuestion[]) : [];
  return list.map((q, i) => ({
    id: `q${i + 1}`,
    header: str(q.header),
    text: str(q.question) ?? "",
    options: (Array.isArray(q.options) ? (q.options as AskOption[]) : []).map((o) => ({
      label: str(o.label) ?? "",
      description: str(o.description),
      preview: str(o.preview),
    })),
    multiSelect: q.multiSelect === true,
    // Claude Code always offers a free-text "Other" answer next to the options.
    allowOther: true,
    secret: false,
  }));
}

/**
 * The `AskUserQuestion` input with the answers filled in, as the tool expects them: keyed by
 * the question text; several selections joined with ", "; free text after the selections.
 */
export function answeredInput(input: Record<string, unknown>, response: RequestResponse): Record<string, unknown> {
  const questions = normaliseQuestions(input);
  const given = response.requestKind === "question" ? response.answers : {};
  const answers: Record<string, string> = {};
  for (const q of questions) {
    const a = given[q.id];
    const parts = [...(a?.selected ?? [])];
    if (a?.other) parts.push(a.other);
    answers[q.text] = parts.join(", ");
  }
  return { ...input, answers };
}

// ── Approvals ────────────────────────────────────────────────────────────────────────────

export interface ToolPermissionContext {
  toolName: string;
  input: Record<string, unknown>;
  cwd: string;
  suggestions?: PermissionUpdate[];
  blockedPath?: string;
  decisionReason?: string;
  title?: string;
  displayName?: string;
  description?: string;
  suppressAlwaysAllowRule?: boolean;
  mcpServer?: { name: string; source: string };
  agentID?: string;
  /** Claude Code's category for `decisionReason` (seen as "other" on 2.1.330; not in sdk.d.ts). */
  decisionReasonType?: string;
  /** A user-configured ask rule forced this prompt: never answered automatically (P342). */
  matchedAskRule?: { source: string; toolName: string; ruleContent?: string };
  /** The prompt should open on its decline option. */
  defaultToNo?: boolean;
}

const FILE_TOOLS = new Set(["Edit", "Write", "NotebookEdit", "MultiEdit"]);
const COMMAND_TOOLS = new Set(["Bash", "PowerShell"]);

function sessionScoped(updates: PermissionUpdate[]): PermissionUpdate[] {
  return updates.map((u) => ({ ...u, destination: "session" }) as PermissionUpdate);
}

/** The approval request for one `canUseTool` call, with only the decisions on offer. */
export function approvalRequest(ctx: ToolPermissionContext): RaisedRequest {
  const { toolName, input } = ctx;
  let approvalKind: ApprovalKind = "other";
  let command: string | null = null;
  let changes: FileChange[] | undefined;
  if (COMMAND_TOOLS.has(toolName)) {
    approvalKind = "command";
    command = str(input.command);
  } else if (FILE_TOOLS.has(toolName)) {
    approvalKind = "fileChange";
    const p = str(input.file_path) ?? str(input.notebook_path);
    if (p) changes = [{ path: p, change: toolName === "Write" ? "add" : "modify", diff: null }];
  } else if (ctx.blockedPath) {
    approvalKind = "permission";
  }
  const suggestions = ctx.suggestions ?? [];
  const decisions: ApprovalDecision[] = [{ id: "allow", label: "Allow once", kind: "allow" }];
  if (suggestions.length > 0 && !ctx.suppressAlwaysAllowRule) {
    decisions.push({ id: "allowSession", label: "Allow for this session", kind: "allowSession" });
    if (suggestions.some((s) => s.destination !== "session")) {
      decisions.push({ id: "allowAlways", label: "Always allow", kind: "allowAlways", description: "Claude Code saves a permission rule." });
    }
  }
  decisions.push({ id: "deny", label: "Deny", kind: "deny", acceptsReason: true });
  decisions.push({ id: "abort", label: "Deny and stop", kind: "abort", acceptsReason: true });
  const title = ctx.title ?? (command ? `Run \`${command}\`` : `${ctx.displayName ?? toolName}`);
  return {
    requestKind: "approval",
    approvalKind,
    title,
    command,
    cwd: ctx.cwd,
    ...(changes ? { changes } : {}),
    reason: ctx.decisionReason ?? ctx.description ?? null,
    scope: ctx.blockedPath ?? null,
    decisions,
    details: {
      tool: toolName,
      input,
      ...(ctx.blockedPath ? { blockedPath: ctx.blockedPath } : {}),
      ...(ctx.mcpServer ? { mcpServer: ctx.mcpServer } : {}),
      ...(ctx.agentID ? { subagent: ctx.agentID } : {}),
      ...(ctx.decisionReason ? { decisionReason: ctx.decisionReason } : {}),
      ...(ctx.decisionReasonType ? { decisionReasonType: ctx.decisionReasonType } : {}),
      ...(ctx.description ? { description: ctx.description } : {}),
      ...(ctx.matchedAskRule ? { matchedAskRule: ctx.matchedAskRule } : {}),
      ...(ctx.defaultToNo ? { defaultToNo: true } : {}),
    },
  };
}

/** What `canUseTool` returns for an approval answer. */
export function approvalResult(ctx: ToolPermissionContext, response: RequestResponse): PermissionResult {
  if (response.requestKind !== "approval") return { behavior: "deny", message: "The user did not approve this." };
  const reason = response.reason?.trim() || null;
  const suggestions = ctx.suggestions ?? [];
  switch (response.decision) {
    case "allow":
      return { behavior: "allow", updatedInput: ctx.input };
    case "allowSession":
      return { behavior: "allow", updatedInput: ctx.input, updatedPermissions: sessionScoped(suggestions) };
    case "allowAlways":
      return { behavior: "allow", updatedInput: ctx.input, updatedPermissions: suggestions };
    case "abort":
      return { behavior: "deny", message: reason ?? "The user denied this and stopped the turn.", interrupt: true };
    default:
      return { behavior: "deny", message: reason ?? "The user denied this." };
  }
}
