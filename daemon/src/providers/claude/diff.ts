// Unified diffs for Claude Code's file-editing tools (D86): a `fileChange` tool call carries
// its change as unified-diff text in `output`, from every provider, and the window parses it.
//
// From the richest source available: the tool's structured result (live turns: Edit and Write
// report a `structuredPatch` with context lines), else the tool's input (stored history: the
// old and new strings, or a written file's content), diffed line by line.

type Json = Record<string, unknown>;

const str = (v: unknown): string | null => (typeof v === "string" ? v : null);

interface Hunk {
  oldStart: number;
  oldLines: number;
  newStart: number;
  newLines: number;
  lines: string[];
}

function header(oldPath: string | null, newPath: string): string[] {
  return [`--- ${oldPath === null ? "/dev/null" : `a/${oldPath}`}`, `+++ b/${newPath}`];
}

function hunkText(h: Hunk): string[] {
  const range = (start: number, n: number) => (n === 1 ? `${start}` : `${start},${n}`);
  return [`@@ -${range(h.oldStart, h.oldLines)} +${range(h.newStart, h.newLines)} @@`, ...h.lines];
}

function splitLines(text: string): string[] {
  if (text === "") return [];
  const lines = text.split("\n");
  if (lines[lines.length - 1] === "") lines.pop();
  return lines;
}

/** A line diff of two texts, as hunks with up to `context` unchanged lines around each change. */
export function lineHunks(before: string, after: string, context = 3, firstLine = 1): Hunk[] {
  const a = splitLines(before);
  const b = splitLines(after);
  // Longest common subsequence, then the edit script.
  const n = a.length;
  const m = b.length;
  const lcs: number[][] = Array.from({ length: n + 1 }, () => new Array<number>(m + 1).fill(0));
  for (let i = n - 1; i >= 0; i--) for (let j = m - 1; j >= 0; j--) lcs[i]![j] = a[i] === b[j] ? lcs[i + 1]![j + 1]! + 1 : Math.max(lcs[i + 1]![j]!, lcs[i]![j + 1]!);
  const ops: Array<{ t: " " | "-" | "+"; s: string }> = [];
  let i = 0;
  let j = 0;
  while (i < n || j < m) {
    if (i < n && j < m && a[i] === b[j]) {
      ops.push({ t: " ", s: a[i]! });
      i++;
      j++;
    } else if (i < n && (j >= m || lcs[i + 1]![j]! >= lcs[i]![j + 1]!)) ops.push({ t: "-", s: a[i++]! });
    else ops.push({ t: "+", s: b[j++]! });
  }
  const changes = ops.map((o, idx) => (o.t === " " ? -1 : idx)).filter((x) => x >= 0);
  const hunks: Hunk[] = [];
  let g = 0;
  while (g < changes.length) {
    let last = g;
    while (last + 1 < changes.length && changes[last + 1]! - changes[last]! <= context * 2 + 1) last++;
    const start = Math.max(0, changes[g]! - context);
    const end = Math.min(ops.length, changes[last]! + 1 + context);
    let oldNo = firstLine;
    let newNo = firstLine;
    for (const op of ops.slice(0, start)) {
      if (op.t !== "+") oldNo++;
      if (op.t !== "-") newNo++;
    }
    const h: Hunk = { oldStart: oldNo, newStart: newNo, oldLines: 0, newLines: 0, lines: [] };
    for (const op of ops.slice(start, end)) {
      h.lines.push(`${op.t}${op.s}`);
      if (op.t !== "+") h.oldLines++;
      if (op.t !== "-") h.newLines++;
    }
    // An empty side starts at the line before (the unified-diff convention).
    if (h.oldLines === 0) h.oldStart = Math.max(0, h.oldStart - 1);
    if (h.newLines === 0) h.newStart = Math.max(0, h.newStart - 1);
    hunks.push(h);
    g = last + 1;
  }
  return hunks;
}

function fileDiff(oldPath: string | null, newPath: string, hunks: Hunk[]): string {
  return [...header(oldPath, newPath), ...hunks.flatMap(hunkText)].join("\n") + "\n";
}

function fromStructured(path: string, patch: unknown, created: boolean): string | null {
  if (!Array.isArray(patch) || patch.length === 0) return null;
  const hunks: Hunk[] = [];
  for (const raw of patch as Json[]) {
    const lines = Array.isArray(raw.lines) ? (raw.lines as unknown[]).filter((l): l is string => typeof l === "string") : null;
    if (!lines || typeof raw.oldStart !== "number" || typeof raw.newStart !== "number") return null;
    hunks.push({ oldStart: raw.oldStart, oldLines: Number(raw.oldLines) || 0, newStart: raw.newStart, newLines: Number(raw.newLines) || 0, lines });
  }
  return fileDiff(created ? null : path, path, hunks);
}

/** The file a file-editing tool call is about. */
export function editedPath(input: Json): string | null {
  return str(input.file_path) ?? str(input.notebook_path) ?? str(input.path);
}

/**
 * The unified diff of one Claude Code file-editing call (Edit, MultiEdit, Write, NotebookEdit),
 * from its structured result when there is one, else from its input. Null: not enough to say.
 */
export function editDiff(name: string, input: Json, structured?: unknown): string | null {
  const path = editedPath(input) ?? (str((structured as Json | undefined)?.filePath) ?? null);
  if (!path) return null;
  const s = structured && typeof structured === "object" ? (structured as Json) : null;
  if (s) {
    const created = s.type === "create" || (name === "Write" && s.originalFile === null);
    const d = fromStructured(path, s.structuredPatch, created);
    if (d) return d;
    if (name === "Write" && typeof s.content === "string") {
      const before = typeof s.originalFile === "string" ? s.originalFile : null;
      return fileDiff(before === null ? null : path, path, lineHunks(before ?? "", s.content));
    }
  }
  switch (name) {
    case "Edit": {
      const before = str(input.old_string);
      const after = str(input.new_string);
      if (before === null || after === null) return null;
      return fileDiff(path, path, lineHunks(before, after));
    }
    case "MultiEdit": {
      const edits = Array.isArray(input.edits) ? (input.edits as Json[]) : [];
      const hunks = edits.flatMap((e) => (typeof e.old_string === "string" && typeof e.new_string === "string" ? lineHunks(e.old_string, e.new_string) : []));
      return hunks.length > 0 ? fileDiff(path, path, hunks) : null;
    }
    case "Write": {
      const content = str(input.content);
      if (content === null) return null;
      // From the input alone a new file and an overwrite look the same: shown as written.
      return fileDiff(null, path, lineHunks("", content));
    }
    case "NotebookEdit": {
      const after = str(input.new_source);
      if (after === null) return null;
      const before = s ? str(s.old_source) : null;
      return fileDiff(path, path, lineHunks(before ?? "", after));
    }
  }
  return null;
}
