// A real temporary git worktree and an isolated yaacd. No harness or live daemon.
import { execFileSync } from "node:child_process";
import * as fs from "node:fs/promises";
import * as path from "node:path";
import assert from "node:assert/strict";
import { test, type TestContext } from "node:test";
import { fsRead, fsTree, gitFileDiff } from "../src/session/files.js";
import { encodeMessage } from "../src/protocol/rpc.js";
import { startDaemon, TestClient, tmpHome } from "./helpers.js";

const LIMIT = 4 * 1024 * 1024;

function fixture(t: TestContext): string {
  const root = tmpHome("yaac-files-");
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  return root;
}

function git(root: string, ...args: string[]): string {
  return execFileSync("git", ["-C", root, ...args], { encoding: "utf8", env: {
    ...process.env, GIT_CONFIG_NOSYSTEM: "1", GIT_CONFIG_GLOBAL: process.platform === "win32" ? "NUL" : "/dev/null",
    GIT_AUTHOR_NAME: "Test", GIT_AUTHOR_EMAIL: "test@example.com",
    GIT_COMMITTER_NAME: "Test", GIT_COMMITTER_EMAIL: "test@example.com",
  } });
}

function init(root: string): void {
  git(root, "init", "-q", "-b", "base");
  git(root, "config", "core.autocrlf", "false");
}

test("tree lists every git mark, ignores and nested changes without updating the index", async t => {
  const root = fixture(t);
  init(root);
  const dir = (...p: string[]) => path.join(root, ...p);
  await fs.mkdir(dir("nested"));
  await fs.mkdir(dir("gone", "deep"), { recursive: true });
  const tracked = {
    ".gitignore": "*.ignored\nignored-dir/\n",
    "modified.txt": "base\n",
    "deleted.txt": "goodbye\n",
    "original.txt": "rename me\n",
    "conflict.txt": "base\n",
    "nested/deep.txt": "before\n",
    "gone/deep/removed.txt": "gone\n",
  };
  await Promise.all(Object.entries(tracked).map(([file, text]) => fs.writeFile(dir(file), text)));
  git(root, "add", ".");
  git(root, "commit", "-qm", "base");
  git(root, "checkout", "-qb", "incoming");
  await fs.writeFile(dir("conflict.txt"), "incoming\n");
  git(root, "commit", "-qam", "incoming");
  git(root, "checkout", "-q", "base");
  await fs.writeFile(dir("conflict.txt"), "local\n");
  git(root, "commit", "-qam", "local");
  assert.throws(() => git(root, "merge", "--no-edit", "incoming"));

  await fs.writeFile(dir("modified.txt"), "staged\n");
  git(root, "add", "modified.txt");
  await fs.writeFile(dir("modified.txt"), "working\n");
  await fs.rm(dir("deleted.txt"));
  await fs.rm(dir("gone"), { recursive: true });
  git(root, "mv", "original.txt", "new name.txt");
  await fs.writeFile(dir("added.txt"), "added\n");
  git(root, "add", "added.txt");
  await fs.writeFile(dir("untracked.txt"), "new\n");
  await fs.writeFile(dir("scratch.ignored"), "ignored\n");
  await fs.mkdir(dir("ignored-dir"));
  await fs.writeFile(dir("ignored-dir", "hidden.txt"), "hidden\n");
  await fs.writeFile(dir("nested", "deep.txt"), "after\n");
  const indexBefore = await fs.readFile(dir(".git", "index"));

  const tree = await fsTree({ root, path: "" });
  assert.equal(tree.repo, true);
  assert.equal(tree.truncated, false);
  const entry = (name: string) => tree.entries.find(entry => entry.name === name)!;
  for (const [name, mark] of Object.entries({
    "added.txt": "A", "modified.txt": "M", "new name.txt": "R", "conflict.txt": "!",
    "deleted.txt": "D", "untracked.txt": "U",
  })) assert.equal(entry(name).git, mark);
  assert.equal(entry("scratch.ignored").ignored, true);
  assert.equal(entry("ignored-dir").ignored, true);
  assert.equal(entry("ignored-dir").changed, false);
  assert.equal(entry("nested").changed, true);
  assert.equal(entry("gone").changed, true);
  assert.equal(entry("gone").kind, "dir");
  assert.equal(entry("deleted.txt").size, undefined);
  assert.equal(entry("modified.txt").size, 8);
  assert.ok(entry("modified.txt").mtime);
  const buckets = tree.entries.map(item => (item.kind === "dir" ? "0" : "1") + item.name);
  assert.deepEqual(buckets, [...buckets].sort());
  assert.equal((await fsTree({ root, path: "ignored-dir" })).entries[0]!.ignored, true);
  assert.equal((await fsTree({ root, path: "nested" })).entries[0]!.git, "M");
  assert.equal((await fsTree({ root: dir("nested"), path: "" })).entries[0]!.git, "M");
  assert.equal((await fsTree({ root, path: "gone/deep" })).entries[0]!.git, "D");

  for (const [file, status, oldText, newText] of [
    ["modified.txt", "M", "base\n", "working\n"],
    ["added.txt", "A", undefined, "added\n"],
    ["deleted.txt", "D", "goodbye\n", undefined],
    ["gone/deep/removed.txt", "D", "gone\n", undefined],
    ["untracked.txt", "U", undefined, "new\n"],
    ["new name.txt", "R", "rename me\n", "rename me\n"],
  ] as const) {
    const diff = await gitFileDiff({ root, path: file });
    assert.deepEqual([diff.status, diff.old, diff.new, diff.binary], [status, oldText, newText, false]);
    if (status === "R") assert.equal(diff.oldPath, "original.txt");
  }
  const conflict = await gitFileDiff({ root, path: "conflict.txt" });
  assert.equal(conflict.status, "!");
  assert.equal(conflict.old, "local\n");
  assert.match(conflict.new!, /<<<<<<< HEAD/);
  assert.deepEqual(await fs.readFile(dir(".git", "index")), indexBefore);
});

test("all methods reject escapes and the tree lists a link without following it", {
  skip: process.platform === "win32" && "Windows symlinks need Developer Mode or elevation",
}, async t => {
  const root = fixture(t);
  const outside = fixture(t);
  init(root);
  await fs.writeFile(path.join(outside, "secret"), "secret");
  await fs.writeFile(path.join(root, "inside.txt"), "inside");
  await fs.mkdir(path.join(root, "sub"));
  await fs.symlink(outside, path.join(root, "outside"));
  await fs.symlink("inside.txt", path.join(root, "inside"));
  const tree = await fsTree({ root, path: "" });
  assert.equal(tree.entries.find(entry => entry.name === "outside")?.kind, "symlink");
  assert.equal((await fsRead({ root, path: "inside", maxBytes: LIMIT })).text, "inside");
  assert.equal((await fsRead({ root, path: "sub/../inside", maxBytes: LIMIT })).text, "inside");
  for (const file of [`../${path.basename(outside)}/secret`, path.join(outside, "secret"), "outside/secret", "outside/../inside"]) {
    const params = { root, path: file };
    for (const read of [fsTree, gitFileDiff]) await assert.rejects(read(params), { code: "invalid_params" });
    await assert.rejects(fsRead({ ...params, maxBytes: LIMIT }), { code: "invalid_params" });
  }
});

test("binary sniffing, BOM stripping, lossy UTF-8 and file size caps", async t => {
  const root = fixture(t);
  init(root);
  const working = path.join(root, "sample");
  await fs.writeFile(working, Buffer.from([0xef, 0xbb, 0xbf, 0x61, 0xff, 0x62]));
  git(root, "add", ".");
  git(root, "commit", "-qm", "base");
  const params = { root, path: "sample" };
  const text = await fsRead({ ...params, maxBytes: LIMIT });
  assert.deepEqual([text.text, text.size, text.binary, text.truncated], ["a�b", 6, undefined, false]);
  const capped = await fsRead({ ...params, maxBytes: 5 });
  assert.deepEqual([capped.text, capped.size, capped.truncated], [undefined, 6, true]);

  await fs.writeFile(working, Buffer.from([0x61, 0, 0x62]));
  const binary = await fsRead({ ...params, maxBytes: LIMIT });
  assert.deepEqual([binary.binary, binary.size, binary.text], [true, 3, undefined]);
  const diff = await gitFileDiff(params);
  assert.deepEqual([diff.binary, diff.old, diff.new], [true, undefined, undefined]);

  const laterNul = Buffer.alloc(8193, 0x61);
  laterNul[8192] = 0;
  await fs.writeFile(working, laterNul);
  assert.equal((await fsRead({ ...params, maxBytes: LIMIT })).binary, undefined);
  await fs.truncate(working, LIMIT);
  const atLimit = await fsRead({ ...params, maxBytes: LIMIT });
  assert.equal(atLimit.truncated, false);
  assert.ok(atLimit.nextOffset);
  await fs.truncate(working, LIMIT + 1);
  const large = await fsRead({ ...params, maxBytes: LIMIT * 2 });
  assert.deepEqual([large.truncated, large.size, large.text], [true, LIMIT + 1, undefined]);
  await assert.rejects(gitFileDiff(params), { code: "file_too_large", message: /4194305 bytes/ });
  git(root, "add", ".");
  git(root, "commit", "-qm", "large blob");
  await fs.writeFile(working, "short");
  await assert.rejects(gitFileDiff(params), { code: "file_too_large", message: /4194305 bytes/ });
});

test("tree truncates after 5,000 sorted children outside a repository", async t => {
  const root = fixture(t);
  const names = Array.from({ length: 5000 }, (_, i) => `a${String(i).padStart(4, "0")}`);
  for (let i = 0; i < names.length; i += 100) {
    await Promise.all(names.slice(i, i + 100).map(name => fs.writeFile(path.join(root, name), "")));
  }
  await fs.mkdir(path.join(root, "z-dir"));
  const result = await fsTree({ root, path: "" });
  assert.equal(result.entries[0]!.name, "z-dir");
  assert.equal(result.entries.at(-1)!.name, "a4998");
  assert.equal(result.entries.length, 5000);
  assert.equal(result.truncated, true);
  assert.equal(result.repo, false);
  assert.ok(result.entries.every(entry => !entry.git && !entry.changed && !entry.ignored));
});

test("daemon advertises Files and reconstructs bounded UTF-8 pages over the wire", async t => {
  const home = tmpHome("yaacd-files-");
  const root = fixture(t);
  init(root);
  const file = path.join(root, "page.txt");
  const before = Buffer.from("x".repeat(30717) + "🚀" + "©".repeat(40_000) + "\n");
  await fs.writeFile(file, before);
  git(root, "add", ".");
  git(root, "commit", "-qm", "base");
  const after = Buffer.concat([
    Buffer.from("\uFEFF" + "x".repeat(15_357) + "\t\"🧪".repeat(200_000)),
    Buffer.from([0xff, 0xe2, 0x80, 0x20]),
  ]);
  await fs.writeFile(file, after);
  const daemon = await startDaemon(home);
  t.after(async () => {
    await daemon.stop("test");
    await fs.rm(home, { recursive: true, force: true });
  });
  const client = await TestClient.connect(daemon.socketPath);
  t.after(() => client.close());
  const hello = await client.initialize();
  assert.ok(hello.features.includes("fileView"));
  const tree = await client.request("fs/tree", { root, path: "" });
  assert.equal(tree.entries.find((entry: { name: string }) => entry.name === "page.txt").git, "M");

  const params = { root, path: "page.txt" };
  let page = await client.request("fs/read", { ...params, maxBytes: LIMIT });
  const initial = page.etag;
  let read = "";
  for (;;) {
    assert.equal(page.etag, initial);
    assert.equal(page.size, after.length);
    assert.ok(Buffer.byteLength(encodeMessage({ type: "response", id: 1, result: page })) < 64 * 1024);
    read += page.text;
    if (page.nextOffset === undefined) break;
    page = await client.request("fs/read", { ...params, maxBytes: LIMIT, offset: page.nextOffset });
  }
  const expected = after.toString("utf8").replace(/^\uFEFF/, "");
  assert.equal(read, expected);
  let diff = await client.request("git/fileDiff", params);
  assert.ok(diff.nextOldOffset && diff.nextNewOffset);
  const etag = diff.etag;
  let oldText = "";
  let newText = "";
  for (;;) {
    assert.equal(diff.etag, etag);
    assert.ok(Buffer.byteLength(encodeMessage({ type: "response", id: 2, result: diff })) < 64 * 1024);
    oldText += diff.old ?? "";
    newText += diff.new ?? "";
    if (diff.nextOldOffset === undefined && diff.nextNewOffset === undefined) break;
    diff = await client.request("git/fileDiff", { ...params, oldOffset: diff.nextOldOffset, newOffset: diff.nextNewOffset });
  }
  assert.equal(oldText, before.toString("utf8"));
  assert.equal(newText, expected);
  await fs.writeFile(file, Buffer.concat([after, Buffer.from("!")]));
  assert.notEqual((await client.request("fs/read", { ...params, maxBytes: LIMIT })).etag, initial);
  assert.notEqual((await client.request("git/fileDiff", params)).etag, etag);
});
