// Skills: discovery on disk (Claude Code), the confirmed-load markers of both harnesses, and the
// structured `skill` input of Codex. Synthetic folders only.

import { test } from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { claudeSkills, frontMatter } from "../src/providers/skills.js";
import { skillPathKey, skillRead, mapItem, userInputToParts } from "../src/providers/codex/mapping.js";
import { toolCallItem, withResult } from "../src/providers/claude/translate.js";
import { Skills } from "../src/session/skills.js";
import type { Item } from "../src/protocol/generated/types.js";

function tmp(): string {
  return fs.mkdtempSync(path.join(os.tmpdir(), "yaac-skills-"));
}
function skill(root: string, name: string, front: string): string {
  const dir = path.join(root, "skills", name);
  fs.mkdirSync(dir, { recursive: true });
  fs.writeFileSync(path.join(dir, "SKILL.md"), `---\n${front}\n---\nbody\n`);
  return path.join(dir, "SKILL.md");
}

test("front matter: plain, quoted, and folded values", () => {
  const fm = frontMatter('---\nname: a\ndescription: "Does a thing"\nlong: >\n  one\n  two\n---\nbody');
  assert.deepEqual(fm, { name: "a", description: "Does a thing", long: "one two" });
  assert.deepEqual(frontMatter("no front matter"), {});
});

test("Claude skills: project beats user, nearer beats farther, hidden ones are left out", () => {
  const home = tmp();
  const config = path.join(home, ".claude");
  const repo = path.join(home, "repo");
  const sub = path.join(repo, "pkg");
  fs.mkdirSync(path.join(repo, ".git"), { recursive: true });
  fs.mkdirSync(sub, { recursive: true });
  skill(config, "shared", "description: user one");
  skill(config, "mine", "description: only mine");
  skill(config, "hidden", "description: no\nuser-invocable: false");
  skill(path.join(repo, ".claude"), "shared", "description: repo one\nargument-hint: [file]");
  skill(path.join(sub, ".claude"), "shared", "description: nearest one");
  fs.mkdirSync(path.join(repo, ".claude", "commands"), { recursive: true });
  fs.writeFileSync(path.join(repo, ".claude", "commands", "deploy.md"), "# Deploy it\nsteps");
  const r = claudeSkills(sub, config, home);
  assert.deepEqual(r.skills.map((s) => s.name), ["deploy", "mine", "shared"]);
  const shared = r.skills.find((s) => s.name === "shared")!;
  assert.equal(shared.description, "nearest one");
  assert.equal(shared.scope, "project");
  assert.equal(r.skills.find((s) => s.name === "deploy")!.description, "Deploy it");
  assert.equal(r.skills.find((s) => s.name === "mine")!.scope, "user");
  // Outside any repo the folder's own skills are found, but nothing above the home folder is read.
  assert.equal(claudeSkills(home, config, home).skills.some((s) => s.name === "shared" && s.scope === "project"), false);
});

test("Claude: a Skill call is 'Skill loaded' only after a successful result", () => {
  const call = toolCallItem("t1", "turn", "Skill", { skill: "yaac-scratch-test" }, "/w");
  assert.equal((call as { skill?: unknown }).skill, undefined);
  const done = withResult(call, { type: "tool_result", content: "Launching skill: yaac-scratch-test" }) as Item & { skill?: { name: string }; title: string };
  assert.equal(done.title, "Skill loaded");
  assert.deepEqual(done.skill, { name: "yaac-scratch-test" });
  const failed = withResult(call, { type: "tool_result", is_error: true, content: "Unknown skill" }) as Item & { skill?: unknown; title: string };
  assert.equal(failed.skill, undefined);
  assert.notEqual(failed.title, "Skill loaded");
  // Another tool is never taken for a skill.
  const bash = withResult(toolCallItem("t2", "turn", "Bash", { command: "cat SKILL.md" }, "/w"), { type: "tool_result", content: "x" }) as Item & { skill?: unknown };
  assert.equal(bash.skill, undefined);
});

const SKILLS = new Map([[skillPathKey("/home/u/.codex/skills/plan/SKILL.md"), "plan"]]);
const read = (over: Record<string, unknown> = {}) => ({
  type: "commandExecution",
  id: "c1",
  status: "completed",
  command: "cat /home/u/.codex/skills/plan/SKILL.md",
  cwd: "/w",
  exitCode: 0,
  aggregatedOutput: "x",
  commandActions: [{ type: "read", command: "cat", name: "SKILL.md", path: "/home/u/.codex/skills/plan/SKILL.md" }],
  ...over,
});

test("Codex: only a successful read of a listed SKILL.md is 'Read skill instructions'", () => {
  const ok = mapItem(read() as never, "t", "completed", SKILLS) as Item & { skill?: { name: string }; title: string };
  assert.equal(ok.title, "Read skill instructions");
  assert.deepEqual(ok.skill, { name: "plan" });
  const none = (it: Record<string, unknown>) => (mapItem(it as never, "t", "completed", SKILLS) as Item & { skill?: unknown }).skill;
  assert.equal(none(read({ exitCode: 1 })), undefined, "failed");
  assert.equal(none(read({ status: "inProgress", exitCode: null })), undefined, "not finished");
  assert.equal(none(read({ commandActions: [{ type: "read", command: "cat", name: "other.md", path: "/w/other.md" }] })), undefined, "another file");
  assert.equal(none(read({ commandActions: [{ type: "search", command: "grep", query: "SKILL.md", path: null }] })), undefined, "a search");
  assert.equal(none(read({ commandActions: [{ type: "unknown", command: "cat /home/u/.codex/skills/plan/SKILL.md" }] })), undefined, "unparsed");
  assert.equal((mapItem(read() as never, "t", "completed") as Item & { skill?: unknown }).skill, undefined, "no skills known");
  // A relative read resolves against the command's folder.
  assert.equal(skillRead(read({ cwd: "/home/u/.codex", commandActions: [{ type: "read", path: "skills/plan/SKILL.md" }] }), "completed", SKILLS), "plan");
  // Windows paths compare with either slash.
  assert.equal(skillPathKey("C:\\Users\\u\\.codex\\skills\\plan\\SKILL.md"), "C:/Users/u/.codex/skills/plan/SKILL.md");
});

test("Codex history: a skill input becomes a skill part", () => {
  const parts = userInputToParts([
    { type: "text", text: "$plan do it", text_elements: [] },
    { type: "skill", name: "plan", path: "/p/SKILL.md" },
  ]);
  assert.deepEqual(parts[1], { type: "skill", name: "plan", path: "/p/SKILL.md" });
});

test("skills/list: needs the capability, an instance and an absolute folder", async () => {
  const noList = { descriptor: { displayName: "Fake", capabilities: ["skills"] }, provider: {} };
  const withList = {
    descriptor: { displayName: "Fake", capabilities: ["skills"] },
    provider: { skills: async (cwd: string) => ({ skills: [{ name: cwd, description: "", path: "/x" }], errors: [] }) },
  };
  const bare = { descriptor: { displayName: "Bare", capabilities: [] }, provider: { skills: async () => ({ skills: [] }) } };
  const reg = { get: (id: string) => ({ a: noList, b: withList, c: bare })[id as "a"] } as never;
  const s = new Skills(reg);
  await assert.rejects(s.list({ instance: "a", cwd: "/w" }), /does not list skills/);
  await assert.rejects(s.list({ instance: "c", cwd: "/w" }), /does not list skills/);
  await assert.rejects(s.list({ instance: "zz", cwd: "/w" }), /no provider instance/);
  await assert.rejects(s.list({ instance: "b", cwd: "rel" }), /absolute/);
  assert.deepEqual((await s.list({ instance: "b", cwd: "/w" })).skills[0]!.name, "/w");
});
