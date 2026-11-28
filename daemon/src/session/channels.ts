// An integrator's channel is local to this daemon and bound to one socket. Its dialogs
// outlive that socket. Each dialog has a durable record while pending, since a thread
// journal can trim acknowledged events even while the request is still waiting.

import * as fs from "node:fs";
import * as path from "node:path";
import type { Socket } from "node:net";
import type {
  ChannelRegisteredParams,
  DialogResultParams,
  InboxListResult,
  Item,
  RequestItem,
  RequestResolution,
} from "../protocol/generated/types.js";
import { METHODS, NOTIFICATIONS } from "../protocol/generated/constants.js";
import { decodeMessage, MessageDecodeError, RpcError } from "../protocol/rpc.js";
import { ensureDir, nowIso, randomId, readJson, writeJsonAtomic } from "../util/fsx.js";
import type { ClientLink } from "./core.js";
import { CHANNEL_LINE_BYTES, parse, parseAnswer, parseOpen, registration, withdrawal } from "./dialog-layout.js";

type Dialog = Extract<RequestItem, { requestKind: "dialog" }>;

interface DialogRecord {
  item: Dialog & Pick<Item, "id" | "turnId" | "status" | "startedAt">;
  threadId?: string;
  integratorId: string;
}

interface Channel {
  info: ChannelRegisteredParams;
  seen: Map<string, string>;
  writeResult(params: DialogResultParams): void;
}

interface ChannelHooks {
  thread(id: string): { id: string; turnId: string } | undefined;
  publishThread(threadId: string, item: Item, started: boolean): void;
  broadcast(method: string, params: unknown): void;
  activity(): void;
  log(line: string): void;
}

const REQUEST_PREFIX = "dialog-";
// Keep a complete inbox or thread pending-request snapshot within messageChunks' 32 MiB bound.
const PENDING_BYTES_LIMIT = 16 * 1024 * 1024;
const CHANNEL_LIMIT = 256;

export class Channels {
  private readonly dir: string;
  private readonly channels = new Map<string, Channel>();
  private readonly pending = new Map<string, DialogRecord>();
  private pendingBytes = 0;

  constructor(home: string, private readonly hooks: ChannelHooks) {
    this.dir = path.join(home, "dialogs");
    ensureDir(this.dir, 0o700);
  }

  get connected(): number {
    return this.channels.size;
  }

  get waiting(): number {
    return this.pending.size;
  }

  private file(id: string): string {
    return path.join(this.dir, `${id}.json`);
  }

  /** A restart ends the original channel. Recover the dialog without reconnecting it. */
  restore(): void {
    for (const file of fs.readdirSync(this.dir)) {
      if (!/^dialog-[A-Za-z0-9_-]+\.json$/.test(file)) continue;
      const record = readJson<DialogRecord>(path.join(this.dir, file));
      if (!record) continue;
      record.item.channelOpen = false;
      if (record.threadId && !this.hooks.thread(record.threadId)) delete record.threadId;
      this.store(record, false);
    }
  }

  private asRequest(record: DialogRecord): Dialog {
    const { id: _id, turnId: _turnId, status: _status, startedAt: _startedAt, ...request } = record.item;
    return request;
  }

  private store(record: DialogRecord, started: boolean, committed?: () => void): void {
    const { item } = record;
    const id = item.requestId;
    const previous = this.pending.get(id);
    const previousBytes = previous ? Buffer.byteLength(JSON.stringify(previous)) : 0;
    const pending = item.state === "pending";
    const bytes = pending ? Buffer.byteLength(JSON.stringify(record)) : 0;
    if (started && this.pendingBytes + bytes > PENDING_BYTES_LIMIT) {
      throw new RpcError("limit_exceeded", "The target already has too much pending dialog content.");
    }
    // The record is authoritative before the request is visible, whether it is projected
    // into a thread journal or into the inbox. No secret answer enters this record.
    writeJsonAtomic(this.file(id), record);
    this.pendingBytes += bytes - previousBytes;
    if (pending) this.pending.set(id, record);
    else this.pending.delete(id);
    committed?.();
    try {
      if (record.threadId) {
        this.hooks.publishThread(record.threadId, { ...item, kind: "request" }, started);
      } else {
        this.hooks.broadcast(NOTIFICATIONS.INBOX_UPDATED, { request: this.asRequest(record) });
      }
    } catch {
      // The authoritative record is already committed. Keep it for replay if the
      // transcript journal or a host connection fails while it is being published.
      this.hooks.log(`Could not publish dialog ${id}; its record is durable.`);
      return;
    } finally {
      this.hooks.activity();
    }
    if (!pending) {
      // A crash before this cleanup replays the terminal state into the associated thread.
      try {
        fs.rmSync(this.file(id), { force: true });
      } catch {
        this.hooks.log(`Could not remove the completed dialog record ${id}.`);
      }
    }
  }

  inbox(): InboxListResult {
    return {
      requests: [...this.pending.values()].filter((record) => !record.threadId).map((record) => this.asRequest(record)),
      channels: [...this.channels.values()].map((channel) => channel.info),
    };
  }

  threadHasPending(threadId: string): boolean {
    for (const record of this.pending.values()) if (record.threadId === threadId) return true;
    return false;
  }

  threadItems(threadId: string): Item[] {
    return [...this.pending.values()].filter((record) => record.threadId === threadId).map(({ item }) => ({ ...item, kind: "request" }));
  }

  /** Preserve outstanding dialogs when the user deletes their thread. */
  threadRemoved(threadId: string): void {
    for (const record of this.pending.values()) {
      if (record.threadId !== threadId) continue;
      const moved = { ...record };
      delete moved.threadId;
      this.store(moved, false);
    }
  }

  respond(threadId: string | undefined, requestId: unknown, raw: unknown, client: ClientLink): void {
    const record = typeof requestId === "string" ? this.pending.get(requestId) : undefined;
    if (!record) throw new RpcError("request_not_pending", "This dialog has already been answered or dismissed.");
    if (record.threadId !== threadId) throw new RpcError("request_not_found", "This dialog is in a different request scope.");
    const channel = this.channels.get(record.item.source.channelId);
    const dismissing = typeof raw === "object" && raw !== null && "cancelled" in raw && raw.cancelled === true;
    if (!channel && !dismissing) throw new RpcError("channel_closed", `${record.item.source.name} is no longer connected.`);
    const { answer, recorded } = parseAnswer(raw, record.item.layout);
    const resolution: RequestResolution = {
      by: "client",
      clientId: client.key,
      ...(client.host ? { clientHost: client.host } : {}),
      at: nowIso(),
      answer: { ...recorded, requestKind: "dialog" },
    };
    this.finish(record, resolution);
    channel?.writeResult(answer.cancelled
      ? { requestId: record.integratorId, outcome: "cancelled" }
      : { requestId: record.integratorId, outcome: "answered", action: answer.action, values: answer.values });
  }

  isDialogId(id: unknown): boolean {
    return typeof id === "string" && id.startsWith(REQUEST_PREFIX);
  }

  private finish(record: DialogRecord, resolution?: RequestResolution): void {
    this.store({
      ...record,
      item: {
        ...record.item,
        state: resolution ? "resolved" : "expired",
        status: "completed",
        ...(resolution ? { resolution } : {}),
      },
    }, false);
  }

  private register(raw: unknown, writeResult: Channel["writeResult"]): Channel {
    const params = parse(registration, raw);
    if (this.channels.size >= CHANNEL_LIMIT) throw new RpcError("limit_exceeded", "Too many integrator channels are connected.");
    const info: ChannelRegisteredParams = { ...params, channelId: randomId("channel-", 16) };
    const channel = { info, writeResult, seen: new Map<string, string>() };
    this.channels.set(info.channelId, channel);
    this.hooks.broadcast(NOTIFICATIONS.CHANNEL_REGISTERED, info);
    this.hooks.activity();
    return channel;
  }

  private open(channel: Channel, raw: unknown): { accepted: true } {
    const { requestId: integratorId } = parse(withdrawal, raw);
    if (channel.seen.has(integratorId)) throw new RpcError("duplicate_request", "This requestId was already used on this channel.");
    const params = parseOpen(raw);
    const thread = params.threadId ? this.hooks.thread(params.threadId) : undefined;
    const requestId = randomId(REQUEST_PREFIX, 16);
    const record: DialogRecord = {
      integratorId: params.requestId,
      ...(thread ? { threadId: thread.id } : {}),
      item: {
        id: `request:${requestId}`,
        turnId: thread?.turnId ?? "",
        status: "inProgress",
        startedAt: nowIso(),
        requestId,
        requestKind: "dialog",
        state: "pending",
        blocking: false,
        title: params.title,
        layout: params.layout,
        source: { channelId: channel.info.channelId, name: channel.info.name },
        channelOpen: true,
      },
    };
    // Once the record is published the integrator ID is consumed, even if its reply is lost.
    this.store(record, true, () => channel.seen.set(integratorId, requestId));
    return { accepted: true };
  }

  private withdraw(channel: Channel, raw: unknown): object {
    const { requestId } = parse(withdrawal, raw);
    const id = channel.seen.get(requestId);
    if (!id) throw new RpcError("request_not_found", "This requestId was not opened on this channel.");
    const record = this.pending.get(id);
    if (!record) throw new RpcError("request_not_pending", "This dialog has already ended.");
    this.finish(record);
    channel.writeResult({ requestId, outcome: "cancelled" });
    return {};
  }

  private disconnect(channel: Channel): void {
    if (!this.channels.delete(channel.info.channelId)) return;
    for (const record of this.pending.values()) {
      if (record.item.source.channelId !== channel.info.channelId) continue;
      try {
        this.store({ ...record, item: { ...record.item, channelOpen: false } }, false);
      } catch {
        this.hooks.log(`Could not publish the disconnect for dialog ${record.item.requestId}.`);
      }
    }
    this.hooks.broadcast(NOTIFICATIONS.CHANNEL_CLOSED, { channelId: channel.info.channelId });
    this.hooks.activity();
  }

  /** Newline-delimited RPC, with the same named errors as the daemon protocol. */
  attach(socket: Socket): void {
    let channel: Channel | undefined;
    let buffer = "";
    const write = (message: object) => {
      if (socket.destroyed || !socket.writable) return;
      if (socket.writableLength > 2 * CHANNEL_LINE_BYTES) {
        socket.destroy();
        return;
      }
      socket.write(`${JSON.stringify({ jsonrpc: "2.0", ...message })}\n`);
    };
    const handle = (line: string) => {
      let id: string | number | null = null;
      try {
        const message = decodeMessage(line);
        if (message.type !== "request") throw new RpcError("invalid_request", "A channel request needs an RPC id.");
        id = message.id;
        let result: unknown;
        if (message.method === METHODS.CHANNEL_REGISTER) {
          if (channel) throw new RpcError("already_registered", "This connection already has a channel.");
          channel = this.register(message.params, (params) => write({ method: NOTIFICATIONS.DIALOG_RESULT, params }));
          result = { channelId: channel.info.channelId };
        } else {
          if (!channel) throw new RpcError("not_registered", "Register this connection with channel/register first.");
          switch (message.method) {
            case METHODS.DIALOG_OPEN: result = this.open(channel, message.params); break;
            case METHODS.DIALOG_CLOSE: result = this.withdraw(channel, message.params); break;
            default: throw new RpcError("method_not_found", "This method is unavailable on a channel connection.");
          }
        }
        write({ id, result });
      } catch (e) {
        const error = e instanceof RpcError ? e.toObject()
          : e instanceof MessageDecodeError ? { code: e.code, message: "Invalid channel RPC message." }
          : { code: "internal_error", message: "The daemon could not complete the channel request." };
        if (error.code === "internal_error") this.hooks.log("Could not complete a channel request.");
        write({ id: e instanceof MessageDecodeError ? e.id : id, error });
      }
    };
    socket.setEncoding("utf8");
    socket.on("data", (chunk: string) => {
      buffer += chunk;
      let newline: number;
      while ((newline = buffer.indexOf("\n")) !== -1 && !socket.destroyed) {
        const line = buffer.slice(0, newline);
        buffer = buffer.slice(newline + 1);
        if (Buffer.byteLength(line) > CHANNEL_LINE_BYTES) {
          write({ id: null, error: { code: "limit_exceeded", message: "Channel message is too large." } });
          socket.end();
          return;
        }
        if (line.trim()) handle(line);
      }
      if (Buffer.byteLength(buffer) > CHANNEL_LINE_BYTES) {
        write({ id: null, error: { code: "limit_exceeded", message: "Channel message is too large." } });
        socket.end();
      }
    });
    const disconnect = () => {
      if (channel) this.disconnect(channel);
      channel = undefined;
    };
    socket.once("end", disconnect);
    socket.once("close", disconnect);
  }
}
