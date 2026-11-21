// The handoff fork (docs/05-threads.md#handoff-fork--everything-else): the source thread's
// history, read from the harness's own store, rendered to a markdown transcript; the fixed
// preamble that points the new agent at it; and where the file lives.
//
// The renderer is shared by every provider: it works on YAAC items (what each provider's
// `readThread` returns), never on a harness's own format.

import type { InputPart, Item, TurnId } from "../protocol/generated/types.js";

/** Bound on the transcript the `handoff` item carries in the journal (the file has all of it). */
export const HANDOFF_ITEM_BYTES = 48 * 1024;
/** One tool call is one line: its title is cut to this many characters. */
export const TOOL_TITLE_CHARS = 160;
/** A failed tool call's line carries this much of its output, on one line. */
export const TOOL_ERROR_CHARS = 160;

/** What the renderer needs to know about attachments; the session core answers from the upload store. */
export interface AttachmentLookup {
  /** An uploaded attachment still on the target: its path there (and name). Null: gone. */
  upload(uploadId: string): { path: string; name: string } | null;
  /** Whether a path a harness recorded (e.g. Codex `localImage`) still exists. */
  exists(path: string): boolean;
}

export interface TranscriptSource {
  title: string;
  /** "Codex", "Claude · work". */
  provider: string;
  /** The target's name ("devbox"), or "this machine". */
  target: string;
  threadId: string;
}

export interface RenderedTranscript {
  markdown: string;
  /** Turns carried. */
  turns: number;
  /** Attachments that are gone from the target, as named in the transcript. */
  omitted: string[];
}

/** The turns of `items`, in order of first appearance. */
export function turnOrder(items: Item[]): TurnId[] {
  const out: TurnId[] = [];
  const seen = new Set<string>();
  for (const it of items) {
    if (!it.turnId || seen.has(it.turnId)) continue;
    seen.add(it.turnId);
    out.push(it.turnId);
  }
  return out;
}

/**
 * The turn a fork cuts off after: `atTurn` when given, else the last turn that is not
 * running. `running` are the turns in progress (here, or as the harness reports).
 */
export function cutoffTurn(items: Item[], running: Array<TurnId | null | undefined>, atTurn?: TurnId | null): TurnId | null {
  const turns = turnOrder(items);
  if (atTurn) return turns.includes(atTurn) ? atTurn : null;
  const busy = new Set(running.filter((t): t is TurnId => !!t));
  // Items still in progress also mark a turn as running (a turn driven elsewhere).
  for (const it of items) if (it.status === "inProgress" && it.kind !== "request") busy.add(it.turnId);
  const done = turns.filter((t) => !busy.has(t));
  return done.at(-1) ?? null;
}

/** The items up to and including turn `cutoff` (none when there is no cut-off). */
export function itemsThrough(items: Item[], cutoff: TurnId | null): Item[] {
  if (!cutoff) return [];
  const turns = turnOrder(items);
  const keep = new Set(turns.slice(0, turns.indexOf(cutoff) + 1));
  return items.filter((i) => keep.has(i.turnId));
}

function oneLine(s: string, max: number): string {
  const flat = s.replace(/\s+/g, " ").trim();
  return flat.length > max ? `${flat.slice(0, max - 1)}…` : flat;
}

function basename(p: string): string {
  const parts = p.split(/[\\/]/);
  return parts[parts.length - 1] || p;
}

/** Codex records a local image as a marker in the message text (`[image: <path>]`). */
const IMAGE_MARKER = /\[image: ([^\]\n]+)\]/g;

function renderUser(parts: InputPart[], supplement: InputPart[], look: AttachmentLookup, omitted: string[]): string {
  const lines: string[] = [];
  const attach = (kind: "image" | "file", uploadId: string, name: string | null | undefined) => {
    const found = look.upload(uploadId);
    const label = name || found?.name || basename(uploadId);
    if (found) lines.push(`[${kind}: ${found.path}]`);
    else {
      omitted.push(label);
      lines.push(`[${kind} omitted: ${label} — no longer on the target]`);
    }
  };
  const own = parts.filter((p) => p.type !== "text");
  for (const p of parts) {
    if (p.type !== "text") continue;
    lines.push(
      p.text.replace(IMAGE_MARKER, (_m, raw: string) => {
        const where = raw.trim();
        if (where.startsWith("/") && !look.exists(where)) {
          omitted.push(basename(where));
          return `[image omitted: ${basename(where)} — no longer on the target]`;
        }
        return `[image: ${where}]`;
      }),
    );
  }
  // The harness may not keep attachments as such (Claude stores the bytes inline); what yaacd
  // journaled for the same turn names them.
  for (const p of own.length > 0 ? own : supplement) {
    if (p.type === "image" || p.type === "file") attach(p.type, p.uploadId, p.name);
  }
  return lines.join("\n\n").trim();
}

function toolLine(it: Extract<Item, { kind: "toolCall" }>): string {
  const what = oneLine(it.title || it.tool, TOOL_TITLE_CHARS);
  const code = typeof it.exitCode === "number" ? ` (exit ${it.exitCode})` : "";
  let outcome: string;
  if (it.status === "declined") outcome = "declined";
  else if (it.status === "inProgress") outcome = "did not finish";
  else if (it.status === "failed" || (typeof it.exitCode === "number" && it.exitCode !== 0)) {
    const err = typeof it.output === "string" ? oneLine(it.output, TOOL_ERROR_CHARS) : "";
    outcome = `failed${code}${err ? `: ${err}` : ""}`;
  } else outcome = `succeeded${code}`;
  return `- Tool (${it.tool}): \`${what.replace(/`/g, "'")}\` — ${outcome}`;
}

/**
 * Renders `items` (oldest first, already cut at the fork point) as a markdown transcript:
 * user and agent messages in full, one line per tool call with its outcome, agent-mesh
 * messages with their sender; reasoning and tool output left out.
 */
export function renderTranscript(items: Item[], source: TranscriptSource, look: AttachmentLookup, journaled: Item[] = []): RenderedTranscript {
  const omitted: string[] = [];
  const turns = turnOrder(items);
  const userParts = new Map<string, InputPart[]>();
  for (const j of journaled) {
    if (j.kind !== "userMessage") continue;
    const extra = j.parts.filter((p) => p.type !== "text");
    if (extra.length > 0) userParts.set(j.turnId, [...(userParts.get(j.turnId) ?? []), ...extra]);
  }
  // Blocks are separated by a blank line; consecutive list lines (tool calls) are not.
  const blocks: string[] = [
    "# Prior conversation",
    `Forked from "${oneLine(source.title, 120)}" (${source.provider} on ${source.target}, thread ${source.threadId}), through turn ${turns.length}.\nHidden reasoning and full tool output are not included.`,
  ];
  const line = (l: string) => blocks.push(l);
  let turnNo = 0;
  let current: string | null = null;
  const supplementUsed = new Set<string>();
  for (const it of items) {
    if (it.turnId !== current) {
      current = it.turnId;
      turnNo += 1;
      blocks.push(`## Turn ${turnNo}`);
    }
    switch (it.kind) {
      case "userMessage": {
        const supplement = supplementUsed.has(it.turnId) ? [] : (userParts.get(it.turnId) ?? []);
        supplementUsed.add(it.turnId);
        const text = renderUser(it.parts, supplement, look, omitted);
        if (text) blocks.push("**User:**", text);
        break;
      }
      case "agentMessage":
        if (it.text.trim()) blocks.push("**Agent:**", it.text.trim());
        break;
      case "toolCall":
        line(toolLine(it));
        break;
      case "meshReceived":
        blocks.push(`**Message from agent ${it.message.from}:**`, it.message.text.trim());
        break;
      case "meshSent":
        blocks.push(`**Message to agent ${it.to.address}:**`, it.message.text.trim());
        break;
      case "meshSpawn":
        line(`- Started ${it.threads.length} agent thread${it.threads.length === 1 ? "" : "s"}`);
        break;
      case "plan":
        line(`- Plan: ${it.steps.map((s) => oneLine(s.text, 80)).join("; ")}`);
        break;
      case "request":
        line(`- ${it.requestKind === "question" || it.requestKind === "form" ? "Question to the user" : "Approval asked"} — ${it.state}`);
        break;
      // Reasoning, notices, an earlier handoff marker, and waits are not carried.
      default:
        break;
    }
  }
  let markdown = "";
  for (let i = 0; i < blocks.length; i++) {
    if (i > 0) markdown += blocks[i]!.startsWith("- ") && blocks[i - 1]!.startsWith("- ") ? "\n" : "\n\n";
    markdown += blocks[i];
  }
  markdown += "\n";
  return { markdown, turns: turns.length, omitted };
}

export interface PreambleSource {
  /** The source thread's target, as the mesh names it. */
  target: string;
  threadId: string;
  /** Its mesh address, when the mesh is attached. */
  address?: string | null;
}

/**
 * The fixed preamble the first message of a handoff fork starts with. Mentions the `yaac`
 * tools only when the new thread is offered them.
 */
export function preamble(transcriptPath: string, source: PreambleSource, meshTools: boolean): string {
  const lines = [
    "[YAAC handoff] This conversation continues an earlier one, held with a different agent.",
    `The prior conversation is in this file: ${transcriptPath}`,
    "Read it before you answer; it is the history of this conversation. It leaves out hidden reasoning and full tool output.",
    `The earlier thread is ${source.threadId} on ${source.target}.`,
  ];
  if (meshTools) {
    lines.push(
      `You can read it or message it with the yaac tools (read_thread, send_message${source.address ? `; its address is ${source.address}` : ""}).`,
    );
  }
  lines.push("The user's message follows.", "", "---", "", "");
  return lines.join("\n");
}

/** Keeps the end of a transcript within `max` bytes, cut at a turn heading where possible. */
export function boundTranscript(markdown: string, max: number, filePath: string): string {
  if (Buffer.byteLength(markdown) <= max) return markdown;
  const note = `*The earlier part of this transcript is in ${filePath}.*\n\n`;
  const budget = max - Buffer.byteLength(note);
  const buf = Buffer.from(markdown);
  let tail = buf.subarray(buf.length - budget).toString("utf8");
  // A cut inside a character leaves a replacement character at the start: drop it.
  tail = tail.replace(/^�+/, "");
  const at = tail.indexOf("\n## Turn ");
  if (at >= 0) tail = tail.slice(at + 1);
  return note + tail;
}

/** The first `max` bytes of a transcript, for the fork dialog's preview. */
export function previewOf(markdown: string, max: number): string {
  const buf = Buffer.from(markdown);
  if (buf.length <= max) return markdown;
  return `${buf.subarray(0, max).toString("utf8").replace(/�+$/, "")}\n…`;
}
