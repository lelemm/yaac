// Files staged by a hub transfer are private and promoted only after hand-over.

import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { createHash } from "node:crypto";
import { RpcError } from "../protocol/rpc.js";
import { ensureDir, randomId } from "../util/fsx.js";
import { ensurePrivateDir } from "./uploads.js";

export const HUB_BINARY = "bin/yaac";
export const HUB_CONFIG = "config/hub.json";
const DATABASES = ["data/cache.sqlite", "data/relay.sqlite", "data/drafts.sqlite"];
const FILE_COMPONENT = "(?!\\.{1,2}(?:/|$))[A-Za-z0-9._@-]{1,255}";
const KEY_FILE = new RegExp(`^data/(?:keys/${FILE_COMPONENT}|ssh/keys/${FILE_COMPONENT}/id_ed25519(?:\\.pub)?)(?![\\s\\S])`);
const FIXED_FILES = [HUB_BINARY, HUB_CONFIG, ...DATABASES, "data/ssh/known_hosts"];

export function validateTransferId(opId: string): void {
  if (typeof opId !== "string" || !opId || opId.length > 128 || /[^A-Za-z0-9_-]/.test(opId)) {
    throw new RpcError("invalid_params", "invalid hub transfer opId");
  }
}

export function validateTransferFile(name: string, size: number, sha256: string): void {
  if (typeof name !== "string" || !(FIXED_FILES.includes(name) || KEY_FILE.test(name))) {
    throw new RpcError("invalid_params", "invalid hub transfer file name");
  }
  if (!Number.isSafeInteger(size) || size < 0 || typeof sha256 !== "string" || sha256.length !== 64 || /[^0-9a-f]/.test(sha256)) {
    throw new RpcError("invalid_params", "hub transfer requires a non-negative size and a lowercase SHA-256");
  }
}

export function hubPaths(env: NodeJS.ProcessEnv) {
  // Match the Linux defaults and profile overrides in yaac-hub's paths.rs.
  const home = env.HOME || os.homedir();
  const configHome = path.resolve(env.XDG_CONFIG_HOME || path.join(home, ".config"));
  const config = path.resolve(env.YAAC_CONFIG_DIR || path.join(configHome, "yaac"));
  const data = path.resolve(env.YAAC_DATA_DIR || path.join(env.XDG_DATA_HOME || path.join(home, ".local", "share"), "yaac"));
  const runtime = path.resolve(env.YAAC_RUNTIME_DIR || (!env.YAAC_DATA_DIR && env.XDG_RUNTIME_DIR
    ? path.join(env.XDG_RUNTIME_DIR, "yaac") : path.join(data, "run")));
  const autostart = path.resolve(env.YAAC_AUTOSTART_DIR || path.join(configHome, "autostart"), "yaac-hub.desktop");
  return { home, configHome, config, data, runtime, autostart };
}

export interface HubProfilePaths {
  config: string;
  data: string;
  runtime: string;
}

/** Create private subdirectories without following links below the profile roots. */
export function prepareParent(root: string, file: string): void {
  ensureDir(path.dirname(root));
  ensurePrivateDir(root);
  let current = root;
  for (const component of path.relative(root, path.dirname(file)).split(path.sep).filter(Boolean)) {
    current = path.join(current, component);
    ensurePrivateDir(current);
  }
}

export class StagedFile {
  readonly path: string;
  private readonly temp: string;
  private fd: number | null;
  private readonly hash = createHash("sha256");
  received = 0;

  constructor(root: string, readonly name: string, readonly size: number, private readonly sha256: string) {
    this.path = path.join(root, name);
    this.temp = path.join(root, randomId(".partial-"));
    prepareParent(root, this.path);
    this.fd = fs.openSync(this.temp, "wx", 0o600);
  }

  write(bytes: Buffer): void {
    if (this.fd === null) throw new Error("hub transfer was cancelled");
    if (this.received + bytes.length > this.size) throw new Error("received more than the declared file size");
    let offset = 0;
    while (offset < bytes.length) offset += fs.writeSync(this.fd, bytes, offset, bytes.length - offset);
    this.hash.update(bytes);
    this.received += bytes.length;
  }

  finish(): void {
    if (this.fd === null) throw new Error("hub transfer was cancelled");
    if (this.received !== this.size) throw new Error(`received ${this.received} bytes, expected ${this.size}`);
    if (this.hash.digest("hex") !== this.sha256) throw new Error("the file does not match the declared SHA-256");
    if (this.name === HUB_BINARY) fs.fchmodSync(this.fd, 0o700);
    fs.fsyncSync(this.fd);
    this.close();
    fs.renameSync(this.temp, this.path);
  }

  discard(): void {
    try { this.close(); } finally { fs.rmSync(this.temp, { force: true }); }
  }

  private close(): void {
    const fd = this.fd;
    this.fd = null;
    if (fd !== null) fs.closeSync(fd);
  }
}

function isPresent(file: string): boolean {
  return fs.lstatSync(file, { throwIfNoEntry: false }) !== undefined;
}

/** The config directory can be on a different volume from the staging directory. */
function moveFile(from: string, to: string): void {
  try {
    fs.renameSync(from, to);
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "EXDEV") throw error;
    const temp = path.join(path.dirname(to), randomId(".transfer-"));
    try {
      fs.copyFileSync(from, temp, fs.constants.COPYFILE_EXCL);
      fs.renameSync(temp, to);
      try {
        fs.unlinkSync(from);
      } catch (error) {
        fs.unlinkSync(to);
        throw error;
      }
    } finally {
      fs.rmSync(temp, { force: true });
    }
  }
}

class Promotion {
  private readonly moves: Array<{ from: string; to: string }> = [];
  constructor(private readonly root: string) {}

  move(from: string, to: string): void {
    moveFile(from, to);
    this.moves.push({ from, to });
  }

  backup(file: string, name: string): void {
    const stat = fs.lstatSync(file, { throwIfNoEntry: false });
    if (!stat) return;
    if (!stat.isFile()) throw new Error(`cannot replace non-regular file ${file}`);
    const backup = path.join(this.root, "replaced", name);
    prepareParent(this.root, backup);
    if (isPresent(backup)) throw new Error(`backup already exists at ${backup}`);
    this.move(file, backup);
  }

  rollback(): string[] {
    const errors: string[] = [];
    for (const { from, to } of this.moves.reverse()) {
      try { moveFile(to, from); } catch (error) { errors.push((error as Error).message); }
    }
    return errors;
  }
}

export function promoteHubFiles(
  root: string,
  names: Iterable<string>,
  paths: HubProfilePaths,
  options: { windows?: boolean; service?: { file: string; content: string; autostart: string } } = {},
): void {
  const promotion = new Promotion(root);
  const { service, windows } = options;
  try {
    for (const name of names) {
      const destRoot = name === HUB_CONFIG ? paths.config : paths.data;
      const dest = path.join(destRoot, name.replace(/^(config|data)\//, ""));
      prepareParent(destRoot, dest);
      promotion.backup(dest, name);
      if (DATABASES.includes(name)) {
        for (const suffix of ["-wal", "-shm", "-journal"]) promotion.backup(dest + suffix, name + suffix);
      }
      promotion.move(path.join(root, name), dest);
    }
    // A machine receiving the hub back may still have its own outbound tombstone.
    const tombstone = path.join(paths.data, "moved.json");
    if (!windows) promotion.backup(tombstone, "data/moved.json");
    if (service) {
      ensureDir(path.dirname(service.file));
      const unit = path.join(root, "yaac-hub.service");
      fs.writeFileSync(unit, service.content, { flag: "wx", mode: 0o600 });
      promotion.backup(service.file, "systemd/yaac-hub.service");
      promotion.backup(service.autostart, "autostart/yaac-hub.desktop");
      promotion.move(unit, service.file);
    }
    if (windows) fs.rmSync(tombstone, { force: true });
  } catch (error) {
    const rollbackErrors = promotion.rollback();
    if (rollbackErrors.length) throw new Error(`${(error as Error).message}. Rollback: ${rollbackErrors.join(", ")}`);
    throw error;
  }
}
