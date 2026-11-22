// Rate limits (D161): the Codex and Claude translators against fixtures of the verified
// shapes (synthetic values), account-key stability, updates, and `provider/rateLimits` end to
// end through a daemon driving a fake Codex app-server.

import { test } from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as path from "node:path";
import { accountKey, windowLabel, wholePercent } from "../src/providers/ratelimits.js";
import { CodexRateLimits, type CodexRateLimitsResponse } from "../src/providers/codex/ratelimits.js";
import { ClaudeRateLimits, eventWindow, usageWindows } from "../src/providers/claude/ratelimits.js";
import type { RateLimitReport } from "../src/providers/ratelimits.js";
import { codexProviderFactory } from "../src/providers/codex/index.js";
import { client, opId, startDaemon, tmpHome, until } from "./helpers.js";
import { FakeCodex } from "./fixtures/codex/fake-codex.js";

const T0 = Date.parse("2026-11-22T13:00:00Z");

// ── Fixtures: the shapes Codex 0.170.0 and Claude Code 2.1.330 answered with, values made up ──

const CODEX_ACCOUNT = { account: { type: "chatgpt", email: "someone@example.com", planType: "pro", workspaceName: null }, userId: "user-synthetic-1", requiresOpenaiAuth: true };

function codexRead(used: number, resetsAt = 1795794433): CodexRateLimitsResponse & Record<string, unknown> {
  const snapshot = {
    limitId: "codex",
    limitName: null,
    normalModelSlug: null,
    primary: { usedPercent: used, windowDurationMins: 10080, resetsAt },
    secondary: null,
    credits: { hasCredits: false, unlimited: false, balance: "0" },
    individualLimit: null,
    spendControlReached: false,
    planType: "pro",
    rateLimitReachedType: null,
  };
  return { ordinaryUsageAllowed: true, rateLimits: snapshot, rateLimitsByLimitId: { codex: snapshot }, rateLimitResetCredits: { availableCount: 0, credits: null }, accountId: "acct-synthetic-1", rateLimitUpsell: null };
}

const CLAUDE_ACCOUNT = { email: "someone@example.com", organization: "Someone's Organization", subscriptionType: "Claude Ultra", apiProvider: "firstParty" as const };

const CLAUDE_USAGE = {
  subscription_type: "max",
  rate_limits_available: true,
  rate_limits: {
    five_hour: { utilization: 100, resets_at: "2026-11-22T13:53:00.123456+00:00" },
    seven_day: { utilization: 55, resets_at: "2026-11-24T13:00:00.968742+00:00" },
    seven_day_opus: null,
    limits: [
      { kind: "session", group: "session", percent: 100, severity: "critical", resets_at: "2026-11-22T13:53:00.123456+00:00", scope: null, is_active: true },
      { kind: "weekly_all", group: "weekly", percent: 55, severity: "normal", resets_at: "2026-11-24T13:00:00.968742+00:00", scope: null, is_active: false },
      { kind: "weekly_scoped", group: "weekly", percent: 31, severity: "normal", resets_at: "2026-11-24T12:59:59.968853+00:00", scope: { model: { id: null, display_name: "Fable" }, surface: null }, is_active: false },
    ],
  },
};

function sink(): { reports: RateLimitReport[]; on: (r: RateLimitReport) => void } {
  const reports: RateLimitReport[] = [];
  return { reports, on: (r) => reports.push(r) };
}

// ── Shared ───────────────────────────────────────────────────────────────────────────────

test("rate limits: account keys are stable, opaque, and per kind", () => {
  const a = accountKey("codex", ["acct", "user"]);
  assert.equal(a, accountKey("codex", ["acct", "user"]));
  assert.notEqual(a, accountKey("claude", ["acct", "user"]));
  assert.notEqual(a, accountKey("codex", ["acct2", "user"]));
  assert.match(a!, /^codex:[0-9a-f]{16}$/);
  assert.ok(!a!.includes("user"));
  assert.equal(accountKey("codex", [null, ""]), null);
  assert.equal(windowLabel(300), "5h");
  assert.equal(windowLabel(10080), "weekly");
  assert.equal(windowLabel(1440), "1d");
  assert.equal(wholePercent(0.874, true), 87);
  assert.equal(wholePercent(140), 100);
});

// ── Codex ────────────────────────────────────────────────────────────────────────────────

test("rate limits (Codex): a read gives the account and its weekly window", () => {
  const s = sink();
  const l = new CodexRateLimits(s.on, () => T0);
  l.setAccount(CODEX_ACCOUNT);
  assert.equal(s.reports.length, 0, "nothing before the limits were read (the key needs the account id)");
  l.applyRead(codexRead(55));
  const r = s.reports.at(-1)!;
  assert.equal(r.account?.label, "Pro");
  assert.equal(r.account?.email, "someone@example.com");
  assert.equal(r.account?.key, accountKey("codex", ["acct-synthetic-1", "user-synthetic-1"]));
  assert.deepEqual(r.windows, [{ id: "primary", label: "weekly", usedPercent: 55, resetsAt: "2026-11-27T15:47:13Z", windowMinutes: 10080 }]);
  assert.equal(r.at, "2026-11-22T13:00:00Z");
});

test("rate limits (Codex): a sparse update merges, nulls clear nothing, and the same values are not reported twice", () => {
  const s = sink();
  const l = new CodexRateLimits(s.on, () => T0);
  l.setAccount(CODEX_ACCOUNT);
  l.applyRead({ ...codexRead(10), rateLimits: { ...codexRead(10).rateLimits!, secondary: { usedPercent: 40, windowDurationMins: 300, resetsAt: 1795360000 } } });
  const n = s.reports.length;
  l.applyUpdate({ rateLimits: { limitId: "codex", primary: { usedPercent: 12, windowDurationMins: 10080, resetsAt: 1795794433 }, secondary: null, planType: null } });
  const r = s.reports.at(-1)!;
  assert.equal(s.reports.length, n + 1);
  assert.deepEqual(r.windows.map((w) => [w.id, w.label, w.usedPercent]), [["primary", "weekly", 12], ["secondary", "5h", 40]]);
  assert.equal(r.account?.label, "Pro", "a null plan in an update keeps the plan");
  l.applyUpdate({ rateLimits: { limitId: "codex", primary: { usedPercent: 12, windowDurationMins: 10080, resetsAt: 1795794433 } } });
  assert.equal(s.reports.length, n + 1);
});

test("rate limits (Codex): another bucket is a scoped window", () => {
  const s = sink();
  const l = new CodexRateLimits(s.on, () => T0);
  l.setAccount(CODEX_ACCOUNT);
  const read = codexRead(20);
  read.rateLimitsByLimitId = { ...read.rateLimitsByLimitId, other: { limitId: "other", limitName: "Spark", primary: { usedPercent: 90, windowDurationMins: 300, resetsAt: 1795360000 } } };
  l.applyRead(read);
  const w = s.reports.at(-1)!.windows;
  assert.deepEqual(w.map((x) => [x.id, x.scope ?? null, x.usedPercent]), [["primary", null, 20], ["other:primary", "Spark", 90]]);
});

test("rate limits (Codex): the same sign-in on two targets has one key; another workspace does not", () => {
  const keys = ["acct-synthetic-1", "acct-synthetic-1", "acct-synthetic-2"].map((acct) => {
    const s = sink();
    const l = new CodexRateLimits(s.on, () => T0);
    l.setAccount(CODEX_ACCOUNT);
    l.applyRead({ ...codexRead(1), accountId: acct });
    return s.reports.at(-1)!.account!.key;
  });
  assert.equal(keys[0], keys[1]);
  assert.notEqual(keys[0], keys[2]);
});

// ── Claude ───────────────────────────────────────────────────────────────────────────────

test("rate limits (Claude): the usage report's rows become the 5h, weekly and scoped windows", () => {
  const w = usageWindows(CLAUDE_USAGE)!;
  assert.deepEqual(w, [
    { id: "session", label: "5h", usedPercent: 100, resetsAt: "2026-11-22T13:53:00Z", windowMinutes: 300 },
    { id: "weekly", label: "weekly", usedPercent: 55, resetsAt: "2026-11-24T13:00:01Z", windowMinutes: 10080 },
    { id: "weekly:Fable", label: "weekly", scope: "Fable", usedPercent: 31, resetsAt: "2026-11-24T13:00:00Z", windowMinutes: 10080 },
  ]);
  // Without the rows (an older CLI), the named buckets.
  const { limits: _l, ...older } = CLAUDE_USAGE.rate_limits;
  assert.deepEqual(usageWindows({ ...CLAUDE_USAGE, rate_limits: older })!.map((x) => [x.id, x.usedPercent]), [["session", 100], ["weekly", 55]]);
  assert.equal(usageWindows({ rate_limits_available: false, rate_limits: null }), null);
});

test("rate limits (Claude): a stream event is a fraction with Unix-second resets, merged by window", () => {
  const ev = { status: "allowed_warning" as const, resetsAt: 1795618800, rateLimitType: "seven_day" as const, utilization: 0.87 };
  assert.deepEqual(eventWindow(ev), { id: "weekly", label: "weekly", usedPercent: 87, resetsAt: "2026-11-25T15:00:00Z", windowMinutes: 10080 });
  assert.equal(eventWindow({ status: "rejected", rateLimitType: "overage", utilization: 1 }), null);
  assert.equal(eventWindow({ status: "allowed", rateLimitType: "seven_day_opus", utilization: 0.5 })?.id, "weekly:Opus");

  const s = sink();
  const l = new ClaudeRateLimits(s.on, () => T0);
  l.setAccount(CLAUDE_ACCOUNT);
  l.applyUsage(CLAUDE_USAGE);
  l.applyEvent(ev);
  const r = s.reports.at(-1)!;
  assert.equal(r.account?.label, "Claude Ultra");
  assert.equal(r.account?.key, accountKey("claude", [CLAUDE_ACCOUNT.email, CLAUDE_ACCOUNT.organization]));
  assert.deepEqual(r.windows.map((w) => [w.id, w.usedPercent]), [["session", 100], ["weekly", 87], ["weekly:Fable", 31]]);
});

test("rate limits (Claude): a cloud-provider sign-in has no account to group by", () => {
  const s = sink();
  const l = new ClaudeRateLimits(s.on, () => T0);
  l.setAccount({ apiProvider: "bedrock" });
  l.applyEvent({ status: "allowed", rateLimitType: "five_hour", utilization: 0.1, resetsAt: 1795360000 });
  assert.equal(s.reports.at(-1)!.account, null);
});

test("rate limits (Claude): reads account and usage from an open query, tolerating a query without the usage call", async () => {
  const s = sink();
  const l = new ClaudeRateLimits(s.on, () => T0);
  const q: any = { accountInfo: async () => CLAUDE_ACCOUNT, usage_EXPERIMENTAL_MAY_CHANGE_DO_NOT_RELY_ON_THIS_API_YET: async (o: any) => (assert.equal(o.skipBehaviors, true), CLAUDE_USAGE) };
  await l.readFrom(q, 1000, () => {});
  assert.equal(s.reports.at(-1)!.windows.length, 3);
  const l2 = new ClaudeRateLimits(s.on, () => T0);
  const logged: string[] = [];
  await l2.readFrom({ accountInfo: async () => CLAUDE_ACCOUNT } as any, 1000, (x) => logged.push(x));
  assert.equal(l2.report(), null);
  assert.deepEqual(logged, []);
});

// ── End to end: provider/rateLimits through a daemon ─────────────────────────────────────

test("rate limits: provider/rateLimits answers, notifies on an update, and holds refreshes", async () => {
  const home = fs.realpathSync(tmpHome("yaac-limits-"));
  const fake = await FakeCodex.start(path.join(home, "codex.sock"));
  let used = 30;
  fake.handlers.set("account/read", () => CODEX_ACCOUNT);
  fake.handlers.set("account/rateLimits/read", () => codexRead(used));
  fake.handlers.set("model/list", () => ({ data: [{ id: "m", model: "m", displayName: "M", isDefault: true }], nextCursor: null }));
  fake.handlers.set("thread/list", () => ({ data: [], nextCursor: null }));
  const daemon = await startDaemon(home, { factories: [codexProviderFactory], env: { YAAC_FAKE_PROVIDER: "0", YAAC_CODEX_PROVIDER: "0" } });
  const c = await client(daemon);
  try {
    const added: any = await c.request("providerInstance/add", { opId: opId(), kind: "codex", endpoint: { socket: fake.socket } });
    const id = added.instance.id;
    const providers: any = await until(async () => {
      const l: any = await c.request("provider/list", {});
      const p = l.providers.find((x: any) => x.instance === id);
      return p?.status === "ready" ? l : null;
    }, "ready");
    assert.ok(providers.providers.find((x: any) => x.instance === id).capabilities.includes("rateLimits"));
    const first: any = await until(async () => {
      const r: any = await c.request("provider/rateLimits", {});
      return r.instances.find((i: any) => i.instance === id && i.windows.length) ? r : null;
    }, "limits");
    const mine = first.instances.find((i: any) => i.instance === id);
    assert.equal(mine.kind, "codex");
    assert.equal(mine.windows[0].usedPercent, 30);
    assert.match(mine.account.key, /^codex:/);
    // A rolling update from Codex is passed on.
    fake.push({ method: "account/rateLimits/updated", params: { rateLimits: { limitId: "codex", primary: { usedPercent: 47, windowDurationMins: 10080, resetsAt: 1795794433 } } } });
    const note: any = await until(() => c.notifications.find((n) => n.method === "provider/rateLimits/updated" && n.params.windows[0]?.usedPercent === 47), "update");
    assert.equal(note.params.instance, id);
    // Refresh reads Codex once; a second refresh within five minutes does not.
    const reads = fake.requests("account/rateLimits/read").length;
    used = 61;
    await c.request("provider/rateLimits", { refresh: true });
    await until(() => fake.requests("account/rateLimits/read").length === reads + 1, "refresh read");
    await until(() => c.notifications.find((n) => n.method === "provider/rateLimits/updated" && n.params.windows[0]?.usedPercent === 61), "refreshed");
    await c.request("provider/rateLimits", { refresh: true });
    await new Promise((r) => setTimeout(r, 200));
    assert.equal(fake.requests("account/rateLimits/read").length, reads + 1);
  } finally {
    await c.close();
    await daemon.stop("test");
    await fake.close();
    fs.rmSync(home, { recursive: true, force: true });
  }
});
