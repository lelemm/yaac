// Outbound scheduling and flow control (docs/03-protocol.md#scheduling-and-flow-control).
//
// Sans-I/O, mirroring the Rust `schedule` module:
//   * Control frames alternate with interactive stream chunks. Both go before bulk; the
//     transport writes at most one data chunk ahead of a new control or interactive frame.
//   * Small, adaptive chunks: data chunks start at 16 KiB and grow (at most doubling) up to
//     64 KiB while a chunk still leaves the wire within TARGET_CHUNK_MS; they shrink otherwise.
//   * Credit: an outbound data stream sends only as many bytes as its receiver granted.

import { DATA_CHUNK_MAX, DATA_CHUNK_MIN } from "./generated/constants.js";
import { encodeDataFrame } from "./frame.js";

export const CHUNK_MIN = DATA_CHUNK_MIN;
export const CHUNK_MAX = DATA_CHUNK_MAX;
export const TARGET_CHUNK_MS = 50;

export type TrafficClass = "control" | "bulk";

interface OutStream {
  interactive: boolean;
  pending: Buffer[];
  offset: number;
  pendingBytes: number;
  credit: number;
  closing: boolean;
  drained?: () => void;
}

export interface ScheduledFrame {
  bytes: Buffer;
  class: TrafficClass | "interactive";
}

export class Scheduler {
  private control: Buffer[] = [];
  private lastControl = false;
  private controlBytes = 0;
  private bulkMessages: Buffer[] = [];
  private bulkBytes = 0;
  private streams = new Map<number, OutStream>();
  private lastStream: number | null = null;
  private turn: "message" | "data" = "message";
  private chunk: number = CHUNK_MIN;

  pushFrame(cls: TrafficClass, frame: Buffer): void {
    if (cls === "control") {
      this.control.push(frame);
      this.controlBytes += frame.length;
    } else {
      this.bulkMessages.push(frame);
      this.bulkBytes += frame.length;
    }
  }

  openStream(stream: number, initialCredit: number, interactive = false): void {
    if (this.streams.has(stream)) throw new Error(`data stream ${stream} is already open`);
    this.streams.set(stream, { interactive, pending: [], offset: 0, pendingBytes: 0, credit: initialCredit, closing: false });
  }

  sendData(stream: number, bytes: Buffer, drained?: () => void): void {
    const out = this.streams.get(stream);
    if (!out) throw new Error(`data stream ${stream} is not open`);
    if (drained) out.drained = drained;
    out.pendingBytes += bytes.length;
    if (bytes.length > 0) out.pending.push(bytes);
  }

  /** `stream/credit` from the receiver. Unknown streams are ignored (may cross a cancel). */
  grantCredit(stream: number, bytes: number): void {
    const out = this.streams.get(stream);
    if (out && Number.isSafeInteger(bytes) && bytes > 0) out.credit = Math.min(Number.MAX_SAFE_INTEGER, out.credit + bytes);
  }

  closeStream(stream: number): void {
    const out = this.streams.get(stream);
    if (!out) return;
    out.closing = true;
    if (out.pendingBytes === 0) this.streams.delete(stream);
  }

  cancelStream(stream: number): void {
    this.streams.delete(stream);
  }

  credit(stream: number): number | undefined {
    return this.streams.get(stream)?.credit;
  }

  chunkSize(): number {
    return this.chunk;
  }

  /** Bytes of control frames waiting. */
  pendingControlBytes(): number {
    return this.controlBytes;
  }

  pendingBulkBytes(): number {
    return this.bulkBytes;
  }

  isIdle(): boolean {
    if (this.control.length > 0 || this.bulkMessages.length > 0) return false;
    for (const s of this.streams.values()) if (s.pendingBytes > 0 && s.credit > 0) return false;
    return true;
  }

  /** The next frame to write, or null. Call only when the transport can write it now. */
  next(): ScheduledFrame | null {
    if (this.lastControl || this.control.length === 0) {
      const frame = this.nextDataFrame(true);
      if (frame) {
        this.lastControl = false;
        return { bytes: frame, class: "interactive" };
      }
    }
    const c = this.control.shift();
    if (c) {
      this.lastControl = true;
      this.controlBytes -= c.length;
      return { bytes: c, class: "control" };
    }
    const order: Array<"message" | "data"> = this.turn === "message" ? ["message", "data"] : ["data", "message"];
    for (const t of order) {
      let frame: Buffer | null = null;
      if (t === "message") {
        const m = this.bulkMessages.shift();
        if (m) {
          this.bulkBytes -= m.length;
          frame = m;
        }
      } else {
        frame = this.nextDataFrame(false);
      }
      if (frame) {
        this.turn = t === "message" ? "data" : "message";
        return { bytes: frame, class: "bulk" };
      }
    }
    return null;
  }

  private nextDataFrame(interactive: boolean): Buffer | null {
    const ready = [...this.streams.entries()]
      .filter(([, s]) => s.interactive === interactive && s.pendingBytes > 0 && s.credit > 0)
      .map(([id]) => id)
      .sort((a, b) => a - b);
    if (ready.length === 0) return null;
    const last = this.lastStream;
    const pick = (last === null ? undefined : ready.find((id) => id > last)) ?? ready[0]!;
    const out = this.streams.get(pick)!;
    const allowed = Math.min(out.credit, interactive || [...this.streams.values()].some(s => s.interactive && s.pendingBytes > 0) ? CHUNK_MIN : this.chunk);
    const parts: Buffer[] = [];
    let taken = 0;
    while (taken < allowed && out.pending.length > 0) {
      const front = out.pending[0]!;
      const rest = front.subarray(out.offset);
      const n = Math.min(rest.length, allowed - taken);
      parts.push(rest.subarray(0, n));
      taken += n;
      out.offset += n;
      if (out.offset === front.length) {
        out.pending.shift();
        out.offset = 0;
      }
    }
    out.credit -= taken;
    out.pendingBytes -= taken;
    if (out.pendingBytes === 0 && out.drained) {
      queueMicrotask(out.drained);
      out.drained = undefined;
    }
    if (out.closing && out.pendingBytes === 0) this.streams.delete(pick);
    this.lastStream = pick;
    return encodeDataFrame(pick, parts.length === 1 ? parts[0]! : Buffer.concat(parts, taken));
  }

  /** `bytes` were written in `elapsedMs`: adapt the data chunk size. */
  onWritten(bytes: number, elapsedMs: number): void {
    if (bytes < CHUNK_MIN / 2) return;
    const secs = Math.max(elapsedMs / 1000, 1e-6);
    const ideal = (bytes / secs) * (TARGET_CHUNK_MS / 1000);
    let next: number = this.chunk;
    if (ideal >= this.chunk * 2) next = this.chunk * 2;
    else if (ideal < this.chunk) next = Math.floor(this.chunk / 2);
    this.chunk = Math.min(CHUNK_MAX, Math.max(CHUNK_MIN, next));
  }
}

export class CreditExceeded extends Error {}

/**
 * The receiving side of one data stream: credit is returned only once received bytes are
 * consumed (written to disk), so a slow consumer slows the sender instead of filling memory.
 */
export class ReceiveWindow {
  private outstanding: number;
  private consumed = 0;

  constructor(
    readonly stream: number,
    readonly window: number,
  ) {
    this.outstanding = window;
  }

  initialCredit(): number {
    return this.window;
  }

  onReceived(bytes: number): void {
    if (bytes > this.outstanding) {
      throw new CreditExceeded(
        `data stream ${this.stream}: received ${bytes} bytes with only ${this.outstanding} bytes of credit`,
      );
    }
    this.outstanding -= bytes;
  }

  /** Returns credit to grant once at least half the window has been consumed. */
  onConsumed(bytes: number): number | null {
    this.consumed += bytes;
    if (this.consumed > 0 && this.consumed * 2 >= this.window) {
      const grant = this.consumed;
      this.consumed = 0;
      this.outstanding += grant;
      return grant;
    }
    return null;
  }
}
