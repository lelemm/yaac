import assert from "node:assert/strict";
import * as fs from "node:fs";
import { test } from "node:test";
import type { ProviderInstanceListResult, ProviderListResult, ThreadSyncResult } from "../src/protocol/generated/types.js";
import { client, FAKE, fakeOf, opId, startDaemon, startThread, text, tmpHome } from "./helpers.js";
import { childOf, untilIdle } from "./mesh-helpers.js";

test("model switches filter provider and agent lists, refuse spawns, and select an enabled default", async () => {
  const home = tmpHome("model-availability-");
  const d = await startDaemon(home);
  const c = await client(d);
  try {
    const parent = await startThread(c, "/fake say ready", { agents: "allow" });
    await untilIdle(d, parent.threadId);
    const runtime = d.core.thread(parent.threadId)!;
    const native = runtime.record.native;
    const attachment = fakeOf(d).meshAttachment(native)!;
    const call = (tool: string, args: Record<string, unknown> = {}) =>
      attachment.call({ tool, args, meta: { threadId: native } });
    const set = (model: string, enabled: boolean) =>
      c.request("providerInstance/setModelEnabled", { opId: opId(), instance: FAKE, model, enabled });
    const descriptor = async () =>
      (await c.request<ProviderListResult>("provider/list")).providers.find((p) => p.instance === FAKE)!;
    const summary = async () =>
      (await c.request<ThreadSyncResult>("thread/sync")).changed?.find((s) => s.id === parent.threadId)!;

    await set("fake-large", false);
    const listed = await descriptor();
    assert.deepEqual(listed.disabledModels?.map((m) => m.id), ["fake-large"]);
    assert.deepEqual(listed.models?.map((m) => m.id), ["fake-small", "fake-mystery"]);
    assert.equal(listed.models?.find((m) => m.isDefault)?.id, "fake-small");
    assert.equal((await summary()).model, "fake-large");
    assert.equal((await summary()).modelOff, true);
    assert.ok(c.notifications.some((n) => n.method === "provider/updated" && n.params.provider.disabledModels?.[0]?.id === "fake-large"));

    const models = await call("list_models");
    assert.doesNotMatch(models.text, /fake-large/);
    assert.match(models.text, /fake-small .*\(default\)/);
    for (const model of ["fake-large", "Fake Large"]) {
      const refused = await call("spawn_thread", { model, text: "/fake say wrong" });
      assert.equal(refused.isError, true);
      assert.match(refused.text, /fake-large is turned off for Fake on /);
    }
    assert.equal(d.mesh.store.peekThread(parent.threadId)?.children.length ?? 0, 0);
    await assert.rejects(startThread(c, "wrong model", { model: "fake-large" }), /fake-large is turned off/);

    const spawned = await call("spawn_thread", { text: "/fake say worker", detached: true });
    assert.equal(spawned.isError, false, spawned.text);
    const worker = d.core.thread(await childOf(d, parent.threadId))!;
    assert.equal((await d.core.computeEffective(worker)).applied?.model, "fake-small");
    const draft = await startThread(c, "/fake say new default");
    assert.equal(d.core.summaryOf(d.core.thread(draft.threadId)!).model, "fake-small");
    const forked = await c.request("thread/forkOnly", { opId: opId(), source: parent.threadId, instance: FAKE });
    assert.equal((await d.core.computeEffective(d.core.thread(forked.threadId)!)).applied?.model, "fake-small");

    await c.request("turn/start", { opId: opId(), threadId: parent.threadId, input: text("/fake say continuing") });
    await untilIdle(d, parent.threadId);
    assert.equal((await d.core.computeEffective(runtime)).applied?.model, "fake-large");
    assert.equal((await summary()).modelOff, true);

    await set("fake-small", false);
    await set("fake-mystery", false);
    await assert.rejects(startThread(c, "all off"), /No enabled models/);
    await set("fake-large", true);
    assert.equal((await descriptor()).models?.find((m) => m.isDefault)?.id, "fake-large");
    assert.equal((await summary()).modelOff, undefined);
    assert.match((await call("list_models")).text, /fake-large .*\(default\)/);
  } finally {
    await c.close();
    await d.stop("test");
    fs.rmSync(home, { recursive: true, force: true });
  }
});

test("off IDs survive restart, apply only to their instance, and leave newly discovered IDs on", async () => {
  const home = tmpHome("model-availability-");
  let daemon = await startDaemon(home);
  let host = await client(daemon);
  try {
    const { instance: extra } = await host.request("providerInstance/add", { opId: opId(), kind: "fake", name: "other" });
    await host.request("providerInstance/setModelEnabled", { opId: opId(), instance: FAKE, model: "fake-large", enabled: false });
    await host.close();
    await daemon.stop("test restart");
    daemon = await startDaemon(home);
    host = await client(daemon);

    const provider = fakeOf(daemon);
    const probe = provider.probe.bind(provider);
    provider.probe = async () => {
      const descriptor = await probe();
      return { ...descriptor, models: [...descriptor.models, { id: "new-id", displayName: "New model" }] };
    };
    await host.request("provider/refresh", { instance: FAKE });
    const { providers } = await host.request<ProviderListResult>("provider/list");
    const main = providers.find((p) => p.instance === FAKE)!;
    const other = providers.find((p) => p.instance === extra.id)!;
    assert.deepEqual(main.disabledModels?.map((m) => m.id), ["fake-large"]);
    assert.ok(main.models?.some((m) => m.id === "new-id"));
    assert.equal(other.disabledModels, undefined);
    assert.equal(other.models?.find((m) => m.isDefault)?.id, "fake-large");
    const { instances } = await host.request<ProviderInstanceListResult>("providerInstance/list");
    assert.deepEqual(instances.find((i) => i.id === FAKE)?.disabledModelIds, ["fake-large"]);
    assert.equal(instances.find((i) => i.id === extra.id)?.disabledModelIds, undefined);
  } finally {
    await host.close();
    await daemon.stop("test");
    fs.rmSync(home, { recursive: true, force: true });
  }
});
