// Framing, canonical JSON, scheduling, negotiation, the dialect window, and the frozen core.

import { test } from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as path from "node:path";
import { fileURLToPath } from "node:url";
import { FrameDecoder, FrameError, encodeDataFrame, encodeMessageFrame, MAX_FRAME_LEN } from "../src/protocol/frame.js";
import { canonical } from "../src/protocol/json.js";
import { decodeMessage, encodeMessage } from "../src/protocol/rpc.js";
import { CHUNK_MAX, CHUNK_MIN, ReceiveWindow, Scheduler, CreditExceeded } from "../src/protocol/scheduler.js";
import { dialectCount, negotiate, NegotiationError } from "../src/protocol/negotiation.js";
import { client, opId, startDaemon, text, tmpHome, until, FAKE, TestClient } from "./helpers.js";
import * as os from "node:os";

const here = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(here, "../../../..");

test("frames survive split and coalesced reads", () => {
  const a = encodeMessageFrame('{"id":1,"method":"ping","params":{}}');
  const b = encodeDataFrame(7, Buffer.from("hello"));
  const all = Buffer.concat([a, b, a]);
  for (let split = 1; split < all.length; split += 3) {
    const d = new FrameDecoder();
    const frames = [...d.decode(all.subarray(0, split)), ...d.decode(all.subarray(split))];
    assert.equal(frames.length, 3);
    assert.equal(frames[1]!.kind, "data");
    if (frames[1]!.kind === "data") {
      assert.equal(frames[1]!.stream, 7);
      assert.equal(frames[1]!.bytes.toString(), "hello");
    }
  }
  // Byte by byte.
  const d = new FrameDecoder();
  let n = 0;
  for (const byte of all) n += d.decode(Buffer.of(byte)).length;
  assert.equal(n, 3);
});

test("zero length, oversize, unknown kind, and short data frames are fatal", () => {
  const cases: Array<[Buffer, string]> = [
    [Buffer.from([0, 0, 0, 0]), "empty"],
    [Buffer.from([0x01, 0x00, 0x10, 0x00, 0x01]), "oversize"], // 1 MiB + 1
    [Buffer.from([2, 0, 0, 0, 0x07, 0]), "unknownKind"],
    [Buffer.from([3, 0, 0, 0, 0x02, 1, 2]), "shortData"],
  ];
  for (const [bytes, reason] of cases) {
    const d = new FrameDecoder();
    assert.throws(() => d.decode(bytes), (e: unknown) => e instanceof FrameError && e.reason === reason);
    // Poisoned: the same error forever.
    assert.throws(() => d.decode(encodeMessageFrame("{}")), FrameError);
  }
  // Exactly the limit is fine; one more is not (measured on the length field).
  assert.doesNotThrow(() => encodeMessageFrame(Buffer.alloc(MAX_FRAME_LEN - 1)));
  assert.throws(() => encodeMessageFrame(Buffer.alloc(MAX_FRAME_LEN)), FrameError);
});

test("canonical JSON: envelope order, sorted keys, no jsonrpc member", () => {
  assert.equal(canonical({ b: 1, a: { d: [3, { z: 1, y: 2 }], c: null }, u: undefined }), '{"a":{"c":null,"d":[3,{"y":2,"z":1}]},"b":1}');
  assert.equal(encodeMessage({ type: "request", id: 3, method: "x", params: { b: 2, a: 1 } }), '{"id":3,"method":"x","params":{"a":1,"b":2}}');
  assert.equal(encodeMessage({ type: "error", id: null, error: { code: "parse_error", message: "m" } }), '{"id":null,"error":{"code":"parse_error","message":"m"}}');
  assert.equal(encodeMessage({ type: "notification", method: "n", params: {} }), '{"method":"n","params":{}}');
  // A jsonrpc member is ignored on input; unknown fields are ignored.
  const m = decodeMessage('{"jsonrpc":"2.0","id":5,"method":"ping","extra":1}');
  assert.deepEqual(m, { type: "request", id: 5, method: "ping", params: {} });
});

test("scheduler: a control frame waits behind at most one bulk chunk", () => {
  const s = new Scheduler();
  s.openStream(1, 10 * 1024 * 1024);
  s.sendData(1, Buffer.alloc(4 * 1024 * 1024));
  for (let i = 0; i < 10; i++) s.pushFrame("bulk", encodeMessageFrame(Buffer.alloc(60 * 1024, 0x20)));
  // A saturated upload and large pages are queued; control arrives at arbitrary moments.
  for (let round = 0; round < 50; round++) {
    const f = s.next();
    assert.ok(f);
    assert.equal(f.class, "bulk");
    assert.ok(f.bytes.length <= CHUNK_MAX + 9 || f.bytes.length <= 60 * 1024 + 5);
    s.pushFrame("control", encodeMessageFrame(`{"id":${round},"result":{}}`));
    const next = s.next();
    assert.equal(next?.class, "control", "control goes before any further bulk");
  }
});

test("scheduler: credit stops a stream, grants resume it; chunks adapt between 16 and 64 KiB", () => {
  const s = new Scheduler();
  s.openStream(9, 20000);
  s.sendData(9, Buffer.alloc(100000));
  const first = s.next()!;
  assert.equal(first.bytes.length, 9 + CHUNK_MIN);
  const second = s.next()!;
  assert.equal(second.bytes.length, 9 + (20000 - CHUNK_MIN));
  assert.equal(s.next(), null, "no credit left");
  s.grantCredit(9, 1000);
  assert.equal(s.next()!.bytes.length, 9 + 1000);
  // Fast link: grows, at most doubling, to the cap.
  for (let i = 0; i < 10; i++) s.onWritten(s.chunkSize(), 1);
  assert.equal(s.chunkSize(), CHUNK_MAX);
  // Slow link: shrinks to the floor.
  for (let i = 0; i < 10; i++) s.onWritten(s.chunkSize(), 5000);
  assert.equal(s.chunkSize(), CHUNK_MIN);
});

test("receive window: credit returns only once bytes are consumed; overrun is an error", () => {
  const w = new ReceiveWindow(3, 1000);
  w.onReceived(600);
  assert.equal(w.onConsumed(400), null);
  assert.equal(w.onConsumed(200), 600);
  w.onReceived(1000);
  assert.throws(() => w.onReceived(1), CreditExceeded);
});

test("negotiation: overlap, and no overlap in each direction", () => {
  assert.match(negotiate({ min: 1, max: 5 }).diagnosticLabel(), /protocol v1/);
  assert.deepEqual(negotiate({ min: 1, max: 1 }).answer(), { min: 1, max: 1, agreed: 1 });
  const daemonTooOld = (() => {
    try {
      negotiate({ min: 7, max: 9 });
    } catch (e) {
      return e as NegotiationError;
    }
    return null;
  })();
  assert.equal(daemonTooOld?.tooOld(), "daemon");
  assert.equal(daemonTooOld?.toErrorObject().code, "protocol_unsupported");
  // A client whose range ends below ours (impossible with v1, shown with an inverted range).
  assert.throws(() => negotiate({ min: 2, max: 1 }), NegotiationError);
  assert.ok(dialectCount() >= 1 && dialectCount() <= 4, "current plus at most three older dialects");
});

test("the frozen core is reproduced byte for byte (protocol/schema/frozen-core-v1.txt)", async () => {
  const fixture = fs.readFileSync(path.join(repo, "protocol/schema/frozen-core-v1.txt"), "utf8");
  const lines = fixture.split("\n").filter((l) => l && !l.startsWith("#"));
  const host = lines.filter((l) => l.startsWith("> ")).map((l) => Buffer.from(l.slice(2), "hex"));
  const expected = Buffer.concat(lines.filter((l) => l.startsWith("< ")).map((l) => Buffer.from(l.slice(2), "hex")));

  const home = tmpHome();
  let exited = "";
  const daemon = await startDaemon(home, { build: "0.1.0+fixture", onExit: (r) => (exited = r) });
  // A turn keeps running after its client has gone (the fixture's busy state).
  const a = await client(daemon, "c_other");
  await a.request("thread/start", { opId: opId(), instance: FAKE, cwd: os.tmpdir(), input: text("/fake stall") });
  await a.close();
  await until(() => daemon.busy().attachedClients === 0, "first client gone");

  const b = await TestClient.connect(daemon.socketPath);
  for (const frame of host) {
    const before = Buffer.concat(b.rawIn).length;
    b.sendRaw(frame);
    await until(() => Buffer.concat(b.rawIn).length > before || b.closed, "an answer");
    await until(async () => {
      const len = Buffer.concat(b.rawIn).length;
      await new Promise((r) => setTimeout(r, 20));
      return Buffer.concat(b.rawIn).length === len;
    }, "the answer to settle");
  }
  await b.waitClosed();
  const got = Buffer.concat(b.rawIn);
  assert.equal(got.toString("hex"), expected.toString("hex"));
  await until(() => exited, "the daemon to exit");
  assert.equal(exited, "handover now");
});
