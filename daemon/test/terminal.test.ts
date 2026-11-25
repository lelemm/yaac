import assert from "node:assert/strict";
import { copyFile, rm } from "node:fs/promises";
import { test } from "node:test";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { join } from "node:path";
import { FrameDecoder } from "../src/protocol/frame.js";
import { shells, shellArgs } from "../src/session/terminal-shells.js";
import { TerminalOutput } from "../src/session/terminals.js";
import { client, tmpHome, startDaemon, until, sleep, TestClient } from "./helpers.js";

function bytes(peer: TestClient, stream: number) {
  const frames = new FrameDecoder().decode(Buffer.concat(peer.rawIn));
  return Buffer.concat(frames.flatMap(f => f.kind === "data" && f.stream === stream ? [f.bytes] : []));
}

test("Unix and Windows shell discovery and invocation", () => {
  const unix = { platform: "darwin" as const, env: { PATH: "/bin", SHELL: "/bin/zsh" },
    login: () => "/bin/fish", listed: () => ["/bin/ksh", "/missing"], executable: (p: string) => p.startsWith("/bin/") };
  assert.deepEqual(shells(unix).map(s => s.name), ["fish", "zsh", "bash", "sh", "ksh"]);
  assert.equal(shells({ ...unix, login: () => "/missing" })[0]?.name, "zsh");
  assert.equal(shells({ ...unix, login: () => null, env: { PATH: "/bin" } })[0]?.name, "bash");
  assert.deepEqual(shellArgs("/bin/fish", "darwin"), ["-l", "-i"]);
  const windows = { ...unix, platform: "win32" as const, env: { PATH: "C:\\bin", ProgramFiles: "C:\\Programs" }, executable: (p: string) => !p.endsWith("git.exe") };
  assert.deepEqual(shells(windows).map(s => s.name), ["PowerShell 7", "Windows PowerShell", "Command Prompt", "Git Bash"]);
  assert.equal(shells({ ...windows, executable: p => !p.endsWith("pwsh.exe") })[0]?.name, "Windows PowerShell");
  assert.ok(shellArgs("pwsh.exe", "win32").includes("-NoLogo"));
});

test("output coalesces fragmented writes until the timer or 16 KiB", t => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const frames: Buffer[] = [];
  const batch = new TerminalOutput(b => frames.push(b));
  batch.append(Buffer.from("ab"));
  batch.append(Buffer.from("cd"));
  assert.equal(frames.length, 0);
  t.mock.timers.tick(4);
  assert.deepEqual(frames, [Buffer.from("abcd")]);
  batch.append(Buffer.alloc(16387));
  assert.equal(frames[1]?.length, 16384);
  t.mock.timers.tick(4);
  assert.equal(frames[2]?.length, 3);
});

test("PTY echo, environment, resize, OSC title, reconnect, replay and exit", { skip: process.platform !== "linux" }, async t => {
  const home = tmpHome();
  const daemon = await startDaemon(home, { idleTimeoutMs: 250 });
  let peer = await client(daemon);
  t.after(async () => { peer.socket.destroy(); await daemon.stop("test"); await rm(home, { recursive: true, force: true }); });
  const size = { cols: 80, rows: 24 };
  const { stream, terminal, initialCredit } = await peer.request("terminal/open", { cwd: home, shell: "/bin/sh", ...size, threadId: "t_example" });
  peer.notify("stream/credit", { stream, bytes: 1 << 20 });
  assert.equal(initialCredit, 1 << 20);
  await peer.request("terminal/resize", { terminal, cols: 95, rows: 41 });
  peer.sendData(stream, Buffer.from("printf '\\033]2;screen-title\\007'; stty size; printf '%s/%s/%s/%s\\n' \"$TERM\" \"$COLORTERM\" \"$TERM_PROGRAM\" \"$PWD\"\n"));
  await until(() => bytes(peer, stream).includes("xterm-256color/truecolor/YAAC/" + home));
  assert.ok(bytes(peer, stream).includes("41 95"));
  assert.ok(peer.notifications.some(n => n.method === "terminal/title" && n.params.title === "screen-title"));
  peer.socket.destroy();
  await sleep(350); // The detached PTY holds the daemon past the idle exit.
  peer = await client(daemon);
  assert.equal((await peer.request("terminal/list")).terminals[0].threadId, "t_example");
  const attached = await peer.request("terminal/attach", { terminal, ...size });
  // The user's name (P343) reaches attached connections and the list; empty clears it.
  await peer.request("terminal/rename", { terminal, name: "  build watch " });
  await until(() => peer.notifications.find(n => n.method === "terminal/renamed" && n.params.name === "build watch"));
  assert.equal((await peer.request("terminal/list")).terminals[0].name, "build watch");
  await peer.request("terminal/rename", { terminal, name: "" });
  await until(() => peer.notifications.find(n => n.method === "terminal/renamed" && n.params.name === undefined));
  assert.equal((await peer.request("terminal/list")).terminals[0].name, undefined);
  await assert.rejects(peer.request("terminal/rename", { terminal: "nope", name: "x" }));
  peer.notify("stream/credit", { stream: attached.stream, bytes: 1 << 20 });
  assert.notEqual(attached.stream, stream);
  await until(() => bytes(peer, attached.stream).includes("41 95"));
  peer.sendData(attached.stream, Buffer.from("exit 7\n"));
  await until(() => peer.notifications.find(n => n.method === "terminal/exited" && n.params.code === 7));
  assert.deepEqual(await peer.request("terminal/list"), { terminals: [] });
});

test("uncredited PTY output stops at 1 MiB, resumes on credit, replay retains 2 MiB", { skip: process.platform !== "linux" }, async t => {
  const home = tmpHome();
  const daemon = await startDaemon(home);
  const peer = await client(daemon);
  t.after(async () => { peer.socket.destroy(); await daemon.stop("test"); await rm(home, { recursive: true, force: true }); });
  const size = { cols: 80, rows: 24 };
  const { terminal, stream } = await peer.request("terminal/open", { cwd: home, shell: "/bin/sh", ...size });
  peer.sendData(stream, Buffer.from("head -c 2400000 /dev/zero | tr '\\000' Z; printf '%s%s' END MARK\n"));
  await sleep(20);
  assert.equal(bytes(peer, stream).length, 0);
  peer.notify("stream/credit", { stream, bytes: 1 << 20 });
  await until(() => bytes(peer, stream).length === 1 << 20);
  await sleep(20);
  assert.equal(bytes(peer, stream).length, 1 << 20);
  peer.notify("stream/credit", { stream, bytes: 2400000 });
  await until(() => bytes(peer, stream).includes("ENDMARK"));
  const replay = await peer.request("terminal/attach", { terminal, ...size });
  peer.notify("stream/credit", { stream: replay.stream, bytes: 2 << 20 });
  await until(() => bytes(peer, replay.stream).length === 2 << 20);
  assert.deepEqual(bytes(peer, replay.stream), bytes(peer, stream).subarray(-(2 << 20)));
  await peer.request("terminal/close", { terminal });
  await until(() => peer.notifications.some(n => n.method === "terminal/exited"));
});

test("isolated bundle without its native runtime reports a reason at initialize", { skip: process.platform !== "linux" }, async t => {
  const home = tmpHome();
  const bundle = join(home, "yaacd.mjs");
  await copyFile(new URL("../../yaacd.mjs", import.meta.url), bundle);
  const socket = join(home, "test.sock");
  const proc = spawn(process.execPath, ["--no-turbofan", bundle, "serve"], { env: {
    ...process.env, YAAC_HOME: home, YAAC_SOCKET: socket, YAAC_CODEX_PROVIDER: "0", YAAC_CLAUDE_PROVIDER: "0", YAAC_FAKE_PROVIDER: "1",
  }, stdio: "ignore" });
  const exited = once(proc, "exit");
  t.after(async () => { proc.kill(); await exited; await rm(home, { recursive: true, force: true }); });
  const peer = await until(() => TestClient.connect(socket).catch(() => null), "test socket");
  try {
    const hello = await peer.initialize();
    assert.ok(!hello.features.includes("terminal"));
    assert.match(hello.terminalUnavailableReason, /pty-/);
    assert.deepEqual(await peer.request("ping"), {});
  } finally { peer.socket.destroy(); }
});

test("two attachments get independent credit and one exit after the slower stream drains", { skip: process.platform !== "linux" }, async t => {
  const home = tmpHome();
  const daemon = await startDaemon(home);
  const peer = await client(daemon);
  t.after(async () => { peer.socket.destroy(); await daemon.stop("test"); await rm(home, { recursive: true, force: true }); });
  const size = { cols: 80, rows: 24 };
  const first = await peer.request("terminal/open", { cwd: home, shell: "/bin/sh", ...size });
  const other = await peer.request("terminal/attach", { terminal: first.terminal, ...size });
  peer.notify("stream/credit", { stream: first.stream, bytes: 1 << 20 });
  peer.sendData(first.stream, Buffer.from("printf '\\033]0;spl'; sleep 0.02; printf 'it\\033\\134'; printf '%s%s' LAST BYTE; exit 3\n"));
  await until(() => bytes(peer, first.stream).includes("LASTBYTE"));
  assert.equal(bytes(peer, other.stream).length, 0);
  assert.equal(peer.notifications.filter(n => n.method === "terminal/exited").length, 0);
  assert.equal(peer.notifications.filter(n => n.method === "terminal/title" && n.params.title === "split").length, 1);
  peer.notify("stream/credit", { stream: other.stream, bytes: 1 << 20 });
  await until(() => peer.notifications.some(n => n.method === "terminal/exited"));
  assert.ok(bytes(peer, other.stream).includes("LASTBYTE"));
  assert.deepEqual(peer.notifications.filter(n => n.method === "terminal/exited").map(n => n.params.code), [3]);
});
