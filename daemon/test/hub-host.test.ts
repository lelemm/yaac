import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as http from "node:http";
import * as net from "node:net";
import * as os from "node:os";
import * as path from "node:path";
import { test, type TestContext } from "node:test";
import { UPLOAD_WINDOW } from "../src/session/uploads.js";
import { sha256Hex } from "../src/util/fsx.js";
import { startDaemon, TestClient, tmpHome, until } from "./helpers.js";

const linuxTest = { skip: process.platform !== "linux" };
const exe = Buffer.from('#!/bin/sh\n[ "$1" = hub ] || exit 2\nprintf started > "$YAAC_DATA_DIR/marker"\n');
const config = Buffer.from('{"hub":{"always":true}}\n');

async function fixture(t: TestContext, options: {
  http?: boolean;
  systemd?: boolean;
  idleTimeoutMs?: number;
  probePort?: number;
  wslDistro?: string;
  wslInterop?: boolean;
  tailscale?: boolean;
  windows?: { running?: boolean; autostartOff?: boolean; noExe?: boolean };
} = {}) {
  const root = tmpHome("yaac-hub-host-");
  const commands = path.join(root, "commands");
  const data = options.windows ? path.join(root, "Windows hub data") : path.join(root, "data", "yaac");
  const settings = options.windows ? path.join(root, "Windows hub config") : path.join(root, "config", "yaac");
  const runtime = options.windows ? path.join(root, "Windows hub runtime") : path.join(root, "run", "yaac");
  const yaacExe = path.join(commands, "YAAC installation.exe");
  const wslInteropPath = path.join(root, "WSLInterop");
  fs.mkdirSync(commands);
  if (options.wslInterop) fs.writeFileSync(wslInteropPath, "enabled\n");
  if (options.tailscale !== false) fs.writeFileSync(path.join(commands, "tailscale"), `#!/bin/sh
[ "$*" = "status --json" ] || exit 1
cat <<'JSON'
{"BackendState":"Running","Self":{"UserID":12345678901234567890,"TailscaleIPs":["127.0.0.2","127.0.0.3"]},"User":{"12345678901234567890":{"LoginName":"me@example.com"}}}
JSON
`, { mode: 0o700 });
  fs.writeFileSync(path.join(commands, "systemctl"), options.systemd ? `#!/bin/sh
case "$*" in
"--user show-environment") printf checked > "$YAAC_TEST_HUB_DATA/systemd-check"; exit 0 ;;
"--user enable --now yaac-hub.service")
  sleep 0.8
  printf systemd > "$YAAC_TEST_HUB_DATA/marker"
  exit 0
  ;;
esac
exit 1
` : "#!/bin/sh\nexit 1\n", { mode: 0o700 });
  if (options.windows && !options.windows.noExe) {
    fs.writeFileSync(yaacExe, `#!${process.execPath}
const fs = require("node:fs");
const path = require("node:path");
const info = ${JSON.stringify({
  build: "1.2.3+windows", configDir: settings, dataDir: data, runtimeDir: runtime,
  running: options.windows.running ?? false,
})};
const args = process.argv.slice(2).join(" ");
const tombstone = path.join(info.dataDir, "moved.json");
if (args === "hub-info") {
  console.log(JSON.stringify({ ...info, moved: fs.existsSync(tombstone) }));
} else if (args === "autostart sync --start") {
  if (fs.existsSync(tombstone)) throw new Error("tombstone still present");
  const config = fs.readFileSync(path.join(info.configDir, "hub.json"), "utf8");
  fs.writeFileSync(path.join(info.dataDir, "autostart-call"), JSON.stringify({ args, config }));
  console.log(${JSON.stringify(options.windows.autostartOff ? "off" : "on")});
} else if (args === "hub") {
  fs.writeFileSync(path.join(info.dataDir, "marker"), args);
} else {
  throw new Error("unexpected YAAC command " + args);
}
`, { mode: 0o700 });
  }
  const daemon = await startDaemon(root, {
    env: {
      ...process.env, YAAC_FAKE_PROVIDER: "1", YAAC_CLAUDE_PROVIDER: "0", YAAC_CODEX_PROVIDER: "0",
      YAAC_CONFIG_DIR: "", YAAC_DATA_DIR: "", YAAC_RUNTIME_DIR: "", YAAC_AUTOSTART_DIR: "",
      YAAC_TEST_HUB_DATA: data,
      YAAC_EXE: options.windows && !options.windows.noExe ? yaacExe : undefined,
      HOME: root, XDG_CONFIG_HOME: path.join(root, "config"), XDG_DATA_HOME: path.join(root, "data"),
      XDG_RUNTIME_DIR: path.join(root, "run"), WSL_DISTRO_NAME: options.wslDistro,
      PATH: options.tailscale === false ? commands : commands + path.delimiter + process.env.PATH,
    },
    hubHost: {
      allowHttpForTests: options.http, probePortForTests: options.probePort ?? 0, wslInteropPathForTests: wslInteropPath,
      platformForTests: options.windows ? "win32" : undefined,
    },
    idleTimeoutMs: options.idleTimeoutMs ?? 0,
  });
  t.after(async () => {
    await daemon.stop("test finished");
    fs.rmSync(root, { recursive: true, force: true });
  });
  const client = await TestClient.connect(daemon.socketPath);
  const init = await client.initialize();
  assert.ok(init.features.includes("hubHost"));
  return { daemon, client, data, settings, runtime, yaacExe };
}

async function put(client: TestClient, opId: string, name: string, bytes: Buffer, sha256 = sha256Hex(bytes)) {
  const { stream, credit } = await client.request("hubHost/put", { opId, name, size: bytes.length, sha256 });
  assert.ok(credit >= bytes.length);
  if (bytes.length) client.sendData(stream, bytes);
  return until(() => client.notifications.find((n) => n.method === "hubHost/putComplete" && n.params.stream === stream));
}

test("probe reports Tailscale, port and newest recorded transfer", linuxTest, async (t) => {
  const port = net.createServer();
  t.after(() => { port.close(); });
  // Hold an ephemeral port on a different loopback address while testing the available port.
  await new Promise<void>((resolve, reject) => port.once("error", reject).listen(0, "127.0.0.1", resolve));
  const probePort = (port.address() as net.AddressInfo).port;
  const { client, data, runtime } = await fixture(t, { probePort });
  fs.mkdirSync(data, { recursive: true });
  const older = path.join(data, "transfer-old.result");
  fs.writeFileSync(older, JSON.stringify({ opId: "old", status: "started" }));
  fs.utimesSync(older, new Date(1), new Date(1));
  const lastTransfer = { opId: "recent", status: "failed", message: "startup failed" };
  fs.writeFileSync(path.join(data, "transfer-recent.result"), JSON.stringify(lastTransfer));
  fs.mkdirSync(runtime, { recursive: true });
  fs.writeFileSync(path.join(runtime, "hub.lock"), "999999999\n");
  assert.deepEqual(await client.request("hubHost/probe"), {
    os: "linux", arch: os.machine(), tailscale: { running: true, login: "me@example.com", ips: ["127.0.0.2", "127.0.0.3"] },
    portFree: true, hubRunning: false, lastTransfer,
  });

  const occupiedPort = net.createServer();
  const hub = net.createServer();
  t.after(() => { occupiedPort.close(); hub.close(); });
  await Promise.all([
    new Promise<void>((resolve, reject) => occupiedPort.once("error", reject).listen(probePort, "127.0.0.2", resolve)),
    new Promise<void>((resolve, reject) => hub.once("error", reject).listen(path.join(runtime, "hub.sock"), resolve)),
  ]);
  const occupied = await client.request("hubHost/probe");
  assert.equal(occupied.portFree, false);
  assert.equal(occupied.hubRunning, true);
});

test("a named WSL distro probes its own Tailscale", linuxTest, async (t) => {
  const { client } = await fixture(t, { wslDistro: "YAAC Test" });
  const probe = await client.request("hubHost/probe");
  assert.deepEqual(probe.wsl, { distro: "YAAC Test" });
  assert.deepEqual(probe.tailscale, { running: true, login: "me@example.com", ips: ["127.0.0.2", "127.0.0.3"] });
  assert.equal(probe.portFree, true);
});

test("the kernel marker identifies WSL when the distro name and Tailscale are unavailable", linuxTest, async (t) => {
  const { client } = await fixture(t, { wslInterop: true, tailscale: false });
  const probe = await client.request("hubHost/probe");
  assert.deepEqual(probe.wsl, { distro: "WSL" });
  assert.deepEqual(probe.tailscale, { running: false, ips: [] });
  assert.equal(probe.portFree, false);
});

test("Windows probes report the installed hub build and running state", linuxTest, async (t) => {
  const { client, yaacExe } = await fixture(t, { windows: { running: true }, wslDistro: "ignored on Windows" });
  const expected = {
    os: "win32", arch: os.machine(), tailscale: { running: true, login: "me@example.com", ips: ["127.0.0.2", "127.0.0.3"] },
    portFree: true,
  };
  assert.deepEqual(await client.request("hubHost/probe"), { ...expected, hubBuild: "1.2.3+windows", hubRunning: true });
  fs.unlinkSync(yaacExe);
  assert.deepEqual(await client.request("hubHost/probe"), { ...expected, hubRunning: false });
});

test("Windows reports no build without YAAC_EXE and refuses binary installation", linuxTest, async (t) => {
  const { client } = await fixture(t, { windows: { noExe: true } });
  assert.equal((await client.request("hubHost/probe")).hubBuild, undefined);
  const opId = "windows-install";
  const size = exe.length;
  const sha256 = sha256Hex(exe);
  await assert.rejects(client.request("hubHost/install", { opId, url: "https://example.test/asset", size, sha256 }), /installed YAAC executable/);
  await assert.rejects(client.request("hubHost/put", { opId, name: "bin/yaac", size, sha256 }), /installed YAAC executable/);
});

for (const autostartOff of [false, true]) {
  test(`Windows promotes into the installed hub profile with autostart ${autostartOff ? "off" : "on"}`, linuxTest, async (t) => {
    const { daemon, client, data, settings } = await fixture(t, { windows: { autostartOff }, systemd: true });
    fs.mkdirSync(data);
    fs.mkdirSync(settings);
    const tombstone = path.join(data, "moved.json");
    const configPath = path.join(settings, "hub.json");
    const autostartCall = path.join(data, "autostart-call");
    fs.writeFileSync(tombstone, "old tombstone");
    fs.writeFileSync(configPath, "old settings");
    assert.equal((await put(client, "cancel", "config/hub.json", config)).params.message, undefined);
    assert.deepEqual(await client.request("hubHost/abort", { opId: "cancel" }), {});
    assert.equal(fs.existsSync(path.join(data, "transfer-cancel")), false);

    const opId = "windows-start";
    const transferDir = path.join(data, `transfer-${opId}`);
    const resultPath = path.join(data, `transfer-${opId}.result`);
    const transferredConfig = autostartOff ? Buffer.from('{"hub":{"always":false}}\n') : config;
    assert.equal((await put(client, opId, "config/hub.json", transferredConfig)).params.message, undefined);
    assert.equal((await put(client, opId, "data/relay.sqlite", Buffer.from("relay"))).params.message, undefined);
    assert.deepEqual(fs.readFileSync(path.join(transferDir, "config", "hub.json")), transferredConfig);
    assert.deepEqual(await client.request("hubHost/start", { opId, armOnClose: true }), { armed: true });
    assert.equal(fs.readFileSync(configPath, "utf8"), "old settings");
    assert.equal(fs.existsSync(tombstone), true);
    assert.equal(fs.existsSync(autostartCall), false);
    await client.close();

    await until(() => fs.existsSync(resultPath), "the Windows transfer result");
    assert.deepEqual(JSON.parse(fs.readFileSync(resultPath, "utf8")), { opId, status: "started" });
    assert.equal(fs.existsSync(tombstone), false);
    assert.deepEqual(fs.readFileSync(configPath), transferredConfig);
    assert.equal(fs.readFileSync(path.join(data, "relay.sqlite"), "utf8"), "relay");
    assert.equal(fs.readFileSync(path.join(transferDir, "replaced", "config", "hub.json"), "utf8"), "old settings");
    assert.deepEqual(JSON.parse(fs.readFileSync(autostartCall, "utf8")), { args: "autostart sync --start", config: transferredConfig.toString() });
    assert.equal(fs.existsSync(path.join(data, "systemd-check")), false);
    const marker = path.join(data, "marker");
    if (autostartOff) await until(() => fs.existsSync(marker), "the detached Windows hub marker");
    assert.equal(fs.existsSync(marker), autostartOff);
    const observer = await TestClient.connect(daemon.socketPath);
    await observer.initialize();
    assert.deepEqual((await observer.request("hubHost/probe")).lastTransfer, { opId, status: "started" });
    await observer.close();
  });
}

test("install verifies a download before publishing the executable", linuxTest, async (t) => {
  const { client, data } = await fixture(t, { http: true });
  const server = http.createServer((req, res) => {
    if (req.url === "/release") { res.writeHead(302, { Location: "/asset" }).end(); return; }
    res.end(exe);
  });
  t.after(() => { server.close(); });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const url = `http://127.0.0.1:${(server.address() as net.AddressInfo).port}/release`;
  const opId = "install";
  const result = await client.request("hubHost/install", { opId, url, size: exe.length, sha256: sha256Hex(exe) });
  assert.equal(result.path, path.join(data, "transfer-install", "bin", "yaac"));
  assert.deepEqual(fs.readFileSync(result.path), exe);
  fs.accessSync(result.path, fs.constants.X_OK);
  assert.deepEqual(client.notifications.filter((n) => n.method === "hubHost/progress").map((n) => n.params.done), [0, exe.length]);

  await assert.rejects(client.request("hubHost/install", { opId: "invalid", url, size: exe.length + 1, sha256: sha256Hex(exe) }), /expected/);
  assert.deepEqual(fs.readdirSync(path.join(data, "transfer-invalid")), ["bin"]);
  assert.deepEqual(fs.readdirSync(path.join(data, "transfer-invalid", "bin")), []);
});

test("puts reject traversal and bad hashes and discard partial writes", linuxTest, async (t) => {
  const { client, data } = await fixture(t);
  const opId = "put";
  for (const name of ["data/../config/hub.json", "data/keys/../relay.sqlite", "/bin/yaac", "data/keys/a/b/../../x"]) {
    await assert.rejects(client.request("hubHost/put", { opId, name, size: 0, sha256: sha256Hex("") }), /invalid hub transfer file name/);
  }
  await assert.rejects(client.request("hubHost/put", { opId: "../../put", name: "bin/yaac", size: 0, sha256: sha256Hex("") }), /invalid hub transfer opId/);
  await assert.rejects(client.request("hubHost/install", { opId, url: "http://127.0.0.1", size: 0, sha256: sha256Hex("") }), /HTTPS/);
  const result = await put(client, opId, "config/hub.json", config, sha256Hex(""));
  assert.match(result.params.message, /SHA-256/);
  const root = path.join(data, "transfer-put");
  assert.deepEqual(fs.readdirSync(root), ["config"]);
  assert.deepEqual(fs.readdirSync(path.join(root, "config")), []);
  await assert.rejects(client.request("hubHost/start", { opId, armOnClose: true }), /SHA-256/);

  const partial = await client.request("hubHost/put", { opId, name: "data/cache.sqlite", size: 10, sha256: sha256Hex("0123456789") });
  client.sendData(partial.stream, Buffer.from("012"));
  await client.request("hubHost/abort", { opId });
  assert.equal(fs.existsSync(root), false);
});

test("only the armed connection's close promotes files, preserves backups and starts the hub", linuxTest, async (t) => {
  const { daemon, client, data, settings } = await fixture(t, { idleTimeoutMs: 1500 });
  const other = await TestClient.connect(daemon.socketPath);
  await other.initialize("other");
  const opId = "start";
  assert.equal((await put(client, opId, "bin/yaac", exe)).params.message, undefined);
  assert.equal((await put(client, opId, "data/ssh/keys/me@server_22-222/id_ed25519", Buffer.from("key"))).params.message, undefined);
  assert.equal((await put(client, opId, "data/ssh/known_hosts", Buffer.from("host key"))).params.message, undefined);
  fs.mkdirSync(settings, { recursive: true });
  fs.writeFileSync(path.join(settings, "hub.json"), "old");
  fs.writeFileSync(path.join(data, "moved.json"), "old tombstone");
  const pendingConfig = await client.request("hubHost/put", { opId, name: "config/hub.json", size: config.length, sha256: sha256Hex(config) });
  const arm = client.request("hubHost/start", { opId, armOnClose: true });
  client.sendData(pendingConfig.stream, config);
  assert.deepEqual(await arm, { armed: true });
  await assert.rejects(other.request("hubHost/abort", { opId }), /in use/);
  await other.close();
  assert.equal(fs.readFileSync(path.join(settings, "hub.json"), "utf8"), "old");
  assert.equal(fs.existsSync(path.join(data, "marker")), false);
  await client.close();

  await until(() => fs.existsSync(path.join(data, "marker")), "the detached hub marker");
  const resultFile = path.join(data, "transfer-start.result");
  await until(() => fs.existsSync(resultFile), "the transfer result");
  assert.deepEqual(JSON.parse(fs.readFileSync(resultFile, "utf8")), { opId, status: "started" });
  assert.deepEqual(fs.readFileSync(path.join(settings, "hub.json")), config);
  assert.equal(fs.readFileSync(path.join(data, "ssh", "keys", "me@server_22-222", "id_ed25519"), "utf8"), "key");
  assert.equal(fs.readFileSync(path.join(data, "ssh", "known_hosts"), "utf8"), "host key");
  assert.equal(fs.readFileSync(path.join(data, "transfer-start", "replaced", "config", "hub.json"), "utf8"), "old");
  assert.equal(fs.readFileSync(path.join(data, "transfer-start", "replaced", "data", "moved.json"), "utf8"), "old tombstone");
  assert.equal(fs.existsSync(path.join(data, "moved.json")), false);
});

test("a large database stream replenishes credit and verifies a replacement", linuxTest, async (t) => {
  const { client, data } = await fixture(t);
  const opId = "cache";
  const name = "data/cache.sqlite";
  await put(client, opId, name, Buffer.from("initial cache"));
  const bytes = Buffer.alloc(UPLOAD_WINDOW + 19, 0x61);
  const { stream, credit } = await client.request("hubHost/put", { opId, name, size: bytes.length, sha256: sha256Hex(bytes) });
  assert.equal(credit, UPLOAD_WINDOW);
  client.sendData(stream, bytes.subarray(0, credit));
  const grant = await until(() => client.notifications.find((n) => n.method === "stream/credit" && n.params.stream === stream));
  assert.equal(grant.params.bytes, 19);
  client.sendData(stream, bytes.subarray(credit));
  const result = await until(() => client.notifications.find((n) => n.method === "hubHost/putComplete" && n.params.stream === stream));
  assert.equal(result.params.message, undefined);
  assert.deepEqual(fs.readFileSync(path.join(data, "transfer-cache", name)), bytes);
});

test("abort disarms and a user systemd manager is the sole starter when available", linuxTest, async (t) => {
  const { daemon, client, data, settings } = await fixture(t, { systemd: true, idleTimeoutMs: 500 });
  for (const opId of ["cancel", "systemd"]) {
    await put(client, opId, "bin/yaac", exe);
    await put(client, opId, "config/hub.json", config);
    await client.request("hubHost/start", { opId, armOnClose: true });
    if (opId === "cancel") {
      await client.request("hubHost/abort", { opId });
      assert.equal(fs.existsSync(path.join(data, "transfer-cancel")), false);
    }
  }
  await client.close();
  await until(() => fs.existsSync(path.join(data, "transfer-systemd.result")), "the transfer result");
  assert.equal(daemon.isStopping, false);
  assert.equal(fs.readFileSync(path.join(data, "marker"), "utf8"), "systemd");
  const unit = path.join(settings, "..", "systemd", "user", "yaac-hub.service");
  assert.ok(fs.readFileSync(unit, "utf8").includes(`${path.join(data, "bin", "yaac")}" hub`));
});

test("startHub false places the files on close without invoking the hub or configuring autostart", linuxTest, async (t) => {
  const { client, data, settings } = await fixture(t, { wslDistro: "YAAC Test", systemd: true });
  const opId = "place";
  const binary = Buffer.from('#!/bin/sh\nprintf invoked > "$YAAC_DATA_DIR/marker"\n');
  const installed = path.join(data, "bin", "yaac");
  const resultFile = path.join(data, `transfer-${opId}.result`);
  await put(client, opId, "bin/yaac", binary);
  await put(client, opId, "config/hub.json", config);
  assert.deepEqual(await client.request("hubHost/start", { opId, armOnClose: true, startHub: false }), { armed: true });
  await assert.rejects(client.request("hubHost/start", { opId, armOnClose: true }), /in use/);
  assert.equal(fs.existsSync(installed), false);
  assert.equal(fs.existsSync(resultFile), false);
  await client.close();

  await until(() => fs.existsSync(resultFile), "the placement result");
  assert.deepEqual(JSON.parse(fs.readFileSync(resultFile, "utf8")), { opId, status: "placed" });
  assert.deepEqual(fs.readFileSync(installed), binary);
  fs.accessSync(installed, fs.constants.X_OK);
  assert.deepEqual(fs.readFileSync(path.join(settings, "hub.json")), config);
  for (const file of ["marker", "systemd-check", "hub.log"]) assert.equal(fs.existsSync(path.join(data, file)), false, file);
  assert.equal(fs.existsSync(path.join(settings, "..", "systemd")), false);
  assert.equal(fs.existsSync(path.join(settings, "..", "autostart")), false);
});
