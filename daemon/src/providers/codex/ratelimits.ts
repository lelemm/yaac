// Codex rate limits (D161), from the app-server's own account surface (verified on
// codex-cli 0.170.0, `codex app-server generate-ts`, and live reads):
//
// - `account/read` → `{ account: { type: "chatgpt", email, planType, workspaceName } | …,
//   userId, requiresOpenaiAuth }`: who is signed in.
// - `account/rateLimits/read` (`{ excludeResetCreditDetails: true }` for a background read) →
//   `{ rateLimits: RateLimitSnapshot, rateLimitsByLimitId: { [limitId]: RateLimitSnapshot },
//   accountId, … }`. A snapshot has `limitId`, `limitName`, `primary` and `secondary`
//   windows `{ usedPercent (0–100), windowDurationMins, resetsAt (Unix seconds) }`, `planType`.
// - `account/rateLimits/updated` → `{ rateLimits: RateLimitSnapshot }`, a *sparse* rolling
//   update: merged into the last read; null values never clear what was seen.
//
// The `rateLimits` snapshot is the account's main bucket; any other bucket in
// `rateLimitsByLimitId` is scoped, labelled with its `limitName` (or id).

import type { RateLimitAccount, RateLimitWindow } from "../../protocol/generated/types.js";
import { accountKey, fromUnixSeconds, mergeWindows, nowIso, sameReport, wholePercent, windowLabel, type RateLimitReport } from "../ratelimits.js";

export interface CodexRateLimitWindow {
  usedPercent?: number | null;
  windowDurationMins?: number | null;
  resetsAt?: number | null;
}

export interface CodexRateLimitSnapshot {
  limitId?: string | null;
  limitName?: string | null;
  primary?: CodexRateLimitWindow | null;
  secondary?: CodexRateLimitWindow | null;
  planType?: string | null;
}

export interface CodexRateLimitsResponse {
  rateLimits?: CodexRateLimitSnapshot | null;
  rateLimitsByLimitId?: Record<string, CodexRateLimitSnapshot | null | undefined> | null;
  accountId?: string | null;
}

export interface CodexAccountResponse {
  account?: { type?: string; email?: string | null; planType?: string | null } | null;
  userId?: string | null;
}

const KIND = "codex";

function cap(s: string): string {
  return s ? s[0]!.toUpperCase() + s.slice(1) : s;
}

function windowOf(bucket: string | null, slot: "primary" | "secondary", w: CodexRateLimitWindow, scope: string | null): RateLimitWindow | null {
  const used = wholePercent(w.usedPercent);
  if (used === null) return null;
  const minutes = typeof w.windowDurationMins === "number" && w.windowDurationMins > 0 ? Math.round(w.windowDurationMins) : null;
  const resetsAt = fromUnixSeconds(w.resetsAt);
  return {
    id: bucket ? `${bucket}:${slot}` : slot,
    label: windowLabel(minutes),
    ...(scope ? { scope } : {}),
    usedPercent: used,
    ...(resetsAt ? { resetsAt } : {}),
    ...(minutes ? { windowMinutes: minutes } : {}),
  };
}

/** The windows of one snapshot. `scoped`: a bucket other than the account's main one. */
export function snapshotWindows(s: CodexRateLimitSnapshot, scoped: boolean): RateLimitWindow[] {
  const bucket = scoped ? (s.limitId ?? s.limitName ?? null) : null;
  const scope = scoped ? (s.limitName ?? s.limitId ?? null) : null;
  const out: RateLimitWindow[] = [];
  for (const slot of ["primary", "secondary"] as const) {
    const w = s[slot];
    if (w) {
      const win = windowOf(bucket, slot, w, scope);
      if (win) out.push(win);
    }
  }
  return out;
}

/** Keeps the last account and limits of one Codex instance, and reports changes. */
export class CodexRateLimits {
  private account: CodexAccountResponse | null = null;
  private accountId: string | null = null;
  private plan: string | null = null;
  private mainLimitId: string | null = null;
  private windows: RateLimitWindow[] = [];
  private at: string | null = null;
  private last: RateLimitReport | null = null;
  /** When the limits were last read (ms), for refresh throttling. */
  lastRead = 0;

  constructor(
    private readonly onChange: (report: RateLimitReport) => void,
    private readonly now: () => number = Date.now,
  ) {}

  /** `account/read`'s answer (each probe). */
  setAccount(acc: CodexAccountResponse | null): void {
    this.account = acc;
    const plan = acc?.account?.planType;
    if (typeof plan === "string" && plan) this.plan = plan;
    this.changed();
  }

  /** `account/rateLimits/read`'s answer: replaces the windows. */
  applyRead(r: CodexRateLimitsResponse): void {
    this.lastRead = this.now();
    if (typeof r.accountId === "string" && r.accountId) this.accountId = r.accountId;
    const main = r.rateLimits ?? null;
    this.mainLimitId = main?.limitId ?? null;
    if (typeof main?.planType === "string" && main.planType) this.plan = main.planType;
    const windows = main ? snapshotWindows(main, false) : [];
    for (const [id, s] of Object.entries(r.rateLimitsByLimitId ?? {})) {
      if (!s || id === this.mainLimitId || (s.limitId ?? id) === this.mainLimitId) continue;
      windows.push(...snapshotWindows({ ...s, limitId: s.limitId ?? id }, true));
    }
    this.windows = windows;
    this.at = nowIso(this.now());
    this.changed();
  }

  /** `account/rateLimits/updated`: a sparse update of one bucket. */
  applyUpdate(p: { rateLimits?: CodexRateLimitSnapshot | null }): void {
    const s = p.rateLimits;
    if (!s) return;
    const scoped = !!s.limitId && !!this.mainLimitId && s.limitId !== this.mainLimitId;
    if (!scoped && typeof s.planType === "string" && s.planType) this.plan = s.planType;
    const next = snapshotWindows(s, scoped);
    if (next.length === 0) return;
    this.windows = mergeWindows(this.windows, next);
    this.at = nowIso(this.now());
    this.changed();
  }

  report(): RateLimitReport | null {
    // Nothing until the limits were read: the key needs the account id that read carries.
    if (!this.at) return null;
    const a = this.account?.account;
    const email = typeof a?.email === "string" && a.email ? a.email : null;
    // The ChatGPT account (workspace) the limits belong to, and the user: the same pair on
    // every target signed in to it.
    const key = accountKey(KIND, [this.accountId, this.account?.userId ?? email]);
    const account: RateLimitAccount | null = key ? { key, ...(this.plan ? { label: cap(this.plan) } : {}), ...(email ? { email } : {}) } : null;
    return { account, windows: this.windows, at: this.at ?? nowIso(this.now()) };
  }

  private changed(): void {
    const r = this.report();
    if (!r || sameReport(r, this.last)) return;
    this.last = r;
    this.onChange(r);
  }
}
