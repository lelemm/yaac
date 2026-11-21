// Pull requests for threads (D119–D121): the forge service of yaacd.
//
// Everything runs here, on the target: `git` for the branch and pushing, the target's own
// `gh` (GitHub) or `az` (Azure DevOps) for the forge. The host only opens URLs.
//
// Caching and refresh (never a busy poll):
// - one entry per (checkout root, branch), shared by every thread on that branch;
// - `forge/prs` answers from the entry while it is fresh (2 min), else asks the forge; the
//   window asks for the rows it shows, again only every few minutes, and with `refresh`
//   when the user opens the row's menu (still at most every 20 s);
// - a change of the checkout's git status (a commit, a push) refreshes the entries of that
//   checkout that someone asked about in the last 30 minutes, and pushes `forge/prs/changed`;
// - a rate-limit answer pauses that host for 10 minutes; the last list is kept and served.

import * as fs from "node:fs";
import * as path from "node:path";
import type {
  ForgeAccess,
  ForgePrCreateParams,
  ForgePrCreateResult,
  ForgePrepareResult,
  ForgePrGetResult,
  ForgePrsResult,
  ForgeRepo,
  PrSummary,
  PushState,
} from "../protocol/generated/types.js";
import { RpcError } from "../protocol/rpc.js";
import type { Core } from "../session/core.js";
import { NotActed } from "../session/ops.js";
import type { ThreadRuntime } from "../session/thread.js";
import { Limiter } from "../util/exec.js";
import { nowIso } from "../util/fsx.js";
import { AzureAdapter } from "./azure.js";
import { parseRemote, slugOf, webUrlOf, type ParsedRemote } from "./detect.js";
import { GithubAdapter, parseAuthHosts } from "./github.js";
import { RemoteStore } from "./remotes.js";
import { ForgeError, cliRunner, fetchHttp, type CliRunner, type ForgeAdapter, type HttpClient } from "./types.js";

export const FRESH_MS = 120_000;
export const MIN_ASK_MS = 20_000;
export const WATCH_MS = 30 * 60_000;
export const RATE_PAUSE_MS = 10 * 60_000;
const ACCESS_OK_MS = 5 * 60_000;
const ACCESS_FAIL_MS = 60_000;
const CTX_MS = 5 * 60_000;
const GIT_DEBOUNCE_MS = 3_000;

export interface ForgeOptions {
  home: string;
  env: NodeJS.ProcessEnv;
  /** This target's name as the user knows it, for reasons ("… on devbox"). */
  targetName: () => string;
  log: (line: string) => void;
  /** Runs `gh`, `az`, and `git` (tests inject a fake). */
  runner?: CliRunner;
  /** HTTPS for Azure DevOps (tests inject a fake). */
  http?: HttpClient;
  /** Clock (tests). */
  now?: () => number;
  /** The least time between two asks of the forge for one branch (tests shorten it). */
  minAskMs?: number;
  /** How long after a git change the forge is asked again (tests shorten it). */
  debounceMs?: number;
}

interface RepoCtx {
  root: string;
  remote: string;
  origin: string | null;
  parsed: ParsedRemote | null;
  repo: ForgeRepo | null;
  adapter: ForgeAdapter | null;
  at: number;
}

interface Entry {
  root: string;
  branch: string;
  result: ForgePrsResult;
  fetchedAt: number;
  askedAt: number;
  inflight: Promise<ForgePrsResult> | null;
  timer: NodeJS.Timeout | null;
}

export interface ThreadForge {
  rt: ThreadRuntime;
  cwd: string;
  root: string;
  branch: string | null;
  ctx: RepoCtx;
}

export class Forge {
  readonly remotes: RemoteStore;
  private readonly cli: CliRunner;
  private readonly http: HttpClient;
  private readonly now: () => number;
  private readonly limiter = new Limiter(2);
  private ctxs = new Map<string, Promise<RepoCtx>>();
  private ctxAt = new Map<string, number>();
  private entries = new Map<string, Entry>();
  private accessCache = new Map<string, { access: ForgeAccess; at: number; ok: boolean }>();
  private ghHosts: { hosts: string[]; at: number } | null = null;
  private pausedUntil = new Map<string, number>();
  private closed = false;
  private unsubscribe: (() => void) | null = null;

  constructor(
    private readonly core: Core,
    private readonly opts: ForgeOptions,
  ) {
    this.remotes = new RemoteStore(opts.home);
    this.cli = opts.runner ?? cliRunner(opts.env);
    this.http = opts.http ?? fetchHttp;
    this.now = opts.now ?? Date.now;
    this.unsubscribe = core.gitChanged.on((root) => this.onGitChanged(root));
  }

  private get minAsk(): number {
    return this.opts.minAskMs ?? MIN_ASK_MS;
  }

  close(): void {
    this.closed = true;
    this.unsubscribe?.();
    for (const e of this.entries.values()) if (e.timer) clearTimeout(e.timer);
  }

  /** The protocol methods this serves. */
  methods(): Map<string, (client: unknown, p: Record<string, unknown>) => Promise<unknown>> {
    const m = new Map<string, (client: unknown, p: Record<string, unknown>) => Promise<unknown>>();
    m.set("forge/prs", (_c, p) => this.prs(threadIdOf(p), p.refresh === true));
    m.set("forge/prepare", (_c, p) => this.prepare(threadIdOf(p), typeof p.base === "string" ? p.base : undefined));
    m.set("forge/pr/create", (_c, p) => this.create(p as unknown as ForgePrCreateParams));
    m.set("forge/pr/get", (_c, p) => {
      if (typeof p.number !== "number") throw new RpcError("invalid_params", "number is required");
      return this.get(threadIdOf(p), p.number);
    });
    return m;
  }

  // ── Context: checkout, branch, remote, forge ─────────────────────────────────────

  private git(cwd: string, args: string[], timeoutMs = 15_000) {
    return this.cli("git", ["-C", cwd, "-c", "core.quotepath=off", ...args], { timeoutMs });
  }

  private async gitOut(cwd: string, args: string[]): Promise<string | null> {
    const r = await this.git(cwd, args);
    return r.code === 0 ? r.stdout.trim() : null;
  }

  private async githubHosts(): Promise<string[]> {
    if (this.ghHosts && this.now() - this.ghHosts.at < 10 * 60_000) return this.ghHosts.hosts;
    const r = await this.cli("gh", ["auth", "status"], { timeoutMs: 15_000 });
    // gh prints the hosts to stdout (2.45) or stderr (older builds).
    const hosts = r.failure === "ENOENT" ? [] : parseAuthHosts(`${r.stdout}\n${r.stderr}`);
    this.ghHosts = { hosts, at: this.now() };
    return hosts;
  }

  /** Resolves a thread to its checkout, branch, and forge. */
  async forThread(threadId: string): Promise<ThreadForge> {
    const rt = this.core.thread(threadId);
    if (!rt || rt.deleted) throw new RpcError("thread_not_found", `no thread ${threadId}`);
    const cwd = rt.record.cwd;
    if (!fs.existsSync(cwd)) throw new RpcError("invalid_params", `${cwd} no longer exists`);
    const root = rt.record.repo?.root ?? (await this.gitOut(cwd, ["rev-parse", "--show-toplevel"]));
    if (!root) return { rt, cwd, root: cwd, branch: null, ctx: { root: cwd, remote: "origin", origin: null, parsed: null, repo: null, adapter: null, at: this.now() } };
    const branch = (await this.gitOut(root, ["symbolic-ref", "--quiet", "--short", "HEAD"])) || null;
    return { rt, cwd, root, branch, ctx: await this.context(root) };
  }

  private context(root: string): Promise<RepoCtx> {
    const at = this.ctxAt.get(root) ?? 0;
    let p = this.ctxs.get(root);
    if (!p || this.now() - at > CTX_MS) {
      p = this.buildContext(root).catch((e) => {
        this.ctxs.delete(root);
        throw e;
      });
      this.ctxs.set(root, p);
      this.ctxAt.set(root, this.now());
    }
    return p;
  }

  private async buildContext(root: string): Promise<RepoCtx> {
    const remote = "origin";
    const origin = await this.gitOut(root, ["remote", "get-url", remote]);
    const base: RepoCtx = { root, remote, origin, parsed: null, repo: null, adapter: null, at: this.now() };
    if (!origin) return base;
    let parsed = parseRemote(origin);
    if (!parsed) parsed = parseRemote(origin, await this.githubHosts());
    if (!parsed) return base;
    base.parsed = parsed;
    const target = this.opts.targetName();
    if (parsed.kind === "github") {
      // A fork with an `upstream` remote that is its parent: pull requests go upstream.
      let owner = parsed.owner;
      let repo = parsed.repo;
      let forkOwner: string | undefined;
      const upstreamUrl = await this.gitOut(root, ["remote", "get-url", "upstream"]);
      const up = upstreamUrl ? parseRemote(upstreamUrl, [parsed.host]) : null;
      if (up && up.kind === "github" && up.host === parsed.host && slugOf(up) !== slugOf(parsed)) {
        let known = this.remotes.get<{ owner: string; repo: string } | false>(origin, "githubBase");
        if (known === undefined) {
          const parent = await GithubAdapter.parentOf(this.cli, parsed.host, parsed.owner, parsed.repo, root);
          known = parent && parent.owner.toLowerCase() === up.owner.toLowerCase() && parent.repo.toLowerCase() === up.repo.toLowerCase() ? parent : false;
          // Only remember a definite answer (gh ran and said so).
          if (parent || known === false) this.remotes.set(origin, "githubBase", known);
        }
        if (known) {
          forkOwner = parsed.owner;
          owner = known.owner;
          repo = known.repo;
        }
      }
      base.repo = { kind: "github", host: parsed.host, slug: `${owner}/${repo}`, webUrl: `https://${parsed.host}/${owner}/${repo}`, fromFork: !!forkOwner };
      base.adapter = new GithubAdapter(this.cli, { host: parsed.host, owner, repo, ...(forkOwner ? { forkOwner } : {}) }, target, root);
    } else {
      base.repo = { kind: "azureDevOps", host: parsed.host, slug: slugOf(parsed), webUrl: webUrlOf(parsed), fromFork: false };
      base.adapter = new AzureAdapter(this.cli, this.http, this.remotes, { org: parsed.org, project: parsed.project, repo: parsed.repo, remote: origin }, target, this.opts.log);
    }
    return base;
  }

  private async access(ctx: RepoCtx): Promise<ForgeAccess> {
    if (!ctx.repo || !ctx.adapter) return { capabilities: [] };
    const key = `${ctx.repo.kind}:${ctx.repo.host}:${ctx.repo.slug}`;
    const c = this.accessCache.get(key);
    if (c && this.now() - c.at < (c.ok ? ACCESS_OK_MS : ACCESS_FAIL_MS)) return { repo: ctx.repo, ...c.access };
    const a = await this.limiter.run(() => ctx.adapter!.access(), true);
    this.accessCache.set(key, { access: a, at: this.now(), ok: a.capabilities.length > 0 });
    return { repo: ctx.repo, ...a };
  }

  private forgetAccess(ctx: RepoCtx): void {
    if (ctx.repo) this.accessCache.delete(`${ctx.repo.kind}:${ctx.repo.host}:${ctx.repo.slug}`);
  }

  // ── forge/prs ────────────────────────────────────────────────────────────────────

  async prs(threadId: string, refresh = false): Promise<ForgePrsResult> {
    const t = await this.forThread(threadId);
    if (!t.ctx.repo) return { root: t.root, ...(t.branch ? { branch: t.branch } : {}), access: { capabilities: [] }, prs: [] };
    if (!t.branch) return { root: t.root, access: await this.access(t.ctx), prs: [] };
    const e = this.entry(t.root, t.branch);
    e.askedAt = this.now();
    const age = this.now() - e.fetchedAt;
    if (e.fetchedAt > 0 && age < (refresh ? this.minAsk : FRESH_MS)) return e.result;
    return this.fetch(e, t.ctx);
  }

  private entry(root: string, branch: string): Entry {
    const key = `${root}\0${branch}`;
    let e = this.entries.get(key);
    if (!e) {
      e = { root, branch, result: { root, branch, access: { capabilities: [] }, prs: [] }, fetchedAt: 0, askedAt: 0, inflight: null, timer: null };
      this.entries.set(key, e);
    }
    return e;
  }

  private fetch(e: Entry, ctx: RepoCtx): Promise<ForgePrsResult> {
    if (e.inflight) return e.inflight;
    const p = (async () => {
      const access = await this.access(ctx);
      const before = JSON.stringify(stable(e.result));
      const next: ForgePrsResult = { root: e.root, branch: e.branch, access, prs: e.result.prs };
      const host = ctx.repo!.host;
      const paused = (this.pausedUntil.get(host) ?? 0) > this.now();
      if (!access.capabilities.includes("prList")) {
        next.prs = [];
      } else if (paused) {
        next.error = "The forge is rate-limiting this account; the list will be refreshed later.";
      } else {
        try {
          next.prs = await this.limiter.run(() => ctx.adapter!.list(e.branch));
        } catch (err) {
          const fe = err as ForgeError;
          if (fe.kind === "rateLimit") this.pausedUntil.set(host, this.now() + RATE_PAUSE_MS);
          if (fe.kind === "auth") this.forgetAccess(ctx);
          next.error = fe.message ?? String(err);
          this.opts.log(`forge: listing ${ctx.repo!.slug} ${e.branch} failed: ${next.error}`);
        }
      }
      next.at = nowIso();
      e.fetchedAt = this.now();
      e.result = next;
      if (JSON.stringify(stable(next)) !== before && !this.closed) this.core.broadcast("forge/prs/changed", next);
      return next;
    })().finally(() => {
      e.inflight = null;
    });
    e.inflight = p;
    return p;
  }

  private onGitChanged(root: string): void {
    if (this.closed) return;
    for (const e of this.entries.values()) {
      if (e.root !== root || this.now() - e.askedAt > WATCH_MS || e.timer) continue;
      const wait = Math.max(this.opts.debounceMs ?? GIT_DEBOUNCE_MS, this.minAsk - (this.now() - e.fetchedAt));
      e.timer = setTimeout(() => {
        e.timer = null;
        if (this.closed) return;
        void this.context(root)
          .then((ctx) => (ctx.repo ? this.fetch(e, ctx) : null))
          .catch(() => {});
      }, wait);
      e.timer.unref?.();
    }
  }

  // ── forge/prepare ────────────────────────────────────────────────────────────────

  async prepare(threadId: string, base?: string): Promise<ForgePrepareResult> {
    const t = await this.forThread(threadId);
    const access = await this.access(t.ctx);
    const out: ForgePrepareResult = { access, branches: [], title: "", body: "", push: await this.pushState(t) };
    if (t.branch) out.branch = t.branch;
    if (!t.ctx.repo) return out;
    let def: string | null = null;
    if (access.capabilities.length > 0) def = await this.limiter.run(() => t.ctx.adapter!.defaultBranch(), true).catch(() => null);
    if (!def) {
      const local = await this.gitOut(t.root, ["symbolic-ref", "--quiet", "--short", `refs/remotes/${t.ctx.remote}/HEAD`]);
      if (local) def = local.replace(new RegExp(`^${t.ctx.remote}/`), "");
    }
    if (def) out.defaultBranch = def;
    const refs = await this.gitOut(t.root, ["for-each-ref", "--format=%(refname:short)", "--count=500", `refs/remotes/${t.ctx.remote}/`]);
    const prefix = `${t.ctx.remote}/`;
    out.branches = (refs ?? "")
      .split("\n")
      .map((s) => s.trim())
      .filter((s) => s.startsWith(prefix) && s !== `${prefix}HEAD` && s !== t.ctx.remote)
      .map((s) => s.slice(prefix.length))
      .filter((b) => b !== t.branch)
      .slice(0, 200);
    if (def && !out.branches.includes(def)) out.branches.unshift(def);
    const against = base ?? def ?? undefined;
    if (against) out.base = against;
    const fill = await this.fill(t, against);
    out.title = fill.title;
    out.body = fill.body;
    return out;
  }

  /** Title and description from the branch's commits (as `gh pr create --fill` does). */
  private async fill(t: ThreadForge, base: string | undefined): Promise<{ title: string; body: string }> {
    const range = base ? `${t.ctx.remote}/${base}..HEAD` : "-20";
    const log = await this.gitOut(t.root, ["log", "--reverse", "--no-merges", "--format=%s%x1f%b%x1e", ...(base ? [range] : ["-n", "20", "HEAD"])]);
    const commits = (log ?? "")
      .split("\x1e")
      .map((c) => c.trim())
      .filter(Boolean)
      .map((c) => {
        const [subject, body] = c.split("\x1f");
        return { subject: (subject ?? "").trim(), body: (body ?? "").trim() };
      });
    if (commits.length === 1) return { title: commits[0]!.subject, body: stripTrailers(commits[0]!.body) };
    const name = (t.branch ?? "").split("/").pop() ?? "";
    const words = name.replace(/[-_]+/g, " ").trim();
    const title = words ? words[0]!.toUpperCase() + words.slice(1) : commits[0]?.subject ?? "";
    return { title, body: commits.map((c) => `- ${c.subject}`).join("\n") };
  }

  private async pushState(t: ThreadForge): Promise<PushState> {
    if (!t.branch) return { upstream: false, ahead: 0, uncommitted: 0 };
    const upstream = await this.gitOut(t.root, ["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}"]);
    let ahead = 0;
    if (upstream) ahead = Number(await this.gitOut(t.root, ["rev-list", "--count", `${upstream}..HEAD`])) || 0;
    else {
      // No upstream: everything not on the remote at all.
      ahead = Number(await this.gitOut(t.root, ["rev-list", "--count", "HEAD", "--not", `--remotes=${t.ctx.remote}`])) || 0;
    }
    const status = await this.git(t.root, ["status", "--porcelain", "-z", "--untracked-files=normal"]);
    const uncommitted = status.code === 0 ? status.stdout.split("\0").filter((l) => /^.. /.test(l)).length : 0;
    return { upstream: !!upstream, ahead, uncommitted };
  }

  // ── forge/pr/create ──────────────────────────────────────────────────────────────

  create(p: ForgePrCreateParams): Promise<ForgePrCreateResult> {
    return this.core.ops.run("forge/pr/create", p.opId, async () => {
      if (typeof p.threadId !== "string" || typeof p.base !== "string" || typeof p.title !== "string") {
        throw new NotActed(new RpcError("invalid_params", "threadId, base, and title are required"));
      }
      if (!p.title.trim()) throw new NotActed(new RpcError("invalid_params", "a title is required"));
      return this.createNow({ threadId: p.threadId, base: p.base, title: p.title.trim(), body: p.body ?? "", draft: !!p.draft });
    });
  }

  /** Creates the pull request (pushing first when needed). Throws `outcome_unknown` when it cannot tell. */
  async createNow(req: { threadId: string; base: string; title: string; body: string; draft: boolean }): Promise<ForgePrCreateResult> {
    const t = await this.forThread(req.threadId).catch((e) => {
      throw new NotActed(e instanceof RpcError ? e : new RpcError("internal_error", String(e)));
    });
    const notActed = (code: string, message: string) => new NotActed(new RpcError(code, message));
    if (!t.ctx.repo) throw notActed("capability_missing", "This folder is not in a repository on a supported forge.");
    if (!t.branch) throw notActed("invalid_params", "The checkout is not on a branch (detached HEAD).");
    if (t.branch === req.base) throw notActed("invalid_params", `The pull request would merge ${t.branch} into itself.`);
    const access = await this.access(t.ctx);
    if (!access.capabilities.includes("prCreate")) throw notActed("capability_missing", access.reason ?? "Pull requests cannot be created from this target.");
    // Push first when the branch has unpushed commits or no upstream (D120).
    const push = await this.pushState(t);
    let pushed = false;
    if (!push.upstream || push.ahead > 0) {
      const r = await this.git(t.root, ["push", "-u", t.ctx.remote, t.branch], 120_000);
      if (r.code !== 0) {
        const why = r.failure === "timeout" ? "git push did not finish within 2 minutes" : (r.stderr || r.stdout).trim().split("\n").slice(-3).join(" ");
        throw notActed("forge_push_failed", `The branch could not be pushed, so no pull request was created: ${why}`);
      }
      pushed = true;
      void this.core.gitStatus.refresh(t.root, true);
    }
    const adapter = t.ctx.adapter!;
    let pr: PrSummary;
    try {
      pr = await this.limiter.run(() => adapter.create({ head: t.branch!, base: req.base, title: req.title, body: req.body, draft: req.draft }), true);
    } catch (err) {
      const fe = err as ForgeError;
      if (fe.kind !== "timeout") throw notActed("forge_failed", `The pull request was not created: ${fe.message}`);
      // It may have been created: look before saying so.
      const found = await adapter
        .list(t.branch)
        .then((l) => l.find((x) => x.state === "open" && x.title === req.title))
        .catch(() => undefined);
      if (!found) {
        throw new RpcError("outcome_unknown", "YAAC could not tell whether the pull request was created (the forge did not answer in time). It will not be created again automatically; check the branch's pull requests.", { pushed });
      }
      pr = found;
    }
    const e = this.entry(t.root, t.branch);
    e.result = { ...e.result, access, prs: [pr, ...e.result.prs.filter((x) => x.number !== pr.number)], at: nowIso() };
    delete e.result.error;
    e.fetchedAt = this.now();
    e.askedAt = this.now();
    this.core.broadcast("forge/prs/changed", e.result);
    return { pr, pushed };
  }

  // ── forge/pr/get ─────────────────────────────────────────────────────────────────

  async get(threadId: string, number: number): Promise<ForgePrGetResult> {
    const t = await this.forThread(threadId);
    const adapter = await this.adapterFor(t, "prGet");
    const d = await this.limiter.run(() => adapter.get(number), true).catch((e) => {
      throw new RpcError("forge_failed", (e as Error).message);
    });
    return { pr: d.pr, body: d.body, reviewers: d.reviewers, checks: d.checks.map((c) => ({ name: c.name, state: c.state, blocking: c.blocking, ...(c.url ? { url: c.url } : {}) })) };
  }

  /** The adapter, when `cap` is available now; otherwise the reason, as an error. */
  async adapterFor(t: ThreadForge, cap: string): Promise<ForgeAdapter> {
    if (!t.ctx.repo || !t.ctx.adapter) throw new RpcError("capability_missing", "This thread's folder is not in a repository on a supported forge (GitHub or Azure DevOps).");
    const access = await this.access(t.ctx);
    if (!access.capabilities.includes(cap as never)) throw new RpcError("capability_missing", access.reason ?? `${cap} is not available here.`);
    return t.ctx.adapter;
  }

  /** The thread's open pull request (the most recent), for tools called without a number. */
  async currentPr(t: ThreadForge): Promise<PrSummary | null> {
    if (!t.branch) return null;
    const r = await this.prs(t.rt.id, true);
    return r.prs.find((p) => p.state === "open") ?? r.prs[0] ?? null;
  }

  /** Runs `fn` under the forge limiter. */
  limited<T>(fn: () => Promise<T>): Promise<T> {
    return this.limiter.run(fn, true);
  }

  /** The folder for diagnostics. */
  static folderName(root: string): string {
    return path.basename(root);
  }
}

function threadIdOf(p: Record<string, unknown>): string {
  if (typeof p.threadId !== "string" || !p.threadId) throw new RpcError("invalid_params", "threadId is required");
  return p.threadId;
}

/** A result without its timestamp, to tell a real change from a re-ask. */
function stable(r: ForgePrsResult): unknown {
  const { at: _at, ...rest } = r;
  return rest;
}

function stripTrailers(body: string): string {
  return body
    .split("\n")
    .filter((l) => !/^(Co-Authored-By|Signed-off-by|Change-Id):/i.test(l.trim()))
    .join("\n")
    .trim();
}
