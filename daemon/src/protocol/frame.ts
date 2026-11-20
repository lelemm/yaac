// The frame format (docs/03-protocol.md#framing). Part of the frozen core.
//
//   length: u32 LE | kind: u8 | payload (length - 1 bytes)
//
// `length` counts the kind byte and the payload and may not exceed MAX_FRAME_LEN (1 MiB).
// Kind 0x01 is one UTF-8 JSON message; 0x02 is `stream: u32 LE` followed by raw bytes. Every
// decoding error is fatal for the stream: framing cannot resynchronise.

import { FRAME_KIND_DATA, FRAME_KIND_MESSAGE, MAX_FRAME_LEN } from "./generated/constants.js";

export { MAX_FRAME_LEN };
export const LENGTH_PREFIX = 4;
export const STREAM_PREFIX = 4;
export const KIND_MESSAGE = FRAME_KIND_MESSAGE;
export const KIND_DATA = FRAME_KIND_DATA;

export type Frame =
  | { kind: "message"; payload: Buffer }
  | { kind: "data"; stream: number; bytes: Buffer };

export class FrameError extends Error {
  constructor(
    readonly reason: "oversize" | "empty" | "unknownKind" | "shortData",
    message: string,
  ) {
    super(message);
    this.name = "FrameError";
  }
}

function prefix(len: number): Buffer {
  if (len > MAX_FRAME_LEN) {
    throw new FrameError("oversize", `frame of ${len} bytes exceeds the ${MAX_FRAME_LEN}-byte limit`);
  }
  const out = Buffer.allocUnsafe(LENGTH_PREFIX);
  out.writeUInt32LE(len, 0);
  return out;
}

/** Encodes a 0x01 frame around one JSON payload. */
export function encodeMessageFrame(payload: Buffer | string): Buffer {
  const body = typeof payload === "string" ? Buffer.from(payload, "utf8") : payload;
  const head = prefix(1 + body.length);
  return Buffer.concat([head, Buffer.of(KIND_MESSAGE), body]);
}

/** Encodes a 0x02 frame carrying `bytes` on `stream`. */
export function encodeDataFrame(stream: number, bytes: Buffer): Buffer {
  const head = prefix(1 + STREAM_PREFIX + bytes.length);
  const sid = Buffer.allocUnsafe(STREAM_PREFIX);
  sid.writeUInt32LE(stream >>> 0, 0);
  return Buffer.concat([head, Buffer.of(KIND_DATA), sid, bytes]);
}

export function encodeFrame(frame: Frame): Buffer {
  return frame.kind === "message" ? encodeMessageFrame(frame.payload) : encodeDataFrame(frame.stream, frame.bytes);
}

/**
 * Incremental decoder: push bytes split anywhere, take whole frames out. After an error it is
 * poisoned and returns the same error forever.
 */
export class FrameDecoder {
  private chunks: Buffer[] = [];
  private buffered = 0;
  private error: FrameError | null = null;
  private readonly maxLen: number;

  constructor(maxLen: number = MAX_FRAME_LEN) {
    this.maxLen = Math.min(maxLen, MAX_FRAME_LEN);
  }

  push(bytes: Buffer): void {
    if (this.error || bytes.length === 0) return;
    this.chunks.push(bytes);
    this.buffered += bytes.length;
  }

  get pending(): number {
    return this.buffered;
  }

  private peek(n: number): Buffer | null {
    if (this.buffered < n) return null;
    if (this.chunks[0]!.length < n) {
      const merged = Buffer.concat(this.chunks);
      this.chunks = [merged];
    }
    return this.chunks[0]!.subarray(0, n);
  }

  private take(n: number): Buffer {
    const all = this.chunks.length === 1 ? this.chunks[0]! : Buffer.concat(this.chunks);
    const out = Buffer.from(all.subarray(0, n));
    const rest = all.subarray(n);
    this.chunks = rest.length > 0 ? [rest] : [];
    this.buffered -= n;
    return out;
  }

  private fail(error: FrameError): never {
    this.error = error;
    this.chunks = [];
    this.buffered = 0;
    throw error;
  }

  /** The next complete frame, `null` when more bytes are needed; throws FrameError (fatal). */
  next(): Frame | null {
    if (this.error) throw this.error;
    const head = this.peek(LENGTH_PREFIX + 1) ?? this.peek(LENGTH_PREFIX);
    if (!head) return null;
    const len = head.readUInt32LE(0);
    if (len === 0) this.fail(new FrameError("empty", "empty frame (length 0)"));
    if (len > this.maxLen) {
      this.fail(new FrameError("oversize", `frame of ${len} bytes exceeds the ${this.maxLen}-byte limit`));
    }
    if (head.length < LENGTH_PREFIX + 1) return null;
    const kind = head[LENGTH_PREFIX]!;
    if (kind !== KIND_MESSAGE && kind !== KIND_DATA) {
      this.fail(new FrameError("unknownKind", `unknown frame kind 0x${kind.toString(16).padStart(2, "0")}`));
    }
    if (this.buffered < LENGTH_PREFIX + len) return null;
    if (kind === KIND_DATA && len - 1 < STREAM_PREFIX) {
      this.fail(new FrameError("shortData", `data frame of ${len - 1} payload bytes is too short for a stream number`));
    }
    const whole = this.take(LENGTH_PREFIX + len);
    const body = whole.subarray(LENGTH_PREFIX + 1);
    if (kind === KIND_MESSAGE) return { kind: "message", payload: body };
    return { kind: "data", stream: body.readUInt32LE(0), bytes: body.subarray(STREAM_PREFIX) };
  }

  /** Pushes bytes and drains every complete frame. */
  decode(bytes: Buffer): Frame[] {
    this.push(bytes);
    const out: Frame[] = [];
    for (let f = this.next(); f; f = this.next()) out.push(f);
    return out;
  }
}
