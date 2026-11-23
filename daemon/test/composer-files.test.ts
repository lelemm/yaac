import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import * as fs from "node:fs/promises";
import * as path from "node:path";
import { test } from "node:test";
import { fsSearch } from "../src/session/fsinfo.js";
import { tmpHome } from "./helpers.js";

test("composer file search stays under the cwd and respects git ignore rules", async () => {
  const root = tmpHome();
  const sub = path.join(root, "sub");
  await fs.mkdir(sub);
  await fs.mkdir(path.join(sub, "docs"));
  await fs.writeFile(path.join(root, ".gitignore"), "*.ignored\n");
  await fs.writeFile(path.join(root, "sibling.md"), "");
  await fs.writeFile(path.join(sub, "docs", "design notes.md"), "");
  await fs.writeFile(path.join(sub, "deleted.md"), "");
  await fs.writeFile(path.join(sub, "sensitive.ignored"), "");
  execFileSync("git", ["init", "-q", root]);
  execFileSync("git", ["-C", root, "add", "--", "sub/deleted.md"]);
  await fs.unlink(path.join(sub, "deleted.md"));

  const result = await fsSearch({ cwd: sub, query: "" });
  assert.deepEqual(result.entries, [{
    name: "docs/design notes.md",
    path: path.join(sub, "docs", "design notes.md"),
    kind: "file",
  }]);
  assert.deepEqual((await fsSearch({ cwd: sub, query: "DOCS\\notes" })).entries, []);
  assert.deepEqual((await fsSearch({ cwd: sub, query: "DOCS notes" })).entries, result.entries);
});

test("composer file search also finds nested files in plain folders", async () => {
  const root = tmpHome();
  await fs.mkdir(path.join(root, "docs"));
  await fs.writeFile(path.join(root, "docs", "release notes.txt"), "");
  const result = await fsSearch({ cwd: root, query: "notes" });
  assert.equal(result.entries[0]?.path, path.join(root, "docs", "release notes.txt"));
  assert.equal(result.truncated, false);
});
