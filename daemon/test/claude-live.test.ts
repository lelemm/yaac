// A live smoke test of the Claude provider against the `claude` installed on this machine.
// Skipped, not failed, when `claude` is missing or not signed in. It runs no turn: it lists
// sessions and reads the model list (which opens one query, closed straight away). It asserts
// shapes only, never particular models.

import { test } from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { ClaudeProvider } from "../src/providers/claude/index.js";
import { findExecutable, realCli } from "../src/providers/claude/sdk.js";

const exe = findExecutable(process.env);
const signedIn = exe ? await realCli(exe, process.env).authStatus() : null;
const skip = !exe ? "claude is not installed" : !signedIn?.loggedIn ? "claude is not signed in" : false;

test("live: the installed claude lists sessions and reports its models", { skip, timeout: 120000 }, async () => {
  const stateDir = fs.mkdtempSync(path.join(os.tmpdir(), "yaac-claude-live-"));
  const env = { ...process.env };
  for (const k of Object.keys(env)) if (k.startsWith("YAAC_")) delete env[k];
  const provider = new ClaudeProvider({ executable: exe, env, stateDir });
  try {
    const page = await provider.scanThreads({ limit: 5 });
    assert.ok(Array.isArray(page.threads));
    for (const t of page.threads) {
      assert.equal(typeof t.native, "string");
      assert.ok(t.originKind === "interactive" || t.originKind === "programmatic");
      assert.ok(!Number.isNaN(Date.parse(t.updatedAt)));
    }
    const probe = await provider.probe();
    assert.equal(probe.status, "ready", probe.statusMessage ?? "");
    assert.equal(typeof probe.harnessVersion, "string");
    assert.ok(probe.models.length > 0);
    for (const m of probe.models) {
      assert.equal(typeof m.id, "string");
      assert.equal(typeof m.displayName, "string");
      assert.ok(Array.isArray(m.efforts));
      for (const e of m.efforts ?? []) assert.equal(typeof e.id, "string");
    }
    assert.ok(probe.models.filter((m) => m.isDefault).length <= 1);
    assert.ok(probe.modes.length > 0);
  } finally {
    await provider.dispose();
    fs.rmSync(stateDir, { recursive: true, force: true });
  }
});
