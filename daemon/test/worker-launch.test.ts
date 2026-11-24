import { test } from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as path from "node:path";
import type { WorkerLaunchEvent } from "../src/session/worker-launch.js";
import { writeJsonAtomic } from "../src/util/fsx.js";
import { client, FAKE, fakeOf, startDaemon, tmpHome } from "./helpers.js";
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
        if (process.argv[2] === "false") {
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
      assert.equal(fs.existsSync(receipt), false);

      const parent = daemon.core.thread(threadId)!;
      const result = await fakeOf(daemon).meshAttachment(parent.record.native)!.call({
        tool: "spawn_thread",
        args: { instance: OTHER, text: "/fake say working" },
        meta: { threadId: parent.record.native },
      });
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
      }
    } finally {
      await conn.close();
      await daemon.stop("test");
      fs.rmSync(home, { recursive: true, force: true });
    }
  });
}
