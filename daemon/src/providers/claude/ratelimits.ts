// Claude Code rate limits (D161), from what the Agent SDK declares (sdk.d.ts 0.3.329, Claude
// Code 2.1.330; verified with live read-only calls):
//
// - `Query.accountInfo()` → `{ email?, organization?, subscriptionType?, apiProvider? }`:
//   who is signed in ("Claude Ultra" is a subscriptionType as reported).
// - `Query.usage_EXPERIMENTAL_MAY_CHANGE_DO_NOT_RELY_ON_THIS_API_YET({ skipBehaviors: true })`
//   → `{ rate_limits_available, rate_limits: { limits: [{ kind, group, percent (0–100),
//   resets_at (ISO 8601), scope?: { model?: { display_name }, surface?: { display_name } } }],
//   five_hour?, seven_day?, … { utilization (0–100), resets_at } } }`. Answers without a turn.
//   Experimental by the SDK's own word, so it is optional: when it is missing or fails, the
//   stream's events alone keep the windows.
// - The stream's `rate_limit_event` (`SDKRateLimitEvent`) → `rate_limit_info: { status,
//   rateLimitType ('five_hour' | 'seven_day' | 'seven_day_opus' | …), utilization (a
//   fraction, 0–1), resetsAt (Unix seconds) }`, sent with a turn when the numbers change.
//
// Window ids are shared by the two sources: `session` (the five-hour window), `weekly` (the
// account's seven-day window), and `weekly:<scope>` for a scoped one.

import type { AccountInfo, Options, SDKRateLimitInfo, SDKUserMessage } from "@anthropic-ai/claude-agent-sdk";
import type { RateLimitAccount, RateLimitWindow } from "../../protocol/generated/types.js";
import { accountKey, fromIso, fromUnixSeconds, mergeWindows, nowIso, sameReport, wholePercent, type RateLimitReport } from "../ratelimits.js";
import type { ClaudeSdk, SdkQuery } from "./sdk.js";

const KIND = "claude";
const SESSION_MINUTES = 5 * 60;
const WEEK_MINUTES = 7 * 24 * 60;

/** The part of the usage report this module reads (declared in sdk.d.ts; read loosely). */
interface UsageRow {
  kind?: unknown;
  percent?: unknown;
  resets_at?: unknown;
  scope?: { model?: { display_name?: unknown } | null; surface?: { display_name?: unknown } | null } | null;
}
interface UsageBucket {
  utilization?: unknown;
  resets_at?: unknown;
}
export interface ClaudeUsage {
  rate_limits_available?: unknown;
  rate_limits?: ({ limits?: UsageRow[] | null } & Record<string, unknown>) | null;
}

/** The query's experimental usage call, when it has one. */
type UsageCall = (opts: { skipBehaviors?: boolean }) => Promise<unknown>;
export function usageCall(q: SdkQuery): UsageCall | null {
  const fn = (q as unknown as Record<string, unknown>).usage_EXPERIMENTAL_MAY_CHANGE_DO_NOT_RELY_ON_THIS_API_YET;
  return typeof fn === "function" ? (fn as UsageCall).bind(q) : null;
}

function win(id: string, used: number, resetsAt: string | null, scope: string | null): RateLimitWindow {
  const weekly = id !== "session";
  return {
    id,
    label: weekly ? "weekly" : "5h",
    ...(scope ? { scope } : {}),
    usedPercent: used,
    ...(resetsAt ? { resetsAt } : {}),
    windowMinutes: weekly ? WEEK_MINUTES : SESSION_MINUTES,
  };
}

function text(v: unknown): string | null {
  return typeof v === "string" && v.trim() ? v.trim() : null;
}

/** `seven_day_opus` → `Opus` (the harness's own enum suffix, shown as a scope). */
function scopeOfType(t: string): string | null {
  const m = /^seven_day_(.+)$/.exec(t);
  if (!m) return null;
  const s = m[1]!.replace(/_/g, " ");
  return s[0]!.toUpperCase() + s.slice(1);
}

/** Window ids for a rate-limit type; null for a type that is not a usage window (overage). */
function idOfType(t: string): { id: string; scope: string | null } | null {
  if (t === "five_hour") return { id: "session", scope: null };
  if (t === "seven_day") return { id: "weekly", scope: null };
  if (t.includes("overage")) return null;
  const scope = scopeOfType(t);
  return scope ? { id: `weekly:${scope}`, scope } : null;
}

/** The windows of a usage report. Prefers the server's rows; falls back to the named buckets. */
export function usageWindows(u: ClaudeUsage): RateLimitWindow[] | null {
  const rl = u.rate_limits;
  if (u.rate_limits_available === false || !rl) return null;
  const out: RateLimitWindow[] = [];
  if (Array.isArray(rl.limits)) {
    for (const row of rl.limits) {
      const used = wholePercent(row.percent);
      const kind = text(row.kind);
      if (used === null || !kind) continue;
      const resets = fromIso(row.resets_at);
      const scope = text(row.scope?.model?.display_name) ?? text(row.scope?.surface?.display_name);
      if (kind === "session") out.push(win("session", used, resets, null));
      else if (scope) out.push(win(`weekly:${scope}`, used, resets, scope));
      else if (kind.startsWith("weekly")) out.push(win("weekly", used, resets, null));
    }
    return out;
  }
  for (const [name, bucket] of Object.entries(rl)) {
    const b = bucket as UsageBucket | null;
    if (!b || typeof b !== "object") continue;
    const used = wholePercent(b.utilization);
    const ids = idOfType(name);
    if (used === null || !ids) continue;
    out.push(win(ids.id, used, fromIso(b.resets_at), ids.scope));
  }
  return out;
}

/** The window a stream `rate_limit_event` speaks of, or null. */
export function eventWindow(info: SDKRateLimitInfo): RateLimitWindow | null {
  const ids = typeof info.rateLimitType === "string" ? idOfType(info.rateLimitType) : null;
  const used = wholePercent(info.utilization, true);
  if (!ids || used === null) return null;
  return win(ids.id, used, fromUnixSeconds(info.resetsAt), ids.scope);
}

/** Keeps the last account and windows of one Claude instance, and reports changes. */
export class ClaudeRateLimits {
  private account: RateLimitAccount | null = null;
  private windows: RateLimitWindow[] = [];
  private at: string | null = null;
  private last: RateLimitReport | null = null;
  /** When the harness last reported anything (ms). */
  lastSeen = 0;

  constructor(
    private readonly onChange: (report: RateLimitReport) => void,
    private readonly now: () => number = Date.now,
  ) {}

  setAccount(info: AccountInfo | null): void {
    if (!info) return;
    // Only a first-party sign-in has plan limits; keyed by the sign-in and its organization.
    const key = info.apiProvider && info.apiProvider !== "firstParty" ? null : accountKey(KIND, [info.email, info.organization]);
    const label = text(info.subscriptionType);
    const email = text(info.email);
    this.account = key ? { key, ...(label ? { label } : {}), ...(email ? { email } : {}) } : null;
    this.changed();
  }

  applyUsage(u: ClaudeUsage): void {
    const windows = usageWindows(u);
    if (!windows) return;
    this.windows = windows;
    this.seen();
  }

  applyEvent(info: SDKRateLimitInfo): void {
    const w = eventWindow(info);
    if (!w) return;
    this.windows = mergeWindows(this.windows, [w]);
    this.seen();
  }

  /** Reads the account and the usage report from an open query (bounded; failures dropped). */
  async readFrom(q: SdkQuery, timeoutMs: number, log: (line: string) => void): Promise<void> {
    const bound = <T>(p: Promise<T>): Promise<T> =>
      Promise.race([p, new Promise<never>((_, reject) => setTimeout(() => reject(new Error(`no answer within ${timeoutMs} ms`)), timeoutMs).unref())]);
    try {
      this.setAccount(await bound(q.accountInfo()));
    } catch (e) {
      log(`could not read the Claude account: ${(e as Error).message}`);
    }
    const usage = usageCall(q);
    if (!usage) return;
    try {
      this.applyUsage((await bound(usage({ skipBehaviors: true }))) as ClaudeUsage);
    } catch (e) {
      log(`could not read the Claude usage: ${(e as Error).message}`);
    }
  }

  report(): RateLimitReport | null {
    if (!this.at) return null;
    return { account: this.account, windows: this.windows, at: this.at };
  }

  private seen(): void {
    this.lastSeen = this.now();
    this.at = nowIso(this.lastSeen);
    this.changed();
  }

  private changed(): void {
    const r = this.report();
    if (!r || sameReport(r, this.last)) return;
    this.last = r;
    this.onChange(r);
  }
}

/**
 * Reads the account and usage with a short query of its own, as the probe does: no session
 * is kept, no tool may run, no prompt is sent. Used for a refresh when nothing newer arrived.
 */
export async function readWithOwnQuery(
  sdk: ClaudeSdk,
  options: Pick<Options, "cwd" | "pathToClaudeCodeExecutable" | "env">,
  limits: ClaudeRateLimits,
  log: (line: string) => void,
): Promise<void> {
  let end: () => void = () => {};
  const input: AsyncIterable<SDKUserMessage> = {
    [Symbol.asyncIterator]: () => ({
      next: () => new Promise<IteratorResult<SDKUserMessage>>((resolve) => (end = () => resolve({ value: undefined, done: true }))),
      return: async () => ({ value: undefined, done: true }),
    }),
  };
  const q = sdk.query({
    prompt: input,
    options: {
      ...options,
      persistSession: false,
      permissionMode: "default",
      canUseTool: async () => ({ behavior: "deny", message: "yaacd is only reading the rate limits" }),
    },
  });
  const drain = (async () => {
    try {
      for await (const _ of q) void _;
    } catch {
      /* closed below */
    }
  })();
  try {
    await limits.readFrom(q, 30_000, log);
  } finally {
    end();
    q.close();
    await Promise.race([drain, new Promise((r) => setTimeout(r, 2000).unref())]);
  }
}
