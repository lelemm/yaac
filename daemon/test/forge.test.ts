// Pull requests (D119–D121): remote detection, the Azure DevOps tenant discovery, the forge
// methods against a fake `gh` / `az`, push-first, reasons, outcome-unknown, and the MCP tools.

import { test } from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as path from "node:path";
import { execFileSync } from "node:child_process";
import { parseRemote, slugOf } from "../src/forge/detect.js";
import { parseAuthHosts, checkStateOf } from "../src/forge/github.js";
import { AzureAdapter, TENANT_KEY, clearTokenCache, voteWord } from "../src/forge/azure.js";
import { RemoteStore } from "../src/forge/remotes.js";
import { PR_TOOLS } from "../src/forge/tools.js";
import { TOOLS, SERVER_INSTRUCTIONS } from "../src/mesh/tools.js";
import { FakeAzure, FakeGh, fakeRunner } from "./forge-fake.js";
import { client, opId, startDaemon, startThread, tmpHome, until, RpcFailure, waitTurnDone } from "./helpers.js";

// ── Detection ───────────────────────────────────────────────────────────────────────

test("remote detection: GitHub and GitHub Enterprise forms, Azure DevOps forms, everything else", () => {
  const table: Array<[string, string | null, string[]?]> = [
    ["https://github.com/lelemm/yaac", "github lelemm/yaac"],
    ["https://github.com/lelemm/yaac.git", "github lelemm/yaac"],
    ["git@github.com:lelemm/yaac.git", "github lelemm/yaac"],
    ["ssh://git@github.com/lelemm/yaac.git", "github lelemm/yaac"],
    ["https://user:tok@github.com/lelemm/yaac.git/", "github lelemm/yaac"],
    ["git@ghe.corp.example:team/svc.git", null],
    ["git@ghe.corp.example:team/svc.git", "github team/svc", ["ghe.corp.example"]],
    ["https://ghe.corp.example/team/svc", "github team/svc", ["GHE.corp.example"]],
    ["https://dev.azure.com/contoso/Big%20Project/_git/widgets", "azureDevOps contoso/Big Project/widgets"],
    ["https://contoso@dev.azure.com/contoso/proj/_git/widgets", "azureDevOps contoso/proj/widgets"],
    ["https://contoso.visualstudio.com/proj/_git/widgets", "azureDevOps contoso/proj/widgets"],
    ["https://contoso.visualstudio.com/DefaultCollection/proj/_git/widgets", "azureDevOps contoso/proj/widgets"],
    ["git@ssh.dev.azure.com:v3/contoso/proj/widgets", "azureDevOps contoso/proj/widgets"],
    ["contoso@vs-ssh.visualstudio.com:v3/contoso/proj/widgets", "azureDevOps contoso/proj/widgets"],
    ["https://gitlab.com/group/repo.git", null],
    ["https://github.com/only-owner", null],
    ["/srv/git/repo.git", null],
    ["", null],
  ];
  for (const [url, want, hosts] of table) {
    const r = parseRemote(url, hosts ?? []);
    assert.equal(r ? `${r.kind} ${slugOf(r)}` : null, want, url);
  }
});

test("gh auth status hosts and check states", () => {
  const out = "github.com\n  ✓ Logged in to github.com account a (keyring)\nghe.corp.example\n  X Failed to log in\n";
  assert.deepEqual(parseAuthHosts(out), ["github.com", "ghe.corp.example"]);
  assert.equal(checkStateOf({ __typename: "CheckRun", status: "IN_PROGRESS" }), "pending");
  assert.equal(checkStateOf({ __typename: "CheckRun", status: "COMPLETED", conclusion: "FAILURE" }), "failure");
  assert.equal(checkStateOf({ __typename: "CheckRun", status: "COMPLETED", conclusion: "SKIPPED" }), "skipped");
  assert.equal(checkStateOf({ __typename: "StatusContext", context: "ci", state: "SUCCESS" }), "success");
  assert.equal(voteWord(-5), "waiting for author");
});

// ── Azure DevOps tenant discovery ───────────────────────────────────────────────────

test("azure devops: the tenant that reaches the organisation is found, remembered, reused, and found again", async () => {
  clearTokenCache();
  const home = tmpHome("yaac-forge-");
  const fake = new FakeAzure();
  const store = new RemoteStore(home);
  const remote = "https://dev.azure.com/contoso/Big%20Project/_git/widgets";
  const repo = { org: "contoso", project: "Big Project", repo: "widgets", remote };
  const a = new AzureAdapter(fake.az, fake.fetch, store, repo, "devbox");
  // Three tenants signed in; only t-work's token reaches contoso.
  assert.deepEqual((await a.access()).capabilities.includes("prList"), true);
  assert.equal(new RemoteStore(home).get(remote, TENANT_KEY), "t-work", "persisted in remotes.json");
  const file = JSON.parse(fs.readFileSync(path.join(home, "remotes.json"), "utf8"));
  assert.deepEqual(Object.keys(file.remotes), ["dev.azure.com/contoso/Big%20Project/_git/widgets"]);
  assert.equal(file.remotes[Object.keys(file.remotes)[0]!].data.azureTenant, "t-work", "a generic data record, not a forge field");
  const probes = fake.http.filter((h) => h.url.includes("/_apis/projects")).length;
  assert.equal(probes, 3, "each tenant probed once, in order, until one works");

  // A new adapter (a restarted daemon) reuses the remembered tenant: no account list, no probe.
  clearTokenCache();
  fake.azCalls = [];
  fake.http = [];
  const b = new AzureAdapter(fake.az, fake.fetch, new RemoteStore(home), repo, "devbox");
  fake.prs.push({ pullRequestId: 41, title: "Widgets", status: "active", isDraft: true, sourceRefName: "refs/heads/feature/x", targetRefName: "refs/heads/main", creationDate: "2026-11-20T09:00:00Z" });
  fake.policies = [{ evaluationId: "e1", status: "rejected", configuration: { isBlocking: true, type: { displayName: "Build" }, settings: { displayName: "CI build" } }, context: { buildId: 55 } }];
  const list = await b.list("feature/x");
  assert.equal(fake.azCalls.filter((c) => c[1] === "list").length, 0);
  assert.equal(fake.http.filter((h) => h.url.includes("/_apis/projects")).length, 0);
  assert.equal(list[0]!.number, 41);
  assert.equal(list[0]!.draft, true);
  assert.equal(list[0]!.checks, "failing");
  assert.equal(list[0]!.url, "https://dev.azure.com/contoso/Big%20Project/_git/widgets/pullrequest/41");

  // The organisation moves to another tenant: a read fails with auth, discovery runs again.
  fake.goodTenant = "t-other";
  const again = await b.list("feature/x");
  assert.equal(again.length, 1);
  assert.equal(new RemoteStore(home).get(remote, TENANT_KEY), "t-other");

  // A write that meets an auth failure is never re-sent.
  fake.goodTenant = "t-home";
  const posts = () => fake.http.filter((h) => h.method === "POST").length;
  await assert.rejects(b.create({ head: "feature/y", base: "main", title: "T", body: "", draft: false }), /nothing was changed/);
  assert.equal(posts(), 1);
  // …and the next one discovers the tenant and goes through.
  const created = await b.create({ head: "feature/y", base: "main", title: "T", body: "B", draft: true });
  assert.equal(created.title, "T");
  assert.deepEqual(fake.http.filter((h) => h.method === "POST").at(-1)!.body, { sourceRefName: "refs/heads/feature/y", targetRefName: "refs/heads/main", title: "T", description: "B", isDraft: true });

  // Checks, the failed build's log, rerun, comments, reply, resolve.
  const d = await b.get(41);
  assert.equal(d.checks[0]!.name, "CI build");
  assert.equal(d.checks[0]!.blocking, true);
  assert.match(await b.failedLog(41), /test_parser failed/);
  assert.deepEqual(await b.rerunFailed(41), ["55"]);
  assert.deepEqual(fake.http.find((h) => h.method === "PATCH" && h.url.includes("/policy/evaluations/e1"))!.body, { status: "queued" });
  fake.threads = [
    { id: 1, status: "active", threadContext: { filePath: "/src/a.ts", rightFileStart: { line: 3 } }, comments: [{ id: 1, author: { displayName: "Ana" }, content: "Rename this", commentType: "text" }] },
    { id: 2, status: "active", comments: [{ id: 1, content: "Policy updated", commentType: "system" }] },
  ];
  const threads = await b.comments(41);
  assert.equal(threads.length, 1, "system threads are left out");
  assert.equal(threads[0]!.path, "/src/a.ts");
  await b.reply(41, "1", "Done");
  await b.resolve(41, "1");
  assert.ok(fake.http.some((h) => h.method === "PATCH" && h.url.includes("/threads/1?") && (h.body as { status: string }).status === "fixed"));
});

test("azure devops: no tenant reaches the organisation, az signed out, az missing", async () => {
  clearTokenCache();
  const home = tmpHome("yaac-forge-");
  const repo = { org: "contoso", project: "p", repo: "r", remote: "https://dev.azure.com/contoso/p/_git/r" };
  const fake = new FakeAzure();
  fake.goodTenant = "t-nowhere";
  const a = new AzureAdapter(fake.az, fake.fetch, new RemoteStore(home), repo, "devbox");
  assert.match((await a.access()).reason!, /None of the tenants az is signed in to on devbox can reach the Azure DevOps organisation contoso/);
  fake.tenants = [];
  assert.match((await new AzureAdapter(fake.az, fake.fetch, new RemoteStore(home), repo, "devbox").access()).reason!, /az is not logged in on devbox · run `az login` there/);
  fake.installed = false;
  assert.match((await new AzureAdapter(fake.az, fake.fetch, new RemoteStore(home), repo, "devbox").access()).reason!, /az is not installed on devbox/);
});

// ── The daemon: forge methods against a fake gh ─────────────────────────────────────

function git(cwd: string, ...args: string[]): string {
  return execFileSync("git", ["-c", "user.email=t@example.invalid", "-c", "user.name=T", "-c", "commit.gpgsign=false", ...args], { cwd, encoding: "utf8" }).trim();
}

/** A checkout whose origin is github.com/acme/widgets; pushes go to a local bare repository. */
function makeRepo(): { work: string; bare: string } {
  const dir = fs.mkdtempSync(path.join(tmpHome("yaac-forge-repo-"), "r"));
  const bare = path.join(dir, "remote.git");
  const work = path.join(dir, "work");
  execFileSync("git", ["init", "-q", "--bare", "-b", "main", bare]);
  execFileSync("git", ["init", "-q", "-b", "main", work]);
  git(work, "remote", "add", "origin", "https://github.com/acme/widgets.git");
  git(work, "config", `url.${bare}.pushInsteadOf`, "https://github.com/acme/widgets.git");
  fs.writeFileSync(path.join(work, "a.txt"), "a\n");
  git(work, "add", ".");
  git(work, "commit", "-q", "-m", "First");
  git(work, "push", "-q", "-u", "origin", "main");
  // A remote-tracking main and origin/HEAD, as a clone would have.
  git(work, "fetch", "-q", bare, "+refs/heads/*:refs/remotes/origin/*");
  git(work, "symbolic-ref", "refs/remotes/origin/HEAD", "refs/remotes/origin/main");
  git(work, "checkout", "-q", "-b", "feature/pr-badge");
  fs.writeFileSync(path.join(work, "b.txt"), "b\n");
  git(work, "add", ".");
  git(work, "commit", "-q", "-m", "Add the PR badge", "-m", "Shows the branch's pull request.\n\nCo-Authored-By: X <x@example.invalid>");
  return { work, bare };
}

async function setup(gh: FakeGh) {
  const { work, bare } = makeRepo();
  const daemon = await startDaemon(tmpHome(), { forge: { runner: fakeRunner(gh), minAskMs: 0, debounceMs: 50 } });
  const c = await client(daemon);
  const { threadId, turnId } = await startThread(c, "/fake say hello", { cwd: work });
  await c.request("thread/subscribe", { threadId, epoch: 1, afterSeq: 0 });
  await waitTurnDone(c, threadId, turnId);
  return { daemon, c, threadId, work, bare };
}

test("forge/prs: the branch's pull requests, open first, with checks; badge absent outside a forge", async () => {
  const gh = new FakeGh();
  gh.prs = [
    { number: 33341, title: "Old try", state: "CLOSED", headRefName: "feature/pr-badge", baseRefName: "main" },
    { number: 33342, title: "PR badge", state: "OPEN", isDraft: true, headRefName: "feature/pr-badge", baseRefName: "main", checks: [{ name: "test", status: "COMPLETED", conclusion: "FAILURE", runId: "9001" }] },
    { number: 33343, title: "Someone else's", state: "OPEN", headRefName: "feature/pr-badge", baseRefName: "main", owner: "stranger" },
    { number: 5, title: "Other branch", state: "OPEN", headRefName: "main", baseRefName: "main" },
  ];
  const { daemon, c, threadId, work } = await setup(gh);
  const r: any = await c.request("forge/prs", { threadId });
  assert.equal(r.root, fs.realpathSync(work) === work ? work : r.root);
  assert.equal(r.branch, "feature/pr-badge");
  assert.deepEqual(r.access.repo, { kind: "github", host: "github.com", slug: "acme/widgets", webUrl: "https://github.com/acme/widgets", fromFork: false });
  assert.ok(r.access.capabilities.includes("prCreate"));
  assert.deepEqual(r.prs.map((p: any) => p.number), [33342, 33341], "open first; another owner's same-named branch left out");
  assert.equal(r.prs[0].draft, true);
  assert.equal(r.prs[0].checks, "failing");
  const listCalls = () => gh.calls.filter((x) => x[0] === "pr" && x[1] === "list").length;
  assert.equal(listCalls(), 1);
  // Served from the cache while fresh; `refresh` asks again.
  await c.request("forge/prs", { threadId });
  assert.equal(listCalls(), 1);
  await c.request("forge/prs", { threadId, refresh: true });
  assert.equal(listCalls(), 2);

  // A thread outside any forge: no repo, so no badge.
  const plain = fs.mkdtempSync(path.join(tmpHome(), "plain"));
  const other = await startThread(c, "/fake say hi", { cwd: plain });
  const none: any = await c.request("forge/prs", { threadId: other.threadId });
  assert.equal(none.access.repo, undefined);
  assert.deepEqual(none.prs, []);
  await c.close();
  await daemon.stop("test");
});

test("forge/prs: not logged in and not installed give a reason; nothing is listed", async () => {
  const gh = new FakeGh();
  gh.loggedIn = false;
  const { daemon, c, threadId } = await setup(gh);
  const r: any = await c.request("forge/prs", { threadId });
  assert.deepEqual(r.access.capabilities, []);
  assert.match(r.access.reason, /^gh is not logged in on .+ · run `gh auth login` there$/);
  assert.equal(gh.calls.filter((x) => x[1] === "list").length, 0);
  await c.close();
  await daemon.stop("test");

  const gh2 = new FakeGh();
  gh2.installed = false;
  const s = await setup(gh2);
  const r2: any = await s.c.request("forge/prs", { threadId: s.threadId });
  assert.equal(r2.access.repo.kind, "github", "github.com is GitHub even without gh");
  assert.match(r2.access.reason, /gh is not installed on/);
  await s.c.close();
  await s.daemon.stop("test");
});

test("forge/prepare and forge/pr/create: default branch, prefill, push first, the new PR pushed to clients", async () => {
  const gh = new FakeGh();
  const { daemon, c, threadId, work, bare } = await setup(gh);
  fs.writeFileSync(path.join(work, "dirty.txt"), "uncommitted\n");
  const prep: any = await c.request("forge/prepare", { threadId });
  assert.equal(prep.defaultBranch, "main");
  assert.equal(prep.base, "main");
  assert.ok(prep.branches.includes("main"));
  assert.equal(prep.title, "Add the PR badge");
  assert.equal(prep.body, "Shows the branch's pull request.", "trailers are dropped");
  assert.deepEqual(prep.push, { upstream: false, ahead: 1, uncommitted: 1 });

  await c.request("forge/prs", { threadId });
  const op = opId();
  const created: any = await c.request("forge/pr/create", { opId: op, threadId, base: "main", title: "Add the PR badge", body: "Body", draft: true });
  assert.equal(created.pushed, true);
  assert.equal(created.pr.number, 100);
  assert.equal(created.pr.draft, true);
  assert.equal(execFileSync("git", ["--git-dir", bare, "rev-parse", "feature/pr-badge"], { encoding: "utf8" }).trim(), git(work, "rev-parse", "HEAD"), "pushed to the remote");
  assert.equal(git(work, "rev-parse", "--abbrev-ref", "@{upstream}"), "origin/feature/pr-badge", "with an upstream");
  assert.ok(fs.existsSync(path.join(work, "dirty.txt")), "uncommitted changes are left alone");
  const createArgs = gh.calls.find((x) => x[1] === "create")!;
  assert.deepEqual(createArgs.slice(0, 12), ["pr", "create", "-R", "github.com/acme/widgets", "--head", "feature/pr-badge", "--base", "main", "--title", "Add the PR badge", "--body", "Body"]);
  assert.ok(createArgs.includes("--draft"));
  const note = await until(() => c.notifications.find((n) => n.method === "forge/prs/changed" && n.params.prs.some((p: any) => p.number === 100)), "forge/prs/changed");
  assert.equal(note.params.branch, "feature/pr-badge");
  // The same opId again returns the recorded outcome; nothing is created twice.
  const again: any = await c.request("forge/pr/create", { opId: op, threadId, base: "main", title: "Add the PR badge", body: "Body", draft: true });
  assert.equal(again.pr.number, 100);
  assert.equal(gh.calls.filter((x) => x[1] === "create").length, 1);
  // Nothing to push the second time.
  const prep2: any = await c.request("forge/prepare", { threadId });
  assert.deepEqual(prep2.push, { upstream: true, ahead: 0, uncommitted: 1 });
  await c.close();
  await daemon.stop("test");
});

test("forge/pr/create: a timed-out create is outcome_unknown, recorded, and never sent again", async () => {
  const gh = new FakeGh();
  const { daemon, c, threadId } = await setup(gh);
  gh.createTimesOut = true;
  const op = opId();
  await assert.rejects(c.request("forge/pr/create", { opId: op, threadId, base: "main", title: "T", body: "" }), (e: unknown) => e instanceof RpcFailure && e.code === "outcome_unknown");
  await assert.rejects(c.request("forge/pr/create", { opId: op, threadId, base: "main", title: "T", body: "" }), (e: unknown) => e instanceof RpcFailure && e.code === "outcome_unknown");
  assert.equal(gh.calls.filter((x) => x[1] === "create").length, 1);
  // When the timed-out create did happen, it is found instead of reported unknown.
  gh.createTimesOut = true;
  gh.createdTimedOut = true;
  const found: any = await c.request("forge/pr/create", { opId: opId(), threadId, base: "main", title: "Second", body: "" });
  assert.equal(found.pr.title, "Second");
  // A definite refusal is not recorded as done: a corrected request may be sent.
  await assert.rejects(c.request("forge/pr/create", { opId: opId(), threadId, base: "feature/pr-badge", title: "Self", body: "" }), (e: unknown) => e instanceof RpcFailure && e.code === "invalid_params");
  await c.close();
  await daemon.stop("test");
});

test("forge/prs/changed after a commit; forge/pr/get with reviewers and checks", async () => {
  const gh = new FakeGh();
  gh.prs = [{ number: 7, title: "Badge", state: "OPEN", headRefName: "feature/pr-badge", baseRefName: "main", body: "Desc", checks: [{ name: "lint", status: "COMPLETED", conclusion: "SUCCESS" }] }];
  const { daemon, c, threadId, work } = await setup(gh);
  await c.request("forge/prs", { threadId });
  const before = gh.calls.filter((x) => x[1] === "list").length;
  gh.prs[0]!.checks = [{ name: "lint", status: "IN_PROGRESS", conclusion: "" }];
  fs.writeFileSync(path.join(work, "c.txt"), "c\n");
  git(work, "add", ".");
  git(work, "commit", "-q", "-m", "More");
  await daemon.core.gitStatus.refresh(daemon.core.thread(threadId)!.record.repo!.root, true);
  const n = await until(() => c.notifications.find((x) => x.method === "forge/prs/changed" && x.params.prs[0]?.checks === "pending"), "refresh after the commit");
  assert.equal(n.params.prs[0].number, 7);
  assert.ok(gh.calls.filter((x) => x[1] === "list").length > before);
  const d: any = await c.request("forge/pr/get", { threadId, number: 7 });
  assert.equal(d.body, "Desc");
  assert.deepEqual(d.reviewers, [
    { name: "reviewer1", state: "approved", required: false },
    { name: "reviewer2", state: "requested", required: false },
  ]);
  assert.equal(d.checks[0].name, "CI / lint");
  await c.close();
  await daemon.stop("test");
});

test("a fork with an upstream remote opens pull requests upstream, head owner:branch", async () => {
  const gh = new FakeGh();
  gh.parent = { owner: "upstream-org", repo: "widgets" };
  const { work, bare } = makeRepo();
  git(work, "remote", "add", "upstream", "https://github.com/upstream-org/widgets.git");
  const daemon = await startDaemon(tmpHome(), { forge: { runner: fakeRunner(gh), minAskMs: 0 } });
  const c = await client(daemon);
  const { threadId } = await startThread(c, "/fake say hello", { cwd: work });
  const r: any = await c.request("forge/prs", { threadId });
  assert.equal(r.access.repo.slug, "upstream-org/widgets");
  assert.equal(r.access.repo.fromFork, true);
  assert.ok(gh.calls.find((x) => x[1] === "list")!.includes("github.com/upstream-org/widgets"));
  await c.request("forge/pr/create", { opId: opId(), threadId, base: "main", title: "Upstream PR", body: "" });
  const args = gh.calls.find((x) => x[1] === "create")!;
  assert.equal(args[args.indexOf("--head") + 1], "acme:feature/pr-badge");
  assert.equal(new RemoteStore(daemon.config.home).get<{ owner: string }>("https://github.com/acme/widgets.git", "githubBase")!.owner, "upstream-org");
  assert.ok(fs.existsSync(bare));
  await c.close();
  await daemon.stop("test");
});

// ── The MCP tools ───────────────────────────────────────────────────────────────────

test("the yaac MCP server lists the PR tools, and they do what they say", async () => {
  for (const t of PR_TOOLS) assert.ok(TOOLS.some((x) => x.name === t.name), t.name);
  assert.ok(!TOOLS.some((t) => /merge|approve/.test(t.name)), "no merge, no approve");
  assert.match(SERVER_INSTRUCTIONS, /pr_create/);
  const gh = new FakeGh();
  const { daemon, c, threadId, work } = await setup(gh);
  const rt = daemon.core.thread(threadId)!;
  const call = async (tool: string, args: Record<string, unknown> = {}) => daemon.mesh.callTool(rt, { tool, args, meta: null });

  let r = await call("pr_list");
  assert.equal(r.isError, false);
  assert.match(r.text, /No pull requests from feature\/pr-badge in acme\/widgets/);
  r = await call("pr_get");
  assert.equal(r.isError, true);
  assert.match(r.text, /There is no pull request for the branch feature\/pr-badge/);

  r = await call("pr_create", { title: "Badge from the agent", body: "Why" });
  assert.equal(r.isError, false, r.text);
  assert.match(r.text, /^Pushed feature\/pr-badge first\. Created #100 \[open\] Badge from the agent/);
  r = await call("pr_create", { title: "Again" });
  assert.equal(r.isError, true);
  assert.match(r.text, /already has an open pull request: #100/);

  gh.prs[0]!.checks = [
    { name: "test", status: "COMPLETED", conclusion: "FAILURE", runId: "9001" },
    { name: "lint", status: "COMPLETED", conclusion: "SUCCESS", runId: "9002" },
  ];
  gh.logs["9001"] = "test\tRun tests\t2026-11-20T10:00:00Z FAIL parser_test.go:12: want 3, got 4\n";
  r = await call("pr_checks");
  assert.match(r.text, /CI \/ test: failure/);
  assert.match(r.text, /Failed log:\n.*want 3, got 4/s);
  assert.ok(gh.calls.some((x) => x[0] === "run" && x[1] === "view" && x.includes("--log-failed") && x[2] === "9001"));
  r = await call("pr_rerun_checks", { number: 100 });
  assert.match(r.text, /Asked to run again: 9001/);
  assert.deepEqual(gh.reruns, ["9001"]);

  gh.threads = [
    { id: "PRRT_1", isResolved: false, path: "src/a.ts", line: 3, comments: [{ id: "c1", author: "ana", body: "Rename this" }] },
    { id: "PRRT_2", isResolved: true, comments: [{ id: "c2", author: "bo", body: "Old" }] },
  ];
  r = await call("pr_comments");
  assert.match(r.text, /Thread PRRT_1 on src\/a\.ts:3:\n  ana: Rename this/);
  assert.doesNotMatch(r.text, /PRRT_2/);
  r = await call("pr_reply", { thread: "PRRT_1", body: "Renamed.", resolve: true });
  assert.equal(r.text, "Replied and resolved thread PRRT_1 on #100.");
  assert.equal(gh.threads[0]!.isResolved, true);
  assert.equal(gh.threads[0]!.comments.at(-1)!.body, "Renamed.");

  // Signed out: the tool says why, in words the agent can pass on (the cached access is
  // dropped by the auth failure of the next list).
  gh.loggedIn = false;
  r = await call("pr_list");
  assert.equal(r.isError, true);
  r = await call("pr_list");
  assert.match(r.text, /gh is not logged in on .+ · run `gh auth login` there/);
  assert.ok(fs.existsSync(work));
  await c.close();
  await daemon.stop("test");
});
