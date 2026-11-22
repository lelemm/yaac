// The markup Claude Code writes into user-role messages, and what each becomes
// (docs/research/harness-surfaces.md#claude-code-markup). Verified against Claude Code 2.1.330
// (its bundled tag constants) and stored transcripts; see the research doc for how.

import type { InputPart, Item } from "../../protocol/generated/types.js";
import { attr, findElement, oneLine, plainOf, splitText, textPart, trimBlock, unescapeXml, userItem, type MarkerNotice, type MarkupTable, type TagRule } from "../markup.js";

const context = (tag: string, label: string, whole = false): TagRule => ({ tag, what: { as: "context", label }, ...(whole ? { whole } : {}) });

export const CLAUDE_RULES: TagRule[] = [
  // A paste the CLI kept apart from the typed text (closing tag repeats the id).
  { tag: "pasted_content", what: { as: "pasted" } },
  context("system-reminder", "System reminder"),
  context("local-command-caveat", "Note from Claude Code"),
  context("local-command-stdout", "Command output"),
  context("local-command-stderr", "Command error output"),
  context("bash-stdout", "Shell output"),
  context("bash-stderr", "Shell error output"),
  context("bash-exit-code", "Shell exit code"),
  context("user-prompt-submit-hook", "Hook output"),
  context("post-tool-use-hook", "Hook output"),
  context("ide_selection", "IDE selection"),
  context("ide_opened_file", "File open in the IDE"),
  context("terminal", "Terminal selection", true),
  context("mcp-resource", "MCP resource"),
  context("task-notification", "Background task notification"),
  context("agent-message", "Message from another session"),
  context("cross-session-message", "Message from another session"),
  context("teammate-message", "Message from a teammate"),
  context("command-contents", "Command text"),
  context("skill-format", "Skill"),
  context("fork-boilerplate", "Fork note"),
  context("user-memory-input", "Memory note"),
];

const COMMAND_TAGS = ["command-name", "command-message", "command-args"];

/**
 * A slash command: `<command-name>/x</command-name>` with `<command-message>` and
 * `<command-args>` beside it, in any order, only whitespace between. A shell command run with
 * `!`: `<bash-input>cmd</bash-input>`.
 */
function special(rest: string): { parts: InputPart[]; length: number } | null {
  if (rest.startsWith("<bash-input>")) {
    const el = findElement(rest, "bash-input");
    if (!el || el.start !== 0) return null;
    return { parts: [textPart(`!${trimBlock(el.inner)}`, "command")], length: el.end };
  }
  const found = new Map<string, string>();
  let pos = 0;
  for (;;) {
    const ws = /^\s*/.exec(rest.slice(pos))![0].length;
    const tag = COMMAND_TAGS.find((t) => rest.startsWith(`<${t}>`, pos + ws));
    if (!tag || found.has(tag)) break;
    const el = findElement(rest, tag, pos + ws);
    if (!el || el.start !== pos + ws) break;
    found.set(tag, trimBlock(el.inner));
    pos = el.end;
  }
  const name = found.get("command-name") ?? (found.has("command-message") ? `/${found.get("command-message")}` : null);
  if (!name) return null;
  const args = found.get("command-args") ?? "";
  const shown = (name.startsWith("/") ? name : `/${name}`) + (args ? ` ${args}` : "");
  return { parts: [textPart(shown, "command")], length: pos };
}

// ── The turn was stopped (D167) ──────────────────────────────────────────────────────────

/**
 * The user-role messages Claude Code writes when a turn is stopped part-way: always a lone text
 * block, exactly one of these (Claude Code 2.1.330's constants; the only two shapes in the
 * stored transcripts). The binary's other "[Request interrupted: …]" and "[Tool call …]" texts
 * are tool or sub-agent results, never prompts.
 */
export const INTERRUPTED_MARKERS: Record<string, { text: string; during: "turn" | "toolUse" }> = {
  "[Request interrupted by user]": { text: "Interrupted", during: "turn" },
  "[Request interrupted by user for tool use]": { text: "Interrupted during a tool call", during: "toolUse" },
};

/** The notice for a message that is wholly Claude Code's interrupted marker, or null. */
export function interruptedMarker(text: string): MarkerNotice | null {
  const m = INTERRUPTED_MARKERS[text.trim()];
  return m ? { noticeKind: "interrupted", text: m.text, details: { during: m.during } } : null;
}

export const CLAUDE_MARKUP: MarkupTable = { rules: CLAUDE_RULES, special, specialStart: /<(?:command-name|command-message|command-args|bash-input)>/, marker: interruptedMarker };

/** Whether a stored prompt is only the interrupted marker (it does not begin a turn). */
export function isInterruptedMarker(content: unknown): boolean {
  const parts = texts(content);
  return parts.length === 1 && interruptedMarker((parts[0] as { text: string }).text) !== null;
}

/** The text of a Claude Code content value (a string, or text and image blocks). */
function texts(content: unknown): InputPart[] {
  if (typeof content === "string") return [textPart(content)];
  if (!Array.isArray(content)) return [];
  return (content as Array<Record<string, unknown>>).map((b) =>
    b.type === "text" ? textPart(typeof b.text === "string" ? b.text : "") : b.type === "image" ? textPart("[image]") : textPart(JSON.stringify(b)),
  );
}

// ── Background task notifications ────────────────────────────────────────────────────────

const field = (xml: string, tag: string) => {
  const el = findElement(xml, tag);
  return el ? unescapeXml(trimBlock(el.inner)) : null;
};

/** An id Claude Code uses for its own bookkeeping (`__orphan_summary__:shell`), not a task. */
const internalId = (id: string) => /^__.*__/.test(id);

export interface TaskNotice {
  text: string;
  body: string;
  details: Record<string, unknown>;
}

/**
 * `<task-notification>` (one or more) as a notice: the one line is its `<summary>`; the body
 * is the summary, note, result and event as plain text, without the tags. Null when the text
 * holds none.
 */
export function taskNotice(text: string): TaskNotice | null {
  const lines: string[] = [];
  const bodies: string[] = [];
  const tasks: Array<Record<string, unknown>> = [];
  let pos = 0;
  for (;;) {
    const el = findElement(text, "task-notification", pos);
    if (!el) break;
    pos = el.end;
    const x = el.inner;
    const ids: string[] = [];
    for (let at = 0; ; ) {
      const t = findElement(x, "task-id", at);
      if (!t) break;
      const id = trimBlock(t.inner);
      if (id && !internalId(id)) ids.push(id);
      at = t.end;
    }
    const status = field(x, "status");
    const summary = field(x, "summary");
    const note = field(x, "note");
    const result = field(x, "result");
    const event = field(x, "event");
    const output = field(x, "output-file");
    const worktree = field(x, "worktreePath");
    const branch = field(x, "worktreeBranch");
    lines.push(oneLine(summary ?? (status ? `Background task ${status}` : "Background task update")));
    const body: string[] = [];
    if (summary) body.push(summary);
    if (event) body.push(event);
    if (result) body.push(result);
    if (note) body.push(note);
    if (output) body.push(`Output: ${output}`);
    if (worktree) body.push(`Worktree: ${worktree}${branch ? ` (${branch})` : ""}`);
    bodies.push(body.join("\n\n"));
    tasks.push({ ...(ids.length ? { taskIds: ids } : {}), ...(status ? { status } : {}), ...(field(x, "tool-use-id") ? { toolUseId: field(x, "tool-use-id") } : {}) });
  }
  if (lines.length === 0) return null;
  // Text after the notifications (Claude Code may append a line for the model).
  const tail = plainOf(splitText(text.slice(pos), CLAUDE_MARKUP));
  const text1 = lines.length === 1 ? lines[0]! : `${lines.length} background task updates · ${lines[0]}`;
  return { text: text1, body: [...bodies, ...(tail ? [tail] : [])].join("\n\n"), details: { tasks } };
}

// ── Messages from other agents ───────────────────────────────────────────────────────────

type Json = Record<string, unknown>;
const s = (v: unknown) => (typeof v === "string" && v ? v : null);

/** The inside of the peer envelope, when the origin does not carry the body. */
function peerBody(text: string): string {
  for (const tag of ["agent-message", "cross-session-message", "teammate-message"]) {
    const el = findElement(text, tag);
    if (el) return trimBlock(el.inner);
  }
  return plainOf(splitText(text, CLAUDE_MARKUP));
}

/** One line naming a message that did not come from the user, and its plain body. */
function originNotice(origin: Json, text: string): { text: string; body: string; noticeKind?: string } {
  const kind = s(origin.kind) ?? "unknown";
  switch (kind) {
    case "peer": {
      const who = s(origin.name) ?? s(origin.from) ?? "another session";
      if (origin.handback === true) {
        return { text: `Subagent ${who} reported back`, body: s(origin.handbackReportText) ?? s(origin.body) ?? peerBody(text) };
      }
      return { text: `Message from ${who}`, body: s(origin.body) ?? peerBody(text) };
    }
    case "task-notification": {
      const t = taskNotice(text);
      if (t) return { text: t.text, body: t.body };
      const body = plainOf(splitText(text, CLAUDE_MARKUP));
      return { text: oneLine(body) || "Background task update", body };
    }
    case "channel":
      return { text: `Message from channel ${s(origin.server) ?? ""}`.trim(), body: plainOf(splitText(text, CLAUDE_MARKUP)) };
    case "coordinator":
      return { text: "Message from the coordinator", body: plainOf(splitText(text, CLAUDE_MARKUP)) };
    case "observer":
      return { text: `Report from observer ${s(origin.from) ?? ""}`.trim(), body: plainOf(splitText(text, CLAUDE_MARKUP)) };
    case "auto-continuation":
      return { text: "Claude Code continued the turn on its own", body: plainOf(splitText(text, CLAUDE_MARKUP)) };
    default: {
      const body = plainOf(splitText(text, CLAUDE_MARKUP));
      return { text: `Message from ${kind}`, body };
    }
  }
}

/** Flags Claude Code stores beside a user-role message. */
export interface PromptFlags {
  /** `isMeta` / `is_meta`: written by the harness, not the user. */
  meta?: boolean;
  /** `isCompactSummary`: the summary that continues a compacted conversation. */
  compactSummary?: boolean;
}

/**
 * A user-role prompt (not a tool result) as an item. From a person: a user message, split
 * along Claude Code's markup. From the harness (`isMeta`): context only. From anything else
 * (another session, a background task, a coordinator): a notice with its meaning, never the
 * raw envelope.
 */
export function promptItem(uuid: string, turnId: string, content: unknown, origin: unknown, flags: PromptFlags = {}): Item {
  const id = `user:${uuid}`;
  const parts = texts(content);
  const raw = parts.map((p) => (p as { text: string }).text).join("\n\n");
  const o = (origin && typeof origin === "object" ? origin : {}) as Json;
  const kind = s(o.kind);
  if (flags.compactSummary) {
    return { id, turnId, status: "completed", kind: "notice", noticeKind: "compaction", text: "The conversation was compacted; Claude Code continues from a summary.", body: plainOf(splitText(raw, CLAUDE_MARKUP), true) } as Item;
  }
  if (kind && kind !== "human") {
    const n = originNotice(o, raw);
    return { id, turnId, status: "completed", kind: "notice", noticeKind: "info", text: oneLine(n.text), ...(n.body ? { body: n.body } : {}), details: { origin: kind } } as Item;
  }
  if (flags.meta) {
    const split = splitText(raw, CLAUDE_MARKUP).map((p) => ((p as { role?: string }).role === "context" ? p : textPart((p as { text: string }).text, "context", "Added by Claude Code")));
    return { id, turnId, status: "completed", kind: "userMessage", parts: split.length > 0 ? split : [textPart("", "context", "Added by Claude Code")] } as Item;
  }
  return userItem({ id, turnId }, parts, CLAUDE_MARKUP);
}

/** Tool output without the reminders Claude Code appends to it for the model. */
export function cleanToolOutput(text: string): string {
  if (!text.includes("<system-reminder")) return text;
  let out = text;
  for (let el = findElement(out, "system-reminder"); el; el = findElement(out, "system-reminder")) {
    out = (out.slice(0, el.start).replace(/\s+$/, "") + out.slice(el.end)).replace(/\n{3,}/g, "\n\n");
  }
  return out.replace(/\s+$/, "");
}

/** Whether a tool error is Claude Code's `<tool_use_error>` envelope: its inside. */
export function unwrapToolError(text: string): string {
  const el = findElement(text, "tool_use_error");
  if (el && !text.slice(0, el.start).trim() && !text.slice(el.end).trim()) return trimBlock(el.inner);
  return text;
}

/** A session's first prompt as a preview: the user's text, without Claude Code's markup. */
export function previewText(firstPrompt: string | null | undefined): string | null {
  if (!firstPrompt) return null;
  if (!firstPrompt.includes("<")) return firstPrompt;
  const plain = plainOf(splitText(firstPrompt, CLAUDE_MARKUP));
  return plain || null;
}

export { attr };
