// Folder trust (D147): when YAAC starts a Codex thread in a folder of a user's project, yaacd
// marks the folder trusted in Codex through the app-server's `config/read` and
// `config/value/write` — once, only when Codex has no word on it yet, never for threads YAAC
// did not start or agents' paths outside projects, and not with the instance's switch off.
// Against the fake Codex app-server.

import { test } from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as path from "node:path";
import { codexProviderFactory } from "../src/providers/codex/index.js";
import type { Item } from "../src/protocol/generated/types.js";
import { client, opId, readAll, startDaemon, text, tmpHome, until, type TestClient } from "./helpers.js";
import { FakeCodex, rpcError } from "./fixtures/codex/fake-codex.js";

type D = Awaited<ReturnType<typeof startDaemon>>;

interface Rig {
  home: string;
  fake: FakeCodex;
  daemon: D;
  c: TestClient;
  instance: string;
  /** Codex's listing of conversations (for discovery). */
  listed: any[];
}

async function rig(): Promise<Rig> {
  const home = fs.realpathSync(tmpHome("yaac-trust-"));
  const fake = await FakeCodex.start(path.join(home, "codex.sock"));
  const listed: any[] = [];
  let n = 0;
  fake.handlers.set("account/read", () => ({ account: { type: "apiKey" }, requiresOpenaiAuth: true }));
  fake.handlers.set("model/list", () => ({ data: [{ id: "m", model: "m", displayName: "M", isDefault: true }], nextCursor: null }));
  fake.handlers.set("thread/list", (p) => ({ data: p?.archived ? [] : listed, nextCursor: null }));
  fake.handlers.set("thread/read", (p) => ({ thread: { id: p.threadId, status: { type: "idle" }, cwd: home, model: "m" } }));
  fake.handlers.set("thread/start", (p) => ({ thread: { id: `cx-${++n}`, status: { type: "idle" }, cwd: p.cwd, model: "m" }, model: "m" }));
  fake.handlers.set("thread/resume", (p) => ({ thread: { id: p.threadId, status: { type: "idle" }, cwd: p.cwd ?? home }, model: "m" }));
  fake.handlers.set("turn/start", () => ({ turn: { id: `t-${++n}`, status: "inProgress", items: [] } }));
  fake.handlers.set("turn/interrupt", () => ({}));
  // No implicit instance: this test must never reach the real Codex.
  const daemon = await startDaemon(home, { factories: [codexProviderFactory], env: { YAAC_FAKE_PROVIDER: "0", YAAC_CODEX_PROVIDER: "0" } });
  const c = await client(daemon);
  const added: any = await c.request("providerInstance/add", { opId: opId(), kind: "codex", endpoint: { socket: fake.socket } });
  return { home, fake, daemon, c, instance: added.instance.id, listed };
}

async function done(r: Rig): Promise<void> {
  await r.c.close();
  await r.daemon.stop("test");
  await r.fake.close();
  fs.rmSync(r.home, { recursive: true, force: true });
}

function folder(r: Rig, name: string): string {
  const d = path.join(r.home, "work", name);
  fs.mkdirSync(d, { recursive: true });
  return d;
}

async function project(r: Rig, dir: string): Promise<string> {
  const p: any = await r.c.request("project/create", { opId: opId(), folder: dir, name: path.basename(dir) });
  return p.project.id;
}

async function start(r: Rig, cwd: string): Promise<string> {
  const s: any = await r.c.request("thread/start", { opId: opId(), instance: r.instance, cwd, input: text("hello") });
  return s.threadId;
}

async function notices(r: Rig, threadId: string): Promise<Array<{ kind: string; text: string }>> {
  const items: Item[] = (await readAll(r.c, threadId)).flatMap((p) => p.items).reverse();
  return items.filter((i) => i.kind === "notice").map((i: any) => ({ kind: i.noticeKind, text: i.text }));
}

const writes = (r: Rig) => r.fake.requests("config/value/write");

test("folder trust: the capability is reported; a new project folder is marked once, with a visible notice", async () => {
  const r = await rig();
  try {
    const list: any = await r.c.request("provider/list", {});
    assert.ok(list.providers.find((p: any) => p.instance === r.instance).capabilities.includes("folderTrust"));
    const dir = folder(r, "api.v2");
    await project(r, dir);
    const first = await start(r, dir);
    // One write: `projects` upserted with the folder as a key of the value (a dotted key path
    // would split the path at its dots), carrying the version that was read.
    assert.equal(writes(r).length, 1);
    const w = writes(r)[0]!.params;
    assert.equal(w.keyPath, "projects");
    assert.equal(w.mergeStrategy, "upsert");
    assert.deepEqual(w.value, { [dir]: { trust_level: "trusted" } });
    assert.match(w.expectedVersion, /^sha256:/);
    assert.deepEqual(r.fake.userConfig.projects, { [dir]: { trust_level: "trusted" } });
    // Asked before the conversation was created, so this very thread already runs trusted.
    const order = r.fake.received.filter((m) => m.method === "config/value/write" || m.method === "thread/start").map((m) => m.method);
    assert.deepEqual(order, ["config/value/write", "thread/start"]);
    assert.deepEqual(await notices(r, first), [{ kind: "folderTrust", text: `Marked ${dir} as trusted in Codex` }]);
    // The next thread there: nothing asked, nothing said.
    const reads = r.fake.requests("config/read").length;
    const second = await start(r, dir);
    assert.equal(writes(r).length, 1);
    assert.equal(r.fake.requests("config/read").length, reads, "a folder handled once is not read again");
    assert.deepEqual(await notices(r, second), []);
    assert.equal(r.daemon.core.folderTrust.recorded(r.daemon.core.registry.get(r.instance)!, dir)?.outcome, "marked");
  } finally {
    await done(r);
  }
});

test("folder trust: a folder Codex already trusts, or that the user marked untrusted, is left alone and nothing is said", async () => {
  const r = await rig();
  try {
    const [a, b] = [folder(r, "trusted"), folder(r, "untrusted")];
    await project(r, a);
    await project(r, b);
    r.fake.userConfig = { projects: { [a]: { trust_level: "trusted" }, [b]: { trust_level: "untrusted" } } };
    const ta = await start(r, a);
    const tb = await start(r, b);
    assert.equal(r.fake.requests("config/read").length, 2, "read first");
    assert.equal(writes(r).length, 0, "never written over, never downgraded");
    assert.deepEqual(r.fake.userConfig.projects[b], { trust_level: "untrusted" });
    assert.deepEqual(await notices(r, ta), []);
    assert.deepEqual(await notices(r, tb), []);
  } finally {
    await done(r);
  }
});

test("folder trust: not for folders outside projects, an agent's spawn outside projects, or threads YAAC did not start", async () => {
  const r = await rig();
  try {
    const loose = folder(r, "loose");
    const inProject = folder(r, "proj");
    await project(r, inProject);
    // A thread the user started in a folder that is no project of theirs.
    const t = await start(r, loose);
    assert.equal(r.fake.requests("config/read").length, 0);
    assert.deepEqual(await notices(r, t), []);
    // An agent's spawn: outside projects nothing; inside a project, as for the user.
    const spawn = (cwd: string) =>
      r.daemon.core.startThreadForAgent({ instance: r.instance, cwd, title: "worker", parts: text("work"), journalItems: [], mesh: [], spawnedBy: t });
    const outside = await spawn(path.join(loose));
    assert.equal(writes(r).length, 0);
    assert.deepEqual(await notices(r, outside.threadId), []);
    const inside = await spawn(path.join(inProject));
    assert.equal(writes(r).length, 1);
    assert.deepEqual(await notices(r, inside.threadId), [{ kind: "folderTrust", text: `Marked ${inProject} as trusted in Codex` }]);
    // A conversation started elsewhere, found by discovery, in another project's folder.
    const other = folder(r, "other");
    await project(r, other);
    const now = Math.floor(Date.now() / 1000);
    r.listed.push({ id: "cx-cli", name: "From the CLI", preview: "p", cwd: other, createdAt: now - 5, updatedAt: now - 5, source: "cli", status: { type: "notLoaded" } });
    await r.c.request("provider/refresh", { instance: r.instance });
    await until(async () => {
      const s: any = await r.c.request("thread/sync", { token: "" });
      return s.changed.some((t: any) => t.nativeId === "cx-cli");
    }, "discovery");
    assert.equal(writes(r).length, 1, "discovered threads are never marked");
    assert.equal(r.daemon.core.folderTrust.recorded(r.daemon.core.registry.get(r.instance)!, other), null);
  } finally {
    await done(r);
  }
});

test("folder trust: the switch off stops it, kept on the instance without reconnecting", async () => {
  const r = await rig();
  try {
    const dir = folder(r, "off");
    await project(r, dir);
    const before = r.fake.connections;
    const upd: any = await r.c.request("providerInstance/update", { opId: opId(), id: r.instance, trustFolders: false });
    assert.equal(upd.instance.trustFolders, false);
    const listed: any = await r.c.request("providerInstance/list", {});
    assert.equal(listed.instances.find((i: any) => i.id === r.instance).trustFolders, false);
    assert.equal(r.fake.connections, before, "only yaacd's switch changed: the connection is kept");
    const t = await start(r, dir);
    assert.equal(r.fake.requests("config/read").length, 0);
    assert.deepEqual(await notices(r, t), []);
    // On again: absent from the stored configuration, which means on.
    const on: any = await r.c.request("providerInstance/update", { opId: opId(), id: r.instance, trustFolders: true });
    assert.equal(on.instance.trustFolders, undefined);
    await start(r, dir);
    assert.equal(writes(r).length, 1);
  } finally {
    await done(r);
  }
});

test("folder trust: a refused write is one notice, never retried, and the thread starts anyway", async () => {
  const r = await rig();
  try {
    const dir = folder(r, "locked");
    await project(r, dir);
    const write = r.fake.handlers.get("config/value/write")!;
    r.fake.handlers.set("config/value/write", () => rpcError(-32603, "config.toml is locked"));
    const t = await start(r, dir);
    assert.equal(r.fake.requests("thread/start").length, 1, "the thread started");
    const n = await notices(r, t);
    assert.equal(n.length, 1);
    assert.equal(n[0]!.kind, "warning");
    assert.match(n[0]!.text, /^Could not mark .* as trusted in Codex: config\.toml is locked\. YAAC will not try again/);
    const t2 = await start(r, dir);
    assert.equal(writes(r).length, 1, "no blind retry");
    assert.deepEqual(await notices(r, t2), []);
    // A configuration changed between the read and the write is refused by Codex, and said so.
    const other = folder(r, "raced");
    await project(r, other);
    r.fake.handlers.set("config/value/write", write);
    const read = r.fake.handlers.get("config/read")!;
    r.fake.handlers.set("config/read", (p, m) => {
      const out = read(p, m);
      r.fake.userConfig.model = "changed-by-someone-else";
      return out;
    });
    const t3 = await start(r, other);
    assert.match((await notices(r, t3))[0]!.text, /its configuration changed while YAAC was writing/);
    assert.equal(r.fake.userConfig.projects, undefined, "nothing written over the newer file");
  } finally {
    await done(r);
  }
});

test("folder trust: a move that changes the working directory marks the new project folder", async () => {
  const r = await rig();
  try {
    const [a, b] = [folder(r, "from"), folder(r, "to")];
    const pb = await project(r, b);
    const t = await start(r, a);
    assert.equal(writes(r).length, 0);
    r.fake.push({ method: "turn/completed", params: { threadId: "cx-1", turn: { id: "t-2", status: "completed", items: [] } } });
    const moved: any = await r.c.request("thread/move", { opId: opId(), threadId: t, project: pb });
    assert.equal(moved.workingDirectory, b);
    assert.equal(writes(r).length, 1);
    const n = await notices(r, t);
    assert.deepEqual(n.at(-1), { kind: "folderTrust", text: `Marked ${b} as trusted in Codex` });
  } finally {
    await done(r);
  }
});
