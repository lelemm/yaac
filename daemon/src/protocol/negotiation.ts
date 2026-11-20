// Protocol versions and negotiation (docs/09-compatibility.md#negotiation).
//
// **The only place in yaacd where a protocol version number is read.** Everything else
// receives a `Negotiated` (which hides the number) and the `Dialect` selected for it, and asks
// the dialect for steps or checks capabilities. `tests/ban.rs` in yaac-protocol scans the rest
// of `daemon/src` to keep it that way.

import { PROTOCOL_MAX, PROTOCOL_MIN, PROTOCOL_VERSION, SCHEMA_HASH } from "./generated/constants.js";
import type { AgreedProtocol, ErrorObject, ProtocolRange } from "./generated/types.js";
import { Dialect } from "./dialect.js";

/** The versions this build speaks, `{ min, max }` on the wire. */
export function supportedRange(): ProtocolRange {
  return { min: PROTOCOL_MIN, max: PROTOCOL_MAX };
}

const agreedOf = new WeakMap<Negotiated, number>();

/** A successful negotiation. The agreed version is not readable from outside this module. */
export class Negotiated {
  constructor(
    agreed: number,
    private readonly local: ProtocolRange,
    private readonly remote: ProtocolRange,
  ) {
    agreedOf.set(this, agreed);
  }

  /** The client is older than this daemon (this daemon adapts to it). */
  peerIsOlder(): boolean {
    return this.remote.max < this.local.max;
  }

  /** The `protocol` member of the `initialize` result. */
  answer(): AgreedProtocol {
    return { min: this.local.min, max: this.local.max, agreed: agreedOf.get(this)! };
  }

  /** "protocol v1; peer speaks v1–v1". Display only (diagnostics). */
  diagnosticLabel(): string {
    return `protocol v${agreedOf.get(this)}; peer speaks v${this.remote.min}–v${this.remote.max}`;
  }
}

/** No common version. */
export class NegotiationError extends Error {
  constructor(
    private readonly local: ProtocolRange,
    private readonly remote: ProtocolRange,
  ) {
    super("no common protocol version");
  }

  /** Which side is too old, from the daemon's point of view. */
  tooOld(): "daemon" | "client" {
    return this.local.max < this.remote.min ? "daemon" : "client";
  }

  /** The frozen-core `protocol_unsupported` error object (byte-identical to the Rust side). */
  toErrorObject(): ErrorObject {
    const d = this.local;
    const c = this.remote;
    const tooOld = this.tooOld();
    const advice =
      tooOld === "daemon"
        ? "the daemon on this target is too old for this YAAC; update it"
        : "this YAAC is too old for the daemon on this target; update YAAC";
    return {
      code: "protocol_unsupported",
      message: `no common protocol version: the daemon speaks ${d.min}–${d.max}, the client speaks ${c.min}–${c.max}; ${advice}`,
      data: { daemon: { min: d.min, max: d.max }, client: { min: c.min, max: c.max }, tooOld },
    };
  }
}

function isVersion(v: unknown): v is number {
  return typeof v === "number" && Number.isInteger(v) && v >= 1 && v <= 65535;
}

/** Validates a wire `{ min, max }`; null when malformed. */
export function parseRange(wire: unknown): ProtocolRange | null {
  if (typeof wire !== "object" || wire === null) return null;
  const r = wire as Record<string, unknown>;
  if (!isVersion(r.min) || !isVersion(r.max)) return null;
  return { min: r.min, max: r.max };
}

/** Agreed = the lower of the two maxima, provided it is not below either minimum. */
export function negotiate(remote: ProtocolRange): Negotiated {
  const local = supportedRange();
  const agreed = Math.min(local.max, remote.max);
  if (agreed < local.min || agreed < remote.min || local.min > local.max || remote.min > remote.max) {
    throw new NegotiationError(local, remote);
  }
  return new Negotiated(agreed, local, remote);
}

/**
 * Every dialect, newest first, each with the frozen-baseline hash of its version. An older
 * version would be added here as `[n, () => new DialectVn(), SCHEMA_HASH_Vn]`, where
 * `DialectVn extends` the next-newer dialect and overrides only what differed.
 */
const ALL: ReadonlyArray<readonly [number, () => Dialect, string]> = [[PROTOCOL_VERSION, () => new Dialect(SCHEMA_HASH), SCHEMA_HASH]];

/** The dialect for a negotiated version. The only function here that compares a version. */
export function select(negotiated: Negotiated): Dialect {
  const agreed = agreedOf.get(negotiated);
  const entry = ALL.find(([v]) => v === agreed);
  if (!entry) throw new Error("no dialect for the agreed protocol version");
  return entry[1]();
}

/** For tests of the window rule: how many dialects exist (current plus at most three). */
export function dialectCount(): number {
  return ALL.length;
}
