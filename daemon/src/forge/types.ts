// The forge layer's seams: how a CLI is run, how HTTPS is spoken, and what an adapter does.
// Both runners are injectable so tests (and a demo) can stand in a fake `gh` / `az`.

import type { CheckState, ForgeAccess, PrCheck, PrReviewer, PrSummary } from "../protocol/generated/types.js";
import { run, type RunResult } from "../util/exec.js";

export interface CliRunner {
  (program: string, args: string[], opts?: { cwd?: string; timeoutMs?: number; maxBytes?: number }): Promise<RunResult>;
}

/** Runs the target's own CLIs: never interactive, never a pager, never colour. */
export function cliRunner(env: NodeJS.ProcessEnv): CliRunner {
  const childEnv: NodeJS.ProcessEnv = {
    ...env,
    GH_PROMPT_DISABLED: "1",
    GH_NO_UPDATE_NOTIFIER: "1",
    GH_PAGER: "cat",
    PAGER: "cat",
    NO_COLOR: "1",
    CLICOLOR: "0",
    GIT_TERMINAL_PROMPT: "0",
    // A push must never wait on a passphrase or host-key prompt nobody can answer.
    GIT_SSH_COMMAND: env.GIT_SSH_COMMAND ?? "ssh -o BatchMode=yes",
    AZURE_CORE_NO_COLOR: "true",
    AZURE_CORE_ONLY_SHOW_ERRORS: "true",
    AZURE_CORE_COLLECT_TELEMETRY: "false",
    LC_ALL: "C",
  };
  return (program, args, opts = {}) => run(program, args, { env: childEnv, cwd: opts.cwd, timeoutMs: opts.timeoutMs ?? 30_000, maxBytes: opts.maxBytes ?? 8 * 1024 * 1024 });
}

export interface HttpResponse {
  status: number;
  /** The body as text. */
  text: string;
  contentType: string;
}

export interface HttpClient {
  (req: { method: string; url: string; token: string; body?: unknown; timeoutMs?: number }): Promise<HttpResponse>;
}

/** HTTPS from yaacd itself (on the target), with a bearer token. */
export const fetchHttp: HttpClient = async (req) => {
  const res = await fetch(req.url, {
    method: req.method,
    headers: {
      Authorization: `Bearer ${req.token}`,
      Accept: "application/json",
      ...(req.body !== undefined ? { "Content-Type": "application/json" } : {}),
    },
    body: req.body !== undefined ? JSON.stringify(req.body) : undefined,
    redirect: "manual",
    signal: AbortSignal.timeout(req.timeoutMs ?? 20_000),
  });
  return { status: res.status, text: await res.text(), contentType: res.headers.get("content-type") ?? "" };
};

/** An error with words the user (or an agent) can act on. */
export class ForgeError extends Error {
  constructor(
    message: string,
    readonly kind: "auth" | "missing" | "rateLimit" | "notFound" | "timeout" | "other" = "other",
  ) {
    super(message);
  }
}

export interface PrDetail {
  pr: PrSummary;
  body: string;
  reviewers: PrReviewer[];
  checks: CheckInfo[];
}

/** A check, with what is needed to fetch its log and rerun it. */
export interface CheckInfo extends PrCheck {
  /** GitHub Actions run id / Azure Pipelines build id. */
  runId?: string;
  /** Azure DevOps policy evaluation id (what a requeue targets). */
  evaluationId?: string;
}

export interface ReviewComment {
  id: string;
  author: string;
  body: string;
  at?: string;
}

export interface ReviewThread {
  id: string;
  resolved: boolean;
  path?: string;
  line?: number;
  comments: ReviewComment[];
}

export interface CreateRequest {
  head: string;
  base: string;
  title: string;
  body: string;
  draft: boolean;
}

/** One repository on one forge, as the target's CLI sees it. */
export interface ForgeAdapter {
  /** Whether the CLI is there and signed in; `capabilities` is empty with a reason if not. */
  access(): Promise<ForgeAccess>;
  list(branch: string): Promise<PrSummary[]>;
  get(number: number): Promise<PrDetail>;
  /** Creates a pull request. A timeout throws `ForgeError("…", "timeout")`: it may have happened. */
  create(req: CreateRequest): Promise<PrSummary>;
  defaultBranch(): Promise<string | null>;
  failedLog(number: number, check?: string): Promise<string>;
  rerunFailed(number: number): Promise<string[]>;
  comments(number: number): Promise<ReviewThread[]>;
  reply(number: number, threadId: string, body: string): Promise<void>;
  resolve(number: number, threadId: string): Promise<void>;
  /** The branch name the pull request's head is given as (`owner:branch` from a fork). */
  headRef(branch: string): string;
}

export function combineChecks(states: CheckState[]): "none" | "pending" | "passing" | "failing" {
  if (states.length === 0) return "none";
  if (states.includes("failure")) return "failing";
  if (states.includes("pending")) return "pending";
  return "passing";
}

/** Keeps the end of a long log: that is where a failure is. */
export function tail(text: string, maxBytes = 24 * 1024): string {
  if (Buffer.byteLength(text) <= maxBytes) return text;
  const cut = text.slice(-maxBytes);
  const nl = cut.indexOf("\n");
  return `… (${Math.round(Buffer.byteLength(text) / 1024)} KiB log; the last ${Math.round(maxBytes / 1024)} KiB follow)\n${nl >= 0 ? cut.slice(nl + 1) : cut}`;
}

/** Open first, then the most recently updated. */
export function orderPrs(prs: PrSummary[]): PrSummary[] {
  const rank = (p: PrSummary) => (p.state === "open" ? 0 : 1);
  return [...prs].sort((a, b) => rank(a) - rank(b) || (b.updatedAt ?? "").localeCompare(a.updatedAt ?? "") || b.number - a.number);
}
