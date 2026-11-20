// Applying events to items, and bounding item size.

import type { DeltaField, Item, ThreadEvent } from "../protocol/generated/types.js";

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
    if (!this.byId.has(item.id)) this.order.push(item.id);
    this.byId.set(item.id, item);
  }

  has(id: string): boolean {
    return this.byId.has(id);
  }

  get(id: string): Item | undefined {
    return this.byId.get(id);
  }

  /** Items oldest first, cloned. */
  snapshot(): Item[] {
    return this.order.map((id) => clone(this.byId.get(id)!));
  }

  items(): Item[] {
    return this.order.map((id) => this.byId.get(id)!);
  }
}
