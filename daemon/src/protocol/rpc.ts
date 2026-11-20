// JSON-RPC 2.0 *shapes* (docs/03-protocol.md#messages). Part of the frozen core.
//
// No `"jsonrpc"` member (ignored if present); an error's `code` is a snake_case string.
// Envelope members in a fixed order — `id`, `method`, then `params` / `result` / `error` —
// and every object inside them canonical (sorted keys).

import { canonical } from "./json.js";
import type { ErrorCode, ErrorObject, RpcId } from "./generated/types.js";

export type Message =
  | { type: "request"; id: RpcId; method: string; params: unknown }
  | { type: "response"; id: RpcId; result: unknown }
  | { type: "error"; id: RpcId | null; error: ErrorObject }
  | { type: "notification"; method: string; params: unknown };

/** An error that becomes an error response. `code` is open: any string is allowed. */
export class RpcError extends Error {
  constructor(
    readonly code: ErrorCode | (string & {}),
    message: string,
    readonly data?: unknown,
  ) {
    super(message);
    this.name = "RpcError";
  }

  toObject(): ErrorObject {
    const out: ErrorObject = { code: this.code as ErrorCode, message: this.message };
    if (this.data !== undefined) out.data = this.data;
    return out;
  }
}

/** Encodes a message as its canonical JSON payload. */
export function encodeMessage(message: Message): string {
  switch (message.type) {
    case "request":
      return `{"id":${canonical(message.id)},"method":${canonical(message.method)},"params":${canonical(message.params ?? {})}}`;
    case "response":
      return `{"id":${canonical(message.id)},"result":${canonical(message.result ?? {})}}`;
    case "error":
      return `{"id":${canonical(message.id)},"error":${canonical(message.error)}}`;
    case "notification":
      return `{"method":${canonical(message.method)},"params":${canonical(message.params ?? {})}}`;
  }
}

export class MessageDecodeError extends Error {
  constructor(
    readonly code: "parse_error" | "invalid_request",
    message: string,
    readonly id: RpcId | null,
  ) {
    super(message);
  }
}

function isObject(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

/** Decodes one `0x01` payload. Throws MessageDecodeError (answerable, not fatal). */
export function decodeMessage(payload: Buffer | string): Message {
  let value: unknown;
  try {
    value = JSON.parse(typeof payload === "string" ? payload : payload.toString("utf8"));
  } catch (e) {
    throw new MessageDecodeError("parse_error", `message is not valid JSON: ${(e as Error).message}`, null);
  }
  if (!isObject(value)) throw new MessageDecodeError("invalid_request", "message is not a JSON object", null);
  let id: RpcId | null = null;
  if (value.id !== undefined && value.id !== null) {
    if (typeof value.id !== "number" && typeof value.id !== "string") {
      throw new MessageDecodeError("invalid_request", "id is not a number or string", null);
    }
    id = value.id;
  }
  if (value.method !== undefined) {
    if (typeof value.method !== "string") throw new MessageDecodeError("invalid_request", "method is not a string", id);
    const params = value.params === undefined || value.params === null ? {} : value.params;
    return id === null
      ? { type: "notification", method: value.method, params }
      : { type: "request", id, method: value.method, params };
  }
  if (value.error !== undefined) {
    const e = value.error;
    if (!isObject(e) || typeof e.code !== "string" || typeof e.message !== "string") {
      throw new MessageDecodeError("invalid_request", "malformed error object", id);
    }
    const error: ErrorObject = { code: e.code as ErrorCode, message: e.message };
    if (e.data !== undefined) error.data = e.data;
    return { type: "error", id, error };
  }
  if (value.result !== undefined) {
    if (id === null) throw new MessageDecodeError("invalid_request", "response without id", null);
    return { type: "response", id, result: value.result };
  }
  throw new MessageDecodeError("invalid_request", "no method, result, or error", id);
}
