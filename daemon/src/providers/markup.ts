// Harness markup inside messages → structured parts (docs/research/harness-surfaces.md#markup-inside-messages).
//
// Harnesses wrap parts of a stored user message in tags that are not the user's text: pasted
// text, slash commands, context for the model. Each provider owns a table of the tags its
// harness writes and passes it here; this module only knows how to apply a table. Only known
// tags are transformed — an unknown `<tag>` and any other text with angle brackets stays text,
// so nothing the user wrote is ever hidden.
//
// YAAC's own wrappers (the mesh frames, the handoff preamble) are the same for every provider
// and live here too.

import type { InputPart, Item, TextRole } from "../protocol/generated/types.js";

/** What a known tag becomes. */
export type TagAs =
  /** Text the user pasted. A code fence directly around the tag belongs to the paste. */
  | { as: "pasted" }
  /** Context for the model, shown only on request. */
  | { as: "context"; label: string }
  /** The tag is the harness's envelope around the user's own words: keep the inside as text. */
  | { as: "unwrap" };

export interface TagRule {
  /** The tag name, exactly as the harness writes it. */
  tag: string;
  what: TagAs;
  /**
   * Only when the element is the whole message (for tag names common enough that a user might
   * type them, like `terminal` or `skill`).
   */
  whole?: boolean;
  /** Text right before the element that is part of it (a heading the harness puts above it). */
  before?: RegExp;
}

/** A provider's table, plus its own steps that a table cannot say (several tags making one part). */
export interface MarkupTable {
  rules: TagRule[];
  /**
   * Runs first on each text: returns the parts for a recognised construct starting at the
   * front of `rest` (and how many characters it took), or null.
   */
  special?: (rest: string) => { parts: InputPart[]; length: number } | null;
  /** Where `special` may match: the start positions of candidate constructs. */
  specialStart?: RegExp;
}

const esc = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

/** `<tag>` or `<tag attr="…">`. */
const openRe = (tag: string) => new RegExp(`<${esc(tag)}((?:\\s+[A-Za-z_:][-A-Za-z0-9_:.]*="[^"]*")*)\\s*>`, "g");
/** `</tag>`, or `</tag attr="…">` (Claude Code repeats the id on the closing tag). */
const closeRe = (tag: string) => new RegExp(`</${esc(tag)}(?:\\s+[A-Za-z_:][-A-Za-z0-9_:.]*="[^"]*")*\\s*>`, "g");

export function textPart(text: string, role?: TextRole, label?: string): InputPart {
  return { type: "text", text, ...(role ? { role } : {}), ...(label ? { label } : {}) } as InputPart;
}

/** Finds the element of `tag` that opens at or after `from`: its bounds and inner text. Nesting of the same tag is counted. */
export function findElement(text: string, tag: string, from = 0): { start: number; end: number; inner: string; attrs: string } | null {
  const o = openRe(tag);
  o.lastIndex = from;
  const m = o.exec(text);
  if (!m) return null;
  let depth = 1;
  let pos = m.index + m[0].length;
  const c = closeRe(tag);
  const o2 = openRe(tag);
  while (depth > 0) {
    c.lastIndex = pos;
    const close = c.exec(text);
    if (!close) return null;
    o2.lastIndex = pos;
    const again = o2.exec(text);
    if (again && again.index < close.index) {
      depth += 1;
      pos = again.index + again[0].length;
      continue;
    }
    depth -= 1;
    if (depth === 0) return { start: m.index, end: close.index + close[0].length, inner: text.slice(m.index + m[0].length, close.index), attrs: m[1] ?? "" };
    pos = close.index + close[0].length;
  }
  return null;
}

/** The value of `name="…"` in an attribute string. */
export function attr(attrs: string, name: string): string | null {
  const m = new RegExp(`\\s${esc(name)}="([^"]*)"`).exec(attrs);
  return m ? m[1]! : null;
}

/** The inside of an element without the blank lines the harness puts around it. */
export function trimBlock(s: string): string {
  return s.replace(/^[ \t]*\r?\n/, "").replace(/^(?:\s*\n)+/, "").replace(/\s+$/, "");
}

/** `&lt;` and friends, as Claude Code escapes text it puts inside its own tags. */
export function unescapeXml(s: string): string {
  return s.replace(/&(lt|gt|quot|apos|amp);/g, (_m, e: string) => ({ lt: "<", gt: ">", quot: '"', apos: "'", amp: "&" })[e]!);
}

/** One line: line breaks and other control characters become single spaces. */
export function oneLine(s: string, max = 200): string {
  let flat = "";
  for (const ch of s) {
    const c = ch.codePointAt(0)!;
    flat += c < 0x20 || c === 0x7f || c === 0x2028 || c === 0x2029 ? " " : ch;
  }
  flat = flat.replace(/\s+/g, " ").trim();
  return flat.length > max ? `${flat.slice(0, max - 1)}…` : flat;
}

const FENCE_BEFORE = /(^|\n)([ \t]*)(`{3,}|~{3,})([^\n`]*)\n\s*$/;

/** Splits one text along a table. Typed text keeps its place between the parts. */
export function splitText(text: string, table: MarkupTable): InputPart[] {
  const out: InputPart[] = [];
  let typed = "";
  const pushTyped = () => {
    const t = typed.replace(/^\s*\n/, "").replace(/\s+$/, "");
    if (t.trim()) out.push(textPart(t));
    typed = "";
  };
  let pos = 0;
  while (pos < text.length) {
    // The nearest known construct from here.
    let best: { start: number; end: number; parts: InputPart[]; absorb?: number; fence?: boolean } | null = null;
    if (table.special && table.specialStart) {
      const re = new RegExp(table.specialStart.source, "g");
      re.lastIndex = pos;
      for (let m = re.exec(text); m; m = re.exec(text)) {
        const s = table.special(text.slice(m.index));
        if (s) {
          best = { start: m.index, end: m.index + s.length, parts: s.parts };
          break;
        }
        re.lastIndex = m.index + 1;
      }
    }
    for (const rule of table.rules) {
      if (best && best.start <= pos) break;
      let el = findElement(text, rule.tag, pos);
      if (!el || (best && el.start >= best.start)) continue;
      if (rule.whole && (text.slice(0, el.start).trim() || text.slice(el.end).trim())) el = null;
      if (!el) continue;
      let start = el.start;
      if (rule.before) {
        const head = new RegExp(`(?:${rule.before.source})$`).exec(text.slice(pos, el.start));
        if (head) start = pos + head.index;
      }
      const inner = trimBlock(el.inner);
      let parts: InputPart[];
      if (rule.what.as === "pasted") parts = [textPart(inner, "pasted")];
      else if (rule.what.as === "context") parts = inner ? [textPart(inner, "context", rule.what.label)] : [];
      else parts = splitText(inner, table);
      best = { start, end: el.end, parts, fence: rule.what.as === "pasted" };
    }
    if (!best) {
      typed += text.slice(pos);
      break;
    }
    let before = text.slice(pos, best.start);
    let end = best.end;
    if (best.fence && best.parts.length === 1) {
      // A fence the user put around the paste: the paste is code, keep the fence with it.
      const f = FENCE_BEFORE.exec(before);
      if (f) {
        const after = new RegExp(`^\\s*\\n[ \\t]*${esc(f[3]!)}[ \\t]*(?:\\n|$)`).exec(text.slice(end));
        if (after) {
          before = before.slice(0, f.index + f[1]!.length);
          end += after[0].length;
          const p = best.parts[0] as { text: string };
          p.text = `${f[3]}${f[4]}\n${p.text}\n${f[3]}`;
        }
      }
    }
    typed += before;
    pushTyped();
    out.push(...best.parts);
    pos = end;
  }
  pushTyped();
  return out;
}

/** Splits every text part of a message; other parts keep their place. */
export function splitParts(parts: InputPart[], table: MarkupTable): InputPart[] {
  const out: InputPart[] = [];
  for (const p of parts) {
    if (p.type === "text" && !(p as { role?: unknown }).role) out.push(...splitText(p.text, table));
    else out.push(p);
  }
  return out.length > 0 ? out : [textPart("")];
}

/** The text of parts as plain text: typed, pasted and command text; context only when asked. */
export function plainOf(parts: InputPart[], withContext = false): string {
  return parts
    .filter((p) => p.type === "text" && (withContext || (p as { role?: string }).role !== "context"))
    .map((p) => (p as { text: string }).text)
    .filter((t) => t.trim())
    .join("\n\n");
}

// ── YAAC's own wrappers ──────────────────────────────────────────────────────────────────

/** The handoff preamble's first line (session/handoff.ts `preamble`). */
export const HANDOFF_HEAD = "[YAAC handoff]";
const HANDOFF_END = "\n---\n";

/** A mesh frame's header (mesh/format.ts `frameMessages`). */
const FRAME_HEAD = /^\[(Task|Message|Result) from (.+?) — via YAAC\b[^\n]*\]\n|^\[Notice from YAAC about (.+?)\]\n|^\[YAAC introduction\]\n/;
const FRAME_END = /\n\[End of (?:task|message|result|notice|introduction)\b[^\n]*\]/;

export interface FramedDelivery {
  /** One line for the notice. */
  text: string;
  /** The bodies, with the frames taken off. */
  body: string;
}

/**
 * A message that is entirely YAAC's mesh frames (the input of a turn the mesh started): what
 * it said, with the frames off. Null when the text is not that.
 */
export function meshFrames(text: string): FramedDelivery | null {
  let rest = text.replace(/^\s+/, "");
  if (!FRAME_HEAD.test(rest)) return null;
  const heads: string[] = [];
  const bodies: string[] = [];
  while (rest) {
    const h = FRAME_HEAD.exec(rest);
    if (!h) return null;
    const after = rest.slice(h[0].length);
    const e = FRAME_END.exec(after);
    if (!e) return null;
    const body = after.slice(0, e.index).replace(/^\(quoted\) /gm, "");
    if (h[1]) {
      const what = h[1] === "Task" ? "Task" : h[1] === "Result" ? "Result" : "Message";
      heads.push(`${what} from ${h[2]}`);
      bodies.push(`${what} from ${h[2]}:\n${body}`);
    } else if (h[3]) {
      heads.push(`Notice about ${h[3]}`);
      bodies.push(`Notice from YAAC about ${h[3]}:\n${body}`);
    } else bodies.push(`YAAC introduction:\n${body}`);
    rest = after.slice(e.index + e[0].length).replace(/^\s+/, "");
  }
  const text1 = heads.length === 0 ? "YAAC introduced this thread to the mesh" : heads.length === 1 ? `${heads[0]} · via YAAC` : `${heads.length} agent messages · via YAAC`;
  return { text: text1, body: bodies.join("\n\n") };
}

/** The handoff preamble at the front of a message: it, and the user's text after it. */
export function handoffPreamble(text: string): { preamble: string; rest: string } | null {
  if (!text.startsWith(HANDOFF_HEAD)) return null;
  const at = text.indexOf(HANDOFF_END);
  if (at < 0) return null;
  return { preamble: text.slice(0, at).trim(), rest: text.slice(at + HANDOFF_END.length).replace(/^\s*\n/, "") };
}

/**
 * A user item from parts, with markup applied: YAAC's own frames first, then the provider's
 * table. A message that is wholly a mesh delivery becomes a notice from YAAC.
 */
export function userItem(base: { id: string; turnId: string; status?: string }, parts: InputPart[], table: MarkupTable): Item {
  const first = parts.find((p) => p.type === "text") as { text: string } | undefined;
  if (first) {
    const framed = meshFrames(first.text);
    if (framed && parts.every((p) => p === first || p.type !== "text")) {
      return { id: base.id, turnId: base.turnId, status: "completed", kind: "notice", noticeKind: "info", text: framed.text, body: framed.body } as Item;
    }
  }
  const pre: InputPart[] = [];
  let rest = parts;
  if (first) {
    const h = handoffPreamble(first.text);
    if (h) {
      pre.push(textPart(h.preamble, "context", "YAAC handoff note"));
      rest = parts.map((p) => (p === (first as unknown) ? textPart(h.rest) : p));
    }
  }
  return { id: base.id, turnId: base.turnId, status: (base.status ?? "completed") as Item["status"], kind: "userMessage", parts: [...pre, ...splitParts(rest, table)] } as Item;
}
