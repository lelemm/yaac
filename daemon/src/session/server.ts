// The user-only Unix socket and the start-up lock: at most one serving yaacd per user
// (docs/02-architecture.md#one-serving-daemon-per-user).

import * as fs from "node:fs";
import * as net from "node:net";
import * as os from "node:os";
import * as path from "node:path";
import { sha256Hex } from "../util/fsx.js";
import { ensurePrivateDir } from "./uploads.js";

/** Unix socket paths are limited to about 104 bytes on some systems. */
const MAX_SOCKET_PATH = 100;

/** Where the daemon of `home` listens: `<home>/run/yaacd.sock`, or a short private fallback. */
export function socketPathFor(home: string, env: NodeJS.ProcessEnv = process.env): string {
  if (env.YAAC_SOCKET) return path.resolve(env.YAAC_SOCKET);
  const preferred = path.join(home, "run", "yaacd.sock");
  if (Buffer.byteLength(preferred) <= MAX_SOCKET_PATH) return preferred;
  const uid = typeof process.getuid === "function" ? process.getuid() : os.userInfo().username;
  return path.join(os.tmpdir(), `yaac-run-${uid}`, `${sha256Hex(path.resolve(home)).slice(0, 16)}.sock`);
}

function tryConnect(socket: string, timeoutMs = 1000): Promise<boolean> {
  return new Promise((resolve) => {
    const s = net.connect(socket);
    const done = (ok: boolean) => {
      clearTimeout(t);
      s.removeAllListeners();
      s.on("error", () => {});
      s.destroy();
      resolve(ok);
    };
    const t = setTimeout(() => done(false), timeoutMs);
    s.once("connect", () => done(true));
    s.once("error", () => done(false));
  });
}

export class AlreadyServing extends Error {}

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
  ensurePrivateDir(path.dirname(runDir));
  ensurePrivateDir(runDir);
  ensurePrivateDir(path.dirname(socket));
  return withStartupLock(runDir, async () => {
    if (fs.existsSync(socket)) {
      if (await tryConnect(socket)) throw new AlreadyServing(`a yaacd is already serving on ${socket}`);
      fs.rmSync(socket, { force: true });
    }
    const server = net.createServer({ allowHalfOpen: false }, onConnection);
    await new Promise<void>((resolve, reject) => {
      server.once("error", reject);
      const old = process.umask(0o177);
      try {
        server.listen(socket, () => {
          server.removeListener("error", reject);
          resolve();
        });
      } finally {
        process.umask(old);
      }
    });
    fs.chmodSync(socket, 0o600);
    fs.writeFileSync(path.join(runDir, "yaacd.pid"), `${process.pid}\n`, { mode: 0o600 });
    return server;
  });
}

export { tryConnect };
