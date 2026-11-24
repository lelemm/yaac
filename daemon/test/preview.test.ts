import { test, type TestContext } from "node:test";
import assert from "node:assert/strict";
import { once } from "node:events";
import { randomBytes } from "node:crypto";
import * as fs from "node:fs/promises";
import * as net from "node:net";
import { FrameDecoder } from "../src/protocol/frame.js";
import { destination } from "../src/session/tunnels.js";
import { linuxPorts, macPorts, windowsPorts } from "../src/session/preview-ports.js";
import { client, startDaemon, tmpHome, until } from "./helpers.js";

async function setup(t: TestContext, connection: (socket: net.Socket) => void) {
  const home = tmpHome();
  const daemon = await startDaemon(home);
  const peer = await client(daemon);
  const sockets = new Set<net.Socket>();
  const server = net.createServer((socket) => {
    sockets.add(socket);
    socket.on("error", () => {});
    socket.on("close", () => sockets.delete(socket));
    connection(socket);
  });
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  t.after(async () => {
    peer.socket.destroy();
    for (const socket of sockets) socket.destroy();
    server.close();
    await daemon.stop("test");
    await fs.rm(home, { force: true, recursive: true });
  });
  const port = (server.address() as net.AddressInfo).port;
  return { peer, port, server, sockets };
}

test("duplex echo pauses at zero credit and resumes across multiple windows", async (t) => {
  const { peer, port, sockets } = await setup(t, (s) => s.pipe(s));
  const { stream, initialCredit } = await peer.request("tunnel/open", { host: "localhost", port, initialCredit: 17 });
  const decoder = new FrameDecoder();
  const received: Buffer[] = [];
  let length = 0;
  peer.socket.on("data", (chunk) => {
    for (const f of decoder.decode(chunk)) if (f.kind === "data") {
      assert.equal(f.stream, stream);
      received.push(f.bytes);
      length += f.bytes.length;
    }
  });
  const input = randomBytes(initialCredit * 3);
  for (let offset = 0; offset < input.length; offset += initialCredit) {
    peer.sendData(stream, input.subarray(offset, offset + initialCredit));
    await until(() => peer.notifications.filter(n => n.method === "stream/credit" && n.params.stream === stream)
      .reduce((total, n) => total + n.params.bytes, 0) >= offset + initialCredit, "socket write credit");
  }
  await until(() => length >= 17);
  assert.equal(length, 17);
  peer.notify("stream/credit", { stream, bytes: input.length - 17 });
  await until(() => length >= input.length);
  assert.deepEqual(Buffer.concat(received), input);
  peer.notify("tunnel/close", { stream });
  await until(() => !sockets.size, "host close reaches socket");
});

test("remote EOF follows the last credited data frame", async (t) => {
  const { peer, port } = await setup(t, s => s.end("server banner"));
  const { stream } = await peer.request("tunnel/open", { port, initialCredit: 0 });
  peer.notify("stream/credit", { stream, bytes: 13 });
  await until(() => peer.notifications.find(n => n.method === "tunnel/close"));
  const frames = new FrameDecoder().decode(Buffer.concat(peer.rawIn));
  assert.deepEqual(frames.filter(f => f.kind === "data").map(f => f.bytes.toString()).join(""), "server banner");
  assert.ok(frames.at(-1)?.kind === "message");
});

test("any destination is resolved and reached from the target (P323)", { skip: process.platform !== "linux" }, async (t) => {
  // 127.0.0.2 is not one of the loopback names the old rule allowed; Linux routes all of 127/8.
  const { peer } = await setup(t, () => {});
  const other = net.createServer((s) => s.end("from 127.0.0.2"));
  other.listen(0, "127.0.0.2");
  await once(other, "listening");
  t.after(() => other.close());
  const port = (other.address() as net.AddressInfo).port;
  for (const host of ["127.0.0.2", "[::ffff:127.0.0.2]"]) {
    const { stream } = await peer.request("tunnel/open", { host, port, initialCredit: 0 });
    peer.notify("stream/credit", { stream, bytes: 64 });
    await until(() => peer.notifications.find(n => n.method === "tunnel/close" && n.params.stream === stream));
    const bytes = new FrameDecoder().decode(Buffer.concat(peer.rawIn))
      .filter(f => f.kind === "data" && f.stream === stream).map(f => f.kind === "data" ? f.bytes.toString() : "").join("");
    assert.equal(bytes, "from 127.0.0.2");
  }
  assert.equal(destination("example.com"), "example.com");
  assert.equal(destination("[::1]"), "::1");
  assert.equal(destination(undefined), "127.0.0.1");
});

test("reject invalid destinations and refused ports; transport loss closes all sockets", async (t) => {
  const { peer, port, server, sockets } = await setup(t, () => {});
  for (const host of ["", "/tmp/socket", "a b", "x".repeat(254), "user@host", "1:2:3", null]) {
    await assert.rejects(peer.request("tunnel/open", { host, port, initialCredit: 1 }), { code: "invalid_params" });
  }
  await Promise.all([0, 1].map(() => peer.request("tunnel/open", { port, initialCredit: 1 })));
  await until(() => sockets.size === 2, "both server-side sockets accepted");
  server.close();
  await assert.rejects(peer.request("tunnel/open", { port, initialCredit: 1 }), { code: "connection_refused" });
  assert.equal(sockets.size, 2);
  peer.socket.destroy();
  await until(() => !sockets.size, "connection teardown");
});

test("Linux proc tables join uid, LISTEN, fds, command and cwd", { skip: process.platform !== "linux" }, async (t) => {
  const root = tmpHome();
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  await fs.mkdir(root + "/net");
  const uid = process.getuid!();
  function row(host: string, inode: number, owner = uid, state = "0A") {
    return `1: ${host}:1435 00000000:0000 ${state} 0:0 0:0 0 ${owner} 0 ${inode}`;
  }
  await fs.writeFile(root + "/net/tcp", [row("0100007F", 42), row("0100007F", 43, uid + 1),
    row("0100007F", 44, uid, "01"), row("010200C0", 45)].join("\n"));
  await fs.writeFile(root + "/net/tcp6", row("00000000000000000000000001000000", 46));
  for (const [pid, command] of [[541, "/bin/node\0server.js"], [542, "/bin/codex\0app-server"]] as const) {
    await fs.mkdir(root + "/" + pid + "/fd", { recursive: true });
    await fs.writeFile(root + "/" + pid + "/cmdline", command);
    await fs.symlink("/project", root + "/" + pid + "/cwd");
    for (const inode of [42, 43, 44, 45, 46]) await fs.symlink("socket:[" + inode + "]", root + "/" + pid + "/fd/" + inode);
  }
  const ports = await linuxPorts(root, uid);
  assert.deepEqual(ports.map(p => p.host).sort(), ["127.0.0.1", "::1"]);
  assert.ok(ports.every(p => p.port === 5173 && p.command === "node" && p.cwd === "/project" && p.pid === 541));
});

test("macOS and Windows listener fixtures filter non-loopback and internal processes", () => {
  assert.deepEqual(macPorts("p113\ncnode\ntIPv6\nn*:5173\nn[::1]:8000\nn192.0.2.1:80\np114\nccodex\nn*:9090",
    "p113\nn/project"), [ { host:"::", port:5173, pid:113, command:"node", cwd:"/project" },
    { host:"::1", port:8000, pid:113, command:"node", cwd:"/project" } ]);
  const one = { host: "127.0.0.1", port: 5173, pid: 113, command: "node.exe" };
  assert.deepEqual(windowsPorts(JSON.stringify([one, { ...one, host: "192.0.2.1" }, { ...one, command: "yaacd.exe" }])), [one]);
});
