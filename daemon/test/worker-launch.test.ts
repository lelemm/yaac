import { test } from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as path from "node:path";
import type { ThreadLaunchEvent, WorkerLaunchEvent } from "../src/session/worker-launch.js";
import { writeJsonAtomic } from "../src/util/fsx.js";
import { client, FAKE, fakeOf, opId, startDaemon, startThread, text, tmpHome, until } from "./helpers.js";
import { childOf, startAllowed, untilIdle } from "./mesh-helpers.js";

const OTHER = "pi_worker";

for (const accept of [true, false]) {
  test(`local worker launch hook ${accept ? "receives both identities" : "can withhold the first prompt"}`, async () => {
    const home = tmpHome("yaac-worker-hook-");
    writeJsonAtomic(path.join(home, "instances.json"), {
      instances: [
        { id: FAKE, kind: "fake", enabled: true, backend: "auto", implicit: true },
        { id: OTHER, kind: "fake", enabled: true, backend: "auto", env: { FAKE_SEED: "0" } },
      ],
    });
    const receipt = path.join(home, "hook-event.json");
    writeJsonAtomic(path.join(home, "worker-launch-hook.json"), {
      command: process.execPath,
      args: ["-e", `
        const fs = require("node:fs");
        const event = JSON.parse(fs.readFileSync(0, "utf8"));
        if (process.env.YAAC_PRIVATE_FIXTURE) process.exit(99);
        fs.writeFileSync(process.argv[1], JSON.stringify(event));
        if (process.argv[2] === "false" && event.event === "worker.beforeFirstPrompt") {
          process.stderr.write("Private diagnostic");
          process.stdout.write(JSON.stringify({ reason: "The configured policy refused this worker." }));
          process.exitCode = 2;
        }
      `, receipt, String(accept)],
    });
    const daemon = await startDaemon(home, { env: {
      YAAC_FAKE_PROVIDER: "1", YAAC_CODEX_PROVIDER: "0", YAAC_CLAUDE_PROVIDER: "0",
      YAAC_PRIVATE_FIXTURE: "synthetic-canary",
    } });
    const conn = await client(daemon);
    try {
      const { threadId } = await startAllowed(conn, "/fake say ready", { cwd: home });
      await untilIdle(daemon, threadId);
      const parent = daemon.core.thread(threadId)!;
      assert.deepEqual(JSON.parse(fs.readFileSync(receipt, "utf8")), {
        event: "thread.beforeFirstPrompt",
        thread: { harness: "fake", instanceId: FAKE, yaacThreadId: threadId, nativeThreadId: parent.record.native },
      });
      const spawn = () => fakeOf(daemon).meshAttachment(parent.record.native)!.call({
        tool: "spawn_thread",
        args: { instance: OTHER, text: "/fake say working" },
        meta: { threadId: parent.record.native },
      });
      const result = await spawn();
      const event = JSON.parse(fs.readFileSync(receipt, "utf8")) as WorkerLaunchEvent;
      assert.deepEqual(event.parent, { harness: "fake", instanceId: FAKE, yaacThreadId: threadId, nativeThreadId: parent.record.native });
      assert.equal(event.event, "worker.beforeFirstPrompt");
      assert.equal(result.isError, !accept);
      if (accept) {
        const childId = await childOf(daemon, threadId);
        const child = daemon.core.thread(childId)!;
        assert.deepEqual(event.child, { harness: "fake", instanceId: OTHER, yaacThreadId: childId, nativeThreadId: child.record.native });
      } else {
        assert.match(result.text, /The configured policy refused this worker/);
        assert.doesNotMatch(result.text, /Private diagnostic/);
        assert.equal(fakeOf(daemon, OTHER).received.length, 0);
        assert.equal(daemon.mesh.store.peekThread(threadId)?.children.length, 0);

        writeJsonAtomic(path.join(home, "worker-launch-hook.json"), {
          command: process.execPath,
          args: ["-e", 'process.stdout.write("x".repeat(32768));'],
        });
        const overflow = await spawn();
        assert.equal(overflow.isError, true);
        assert.match(overflow.text, /exceeded its output limit/);
        assert.equal(fakeOf(daemon, OTHER).received.length, 0);
        assert.equal(daemon.mesh.store.peekThread(threadId)?.children.length, 0);
      }
    } finally {
      await conn.close();
      await daemon.stop("test");
      fs.rmSync(home, { recursive: true, force: true });
    }
  });
}

test("the thread launch hook can withhold the first UI prompt and return a public reason", async (t) => {
  const home = tmpHome("yaac-thread-hook-");
  writeJsonAtomic(path.join(home, "worker-launch-hook.json"), {
    command: process.execPath,
    args: ["-e", `
      const event = JSON.parse(require("node:fs").readFileSync(0, "utf8"));
      if (event.event !== "thread.beforeFirstPrompt") process.exit(91);
      process.stderr.write("private diagnostic");
      process.stdout.write(JSON.stringify({ reason: "The configured policy refused this thread." }));
      process.exitCode = 2;
    `],
  });
  const daemon = await startDaemon(home);
  t.after(async () => {
    await daemon.stop("test");
    fs.rmSync(home, { recursive: true, force: true });
  });
  const conn = await client(daemon);
  await assert.rejects(startThread(conn, "/fake say don't send this", { cwd: home }), (error: Error) => {
    assert.match(error.message, /The configured policy refused this thread/);
    assert.doesNotMatch(error.message, /private diagnostic/);
    return true;
  });
  assert.deepEqual(fakeOf(daemon).received, []);
});

test("native forks and handoffs run the thread launch hook once, including an idle fork", async (t) => {
  const home = tmpHome("yaac-fork-hook-");
  writeJsonAtomic(path.join(home, "instances.json"), {
    instances: [{ id: OTHER, kind: "fake", enabled: true, backend: "auto", env: { FAKE_SEED: "0" } }],
  });
  const receipt = path.join(home, "hook-events.jsonl");
  const release = path.join(home, "release-hook");
  writeJsonAtomic(path.join(home, "worker-launch-hook.json"), {
    command: process.execPath,
    args: ["-e", `
      const fs = require("node:fs");
      const event = fs.readFileSync(0, "utf8");
      fs.appendFileSync(process.argv[1], event);
      const wait = setInterval(() => { if (fs.existsSync(process.argv[2])) clearInterval(wait); }, 5);
    `, receipt, release],
  });
  const daemon = await startDaemon(home);
  t.after(async () => {
    fs.writeFileSync(release, "");
    await daemon.stop("test");
    fs.rmSync(home, { recursive: true, force: true });
  });
  const conn = await client(daemon);
  const events = (): ThreadLaunchEvent[] => fs.existsSync(receipt)
    ? fs.readFileSync(receipt, "utf8").split("\n").slice(0, -1).map((line) => JSON.parse(line))
    : [];
  const starting = startThread(conn, "/fake say ready", { cwd: home });
  await until(() => events().length === 1, "the thread launch hook to start");
  assert.deepEqual(fakeOf(daemon).received, [], "the first prompt waits for the hook");
  fs.writeFileSync(release, "");
  const source = await starting;
  await untilIdle(daemon, source.threadId);

  const fork = await conn.request("thread/fork", {
    opId: opId(), source: source.threadId, instance: FAKE, input: text("/fake say native fork"),
  });
  assert.equal(fork.forkKind, "native");
  const idle = await conn.request("thread/forkOnly", {
    opId: opId(), source: source.threadId, instance: FAKE,
  });
  assert.equal(idle.forkKind, "native");
  const handoff = await conn.request("thread/fork", {
    opId: opId(), source: source.threadId, instance: OTHER, input: text("/fake say handoff"),
  });
  assert.equal(handoff.forkKind, "handoff");
  for (const [index, id] of [source.threadId, fork.threadId, idle.threadId, handoff.threadId].entries()) {
    const { record } = daemon.core.thread(id)!;
    assert.deepEqual(events()[index], {
      event: "thread.beforeFirstPrompt",
      thread: { harness: "fake", instanceId: record.instance, yaacThreadId: id, nativeThreadId: record.native },
    });
  }
  await conn.request("turn/start", { opId: opId(), threadId: idle.threadId, input: text("/fake say first prompt after idle fork") });
  assert.equal(events().length, 4, "the first turn of an idle fork does not rerun the launch hook");
});
