// Exercise the staged daemon and runtime on the same host that packages the installer.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { setTimeout as delay } from "node:timers/promises";

const bundle = path.resolve(process.argv[2]);
const home = mkdtempSync(path.join(os.tmpdir(), "yaac-native-"));
const pidFile = path.join(home, "run", "yaacd.pid");
const env = {
  ...process.env,
  YAAC_HOME: home,
  YAAC_SOCKET: "",
  YAAC_CODEX_PROVIDER: "0",
  YAAC_CLAUDE_PROVIDER: "0",
  YAAC_FAKE_PROVIDER: "1",
  YAAC_IDLE_TIMEOUT_MS: "3000",
};

async function probe(keepServing) {
  const bridge = spawn(process.execPath, [bundle, "connect"], { env, windowsHide: true, stdio: ["pipe", "pipe", "inherit"] });
  const exited = once(bridge, "exit");
  const timeout = setTimeout(() => bridge.kill(), 20_000);
  let buffered = Buffer.alloc(0);
  const frames = bridge.stdout[Symbol.asyncIterator]();

  let terminalOutput = "";
  async function request(id, method, params = {}) {
    const body = Buffer.from(JSON.stringify({ jsonrpc: "2.0", id, method, params }));
    const header = Buffer.alloc(5);
    header.writeUInt32LE(body.length + 1);
    header[4] = 1;
    bridge.stdin.write(Buffer.concat([header, body]));
    for (;;) {
      const length = buffered.length >= 4 ? buffered.readUInt32LE() : 0;
      if (length > 0 && buffered.length >= length + 4) {
        if (buffered[4] === 2) {
          terminalOutput += buffered.subarray(9, length + 4).toString();
          buffered = buffered.subarray(length + 4);
          continue;
        }
        const response = JSON.parse(buffered.subarray(5, length + 4));
        buffered = buffered.subarray(length + 4);
        if (response.id !== id) continue;
        assert.ok(response.result, JSON.stringify(response));
        return response.result;
      }
      const next = await frames.next();
      assert.equal(next.done, false, `bridge closed before ${method}`);
      buffered = Buffer.concat([buffered, next.value]);
    }
  }

  try {
    const init = await request(1, "initialize", {
      protocol: { min: 1, max: 1 },
      client: { name: "installer-smoke", version: "0", id: "smoke", host: "test" },
    });
    assert.equal(init.target.os, "windows");
    const { providers } = await request(2, "provider/list");
    assert.equal(providers.find((p) => p.kind === "fake")?.status, "ready");
    if (keepServing) {
      assert.ok(init.features.includes("terminal"), init.terminalUnavailableReason);
      const { shells } = await request(4, "terminal/shells");
      assert.match(shells[0].path, /(?:pwsh|powershell)\.exe$/i);
      const { terminal, stream } = await request(5, "terminal/open", { cwd: home, cols: 100, rows: 30 });
      const grant = Buffer.from(JSON.stringify({ method: "stream/credit", params: { stream, bytes: 1 << 20 } }));
      const grantHeader = Buffer.alloc(5);
      grantHeader.writeUInt32LE(grant.length + 1);
      grantHeader[4] = 1;
      bridge.stdin.write(Buffer.concat([grantHeader, grant]));
      const command = Buffer.from("Write-Output ('PTY' + [Console]::OutputEncoding.CodePage + 'OK')\r");
      const header = Buffer.alloc(9);
      header.writeUInt32LE(command.length + 5);
      header[4] = 2;
      header.writeUInt32LE(stream, 5);
      bridge.stdin.write(Buffer.concat([header, command]));
      for (let attempt = 0; attempt < 100 && !terminalOutput.includes("PTY65001OK"); attempt++) {
        await delay(100);
        await request(6, "ping");
      }
      assert.ok(terminalOutput.includes("PTY65001OK"), terminalOutput);
      await request(7, "terminal/close", { terminal });
      console.log("ConPTY round trip and UTF-8 code page verified with " + shells[0].path);
    }
    const pid = readFileSync(pidFile, "utf8");
    if (!keepServing) await request(3, "daemon/handover", { when: "now" });
    bridge.stdin.end();
    assert.equal((await exited)[0], 0);
    return pid;
  } finally {
    clearTimeout(timeout);
    if (bridge.exitCode === null && bridge.signalCode === null) bridge.kill();
  }
}

async function waitForShutdown() {
  for (let i = 0; i < 100 && existsSync(pidFile); i++) await delay(100);
  assert.equal(existsSync(pidFile), false, "the daemon released its endpoint");
}

try {
  const first = await probe(true);
  assert.equal(await probe(false), first, "a later bridge attached to the same daemon");
  await waitForShutdown();
  await probe(false);
  await waitForShutdown();
  console.log("Native Windows daemon connected, listed providers, and reattached.");
} finally {
  if (!existsSync(pidFile)) rmSync(home, { recursive: true, force: true });
}
