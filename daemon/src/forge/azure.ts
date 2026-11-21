// Azure DevOps: the target's own `az` supplies a Microsoft Entra token; yaacd speaks the
// Azure DevOps REST API (7.1) with it, over HTTPS from the target.
//
// Why not `az repos …`: the azure-devops extension may be missing, and it takes its token from
// the current subscription's tenant — with several tenants signed in, often the wrong one.
// A token asked for the right tenant (`az account get-access-token --tenant`) is exact.
//
// Tenant discovery (D119): `az account list --all` → every distinct tenant id → for each, a
// token for Azure DevOps (resource `499b84ac-1321-427f-aa17-267ca6975798`, from the Azure
// DevOps docs) → a cheap authorised call to the organisation
// (`GET https://dev.azure.com/{org}/_apis/projects?$top=1&api-version=7.1`). The first tenant
// that answers 200 with JSON is remembered in the per-remote record (`azureTenant`) and
// reused; a call that fails with an auth error discovers again (reads only — a create is
// never re-sent).
//
// NOT verified against an installed `az` (none on the development machine): the `az account
// list` field `tenantId`/`homeTenantId`, the `accessToken` key, and the wrong-tenant HTTP
// answer (treated as anything but 200 + JSON). The REST paths and fields are from the 7.1
// reference pages; the build log endpoint and the PR web URL form are not.

import type { CheckState, ForgeAccess, ForgeCapability, PrReviewer, PrSummary } from "../protocol/generated/types.js";
import type { RemoteStore } from "./remotes.js";
import {
  combineChecks,
  ForgeError,
  orderPrs,
  tail,
  type CheckInfo,
  type CliRunner,
  type CreateRequest,
  type ForgeAdapter,
  type HttpClient,
  type HttpResponse,
  type PrDetail,
  type ReviewThread,
} from "./types.js";

export const AZURE_DEVOPS_RESOURCE = "499b84ac-1321-427f-aa17-267ca6975798";
export const AZ_ALL: ForgeCapability[] = ["prList", "prGet", "prCreate", "prChecks", "prRerun", "prComments", "prReply"];
const API = "api-version=7.1";
const POLICY_API = "api-version=7.1-preview.1";
export const TENANT_KEY = "azureTenant";

export interface AzureRepo {
  org: string;
  project: string;
  repo: string;
  /** The remote URL: the key of the per-remote record. */
  remote: string;
}

interface Token {
  tenant: string;
  token: string;
  expires: number;
}

/** Tokens per (tenant), shared by every repository in this daemon. */
const tokenCache = new Map<string, Token>();

export function clearTokenCache(): void {
  tokenCache.clear();
}

export class AzureAdapter implements ForgeAdapter {
  private repoInfo: { id: string; projectId: string; defaultBranch: string | null; webUrl: string } | null = null;

  constructor(
    private readonly az: CliRunner,
    private readonly http: HttpClient,
    private readonly remotes: RemoteStore,
    private readonly r: AzureRepo,
    private readonly targetName: string,
    private readonly log: (l: string) => void = () => {},
  ) {}

  private get base(): string {
    return `https://dev.azure.com/${encodeURIComponent(this.r.org)}`;
  }

  private get projectBase(): string {
    return `${this.base}/${encodeURIComponent(this.r.project)}`;
  }

  private loginReason(): string {
    return `az is not logged in on ${this.targetName} — run \`az login\` there`;
  }

  private async azJson(args: string[]): Promise<unknown> {
    const r = await this.az("az", [...args, "--output", "json", "--only-show-errors"], { timeoutMs: 30_000 });
    if (r.failure === "ENOENT") throw new ForgeError(`az is not installed on ${this.targetName}`, "missing");
    if (r.failure === "timeout") throw new ForgeError("az did not answer in time", "timeout");
    if (r.code !== 0) {
      const err = (r.stderr || r.stdout).trim();
      if (/az login|not logged in|expired|AADSTS/i.test(err)) throw new ForgeError(this.loginReason(), "auth");
      throw new ForgeError(err.split("\n")[0] || `az failed (exit ${r.code})`);
    }
    try {
      return JSON.parse(r.stdout);
    } catch {
      throw new ForgeError("az did not answer with JSON");
    }
  }

  /** Every tenant the signed-in accounts know, the default subscription's first. */
  async tenants(): Promise<string[]> {
    const list = (await this.azJson(["account", "list", "--all"])) as Array<{ tenantId?: string; homeTenantId?: string; isDefault?: boolean }>;
    if (!Array.isArray(list) || list.length === 0) throw new ForgeError(this.loginReason(), "auth");
    const sorted = [...list].sort((a, b) => Number(!!b.isDefault) - Number(!!a.isDefault));
    const out: string[] = [];
    for (const a of sorted) for (const t of [a.tenantId, a.homeTenantId]) if (t && !out.includes(t)) out.push(t);
    return out;
  }

  private async tokenFor(tenant: string): Promise<string | null> {
    const cached = tokenCache.get(tenant);
    if (cached && cached.expires > Date.now() + 60_000) return cached.token;
    let res: { accessToken?: string; expires_on?: number | string; expiresOn?: string };
    try {
      res = (await this.azJson(["account", "get-access-token", "--resource", AZURE_DEVOPS_RESOURCE, "--tenant", tenant])) as typeof res;
    } catch (e) {
      if ((e as ForgeError).kind === "missing") throw e;
      return null;
    }
    if (!res?.accessToken) return null;
    const exp = typeof res.expires_on === "number" ? res.expires_on * 1000 : Number(res.expires_on) * 1000 || Date.now() + 30 * 60_000;
    tokenCache.set(tenant, { tenant, token: res.accessToken, expires: exp });
    return res.accessToken;
  }

  private static authorised(r: HttpResponse): boolean {
    return r.status === 200 && /json/i.test(r.contentType) && r.text.trim().startsWith("{");
  }

  /** Finds the tenant whose token reaches this organisation; remembers it. */
  async discover(): Promise<{ tenant: string; token: string }> {
    const tenants = await this.tenants();
    for (const tenant of tenants) {
      const token = await this.tokenFor(tenant);
      if (!token) continue;
      let r: HttpResponse;
      try {
        r = await this.http({ method: "GET", url: `${this.base}/_apis/projects?$top=1&${API}`, token });
      } catch {
        continue;
      }
      if (AzureAdapter.authorised(r)) {
        this.remotes.set(this.r.remote, TENANT_KEY, tenant);
        this.log(`forge: azure devops ${this.r.org} is reached through tenant ${tenant}`);
        return { tenant, token };
      }
    }
    throw new ForgeError(`None of the tenants az is signed in to on ${this.targetName} can reach the Azure DevOps organisation ${this.r.org} — run \`az login --tenant <the organisation's tenant>\` there`, "auth");
  }

  private async credentials(): Promise<{ tenant: string; token: string }> {
    const known = this.remotes.get<string>(this.r.remote, TENANT_KEY);
    if (known) {
      const token = await this.tokenFor(known);
      if (token) return { tenant: known, token };
    }
    return this.discover();
  }

  /** One REST call. Reads re-discover the tenant once on an auth failure; writes never repeat. */
  private async call(method: string, url: string, body?: unknown, read = method === "GET"): Promise<HttpResponse> {
    let cred = await this.credentials();
    let r = await this.send(method, url, cred.token, body);
    if (r.status === 401 || r.status === 203 || (r.status === 200 && !/json|text\/plain/i.test(r.contentType))) {
      tokenCache.delete(cred.tenant);
      this.remotes.set(this.r.remote, TENANT_KEY, undefined);
      if (!read) throw new ForgeError("Azure DevOps refused the sign-in; nothing was changed. Try again.", "auth");
      cred = await this.discover();
      r = await this.send(method, url, cred.token, body);
    }
    if (r.status === 429) throw new ForgeError("Azure DevOps is rate-limiting this account; try again later.", "rateLimit");
    if (r.status === 404) throw new ForgeError(`Azure DevOps has no ${url.replace(/\?.*$/, "")}`, "notFound");
    if (r.status < 200 || r.status >= 300) throw new ForgeError(`Azure DevOps answered ${r.status}: ${errorText(r.text)}`);
    return r;
  }

  private async send(method: string, url: string, token: string, body?: unknown): Promise<HttpResponse> {
    try {
      return await this.http({ method, url, token, ...(body !== undefined ? { body } : {}), timeoutMs: method === "GET" ? 20_000 : 45_000 });
    } catch (e) {
      const timedOut = (e as Error)?.name === "TimeoutError" || (e as Error)?.name === "AbortError";
      throw new ForgeError(timedOut ? "Azure DevOps did not answer in time" : `Azure DevOps could not be reached: ${(e as Error)?.message ?? e}`, timedOut ? "timeout" : "other");
    }
  }

  private async json<T>(method: string, url: string, body?: unknown): Promise<T> {
    const r = await this.call(method, url, body);
    return JSON.parse(r.text) as T;
  }

  async access(): Promise<ForgeAccess> {
    try {
      await this.credentials();
      return { capabilities: [...AZ_ALL] };
    } catch (e) {
      return { capabilities: [], reason: (e as Error).message };
    }
  }

  headRef(branch: string): string {
    return branch;
  }

  private async repository(): Promise<{ id: string; projectId: string; defaultBranch: string | null; webUrl: string }> {
    if (this.repoInfo) return this.repoInfo;
    const r = await this.json<{ id: string; defaultBranch?: string; webUrl?: string; project?: { id?: string } }>(
      "GET",
      `${this.projectBase}/_apis/git/repositories/${encodeURIComponent(this.r.repo)}?${API}`,
    );
    this.repoInfo = {
      id: r.id,
      projectId: r.project?.id ?? "",
      defaultBranch: r.defaultBranch ? r.defaultBranch.replace(/^refs\/heads\//, "") : null,
      webUrl: r.webUrl ?? `${this.projectBase}/_git/${encodeURIComponent(this.r.repo)}`,
    };
    return this.repoInfo;
  }

  private summary(p: AzPr, webUrl: string, checks?: CheckState[]): PrSummary {
    const state = p.status === "completed" ? "merged" : p.status === "abandoned" ? "closed" : "open";
    return {
      number: p.pullRequestId,
      title: p.title ?? "",
      state,
      draft: !!p.isDraft,
      url: `${webUrl}/pullrequest/${p.pullRequestId}`,
      head: (p.sourceRefName ?? "").replace(/^refs\/heads\//, ""),
      base: (p.targetRefName ?? "").replace(/^refs\/heads\//, ""),
      ...(checks ? { checks: combineChecks(checks) } : {}),
      ...(p.createdBy?.displayName ? { author: p.createdBy.displayName } : {}),
      ...(p.closedDate || p.creationDate ? { updatedAt: p.closedDate ?? p.creationDate } : {}),
    };
  }

  async list(branch: string): Promise<PrSummary[]> {
    const repo = await this.repository();
    const q = `searchCriteria.sourceRefName=${encodeURIComponent(`refs/heads/${branch}`)}&searchCriteria.status=all&$top=20&${API}`;
    const r = await this.json<{ value?: AzPr[] }>("GET", `${this.projectBase}/_apis/git/repositories/${repo.id}/pullrequests?${q}`);
    const prs = r.value ?? [];
    const out: PrSummary[] = [];
    let policyCalls = 0;
    for (const p of prs) {
      // Policies cost a call per pull request: only for the first few open ones.
      let checks: CheckState[] | undefined;
      if (p.status === "active" && policyCalls < 3) {
        policyCalls++;
        checks = await this.policies(p.pullRequestId).then((c) => c.map((x) => x.state)).catch(() => undefined);
      }
      out.push(this.summary(p, repo.webUrl, checks));
    }
    return orderPrs(out);
  }

  private async policies(number: number): Promise<CheckInfo[]> {
    const repo = await this.repository();
    const artifact = `vstfs:///CodeReview/CodeReviewId/${repo.projectId}/${number}`;
    const r = await this.json<{ value?: AzPolicy[] }>("GET", `${this.projectBase}/_apis/policy/evaluations?artifactId=${encodeURIComponent(artifact)}&${POLICY_API}`);
    return (r.value ?? []).map((e) => {
      const settings = (e.configuration?.settings ?? {}) as { displayName?: string };
      const build = e.context && typeof e.context === "object" ? (e.context as { buildId?: number | string }).buildId : undefined;
      return {
        name: settings.displayName || e.configuration?.type?.displayName || "policy",
        state: policyState(e.status),
        blocking: !!e.configuration?.isBlocking,
        evaluationId: e.evaluationId,
        ...(build !== undefined && build !== null ? { runId: String(build), url: `${this.projectBase}/_build/results?buildId=${build}` } : {}),
      };
    });
  }

  async get(number: number): Promise<PrDetail> {
    const repo = await this.repository();
    const p = await this.json<AzPr>("GET", `${this.projectBase}/_apis/git/pullrequests/${number}?${API}`);
    const checks = await this.policies(number).catch(() => [] as CheckInfo[]);
    const reviewers: PrReviewer[] = (p.reviewers ?? []).map((r) => ({ name: r.displayName ?? "reviewer", state: voteWord(r.vote, r.hasDeclined), required: !!r.isRequired }));
    return { pr: this.summary(p, repo.webUrl, checks.map((c) => c.state)), body: p.description ?? "", reviewers, checks };
  }

  async create(req: CreateRequest): Promise<PrSummary> {
    const repo = await this.repository();
    const body = { sourceRefName: `refs/heads/${req.head}`, targetRefName: `refs/heads/${req.base}`, title: req.title, description: req.body, isDraft: req.draft };
    const r = await this.call("POST", `${this.projectBase}/_apis/git/repositories/${repo.id}/pullrequests?${API}`, body, false);
    return this.summary(JSON.parse(r.text) as AzPr, repo.webUrl, []);
  }

  async defaultBranch(): Promise<string | null> {
    return (await this.repository()).defaultBranch;
  }

  async failedLog(number: number, check?: string): Promise<string> {
    const failed = (await this.policies(number)).filter((c) => c.state === "failure" && (!check || c.name.toLowerCase().includes(check.toLowerCase())));
    const build = failed.find((c) => c.runId);
    if (!build) return failed.length ? `The failed policy ${failed[0]!.name} is not a build; see the pull request.` : "";
    const timeline = await this.json<{ records?: AzRecord[] }>("GET", `${this.projectBase}/_apis/build/builds/${build.runId}/timeline?${API}`);
    const logs = (timeline.records ?? []).filter((r) => r.result === "failed" && r.log?.id !== undefined);
    const parts: string[] = [];
    for (const rec of logs.slice(0, 3)) {
      const r = await this.call("GET", `${this.projectBase}/_apis/build/builds/${build.runId}/logs/${rec.log!.id}?${API}`);
      parts.push(`── ${rec.name ?? "step"} ──\n${r.text}`);
    }
    return tail(parts.join("\n"));
  }

  async rerunFailed(number: number): Promise<string[]> {
    const failed = (await this.policies(number)).filter((c) => c.state === "failure" && c.evaluationId && c.runId);
    for (const c of failed) await this.call("PATCH", `${this.projectBase}/_apis/policy/evaluations/${c.evaluationId}?${POLICY_API}`, { status: "queued" }, false);
    return failed.map((c) => c.runId!);
  }

  async comments(number: number): Promise<ReviewThread[]> {
    const repo = await this.repository();
    const r = await this.json<{ value?: AzThread[] }>("GET", `${this.projectBase}/_apis/git/repositories/${repo.id}/pullRequests/${number}/threads?${API}`);
    return (r.value ?? [])
      .filter((t) => !t.isDeleted && (t.comments ?? []).some((c) => c.commentType !== "system" && !c.isDeleted))
      .map((t) => ({
        id: String(t.id),
        resolved: !!t.status && t.status !== "active" && t.status !== "pending",
        ...(t.threadContext?.filePath ? { path: t.threadContext.filePath } : {}),
        ...(typeof t.threadContext?.rightFileStart?.line === "number" ? { line: t.threadContext.rightFileStart.line } : {}),
        comments: (t.comments ?? []).filter((c) => !c.isDeleted).map((c) => ({ id: String(c.id), author: c.author?.displayName ?? "someone", body: c.content ?? "", ...(c.publishedDate ? { at: c.publishedDate } : {}) })),
      }));
  }

  async reply(number: number, threadId: string, body: string): Promise<void> {
    const repo = await this.repository();
    await this.call("POST", `${this.projectBase}/_apis/git/repositories/${repo.id}/pullRequests/${number}/threads/${threadId}/comments?${API}`, { content: body, parentCommentId: 1, commentType: 1 }, false);
  }

  async resolve(number: number, threadId: string): Promise<void> {
    const repo = await this.repository();
    await this.call("PATCH", `${this.projectBase}/_apis/git/repositories/${repo.id}/pullRequests/${number}/threads/${threadId}?${API}`, { status: "fixed" }, false);
  }
}

interface AzPr {
  pullRequestId: number;
  title?: string;
  description?: string;
  status?: string;
  isDraft?: boolean;
  sourceRefName?: string;
  targetRefName?: string;
  creationDate?: string;
  closedDate?: string;
  createdBy?: { displayName?: string };
  reviewers?: Array<{ displayName?: string; vote?: number; isRequired?: boolean; hasDeclined?: boolean }>;
}

interface AzPolicy {
  evaluationId: string;
  status?: string;
  configuration?: { isBlocking?: boolean; type?: { displayName?: string }; settings?: unknown };
  context?: unknown;
}

interface AzRecord {
  name?: string;
  result?: string;
  log?: { id: number };
}

interface AzThread {
  id: number;
  status?: string;
  isDeleted?: boolean;
  threadContext?: { filePath?: string; rightFileStart?: { line?: number } } | null;
  comments?: Array<{ id: number; author?: { displayName?: string }; content?: string; publishedDate?: string; commentType?: string; isDeleted?: boolean }>;
}

export function policyState(s: string | undefined): CheckState {
  switch (s) {
    case "approved":
      return "success";
    case "rejected":
    case "broken":
      return "failure";
    case "notApplicable":
      return "skipped";
    default:
      return "pending";
  }
}

/** Vote values from the Azure DevOps docs: 10 approved, 5 with suggestions, 0 none, -5 waiting, -10 rejected. */
export function voteWord(vote: number | undefined, declined?: boolean): string {
  if (declined) return "declined";
  switch (vote) {
    case 10:
      return "approved";
    case 5:
      return "approved with suggestions";
    case -5:
      return "waiting for author";
    case -10:
      return "rejected";
    default:
      return "no vote";
  }
}

function errorText(text: string): string {
  try {
    const j = JSON.parse(text) as { message?: string };
    if (j.message) return j.message;
  } catch {
    /* not JSON */
  }
  return text.slice(0, 200);
}
