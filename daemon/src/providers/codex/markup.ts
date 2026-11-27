// The markup Codex writes into user-role messages, and what each becomes
// (docs/research/harness-surfaces.md#codex-markup). Verified against codex-cli 0.170.0 (its
// binary's fragment markers and the app-server schema) and stored rollouts; see the research doc.
//
// The app-server's `userMessage` items are normally the user's own input: Codex keeps its
// contextual fragments (environment, AGENTS.md, a shell command the user ran, an aborted turn)
// out of them. The table still knows those fragments, so a message that carries one — from an
// older Codex, or a path that does not filter — never shows its tags.
//
// A voice call's handoff (`<realtime_delegation>`, read from the rust-v0.172.0 source) is the
// exception: Codex sends it as the turn's input, so it does arrive as a `userMessage`, live and
// in history. It becomes a notice (`voiceCallDelegation`).

import type { InputPart } from "../../protocol/generated/types.js";
import { findElement, oneLine, textPart, trimBlock, type MarkerNotice, type MarkupTable, type TagRule } from "../markup.js";

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
  const delegation = voiceCallDelegation(t);
  if (delegation) return delegation;
  const el = findElement(t, "turn_aborted");
  if (!el || el.start !== 0 || el.end !== t.length) return null;
  return { noticeKind: "interrupted", text: "Interrupted", details: { during: "turn" } };
}

/** Codex's escaping inside a realtime delegation (`escape_xml_text`: `&`, `<`, `>` only). */
const unescapeDelegation = (s: string) => s.replace(/&lt;/g, "<").replace(/&gt;/g, ">").replace(/&amp;/g, "&");

/** One line of a delegation's transcript: `user: …` or `assistant: …`. */
export interface VoiceCallLine {
  /** Absent for the cut-off head of a truncated transcript (Codex keeps its last 4 KiB). */
  speaker?: "user" | "assistant";
  text: string;
}

const SPEAKER_LABEL = { user: "You", assistant: "Voice" } as const;

/**
 * A voice call's handoff to the thread's agent (codex-cli 0.172.0,
 * `core/src/context/realtime_delegation.rs`): a user-role message that is wholly
 *
 * ```
 * <realtime_delegation>
 *   <source>transcript_tail_flush</source>      (only for the flush at hang-up)
 *   <input>…</input>
 *   <transcript_delta>user: …\nassistant: …</transcript_delta>   (optional)
 * </realtime_delegation>
 * ```
 *
 * It is the realtime model speaking for the user, not the user's own words (invariant 13):
 * a quiet notice, never a user bubble. The transcript becomes speaker-labelled lines.
 */
export function voiceCallDelegation(text: string): MarkerNotice | null {
  const t = text.trim();
  const el = findElement(t, "realtime_delegation");
  if (!el || el.start !== 0 || el.end !== t.length) return null;
  const inner = el.inner;
  const source = findElement(inner, "source");
  const input = findElement(inner, "input");
  if (!input) return null;
  const delta = findElement(inner, "transcript_delta");
  const flush = source ? trimBlock(source.inner) === "transcript_tail_flush" : false;
  const said = unescapeDelegation(input.inner.trim());
  const lines: VoiceCallLine[] = [];
  if (delta) {
    for (const raw of unescapeDelegation(delta.inner).split("\n")) {
      const m = /^(user|assistant): ?(.*)$/.exec(raw);
      if (m) lines.push({ speaker: m[1] as VoiceCallLine["speaker"], text: m[2]! });
      else if (raw.trim() && lines.length > 0) lines[lines.length - 1]!.text += `\n${raw}`;
      else if (raw.trim()) lines.push({ text: raw }); // the cut-off head ("…rest of a line")
    }
  }
  const transcript = lines.map((l) => (l.speaker ? `${SPEAKER_LABEL[l.speaker]}: ${l.text}` : l.text)).join("\n");
  const line = flush ? "Voice call ended · transcript handed to the agent" : `From the voice call: «${oneLine(said, 160)}»`;
  // The flush's input is Codex's fixed note to the agent, not something the user said.
  const body = flush ? transcript : [said, transcript && `Transcript:\n${transcript}`].filter(Boolean).join("\n\n");
  return {
    noticeKind: "info",
    text: line,
    ...(body ? { body } : {}),
    details: { voiceCall: { source: flush ? "transcriptTailFlush" : "handoff", input: said, lines } },
  };
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
