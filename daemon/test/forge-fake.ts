// A fake `gh` and `az` (and Azure DevOps over HTTPS) for the forge tests. `git` runs for real.
// The fake answers the exact commands the adapters send — the shapes are the ones checked
// against gh 2.45.0 (and, for Azure DevOps, the 7.1 REST reference) — and records every call.

import { run, type RunResult } from "../src/util/exec.js";
import type { CliRunner, HttpClient } from "../src/forge/types.js";

export interface FakePr {
  number: number;
  title: string;
  state: "OPEN" | "MERGED" | "CLOSED";
  isDraft?: boolean;
  headRefName: string;
  baseRefName: string;
  owner?: string;
  body?: string;
  checks?: Array<{ name: string; status: string; conclusion: string; runId?: string }>;
}

export interface FakeThread {
  id: string;
  isResolved: boolean;
  path?: string;
  line?: number;
  comments: Array<{ id: string; author: string; body: string }>;
}

export class FakeGh {
  calls: string[][] = [];
  installed = true;
  loggedIn = true;
  /** Hosts `gh auth status` lists. */
  hosts = ["github.com"];
  prs: FakePr[] = [];
  threads: FakeThread[] = [];
  logs: Record<string, string> = {};
  reruns: string[] = [];
  parent: { owner: string; repo: string } | null = null;
  /** The next `pr create` times out (it may or may not have happened). */
  createTimesOut = false;
  createdTimedOut = false;
  nextNumber = 100;
  defaultBranch = "main";

  private ok(stdout: string): RunResult {
    return { code: 0, stdout, stderr: "" };
  }

  private json(prs: FakePr[], repo: string): unknown[] {
    return prs.map((p) => ({
      number: p.number,
      title: p.title,
      state: p.state,
      isDraft: !!p.isDraft,
      url: `https://github.com/${repo}/pull/${p.number}`,
      headRefName: p.headRefName,
      baseRefName: p.baseRefName,
      statusCheckRollup: (p.checks ?? []).map((c) => ({
        __typename: "CheckRun",
        name: c.name,
        status: c.status,
        conclusion: c.conclusion,
        workflowName: "CI",
        detailsUrl: c.runId ? `https://github.com/${repo}/actions/runs/${c.runId}/job/1` : "https://example.invalid/check",
      })),
      author: { login: "lelemm" },
      updatedAt: `2026-11-20T10:00:${String(p.number % 60).padStart(2, "0")}Z`,
      headRepositoryOwner: { login: p.owner ?? "acme" },
      body: p.body ?? "",
      latestReviews: [{ author: { login: "reviewer1" }, state: "APPROVED" }],
      reviewRequests: [{ login: "reviewer2" }],
    }));
  }

  handle(args: string[]): RunResult {
    this.calls.push(args);
    if (!this.installed) return { code: null, stdout: "", stderr: "", failure: "ENOENT" };
    const flag = (name: string) => {
      const i = args.indexOf(name);
      return i >= 0 ? args[i + 1] : undefined;
    };
    const repo = (flag("-R") ?? "").split("/").slice(-2).join("/");
    const [a, b] = args;
    if (a === "auth" && b === "status") {
      if (!this.loggedIn) return { code: 1, stdout: "", stderr: "You are not logged into any GitHub hosts. To log in, run: gh auth login\n" };
      return this.ok(this.hosts.map((h) => `${h}\n  ✓ Logged in to ${h} account lelemm (keyring)\n  - Active account: true\n`).join(""));
    }
    if (!this.loggedIn) return { code: 4, stdout: "", stderr: "To get started with GitHub CLI, please run:  gh auth login\n" };
    if (a === "pr" && b === "list") {
      const head = flag("--head");
      return this.ok(JSON.stringify(this.json(this.prs.filter((p) => !head || p.headRefName === head), repo)));
    }
    if (a === "pr" && b === "view") {
      const n = Number(args[2]);
      const p = this.prs.find((x) => x.number === n);
      if (!p) return { code: 1, stdout: "", stderr: `GraphQL: Could not resolve to a PullRequest with the number of ${n}.` };
      return this.ok(JSON.stringify(this.json([p], repo)[0]));
    }
    if (a === "pr" && b === "create") {
      const number = this.nextNumber++;
      const head = flag("--head")!;
      const pr: FakePr = { number, title: flag("--title")!, state: "OPEN", isDraft: args.includes("--draft"), headRefName: head.split(":").pop()!, baseRefName: flag("--base")!, body: flag("--body") ?? "", owner: head.includes(":") ? head.split(":")[0] : "acme" };
      if (this.createTimesOut) {
        this.createTimesOut = false;
        if (this.createdTimedOut) this.prs.push(pr);
        return { code: null, stdout: "", stderr: "", failure: "timeout" };
      }
      this.prs.push(pr);
      return this.ok(`\nCreating pull request for ${head} into ${pr.baseRefName} in ${repo}\n\nhttps://github.com/${repo}/pull/${number}\n`);
    }
    if (a === "repo" && b === "view") {
      if (args.join(" ").includes("parent")) return this.ok(JSON.stringify({ parent: this.parent ? { name: this.parent.repo, owner: { login: this.parent.owner } } : null }));
      return this.ok(JSON.stringify({ defaultBranchRef: { name: this.defaultBranch } }));
    }
    if (a === "run" && b === "view") return this.ok(this.logs[args[2]!] ?? "");
    if (a === "run" && b === "rerun") {
      this.reruns.push(args[2]!);
      return this.ok("");
    }
    if (a === "api") {
      const q = args.find((x) => x.startsWith("query="))!;
      const field = (k: string) => args.find((x) => x.startsWith(`${k}=`))?.slice(k.length + 1);
      if (q.includes("reviewThreads")) {
        const nodes = this.threads.map((t) => ({ id: t.id, isResolved: t.isResolved, path: t.path, line: t.line ?? null, comments: { nodes: t.comments.map((c) => ({ id: c.id, author: { login: c.author }, body: c.body, createdAt: "2026-11-20T10:00:00Z" })) } }));
        return this.ok(JSON.stringify({ data: { repository: { pullRequest: { reviewThreads: { nodes } } } } }));
      }
      const t = this.threads.find((x) => x.id === field("thread"));
      if (!t) return this.ok(JSON.stringify({ errors: [{ message: "Could not resolve to a node" }] }));
      if (q.includes("addPullRequestReviewThreadReply")) t.comments.push({ id: `c${t.comments.length + 1}`, author: "lelemm", body: field("body") ?? "" });
      if (q.includes("resolveReviewThread")) t.isResolved = true;
      return this.ok(JSON.stringify({ data: {} }));
    }
    return { code: 1, stdout: "", stderr: `fake gh: unknown command ${args.join(" ")}` };
  }
}

/** Azure DevOps: `az` gives a token per tenant; only `goodTenant`'s token reaches the organisation. */
export class FakeAzure {
  azCalls: string[][] = [];
  http: Array<{ method: string; url: string; body?: unknown }> = [];
  installed = true;
  tenants = ["t-home", "t-other", "t-work"];
  goodTenant = "t-work";
  prs: Array<Record<string, unknown>> = [];
  policies: Array<Record<string, unknown>> = [];
  threads: Array<Record<string, unknown>> = [];

  az: CliRunner = async (program, args) => {
    if (program !== "az") throw new Error(`unexpected ${program}`);
    this.azCalls.push(args);
    if (!this.installed) return { code: null, stdout: "", stderr: "", failure: "ENOENT" };
    if (args[0] === "account" && args[1] === "list") {
      return { code: 0, stdout: JSON.stringify(this.tenants.map((t, i) => ({ tenantId: t, homeTenantId: t, isDefault: i === 0, name: `sub-${t}` }))), stderr: "" };
    }
    if (args[0] === "account" && args[1] === "get-access-token") {
      const t = args[args.indexOf("--tenant") + 1]!;
      return { code: 0, stdout: JSON.stringify({ accessToken: `token-${t}`, expires_on: Math.floor(Date.now() / 1000) + 3600, tenant: t }), stderr: "" };
    }
    return { code: 1, stdout: "", stderr: "unknown" };
  };

  fetch: HttpClient = async (req) => {
    this.http.push({ method: req.method, url: req.url, ...(req.body !== undefined ? { body: req.body } : {}) });
    if (req.token !== `token-${this.goodTenant}`) return { status: 401, text: "TF400813: not authorized", contentType: "text/plain" };
    const u = new URL(req.url);
    const json = (v: unknown, status = 200) => ({ status, text: JSON.stringify(v), contentType: "application/json; charset=utf-8" });
    const p = u.pathname;
    if (p.endsWith("/_apis/projects")) return json({ count: 1, value: [{ id: "p1" }] });
    if (/\/_apis\/git\/repositories\/[^/]+$/.test(p)) return json({ id: "r-guid", name: "widgets", defaultBranch: "refs/heads/main", webUrl: "https://dev.azure.com/contoso/Big%20Project/_git/widgets", project: { id: "proj-guid" } });
    if (p.endsWith("/pullrequests") && req.method === "GET") {
      const src = u.searchParams.get("searchCriteria.sourceRefName");
      return json({ value: this.prs.filter((x) => x.sourceRefName === src), count: 0 });
    }
    if (p.endsWith("/pullrequests") && req.method === "POST") {
      const b = req.body as Record<string, unknown>;
      const pr = { pullRequestId: 900 + this.prs.length, status: "active", ...b, createdBy: { displayName: "Lele" }, creationDate: "2026-11-20T10:00:00Z" };
      this.prs.push(pr);
      return json(pr, 201);
    }
    const byId = /\/_apis\/git\/pullrequests\/(\d+)$/.exec(p);
    if (byId) return json(this.prs.find((x) => x.pullRequestId === Number(byId[1])) ?? {}, this.prs.some((x) => x.pullRequestId === Number(byId[1])) ? 200 : 404);
    if (p.endsWith("/_apis/policy/evaluations")) return json({ value: this.policies });
    const evalPatch = /\/_apis\/policy\/evaluations\/([^/]+)$/.exec(p);
    if (evalPatch && req.method === "PATCH") return json({ evaluationId: evalPatch[1], status: "queued" });
    if (/\/build\/builds\/\d+\/timeline$/.test(p)) return json({ records: [{ name: "Run tests", result: "failed", log: { id: 7 } }, { name: "Build", result: "succeeded", log: { id: 3 } }] });
    if (/\/build\/builds\/\d+\/logs\/7$/.test(p)) return { status: 200, text: "##[error] test_parser failed: expected 3, got 4\n", contentType: "text/plain" };
    if (p.endsWith("/threads") && req.method === "GET") return json({ value: this.threads });
    if (/\/threads\/\d+\/comments$/.test(p)) return json({ id: 2, content: (req.body as { content: string }).content });
    if (/\/threads\/\d+$/.test(p) && req.method === "PATCH") return json({ id: 1, status: (req.body as { status: string }).status });
    return json({ message: `fake: no ${req.method} ${p}` }, 404);
  };
}

/** A runner: `gh` and `az` are fakes, `git` (and anything else) runs for real. */
export function fakeRunner(gh: FakeGh, azure?: FakeAzure, env: NodeJS.ProcessEnv = process.env): CliRunner {
  const childEnv = { ...env, GIT_TERMINAL_PROMPT: "0", LC_ALL: "C" };
  return async (program, args, opts = {}) => {
    if (program === "gh") return gh.handle(args);
    if (program === "az") return azure ? azure.az(program, args, opts) : { code: null, stdout: "", stderr: "", failure: "ENOENT" };
    return run(program, args, { env: childEnv, cwd: opts.cwd, timeoutMs: opts.timeoutMs ?? 30_000 });
  };
}
