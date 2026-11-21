// The pull-request tools of the `yaac` MCP server (D121). Scoped to the calling thread's
// repository and branch; allowed like the other mesh tools; each call shows in the transcript
// as the harness's own tool call. Merge and approve are deliberately not offered.
//
// Definitions only here (the small `yaacd mcp` bridge bundle lists them); the handler is in
// `pr-tools.ts`.

import type { ToolDef } from "../mesh/tools.js";

const str = (description: string) => ({ type: "string", description });
const bool = (description: string) => ({ type: "boolean", description });
const num = (description: string) => ({ type: "integer", description, minimum: 1 });
const NUMBER = num("The pull request's number. Default: the open pull request of your branch.");

export const PR_TOOLS: ToolDef[] = [
  {
    name: "pr_list",
    description: "The pull requests whose head is your branch (or `branch`) in your repository, on GitHub or Azure DevOps: number, state (open, draft, merged, closed), checks, title, URL. Runs on your machine with its own `gh` / `az` login; says why when that is missing.",
    inputSchema: { type: "object", properties: { branch: str("Another branch of your repository. Default: your current branch.") }, additionalProperties: false },
  },
  {
    name: "pr_get",
    description: "One pull request in full: title, state, branches, description, reviewers and their votes, and every check or policy with its status.",
    inputSchema: { type: "object", properties: { number: NUMBER }, additionalProperties: false },
  },
  {
    name: "pr_create",
    description: "Open a pull request from your current branch. Pushes the branch first (`git push -u`) when it has unpushed commits or no upstream; uncommitted changes are not included, so commit first. Check `pr_list` before creating: never create a second one for the same branch. If the outcome is uncertain it says so and is not retried — check with `pr_list`.",
    inputSchema: {
      type: "object",
      properties: {
        title: str("Title."),
        body: str("Description (markdown)."),
        base: str("Target branch. Default: the repository's default branch."),
        draft: bool("Open it as a draft."),
      },
      required: ["title"],
      additionalProperties: false,
    },
  },
  {
    name: "pr_checks",
    description: "The pull request's checks (GitHub) or policies (Azure DevOps) with their status, and — with `log` — the log of the failed steps of a failed run (the end of it), to find out why CI fails.",
    inputSchema: {
      type: "object",
      properties: { number: NUMBER, log: bool("Also fetch the failed run's log (default true when something failed)."), check: str("Only the failed check whose name contains this.") },
      additionalProperties: false,
    },
  },
  {
    name: "pr_rerun_checks",
    description: "Run the pull request's failed checks again (failed GitHub Actions jobs, or failed Azure Pipelines build policies). Use it after a flaky failure, not to retry a real one.",
    inputSchema: { type: "object", properties: { number: NUMBER }, additionalProperties: false },
  },
  {
    name: "pr_comments",
    description: "The pull request's review comment threads: each thread's id, file and line, whether it is resolved, and its comments with their authors. Unresolved only unless `includeResolved`.",
    inputSchema: { type: "object", properties: { number: NUMBER, includeResolved: bool("Also resolved threads.") }, additionalProperties: false },
  },
  {
    name: "pr_reply",
    description: "Reply to a review comment thread, and/or mark it resolved. Take the thread id from `pr_comments`. Say what you changed; do not resolve a thread you did not address.",
    inputSchema: {
      type: "object",
      properties: { number: NUMBER, thread: str("The thread id from pr_comments."), body: str("Your reply."), resolve: bool("Also mark the thread resolved.") },
      required: ["thread"],
      additionalProperties: false,
    },
  },
];

export const PR_TOOL_NAMES = new Set(PR_TOOLS.map((t) => t.name));

/** The line the server instructions add for these tools. */
export const PR_INSTRUCTIONS =
  "- For pull requests of your branch (GitHub or Azure DevOps): `pr_list`, `pr_get`, `pr_create`, `pr_checks` (with the failed log), `pr_rerun_checks`, `pr_comments`, `pr_reply`. Merging and approving are left to the user.";
