// The markup Codex writes into user-role messages, and what each becomes
// (docs/research/harness-surfaces.md#codex-markup). Verified against codex-cli 0.170.0 (its
// binary's fragment markers and the app-server schema) and stored rollouts; see the research doc.
//
// The app-server's `userMessage` items are normally the user's own input: Codex keeps its
// contextual fragments (environment, AGENTS.md, a shell command the user ran, an aborted turn)
// out of them. The table still knows those fragments, so a message that carries one — from an
// older Codex, or a path that does not filter — never shows its tags.

import type { InputPart } from "../../protocol/generated/types.js";
import { findElement, textPart, trimBlock, type MarkerNotice, type MarkupTable, type TagRule } from "../markup.js";

const context = (tag: string, label: string, whole = false): TagRule => ({ tag, what: { as: "context", label }, ...(whole ? { whole } : {}) });

export const CODEX_RULES: TagRule[] = [
  context("environment_context", "Environment"),
  context("user_instructions", "Instructions (AGENTS.md)"),
  // `# AGENTS.md instructions for <dir>` and the file, in `<INSTRUCTIONS>`.
  { tag: "INSTRUCTIONS", what: { as: "context", label: "Instructions (AGENTS.md)" }, before: /# AGENTS\.md instructions for [^\n]*\n\s*/ },
  context("turn_aborted", "Turn interrupted"),
  context("subagent_notification", "Sub-agent notification"),
  context("agent_message_board_notification", "Agent message board"),
  context("codex_internal_context", "Codex context"),
  context("image_resize_notice", "Image resized"),
  context("skill", "Skill", true),
  context("objective", "Goal", true),
  context("untrusted_objective", "Goal", true),
  // The user's answer to a question the agent asked with send_user_message.
  { tag: "send_user_message_question_reply", what: { as: "unwrap" } },
];

/** `<user_shell_command><command>…</command><result>…</result></user_shell_command>`: a shell command the user ran. */
function special(rest: string): { parts: InputPart[]; length: number } | null {
  const el = findElement(rest, "user_shell_command");
  if (!el || el.start !== 0) return null;
  const cmd = findElement(el.inner, "command");
  const result = findElement(el.inner, "result");
  const parts: InputPart[] = [];
  if (cmd) parts.push(textPart(`!${trimBlock(cmd.inner)}`, "command"));
  const out = result ? trimBlock(result.inner) : cmd ? "" : trimBlock(el.inner);
  if (out) parts.push(textPart(out, "context", "Shell output"));
  return { parts, length: el.end };
}

/**
 * A message that is wholly `<turn_aborted>…</turn_aborted>`: the same quiet notice as Claude
 * Code's interrupted marker (D167). Codex normally keeps it out of `userMessage` items; a turn
 * it stopped says so by its status ("Worked for … · interrupted").
 */
function marker(text: string): MarkerNotice | null {
  const t = text.trim();
  const el = findElement(t, "turn_aborted");
  if (!el || el.start !== 0 || el.end !== t.length) return null;
  return { noticeKind: "interrupted", text: "Interrupted", details: { during: "turn" } };
}

export const CODEX_MARKUP: MarkupTable = { rules: CODEX_RULES, special, specialStart: /<user_shell_command>/, marker };

/** A placeholder the Codex composer puts where a long paste is (`[Pasted Content 1234 chars]`). */
const PASTE_PLACEHOLDER = /^\[Pasted Content\b/;

/**
 * A `text` input with its `text_elements`: an element whose placeholder names a paste, and
 * whose range holds the pasted text, becomes a pasted part.
 */
export function textWithElements(text: string, elements: unknown): InputPart[] {
  const els = (Array.isArray(elements) ? elements : [])
    .map((e) => e as { byteRange?: { start?: number; end?: number }; byte_range?: { start?: number; end?: number }; placeholder?: unknown })
    .map((e) => ({ range: e.byteRange ?? e.byte_range ?? {}, placeholder: typeof e.placeholder === "string" ? e.placeholder : null }))
    .filter((e) => e.placeholder && PASTE_PLACEHOLDER.test(e.placeholder) && typeof e.range.start === "number" && typeof e.range.end === "number")
    .sort((a, b) => a.range.start! - b.range.start!);
  if (els.length === 0) return [textPart(text)];
  const buf = Buffer.from(text, "utf8");
  const parts: InputPart[] = [];
  let at = 0;
  for (const e of els) {
    const start = e.range.start!;
    const end = e.range.end!;
    if (start < at || end > buf.length || end <= start) continue;
    const pasted = buf.subarray(start, end).toString("utf8");
    if (pasted === e.placeholder) continue; // the range holds only the placeholder: nothing to show apart
    const before = buf.subarray(at, start).toString("utf8");
    if (before.trim()) parts.push(textPart(before.trim()));
    parts.push(textPart(pasted, "pasted"));
    at = end;
  }
  const tail = buf.subarray(at).toString("utf8");
  if (tail.trim() || parts.length === 0) parts.push(textPart(parts.length === 0 ? text : tail.trim()));
  return parts;
}
