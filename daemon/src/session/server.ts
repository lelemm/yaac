// Local IPC and the start-up lock: at most one serving yaacd per user
// (docs/02-architecture.md#one-serving-daemon-per-user).

import * as fs from "node:fs";
import * as net from "node:net";
import * as os from "node:os";
import * as path from "node:path";
import * as tls from "node:tls";
import { randomBytes } from "node:crypto";
import { sha256Hex, writeAtomic } from "../util/fsx.js";
import { BRIDGE_MAGIC } from "../mesh/link.js";
import { ensurePrivateDir } from "./uploads.js";

/** Unix socket paths are limited to about 104 bytes on some systems. */
const MAX_SOCKET_PATH = 100;
const PIPE_PREFIX = "\\\\.\\pipe\\";
const PSK_ID = "yaacd";
const PSK_TLS = { minVersion: "TLSv1.2", maxVersion: "TLSv1.2", ciphers: "PSK-AES256-GCM-SHA384" } as const;

/** Distinguish the framed host protocol from the MCP bridge. */
export function sniffConnection(socket: net.Socket): Promise<"host" | "bridge" | null> {
  return new Promise((resolve) => {
    const done = (kind: "host" | "bridge" | null) => {
      socket.off("readable", onReadable);
      socket.off("close", onClose);
      socket.off("end", onClose);
      resolve(kind);
    };
    const onReadable = () => {
      const head = socket.read(4) as Buffer | null;
      if (head === null) return;
      if (head.length < 4) return done(null);
      if (head.equals(BRIDGE_MAGIC)) return done("bridge");
      socket.unshift(head);
      done("host");
    };
    const onClose = () => done(null);
    socket.on("readable", onReadable);
    socket.once("close", onClose);
    socket.once("end", onClose);
    if (socket.destroyed || socket.readableEnded) onClose();
  });
}

/** Where the daemon of `home` listens: a Windows pipe or a private Unix socket. */
export function socketPathFor(home: string, env: NodeJS.ProcessEnv = process.env): string {
  if (process.platform === "win32") {
    if (env.YAAC_SOCKET?.startsWith(PIPE_PREFIX)) return env.YAAC_SOCKET;
    return `${PIPE_PREFIX}yaacd-${sha256Hex(path.resolve(home).toLowerCase()).slice(0, 32)}`;
  }
  if (env.YAAC_SOCKET) return path.resolve(env.YAAC_SOCKET);
  const preferred = path.join(home, "run", "yaacd.sock");
  if (Buffer.byteLength(preferred) <= MAX_SOCKET_PATH) return preferred;
  const uid = typeof process.getuid === "function" ? process.getuid() : os.userInfo().username;
  return path.join(os.tmpdir(), `yaac-run-${uid}`, `${sha256Hex(path.resolve(home)).slice(0, 16)}.sock`);
}

/** Stable integrator endpoint. The Windows name belongs to the OS account, independent of YAAC_HOME. */
export function channelPathFor(home: string, platform = process.platform, username?: string): string {
  if (platform === "win32") return `${PIPE_PREFIX}yaac-channel-${username ?? os.userInfo().username}`;
  return path.join(home, "run", "channel.sock");
}

function pipeKeyFile(socket: string): string {
  // Use the OS account's profile, even when YAAC_HOME was placed on a shared drive.
  return path.join(os.userInfo().homedir, ".yaac", "pipe-keys", sha256Hex(socket.toLowerCase()));
}

function pipeKey(socket: string, create: boolean): Buffer | undefined {
  const file = pipeKeyFile(socket);
  try {
    const key = fs.readFileSync(file);
    if (key.length !== 32) throw new Error(`invalid yaacd pipe key at ${file}`);
    return key;
  } catch (e) {
    if ((e as NodeJS.ErrnoException).code !== "ENOENT") throw e;
    if (!create) return undefined;
    const key = randomBytes(32);
    writeAtomic(file, key);
    return key;
  }
}

/** Release this endpoint's credential after its owner has finished writing state. */
export function removePipeKey(socket: string): void {
  fs.rmSync(pipeKeyFile(socket), { force: true });
}

/** The Windows pipe authenticates both ends with a key in the user's profile. */
export function connectDaemonSocket(socket: string, timeoutMs = 1000): Promise<net.Socket | null> {
  const windows = process.platform === "win32";
  const key = windows ? pipeKey(socket, false) : undefined;
  if (windows && !key) return Promise.resolve(null);
  const s = key
    ? tls.connect({ ...PSK_TLS, path: socket, rejectUnauthorized: false, pskCallback: () => ({ identity: PSK_ID, psk: key }) })
    : net.connect(socket);
  return waitForConnection(s, key ? "secureConnect" : "connect", timeoutMs);
}

export function connectChannelSocket(socket: string, timeoutMs = 1000): Promise<net.Socket | null> {
  return waitForConnection(net.connect(socket), "connect", timeoutMs);
}

function waitForConnection(s: net.Socket, ready: "connect" | "secureConnect", timeoutMs: number): Promise<net.Socket | null> {
  return new Promise((resolve) => {
    const done = (ok: boolean) => {
      clearTimeout(t);
      s.removeListener(ready, connected);
      s.removeListener("error", failed);
      if (!ok) {
        s.on("error", () => {});
        s.destroy();
      }
      resolve(ok ? s : null);
    };
    const connected = () => done(true);
    const failed = () => done(false);
    const t = setTimeout(() => done(false), timeoutMs);
    s.once(ready, connected);
    s.once("error", failed);
  });
}

async function isServing(socket: string): Promise<boolean> {
  const s = await connectDaemonSocket(socket);
  s?.destroy();
  return s !== null;
}

export class AlreadyServing extends Error {}

/** How long a starting daemon waits for the previous one (named by the pid file) to exit. */
export const PREVIOUS_EXIT_WAIT_MS = 8000;

function alive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch (e) {
    return (e as NodeJS.ErrnoException).code === "EPERM";
  }
}

/** Waits until the process in `<runDir>/yaacd.pid` (if any, and not this one) has exited. */
async function waitForPrevious(runDir: string, maxMs = PREVIOUS_EXIT_WAIT_MS): Promise<void> {
  let pid: number;
  try {
    pid = Number(fs.readFileSync(path.join(runDir, "yaacd.pid"), "utf8").trim());
  } catch {
    return;
  }
  if (!Number.isInteger(pid) || pid <= 0 || pid === process.pid) return;
  const deadline = Date.now() + maxMs;
  while (alive(pid) && Date.now() < deadline) await new Promise((r) => setTimeout(r, 50));
}

async function withStartupLock<T>(runDir: string, fn: () => Promise<T>): Promise<T> {
  const lock = path.join(runDir, "startup.lock");
  const deadline = Date.now() + 10000;
  for (;;) {
    try {
      fs.mkdirSync(lock, { mode: 0o700 });
      break;
    } catch (e) {
      if ((e as NodeJS.ErrnoException).code !== "EEXIST") throw e;
      try {
        // A lock left by a process that died during start-up.
        if (Date.now() - fs.statSync(lock).mtimeMs > 15000) fs.rmSync(lock, { recursive: true, force: true });
      } catch {
        /* raced with its owner */
      }
      if (Date.now() > deadline) throw new Error(`could not take the start-up lock ${lock}`);
      await new Promise((r) => setTimeout(r, 50));
    }
  }
  try {
    return await fn();
  } finally {
    fs.rmSync(lock, { recursive: true, force: true });
  }
}

/**
 * Binds the socket exclusively: under the start-up lock, a live socket means another daemon
 * serves (AlreadyServing); a dead one is a leftover and is replaced.
 */
export async function listenExclusive(socket: string, runDir: string, onConnection: (s: net.Socket) => void): Promise<net.Server> {
  const windows = process.platform === "win32";
  ensurePrivateDir(path.dirname(runDir));
  ensurePrivateDir(runDir);
  if (!windows) ensurePrivateDir(path.dirname(socket));
  return withStartupLock(runDir, async () => {
    if ((windows || fs.existsSync(socket)) && await isServing(socket)) throw new AlreadyServing(`a yaacd is already serving on ${socket}`);
    // A daemon that stopped listening may still be finishing (a hand-over closes its socket
    // first, then flushes journals and marks the state clean): it owns the state until it
    // exits, so wait for it — bounded, in case the pid file names some other process now.
    await waitForPrevious(runDir);
    if (!windows && fs.existsSync(socket)) {
      if (await isServing(socket)) throw new AlreadyServing(`a yaacd is already serving on ${socket}`);
      fs.rmSync(socket, { force: true });
    }
    const key = windows ? pipeKey(socket, true) : undefined;
    const server = key
      ? tls.createServer({ ...PSK_TLS, handshakeTimeout: 1000, pskCallback: (_socket, identity) => identity === PSK_ID ? key : null }, onConnection)
      : net.createServer({ allowHalfOpen: false }, onConnection);
    await bindPrivate(server, socket);
    if (!windows) fs.chmodSync(socket, 0o600);
    fs.writeFileSync(path.join(runDir, "yaacd.pid"), `${process.pid}\n`, { mode: 0o600 });
    return server;
  });
}

async function bindPrivate(server: net.Server, address: string): Promise<void> {
  await new Promise<void>((resolve, reject) => {
    server.once("error", reject);
    const old = process.umask(0o177);
    try {
      server.listen(address, () => {
        server.removeListener("error", reject);
        resolve();
      });
    } finally {
      process.umask(old);
    }
  });
}

/** Bind the fixed channel endpoint without touching one that is owned by a running server. */
export async function listenChannels(endpoint: string, onConnection: (s: net.Socket) => void): Promise<net.Server> {
  const listen = async () => {
    const server = net.createServer({ allowHalfOpen: false }, onConnection);
    await bindPrivate(server, endpoint);
    return server;
  };

  // libuv binds named pipes with FILE_FLAG_FIRST_PIPE_INSTANCE. Windows refuses
  // the bind when another server already owns the name, including in another home.
  if (process.platform === "win32") return listen();
  if (Buffer.byteLength(endpoint) > MAX_SOCKET_PATH) throw new Error("channel socket path is too long");
  const runDir = path.dirname(endpoint);
  ensurePrivateDir(path.dirname(runDir));
  ensurePrivateDir(runDir);
  return withStartupLock(runDir, async () => {
    const stale = fs.lstatSync(endpoint, { throwIfNoEntry: false });
    if (stale) {
      if (!stale.isSocket()) throw new Error("the channel socket path is occupied by another file");
      // Only a positive refusal lets us remove a Unix socket left behind by a crashed daemon.
      const code = await new Promise<string | undefined>((resolve) => {
        const probe = net.connect(endpoint);
        const timer = setTimeout(() => {
          probe.destroy();
          resolve("ETIMEDOUT");
        }, 1000);
        probe.once("connect", () => {
          clearTimeout(timer);
          probe.destroy();
          resolve(undefined);
        });
        probe.once("error", (error: NodeJS.ErrnoException) => {
          clearTimeout(timer);
          resolve(error.code);
        });
      });
      if (code !== "ECONNREFUSED" && code !== "ENOENT") throw new AlreadyServing("the channel endpoint is already in use");
      const current = fs.lstatSync(endpoint, { throwIfNoEntry: false });
      if (current && (current.dev !== stale.dev || current.ino !== stale.ino)) {
        throw new AlreadyServing("the channel endpoint changed during startup");
      }
      if (current) fs.unlinkSync(endpoint);
    }
    return listen();
  });
}
