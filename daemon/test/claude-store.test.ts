import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { test } from "node:test";
import { loadRealSdk } from "../src/providers/claude/sdk.js";

test("Claude SDK session operations use each instance's home", async (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "yaac-claude-stores-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const sessionId = randomUUID();
  const cwd = path.join(root, "project");
  const daemonHome = process.env.CLAUDE_CONFIG_DIR;

  function store(name: string) {
    const home = path.join(root, name);
    const project = path.join(home, "projects", "-project");
    fs.mkdirSync(project, { recursive: true });
    const uuid = randomUUID();
    const common = { sessionId, cwd, timestamp: new Date().toISOString(), isSidechain: false };
    const records = [
      { ...common, uuid, parentUuid: null, type: "user", message: { role: "user", content: name } },
      {
        ...common, uuid: randomUUID(), parentUuid: uuid, type: "assistant",
        message: { id: "msg_reply", role: "assistant", content: [{ type: "text", text: "Reply" }] },
      },
    ];
    fs.writeFileSync(path.join(project, `${sessionId}.jsonl`), records.map((record) => JSON.stringify(record)).join("\n") + "\n");
    return home;
  }

  const firstHome = store("First");
  const secondHome = store("Second");
  const first = await loadRealSdk(() => firstHome);
  const second = await loadRealSdk(() => secondHome);
  t.after(() => {
    first.dispose?.();
    second.dispose?.();
  });

  const [firstMessages, secondMessages] = await Promise.all([
    first.getSessionMessages(sessionId),
    second.getSessionMessages(sessionId),
  ]);
  assert.deepEqual(firstMessages[0]?.message, { role: "user", content: "First" });
  assert.deepEqual(secondMessages[0]?.message, { role: "user", content: "Second" });

  await first.renameSession(sessionId, "Renamed");
  const [firstInfo, secondInfo] = await Promise.all([
    first.getSessionInfo(sessionId),
    second.getSessionInfo(sessionId),
  ]);
  assert.equal(firstInfo?.customTitle, "Renamed");
  assert.ok(secondInfo);
  assert.equal(secondInfo.customTitle, undefined);
  assert.equal(process.env.CLAUDE_CONFIG_DIR, daemonHome);
});
