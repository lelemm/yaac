// The dialect chain, server role (docs/09-compatibility.md#dialects).
//
// `Dialect` is the CURRENT protocol, complete and readable top to bottom. It translates
// between the version-free model the session layer uses and the wire at the agreed version.
// Each step is small and named so that an older version can override exactly one of them:
//
//   // dialect-v<n>.ts — only what v<n> did differently from the next-newer version
//   export class DialectVn extends Dialect {
//     protected override encodeSummaryFields(s: ThreadSummary): Record<string, unknown> { … }
//   }
//
// Steps take domain values in and produce raw JSON out (or the reverse); they never mention a
// version. A step that needs another step calls it through `this`, so an older dialect's
// overrides are honoured all the way down. Nothing here may read a version number.

import type {
  Capability,
  Item,
  ProviderDescriptor,
  ThreadEffective,
  ThreadEvent,
  ThreadEventParams,
  ThreadSummary,
} from "./generated/types.js";
import { METHODS, NOTIFICATIONS } from "./generated/constants.js";

/** Wire-level features of a version; a version that lacks one withholds the capabilities. */
export class FeatureSet {
  constructor(private readonly withheld: readonly Capability[] = []) {}
  static readonly FULL = new FeatureSet();

  fold(capabilities: readonly Capability[]): Capability[] {
    return capabilities.filter((c) => !this.withheld.includes(c));
  }

  allows(capability: Capability): boolean {
    return !this.withheld.includes(capability);
  }
}

type Json = Record<string, unknown>;

export class Dialect {
  constructor(private readonly hash: string) {}

  /** Hash of this version's frozen baseline schema (the `schemaHash` of `initialize`). */
  schemaHash(): string {
    return this.hash;
  }

  /** The wire-level features of this version. The current one has them all. */
  features(): FeatureSet {
    return FeatureSet.FULL;
  }

  // ── Inbound ────────────────────────────────────────────────────────────────────────────

  /** Wire request params → current-shape params. Current shapes are the wire shapes. */
  decodeParams(_method: string, wire: unknown): unknown {
    return wire;
  }

  // ── Outbound: results ──────────────────────────────────────────────────────────────────

  /** Current-shape result → wire result for `method`. */
  encodeResult(method: string, result: unknown): unknown {
    const r = result as Json;
    switch (method) {
      case METHODS.PROVIDER_LIST:
        return { ...r, providers: (r.providers as ProviderDescriptor[]).map((p) => this.encodeDescriptor(p)) };
      case METHODS.THREAD_SYNC:
        return r.changed ? { ...r, changed: (r.changed as ThreadSummary[]).map((s) => this.encodeSummary(s)) } : r;
      case METHODS.THREAD_READ:
        return { ...r, items: (r.items as Item[]).map((i) => this.encodeItem(i)) };
      case METHODS.THREAD_SUBSCRIBE:
        return this.encodeSubscribed(r);
      default:
        return result;
    }
  }

  protected encodeSubscribed(r: Json): unknown {
    return r.effective ? { ...r, effective: this.encodeEffective(r.effective as ThreadEffective) } : r;
  }

  // ── Outbound: notifications ────────────────────────────────────────────────────────────

  /** Current-shape notification params → wire params. */
  encodeNotification(method: string, params: unknown): unknown {
    const p = params as Json;
    switch (method) {
      case NOTIFICATIONS.THREAD_EVENT:
        return this.encodeEvent(params as ThreadEventParams);
      case NOTIFICATIONS.THREAD_UPDATED:
        return { ...p, thread: this.encodeSummary(p.thread as ThreadSummary) };
      case NOTIFICATIONS.PROVIDER_UPDATED:
        return { ...p, provider: this.encodeDescriptor(p.provider as ProviderDescriptor) };
      default:
        return params;
    }
  }

  /** One journaled event → wire `thread/event` params. */
  encodeEvent(params: ThreadEventParams): unknown {
    return { ...params, event: this.encodeEventBody(params.event) };
  }

  protected encodeEventBody(event: ThreadEvent): unknown {
    const e = event as unknown as Json;
    if ("item" in e && e.item) return { ...e, item: this.encodeItem(e.item as Item) };
    if (e.type === "settingsChanged" && e.effective) {
      return { ...e, effective: this.encodeEffective(e.effective as ThreadEffective) };
    }
    return event;
  }

  // ── Shared encoders ────────────────────────────────────────────────────────────────────

  encodeItem(item: Item): unknown {
    return item;
  }

  encodeSummary(summary: ThreadSummary): unknown {
    return summary;
  }

  encodeDescriptor(descriptor: ProviderDescriptor): unknown {
    return descriptor.capabilities
      ? { ...descriptor, capabilities: this.features().fold(descriptor.capabilities) }
      : descriptor;
  }

  encodeEffective(effective: ThreadEffective): unknown {
    return effective.capabilities
      ? { ...effective, capabilities: this.features().fold(effective.capabilities) }
      : effective;
  }
}
