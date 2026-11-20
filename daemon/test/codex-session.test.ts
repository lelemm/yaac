// The Codex provider under the real session layer: a yaacd with the Codex factory, an
// instance pointed at a fake app-server replaying the recorded turn, and a protocol client.

import { test } from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as path from "node:path";
import { codexProviderFactory } from "../src/providers/codex/index.js";
import { client, opId, readAll, startDaemon, text, tmpHome, until } from "./helpers.js";
import { FakeCodex, fixture, type Step } from "./fixtures/codex/fake-codex.js";

test("codex + session layer: a recorded turn with an approval, journaled and answered over the protocol", async () => {
  const home = tmpHome("yaac-cxs-");
  const fake = await FakeCodex.start(path.join(home, "codex.sock"));
  fake.handlers.set("account/read", () => ({ account: { type: "apiKey" }, requiresOpenaiAuth: true }));
  fake.handlers.set("model/list", () => ({ data: [], nextCursor: null }));
  fake.handlers.set("thread/list", () => ({ data: [], nextCursor: null }));
  fake.handlers.set("thread/read", (p) => ({ thread: { id: p.threadId, status: { type: "idle" }, cwd: home } }));
  // No implicit instance: this test must never reach the real Codex.
  const daemon = await startDaemon(home, { factories: [codexProviderFactory], env: { YAAC_FAKE_PROVIDER: "0", YAAC_CODEX_PROVIDER: "0" } });
  try {
    const c = await client(daemon);
    const steps = fixture<{ steps: Step[] }>("turn-approval.json").steps;
    // The probe's own initialize is answered by the default handler; the script's first
    // `initialize` step is dropped — the connection is already up when the turn starts.
    const script = fake.run(steps.filter((s) => !("expect" in s && s.expect === "initialize") && !("expectNotify" in s)));
    const addedRes: any = await c.request("providerInstance/add", { opId: opId(), kind: "codex", endpoint: { socket: fake.socket } });
    const added = addedRes.instance;
    const list: any = await c.request("provider/list", {});
    const desc = list.providers.find((p: any) => p.instance === added.id);
    assert.equal(desc.status, "ready");
    assert.equal(desc.backend, "daemon");

    const work = fs.mkdtempSync(path.join(home, "w-"));
    const started: any = await c.request("thread/start", { opId: opId(), instance: added.id, cwd: work, input: text("Run the shell command `echo yaac-fixture` and then reply with just the word done.") });
    assert.equal(started.turnId, "01a1fa3e-0086-76e0-b64c-b654bc53bcb8");
    await c.request("thread/subscribe", { threadId: started.threadId, epoch: 1, afterSeq: 0 });
    const req = await c.waitEvent((e) => e.event.type === "itemStarted" && (e.event as any).item.kind === "request", "the approval");
    const item = (req.event as any).item;
    assert.equal(item.requestKind, "approval");
    assert.deepEqual(item.decisions.map((d: any) => d.id), ["accept", "acceptWithExecpolicyAmendment", "cancel"]);
    await c.request("request/respond", { opId: opId(), threadId: started.threadId, requestId: item.requestId, response: { requestKind: "approval", decision: "accept" } });
    await script;
    await c.waitEvent((e) => e.threadId === started.threadId && e.event.type === "turnCompleted", "the turn to complete");
    const states = c.events(started.threadId).filter((e) => e.event.type === "requestChanged").map((e) => (e.event as any).state);
    await until(() => c.events(started.threadId).some((e) => e.event.type === "requestChanged" && (e.event as any).state === "resolved"), "the approval to resolve");
    assert.ok(states.includes("submitted") || states.includes("resolved"));
    assert.deepEqual(fake.replies.get(0), { decision: "accept" });

    // thread/read pages are newest first (docs/03-protocol.md); put them in conversation order.
    const items = (await readAll(c, started.threadId)).flatMap((p) => p.items).reverse();
    const kinds = items.map((i) => i.kind);
    // yaacd's own record of the user's message, once; the joined input from another client; the rest from Codex.
    assert.equal(items.filter((i) => i.kind === "userMessage").length, 2);
    for (const k of ["reasoning", "agentMessage", "toolCall", "request"]) assert.ok(kinds.includes(k as any), `has ${k}`);
    const cmd = items.find((i) => i.kind === "toolCall") as any;
    assert.equal(cmd.status, "completed");
    assert.equal(cmd.exitCode, 0);
    const agent = items.filter((i) => i.kind === "agentMessage").map((i: any) => i.text);
    assert.deepEqual(agent, ["Running the command.", "done"]);
    await c.close();
  } finally {
    await daemon.stop("test");
    await fake.close();
    fs.rmSync(home, { recursive: true, force: true });
  }
});
