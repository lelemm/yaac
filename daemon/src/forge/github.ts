// GitHub through the target's own `gh` (verified against gh 2.45.0; docs/research/harness-surfaces.md
// lists how). Every call names the repository with `-R HOST/OWNER/REPO`, asks for JSON, and
// runs with prompts disabled. `gh pr checks` has no `--json` in 2.45, so checks come from
// `gh pr view --json statusCheckRollup`; review threads come from GraphQL (`gh api graphql`).

import type { CheckState, ForgeAccess, ForgeCapability, PrReviewer, PrSummary } from "../protocol/generated/types.js";
import {
  combineChecks,
  ForgeError,
  orderPrs,
  tail,
  type CheckInfo,
  type CliRunner,
  type CreateRequest,
  type ForgeAdapter,
  type PrDetail,
  type ReviewThread,
} from "./types.js";

export const GH_ALL: ForgeCapability[] = ["prList", "prGet", "prCreate", "prChecks", "prRerun", "prComments", "prReply"];

const LIST_FIELDS = "number,title,state,isDraft,url,headRefName,baseRefName,statusCheckRollup,author,updatedAt,headRepositoryOwner";
const VIEW_FIELDS = `${LIST_FIELDS},body,latestReviews,reviewRequests`;

/** The hosts `gh auth status` lists (one per unindented line), for GitHub Enterprise detection. */
export function parseAuthHosts(out: string): string[] {
  const hosts: string[] = [];
  for (const line of out.split("\n")) {
    if (!line || /^\s/.test(line)) continue;
    const h = line.trim();
    if (/^[a-z0-9.-]+(:\d+)?$/i.test(h)) hosts.push(h.toLowerCase());
  }
  return hosts;
}

interface RollupItem {
  __typename?: string;
  name?: string;
  context?: string;
  status?: string;
  conclusion?: string;
  state?: string;
  detailsUrl?: string;
  targetUrl?: string;
  workflowName?: string;
}

export function checkStateOf(c: RollupItem): CheckState {
  if (c.__typename === "StatusContext" || (c.context !== undefined && c.status === undefined)) {
    const s = (c.state ?? "").toUpperCase();
    if (s === "SUCCESS") return "success";
    if (s === "FAILURE" || s === "ERROR") return "failure";
    return "pending";
  }
  if ((c.status ?? "").toUpperCase() !== "COMPLETED") return "pending";
  const k = (c.conclusion ?? "").toUpperCase();
  if (k === "SUCCESS") return "success";
  if (k === "FAILURE" || k === "TIMED_OUT" || k === "ACTION_REQUIRED" || k === "STARTUP_FAILURE" || k === "CANCELLED") return "failure";
  return "skipped";
}

export function checksOf(rollup: unknown): CheckInfo[] {
  if (!Array.isArray(rollup)) return [];
  return (rollup as RollupItem[]).map((c) => {
    const url = c.detailsUrl ?? c.targetUrl;
    const run = url ? /\/actions\/runs\/(\d+)/.exec(url) : null;
    const name = c.name ?? c.context ?? "check";
    return {
      name: c.workflowName && c.name ? `${c.workflowName} / ${c.name}` : name,
      state: checkStateOf(c),
      ...(url ? { url } : {}),
      blocking: false,
      ...(run ? { runId: run[1]! } : {}),
    };
  });
}

interface GhPr {
  number: number;
  title: string;
  state: string;
  isDraft?: boolean;
  url: string;
  headRefName: string;
  baseRefName: string;
  statusCheckRollup?: unknown;
  author?: { login?: string } | null;
  updatedAt?: string;
  headRepositoryOwner?: { login?: string } | null;
  body?: string;
  latestReviews?: Array<{ author?: { login?: string }; state?: string }>;
  reviewRequests?: Array<{ login?: string; name?: string; slug?: string }>;
}

export function summaryOf(p: GhPr): PrSummary {
  const state = p.state === "MERGED" ? "merged" : p.state === "CLOSED" ? "closed" : "open";
  const checks = checksOf(p.statusCheckRollup);
  return {
    number: p.number,
    title: p.title,
    state,
    draft: !!p.isDraft,
    url: p.url,
    head: p.headRefName,
    base: p.baseRefName,
    checks: combineChecks(checks.map((c) => c.state)),
    ...(p.author?.login ? { author: p.author.login } : {}),
    ...(p.updatedAt ? { updatedAt: p.updatedAt } : {}),
  };
}

const REVIEW_WORDS: Record<string, string> = {
  APPROVED: "approved",
  CHANGES_REQUESTED: "changes requested",
  COMMENTED: "commented",
  DISMISSED: "dismissed",
  PENDING: "pending",
};

export interface GithubRepo {
  host: string;
  /** Where pull requests live (upstream for a fork). */
  owner: string;
  repo: string;
  /** The fork's owner, when pull requests are opened from a fork. */
  forkOwner?: string;
}

export class GithubAdapter implements ForgeAdapter {
  constructor(
    private readonly gh: CliRunner,
    private readonly r: GithubRepo,
    private readonly targetName: string,
    private readonly cwd: string,
  ) {}

  private get repoArg(): string {
    return `${this.r.host}/${this.r.owner}/${this.r.repo}`;
  }

  /** Runs gh; failures become a ForgeError with a reason the user can act on. */
  private async run(args: string[], timeoutMs = 30_000, maxBytes?: number): Promise<string> {
    const r = await this.gh("gh", args, { cwd: this.cwd, timeoutMs, ...(maxBytes ? { maxBytes } : {}) });
    if (r.code === 0) return r.stdout;
    if (r.failure === "ENOENT") throw new ForgeError(`gh is not installed on ${this.targetName}`, "missing");
    if (r.failure === "timeout") throw new ForgeError(`gh did not answer within ${Math.round(timeoutMs / 1000)} s`, "timeout");
    const err = (r.stderr || r.stdout).trim();
    // gh exits 4 when it needs authentication.
    if (r.code === 4 || /gh auth login|authentication|not logged in/i.test(err)) throw new ForgeError(this.loginReason(), "auth");
    if (/rate limit/i.test(err)) throw new ForgeError(`GitHub's rate limit was reached: ${firstLine(err)}`, "rateLimit");
    if (/not found|could not resolve/i.test(err)) throw new ForgeError(firstLine(err) || "not found", "notFound");
    throw new ForgeError(firstLine(err) || `gh failed (exit ${r.code})`);
  }

  private loginReason(): string {
    const login = this.r.host === "github.com" ? "gh auth login" : `gh auth login --hostname ${this.r.host}`;
    return `gh is not logged in on ${this.targetName} · run \`${login}\` there`;
  }

  async access(): Promise<ForgeAccess> {
    try {
      await this.run(["auth", "status", "--hostname", this.r.host], 15_000);
      return { capabilities: [...GH_ALL] };
    } catch (e) {
      const err = e as ForgeError;
      return { capabilities: [], reason: err.kind === "missing" ? err.message : err.kind === "auth" || err.kind === "other" ? this.loginReason() : err.message };
    }
  }

  headRef(branch: string): string {
    return this.r.forkOwner ? `${this.r.forkOwner}:${branch}` : branch;
  }

  async list(branch: string): Promise<PrSummary[]> {
    const out = await this.run(["pr", "list", "-R", this.repoArg, "--head", branch, "--state", "all", "--limit", "20", "--json", LIST_FIELDS]);
    let prs = JSON.parse(out) as GhPr[];
    // `--head` matches the branch name in any fork: keep the ones from our own.
    const owner = this.r.forkOwner ?? this.r.owner;
    prs = prs.filter((p) => p.headRefName === branch && (!p.headRepositoryOwner?.login || p.headRepositoryOwner.login.toLowerCase() === owner.toLowerCase()));
    return orderPrs(prs.map(summaryOf));
  }

  private async view(number: number): Promise<GhPr> {
    return JSON.parse(await this.run(["pr", "view", String(number), "-R", this.repoArg, "--json", VIEW_FIELDS])) as GhPr;
  }

  async get(number: number): Promise<PrDetail> {
    const p = await this.view(number);
    const reviewers: PrReviewer[] = [];
    for (const r of p.latestReviews ?? []) {
      const name = r.author?.login;
      if (name) reviewers.push({ name, state: REVIEW_WORDS[(r.state ?? "").toUpperCase()] ?? (r.state ?? "").toLowerCase(), required: false });
    }
    for (const r of p.reviewRequests ?? []) {
      const name = r.login ?? r.slug ?? r.name;
      if (name && !reviewers.some((x) => x.name === name)) reviewers.push({ name, state: "requested", required: false });
    }
    return { pr: summaryOf(p), body: p.body ?? "", reviewers, checks: checksOf(p.statusCheckRollup) };
  }

  async create(req: CreateRequest): Promise<PrSummary> {
    const args = ["pr", "create", "-R", this.repoArg, "--head", this.headRef(req.head), "--base", req.base, "--title", req.title, "--body", req.body];
    if (req.draft) args.push("--draft");
    const out = await this.run(args, 60_000);
    // gh prints the new pull request's URL.
    const url = out.trim().split("\n").reverse().find((l) => /^https?:\/\//.test(l.trim()))?.trim();
    const number = url ? Number(/\/pull\/(\d+)/.exec(url)?.[1]) : NaN;
    if (Number.isFinite(number)) {
      try {
        return summaryOf(await this.view(number));
      } catch {
        /* fall through to a summary from what was asked */
      }
    }
    return { number: Number.isFinite(number) ? number : 0, title: req.title, state: "open", draft: req.draft, url: url ?? "", head: req.head, base: req.base, checks: "none" };
  }

  async defaultBranch(): Promise<string | null> {
    const out = await this.run(["repo", "view", `${this.r.host}/${this.r.owner}/${this.r.repo}`, "--json", "defaultBranchRef"]);
    return (JSON.parse(out) as { defaultBranchRef?: { name?: string } }).defaultBranchRef?.name ?? null;
  }

  /** The repository's parent (`gh repo view --json parent`): a fork's upstream. */
  static async parentOf(gh: CliRunner, host: string, owner: string, repo: string, cwd: string): Promise<{ owner: string; repo: string } | null> {
    const r = await gh("gh", ["repo", "view", `${host}/${owner}/${repo}`, "--json", "parent"], { cwd, timeoutMs: 20_000 });
    if (r.code !== 0) return null;
    try {
      const p = (JSON.parse(r.stdout) as { parent?: { name?: string; owner?: { login?: string } } | null }).parent;
      return p?.name && p.owner?.login ? { owner: p.owner.login, repo: p.name } : null;
    } catch {
      return null;
    }
  }

  private async failedRuns(number: number): Promise<{ runs: string[]; checks: CheckInfo[] }> {
    const checks = (await this.get(number)).checks;
    const runs = [...new Set(checks.filter((c) => c.state === "failure" && c.runId).map((c) => c.runId!))];
    return { runs, checks };
  }

  async failedLog(number: number, check?: string): Promise<string> {
    const { checks } = await this.failedRuns(number);
    const failed = checks.filter((c) => c.state === "failure" && (!check || c.name.toLowerCase().includes(check.toLowerCase())));
    const run = failed.find((c) => c.runId)?.runId;
    if (!run) {
      if (failed.length === 0) return "";
      return `The failed check ${failed[0]!.name} is not a GitHub Actions run; see ${failed[0]!.url ?? "the pull request"}.`;
    }
    const log = await this.run(["run", "view", run, "-R", this.repoArg, "--log-failed"], 60_000, 32 * 1024 * 1024);
    return tail(log);
  }

  async rerunFailed(number: number): Promise<string[]> {
    const { runs } = await this.failedRuns(number);
    for (const run of runs) await this.run(["run", "rerun", run, "-R", this.repoArg, "--failed"], 30_000);
    return runs;
  }

  private async graphql(query: string, fields: Record<string, string | number>): Promise<unknown> {
    const args = ["api", "--hostname", this.r.host, "graphql", "-f", `query=${query}`];
    for (const [k, v] of Object.entries(fields)) args.push(typeof v === "number" ? "-F" : "-f", `${k}=${v}`);
    const out = JSON.parse(await this.run(args)) as { data?: unknown; errors?: Array<{ message?: string }> };
    if (out.errors?.length) throw new ForgeError(out.errors.map((e) => e.message).join("; "));
    return out.data;
  }

  async comments(number: number): Promise<ReviewThread[]> {
    const q =
      "query($owner:String!,$name:String!,$number:Int!){repository(owner:$owner,name:$name){pullRequest(number:$number){reviewThreads(first:100){nodes{id isResolved path line comments(first:50){nodes{id author{login} body createdAt}}}}}}}";
    const data = (await this.graphql(q, { owner: this.r.owner, name: this.r.repo, number })) as {
      repository?: { pullRequest?: { reviewThreads?: { nodes?: Array<{ id: string; isResolved: boolean; path?: string; line?: number | null; comments?: { nodes?: Array<{ id: string; author?: { login?: string } | null; body: string; createdAt?: string }> } }> } } };
    };
    return (data.repository?.pullRequest?.reviewThreads?.nodes ?? []).map((t) => ({
      id: t.id,
      resolved: t.isResolved,
      ...(t.path ? { path: t.path } : {}),
      ...(typeof t.line === "number" ? { line: t.line } : {}),
      comments: (t.comments?.nodes ?? []).map((c) => ({ id: c.id, author: c.author?.login ?? "someone", body: c.body, ...(c.createdAt ? { at: c.createdAt } : {}) })),
    }));
  }

  async reply(_number: number, threadId: string, body: string): Promise<void> {
    await this.graphql("mutation($thread:ID!,$body:String!){addPullRequestReviewThreadReply(input:{pullRequestReviewThreadId:$thread,body:$body}){comment{id}}}", { thread: threadId, body });
  }

  async resolve(_number: number, threadId: string): Promise<void> {
    await this.graphql("mutation($thread:ID!){resolveReviewThread(input:{threadId:$thread}){thread{id isResolved}}}", { thread: threadId });
  }
}

function firstLine(s: string): string {
  return s.split("\n").map((l) => l.trim()).find(Boolean) ?? "";
}
