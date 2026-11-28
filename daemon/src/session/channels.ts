// Each integrator channel can hold one socket at a time. Dialog decisions and
// delivery receipts outlive that socket, and pending dialogs outlive thread-journal trimming.

import * as fs from "node:fs";
import * as path from "node:path";
import type { Socket } from "node:net";
import type {
  ChannelRegisteredParams,
  DialogOpenResult,
  DialogResultParams,
  DialogStatusResult,
  InboxListResult,
  Item,
  RequestItem,
  RequestResolution,
} from "../protocol/generated/types.js";
import { METHODS, NOTIFICATIONS } from "../protocol/generated/constants.js";
import { RpcError } from "../protocol/rpc.js";
import { ensureDir, nowIso, randomId, readJson, writeJsonAtomic } from "../util/fsx.js";
import type { ClientLink } from "./core.js";
import { CHANNEL_LINE_BYTES, parse, parseAnswer, parseOpen, registration, withdrawal } from "./dialog-layout.js";

type Dialog = Extract<RequestItem, { requestKind: "dialog" }>;

interface DialogRecord {
  item: Dialog & Pick<Item, "id" | "turnId" | "status" | "startedAt">;
  threadId?: string;
  integratorId: string;
  result?: DialogResultParams;
  delivered?: boolean;
}

interface Receipt {
  channelId: string;
  result: DialogResultParams;
  delivered?: boolean;
}

type StoredDialog = DialogRecord | Receipt;

interface ChannelIdentity {
  info: ChannelRegisteredParams;
  seen: Map<string, string>;
  outbox: Set<string>;
}

interface Channel {
  identity: ChannelIdentity;
  sent: Set<string>;
  writing: boolean;
  writeResult(params: DialogResultParams, written: (error?: Error | null) => void): boolean;
}

interface ChannelHooks {
  thread(id?: string, nativeThreadId?: string): { id: string; turnId: string } | undefined;
  publishThread(threadId: string, item: Item, started: boolean): void;
  broadcast(method: string, params: unknown): void;
  activity(): void;
  log(line: string): void;
}

const REQUEST_PREFIX = "dialog-";
// Keep a complete inbox or thread pending-request snapshot within messageChunks' 32 MiB bound.
const PENDING_BYTES_LIMIT = 16 * 1024 * 1024;
const CHANNEL_LIMIT = 256;
const RPC_CODES = new Map([
  ["parse_error", -32700], ["invalid_request", -32600], ["method_not_found", -32601], ["invalid_params", -32602],
]);

function rpcError(reason: string, message: string) {
  return { code: RPC_CODES.get(reason) ?? -32000, message, data: { reason } };
}

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export class Channels {
  private readonly dir: string;
  private readonly identities = new Map<string, ChannelIdentity>();
  private readonly channels = new Map<string, Channel>();
  // Secrets waiting for their first socket write exist only in this daemon's memory.
  private readonly secrets = new Map<string, NonNullable<DialogResultParams["values"]>>();
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

  private remember(info: ChannelRegisteredParams): ChannelIdentity {
    const identity = { info, seen: new Map<string, string>(), outbox: new Set<string>() };
    this.identities.set(info.channelId, identity);
    return identity;
  }

  /** Restore durable channel identities and decisions. Every connection starts disconnected. */
  restore(): void {
    const files = fs.readdirSync(this.dir);
    for (const file of files) {
      if (!/^channel-[A-Za-z0-9_-]+\.json$/.test(file)) continue;
      const info = readJson<ChannelRegisteredParams>(path.join(this.dir, file));
      if (info) this.remember(info);
    }
    for (const file of files) {
      if (!/^dialog-[A-Za-z0-9_-]+\.json$/.test(file)) continue;
      const id = file.slice(0, -".json".length);
      const stored = readJson<StoredDialog>(this.file(id));
      if (!stored) continue;
      if (!("item" in stored)) {
        const channel = this.identities.get(stored.channelId);
        channel?.seen.set(stored.result.requestId, id);
        if (!stored.delivered) channel?.outbox.add(id);
        continue;
      }
      const source = stored.item.source;
      let channel = this.identities.get(source.channelId);
      if (!channel) {
        // Dialogs written before resumable channels carried their source on the item.
        writeJsonAtomic(this.file(source.channelId), source);
        channel = this.remember(source);
      }
      channel.seen.set(stored.integratorId, id);
      if (stored.item.state !== "pending") {
        stored.result ??= this.oldResult(stored);
        if (!stored.delivered) channel.outbox.add(id);
      }
      stored.item.channelOpen = false;
      if (stored.threadId && !this.hooks.thread(stored.threadId)) delete stored.threadId;
      this.store(stored, false);
    }
  }

  private oldResult(record: DialogRecord): DialogResultParams {
    const answer = record.item.resolution?.answer;
    if (answer?.requestKind === "dialog" && !answer.cancelled) {
      return { requestId: record.integratorId, outcome: "answered", action: answer.action, values: answer.values };
    }
    return { requestId: record.integratorId, outcome: record.item.state === "unknown" ? "unknown" : "cancelled" };
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
    if (record.result) {
      // After the terminal item has been published, retain only the decision and its
      // delivery receipt. A crash before compaction replays the terminal item.
      const receipt: Receipt = { channelId: item.source.channelId, result: record.result, delivered: record.delivered };
      try {
        writeJsonAtomic(this.file(id), receipt);
      } catch {
        this.hooks.log(`Could not compact the completed dialog record ${id}.`);
      }
    }
  }

  inbox(): InboxListResult {
    return {
      requests: [...this.pending.values()].filter((record) => !record.threadId).map((record) => this.asRequest(record)),
      channels: [...this.channels.values()].map((channel) => channel.identity.info),
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
    const { answer, recorded } = parseAnswer(raw, record.item.layout);
    const resolution: RequestResolution = {
      by: "client",
      clientId: client.key,
      ...(client.host ? { clientHost: client.host } : {}),
      at: nowIso(),
      answer: { ...recorded, requestKind: "dialog" },
    };
    const values = recorded.values ?? {};
    const secrets = Object.fromEntries(Object.entries(answer.values ?? {}).filter(([id]) => !Object.hasOwn(values, id)));
    const result: DialogResultParams = answer.cancelled
      ? { requestId: record.integratorId, outcome: "cancelled" }
      : { requestId: record.integratorId, outcome: "answered", action: answer.action, values };
    this.finish(record, result, resolution, secrets);
  }

  isDialogId(id: unknown): boolean {
    return typeof id === "string" && id.startsWith(REQUEST_PREFIX);
  }

  private finish(
    record: DialogRecord, result: DialogResultParams, resolution?: RequestResolution,
    secrets: NonNullable<DialogResultParams["values"]> = {},
  ): void {
    this.store({
      ...record,
      result,
      item: {
        ...record.item,
        state: resolution ? "resolved" : "expired",
        status: "completed",
        ...(resolution ? { resolution } : {}),
      },
    }, false);
    const id = record.item.requestId;
    if (Object.keys(secrets).length) this.secrets.set(id, secrets);
    const channelId = record.item.source.channelId;
    this.identities.get(channelId)!.outbox.add(id);
    const live = this.channels.get(channelId);
    if (live) this.flushResults(live);
  }

  private takeResult(id: string, result: DialogResultParams): DialogResultParams {
    const secrets = this.secrets.get(id);
    this.secrets.delete(id);
    return secrets ? { ...result, values: { ...result.values, ...secrets } } : result;
  }

  private flushResults(channel: Channel): void {
    const { info, outbox } = channel.identity;
    if (channel.writing || this.channels.get(info.channelId) !== channel) return;
    try {
      for (const id of outbox) {
        if (channel.sent.has(id)) continue;
        const receipt = readJson<StoredDialog>(this.file(id));
        if (!receipt?.result || receipt.delivered) continue;
        channel.sent.add(id);
        channel.writing = channel.writeResult(this.takeResult(id, receipt.result), (error) => {
          channel.writing = false;
          if (error) return;
          try {
            writeJsonAtomic(this.file(id), { ...receipt, delivered: true });
            outbox.delete(id);
          } catch {
            // A future connection may be sent the redacted decision again.
            this.hooks.log(`Could not mark dialog result ${id} as delivered.`);
          }
          this.flushResults(channel);
        });
        return;
      }
    } catch {
      this.hooks.log(`Could not read results for channel ${info.channelId}.`);
    }
  }

  private register(raw: unknown, writeResult: Channel["writeResult"]): Channel {
    const { resume, ...params } = parse(registration, raw);
    if (resume && this.channels.has(resume)) throw new RpcError("channel_busy", "That channel already has a live connection.");
    if (this.channels.size >= CHANNEL_LIMIT) throw new RpcError("limit_exceeded", "Too many integrator channels are connected.");
    let identity: ChannelIdentity;
    if (resume) {
      const existing = this.identities.get(resume);
      if (!existing) throw new RpcError("channel_not_found", "That channel is unknown on this target.");
      identity = existing;
    } else {
      const info: ChannelRegisteredParams = { ...params, channelId: randomId("channel-", 16) };
      writeJsonAtomic(this.file(info.channelId), info);
      identity = this.remember(info);
    }
    const channel: Channel = { identity, writeResult, sent: new Set(), writing: false };
    this.channels.set(identity.info.channelId, channel);
    this.hooks.broadcast(NOTIFICATIONS.CHANNEL_REGISTERED, identity.info);
    if (resume) this.setChannelOpen(channel, true);
    this.hooks.activity();
    return channel;
  }

  private open(channel: Channel, raw: unknown): DialogOpenResult {
    const { requestId: integratorId } = parse(withdrawal, raw);
    if (channel.identity.seen.has(integratorId)) throw new RpcError("duplicate_request", "This requestId was already used on this channel.");
    const params = parseOpen(raw);
    const thread = this.hooks.thread(params.threadId ?? undefined, params.nativeThreadId ?? undefined);
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
        source: { channelId: channel.identity.info.channelId, name: channel.identity.info.name },
        channelOpen: true,
      },
    };
    // Once the record is published the integrator ID is consumed, even if its reply is lost.
    this.store(record, true, () => channel.identity.seen.set(integratorId, requestId));
    return thread
      ? { accepted: true, placement: "thread", threadId: thread.id }
      : { accepted: true, placement: "inbox" };
  }

  private scopedId(channel: Channel, requestId: string): string {
    const id = channel.identity.seen.get(requestId);
    if (!id) throw new RpcError("request_not_found", "This requestId was not opened on this channel.");
    return id;
  }

  private status(channel: Channel, raw: unknown): DialogStatusResult {
    const { requestId } = parse(withdrawal, raw);
    const id = this.scopedId(channel, requestId);
    if (this.pending.has(id)) return { state: "pending" };
    const result = readJson<StoredDialog>(this.file(id))?.result;
    if (!result) return { state: "unknown" };
    const { outcome, action, values } = this.takeResult(id, result);
    return { state: outcome, action, values };
  }

  private withdraw(channel: Channel, raw: unknown): object {
    const { requestId } = parse(withdrawal, raw);
    const record = this.pending.get(this.scopedId(channel, requestId));
    if (!record) throw new RpcError("request_not_pending", "This dialog has already ended.");
    this.finish(record, { requestId, outcome: "cancelled" });
    return {};
  }

  private setChannelOpen(channel: Channel, channelOpen: boolean): void {
    for (const record of this.pending.values()) {
      if (record.item.source.channelId !== channel.identity.info.channelId) continue;
      try {
        this.store({ ...record, item: { ...record.item, channelOpen } }, false);
      } catch {
        this.hooks.log(`Could not publish the channel status for dialog ${record.item.requestId}.`);
      }
    }
  }

  private disconnect(channel: Channel): void {
    const { channelId } = channel.identity.info;
    if (this.channels.get(channelId) !== channel) return;
    this.channels.delete(channelId);
    this.setChannelOpen(channel, false);
    this.hooks.broadcast(NOTIFICATIONS.CHANNEL_CLOSED, { channelId });
    this.hooks.activity();
  }

  /** Newline-delimited JSON-RPC 2.0. The hub transport has its own error envelope. */
  attach(socket: Socket): void {
    let channel: Channel | undefined;
    let buffer = "";
    const write = (message: object, written?: (error?: Error | null) => void): boolean => {
      if (socket.destroyed || !socket.writable) return false;
      if (socket.writableLength > 2 * CHANNEL_LINE_BYTES) {
        socket.destroy();
        return false;
      }
      socket.write(`${JSON.stringify({ jsonrpc: "2.0", ...message })}\n`, written);
      return true;
    };
    const handle = (line: string) => {
      let raw: unknown;
      try {
        raw = JSON.parse(line);
      } catch {
        write({ id: null, error: rpcError("parse_error", "Invalid channel JSON.") });
        return;
      }
      const id = isObject(raw) && (typeof raw.id === "string" || (typeof raw.id === "number" && Number.isFinite(raw.id))) ? raw.id : null;
      if (!isObject(raw) || raw.jsonrpc !== "2.0" || typeof raw.method !== "string" || id === null) {
        write({ id: null, error: rpcError("invalid_request", "Expected a JSON-RPC 2.0 channel request with an id.") });
        return;
      }
      try {
        let result: unknown;
        if (raw.method === METHODS.CHANNEL_REGISTER) {
          if (channel) throw new RpcError("already_registered", "This connection already has a channel.");
          channel = this.register(raw.params, (params, written) => write({ method: NOTIFICATIONS.DIALOG_RESULT, params }, written));
          result = { channelId: channel.identity.info.channelId };
          // A reconnecting integrator learns which channel it owns before replay begins.
          write({ id, result });
          this.flushResults(channel);
          return;
        }
        if (!channel) throw new RpcError("not_registered", "Register this connection with channel/register first.");
        switch (raw.method) {
          case METHODS.DIALOG_OPEN: result = this.open(channel, raw.params); break;
          case METHODS.DIALOG_CLOSE: result = this.withdraw(channel, raw.params); break;
          case METHODS.DIALOG_STATUS: result = this.status(channel, raw.params); break;
          default: throw new RpcError("method_not_found", "This method is unavailable on a channel connection.");
        }
        write({ id, result });
      } catch (e) {
        const error = e instanceof RpcError ? rpcError(e.code, e.message)
          : rpcError("internal_error", "The daemon could not complete the channel request.");
        if (!(e instanceof RpcError)) this.hooks.log("Could not complete a channel request.");
        write({ id, error });
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
          write({ id: null, error: rpcError("limit_exceeded", "Channel message is too large.") });
          socket.end();
          return;
        }
        if (line.trim()) handle(line);
      }
      if (Buffer.byteLength(buffer) > CHANNEL_LINE_BYTES) {
        write({ id: null, error: rpcError("limit_exceeded", "Channel message is too large.") });
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
