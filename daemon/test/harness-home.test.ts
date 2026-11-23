// The harness home (P300): a typed `home` on an instance's configuration, mapped by yaacd to the
// kind's own variable (Codex: CODEX_HOME, Claude Code: CLAUDE_CONFIG_DIR), offered under the
// `configurableHome` capability. Precedence: `home`, then the variable in the instance's `env`,
// then the daemon's environment (P297's `.bashrc` values on WSL), then the harness default. A
// home that leads to another store is refused with `store_change` until it is confirmed, and
// the instance's threads are not re-pointed: they show unavailable until it is changed back.

import { test } from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as path from "node:path";
import { FAKE, RpcFailure, client, opId, startDaemon, startThread, text, tmpHome, waitTurnDone } from "./helpers.js";
import { codexProviderFactory } from "../src/providers/codex/index.js";
import { claudeProviderFactory } from "../src/providers/claude/index.js";
import { planEndpoint } from "../src/providers/codex/backend.js";
import { ProviderRegistry } from "../src/providers/registry.js";
import type { HarnessProvider, ProviderContext, ProviderFactory } from "../src/providers/types.js";
import { Emitter } from "../src/util/emitter.js";

const code = (c: string) => (e: unknown) => e instanceof RpcFailure && e.code === c;

test("each kind maps the home to its own variable; Codex's socket follows it", () => {
  assert.equal(codexProviderFactory.home?.variable, "CODEX_HOME");
  assert.equal(claudeProviderFactory.home?.variable, "CLAUDE_CONFIG_DIR");
  assert.equal(codexProviderFactory.home?.defaultHome({ HOME: "/home/me" }), path.join("/home/me", ".codex"));
  assert.equal(claudeProviderFactory.home?.defaultHome({ HOME: "/home/me" }), path.join("/home/me", ".claude"));
  // The daemon yaacd connects to (or starts) is the one under the new home.
  const plan = planEndpoint({ env: { CODEX_HOME: "/home/me/harnessess/.codex" }, baseEnv: { HOME: "/home/me" } });
  assert.equal(plan.socket, path.join("/home/me/harnessess/.codex", "app-server-control", "app-server-control.sock"));
  assert.equal(plan.env.CODEX_HOME, "/home/me/harnessess/.codex");
  // Codex's store is its real home: a symlink to the same folder is the same store.
  const dir = tmpHome("yaac-codexhome-");
  fs.mkdirSync(path.join(dir, "real"));
  fs.symlinkSync(path.join(dir, "real"), path.join(dir, "link"));
  assert.equal(codexProviderFactory.home?.storeKey(path.join(dir, "link")), codexProviderFactory.home?.storeKey(path.join(dir, "real")));
  assert.notEqual(codexProviderFactory.home?.storeKey(path.join(dir, "real")), codexProviderFactory.home?.storeKey(dir));
});

/** A kind that records the configuration it was created with. */
function capturing(seen: ProviderContext[]): ProviderFactory {
  return {
    kind: "cap",
    implicit: () => false,
    home: { variable: "CAP_HOME", defaultHome: (env) => path.join(env.HOME ?? "/nohome", ".cap"), storeKey: (h) => `cap:${path.resolve(h)}` },
    create(ctx) {
      seen.push(ctx);
      const provider = {
        events: new Emitter(),
        probe: async () => ({ displayName: "Cap", backend: "sdk", store: { key: null }, status: "needsAuth", capabilities: [], models: [], modelsComplete: false, modes: [] }),
        dispose: async () => {},
      };
      return provider as unknown as HarnessProvider;
    },
  };
}

test("precedence: home, then the instance's env, then the daemon's environment, then the default", async () => {
  const dir = tmpHome("yaac-home-");
  const [a, b] = [path.join(dir, "a"), path.join(dir, "b")];
  fs.mkdirSync(a);
  fs.mkdirSync(b);
  const seen: ProviderContext[] = [];
  const reg = new ProviderRegistry(path.join(dir, "yaac"), { HOME: dir, CAP_HOME: "/from/bashrc" }, () => {});
  reg.registerKind(capturing(seen));
  reg.load();
  const withHome = await reg.add({ opId: "op1", kind: "cap", enabled: true, backend: "auto", env: { CAP_HOME: b }, home: a });
  assert.equal(withHome.home, a);
  assert.equal(seen.at(-1)!.config.env?.CAP_HOME, a, "home wins over env and the daemon's environment");
  assert.equal(reg.configuredHome(withHome), a);
  // The capability is the kind's, even when the harness is not signed in; the home it is
  // configured to use is shown when the harness did not report one.
  const d = reg.get(withHome.id)!.descriptor;
  assert.ok(d.capabilities?.includes("configurableHome"));
  assert.equal(d.store.home, a);
  const withEnv = await reg.add({ opId: "op2", kind: "cap", enabled: true, backend: "auto", env: { CAP_HOME: b } });
  assert.equal(seen.at(-1)!.config.env?.CAP_HOME, b);
  assert.equal(reg.configuredHome(withEnv), b);
  const fromProcess = await reg.add({ opId: "op3", kind: "cap", enabled: true, backend: "auto", name: "p" });
  assert.equal(reg.configuredHome(fromProcess), "/from/bashrc");
  const reg2 = new ProviderRegistry(path.join(dir, "yaac2"), { HOME: dir }, () => {});
  reg2.registerKind(capturing([]));
  reg2.load();
  const plain = await reg2.add({ opId: "op4", kind: "cap", enabled: true, backend: "auto", name: "d" });
  assert.equal(reg2.configuredHome(plain), path.join(dir, ".cap"));
  // `~` is the daemon's home; a relative path or a missing folder is refused.
  const tilde = await reg2.add({ opId: "op5", kind: "cap", enabled: true, backend: "auto", home: "~/a" });
  assert.equal(tilde.home, a);
  await assert.rejects(reg2.add({ opId: "op6", kind: "cap", enabled: true, backend: "auto", home: "rel/dir" }), /absolute/);
  await assert.rejects(reg2.add({ opId: "op7", kind: "cap", enabled: true, backend: "auto", home: path.join(dir, "missing") }), /no folder/);
  await reg.dispose();
  await reg2.dispose();
});

test("another store waits for confirmation; the threads stay and show unavailable until it is changed back", async () => {
  const home = tmpHome();
  const daemon = await startDaemon(home);
  const c = await client(daemon);
  const list: any = await c.request("provider/list");
  const fake = list.providers.find((p: any) => p.instance === FAKE);
  assert.ok(fake.capabilities.includes("configurableHome"));
  const original: string = fake.store.home;
  assert.ok(original);
  const { threadId, turnId } = await startThread(c, "/fake say hello");
  await c.request("thread/subscribe", { threadId, epoch: 1, afterSeq: 0 });
  await waitTurnDone(c, threadId, turnId);

  // The same folder spelled differently is the same store (the kind's store key): no question.
  const link = original;
  await c.request("providerInstance/update", { opId: opId(), id: FAKE, home: `${original}/./` });
  await c.request("providerInstance/update", { opId: opId(), id: FAKE, home: link });
  assert.equal(daemon.core.summaryOf(daemon.core.thread(threadId)!).unavailable, undefined);

  // Another folder: refused, nothing changed, the two homes in the error's data.
  const other = path.join(home, "other-store");
  fs.mkdirSync(other);
  const err = await c.request("providerInstance/update", { opId: opId(), id: FAKE, home: other }).catch((e) => e);
  assert.ok(code("store_change")(err), String(err));
  assert.equal((err as RpcFailure).error.data && ((err as RpcFailure).error.data as any).next, other);
  const cfgs: any = await c.request("providerInstance/list");
  assert.equal(cfgs.instances.find((i: any) => i.id === FAKE).home, link, "unchanged");

  // Confirmed (a new operation): changed; the thread is kept, not re-pointed, and unavailable.
  await c.request("providerInstance/update", { opId: opId(), id: FAKE, home: other, confirmStoreChange: true });
  const away = daemon.core.summaryOf(daemon.core.thread(threadId)!);
  assert.match(away.unavailable ?? "", /another store/);
  assert.equal(away.instance, FAKE);
  await assert.rejects(c.request("turn/start", { opId: opId(), threadId, input: text("/fake say again") }), code("provider_unavailable"));
  // A thread started now belongs to the new store.
  const fresh = await startThread(c, "/fake say new");
  await c.request("thread/subscribe", { threadId: fresh.threadId, epoch: 1, afterSeq: 0 });
  await waitTurnDone(c, fresh.threadId, fresh.turnId);
  assert.equal(daemon.core.summaryOf(daemon.core.thread(fresh.threadId)!).unavailable, undefined);

  // Back to the default (null): another store again, so confirmed; the old thread works again.
  await assert.rejects(c.request("providerInstance/update", { opId: opId(), id: FAKE, home: null }), code("store_change"));
  await c.request("providerInstance/update", { opId: opId(), id: FAKE, home: null, confirmStoreChange: true });
  assert.equal(daemon.core.summaryOf(daemon.core.thread(threadId)!).unavailable, undefined);
  assert.match(daemon.core.summaryOf(daemon.core.thread(fresh.threadId)!).unavailable ?? "", /another store/);
  const again: any = await c.request("turn/start", { opId: opId(), threadId, input: text("/fake say again") });
  await waitTurnDone(c, threadId, again.turnId);
  await daemon.stop("test");
});
