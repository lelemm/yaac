// Fork (docs/05-threads.md#fork): native within one store, handoff everywhere else — the
// transcript renderer, the stored file, the preamble, the cut-off, and restarts.

import { test } from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { createHash } from "node:crypto";
import { FAKE, RpcFailure, client, fakeOf, opId, readAll, startDaemon, startThread, text, tmpHome, until, waitTurnDone } from "./helpers.js";
import type { Item } from "../src/protocol/generated/types.js";
import {
  HANDOFF_ITEM_BYTES,
  TOOL_ERROR_CHARS,
  boundTranscript,
  cutoffTurn,
  itemsThrough,
  preamble,
  previewOf,
  renderTranscript,
  type AttachmentLookup,
} from "../src/session/handoff.js";

const OTHER = "pi_fake2";

/** A home with a second fake instance that keeps its own store (so: a handoff destination). */
function homeWithTwoStores(extraEnv: Record<string, string> = {}): string {
  const home = tmpHome();
  fs.writeFileSync(
    path.join(home, "instances.json"),
    JSON.stringify({ instances: [{ id: OTHER, kind: "fake", name: "other", enabled: true, backend: "auto", env: { FAKE_SEED: "0", ...extraEnv } }] }),
  );
  return home;
}

/** A home with a second fake instance on the same store as `pi_fake` (so: native). */
function homeWithSharedStore(): string {
  const home = tmpHome();
  const store = path.join(home, "shared-store");
  fs.writeFileSync(
    path.join(home, "instances.json"),
    JSON.stringify({
      instances: [
        { id: FAKE, kind: "fake", enabled: true, backend: "auto", implicit: true, env: { FAKE_STORE: store } },
        { id: OTHER, kind: "fake", name: "other", enabled: true, backend: "auto", env: { FAKE_STORE: store, FAKE_SEED: "0" } },
      ],
    }),
  );
  return home;
}

const items = (pages: Array<{ items: Item[] }>) => pages.flatMap((p) => p.items).reverse() as Array<Item & Record<string, any>>;

// ── The renderer ───────────────────────────────────────────────────────────────────────────

const none: AttachmentLookup = { upload: () => null, exists: () => false };
const src = { title: "Fix the decoder", provider: "Codex", target: "devbox", threadId: "th_src" };
const it = (turnId: string, id: string, body: Record<string, unknown>, status = "completed"): Item => ({ id, turnId, status, ...body }) as unknown as Item;

test("renderer: messages in full, one line per tool call with its outcome, no reasoning", () => {
  const long = "x".repeat(5000);
  const r = renderTranscript(
    [
      it("t1", "u1", { kind: "userMessage", parts: [{ type: "text", text: "Why does the decoder fail?" }] }),
      it("t1", "r1", { kind: "reasoning", text: "secret thoughts" }),
      it("t1", "c1", { kind: "toolCall", tool: "command", title: "cargo test", exitCode: 0, output: long }),
      it("t1", "c2", { kind: "toolCall", tool: "command", title: "cargo build\n--release", exitCode: 101, output: `error[E0308]: mismatched types\n${long}` }, "failed"),
      it("t1", "c3", { kind: "toolCall", tool: "fileChange", title: "Edited src/frame.rs", output: "@@ diff @@" }),
      it("t1", "c4", { kind: "toolCall", tool: "command", title: "rm -rf /" }, "declined"),
      it("t1", "a1", { kind: "agentMessage", text: "The length prefix is read as little-endian." }),
      it("t2", "u2", { kind: "userMessage", parts: [{ type: "text", text: "Fix it." }] }),
      it("t2", "a2", { kind: "agentMessage", text: "Done." }),
    ],
    src,
    none,
  );
  const md = r.markdown;
  assert.equal(r.turns, 2);
  assert.match(md, /^# Prior conversation/);
  assert.match(md, /Forked from "Fix the decoder" \(Codex on devbox, thread th_src\), through turn 2\./);
  assert.match(md, /## Turn 1\n\n\*\*User:\*\*\n\nWhy does the decoder fail\?/);
  assert.ok(md.includes("- Tool (command): `cargo test` — succeeded (exit 0)\n- Tool (command): `cargo build --release` — failed (exit 101): error[E0308]: mismatched types"), md);
  assert.ok(md.includes("- Tool (fileChange): `Edited src/frame.rs` — succeeded"));
  assert.ok(md.includes("- Tool (command): `rm -rf /` — declined"));
  assert.ok(md.includes("**Agent:**\n\nThe length prefix is read as little-endian."));
  assert.ok(md.includes("## Turn 2"));
  assert.ok(!md.includes("secret thoughts"), "no reasoning");
  assert.ok(!md.includes("xxxxxxxxxx".repeat(TOOL_ERROR_CHARS / 10 + 1)), "long output is trimmed to one short line");
  assert.ok(!md.includes("@@ diff @@"), "no tool output for a success");
  assert.ok(md.length < 2000, `bulky output left out (${md.length} chars)`);
});

test("renderer: attachments still on the target are referenced, gone ones are marked omitted", () => {
  const look: AttachmentLookup = {
    upload: (id) => (id === "u_here" ? { path: "/tmp/yaac-uploads/t/1-plot.png", name: "plot.png" } : null),
    exists: (p) => p === "/work/still.png",
  };
  const r = renderTranscript(
    [
      it("t1", "u1", { kind: "userMessage", parts: [{ type: "text", text: "Look" }, { type: "image", uploadId: "u_here", name: "plot.png" }, { type: "file", uploadId: "u_gone", name: "log.txt" }] }),
      // Codex keeps a local image as a marker in the text.
      it("t2", "u2", { kind: "userMessage", parts: [{ type: "text", text: "And [image: /work/still.png] and [image: /tmp/yaac-uploads/old/2-shot.png]" }] }),
      // Claude keeps image bytes inline: what yaacd journaled for the turn names the file.
      it("t3", "u3", { kind: "userMessage", parts: [{ type: "text", text: "Third [image]" }] }),
    ],
    src,
    look,
    [it("t3", "j3", { kind: "userMessage", parts: [{ type: "text", text: "Third" }, { type: "image", uploadId: "u_gone2", name: "screen.png" }] })],
  );
  assert.ok(r.markdown.includes("[image: /tmp/yaac-uploads/t/1-plot.png]"));
  assert.ok(r.markdown.includes("[file omitted: log.txt — no longer on the target]"));
  assert.ok(r.markdown.includes("[image: /work/still.png]"));
  assert.ok(r.markdown.includes("[image omitted: 2-shot.png — no longer on the target]"));
  assert.ok(r.markdown.includes("[image omitted: screen.png — no longer on the target]"));
  assert.deepEqual(r.omitted, ["log.txt", "2-shot.png", "screen.png"]);
});

test("renderer: agent-mesh messages keep their sender; cut-off skips a running turn", () => {
  const all = [
    it("t1", "u1", { kind: "userMessage", parts: [{ type: "text", text: "go" }] }),
    it("t1", "m1", { kind: "meshReceived", from: { address: "local/th_w" }, message: { id: "m", from: "local/th_w", kind: "result", seq: 1, sentAt: "", text: "worker result" } }),
    it("t2", "u2", { kind: "userMessage", parts: [{ type: "text", text: "next" }] }),
    it("t2", "a2", { kind: "agentMessage", text: "half" }, "inProgress"),
  ];
  // t2 has an item in progress: it is running (here or elsewhere).
  assert.equal(cutoffTurn(all, []), "t1");
  assert.equal(cutoffTurn(all.slice(0, 3), ["t2"]), "t1");
  assert.equal(cutoffTurn(all.slice(0, 3), []), "t2");
  assert.equal(cutoffTurn(all, [], "t2"), "t2");
  assert.equal(cutoffTurn(all, [], "nope"), null);
  assert.equal(cutoffTurn(all.slice(2), ["t2"]), null);
  const md = renderTranscript(itemsThrough(all, "t1"), src, none).markdown;
  assert.ok(md.includes("**Message from agent local/th_w:**\n\nworker result"));
  assert.ok(!md.includes("next"));
});

test("preamble: points at the file and the source; mentions the yaac tools only when offered", () => {
  const withMesh = preamble("/home/u/.yaac/handoffs/ab/transcript.md", { target: "devbox", threadId: "th_src", address: "tg_1/th_src" }, true);
  const without = preamble("/home/u/.yaac/handoffs/ab/transcript.md", { target: "devbox", threadId: "th_src", address: "tg_1/th_src" }, false);
  for (const p of [withMesh, without]) {
    assert.ok(p.includes("/home/u/.yaac/handoffs/ab/transcript.md"));
    assert.ok(p.includes("The earlier thread is th_src on devbox."));
    assert.ok(p.endsWith("---\n\n"));
  }
  assert.ok(withMesh.includes("read_thread") && withMesh.includes("tg_1/th_src"));
  assert.ok(!without.includes("yaac tools") && !without.includes("read_thread"));
});

test("bounds: the item keeps the transcript's end at a turn heading; the preview its start", () => {
  const turns = Array.from({ length: 400 }, (_, i) => `## Turn ${i + 1}\n\n**User:**\n\n${"word ".repeat(40)}`).join("\n\n");
  const bounded = boundTranscript(turns, HANDOFF_ITEM_BYTES, "/f.md");
  assert.ok(Buffer.byteLength(bounded) <= HANDOFF_ITEM_BYTES);
  assert.ok(bounded.startsWith("*The earlier part of this transcript is in /f.md.*\n\n## Turn "));
  assert.ok(bounded.includes("## Turn 400"));
  assert.equal(boundTranscript("short", HANDOFF_ITEM_BYTES, "/f.md"), "short");
  const pv = previewOf("é".repeat(3000), 1001);
  assert.ok(!pv.includes("�") && pv.endsWith("\n…"));
});

// ── Through the daemon, with the fake provider ─────────────────────────────────────────────

test("native fork within one store: preview, fork with a first message and another model", async () => {
  const home = homeWithSharedStore();
  const daemon = await startDaemon(home);
  const c = await client(daemon);
  const t = await startThread(c, "/fake say first answer");
  await c.request("thread/subscribe", { threadId: t.threadId, epoch: 1, afterSeq: 0 });
  await waitTurnDone(c, t.threadId, t.turnId);
  const pv: any = await c.request("thread/forkPreview", { source: t.threadId, instance: OTHER });
  assert.equal(pv.forkKind, "native");
  assert.equal(pv.cutoffTurn, t.turnId);
  const f: any = await c.request("thread/fork", { opId: opId(), source: t.threadId, instance: OTHER, model: "fake-small", input: text("/fake say continued") });
  assert.equal(f.forkKind, "native");
  await c.request("thread/subscribe", { threadId: f.threadId, epoch: 1, afterSeq: 0 });
  await waitTurnDone(c, f.threadId, f.turnId);
  const got = items(await readAll(c, f.threadId));
  const h = got.find((i) => i.kind === "handoff")!;
  assert.equal(h.forkKind, "native");
  assert.equal(h.transcript, undefined, "a native fork carries no transcript");
  const sum: any = (await c.request("thread/get", { threadId: f.threadId }).catch(() => null)) ?? null;
  if (sum) assert.equal(sum.thread.forkedFrom, t.threadId);
  await c.close();
  await daemon.stop("test");
});

test("handoff fork: transcript file, preamble on the first turn, handoff item, forkedFrom; forkOnly refuses", async () => {
  const home = homeWithTwoStores();
  const daemon = await startDaemon(home);
  const c = await client(daemon);
  const t = await startThread(c, "Please look at the decoder\n/fake tool cargo test => 3 passed\n/fake tool-fail cargo clippy => error: unused variable\n/fake reason private thoughts\n/fake say The decoder is fine.");
  await c.request("thread/subscribe", { threadId: t.threadId, epoch: 1, afterSeq: 0 });
  await waitTurnDone(c, t.threadId, t.turnId);

  const pv: any = await c.request("thread/forkPreview", { source: t.threadId, instance: OTHER, maxPreviewBytes: 300 });
  assert.equal(pv.forkKind, "handoff");
  assert.equal(pv.cutoffTurn, t.turnId);
  assert.equal(pv.destinationAccount, "fake-user");
  assert.ok(pv.transcriptBytes > 300);
  assert.ok(pv.transcriptPreview.startsWith("# Prior conversation"));
  assert.ok(Buffer.byteLength(pv.transcriptPreview) <= 300 + 4);
  assert.deepEqual(pv.omittedAttachments, []);
  assert.equal(pv.unavailableReason, undefined);
  assert.equal(fs.existsSync(path.join(home, "handoffs")), false, "a preview writes nothing");

  await assert.rejects(c.request("thread/forkOnly", { opId: opId(), source: t.threadId, instance: OTHER }), (e: RpcFailure) => e.code === "capability_missing");

  const other = fakeOf(daemon, OTHER);
  const op = opId();
  const f: any = await c.request("thread/fork", { opId: op, source: t.threadId, instance: OTHER, input: text("/fake say Carrying on.") });
  assert.equal(f.forkKind, "handoff");
  await c.request("thread/subscribe", { threadId: f.threadId, epoch: 1, afterSeq: 0 });
  await waitTurnDone(c, f.threadId, f.turnId);

  // The file, in ~/.yaac/handoffs/, private.
  const file = path.join(home, "handoffs", createHash("sha256").update(`fork:${op}`).digest("hex").slice(0, 24), "transcript.md");
  const md = fs.readFileSync(file, "utf8");
  assert.equal(fs.statSync(file).mode & 0o777, 0o600);
  assert.ok(md.includes("Please look at the decoder"));
  assert.ok(md.includes("- Tool (command): `cargo test` — succeeded (exit 0)"));
  assert.ok(md.includes("- Tool (command): `cargo clippy` — failed (exit 1): error: unused variable"));
  assert.ok(md.includes("The decoder is fine."));
  assert.equal(md.split("private thoughts").length, 2, "only the directive in the user's message, no reasoning item");
  assert.equal(md.split("3 passed").length, 2, "a success's output is left out (only the directive shows it)");

  // The first message: the preamble, then the user's message; the folder is readable.
  const handed = other.received.find((r) => r.thread === other.nativeIds().at(-1))!;
  assert.ok(handed.input.text.startsWith("[YAAC handoff]"));
  assert.ok(handed.input.text.includes(file));
  assert.ok(handed.input.text.includes(`The earlier thread is ${t.threadId} on`));
  assert.ok(handed.input.text.includes("read_thread"), "the destination is offered the yaac tools");
  assert.ok(handed.input.text.endsWith("---\n\n/fake say Carrying on."));
  assert.deepEqual(handed.input.readable, [path.dirname(file)]);

  // The new thread: the handoff item first, then the user's own message (no preamble).
  const got = items(await readAll(c, f.threadId));
  assert.equal(got[0]!.kind, "handoff");
  assert.equal(got[0]!.forkKind, "handoff");
  assert.equal(got[0]!.forkedFrom, t.threadId);
  assert.equal(got[0]!.cutoffTurn, t.turnId);
  assert.equal(got[0]!.transcript, md);
  assert.equal(got[0]!.transcriptPath, file);
  const user = got.find((i) => i.kind === "userMessage")!;
  assert.deepEqual(user.parts, text("/fake say Carrying on."));
  const rt = daemon.core.thread(f.threadId)!;
  assert.equal(rt.record.forkedFrom, t.threadId);
  assert.equal(daemon.core.summaryOf(rt).title, `${daemon.core.summaryOf(daemon.core.thread(t.threadId)!).title} (fork)`);

  // The same opId again: the recorded outcome, no second thread.
  const again: any = await c.request("thread/fork", { opId: op, source: t.threadId, instance: OTHER, input: text("/fake say Carrying on.") });
  assert.equal(again.threadId, f.threadId);
  assert.equal(other.nativeIds().length, 1);

  // Deleting the fork removes its transcript.
  await c.request("thread/delete", { opId: opId(), threadId: f.threadId });
  assert.equal(fs.existsSync(path.dirname(file)), false);
  await c.close();
  await daemon.stop("test");
});

test("handoff fork mid-turn cuts at the last completed turn", async () => {
  const home = homeWithTwoStores();
  const daemon = await startDaemon(home);
  const c = await client(daemon);
  const t = await startThread(c, "/fake say turn one done");
  await c.request("thread/subscribe", { threadId: t.threadId, epoch: 1, afterSeq: 0 });
  await waitTurnDone(c, t.threadId, t.turnId);
  const running: any = await c.request("turn/start", { opId: opId(), threadId: t.threadId, input: text("second question\n/fake say partial answer\n/fake stall") });
  await until(() => fakeOf(daemon).pendingRequests().length >= 0 && daemon.core.thread(t.threadId)!.activeTurn === running.turnId, "turn two running");
  const pv: any = await c.request("thread/forkPreview", { source: t.threadId, instance: OTHER });
  assert.equal(pv.cutoffTurn, t.turnId);
  assert.ok(!pv.transcriptPreview.includes("second question"));
  const f: any = await c.request("thread/fork", { opId: opId(), source: t.threadId, instance: OTHER, input: text("/fake say ok") });
  const md = fs.readFileSync(daemon.core.thread(f.threadId)!.record.handoffTranscript!, "utf8");
  assert.ok(md.includes("turn one done"));
  assert.ok(!md.includes("second question") && !md.includes("partial answer"));
  await c.request("turn/interrupt", { threadId: t.threadId, turnId: running.turnId });
  await c.close();
  await daemon.stop("test");
});

test("handoff fork: an attachment gone from the target is marked omitted; one still there is linked", async () => {
  const home = homeWithTwoStores();
  const daemon = await startDaemon(home);
  const c = await client(daemon);
  const upload = async (name: string, body: string) => {
    const data = Buffer.from(body);
    const b: any = await c.request("upload/begin", { name, size: data.length, sha256: createHash("sha256").update(data).digest("hex") });
    if (!b.exists) {
      c.sendData(b.stream, data);
      await c.request("upload/end", { uploadId: b.uploadId });
    }
    return b.uploadId as string;
  };
  const kept = await upload("kept.txt", "kept");
  const gone = await upload("gone.txt", "gone");
  const t = await startThread(c, "x", { input: [{ type: "text", text: "/fake say read" }, { type: "file", uploadId: kept }, { type: "file", uploadId: gone }] });
  await c.request("thread/subscribe", { threadId: t.threadId, epoch: 1, afterSeq: 0 });
  await waitTurnDone(c, t.threadId, t.turnId);
  // The temp folder was cleaned of one blob.
  const meta = daemon.core.uploads.meta(gone)!;
  for (const f of fs.readdirSync(path.join(home, "tmp"), { recursive: true }) as string[]) {
    const full = path.join(home, "tmp", f);
    if (fs.statSync(full).isFile() && path.basename(full).startsWith(meta.sha256.slice(0, 16))) fs.rmSync(full);
  }
  assert.equal(daemon.core.uploads.present(gone), false, "the blob is gone");
  const pv: any = await c.request("thread/forkPreview", { source: t.threadId, instance: OTHER });
  assert.deepEqual(pv.omittedAttachments, ["gone.txt"]);
  const f: any = await c.request("thread/fork", { opId: opId(), source: t.threadId, instance: OTHER, input: text("/fake say ok") });
  const md = fs.readFileSync(daemon.core.thread(f.threadId)!.record.handoffTranscript!, "utf8");
  assert.ok(md.includes("[file omitted: gone.txt — no longer on the target]"));
  const linked = /\[file: ([^\]]+kept\.txt)\]/.exec(md)?.[1];
  assert.ok(linked && fs.existsSync(linked), "the file still there is linked into the fork's folder");
  const handed = fakeOf(daemon, OTHER).received.at(-1)!;
  assert.ok(handed.input.readable!.includes(path.dirname(linked!)));
  await c.close();
  await daemon.stop("test");
});

test("handoff fork survives a restart: the transcript file and the handoff item stay", async () => {
  const home = homeWithTwoStores();
  let daemon = await startDaemon(home);
  let c = await client(daemon);
  const t = await startThread(c, "/fake say before the restart");
  await c.request("thread/subscribe", { threadId: t.threadId, epoch: 1, afterSeq: 0 });
  await waitTurnDone(c, t.threadId, t.turnId);
  const f: any = await c.request("thread/fork", { opId: opId(), source: t.threadId, instance: OTHER, input: text("/fake say after") });
  await c.request("thread/subscribe", { threadId: f.threadId, epoch: 1, afterSeq: 0 });
  await waitTurnDone(c, f.threadId, f.turnId);
  const file = daemon.core.thread(f.threadId)!.record.handoffTranscript!;
  await c.close();
  await daemon.stop("test");

  daemon = await startDaemon(home);
  c = await client(daemon);
  assert.ok(fs.existsSync(file));
  const got = items(await readAll(c, f.threadId));
  const h = got.find((i) => i.kind === "handoff")!;
  assert.ok(String(h.transcript).includes("before the restart"));
  assert.equal(h.transcriptPath, file);
  assert.equal(daemon.core.thread(f.threadId)!.record.handoffTranscript, file);
  // The fork goes on normally.
  const next: any = await c.request("turn/start", { opId: opId(), threadId: f.threadId, input: text("/fake say second") });
  await c.request("thread/subscribe", { threadId: f.threadId, epoch: 1, afterSeq: 0 });
  await waitTurnDone(c, f.threadId, next.turnId);
  assert.ok(!fakeOf(daemon, OTHER).received.at(-1)!.input.text.includes("[YAAC handoff]"), "the preamble goes only on the first message");
  await c.close();
  await daemon.stop("test");
});

test("handoff preview: unavailable without a completed turn; a missing destination is an error", async () => {
  const home = homeWithTwoStores();
  const daemon = await startDaemon(home);
  const c = await client(daemon);
  const t = await startThread(c, "/fake stall");
  await until(() => daemon.core.thread(t.threadId)!.activeTurn, "running");
  const pv: any = await c.request("thread/forkPreview", { source: t.threadId, instance: OTHER });
  assert.equal(pv.forkKind, "handoff");
  assert.match(pv.unavailableReason, /no completed turn/);
  await assert.rejects(c.request("thread/fork", { opId: opId(), source: t.threadId, instance: OTHER, input: text("x") }), (e: RpcFailure) => e.code === "invalid_params");
  await assert.rejects(c.request("thread/forkPreview", { source: t.threadId, instance: "pi_nope" }), (e: RpcFailure) => e.code === "instance_not_found");
  await c.request("turn/interrupt", { threadId: t.threadId, turnId: t.turnId });
  void os;
  await c.close();
  await daemon.stop("test");
});
