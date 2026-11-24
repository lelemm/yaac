// Idle exit (docs/02-architecture.md#staying-small): `yaacd serve` exits by itself once no
// host is attached, no turn is running and no request is pending, for the idle period — and
// never earlier, never losing what it journaled.
//
// Most tests run the built bundle as a real process, so nothing in the event loop (a socket, a
// child, a timer) can hide a daemon that does not exit. Each asserts the exit within the idle
// timeout plus a slack generous enough for a slow machine.

import { describe, test } from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as net from "node:net";
import * as path from "node:path";
import { spawn, type ChildProcess } from "node:child_process";
import { fileURLToPath } from "node:url";
import { codexProviderFactory } from "../src/providers/codex/index.js";
import { FakeCodex } from "./fixtures/codex/fake-codex.js";
import { encodeMessageFrame } from "../src/protocol/frame.js";
import { encodeMessage } from "../src/protocol/rpc.js";
import { FAKE, TestClient, client, fakeOf, opId, readAll, sleep, startDaemon, text, tmpHome, until } from "./helpers.js";

const here = path.dirname(fileURLToPath(import.meta.url));
const BUNDLE = path.resolve(here, "../../yaacd.mjs");

const IDLE_MS = 1000;
/** How late an exit may be on a slow, loaded machine. */
const SLACK_MS = 4000;

class Served {
  exitedAt: number | null = null;
  exitCode: number | null = null;
  exitSignal: string | null = null;
  readonly socket: string;
  constructor(
    readonly home: string,
    readonly child: ChildProcess,
  ) {
    this.socket = path.join(home, "d.sock");
    child.on("exit", (code, signal) => {
      this.exitedAt = Date.now();
      this.exitCode = code;
      this.exitSignal = signal;
    });
  }

  static async start(home: string, idleMs = IDLE_MS, extraEnv: Record<string, string> = {}): Promise<Served> {
    // A runtime that dies of a signal while starting (seen on unstable development machines)
    // is started again; any other early exit fails the test.
    for (let attempt = 1; ; attempt++) {
      const s = await Served.startOnce(home, idleMs, extraEnv);
      if (s.alive || !s.exitSignal || attempt === 3) {
        assert.equal(s.exitedAt, null, `the daemon exited during start-up (code ${s.exitCode}, signal ${s.exitSignal})`);
        return s;
      }
    }
  }

  private static async startOnce(home: string, idleMs: number, extraEnv: Record<string, string>): Promise<Served> {
    fs.mkdirSync(path.join(home, "tmp"), { recursive: true });
    const child = spawn(process.execPath, ["--no-turbofan", BUNDLE, "serve"], {
      env: {
        ...process.env,
        YAAC_HOME: path.join(home, ".yaac"),
        YAAC_SOCKET: path.join(home, "d.sock"),
        YAAC_FAKE_PROVIDER: "1",
        YAAC_CODEX_PROVIDER: "0",
        YAAC_CLAUDE_PROVIDER: "0",
        YAAC_IDLE_TIMEOUT_MS: String(idleMs),
        TMPDIR: path.join(home, "tmp"),
        ...extraEnv,
      },
      stdio: ["ignore", "ignore", "inherit"],
    });
    const s = new Served(home, child);
    await until(async () => s.exitedAt !== null || (fs.existsSync(s.socket) && (await canConnect(s.socket))), "the daemon to listen", 15000);
    return s;
  }

  async connect(): Promise<TestClient> {
    const c = await TestClient.connect(this.socket);
    await c.initialize();
    return c;
  }

  get alive(): boolean {
    return this.exitedAt === null;
  }

  /** Waits for the exit; returns how long after `since` it came. */
  async exited(since: number, within: number, what: string): Promise<number> {
    await until(() => !this.alive, what, within);
    assert.equal(this.exitCode, 0, `a clean exit (signal ${this.exitSignal})`);
    return this.exitedAt! - since;
  }

  kill(): void {
    if (this.alive) this.child.kill("SIGKILL");
  }

  log(): string {
    try {
      return fs.readFileSync(path.join(this.home, ".yaac", "logs", "yaacd.log"), "utf8");
    } catch {
      return "";
    }
  }

  stateClean(): boolean {
    return JSON.parse(fs.readFileSync(path.join(this.home, ".yaac", "state.json"), "utf8")).clean === true;
  }
}

function canConnect(socket: string): Promise<boolean> {
  return new Promise((resolve) => {
    const s = net.connect(socket);
    s.once("connect", () => {
      s.destroy();
      resolve(true);
    });
    s.once("error", () => resolve(false));
  });
}

/** A liveness probe: connect, send nothing, close (what the hub's test harness does). */
function probe(socket: string): Promise<void> {
  return new Promise((resolve) => {
    const s = net.connect(socket);
    s.once("connect", () => {
      s.end();
      resolve();
    });
    s.once("error", () => resolve());
  });
}

async function withServed(fn: (s: Served) => Promise<void>, idleMs = IDLE_MS, env: Record<string, string> = {}): Promise<void> {
  const s = await Served.start(tmpHome("yidle-"), idleMs, env);
  try {
    await fn(s);
  } finally {
    s.kill();
  }
}

// Each case owns a daemon, a socket and a temp home. Overlap their idle waits.
describe("Native daemon idle exit", { concurrency: 3 }, () => {
  test("with no host ever attached, the daemon exits after the idle period, cleanly", async () => {
    await withServed(async (s) => {
      const t0 = Date.now();
      const took = await s.exited(t0, IDLE_MS + SLACK_MS, "the idle exit");
      assert.ok(took >= IDLE_MS - 50, `not before the idle period (${took} ms)`);
      assert.ok(s.stateClean(), "state.json records a clean stop");
      assert.ok(!fs.existsSync(s.socket), "the socket is removed");
      assert.match(s.log(), /stopping: idle/);
    });
  });

  test("liveness probes (connect, send nothing, close) do not hold it or restart the idle period", async () => {
    await withServed(async (s) => {
      const c = await s.connect();
      c.socket.destroy();
      const t0 = Date.now();
      // Probes every 100 ms, as a test harness checking whether the daemon is still up.
      let probing = true;
      const loop = (async () => {
        while (probing && s.alive) {
          await probe(s.socket);
          await sleep(100);
        }
      })();
      try {
        await s.exited(t0, IDLE_MS + SLACK_MS, "the idle exit while being probed");
      } finally {
        probing = false;
        await loop;
      }
    });
  });

  test("a connection that never initializes does not hold it forever", async () => {
    await withServed(async (s) => {
      // Connected, half a frame sent, never closed: not a host.
      const silent = await new Promise<net.Socket>((resolve, reject) => {
        const x = net.connect(s.socket);
        x.once("connect", () => resolve(x));
        x.once("error", reject);
      });
      silent.on("error", () => {});
      silent.write(encodeMessageFrame(encodeMessage({ type: "request", id: 1, method: "initialize", params: {} })).subarray(0, 6));
      const t0 = Date.now();
      try {
        // The idle period, plus the grace a connection gets to finish its handshake.
        await s.exited(t0, IDLE_MS + 10_000 + SLACK_MS, "the idle exit despite a silent connection");
      } finally {
        silent.destroy();
      }
    });
  });

  test("handshake grace, attached host, and idle exit after disconnect", async () => {
    await withServed(async (s) => {
      const c = await TestClient.connect(s.socket);
      c.sendRaw(encodeMessageFrame(encodeMessage({ type: "request", id: 0, method: "daemon/status", params: {} })));
      await until(() => c.messages.length > 0, "the frozen-core answer");
      // The idle period passes while the connection has not initialized; it is within its grace.
      await sleep(IDLE_MS + 700);
      assert.ok(s.alive, "a connection mid-handshake gets its grace");
      await c.initialize();
      const t = await c.request("thread/start", { opId: opId(), instance: FAKE, cwd: s.home, input: text("/fake say hello") });
      await c.request("thread/subscribe", { threadId: t.threadId, epoch: 1, afterSeq: 0 });
      await c.waitEvent((e) => e.event.type === "turnCompleted", "the turn");
      await sleep(IDLE_MS * 2);
      assert.ok(s.alive, "an attached host keeps the daemon after its turn completes");
      const t0 = Date.now();
      c.socket.destroy();
      await s.exited(t0, IDLE_MS + SLACK_MS, "the idle exit after the host left");
    });
  });

  test("a host that disconnects mid-turn — the turn runs to its end, journaled, and only then the idle period starts", async () => {
    await withServed(async (s) => {
      const c = await s.connect();
      const words = Array.from({ length: 60 }, (_, i) => `w${i}`);
      const t = await c.request("thread/start", { opId: opId(), instance: FAKE, cwd: s.home, input: text(`/fake pace 50\n/fake say ${words.join(" ")}`) });
      await c.request("thread/subscribe", { threadId: t.threadId, epoch: 1, afterSeq: 0 });
      await c.waitEvent((e) => e.event.type === "itemDelta", "streaming to start");
      c.socket.destroy();
      // 60 words at 50 ms: about 3 s of turn, longer than the idle period.
      await sleep(IDLE_MS + 500);
      assert.ok(s.alive, "a running turn keeps the daemon");
      await s.exited(Date.now(), 3000 + IDLE_MS + SLACK_MS, "the idle exit after the turn");
      // Nothing was lost: a fresh daemon serves the whole turn from its journal.
      const s2 = await Served.start(s.home, 0);
      try {
        const c2 = await s2.connect();
        const items = (await readAll(c2, t.threadId)).flatMap((p) => p.items);
        const agent = items.find((i) => i.kind === "agentMessage") as any;
        assert.equal(agent.text, words.join(" "));
        const sync: any = await c2.request("thread/sync", {});
        assert.equal(sync.changed.find((x: any) => x.id === t.threadId).state, "idle", "the turn completed, not interrupted");
        c2.socket.destroy();
      } finally {
        s2.kill();
      }
    });
  });

  test("a pending approval keeps the daemon up with no host; answered later, it exits", async () => {
    await withServed(async (s) => {
      const c = await s.connect();
      const t = await c.request("thread/start", { opId: opId(), instance: FAKE, cwd: s.home, input: text("/fake approve make deploy\n/fake say deployed") });
      await c.request("thread/subscribe", { threadId: t.threadId, epoch: 1, afterSeq: 0 });
      const raised = await c.waitEvent((e) => e.event.type === "itemStarted" && (e.event as any).item.kind === "request", "the approval");
      const requestId = (raised.event as any).item.requestId;
      c.socket.destroy();
      await sleep(IDLE_MS * 3);
      assert.ok(s.alive, "a pending approval is never dropped by an idle exit");
      const c2 = await s.connect();
      const sync: any = await c2.request("thread/sync", {});
      assert.ok(sync.changed.find((x: any) => x.id === t.threadId), "the thread is still there");
      await c2.request("thread/subscribe", { threadId: t.threadId, epoch: 1, afterSeq: 0 });
      const replayed = await c2.waitEvent((e) => e.event.type === "itemStarted" && (e.event as any).item.requestId === requestId, "the approval, replayed");
      assert.equal((replayed.event as any).item.state, "pending");
      await c2.request("request/respond", { opId: opId(), threadId: t.threadId, requestId, response: { requestKind: "approval", decision: "allow" } });
      await c2.waitEvent((e) => e.event.type === "turnCompleted", "the turn to finish");
      const t0 = Date.now();
      c2.socket.destroy();
      await s.exited(t0, IDLE_MS + SLACK_MS, "the idle exit once answered");
    });
  });

  test("a host that disconnects mid-upload — the partial upload is dropped and the daemon exits", async () => {
    await withServed(async (s) => {
      const c = await s.connect();
      const size = 4 * 1024 * 1024;
      const begin: any = await c.request("upload/begin", { name: "big.bin", size, sha256: "0".repeat(64) });
      c.sendData(begin.stream, Buffer.alloc(Math.min(begin.credit, 64 * 1024), 1));
      await sleep(50);
      const t0 = Date.now();
      c.socket.destroy();
      await s.exited(t0, IDLE_MS + SLACK_MS, "the idle exit after a dropped upload");
      const partials = fs.readdirSync(path.join(s.home, "tmp"), { recursive: true, encoding: "utf8" }).filter((f) => path.basename(path.dirname(f)) === "partial");
      assert.deepEqual(partials, [], "no partial upload left behind");
    });
  });

  test("a message waiting in the outbox for the hub does not hold the daemon, and survives the exit", async () => {
    await withServed(async (s) => {
      const c = await s.connect();
      await c.request("mesh/roster", { selfId: "tg_here", selfName: "here", targets: [{ id: "tg_here", name: "here", reachable: true }, { id: "tg_far", name: "far", reachable: false }] });
      const t = await c.request("thread/start", { opId: opId(), instance: FAKE, cwd: s.home, agents: "allow", input: text("/fake mesh-send tg_far/t_peer | hello over there\n/fake say sent") });
      await c.request("thread/subscribe", { threadId: t.threadId, epoch: 1, afterSeq: 0 });
      // The hub (this client) never answers the relay; the message stays in the outbox.
      await c.waitEvent((e) => e.event.type === "turnCompleted", "the turn");
      const t0 = Date.now();
      c.socket.destroy();
      await s.exited(t0, IDLE_MS + SLACK_MS, "the idle exit with mail in the outbox");
      const store = fs.readFileSync(path.join(s.home, ".yaac", "mesh", "messages.jsonl"), "utf8");
      assert.match(store, /hello over there/, "the message is on disk");
      assert.match(store, /relayPending/, "still waiting to be relayed");
    });
  });
});

// ── In process: what the stop path waits for ─────────────────────────────────────────────

test("idle exit: a provider that never finishes disposing does not keep the daemon from stopping", async () => {
  const home = tmpHome("yidle-");
  let exited = "";
  const d = await startDaemon(home, { idleTimeoutMs: 300, onExit: (r) => (exited = r) });
  fakeOf(d).dispose = () => new Promise<void>(() => {});
  const t0 = Date.now();
  await until(() => exited, "the idle stop to finish", 300 + 5000 + SLACK_MS);
  assert.equal(exited, "idle");
  assert.ok(Date.now() - t0 >= 300);
  assert.equal(JSON.parse(fs.readFileSync(path.join(home, "state.json"), "utf8")).clean, true);
});

test("idle exit: with a Codex daemon connection open, yaacd exits and leaves the Codex daemon running", async () => {
  const home = tmpHome("yidle-");
  const fake = await FakeCodex.start(path.join(home, "codex.sock"));
  fake.handlers.set("account/read", () => ({ account: { type: "apiKey" }, requiresOpenaiAuth: true }));
  fake.handlers.set("model/list", () => ({ data: [], nextCursor: null }));
  fake.handlers.set("thread/list", () => ({ data: [], nextCursor: null }));
  let exited = "";
  const d = await startDaemon(home, { idleTimeoutMs: 500, factories: [codexProviderFactory], env: { YAAC_FAKE_PROVIDER: "0", YAAC_CODEX_PROVIDER: "0" }, onExit: (r) => (exited = r) });
  try {
    const c = await client(d);
    const added: any = await c.request("providerInstance/add", { opId: opId(), kind: "codex", endpoint: { socket: fake.socket } });
    const list: any = await c.request("provider/list", {});
    assert.equal(list.providers.find((p: any) => p.instance === added.instance.id).status, "ready");
    const t0 = Date.now();
    c.socket.destroy();
    await until(() => exited, "the idle stop with Codex connected", 500 + SLACK_MS);
    assert.ok(Date.now() - t0 >= 450);
    assert.equal(exited, "idle");
    assert.ok(await canConnect(fake.socket), "the Codex daemon is never stopped by yaacd");
  } finally {
    await d.stop("test");
    await fake.close();
  }
});

test("idle exit waits for a reported producer after the originating turn ends", async () => {
  const home = tmpHome("yidle-command-");
  let exited = "";
  const d = await startDaemon(home, { idleTimeoutMs: 300, onExit: (reason) => { exited = reason; } });
  try {
    const c = await client(d);
    const started = await c.request<{ threadId: string }>("thread/start", {
      opId: opId(), instance: FAKE, cwd: home, input: text("/fake say hello"),
    });
    const thread = d.core.thread(started.threadId)!;
    const provider = fakeOf(d);
    provider.events.emit({ type: "runningCommands", thread: thread.record.native,
      commands: [{ id: "producer", command: "npm run preview", observedAt: new Date().toISOString(), canStop: false }] });
    await until(() => !thread.activeTurn, "producer's turn to end");
    await c.close();
    await sleep(450);
    assert.equal(exited, "", "the live producer keeps yaacd running");
    provider.events.emit({ type: "runningCommands", thread: thread.record.native, commands: [] });
    await until(() => exited === "idle", "idle exit after producer ends", 300 + SLACK_MS);
  } finally {
    await d.stop("test");
    fs.rmSync(home, { recursive: true, force: true });
  }
});
