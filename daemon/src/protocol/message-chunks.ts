// Optional oversized JSON messages, mirrored by yaac-protocol/src/message_chunks.rs.
import { encodeDataFrame, encodeMessageFrame, FrameError, MAX_FRAME_LEN, type Frame } from "./frame.js";

const STREAM = 0;
const HEADER = 9;
const CHUNK = 16 * 1024;
const MAX_MESSAGE = 32 * 1024 * 1024;
const LANES = 2;

export function encodeMessageFrames(text: string, lane: number | null): Buffer[] {
  const payload = Buffer.from(text, "utf8");
  if (lane === null || payload.length < MAX_FRAME_LEN) return [encodeMessageFrame(payload)];
  if (payload.length > MAX_MESSAGE) throw new FrameError("oversize", `message of ${payload.length} bytes exceeds the ${MAX_MESSAGE}-byte limit`);
  const frames: Buffer[] = [];
  for (let offset = 0; offset < payload.length; offset += CHUNK) {
    const part = payload.subarray(offset, offset + CHUNK);
    const bytes = Buffer.allocUnsafe(HEADER + part.length);
    bytes[0] = lane;
    bytes.writeUInt32LE(payload.length, 1);
    bytes.writeUInt32LE(offset, 5);
    part.copy(bytes, HEADER);
    frames.push(encodeDataFrame(STREAM, bytes));
  }
  return frames;
}

export class MessageChunks {
  private partial: ({ total: number; length: number; parts: Buffer[] } | undefined)[] = [];

  accept(frame: Frame): Frame | null {
    if (frame.kind !== "data" || frame.stream !== STREAM) return frame;
    const { bytes } = frame;
    if (bytes.length < HEADER) throw new FrameError("invalidMessageFragment", "short message fragment");
    const lane = bytes[0]!;
    const total = bytes.readUInt32LE(1);
    const offset = bytes.readUInt32LE(5);
    const part = bytes.subarray(HEADER);
    const pending = this.partial[lane];
    if (lane >= LANES || total < MAX_FRAME_LEN || total > MAX_MESSAGE || offset >= total ||
        part.length !== Math.min(CHUNK, total - offset) || (offset === 0) !== (pending === undefined) ||
        (pending && (pending.total !== total || pending.length !== offset))) {
      throw new FrameError("invalidMessageFragment", "invalid message fragment");
    }
    const message = pending ?? { total, length: 0, parts: [] };
    message.parts.push(part);
    message.length += part.length;
    if (message.length === total) {
      this.partial[lane] = undefined;
      return { kind: "message", payload: Buffer.concat(message.parts, total) };
    }
    this.partial[lane] = message;
    return null;
  }
}
