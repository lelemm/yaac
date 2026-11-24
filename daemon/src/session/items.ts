// Applying events to items, and bounding item size.

import type { DeltaField, Item, ThreadEvent, TimelinePrompt } from "../protocol/generated/types.js";

/** Bound on a tool call's output kept inline; the rest is read on demand. */
export const OUTPUT_HEAD_BYTES = 16 * 1024;

export function applyDelta(item: Item, field: DeltaField, delta: string): void {
  const it = item as unknown as Record<string, unknown>;
  switch (field) {
    case "text":
      it.text = `${typeof it.text === "string" ? it.text : ""}${delta}`;
      return;
    case "reasoning":
      it.reasoning = `${typeof it.reasoning === "string" ? it.reasoning : ""}${delta}`;
      return;
    case "output":
      it.output = `${typeof it.output === "string" ? it.output : ""}${delta}`;
      return;
    default:
      // An unknown field: nothing this build can append to (forward compatibility).
      return;
  }
}

/** The item id an event concerns, if any. */
export function eventItemId(event: ThreadEvent): string | null {
  const e = event as unknown as Record<string, unknown>;
  if (e.item && typeof (e.item as Item).id === "string") return (e.item as Item).id;
  if (typeof e.itemId === "string") return e.itemId;
  return null;
}

/** Cuts a UTF-8 string to at most `max` bytes, on a character boundary. */
export function utf8Head(text: string, max: number): string {
  const buf = Buffer.from(text, "utf8");
  if (buf.length <= max) return text;
  let end = max;
  while (end > 0 && (buf[end]! & 0xc0) === 0x80) end--;
  return buf.subarray(0, end).toString("utf8");
}

export function clone<T>(value: T): T {
  return structuredClone(value);
}

/** Ordered items of a thread, updated by events. */
export class Projection {
  private order: string[] = [];
  private byId = new Map<string, Item>();
  private offsets = new Map<string, number>();
  private prompts = new Map<string, TimelinePrompt>();
  private firstPromptItem?: string;

  apply(event: ThreadEvent): void {
    const e = event as unknown as Record<string, unknown>;
    switch (event.type) {
      case "itemStarted":
      case "itemUpdated":
      case "itemCompleted":
        this.upsert(clone((e.item as Item)));
        return;
      case "itemDelta": {
        const item = this.byId.get(e.itemId as string);
        if (item) applyDelta(item, e.field as DeltaField, e.delta as string);
        return;
      }
      case "requestChanged": {
        const item = this.byId.get(e.itemId as string) as Record<string, unknown> | undefined;
        if (item) {
          item.state = e.state;
          if (e.resolution !== undefined) item.resolution = e.resolution;
          // A request that can no longer be answered is finished; its `state` says how.
          if (e.state === "resolved" || e.state === "expired" || e.state === "unknown") item.status = "completed";
        }
        return;
      }
      default:
        return;
    }
  }

  upsert(item: Item): void {
    if (!this.byId.has(item.id)) {
      this.offsets.set(item.id, this.order.length);
      this.order.push(item.id);
    }
    this.byId.set(item.id, item);
    if (item.kind === "userMessage" || item.kind === "meshReceived") {
      this.firstPromptItem ??= item.id;
      const prompt = timelinePrompt(item);
      if (prompt) this.prompts.set(item.id, prompt);
      else this.prompts.delete(item.id);
    }
  }

  has(id: string): boolean {
    return this.byId.has(id);
  }

  get(id: string): Item | undefined {
    return this.byId.get(id);
  }

  /** Iterate backward from the live end or immediately before an item. */
  *historyBefore(before?: string): IterableIterator<Item> {
    const end = before === undefined ? this.order.length : this.offsets.get(before);
    if (end === undefined) throw new Error("history anchor missing");
    for (let i = end - 1; i >= 0; i--) yield this.byId.get(this.order[i]!)!;
  }

  get length(): number { return this.order.length; }

  offset(id: string): number | undefined { return this.offsets.get(id); }

  /** Beginning of the contiguous turn containing this item. */
  turnStart(offset: number): number {
    const turn = this.byId.get(this.order[offset]!)?.turnId;
    while (offset > 0 && this.byId.get(this.order[offset - 1]!)?.turnId === turn) offset--;
    return offset;
  }

  *historyFrom(start: number): IterableIterator<Item> {
    for (let i = start; i < this.order.length; i++) yield this.byId.get(this.order[i]!)!;
  }

  timeline(spawned: boolean): TimelinePrompt[] {
    const first = spawned && this.firstPromptItem ? this.get(this.firstPromptItem) : undefined;
    return first?.kind === "meshReceived"
      ? [promptTitle(first, first.message.text), ...this.prompts.values()]
      : [...this.prompts.values()];
  }

  /** Items oldest first, cloned. */
  snapshot(): Item[] {
    return this.order.map((id) => clone(this.byId.get(id)!));
  }

  items(): Item[] {
    return this.order.map((id) => this.byId.get(id)!);
  }
}

function timelinePrompt(item: Item): TimelinePrompt | null {
  if (item.kind !== "userMessage") return null;
  const parts = item.parts;
  const typed = parts.flatMap((part) => part.type === "text" && part.role !== "context" ? [part.text] : []).find((text) => text.trim());
  if (!typed && !parts.some((p) => p.type === "image" || p.type === "file") && parts.some((p) => p.type === "text" && p.role === "context")) return null;
  return promptTitle(item, typed ?? "Attachments");
}

function promptTitle(item: Item, text: string): TimelinePrompt {
  const line = (text.split(/\r?\n/).map((line) => line.trim()).find(Boolean) ?? "").replace(/^#+/, "").trim().replace(/\s+/g, " ");
  const chars = Array.from(line);
  return { itemId: item.id, title: chars.length > 80 ? chars.slice(0, 79).join("") + "…" : line, at: item.startedAt ?? item.completedAt };
}
