// Providers, the thread index, discovery, fork, thread management, pickers, and lifecycle.

import { test } from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as path from "node:path";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { FAKE, TestClient, client, fakeOf, opId, readAll, sleep, startDaemon, startThread, text, tmpHome, until, waitTurnDone, RpcFailure } from "./helpers.js";
import { AlreadyServing } from "../src/session/server.js";
import { normaliseRemote } from "../src/session/fsinfo.js";

const code = (c: string) => (e: unknown) => e instanceof RpcFailure && e.code === c;

test("before initialize only the frozen calls work; the mesh methods the daemon serves exist", async () => {
  const daemon = await startDaemon(tmpHome());
  const c = await TestClient.connect(daemon.socketPath);
  await assert.rejects(c.request("ping"), code("not_initialized"));
  const st: any = await c.request("daemon/status");
  assert.deepEqual(st.protocol, { min: 1, max: 1 });
  await c.initialize();
  assert.deepEqual(await c.request("ping"), {});
  await assert.rejects(c.request("initialize", { protocol: { min: 1, max: 1 }, client: { name: "x", version: "1" } }), code("invalid_request"));
  // The mesh (WP7): `mesh/relay` is a request the daemon sends, never one it serves; the
  // methods the hub and the window send exist and check their params.
  await assert.rejects(c.request("mesh/relay", {}), code("method_not_found"));
  for (const m of ["mesh/deliver", "mesh/query", "mesh/roster", "mesh/budget", "mesh/receipt", "mesh/usage"]) {
    await assert.rejects(c.request(m, {}), code("invalid_params"));
  }
  assert.ok(((await c.request("mesh/policy", {})) as any).policy.limits.spawnDepth > 0);
  await assert.rejects(c.request("no/such"), code("method_not_found"));
  // item/read exists now; without its params it is invalid, not unknown.
  await assert.rejects(c.request("item/read", {}), code("invalid_params"));
  const diag: any = await c.request("diagnostics/get");
  assert.ok(diag.sections.some((s: any) => s.title === "daemon"));
  await c.close();
  await daemon.stop("test");
});

test("providers: the fake's descriptor, instances, revisions, and two instances sharing a store", async () => {
  const home = tmpHome();
  const daemon = await startDaemon(home);
  const c = await client(daemon);
  const list: any = await c.request("provider/list");
  const fake = list.providers.find((p: any) => p.instance === FAKE);
  assert.equal(fake.kind, "fake");
  assert.equal(fake.displayName, "Fake");
  assert.ok(fake.revision >= 1);
  assert.ok(fake.capabilities.includes("threadFork"));
  assert.deepEqual(fake.models.map((m: any) => m.id), ["fake-large", "fake-small", "fake-mystery"]);
  const inst: any = await c.request("providerInstance/list");
  assert.equal(inst.instances[0].implicit, true);
  await assert.rejects(c.request("providerInstance/remove", { opId: opId(), id: FAKE }), code("invalid_params"));
  await assert.rejects(c.request("providerInstance/add", { opId: opId(), kind: "nope", enabled: true, backend: "auto" }), code("invalid_params"));
  // A second instance over the same store: its conversations are the same threads.
  const store = path.join(home, "providers", FAKE, "store");
  const added: any = await c.request("providerInstance/add", { opId: opId(), kind: "fake", name: "twin", enabled: true, backend: "auto", env: { FAKE_STORE: store } });
  const twin = added.instance.id;
  await c.request("provider/refresh", {});
  const all: any = await c.request("provider/list");
  const t = all.providers.find((p: any) => p.instance === twin);
  assert.equal(t.displayName, "Fake · twin");
  assert.equal(t.store.key, fake.store.key);
  const sync: any = await c.request("thread/sync", {});
  const titles = sync.changed.map((s: any) => s.title);
  assert.equal(titles.filter((x: string) => x.startsWith("Refactor the parser")).length, 1, "listed once");
  // An update bumps the revision and is pushed.
  await c.request("providerInstance/update", { opId: opId(), id: twin, name: "renamed" });
  const upd = await until(() => c.notifications.find((n) => n.method === "provider/updated" && n.params.provider.instance === twin && n.params.provider.name === "renamed"), "provider/updated");
  assert.ok(upd.params.provider.revision > t.revision);
  await c.request("providerInstance/remove", { opId: opId(), id: twin });
  assert.equal(((await c.request("provider/list")) as any).providers.length, 1);
  await c.close();
  await daemon.stop("test");
});

test("thread index: full listing with threads created outside YAAC, sync tokens, paging, stale scans", async () => {
  const home = tmpHome();
  const daemon = await startDaemon(home);
  const c = await client(daemon);
  const full: any = await c.request("thread/sync", {});
  assert.equal(full.full, true);
  const origins = full.changed.map((s: any) => s.originKind).sort();
  assert.deepEqual(origins, ["interactive", "programmatic"]);
  const t = await startThread(c, "/fake say hi");
  await c.request("thread/subscribe", { threadId: t.threadId, epoch: 1, afterSeq: 0 });
  await waitTurnDone(c, t.threadId, t.turnId);
  const inc: any = await c.request("thread/sync", { token: full.token });
  assert.equal(inc.full, false);
  assert.ok(inc.changed.some((s: any) => s.id === t.threadId && s.originKind === "yaac"));
  assert.ok(!inc.changed.some((s: any) => s.originKind === "interactive"), "only what changed");
  const pushed = c.notifications.filter((n) => n.method === "thread/updated" && n.params.thread.id === t.threadId);
  assert.ok(pushed.length >= 1);
  // A conversation started outside YAAC appears (pushed).
  const fake = fakeOf(daemon);
  fake.addOutsideThread("Started in a terminal just now");
  await until(() => c.notifications.find((n) => n.method === "thread/updated" && n.params.thread.title === "Started in a terminal just now"), "outside thread pushed");
  // Paging by bytes.
  const p1: any = await c.request("thread/sync", { maxBytes: 1024 });
  assert.equal(p1.more, true);
  assert.ok(p1.changed.length >= 1 && p1.changed.length < 4);
  let n = p1.changed.length;
  let tok = p1.token;
  for (let i = 0; i < 10; i++) {
    const p: any = await c.request("thread/sync", { token: tok, maxBytes: 1024 });
    n += p.changed.length;
    tok = p.token;
    if (!p.more) break;
  }
  assert.equal(n, 4);
  // A failed scan marks the instance stale and removes nothing.
  fake.failNextScan = true;
  await c.request("provider/refresh", { instance: FAKE });
  const after: any = await c.request("thread/sync", {});
  assert.deepEqual(after.staleInstances, [FAKE]);
  assert.equal(after.changed.length, 4);
  // A token from another generation gets a full listing.
  const other: any = await c.request("thread/sync", { token: "idxzzz:3" });
  assert.equal(other.full, true);
  await c.close();
  await daemon.stop("test");
});

test("native fork, fork preview, rename, archive, delete", async () => {
  const home = tmpHome();
  const daemon = await startDaemon(home);
  const c = await client(daemon);
  const src = await startThread(c, "/fake say original answer");
  await c.request("thread/subscribe", { threadId: src.threadId, epoch: 1, afterSeq: 0 });
  await waitTurnDone(c, src.threadId, src.turnId);
  const preview: any = await c.request("thread/forkPreview", { source: src.threadId, instance: FAKE });
  assert.equal(preview.forkKind, "native");
  assert.equal(preview.cutoffTurn, src.turnId);
  const fork: any = await c.request("thread/fork", { opId: opId(), source: src.threadId, instance: FAKE, input: text("/fake say forked reply") });
  assert.equal(fork.forkKind, "native");
  await c.request("thread/subscribe", { threadId: fork.threadId, epoch: 1, afterSeq: 0 });
  await waitTurnDone(c, fork.threadId, fork.turnId);
  const items = (await readAll(c, fork.threadId)).flatMap((p) => p.items).reverse();
  const kinds = items.map((i) => i.kind);
  assert.deepEqual(kinds, ["userMessage", "agentMessage", "handoff", "userMessage", "agentMessage"]);
  const sync: any = await c.request("thread/sync", {});
  assert.equal(sync.changed.find((s: any) => s.id === fork.threadId).forkedFrom, src.threadId);
  await assert.rejects(c.request("thread/fork", { opId: opId(), source: src.threadId, instance: FAKE, atTurn: src.turnId, input: text("x") }), code("capability_missing"));

  await c.request("thread/rename", { opId: opId(), threadId: src.threadId, title: "Renamed" });
  await c.request("thread/archive", { opId: opId(), threadId: src.threadId });
  const s2: any = await c.request("thread/sync", {});
  const row = s2.changed.find((s: any) => s.id === src.threadId);
  assert.equal(row.title, "Renamed");
  assert.equal(row.archived, true);
  await c.request("thread/delete", { opId: opId(), threadId: src.threadId });
  await until(() => c.notifications.find((n) => n.method === "thread/removed" && n.params.threadId === src.threadId), "thread/removed");
  const s3: any = await c.request("thread/sync", { token: s2.token });
  assert.deepEqual(s3.removed, [src.threadId]);
  await assert.rejects(c.request("thread/read", { threadId: src.threadId }), code("thread_not_found"));
  await c.close();
  await daemon.stop("test");
});

test("delete (D110): YAAC's record always; the harness's session only when asked", async () => {
  const home = tmpHome();
  const daemon = await startDaemon(home);
  const c = await client(daemon);
  const fake = fakeOf(daemon);
  const a = await startThread(c, "/fake say one");
  const b = await startThread(c, "/fake say two");
  await c.request("thread/subscribe", { threadId: a.threadId, epoch: 1, afterSeq: 0 });
  await c.request("thread/subscribe", { threadId: b.threadId, epoch: 1, afterSeq: 0 });
  await waitTurnDone(c, a.threadId, a.turnId);
  await waitTurnDone(c, b.threadId, b.turnId);
  const nativeA = daemon.core.thread(a.threadId)!.record.native;
  const nativeB = daemon.core.thread(b.threadId)!.record.native;
  // Also in the harness: the session is gone there too.
  await c.request("thread/delete", { opId: opId(), threadId: a.threadId, alsoInHarness: true });
  await until(() => c.notifications.find((n) => n.method === "thread/removed" && n.params.threadId === a.threadId), "a removed");
  assert.ok(!fake.nativeIds().includes(nativeA), "the harness deleted its session");
  // YAAC's record only: the harness keeps it, and a rescan does not bring it back.
  await c.request("thread/delete", { opId: opId(), threadId: b.threadId, alsoInHarness: false });
  await until(() => c.notifications.find((n) => n.method === "thread/removed" && n.params.threadId === b.threadId), "b removed");
  assert.ok(fake.nativeIds().includes(nativeB), "the harness keeps its session");
  assert.equal(daemon.core.thread(b.threadId), undefined);
  await daemon.core.scanAll();
  const sync: any = await c.request("thread/sync", {});
  assert.ok(!sync.changed.some((s: any) => s.nativeId === nativeB), "not rediscovered");
  // The same operation id again is answered from the log, never acted twice.
  const op = opId();
  const x = await startThread(c, "/fake say three");
  await c.request("thread/subscribe", { threadId: x.threadId, epoch: 1, afterSeq: 0 });
  await waitTurnDone(c, x.threadId, x.turnId);
  await c.request("thread/delete", { opId: op, threadId: x.threadId });
  assert.deepEqual(await c.request("thread/delete", { opId: op, threadId: x.threadId }), {});
  // Asked by its own id, it is wanted again.
  const found: any = await c.request("thread/lookup", { id: nativeB });
  assert.equal(found.threads.length, 1);
  await c.close();
  await daemon.stop("test");
});

test("input validation: models from the live list, images only where the model can see them", async () => {
  const home = tmpHome();
  const daemon = await startDaemon(home);
  const c = await client(daemon);
  await assert.rejects(startThread(c, "hi", { model: "gpt-nonexistent" }), code("model_unavailable"));
  const png = Buffer.from("89504e470d0a1a0a0000000d49484452", "hex");
  const b: any = await c.request("upload/begin", { name: "shot.png", size: png.length, sha256: createHash("sha256").update(png).digest("hex") });
  c.sendData(b.stream, png);
  await c.request("upload/end", { uploadId: b.uploadId });
  const image = [{ type: "image", uploadId: b.uploadId }];
  await assert.rejects(startThread(c, "x", { model: "fake-small", input: image }), code("capability_missing"));
  const ok: any = await startThread(c, "x", { model: "fake-large", input: [{ type: "text", text: "/fake say I see it" }, ...image] });
  await c.request("thread/subscribe", { threadId: ok.threadId, epoch: 1, afterSeq: 0 });
  const sub: any = await c.request("thread/subscribe", { threadId: ok.threadId, epoch: 1, afterSeq: 0 });
  assert.equal(sub.effective.imageInput, "supported");
  assert.deepEqual(sub.effective.applied.model, "fake-large");
  await waitTurnDone(c, ok.threadId, ok.turnId);
  assert.equal(fakeOf(daemon).received.at(-1)!.input.images.length, 1);
  // A rejected request did not act, so the same opId can be used once it is corrected.
  const op = opId();
  await assert.rejects(c.request("turn/start", { opId: op, threadId: ok.threadId, input: [] }), code("invalid_params"));
  const r: any = await c.request("turn/start", { opId: op, threadId: ok.threadId, input: text("/fake say fine") });
  await waitTurnDone(c, ok.threadId, r.turnId);
  await assert.rejects(c.request("turn/steer", { opId: opId(), threadId: ok.threadId, turnId: r.turnId, input: text("late") }), code("turn_mismatch"));
  await c.close();
  await daemon.stop("test");
});

test("steering the running turn by its id", async () => {
  const daemon = await startDaemon(tmpHome());
  const c = await client(daemon);
  const t = await startThread(c, "/fake sleep 300\n/fake say after the pause");
  await c.request("thread/subscribe", { threadId: t.threadId, epoch: 1, afterSeq: 0 });
  await c.request("turn/steer", { opId: opId(), threadId: t.threadId, turnId: t.turnId, input: text("also check the docs") });
  await waitTurnDone(c, t.threadId, t.turnId);
  const items = (await readAll(c, t.threadId)).flatMap((p) => p.items);
  assert.ok(items.some((i) => (i as any).text === "Steered: also check the docs"));
  await c.close();
  await daemon.stop("test");
});

test("fs/list and project/resolve", async () => {
  const home = tmpHome();
  const repo = path.join(home, "work", "myrepo");
  fs.mkdirSync(path.join(repo, "src"), { recursive: true });
  fs.writeFileSync(path.join(repo, ".hidden"), "");
  fs.writeFileSync(path.join(repo, "a.txt"), "");
  const daemon = await startDaemon(home);
  const c = await client(daemon);
  const l: any = await c.request("fs/list", { path: repo });
  assert.deepEqual(l.entries.map((e: any) => `${e.kind}:${e.name}`), ["directory:src", "file:a.txt"]);
  assert.equal(l.parent, path.dirname(repo));
  const plain: any = await c.request("project/resolve", { path: repo });
  assert.equal(plain.project.path, repo);
  assert.equal(plain.worktree ?? null, null);
  let git = true;
  try {
    execFileSync("git", ["init", "-q", "-b", "main", repo]);
    execFileSync("git", ["-C", repo, "remote", "add", "origin", "git@github.com:Example/MyRepo.git"]);
  } catch {
    git = false;
  }
  if (git) {
    const r: any = await c.request("project/resolve", { path: path.join(repo, "src") });
    assert.equal(fs.realpathSync(r.gitRoot), fs.realpathSync(repo));
    assert.equal(r.branch, "main");
    assert.equal(r.originUrl, "git@github.com:Example/MyRepo.git");
    assert.equal(r.project.displayName, "myrepo");
    assert.equal(r.project.crossTargetKey, "github.com/Example/MyRepo");
  }
  assert.equal(normaliseRemote("https://user@GitHub.com/Example/MyRepo.git"), "github.com/Example/MyRepo");
  await c.close();
  await daemon.stop("test");
});

test("one serving daemon per user; idle exit; hand-over when idle", async () => {
  const home = tmpHome();
  let reason = "";
  const daemon = await startDaemon(home, { idleTimeoutMs: 300, onExit: (r) => (reason = r) });
  const index = path.join(home, "index.json");
  const generation = () => JSON.parse(fs.readFileSync(index, "utf8")).generation;
  const before = generation();
  await assert.rejects(startDaemon(home), AlreadyServing);
  assert.equal(generation(), before, "the losing daemon never rewrites the serving daemon's index");
  const c = await client(daemon);
  await sleep(500);
  assert.equal(reason, "", "an attached client keeps it up");
  const t = await startThread(c, "/fake stall");
  await c.close();
  await sleep(500);
  assert.equal(reason, "", "a running turn keeps it up");
  const d = await client(daemon);
  await d.request("turn/interrupt", { threadId: t.threadId, turnId: t.turnId });
  await d.close();
  await until(() => reason, "idle exit");
  assert.equal(reason, "idle");
  // A hand-over at idle happens even with a client attached.
  let r2 = "";
  const d2 = await startDaemon(home, { onExit: (r) => (r2 = r) });
  const c2 = await client(d2);
  assert.equal(((await c2.request("daemon/handover", { when: "idle" })) as any).state, "scheduled");
  await c2.waitClosed();
  await until(() => r2, "hand-over exit");
  assert.equal(r2, "handover idle");
});

test("a starting daemon waits for the previous one to finish exiting before it binds", async () => {
  const home = tmpHome();
  // The previous daemon: stopped listening (no socket), still finishing for a moment.
  const { spawn } = await import("node:child_process");
  const previous = spawn(process.execPath, ["-e", "setTimeout(() => {}, 700)"], { stdio: "ignore" });
  const exited = new Promise<void>((r) => previous.once("exit", () => r()));
  let gone = false;
  void exited.then(() => (gone = true));
  fs.mkdirSync(path.join(home, "run"), { recursive: true, mode: 0o700 });
  fs.writeFileSync(path.join(home, "run", "yaacd.pid"), `${previous.pid}\n`);
  const started = Date.now();
  const daemon = await startDaemon(home);
  assert.ok(gone, `it started only after the previous daemon exited (after ${Date.now() - started} ms)`);
  assert.equal(fs.readFileSync(path.join(home, "run", "yaacd.pid"), "utf8").trim(), String(process.pid));
  await daemon.stop("test");
});

test("handover now interrupts running turns, marked as interrupted by an update", async () => {
  const home = tmpHome();
  const daemon = await startDaemon(home);
  const c = await client(daemon);
  const t = await startThread(c, "/fake stall");
  await c.request("thread/subscribe", { threadId: t.threadId, epoch: 1, afterSeq: 0 });
  await c.request("daemon/handover", { when: "now" });
  await c.waitClosed();
  const d2 = await startDaemon(home);
  const c2 = await client(d2);
  await c2.request("thread/subscribe", { threadId: t.threadId, epoch: 1, afterSeq: 0 });
  const done = await waitTurnDone(c2, t.threadId, t.turnId);
  assert.equal((done.event as any).status, "interruptedByUpdate");
  const items = (await readAll(c2, t.threadId)).flatMap((p) => p.items);
  assert.ok(items.some((i) => i.kind === "notice" && (i as any).noticeKind === "interruptedByUpdate"));
  await c2.close();
  await d2.stop("test");
});
