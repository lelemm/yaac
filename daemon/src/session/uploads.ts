// The upload store (docs/06-attachments.md#the-upload-folder).
//
//   <temp>/yaac-<user>/uploads/
//     blobs/<full-sha256>                     the bytes, stored once
//     meta/<uploadId>.json                    name, media type, size, digest
//     threads/<thread>/<n>-<sanitised-name>   a link to the blob, under the name the agent sees
//
// The folder is private even though temp is shared: yaacd creates it user-only and refuses
// one it does not own, that others can read, or that is a link. Bytes arrive on a data
// stream under credit; credit is returned only once bytes are on disk.

import { artifactFormat } from "./artifact-format.js";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { createHash, type Hash } from "node:crypto";
import type { UploadBeginParams, UploadBeginResult, UploadEndResult, UploadReadResult } from "../protocol/generated/types.js";
import { RpcError } from "../protocol/rpc.js";
import { ReceiveWindow } from "../protocol/scheduler.js";
import { randomId, readJson, sha256Hex, writeJsonAtomic } from "../util/fsx.js";

export const UPLOAD_WINDOW = 256 * 1024;
export const UPLOAD_MAX_FILE = 512 * 1024 * 1024;
export const UPLOAD_MAX_CONCURRENT = 8;
/** Bytes declared by one connection's unfinished uploads, together. */
export const UPLOAD_MAX_IN_PROGRESS = 2 * 1024 * 1024 * 1024;
export const UPLOAD_READ_MAX = 512 * 1024;

export interface UploadMeta {
  uploadId: string;
  sha256: string;
  size: number;
  name: string;
  mediaType: string | null;
}

interface Partial {
  meta: UploadMeta;
  stream: number;
  file: string;
  fd: number;
  received: number;
  hash: Hash;
  window: ReceiveWindow;
  owner: object;
}

export function sanitiseName(name: string): string {
  const base = path.basename(String(name ?? "").replace(/\\/g, "/"));
  let clean = base.replace(/[^A-Za-z0-9._-]+/g, "_").replace(/^[.]+/, "");
  if (clean.length > 100) {
    const ext = path.extname(clean).slice(0, 12);
    clean = clean.slice(0, 100 - ext.length) + ext;
  }
  return clean || "file";
}

/** Media type by content (magic bytes), not by extension. */
export function sniffMediaType(head: Buffer): string | null {
  const starts = (sig: number[], at = 0) => sig.every((b, i) => head[at + i] === b);
  if (starts([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a])) return "image/png";
  if (starts([0xff, 0xd8, 0xff])) return "image/jpeg";
  if (starts([0x47, 0x49, 0x46, 0x38])) return "image/gif";
  if (starts([0x52, 0x49, 0x46, 0x46]) && starts([0x57, 0x45, 0x42, 0x50], 8)) return "image/webp";
  if (starts([0x25, 0x50, 0x44, 0x46])) return "application/pdf";
  return null;
}

/** Creates `dir` user-only, or verifies an existing one is ours, private, and not a link. */
export function ensurePrivateDir(dir: string): void {
  try {
    fs.mkdirSync(dir, { mode: 0o700 });
  } catch (e) {
    if ((e as NodeJS.ErrnoException).code !== "EEXIST") throw e;
  }
  const st = fs.lstatSync(dir);
  if (st.isSymbolicLink() || !st.isDirectory()) throw new Error(`${dir} is not a directory yaacd can use`);
  if (typeof process.getuid === "function" && st.uid !== process.getuid()) {
    throw new Error(`${dir} belongs to another user`);
  }
  // Ours, not a link: make sure nobody else can read or plant files in it.
  if ((st.mode & 0o077) !== 0) fs.chmodSync(dir, 0o700);
}

export class UploadStore {
  readonly root: string;
  private partials = new Map<string, Partial>();
  private byStream = new Map<number, Partial>();
  private nextStream = 1;

  /** Uploads and tunnels share the wire's stream-number space. */
  allocateStream(): number {
    if (this.nextStream > 0xffffffff) throw new RpcError("limit_exceeded", "data stream numbers exhausted");
    return this.nextStream++;
  }
  constructor(tempDir: string = os.tmpdir(), user: string = os.userInfo().username) {
    this.root = path.join(tempDir, `yaac-${sanitiseName(user)}`, "uploads");
  }

  private prepare(): void {
    // The system can remove temporary directories while yaacd is still running.
    const top = path.dirname(this.root);
    fs.mkdirSync(path.dirname(top), { recursive: true });
    ensurePrivateDir(top);
    ensurePrivateDir(this.root);
    for (const d of ["blobs", "meta", "threads", "partial"]) ensurePrivateDir(path.join(this.root, d));
  }

  private blobPath(sha: string): string {
    return path.join(this.root, "blobs", sha);
  }

  private metaPath(uploadId: string): string {
    return path.join(this.root, "meta", `${uploadId}.json`);
  }

  /** Store an image supplied by a harness without putting its bytes in the journal. */
  storeToolImage(bytes: Buffer, label: string): { uploadId: string; name: string } | null {
    const mediaType = sniffMediaType(bytes);
    if (!mediaType?.startsWith("image/")) return null;
    return this.storeSnapshot(bytes, label, mediaType, "tool");
  }

  storeArtifact(bytes: Buffer, name: string, accept: (uploadId: string) => void): { uploadId: string; name: string } {
    const ext = path.extname(name).slice(1).toLowerCase();
    const type = artifactFormat(ext, bytes, sniffMediaType(bytes));
    return this.storeSnapshot(bytes, sanitiseName(name), type, `artifact_${ext}`, accept);
  }

  private storeSnapshot(bytes: Buffer, label: string, mediaType: string, kind: string, accept?: (uploadId: string) => void) {
    const sha256 = sha256Hex(bytes);
    const uploadId = `u_${kind}_${sha256}`;
    accept?.(uploadId);
    this.prepare();
    const blob = this.blobPath(sha256);
    if (!fs.existsSync(blob)) {
      const temp = path.join(this.root, "partial", randomId("tool_", 12));
      try {
        fs.writeFileSync(temp, bytes, { flag: "wx", mode: 0o600 });
        fs.renameSync(temp, blob);
      } finally {
        fs.rmSync(temp, { force: true });
      }
    }
    if (!fs.existsSync(this.metaPath(uploadId))) {
      writeJsonAtomic(this.metaPath(uploadId), { uploadId, sha256, size: bytes.length, name: sanitiseName(label), mediaType });
    }
    return { uploadId, name: label };
  }

  begin(params: UploadBeginParams, owner: object): UploadBeginResult {
    this.prepare();
    if (typeof params.sha256 !== "string" || !/^[0-9a-f]{64}$/.test(params.sha256)) {
      throw new RpcError("invalid_params", "sha256 must be 64 lowercase hex digits");
    }
    if (typeof params.size !== "number" || params.size < 0 || !Number.isInteger(params.size)) {
      throw new RpcError("invalid_params", "size must be a non-negative integer");
    }
    if (params.size > UPLOAD_MAX_FILE) {
      throw new RpcError("limit_exceeded", `files are limited to ${UPLOAD_MAX_FILE} bytes on this target`);
    }
    const uploadId = randomId("u_", 12);
    const meta: UploadMeta = {
      uploadId,
      sha256: params.sha256,
      size: params.size,
      name: sanitiseName(params.name),
      mediaType: params.mediaType ?? null,
    };
    const blob = this.blobPath(params.sha256);
    if (fs.existsSync(blob) && fs.statSync(blob).size === params.size) {
      writeJsonAtomic(this.metaPath(uploadId), meta);
      return { uploadId, exists: true };
    }
    const mine = [...this.partials.values()].filter((p) => p.owner === owner);
    if (mine.length >= UPLOAD_MAX_CONCURRENT) {
      throw new RpcError("limit_exceeded", `at most ${UPLOAD_MAX_CONCURRENT} uploads at once`);
    }
    const inProgress = mine.reduce((sum, p) => sum + p.meta.size, 0);
    if (inProgress + params.size > UPLOAD_MAX_IN_PROGRESS) {
      throw new RpcError(
        "limit_exceeded",
        `unfinished uploads are limited to ${UPLOAD_MAX_IN_PROGRESS} bytes together; wait for one to finish`,
      );
    }
    const stream = this.allocateStream();
    const file = path.join(this.root, "partial", uploadId);
    const fd = fs.openSync(file, "w", 0o600);
    const partial: Partial = {
      meta,
      stream,
      file,
      fd,
      received: 0,
      hash: createHash("sha256"),
      window: new ReceiveWindow(stream, UPLOAD_WINDOW),
      owner,
    };
    this.partials.set(uploadId, partial);
    this.byStream.set(stream, partial);
    return { uploadId, exists: false, stream, credit: partial.window.initialCredit() };
  }

  /** A data frame. Throws (fatal for the connection) when the sender exceeded its credit. */
  onData(stream: number, bytes: Buffer, owner: object, grant: (stream: number, bytes: number) => void): void {
    const p = this.byStream.get(stream);
    if (!p || p.owner !== owner) return; // may cross a cancel
    p.window.onReceived(bytes.length);
    if (p.received + bytes.length > p.meta.size) {
      p.received += bytes.length; // remembered as a mismatch; nothing more is written
    } else {
      fs.writeSync(p.fd, bytes);
      p.hash.update(bytes);
      p.received += bytes.length;
    }
    const more = p.window.onConsumed(bytes.length);
    if (more !== null) grant(stream, more);
  }

  end(uploadId: string, owner: object): UploadEndResult {
    const p = this.partials.get(uploadId);
    if (!p || p.owner !== owner) throw new RpcError("upload_not_found", `no upload ${uploadId} in progress`);
    this.drop(p);
    const digest = p.hash.digest("hex");
    if (p.received !== p.meta.size || digest !== p.meta.sha256) {
      fs.rmSync(p.file, { force: true });
      throw new RpcError(
        "upload_mismatch",
        p.received !== p.meta.size
          ? `received ${p.received} bytes, expected ${p.meta.size}`
          : "the uploaded bytes do not match the declared SHA-256",
        { received: p.received, expected: p.meta.size },
      );
    }
    const head = Buffer.alloc(16);
    const fd = fs.openSync(p.file, "r");
    fs.readSync(fd, head, 0, 16, 0);
    fs.closeSync(fd);
    p.meta.mediaType = sniffMediaType(head) ?? p.meta.mediaType;
    fs.renameSync(p.file, this.blobPath(p.meta.sha256));
    writeJsonAtomic(this.metaPath(uploadId), p.meta);
    return { uploadId, size: p.meta.size };
  }

  cancel(uploadId: string, owner: object): void {
    const p = this.partials.get(uploadId);
    if (!p || p.owner !== owner) return;
    this.drop(p);
    fs.rmSync(p.file, { force: true });
  }

  private drop(p: Partial): void {
    this.partials.delete(p.meta.uploadId);
    this.byStream.delete(p.stream);
    try {
      fs.closeSync(p.fd);
    } catch {
      /* closed */
    }
  }

  /** Removes the partial uploads of a dropped connection. */
  dropOwner(owner: object): void {
    for (const p of [...this.partials.values()]) if (p.owner === owner) this.cancel(p.meta.uploadId, owner);
  }

  meta(uploadId: string): UploadMeta | null {
    if (!/^u_[A-Za-z0-9_-]+$/.test(uploadId)) return null;
    try {
      return readJson<UploadMeta>(this.metaPath(uploadId)) ?? null;
    } catch {
      return null;
    }
  }

  present(uploadId: string): boolean {
    const m = this.meta(uploadId);
    return !!m && fs.existsSync(this.blobPath(m.sha256));
  }

  read(uploadId: string, offset: number, maxBytes: number): UploadReadResult {
    const m = this.meta(uploadId);
    const blob = m ? this.blobPath(m.sha256) : null;
    if (!m || !blob || !fs.existsSync(blob)) return { present: false };
    const size = fs.statSync(blob).size;
    const start = Math.max(0, Math.min(offset, size));
    const n = Math.max(0, Math.min(maxBytes, UPLOAD_READ_MAX, size - start));
    const buf = Buffer.alloc(n);
    const fd = fs.openSync(blob, "r");
    try {
      fs.readSync(fd, buf, 0, n, start);
    } finally {
      fs.closeSync(fd);
    }
    return { present: true, size, mediaType: m.mediaType, dataBase64: buf.toString("base64"), more: start + n < size };
  }

  /** Links an upload into a thread's folder under its own name; returns the path the agent sees. */
  /** A thread's attachment folder, created (empty until something is attached). */
  threadDir(threadKey: string): string {
    this.prepare();
    const dir = path.join(this.root, "threads", sha256Hex(threadKey).slice(0, 24));
    ensurePrivateDir(dir);
    return dir;
  }

  linkForThread(threadKey: string, uploadId: string): { path: string; meta: UploadMeta } {
    const dir = this.threadDir(threadKey);
    const m = this.meta(uploadId);
    if (!m) throw new RpcError("upload_not_found", `no upload ${uploadId} on this target`);
    const blob = this.blobPath(m.sha256);
    if (!fs.existsSync(blob)) throw new RpcError("upload_not_found", `${m.name} is no longer on the target; attach it again`);
    const existing = fs.readdirSync(dir);
    const same = existing.find((f) => f.endsWith(`-${m.name}`) && fs.statSync(path.join(dir, f)).ino === fs.statSync(blob).ino);
    if (same) return { path: path.join(dir, same), meta: m };
    const target = path.join(dir, `${existing.length + 1}-${m.name}`);
    try {
      fs.linkSync(blob, target);
    } catch {
      fs.copyFileSync(blob, target);
    }
    return { path: target, meta: m };
  }

  removeThread(threadKey: string): void {
    fs.rmSync(path.join(this.root, "threads", sha256Hex(threadKey).slice(0, 24)), { recursive: true, force: true });
  }
}
