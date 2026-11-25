// The Codex provider under the real session layer: a yaacd with the Codex factory, an
// instance pointed at a fake app-server replaying the recorded turn, and a protocol client.

import { test } from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as path from "node:path";
import { codexProviderFactory } from "../src/providers/codex/index.js";
import { client, opId, readAll, startDaemon, text, tmpHome, until } from "./helpers.js";
import { FakeCodex, fixture, type Step } from "./fixtures/codex/fake-codex.js";

test("command approval is delivered immediately and updated from an ephemeral Codex turn", async () => {
  const home = tmpHome("yaac-approval-explanation-");
  const fake = await FakeCodex.start(path.join(home, "codex.sock"));
  const model = "fixture-explanation-model";
  const command = "printf 'hello\\n'";
  const threadId = "normal-thread";
  const explanationId = "ephemeral-explanation";
  fake.handlers.set("account/read", () => ({ account: { type: "apiKey" } }));
  fake.handlers.set("model/list", () => ({ data: [{ id: model, model, displayName: "Explanation fixture" }], nextCursor: null }));
  fake.handlers.set("thread/list", () => ({ data: [], nextCursor: null }));
  fake.handlers.set("thread/start", (params) => {
    const thread = { id: params.ephemeral ? explanationId : threadId, status: { type: "idle" }, cwd: home,
      ephemeral: params.ephemeral ?? false, threadSource: params.threadSource };
    fake.push({ method: "thread/started", params: { thread } });
    return { thread };
  });
  fake.handlers.set("turn/start", (params) => ({ turn: { id: `${params.threadId}-turn` } }));
  fake.handlers.set("thread/unsubscribe", () => ({ status: "unsubscribed" }));
  const daemon = await startDaemon(home, { factories: [codexProviderFactory], env: { YAAC_FAKE_PROVIDER: "0", YAAC_CODEX_PROVIDER: "0" } });
  try {
    const c = await client(daemon);
    const { instance } = await c.request("providerInstance/add", {
      opId: opId(), kind: "codex", endpoint: { socket: fake.socket },
    });
    const started = await c.request("thread/start", { opId: opId(), instance: instance.id, cwd: home, input: text("Say hello") });
    await c.request("thread/subscribe", { threadId: started.threadId, epoch: 1, afterSeq: 0 });
    fake.push({ id: 123, method: "item/commandExecution/requestApproval", params: {
      threadId, turnId: started.turnId, itemId: "command", command, cwd: home,
    } });
    const first = await c.waitEvent((entry) => entry.event.type === "itemStarted" &&
      entry.event.item.kind === "request", "approval before explanation");
    const item = (first.event as any).item;
    assert.equal(item.title, "Run a command");
    assert.equal(item.commandExplanation, undefined);
    const explaining = c.request("approval/explain", { tool: "shell", input: command, models: { main: { instance: instance.id, model } } });
    const start = await until(() => fake.requests("turn/start").find((entry) => entry.params.threadId === explanationId),
      "ephemeral turn start");
    assert.equal(JSON.parse(start.params.input[0].text).input, command);
    const explainStart = fake.requests("thread/start").find((entry) => entry.params.ephemeral)!;
    assert.equal(explainStart.params.model, model);
    assert.equal(explainStart.params.sandbox, "read-only");
    fake.push({ method: "item/completed", params: { threadId: explanationId,
      item: { id: "text", type: "agentMessage", text: "Prints hello followed by a newline." },
    } });
    fake.push({ method: "turn/completed", params: { threadId: explanationId,
      turn: { id: `${explanationId}-turn`, status: "completed" },
    } });
    assert.deepEqual(await explaining, { text: "Prints hello followed by a newline." });
    assert.equal(daemon.core.allThreads().length, 1, "the ephemeral turn never becomes a YAAC thread");
    assert.equal(fake.replies.has(123), false, "only the user answers the original request");
  } finally {
    await daemon.stop("test");
    await fake.close();
  }
});

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

test("Codex tool images are journaled as references and read from the daemon's configured upload store", async () => {
  const home = tmpHome("yaac-cxi-");
  const imagePath = path.join(home, "pixel.png");
  const data = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+XBBkAAAAASUVORK5CYII=";
  const image = Buffer.from(data, "base64");
  fs.writeFileSync(imagePath, image);
  const fake = await FakeCodex.start(path.join(home, "codex.sock"));
  const thread = { id: "image-thread", cwd: home, status: { type: "idle" } };
  const turn = { id: "image-turn", status: "inProgress" };
  fake.handlers.set("account/read", () => ({ account: { type: "apiKey" }, requiresOpenaiAuth: true }));
  fake.handlers.set("model/list", () => ({ data: [], nextCursor: null }));
  fake.handlers.set("thread/list", () => ({ data: [], nextCursor: null }));
  fake.handlers.set("thread/start", () => ({ thread }));
  fake.handlers.set("thread/read", () => ({ thread }));
  fake.handlers.set("turn/start", () => ({ turn }));
  const daemon = await startDaemon(home);
  try {
    const c = await client(daemon);
    const added = await c.request("providerInstance/add", { opId: opId(), kind: "codex", endpoint: { socket: fake.socket } });
    const { threadId } = await c.request("thread/start", { opId: opId(), instance: added.instance.id, cwd: home, input: text("Inspect images") });
    await c.request("thread/subscribe", { threadId, epoch: 1, afterSeq: 0 });
    const url = `data:image/png;base64,${data}`;
    const generatedFromPath = { type: "imageGeneration", id: "generated-path", status: "completed", savedPath: imagePath, fileId: "file-fixture" };
    const generatedInline = { type: "imageGeneration", id: "generated-inline", status: "completed", result: data };
    const items = [
      { type: "imageView", id: "view", path: imagePath },
      { type: "mcpToolCall", id: "mcp", server: "fixture", tool: "photo", status: "completed", result: { content: [
        { type: "text", text: "Two views" },
        { type: "image", mimeType: "image/png", data },
        { type: "resource", resource: { uri: "fixture://pixel", mimeType: "image/png", blob: data } },
        { type: "image", mimeType: "image/png", data: Buffer.from("plain text").toString("base64") },
      ] } },
      { type: "dynamicToolCall", id: "dynamic", tool: "photo", status: "completed", contentItems: [
        { type: "inputText", text: "Screenshot" },
        { type: "inputImage", imageUrl: url },
        { type: "inputImage", imageUrl: "https://example.invalid/image.png" },
      ] },
      { type: "functionCallOutput", id: "function", namespace: "fixture", name: "photo", output: [
        { type: "input_text", text: "Output" },
        { type: "input_image", image_url: url },
      ] },
      generatedFromPath,
      generatedInline,
    ];
    const emit = (method: string, item: (typeof items)[number]) => fake.push({ method, params: { threadId: thread.id, turnId: turn.id, item } });
    emit("item/started", items[0]!);
    for (const item of items) {
      if (item.id === generatedInline.id) emit("item/started", { ...generatedInline, status: "generating" });
      emit("item/completed", item);
    }
    const completion = await c.waitEvent((ev) => ev.event.type === "itemCompleted" && ev.event.item.id === generatedInline.id, "all image results to arrive");
    assert.ok(completion.event.type === "itemCompleted" && completion.event.item.kind === "toolCall");
    assert.equal(completion.event.item.images?.[0]?.name, "Generated image");
    assert.ok(!JSON.stringify(completion).includes(data));
    const viewStart = c.events(threadId).find((e) => e.event.type === "itemStarted" && e.event.item.id === "view")?.event;
    assert.ok(viewStart?.type === "itemStarted" && viewStart.item.kind === "toolCall");
    assert.equal(viewStart.item.images, undefined);
    const genStart = c.events(threadId).find((e) => e.event.type === "itemStarted" && e.event.item.id === generatedInline.id)?.event;
    assert.ok(genStart?.type === "itemStarted" && genStart.item.kind === "toolCall");
    assert.equal(genStart.item.status, "inProgress");
    assert.equal(genStart.item.images, undefined);
    assert.ok(!JSON.stringify(genStart).includes(data));

    const rt = daemon.core.thread(threadId)!;
    const bigImage = Buffer.concat([image, Buffer.alloc(1_520_000)]);
    const bigResult = bigImage.toString("base64");
    const legacy = rt.journal!.append({ type: "itemCompleted", item: { id: "legacy", turnId: turn.id,
      kind: "toolCall", tool: "other", title: "imageGeneration", status: "completed", input: { ...generatedInline, result: bigResult } } });
    const replay = daemon.core.renderEvent(legacy, "image-test");
    assert.ok(replay.type === "itemCompleted" && replay.item.kind === "toolCall");
    assert.equal(replay.item.title, "Generate image");
    assert.equal(replay.item.images?.length, 1);
    const bigUpload = replay.item.images![0]!.uploadId;
    await c.request("thread/subscribe", { threadId, epoch: 1, afterSeq: legacy.seq - 1 });
    const replayed = await c.waitEvent((ev) => ev.event.type === "itemCompleted" && ev.event.item.id === "legacy", "legacy image to replay");
    assert.ok(JSON.stringify(replayed).length < 1024);
    assert.deepEqual(await c.request("ping"), {});
    rt.projection = null;

    fs.unlinkSync(imagePath);
    const page = await c.request("thread/read", { threadId });
    const tools = page.items.filter((item: { kind: string }) => item.kind === "toolCall").reverse();
    assert.deepEqual(tools.map((item: { id: string }) => item.id), [...items.map((item) => item.id), "legacy"]);
    assert.deepEqual(tools.map((item: { images: unknown[] }) => item.images.length), [1, 2, 1, 1, 1, 1, 1]);
    assert.equal(tools[0].images[0].name, "pixel.png");
    assert.equal(tools[1].output, "Two views\n[image]\n[image]\n[image]");
    assert.equal(tools[2].output, "Screenshot\n[image]\n[image]");
    assert.ok(!JSON.stringify(page).includes(data));
    assert.ok(!JSON.stringify(page).includes(bigResult));
    const { uploadId } = tools[0].images[0];
    assert.ok(tools.slice(0, -1).every((item: { images: Array<{ uploadId: string }> }) => item.images.every((image) => image.uploadId === uploadId)));
    assert.deepEqual(page.uploads, [{ uploadId: bigUpload, present: true }, { uploadId, present: true }]);
    const head = await c.request("upload/read", { uploadId, maxBytes: 12 });
    const tail = await c.request("upload/read", { uploadId, offset: 12, maxBytes: 1024 });
    assert.equal(head.mediaType, "image/png");
    assert.equal(head.more, true);
    assert.equal(tail.more, false);
    assert.deepEqual(Buffer.concat([Buffer.from(head.dataBase64, "base64"), Buffer.from(tail.dataBase64, "base64")]), image);
    let offset = 0;
    const chunks: Buffer[] = [];
    while (offset < bigImage.length) {
      const part = await c.request("upload/read", { uploadId: bigUpload, offset, maxBytes: 512 * 1024 });
      const bytes = Buffer.from(part.dataBase64, "base64");
      assert.ok(bytes.length > 0);
      offset += bytes.length;
      chunks.push(bytes);
    }
    assert.ok(chunks.length > 1);
    assert.deepEqual(Buffer.concat(chunks), bigImage);
    await c.close();
  } finally {
    await daemon.stop("test");
    await fake.close();
    fs.rmSync(home, { recursive: true, force: true });
  }
});

test("codex + session layer: threads archived in Codex are listed and marked archived; archive and unarchive go to Codex and nothing leaves the index (P131)", async () => {
  const home = tmpHome("yaac-cxa-");
  const fake = await FakeCodex.start(path.join(home, "codex.sock"));
  const now = Math.floor(Date.now() / 1000);
  // Codex's own store: one active conversation, one archived in Codex before YAAC saw it.
  const store = new Map<string, { thread: any; archived: boolean }>([
    ["cx-active", { thread: { id: "cx-active", name: "Active", preview: "a", cwd: home, createdAt: now - 60, updatedAt: now - 60, source: "vscode", status: { type: "notLoaded" } }, archived: false }],
    ["cx-archived", { thread: { id: "cx-archived", name: "Archived", preview: "b", cwd: home, createdAt: now - 120, updatedAt: now - 120, source: "cli", status: { type: "notLoaded" } }, archived: true }],
  ]);
  fake.handlers.set("account/read", () => ({ account: { type: "apiKey" }, requiresOpenaiAuth: true }));
  fake.handlers.set("model/list", () => ({ data: [], nextCursor: null }));
  fake.handlers.set("thread/list", (p) => ({ data: [...store.values()].filter((e) => e.archived === (p.archived === true)).map((e) => e.thread), nextCursor: null }));
  fake.handlers.set("thread/read", (p) => ({ thread: store.get(p.threadId)!.thread }));
  fake.handlers.set("thread/archive", (p) => {
    store.get(p.threadId)!.archived = true;
    return {};
  });
  fake.handlers.set("thread/unarchive", (p) => {
    store.get(p.threadId)!.archived = false;
    return { thread: store.get(p.threadId)!.thread };
  });
  const daemon = await startDaemon(home, { factories: [codexProviderFactory], env: { YAAC_FAKE_PROVIDER: "0", YAAC_CODEX_PROVIDER: "0" } });
  try {
    const c = await client(daemon);
    const added: any = await c.request("providerInstance/add", { opId: opId(), kind: "codex", endpoint: { socket: fake.socket } });
    const instance = added.instance.id;
    const rows = async (): Promise<any[]> => {
      await c.request("provider/refresh", { instance });
      const out: any[] = [];
      let token = "";
      for (;;) {
        const r: any = await c.request("thread/sync", { token });
        out.push(...r.changed);
        token = r.token;
        if (!r.more) return out.filter((t) => t.instance === instance);
      }
    };
    const row = (all: any[], n: string) => all.find((t) => t.nativeId === n);

    let all = await rows();
    assert.equal(row(all, "cx-active")?.archived, false);
    assert.equal(row(all, "cx-archived")?.archived, true, "archived in Codex: listed, in YAAC's Archived section");
    assert.ok(fake.requests("thread/list").some((q) => q.params.archived === true), "the archived listing is asked for");

    // Archive from YAAC: Codex's own archive; a rescan keeps it, archived.
    await c.request("thread/archive", { opId: opId(), threadId: row(all, "cx-active").id });
    assert.equal(fake.requests("thread/archive").length, 1);
    all = await rows();
    assert.equal(row(all, "cx-active")?.archived, true);

    // Unarchive from YAAC: Codex's own unarchive, for both.
    for (const n of ["cx-active", "cx-archived"]) await c.request("thread/unarchive", { opId: opId(), threadId: row(all, n).id });
    assert.deepEqual(fake.requests("thread/unarchive").map((q) => q.params.threadId), ["cx-active", "cx-archived"]);
    all = await rows();
    assert.equal(row(all, "cx-active")?.archived, false);
    assert.equal(row(all, "cx-archived")?.archived, false);
    await c.close();
  } finally {
    await daemon.stop("test");
    await fake.close();
    fs.rmSync(home, { recursive: true, force: true });
  }
});

test("codex + session layer: early foreign events are journaled before a sub-agent is filed under its parent", async () => {
  const home = tmpHome("yaac-cxm-");
  const fake = await FakeCodex.start(path.join(home, "codex.sock"));
  const cwd = fs.mkdtempSync(path.join(home, "project-"));
  const createdAt = Math.floor(Date.now() / 1000) - 120;
  const parent = { id: "parent", name: "Parent", cwd, source: "vscode", createdAt, updatedAt: createdAt, status: { type: "idle" } };
  const child = {
    id: "child", name: null, cwd, createdAt, updatedAt: createdAt + 1, status: { type: "active", activeFlags: [] },
    source: { subagent: { thread_spawn: { parent_thread_id: parent.id, agent_nickname: "Kepler", agent_role: "explorer" } } },
  };
  const ordinary = { ...parent, id: "ordinary", name: "External session" };
  const legacy = { ...child, id: "legacy" };
  const oldRow = { ...parent, id: legacy.id, cwd: home, name: null };
  const listing: Record<string, unknown>[] = [parent, oldRow];
  let releaseChild!: (value: { thread: typeof child }) => void;
  const readChild = new Promise<{ thread: typeof child }>((resolve) => { releaseChild = resolve; });
  fake.handlers.set("account/read", () => ({ account: { type: "apiKey" }, requiresOpenaiAuth: true }));
  fake.handlers.set("model/list", () => ({ data: [], nextCursor: null }));
  fake.handlers.set("thread/list", (p) => ({ data: p.archived ? [] : listing, nextCursor: null }));
  fake.handlers.set("thread/read", (p) => p.threadId === child.id ? readChild : { thread: p.threadId === ordinary.id ? ordinary : parent });
  fake.handlers.set("thread/items/list", () => ({ data: [], nextCursor: null }));
  const daemon = await startDaemon(home, { factories: [codexProviderFactory], env: { YAAC_FAKE_PROVIDER: "0", YAAC_CODEX_PROVIDER: "0" } });
  try {
    const c = await client(daemon);
    const { instance } = await c.request("providerInstance/add", { opId: opId(), kind: "codex", endpoint: { socket: fake.socket } });
    await c.request("provider/refresh", { instance: instance.id });
    const before = daemon.core.threadByNative(instance.id, legacy.id)!;
    delete before.record.metadataPending; // Restore the record shape the old unknown-event path wrote.
    before.saveRecord();
    const sync = () => c.request("thread/sync", {});
    assert.equal((await sync()).changed.find((t: any) => t.nativeId === legacy.id).cwd, home);

    fake.push({ method: "thread/status/changed", params: { threadId: child.id, status: child.status } });
    fake.push({ method: "turn/started", params: { threadId: child.id, turn: { id: "foreign-turn", status: "inProgress" } } });
    fake.push({ method: "item/completed", params: { threadId: child.id, turnId: "foreign-turn", item: { type: "agentMessage", id: "early-answer", text: "Early answer" } } });
    fake.push({ id: 11, method: "item/commandExecution/requestApproval", params: { threadId: child.id, turnId: "foreign-turn", itemId: "command", availableDecisions: ["accept", "cancel"] } });
    const rt = await until(() => {
      const found = daemon.core.threadByNative(instance.id, child.id);
      return found?.pendingRequestCount() ? found : undefined;
    }, "the early approval to be recorded");
    assert.ok(fs.existsSync(path.join(rt.dir, "requests.json")));
    assert.ok(rt.journal?.entries().some((e) => e.event.type === "itemStarted" && e.event.item.kind === "request"));
    assert.equal((await sync()).changed.some((t: any) => t.nativeId === child.id), false);
    assert.equal(c.notifications.some((n) => n.method === "thread/updated" && n.params.thread.nativeId === child.id), false);
    assert.equal(fake.requests("thread/read").filter((q) => q.params.threadId === child.id).length, 1);
    assert.equal(fake.replies.has(11), false);

    releaseChild({ thread: child });
    const row = await until(async () => (await sync()).changed.find((t: any) => t.nativeId === child.id), "the child to be indexed");
    assert.equal(row.id, rt.id);
    assert.equal(row.title, "Kepler");
    assert.equal(row.cwd, cwd);
    assert.equal(row.originKind, "programmatic");
    assert.equal(row.createdAt, new Date(createdAt * 1000).toISOString());
    assert.equal(row.control, "external");
    assert.equal(row.state, "needsAttention");
    assert.equal(row.subagent.agentType, "explorer");
    assert.ok(row.spawnedBy.endsWith((await sync()).changed.find((t: any) => t.nativeId === parent.id).id));
    assert.equal((await sync()).changed.find((t: any) => t.nativeId === parent.id).children, 1);
    const items = (await readAll(c, row.id)).flatMap((p) => p.items);
    assert.ok(items.some((i) => i.kind === "agentMessage" && i.text === "Early answer"));
    const request = items.find((i) => i.kind === "request");
    assert.ok(request && request.state === "pending");
    await c.request("request/respond", { opId: opId(), threadId: row.id, requestId: request.requestId, response: { requestKind: "approval", decision: "accept" } });
    await until(() => fake.replies.has(11), "the original server request to receive the answer");
    assert.deepEqual(fake.replies.get(11), { decision: "accept" });

    fake.push({ method: "thread/status/changed", params: { threadId: ordinary.id, status: ordinary.status } });
    const external = await until(async () => (await sync()).changed.find((t: any) => t.nativeId === ordinary.id), "the ordinary external thread to be indexed");
    assert.equal(external.title, ordinary.name);
    assert.equal(external.originKind, "interactive");
    assert.equal(external.control, "free");
    assert.equal(external.subagent, undefined);

    listing[1] = legacy;
    await c.request("provider/refresh", { instance: instance.id });
    const repaired = (await sync()).changed.find((t: any) => t.nativeId === legacy.id);
    assert.equal(repaired.id, before.id);
    assert.equal(repaired.cwd, cwd);
    assert.equal(repaired.originKind, "programmatic");
    assert.equal(repaired.subagent.agentId, legacy.id);
    assert.ok(repaired.spawnedBy);
    assert.equal(fake.connections, 1);
    await c.close();
  } finally {
    await daemon.stop("test");
    await fake.close();
    fs.rmSync(home, { recursive: true, force: true });
  }
});
