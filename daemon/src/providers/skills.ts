// Skill discovery on disk, for a harness that does not list its skills itself (Claude Code).
//
// A skill is a folder with a `SKILL.md` whose front matter names it; a custom slash command is
// a `.md` file under `commands/`. Both are run with `/name`. Nothing here is a hardcoded list:
// it reads what the user and the project keep. Never throws for one unreadable entry.

import * as fs from "node:fs";
import * as path from "node:path";
import type { SkillInfo, SkillScope, SkillsListResult } from "../protocol/generated/types.js";

/** The front matter of a markdown file as flat `key: value` pairs, and nothing else of the file. */
export function frontMatter(text: string): Record<string, string> {
  const m = /^---\r?\n([\s\S]*?)\r?\n---/.exec(text);
  const out: Record<string, string> = {};
  if (!m) return out;
  const lines = m[1]!.split(/\r?\n/);
  for (let i = 0; i < lines.length; i++) {
    const kv = /^([A-Za-z0-9_-]+):\s*(.*)$/.exec(lines[i]!);
    if (!kv) continue;
    let value = kv[2]!.trim();
    // A folded or literal block (`>` / `|`): the indented lines that follow.
    if (/^[>|][+-]?$/.test(value)) {
      const parts: string[] = [];
      while (i + 1 < lines.length && /^\s+\S/.test(lines[i + 1]!)) parts.push(lines[++i]!.trim());
      value = parts.join(" ");
    }
    out[kv[1]!] = value.replace(/^(["'])(.*)\1$/, "$2");
  }
  return out;
}

function firstLine(text: string): string {
  const body = text.replace(/^---\r?\n[\s\S]*?\r?\n---\r?\n?/, "");
  for (const l of body.split(/\r?\n/)) {
    const t = l.replace(/^#+\s*/, "").trim();
    if (t) return t.slice(0, 200);
  }
  return "";
}

function read(file: string): string | null {
  try {
    return fs.readFileSync(file, "utf8");
  } catch {
    return null;
  }
}

function entries(dir: string): fs.Dirent[] {
  try {
    return fs.readdirSync(dir, { withFileTypes: true });
  } catch {
    return [];
  }
}

/** `<root>/<name>/SKILL.md` for each skill folder, and `<commands>/<name>.md` for each command. */
export function scanSkillRoot(root: string, scope: SkillScope, out: SkillsListResult): SkillInfo[] {
  const found: SkillInfo[] = [];
  for (const e of entries(path.join(root, "skills"))) {
    if (!e.isDirectory() && !e.isSymbolicLink()) continue;
    const file = path.join(root, "skills", e.name, "SKILL.md");
    const text = read(file);
    if (text === null) continue;
    const fm = frontMatter(text);
    if (fm["user-invocable"] === "false") continue;
    found.push(info(fm.name || e.name, fm, text, file, scope));
  }
  for (const e of entries(path.join(root, "commands"))) {
    if (!e.isFile() && !e.isSymbolicLink()) continue;
    if (!e.name.endsWith(".md")) continue;
    const file = path.join(root, "commands", e.name);
    const text = read(file);
    if (text === null) {
      out.errors?.push(`${file}: unreadable`);
      continue;
    }
    found.push(info(e.name.slice(0, -3), frontMatter(text), text, file, scope));
  }
  return found;
}

function info(name: string, fm: Record<string, string>, text: string, file: string, scope: SkillScope): SkillInfo {
  const hint = fm["argument-hint"];
  return { name, description: fm.description || firstLine(text), path: file, scope, ...(hint ? { argumentHint: hint } : {}) };
}

/**
 * Claude Code's skills for a folder: the project's (`.claude` in the folder and each parent up
 * to the git root, or the home folder) and the user's (`<configDir>`). A project skill hides a
 * user skill of the same name, and a nearer one a farther one — as Claude Code resolves them.
 */
export function claudeSkills(cwd: string, configDir: string, home: string): SkillsListResult {
  const out: SkillsListResult = { skills: [], errors: [] };
  const byName = new Map<string, SkillInfo>();
  const add = (list: SkillInfo[]) => {
    for (const s of list) if (!byName.has(s.name)) byName.set(s.name, s);
  };
  let dir = path.resolve(cwd);
  const stop = path.resolve(home);
  for (let depth = 0; depth < 32; depth++) {
    if (dir === stop) break;
    add(scanSkillRoot(path.join(dir, ".claude"), "project", out));
    if (fs.existsSync(path.join(dir, ".git"))) break;
    const parent = path.dirname(dir);
    if (parent === dir) break;
    dir = parent;
  }
  add(scanSkillRoot(configDir, "user", out));
  out.skills = [...byName.values()].sort((a, b) => a.name.localeCompare(b.name));
  return out;
}
