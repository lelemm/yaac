// A read-only smoke test against the real Codex daemon on this machine. Skipped — not failed —
// when there is no running daemon or no sign-in. It never starts a daemon (the socket is given
// explicitly, which makes the instance connect-only), never starts a turn, never creates,
// renames, archives, or deletes a thread, and asserts only shapes, never specific models.

import { test } from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { CodexProvider } from "../src/providers/codex/index.js";

const SOCKET = path.join(process.env.CODEX_HOME || path.join(os.homedir(), ".codex"), "app-server-control", "app-server-control.sock");

function socketPresent(): boolean {
  try {
    return fs.statSync(SOCKET).isSocket();
  } catch {
    return false;
  }
}

test("codex live: connect, initialize, list models and threads (read-only)", { skip: socketPresent() ? false : `no Codex daemon socket at ${SOCKET}` }, async (t) => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "yaac-cxlive-"));
  const provider = new CodexProvider(
    { config: { id: "pi_codex_live", kind: "codex", enabled: true, backend: "auto" }, stateDir: path.join(dir, "state"), home: dir, env: process.env, log: () => {} },
    {
      socket: SOCKET,
      exec: async () => {
        throw new Error("the live smoke test must never run codex");
      },
    },
  );
  try {
    const probe = await provider.probe();
    if (probe.status === "unreachable") {
      t.skip(`the Codex daemon is not answering: ${probe.statusMessage}`);
      return;
    }
    if (probe.status === "needsAuth") {
      t.skip("Codex is not signed in");
      return;
    }
    assert.equal(probe.status, "ready", probe.statusMessage ?? "");
    assert.equal(probe.backend, "daemon");
    assert.match(probe.harnessVersion ?? "", /^\d+\.\d+/);
    assert.match(probe.store.key ?? "", /^codex:[0-9a-f]{16}$/);
    assert.equal(typeof probe.store.home, "string");
    assert.equal(probe.modelsComplete, true);
    assert.ok(probe.models.length > 0);
    assert.equal(new Set(probe.models.map((m) => m.id)).size, probe.models.length, "model ids are unique across pages");
    assert.ok(probe.models.filter((m) => m.isDefault).length <= 1);
    for (const m of probe.models) {
      assert.equal(typeof m.id, "string");
      assert.ok(m.id.length > 0);
      assert.equal(typeof m.displayName, "string");
      for (const e of m.efforts ?? []) assert.equal(typeof e.id, "string");
      const img = m.capabilities?.find((c) => c.capability === "imageInput")?.support;
      assert.ok(img === "supported" || img === "unsupported" || img === "unknown");
    }
    const page = await provider.scanThreads({ limit: 5 });
    assert.ok(Array.isArray(page.threads));
    for (const th of page.threads) {
      assert.equal(typeof th.native, "string");
      assert.ok(th.originKind === "interactive" || th.originKind === "programmatic");
      assert.ok(!Number.isNaN(Date.parse(th.createdAt)));
      assert.equal(th.archived, false);
    }
  } finally {
    await provider.dispose();
    fs.rmSync(dir, { recursive: true, force: true });
  }
});
