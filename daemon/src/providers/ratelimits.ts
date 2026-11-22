// Plan rate limits (D161): what a provider reports about the account it runs as and that
// account's usage windows. Each provider translates its harness's own shapes into these in a
// module of its own (`codex/ratelimits.ts`, `claude/ratelimits.ts`); this module holds what
// they share. Nothing here names a model or a plan.

import { createHash } from "node:crypto";
import type { RateLimitAccount, RateLimitWindow } from "../protocol/generated/types.js";

/** What a provider instance reports; yaacd adds the instance and kind. */
export interface RateLimitReport {
  account: RateLimitAccount | null;
  windows: RateLimitWindow[];
  /** When the harness last reported any of it (RFC 3339). */
  at: string;
}

/**
 * The account key: the provider kind and a digest of the harness's own account identifiers,
 * so the same account on two targets has the same key and the identifiers never leave in
 * clear. Null when the harness gave no identifier at all.
 */
export function accountKey(kind: string, parts: ReadonlyArray<string | null | undefined>): string | null {
  const known = parts.map((p) => (typeof p === "string" ? p.trim() : ""));
  if (known.every((p) => p === "")) return null;
  const digest = createHash("sha256").update([kind, ...known].join("\n")).digest("hex").slice(0, 16);
  return `${kind}:${digest}`;
}

/** A share in whole percent, 0–100. `fraction`: the harness reports 0–1. */
export function wholePercent(value: unknown, fraction = false): number | null {
  if (typeof value !== "number" || !Number.isFinite(value)) return null;
  const pct = fraction ? value * 100 : value;
  return Math.min(100, Math.max(0, Math.round(pct)));
}

/** RFC 3339 from Unix seconds. */
export function fromUnixSeconds(value: unknown): string | null {
  if (typeof value !== "number" || !Number.isFinite(value) || value <= 0) return null;
  return new Date(value * 1000).toISOString().replace(/\.\d{3}Z$/, "Z");
}

/** RFC 3339 (UTC, whole seconds) from an ISO 8601 string, or null. */
export function fromIso(value: unknown): string | null {
  if (typeof value !== "string") return null;
  const ms = Date.parse(value);
  if (!Number.isFinite(ms)) return null;
  return new Date(Math.round(ms / 1000) * 1000).toISOString().replace(/\.\d{3}Z$/, "Z");
}

/** A short label for a window of this many minutes: `5h`, `weekly`, `30d`. */
export function windowLabel(minutes: number | null | undefined): string {
  if (typeof minutes !== "number" || minutes <= 0) return "limit";
  if (minutes === 7 * 24 * 60) return "weekly";
  if (minutes % (24 * 60) === 0) return `${minutes / (24 * 60)}d`;
  if (minutes % 60 === 0) return `${minutes / 60}h`;
  return `${minutes}m`;
}

/** Replaces windows by id and adds new ones at the end, keeping the earlier order. */
export function mergeWindows(prev: readonly RateLimitWindow[], next: readonly RateLimitWindow[]): RateLimitWindow[] {
  const out = prev.map((w) => next.find((n) => n.id === w.id) ?? w);
  for (const n of next) if (!out.some((w) => w.id === n.id)) out.push(n);
  return out;
}

/** Whether two reports say the same (ignoring when they were made). */
export function sameReport(a: RateLimitReport | null, b: RateLimitReport | null): boolean {
  if (!a || !b) return a === b;
  return JSON.stringify([a.account, a.windows]) === JSON.stringify([b.account, b.windows]);
}

export function nowIso(now: number = Date.now()): string {
  return new Date(now).toISOString().replace(/\.\d{3}Z$/, "Z");
}
