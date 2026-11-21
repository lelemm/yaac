// The pull-request tools' handler (D121): each call is scoped to the calling thread's
// checkout, answers in short plain text an agent can read, and never merges or approves.

import { randomBytes } from "node:crypto";
import type { PrSummary } from "../protocol/generated/types.js";
import { RpcError } from "../protocol/rpc.js";
import type { ThreadRuntime } from "../session/thread.js";
import type { Forge, ThreadForge } from "./forge.js";
import type { ForgeAdapter } from "./types.js";

export class PrToolError extends Error {}

function line(p: PrSummary): string {
  const state = p.state === "open" && p.draft ? "draft" : p.state;
  const checks = p.checks && p.checks !== "none" ? `, checks ${p.checks}` : "";
  return `#${p.number} [${state}${checks}] ${p.title} (${p.head} → ${p.base}) ${p.url}`;
}

export class PrTools {
  constructor(private readonly forge: Forge) {}

  async call(rt: ThreadRuntime, tool: string, a: Record<string, unknown>): Promise<string> {
    try {
      const t = await this.forge.forThread(rt.id);
      switch (tool) {
        case "pr_list":
          return await this.list(t, a);
        case "pr_get":
          return await this.get(t, a);
        case "pr_create":
          return await this.create(t, a);
        case "pr_checks":
          return await this.checks(t, a);
        case "pr_rerun_checks":
          return await this.rerun(t, a);
        case "pr_comments":
          return await this.comments(t, a);
        case "pr_reply":
          return await this.reply(t, a);
        default:
          throw new PrToolError(`There is no yaac tool named ${tool}.`);
      }
    } catch (e) {
      if (e instanceof RpcError) throw new PrToolError(e.message);
      throw e instanceof PrToolError ? e : new PrToolError((e as Error)?.message ?? String(e));
    }
  }

  private async adapter(t: ThreadForge, cap: string): Promise<ForgeAdapter> {
    return this.forge.adapterFor(t, cap);
  }

  private async number(t: ThreadForge, a: Record<string, unknown>): Promise<number> {
    if (typeof a.number === "number" && a.number > 0) return a.number;
    const pr = await this.forge.currentPr(t);
    if (!pr) throw new PrToolError(`There is no pull request for ${t.branch ? `the branch ${t.branch}` : "this checkout (it is not on a branch)"}. Create one with pr_create.`);
    return pr.number;
  }

  private async list(t: ThreadForge, a: Record<string, unknown>): Promise<string> {
    const branch = typeof a.branch === "string" && a.branch ? a.branch : t.branch;
    if (!branch) throw new PrToolError("This checkout is not on a branch (detached HEAD); name one with `branch`.");
    let prs: PrSummary[];
    if (branch === t.branch) {
      const r = await this.forge.prs(t.rt.id, true);
      if (!r.access.capabilities.includes("prList")) throw new PrToolError(r.access.reason ?? "Pull requests cannot be listed from this machine.");
      if (r.error) throw new PrToolError(r.error);
      prs = r.prs;
    } else {
      const ad = await this.adapter(t, "prList");
      prs = await this.forge.limited(() => ad.list(branch));
    }
    if (prs.length === 0) return `No pull requests from ${branch} in ${t.ctx.repo!.slug}.`;
    return [`Pull requests from ${branch} in ${t.ctx.repo!.slug}:`, ...prs.map(line)].join("\n");
  }

  private async get(t: ThreadForge, a: Record<string, unknown>): Promise<string> {
    const n = await this.number(t, a);
    const ad = await this.adapter(t, "prGet");
    const d = await this.forge.limited(() => ad.get(n));
    const out = [line(d.pr)];
    if (d.reviewers.length) out.push(`Reviewers: ${d.reviewers.map((r) => `${r.name} (${r.state}${r.required ? ", required" : ""})`).join(", ")}`);
    else out.push("Reviewers: none yet");
    if (d.checks.length) out.push("Checks:", ...d.checks.map((c) => `- ${c.name}: ${c.state}${c.blocking ? " (blocking)" : ""}${c.url ? ` ${c.url}` : ""}`));
    else out.push("Checks: none");
    out.push("", "Description:", d.body.trim() || "(empty)");
    return out.join("\n");
  }

  private async create(t: ThreadForge, a: Record<string, unknown>): Promise<string> {
    const title = typeof a.title === "string" ? a.title.trim() : "";
    if (!title) throw new PrToolError("A title is required.");
    if (!t.branch) throw new PrToolError("This checkout is not on a branch (detached HEAD).");
    const existing = (await this.forge.prs(t.rt.id, true)).prs.find((p) => p.state === "open");
    if (existing) throw new PrToolError(`The branch ${t.branch} already has an open pull request: ${line(existing)}`);
    let base = typeof a.base === "string" && a.base ? a.base : null;
    if (!base) {
      const prep = await this.forge.prepare(t.rt.id);
      base = prep.defaultBranch ?? null;
    }
    if (!base) throw new PrToolError("The repository's default branch could not be told; pass `base`.");
    // An agent's call carries no operation id of its own: one per call, so an uncertain
    // outcome is recorded and reported, never repeated by yaacd.
    const opId = `tool-${randomBytes(8).toString("hex")}`;
    try {
      const r = await this.forge.create({ opId, threadId: t.rt.id, base, title, body: typeof a.body === "string" ? a.body : "", draft: a.draft === true });
      return `${r.pushed ? `Pushed ${t.branch} first. ` : ""}Created ${line(r.pr)}`;
    } catch (e) {
      if (e instanceof RpcError && e.code === "outcome_unknown") throw new PrToolError(`${e.message} Do not call pr_create again before checking pr_list.`);
      throw e;
    }
  }

  private async checks(t: ThreadForge, a: Record<string, unknown>): Promise<string> {
    const n = await this.number(t, a);
    const ad = await this.adapter(t, "prChecks");
    const d = await this.forge.limited(() => ad.get(n));
    const out = [`Checks of #${n}:`];
    if (d.checks.length === 0) out.push("(none)");
    for (const c of d.checks) out.push(`- ${c.name}: ${c.state}${c.blocking ? " (blocking)" : ""}${c.url ? ` ${c.url}` : ""}`);
    const failed = d.checks.some((c) => c.state === "failure");
    if (failed && a.log !== false) {
      const check = typeof a.check === "string" && a.check ? a.check : undefined;
      const log = await this.forge.limited(() => ad.failedLog(n, check));
      out.push("", log ? `Failed log:\n${log}` : "No log of a failed step was found.");
    }
    return out.join("\n");
  }

  private async rerun(t: ThreadForge, a: Record<string, unknown>): Promise<string> {
    const n = await this.number(t, a);
    const ad = await this.adapter(t, "prRerun");
    const runs = await this.forge.limited(() => ad.rerunFailed(n));
    return runs.length ? `Asked to run again: ${runs.join(", ")}. Check with pr_checks in a few minutes.` : `#${n} has no failed run to rerun.`;
  }

  private async comments(t: ThreadForge, a: Record<string, unknown>): Promise<string> {
    const n = await this.number(t, a);
    const ad = await this.adapter(t, "prComments");
    const all = await this.forge.limited(() => ad.comments(n));
    const threads = a.includeResolved === true ? all : all.filter((x) => !x.resolved);
    if (threads.length === 0) return `#${n} has no ${a.includeResolved === true ? "" : "unresolved "}review comments.`;
    const out: string[] = [];
    for (const th of threads) {
      out.push(`Thread ${th.id}${th.path ? ` on ${th.path}${th.line ? `:${th.line}` : ""}` : ""}${th.resolved ? " (resolved)" : ""}:`);
      for (const c of th.comments) out.push(`  ${c.author}: ${c.body.replace(/\n/g, "\n    ")}`);
    }
    return out.join("\n");
  }

  private async reply(t: ThreadForge, a: Record<string, unknown>): Promise<string> {
    const thread = typeof a.thread === "string" ? a.thread : "";
    if (!thread) throw new PrToolError("`thread` is required (an id from pr_comments).");
    const body = typeof a.body === "string" ? a.body.trim() : "";
    if (!body && a.resolve !== true) throw new PrToolError("Give a `body`, `resolve: true`, or both.");
    const n = await this.number(t, a);
    const ad = await this.adapter(t, "prReply");
    if (body) await this.forge.limited(() => ad.reply(n, thread, body));
    if (a.resolve === true) await this.forge.limited(() => ad.resolve(n, thread));
    return `${body ? "Replied" : ""}${body && a.resolve === true ? " and resolved" : a.resolve === true ? "Resolved" : ""} thread ${thread} on #${n}.`;
  }
}
