# Harness surfaces

What Codex and Claude Code expose to a program, as observed on **2026-11-20**. Everything here
was read from installed tools or published packages, not recalled. Both harnesses change often;
re-run the commands under "How to re-verify" before depending on a detail.

| Component | Version examined |
| --- | --- |
| `codex` CLI | 0.170.0 |
| Codex app-server daemon (running) | 0.171.0 — **newer than the CLI on `PATH`** |
| `@openai/codex-sdk` | 0.171.0 |
| `claude` CLI (Claude Code) | 2.1.329 |
| `@anthropic-ai/claude-agent-sdk` | 0.3.329 |

## Codex

### App-server and daemon

`codex app-server` is a JSON-RPC server, marked experimental by the CLI.

**The daemon is versioned separately from the CLI.** `codex app-server daemon version` reports
both (`cliVersion`, `appServerVersion`), and the daemon keeps its own managed copy of Codex
under `~/.codex/packages/app-server-daemon/`. A schema generated from the CLI describes the
CLI's version, not necessarily the daemon you are talking to.

**Transports differ by listener** (tested):

- The daemon's control socket (`~/.codex/app-server-control/app-server-control.sock`) speaks
  **WebSocket**: an HTTP `Upgrade` request on the Unix socket is answered with
  `101 Switching Protocols`.
- A private `--listen stdio://` server speaks newline-delimited JSON.
- `codex app-server proxy` copies bytes between standard I/O and the socket. It does not
  translate, so what you send through it must already be WebSocket.

**Handshake:** `initialize` (with `clientInfo`, optional `capabilities`), then an `initialized`
notification.

**Home versus socket:** the managed daemon derives its default socket and state from
`CODEX_HOME`; `daemon start` takes no socket argument. A second endpoint comes from a second
home, or from an explicitly launched `app-server --listen unix://PATH`. Session store,
configuration, and sign-in follow the home, not the socket.

| Command | What it does |
| --- | --- |
| `codex app-server --listen <url>` | Runs a server. Transports: `stdio://` (default), `unix://`, `unix://PATH`, `ws://IP:PORT`, `off`. |
| `codex app-server daemon start \| stop \| restart \| version \| bootstrap \| update` | Manages a long-lived local daemon. `bootstrap` installs "durable local app-server management for SSH-driven use". |
| `codex app-server proxy [--sock <path>]` | Proxies standard I/O bytes to a running daemon's control socket. `--sock` selects which socket, which is what makes several daemons per machine addressable. |
| `codex app-server generate-ts` / `generate-json-schema --out <dir>` | Emits the protocol as TypeScript bindings or JSON Schema. **The authoritative reference for the protocol.** |
| `codex remote-control start \| stop \| pair` | Daemon with remote control enabled. Not used by YAAC. |

State on disk under `~/.codex/`: `app-server-control/`, `app-server-daemon/`, `sessions/`,
`session_index.jsonl`, `models_cache.json`.

**Client → server requests** (the ones YAAC cares about; the full list is over a hundred):

- Session: `initialize`
- Threads: `thread/start`, `thread/resume`, `thread/fork`, `thread/list`, `thread/read`,
  `thread/turns/list`, `thread/items/list`, `thread/name/set`, `thread/archive`,
  `thread/unarchive`, `thread/delete`, `thread/unsubscribe`, `thread/compact/start`,
  `thread/revert`, `thread/metadata/update`
- Turns: `turn/start`, `turn/steer`, `turn/interrupt`
- Models and account: `model/list`, `modelProvider/list`, `account/read`,
  `account/rateLimits/read`
- Also present: `fs/*` (read, write, list, watch), `command/exec`, `skills/list`, `plugin/*`,
  `mcpServerStatus/list`, `config/read`, `review/start`, `fuzzyFileSearch`

**Server → client requests** (the server waits for an answer):

`item/commandExecution/requestApproval`, `item/fileChange/requestApproval`,
`item/permissions/requestApproval`, `item/tool/requestUserInput`,
`mcpServer/elicitation/request`, `item/tool/call`, plus legacy `applyPatchApproval` and
`execCommandApproval`.

**Server → client notifications** (selection):

- Lifecycle: `thread/started`, `thread/status/changed`, `thread/name/updated`,
  `thread/archived`, `thread/deleted`, `thread/closed`, `thread/compacted`
- Turn: `turn/started`, `turn/completed`, `turn/plan/updated`, `turn/diff/updated`
- Items: `item/started`, `item/completed`, `item/agentMessage/delta`,
  `item/reasoning/textDelta`, `item/reasoning/summaryTextDelta`,
  `item/commandExecution/outputDelta`, `item/fileChange/patchUpdated`,
  `item/mcpToolCall/progress`
- Usage and account: `thread/tokenUsage/updated`, `account/rateLimits/updated`,
  `account/updated`
- Other: `model/rerouted`, `warning`, `error`, `serverRequest/resolved`

**Types worth knowing:**

- `Model`: `id`, `model`, `displayName`, `description`, `isDefault`, `hidden`,
  `supportedReasoningEfforts[]`, `defaultReasoningEffort`, `inputModalities[]` (defaults to
  `["text","image"]`), `serviceTiers[]`.
- `ReasoningEffort` is an open string in the schema, not a closed enum.
- `InputModality`: `text`, `image`, `audio`.
- `UserInput` variants: `text`, `image` (by URL or file id), `localImage { path }`, `audio`,
  `localAudio`.
- `TurnStartParams`: `threadId`, `input[]`, and optional per-turn `model`, `effort`, `cwd`,
  `approvalPolicy`, `sandboxPolicy`, `clientUserMessageId`. Per-turn `model` is what makes
  mid-thread model switching direct.
- `ThreadForkParams`: `threadId`, optional `lastTurnId`, `model`, `modelProvider`, `cwd`.
  Re-checked 2026-11-20 against CLI 0.170.0 / daemon 0.171.0: `lastTurnId` is inclusive and
  may not name a turn in progress; also `approvalPolicy`, `sandbox`, `config`, `ephemeral`,
  `excludeTurns` (return metadata only; use `thread/turns/list`). `ThreadForkResponse`:
  `thread`, `model`, `modelProvider`, `cwd`, `approvalPolicy`, `sandbox`, `reasoningEffort`.
  A live Codex→Codex fork with another model kept the conversation.
- `ThreadListParams`: `cursor`, `limit`, `cwd` filter, `searchTerm`, `archived`, `sortKey`,
  `sourceKinds`.
- `Thread`: `id`, `sessionId`, `cwd`, `gitInfo`, `name`, `preview`, `model`, `modelProvider`,
  `reasoningEffort`, `forkedFromId`, `parentThreadId`, `status`, `createdAt`, `updatedAt`.
  **`id` identifies the thread** ("Codex-generated thread IDs are UUIDv7"). **`sessionId` is
  "shared by threads that belong to the same session tree"** — it is not a conversation key.
- `TurnInterruptParams` requires `threadId` **and `turnId`**. `TurnSteerParams` requires
  `expectedTurnId`, and fails if that is not the active turn.
- `turn/start` `model` and `effort` are documented as overriding "this turn and subsequent
  turns".
- `ThreadRevertParams` (`beforeTurnId`): "only changes persisted conversation history. It does
  not revert local file changes."
- `ModelListParams`: `cursor`, `limit`, `includeHidden`, `modelProvider` — the listing is paged.
- `ThreadListParams.sourceKinds`: "when omitted or empty, defaults to interactive sources";
  `archived` true returns only archived threads, false or null only non-archived.
  Re-checked 2026-11-21 (CLI 0.170.0 `generate-ts`, daemon package 0.171.0): unchanged, so the
  full index needs two listings, active then archived. Also `cwd` (string or list),
  `useStateDbOnly`, `sectionId`, `excludedThreadIds`. `thread/unarchive` and the
  `thread/archived` / `thread/unarchived` notifications are present. On disk, the archived flag
  and `archived_at` are columns of the `threads` table in `$CODEX_HOME/state_<n>.sqlite`;
  `archived_sessions/` can be empty while threads are archived, so count archived threads
  through `thread/list`, not the folder.
- **Where `codex` lives**: the standalone installer puts a launcher at `~/.local/bin/codex`
  pointing into `$CODEX_HOME/packages/standalone/current/bin/codex`. `wsl.exe <cmd>` and
  `ssh host <cmd>` run a non-login shell whose `PATH` does not include `~/.local/bin`, so a
  daemon started that way does not find `codex` on `PATH` (seen 2026-11-21: a WSL target whose
  Codex instance was *not installed* until P130). After a reboot the Codex daemon is not
  running and its control-socket symlink dangles, so `yaacd` has to start it.
- `ToolRequestUserInputParams` (marked experimental): `threadId`, `turnId`, `itemId`,
  `isBlocking`, `questions[]` each with `id`, `header`, `question`, optional `options[]`
  (`label`, `description`), `isOther`, `isSecret`. The response maps question id to
  `{ answers: string[] }`.

### Codex SDK

**Not used by YAAC** — Codex is driven through its daemon only. Recorded here because it is
why that decision was made.

A thin wrapper that runs `codex exec --experimental-json` and parses its event stream. It depends on the
`@openai/codex` package for the binary and accepts `codexPathOverride` to use another one.

```ts
const codex = new Codex({ codexPathOverride, env, config });
const thread = codex.startThread({ model, workingDirectory, sandboxMode,
                                   modelReasoningEffort, approvalPolicy });
// or: codex.resumeThread(id, options)
const { events } = await thread.runStreamed(input, { signal });
```

- Input: a string, or parts of type `text` and `local_image { path }`.
- Events: `thread.started`, `turn.started`, `turn.completed` (with usage), `turn.failed`,
  `item.started`, `item.updated`, `item.completed`, `error`.
- Item types: `agent_message`, `reasoning`, `command_execution`, `file_change`,
  `mcp_tool_call`, `web_search`, `todo_list`, `error`.
- **Not available:** listing threads, reading history, fork, listing models, interactive
  approvals, questions to the user, steering. Approval policy is set per thread up front.
- `thread.id` is `null` until the first turn starts; the id arrives in `thread.started`.
- `CodexOptions.env`: "When provided, the SDK will not inherit variables from `process.env`."

### Codex CLI, for reference

`codex exec` accepts `--model`, `--image`, `--cd`, `--sandbox`, `--json`, `-c key=value`
(for example `-c model_reasoning_effort="high"`), and has `resume` and `fork` subcommands.

## Claude Code

### Agent SDK

The SDK runs a Claude Code process and exchanges messages with it. It ships the Claude Code
binary through per-platform optional packages and accepts `pathToClaudeCodeExecutable` to use an
installed one instead. Requires Node ≥ 18.14.

```ts
const q = query({ prompt: asyncIterableOfUserMessages, options });
for await (const message of q) { … }
```

**`options`** (selection): `cwd`, `model`, `effort`, `thinking`, `permissionMode`, `canUseTool`,
`includePartialMessages`, `resume`, `sessionId`, `forkSession`, `resumeSessionAt`, `continue`,
`persistSession`, `settingSources`, `additionalDirectories`, `env`,
`pathToClaudeCodeExecutable`, `spawnClaudeCodeProcess`, `sessionStore`, `fallbackModel`.

**Asking the user:** permission prompts arrive through the `canUseTool` callback. Structured
questions are the `AskUserQuestion` tool, whose input is `questions[]` each with `question`,
`header`, 2–4 `options` (`label`, `description`, optional `preview`), and `multiSelect`. Other
blocking dialogs arrive through `onUserDialog`, which must be opted into by listing the dialog
kinds the client can render. After a transport gap, `reinitialize()` returns the requests the
process is still blocked on so they can be re-delivered.

**Effort on a live query:** `applyFlagSettings({ effortLevel })` is documented for this.

**Methods on the running query:** `interrupt()`, `setModel()`, `setPermissionMode()`,
`applyFlagSettings()`,
`supportedModels()`, `supportedCommands()`, `supportedAgents()`, `accountInfo()`,
`getContextUsage()`, `mcpServerStatus()`, `rewindFiles()`, `streamInput()`,
`initializationResult()`, `reinitialize()`, `close()`.

**Session functions** (no running query needed): `listSessions()`, `getSessionInfo()`,
`getSessionMessages()`, `forkSession()`, `renameSession()`, `tagSession()`, `archiveSession()`,
`unarchiveSession()`, `deleteSession()`, `listSubagents()`, `getSubagentMessages()`.

`forkSession(sessionId, { upToMessageId?, title?, dir?, sessionStore? })` → `{ sessionId }`
(re-checked 2026-11-20, SDK 0.3.329, CLI 2.1.330): `upToMessageId` slices inclusive at that
message `uuid`, omitted copies everything; the title defaults to the original's plus " (fork)".
A live Claude→Claude fork kept the conversation.

**Types worth knowing:**

- `ModelInfo`: `value`, `displayName`, `description`, `resolvedModel?`, `supportsEffort?`,
  `supportedEffortLevels?`, `supportsAdaptiveThinking?`, `supportsFastMode?`,
  `supportsAutoMode?`. **There is no image-input field.**
- `SDKSessionInfo`: `sessionId`, `summary`, `lastModified`, `createdAt?`, `customTitle?`,
  `firstPrompt?`, `gitBranch?`, `cwd?`, `tag?`, `archived?`.
- `ListSessionsOptions`: `dir?` (omit for all projects), `limit`, `offset`, `includeWorktrees`
  (default true), `includeProgrammatic` (default true; covers SDK and daemon-worker sessions).
  There is no archived filter: `archiveSession()` "stays on disk, and `listSessions` still
  returns it, with `archived: true`"; `unarchiveSession()` makes it list with `archived: false`;
  a prompt sent in an archived session unarchives it (re-checked 2026-11-21, SDK 0.3.329).

Images are sent as image content blocks inside the user message, not as paths.

### The Claude Code daemon

Claude Code has an on-demand local daemon that supervises background sessions.

- State on disk under `~/.claude/`: `daemon/` (containing `control.key`, `roster.json`,
  `attach-journal/`, `dispatch/`), `daemon.lock`, `daemon.log`, `daemon.status.json`
  (`supervisorPid`, `workers`).
- CLI surface: `claude --bg` starts a background session; `claude agents --json` lists active
  sessions — **interactive and background** — as
  `{ id, cwd, kind, startedAt, sessionId, name, state }`, which is the signal YAAC uses to tell
  that a session is in use elsewhere; `claude attach <id>` opens
  one in a terminal; `claude logs`, `stop`, `respawn`, `rm` manage them.
- The SDK's type definitions mention daemon sessions in passing (`includeProgrammatic`,
  `reinitialize()` "after … reattaching to a daemon") but export **no function to connect to
  the daemon and drive a turn**.

Conclusion: there is a daemon, and its sessions can be listed, but we found no supported way for
a third-party program to converse through it. This is why the Claude daemon backend is an empty
slot in the [provider design](../04-providers.md#daemon-backend-reserved) and an
[open decision](../decisions.md).

## How to re-verify

```sh
# Codex: dump the app-server protocol and list every method
codex app-server generate-json-schema --out /tmp/codex-proto
codex app-server --help && codex app-server daemon --help && codex app-server proxy --help

# SDKs: fetch the packages and read their type definitions (read only — do not execute)
npm pack @openai/codex-sdk @anthropic-ai/claude-agent-sdk
#   codex-sdk:  package/dist/index.d.ts
#   claude sdk: package/sdk.d.ts

# Claude: CLI surface for background sessions
claude --help && claude agents --help && claude agents --json
```

## Verified against the real Codex daemon (0.171.0, 2026-11-20)

Run by the worker that built the Codex provider. These replace "read from source" for the
matching rows in the mesh table further down.

- **WebSocket handshake** on the control socket: confirmed. The reply carries
  `x-codex-websocket-max-unfragmented-message-bytes: 16777216`.
- **The `initialize` reply** holds only `userAgent`, `codexHome`, `platformFamily`,
  `platformOs`. There is **no version field** — the server version is inside `userAgent` — and
  **no account**; the account comes from `account/read`.
- **`model/list` is paged**: confirmed. `includeHidden` adds hidden models.
- **`thread/list` defaults to interactive sources**: confirmed. An **unknown source kind fails
  the whole request**. Threads started by app-server clients — the Codex TUI, and YAAC itself —
  are recorded with source `vscode`; the client's name is in `originator`.
- **Per-thread `config` on `thread/start` works**: an `mcp_servers.<name>` entry passed there
  made Codex start that MCP server for that thread, on start and on fork.
  `developerInstructions` was accepted.
- **`turn/start` on a busy thread joins the running turn**: confirmed — same turn id back, no
  new `turn/started`, the input added to that turn.
- The decisions an approval offers (`availableDecisions`) arrive only when the client opts in
  to the experimental API.
- Lifecycle notifications reach every connection; turn and item events reach subscribers only.
  A pending request is re-sent to a connection that subscribes later.
- `turn/completed` carries no usage; usage arrives through `thread/tokenUsage/updated`.
- `thread/delete` fails while a fork still references the thread.
- **Still unchecked:** the calling thread's id in MCP tool-call metadata, the tool timeout
  setting, connection sharing, tool-output input, `thread/inject_items`, whether developer
  instructions survive a cold resume, and a live `requestUserInput`.

## Verified live while building the mesh (2026-11-20)

- **Codex states the calling thread on every MCP tool call**: `_meta` carries `threadId`
  (equal to the calling thread), `sessionId`, `callId`, `itemId`, `windowId`, a progress
  token, and turn metadata. Claude Code sends a progress token and its tool-use id.
- **A per-thread `mcp_servers` entry works** on Codex, and accepts
  `default_tools_approval_mode = "approve"` — without it Codex asks the user to allow each
  call to the server, as a form request.
- `tool_timeout_sec` is accepted on that entry; its effect was not exercised.
- **Claude Code defers MCP tool schemas** behind its tool search, so a small model may call a
  tool with a guessed parameter name on its first try.
- **One `yaacd mcp` bridge process costs about 80 MB** resident (a bare Node process is about
  43 MB). There is one per live Codex thread.
- Still unsettled: tool-output-role input on Codex, whether developer instructions survive a
  cold resume, and Codex's MCP connection sharing.

## Verified against the real Claude SDK and CLI (2026-11-20)

Run by the worker that built the Claude provider, not read from documentation:

- `applyFlagSettings({ effortLevel })` **takes effect on a live query**, without spending a
  turn. `setModel()` resets effort to the new model's default, so the chosen effort has to be
  applied again after a model switch.
- `claude agents --json` entries differ by kind. Interactive:
  `{ pid, cwd, kind: "interactive", startedAt, sessionId, name, status }`. Background:
  `{ id, cwd, kind: "background", startedAt, sessionId, name, state }`.
- **A process started through the SDK also appears in that list**, as `kind: "interactive"`
  with its pid. Anything using the list to detect "in use elsewhere" must exclude its own
  children.
- The `sessionId` option is honoured, and the `uuid` set on a user message is kept in the
  stored transcript — so a turn can have the same id live and in history.
- The complete assistant message arrives before the stream's `content_block_stop`.
- **An idle query's child process uses about 229 MB of memory.** An active one was not measured.
- The SDK imports only Node built-ins and bundles into a single file with esbuild unchanged.

## For the agent mesh

Facts that the [mesh design](../10-agent-mesh.md) leans on. **Provenance matters here:** the
rows marked *source* were reported by the Codex reviewer from reading the Codex 0.171.0 release
source and its tests — they were not run by us. Spike S10 exists to confirm them.

| Fact | How we know |
| --- | --- |
| `ThreadStartParams` has a free-form `config` object and a `developerInstructions` string. | Schema, read here. |
| `codex mcp add` accepts a stdio command with `--env`, or a `--url` with `--bearer-token-env-var`. | CLI help, read here. |
| The Codex schema defines `DynamicToolSpec` and a server request `item/tool/call`. | Schema, read here. |
| The Claude Agent SDK accepts `mcpServers` per query, including an in-process `type: "sdk"` server, and an `appendSystemPrompt`. | SDK type definitions, read here. |
| One Codex thread can be given its own MCP server through `thread/start` config, as the dotted key `mcp_servers.<name>` with `command`, `args`, `env`. | *Source.* The reviewer also ran the 0.171.0 binary and saw it parse this configuration. |
| A stdio MCP server is one child process per loaded thread unless the experimental `mcp_connection_sharing` feature is on. | *Source*; the feature flag was seen listed as under development and off. |
| Codex's MCP `tools/call` requests carry the calling thread in `params._meta.threadId` (also `sessionId`, `callId`, and turn metadata). | *Source.* |
| The MCP tool-call timeout defaults to 300 seconds and is set per server with `tool_timeout_sec`. | *Source* (the public guide says 60). The binary accepted `tool_timeout_sec`. |
| `turn/start` on a thread with a turn in progress **steers that turn** and returns its id; it does not queue a new turn. | *Source* (an upstream test). |
| `turn/start` can take `input: []` with a `toolOutput` so that agent-originated input arrives in the tool role. | *Source.* `toolOutput` is present in the schema read here. |
| `developerInstructions` persists in history once the first turn has run; it must be passed again on a cold resume, and is ignored when rejoining a loaded thread. `thread/inject_items` can add a developer message to an existing thread. | *Source.* `thread/inject_items` is in the method list read here. |
| A thread with no subscribers can be unloaded once idle; a running turn survives an unsubscribe. | *Source* (upstream tests). |

## Forge CLIs for pull requests (2026-11-21)

What `yaacd`'s forge layer (`daemon/src/forge/`) relies on, for D119–D121.

**GitHub — `gh` 2.45.0, verified on the development machine** (help output, and read-only
calls against `lelemm/yaac` and the public `cli/cli`):

| Use | Command | Checked |
| --- | --- | --- |
| Signed in? GHE hosts | `gh auth status [--hostname H]` — hosts are the unindented lines; exit 1 when signed out ("You are not logged into any GitHub hosts…") | live |
| Branch's PRs | `gh pr list -R HOST/OWNER/REPO --head B --state all --limit 20 --json number,title,state,isDraft,url,headRefName,baseRefName,statusCheckRollup,author,updatedAt,headRepositoryOwner` — `--head` also matches same-named branches of other forks (filter on `headRepositoryOwner.login`); `state` is `OPEN`/`MERGED`/`CLOSED` | live |
| One PR | `gh pr view N -R … --json …,body,latestReviews,reviewRequests` | live |
| Checks | `statusCheckRollup`: `CheckRun {name,status,conclusion,detailsUrl,workflowName}` or `StatusContext {context,state,targetUrl}`. **`gh pr checks` has no `--json` in 2.45.** The Actions run id is in `detailsUrl` (`/actions/runs/<id>/job/<id>`) | live |
| Failed log / rerun | `gh run view <run> -R … --log-failed`; `gh run rerun <run> -R … --failed` | help only (no PR of ours to run them on) |
| Create | `gh pr create -R … --head [owner:]B --base T --title … --body … [--draft]` — prints the new URL | help only (no real PR created) |
| Default branch, fork parent | `gh repo view HOST/OWNER/REPO --json defaultBranchRef` / `--json parent` | live |
| Review threads | `gh api --hostname H graphql` with `repository.pullRequest.reviewThreads{id isResolved path line comments{…}}`; mutations `addPullRequestReviewThreadReply(input:{pullRequestReviewThreadId, body})` and `resolveReviewThread(input:{threadId})` | query live; mutations by schema introspection |
| Needs a login | exit code 4 ("To get started with GitHub CLI, please run: gh auth login") | live |

**Azure DevOps — `az` is not installed on the development machine.** Built from the Microsoft
Learn reference pages (az `account`, `config`; Azure DevOps REST 7.1) and tested against a fake:

- Confirmed in the docs: `az account list [--all]`; `az account get-access-token --resource
  … --tenant <id>` (with `expires_on` POSIX and `expiresOn` local); the Azure DevOps resource id
  `499b84ac-1321-427f-aa17-267ca6975798`; the REST paths for repositories, pull requests
  (`searchCriteria.sourceRefName`, `searchCriteria.status=all`), create (`isDraft`), policy
  evaluations (artifact `vstfs:///CodeReview/CodeReviewId/{projectId}/{prId}`, api-version
  `7.1-preview.1`, requeue by PATCH `status: queued`), build timeline, PR threads, thread
  comments, thread update (`status: fixed`); vote values 10/5/0/-5/-10.
- **Not confirmed**: the `tenantId`/`homeTenantId` and `accessToken` field names of the `az`
  output; what a token from the wrong tenant gets back (treated as: anything but 200 with
  JSON); the build-log endpoint `…/build/builds/{id}/logs/{logId}`; the timeline path without
  `{timelineId}`; the PR web URL `{repo webUrl}/pullrequest/{id}`; `context.buildId` in a policy
  evaluation (untyped in the schema); `$top` on the projects probe.

## Take over (verified 2026-11-21)

What happens when YAAC sends to a conversation another program holds (D140). Run with scratch
sessions only, deleted afterwards; no existing session was touched.

**Codex** — a private `codex app-server --listen unix://…` (CLI 0.170.0) with a scratch
`CODEX_HOME`, two WebSocket clients A and B:

- A starts a thread and runs a turn. B's `thread/read` shows it `idle`; B's `thread/resume`
  of the thread A has loaded **succeeds**, and B's `turn/start` is **accepted** and runs.
- A, still subscribed, **receives B's turn** (`turn/started`, `turn/completed`), and can start a
  turn afterwards itself. Codex does not lock a thread to one client: "the other program will
  not see these messages" is not literally true for a Codex client that stays subscribed — it
  gets the events; whether it shows them is up to that client.
- Codex cannot name which client started a turn; `yaacd` names the holder by that turn.
  `turn/start` during another client's turn joins it (known), so a takeover is refused then.

**Claude Code** — `claude` 2.1.330 and `@anthropic-ai/claude-agent-sdk` 0.3.329, a session
created with `claude -p --session-id`:

- Opened interactively (`claude --resume <id>` in tmux), `claude agents --json` lists it as
  `{ pid, kind: "interactive", sessionId, status: "idle" }`. An SDK `query({ resume: <id> })`
  then **runs normally** — same session id, a normal `result` — and the terminal does not show
  that turn.
- Run as a background session (`claude --bg --resume <id>`), the list shows
  `{ pid, id, kind: "background", sessionId, status: "idle", state: "done" }` — the pid is the
  shared background host's, so the id names it. An SDK resume is **refused at start-up**: stderr
  "That session is running in the background (<id>). Run `claude attach <id>` to open it, or
  `claude stop <id>` first to resume it here. Add --fork-session to branch off a copy instead.",
  exit code 1; with `CLAUDE_CODE_STARTUP_FAILURE_RESULTS=1` also an `error_during_execution`
  result with `startup_failure_reason: "session_held_by_background"`. Even `state: "done"`
  holds it. YAAC therefore disables **Take over** for a background holder with that reason and
  never stops the session itself.

## Folder trust (verified 2026-11-22)

Whether YAAC can mark a folder trusted for a harness (D147). Every check ran against throwaway
homes under `/tmp`; the owner's `~/.codex` and `~/.claude*` were not touched.

**Codex** — `codex app-server` (stdio) of codex-cli 0.170.0 with a scratch `CODEX_HOME`, and
`codex app-server generate-ts`:

- Trust is `[projects."<abs path>"] trust_level = "trusted"` (or `"untrusted"`) in the user's
  `config.toml`. The app-server has `config/read { includeLayers?, cwd? }` →
  `{ config, origins, layers }` (`config.projects` is the effective map; each layer has a
  `version`, `sha256:…`), `config/value/write { keyPath, value, mergeStrategy, filePath?,
  expectedVersion? }` and `config/batchWrite { edits, … }`. `MergeStrategy` is
  `"replace" | "upsert"`; `upsert` merges tables deeply (other keys of the folder's table and
  other folders are kept). The reply is `{ status: "ok" | "okOverridden", version, filePath }`.
- `keyPath` is split at dots: `projects./tmp/a.b/c` wrote `[projects."/tmp/a"."b/c"]`. A quoted
  segment (`projects."/tmp/a.b/c".trust_level`) is parsed correctly, but the robust form is
  `keyPath: "projects"` with the folder as a key of the value, `{ "<folder>": { trust_level:
  "trusted" } }`, `upsert` — it round-trips spaces, dots, backslashes and quotes
  (`C:\Users\a "q".b` was written as a literal key). That is what `yaacd` sends.
- A stale `expectedVersion` is refused: JSON-RPC error `-32600`, `data.config_write_error_code:
  "configVersionConflict"`, "Configuration was modified since last read." The user layer has
  a version even when `config.toml` does not exist yet.
- Which entry Codex consults (seen through the `disabledReason` of project config layers with
  `config/read { cwd, includeLayers: true }`): an entry for the working directory itself, or
  for the root of its git repository — for a linked worktree, the **main** checkout's root
  ("add /…/repo as a trusted project"). An entry for a mere ancestor folder does not count;
  an exact `untrusted` entry for a subfolder overrides a trusted repository root.

**Claude Code** — `claude` 2.1.330 and `@anthropic-ai/claude-agent-sdk` 0.3.329, with `HOME`
and `CLAUDE_CONFIG_DIR` set to scratch folders:

- Trust is `projects["<abs path>"].hasTrustDialogAccepted: true` in `.claude.json`
  (`$CLAUDE_CONFIG_DIR/.claude.json`; `~/.claude.json` by default). An ordinary SDK session
  records nothing (the trust dialog is skipped in non-interactive use, per `claude --help`).
- The CLI has no command that sets it (`claude --help` lists none; there is no `config`
  subcommand). The SDK's `query()` passes an option `workspaceTrust: { accepted: true,
  directory }` to the CLI's `initialize`, and with it the CLI wrote the flag and answered
  `workspace_trust_recorded: true`. But the option is **not in the SDK's published types**
  (`sdk.d.ts` has no `workspaceTrust`), and the CLI's schema marks it `@internal`: "the host's
  attestation that the user accepted a trust dialog". YAAC does not use it, and does not edit
  `.claude.json`; Claude Code does not report `folderTrust`.

## Fast mode (checked 2026-11-21)

Read from `codex app-server generate-ts` (CLI 0.170.0, against daemon 0.171.0) and the
installed `@anthropic-ai/claude-agent-sdk` 0.3.329 (its bundled CLI and `claude` 2.1.330),
then one real turn each (threads deleted afterwards).

- **Codex.** `Model.serviceTiers[]` is `{ id, name, description }`; on this machine every listed
  model has `{ id: "priority", name: "Fast", description: "1.5x speed…" / "2x speed, increased
  usage" }`, one also `ultrafast` ("Ultrafast"). `additionalSpeedTiers` (`["fast"]`) is
  deprecated in favour of `serviceTiers`; `defaultServiceTier` is the catalog default.
  `turn/start` takes `serviceTier` ("this turn and subsequent turns") and `serviceTierForTurn`
  ("only when this request starts a new turn. Use \"default\" for standard speed … Does not
  change the thread's tier"); `thread/start` takes `serviceTier`; config has `service_tier`.
  A real turn with `serviceTierForTurn: "priority"` on `gpt-6.2-sol` completed.
- **Claude Code.** `ModelInfo.supportsFastMode?` from `supportedModels()` (here: `default`,
  `opus`, `fable`, `claude-opus-5`, `claude-opus-4-8` say true; the others say nothing). The
  switch is the settings key `fastMode` (flag layer: `Options.settings` or
  `applyFlagSettings`). Without it in the flag layer the SDK reports
  `fast_mode_disabled_reason: "sdk_opt_in_required"`. `system/init` and `result` carry
  `fast_mode_state` (`off` / `cooldown` / `on`) and `fast_mode_disabled_reason`. A real turn
  with `settings: { fastMode: true }` reported `on` at init but `off` /
  `extra_usage_disabled` in its result: this account cannot be served fast mode, which the
  per-model flag does not reveal.

## Markup inside messages

*Checked 2026-11-21 against Claude Code **2.1.330** (`claude --version`), the Agent SDK
**0.3.329**, and codex-cli **0.170.0**. Owner's request: D141; how YAAC treats it: P120–P123.*

Both harnesses store a user-role message as one text and wrap parts of it in tags that are not
the user's words: pasted text, slash commands, context for the model, envelopes around messages
from other agents and background tasks. YAAC must not show those tags. Each provider has a
table of what its harness writes (`daemon/src/providers/claude/markup.ts`,
`daemon/src/providers/codex/markup.ts`); one engine applies it (`daemon/src/providers/markup.ts`).
Only listed tags are transformed; an unknown tag, an unclosed one, and other text with angle
brackets stay text.

**How this was checked.** Tag *names and structure only* were collected — no message content was
copied anywhere:

- the harness binaries' string tables (`strings -n 8` on the `claude` and `codex` binaries),
  searched for `<tag>` literals and read around the code that builds user-role messages.
  Claude Code keeps its tag names as constants: `command-name`, `command-message`,
  `command-args`, `bash-input`, `bash-stdout`, `bash-stderr`, `bash-exit-code`,
  `local-command-stdout`, `local-command-stderr`, `local-command-caveat`, `task-notification`,
  `task-id`, `tool-use-id`, `task-type`, `output-file`, and a list `teammate-message`,
  `cross-session-message`, `agent-message`, `system-reminder`, `user-prompt-submit-hook`,
  `fork-boilerplate`, `user-memory-input`, `ide_selection`, `ide_opened_file`, `terminal`,
  `post-tool-use-hook`, `mcp-resource-update`;
- the SDK's types (`sdk.d.ts`: `SDKMessageOrigin`, `SessionMessage`) and what
  `getSessionMessages` really returns: it passes `origin`, `is_meta` and `isCompactSummary`
  through, though the type does not declare them;
- the owner's stored transcripts (`~/.claude/projects/*/*.jsonl`, 365 files;
  `~/.codex/sessions/**/*.jsonl`, 213 files), read by scripts that print only tag names,
  attribute names, counts, origin kinds and masked shapes (a run of text printed as `~N~`);
- the Codex app-server schema (`codex app-server generate-ts`): `UserInput`, `TextElement`,
  `ThreadItem`, `HookPromptFragment`.

### Claude Code markup

Claude Code says *who* a user-role message is from in `origin.kind` (`human`, `peer`,
`task-notification`, `coordinator`, `channel`, `observer`, `observer-activity`,
`auto-continuation`, `unclassified`; absent on older or harness-written messages), and marks
what it wrote itself with `isMeta` (`is_meta` from the SDK) and `isCompactSummary`. Counts are
user-role messages in the owner's transcripts carrying the tag.

| Tag | Where (seen) | What it carries | How YAAC shows it |
| --- | --- | --- | --- |
| `<pasted_content id="…">` … `</pasted_content id="…">` | human (87) | A paste the CLI kept apart from the typed text; the **closing tag repeats the id**. 20 of 87 sat inside a code fence the user typed (` ```lang ` before, ` ``` ` after) — which is why it rendered as a code block with line numbers. | `pasted` part: "Pasted text · N lines", collapsed; a fence directly around it goes with it (then shown as code) |
| `<command-name>/x</command-name>` `<command-message>x</command-message>` `<command-args>a</command-args>` | no origin (23) | A slash command the user ran | `command` part: chip "/x a" |
| `<local-command-stdout>`, `<local-command-stderr>` | no origin (23) | Output of a local command (`/model`, `/clear`) | `context` "Command output" / "Command error output" |
| `<local-command-caveat>` | isMeta (23) | "Caveat: the messages below were generated by the user running local commands…" | `context` "Note from Claude Code" |
| `<bash-input>`, `<bash-stdout>`, `<bash-stderr>`, `<bash-exit-code>` | no origin (11) | A `!` shell command and its output | `command` "!cmd"; output as `context` "Shell output" |
| `<system-reminder>` (also `id="…"`, closed with the id) | isMeta (210), no origin (32), tool results (13) | Reminders for the model (e.g. `<tools-used-before-summary>` after a compaction) | `context` "System reminder"; removed from tool output |
| `<user-prompt-submit-hook>`, `<post-tool-use-hook>` | binary | Hook output added to the prompt | `context` "Hook output" |
| `<ide_selection>`, `<ide_opened_file>`, `<terminal name="…">` | binary (Claude Code tests each as the whole text) | IDE selection, open file, terminal selection | `context` (`terminal` only as the whole message) |
| `<mcp-resource server="…" uri="…">` | binary (written `isMeta`) | An @-mentioned MCP resource | `context` "MCP resource" |
| `<command-contents>`, `<skill-format>`, `<fork-boilerplate>`, `<user-memory-input>` | binary | Skill and fork text, memory input | `context` |
| `<task-notification>` with `<task-id>` (0…n; `__orphan_summary__:shell` is an internal scan marker), `<tool-use-id>`, `<output-file>`, `<status>`, `<summary>`, `<note>`, `<result>`, `<usage>` (`<subagent_tokens>`, `<tool_uses>`, `<duration_ms>`), `<worktree>` (`<worktreePath>`, `<worktreeBranch>`), `<event>` | origin `task-notification` (2955; 523 also isMeta); tool results (14) | A background command, agent, monitor or workflow finished, failed, was stopped or posted an event. Statuses seen: `completed` 1792, `failed` 97, `stopped` 70, `killed` 2; the binary's own description adds `blocked`. Text inside is XML-escaped (`&lt;`). | Notice: the `<summary>` (unescaped, one line); body: summary, event, result, note, "Output: …", "Worktree: …"; no tags |
| `Another Claude session sent a message:` + `<agent-message from="…">` … + a trailer for the model | origin `peer`, isMeta (557) | Another session's message; `origin.handback: true` with `handbackReportText` for a **subagent hand-back**; `origin.name`; `origin.body` (envelope stripped) | Notice "Subagent ‹name› reported back" / "Message from ‹name›"; body: the report or body |
| `<cross-session-message from="…" from-name="…" from-mode="…">`, `<teammate-message>` | origin `peer` (8), binary | Peer and teammate messages | Same, via `origin`; inside other text: `context` |
| (no tag) `[Request interrupted by user]`, `[Request interrupted by user for tool use]` | no origin, always one text block, the whole message (147 and 37; verified 2026-11-22 against Claude Code 2.1.330, where they are the constants `FA` and `ou`) | Claude Code's record that the turn was stopped — by Esc, or by the SDK's `interrupt()`, whoever asked for it. The binary's other `[Request interrupted: …]`, `[Tool call …]` and `[Agent call interrupted …]` texts are tool or sub-agent results, never a prompt (none seen as one). | Notice `interrupted` (D167): "Interrupted" / "Interrupted during a tool call", a quiet line with a stop mark. Only the exact whole text: a message that quotes it or adds to it stays the user's. In history it stays in the turn it stopped. |
| (no tag) `isCompactSummary` | 1 per compaction | "This session is being continued from a previous conversation…" | Compaction notice, the summary as body |
| `<tool_use_error>` | tool results (56) | A tool's error message | Its inside |
| `<persisted-output>` | tool results (35) | "Output too large, saved to …" | Left as is: it is the tool's own output |
| `[SYSTEM NOTIFICATION - NOT USER INPUT]` | for the model only; not in stored text | A frame around injected events | — |

Live, the same messages arrive as SDK `user` messages with `origin` (and `isSynthetic`); the
live translator uses the same functions as history. A message the harness wrote (`isMeta`) is
context only and does not begin a turn. A session's `firstPrompt` (the preview) loses its
markup the same way.

### Codex markup

The app-server's `userMessage` items are the user's own input: in 555 stored `UserMessage`
items only one wrapper appeared (`<send_user_message_question_reply>`, the user's answer to a
question the agent asked). Codex's contextual fragments live in `response_item` records the
app-server does not turn into `userMessage` items. The table knows them anyway, so a message
that carries one (an older Codex, another path) never shows its tags.

| Tag | Where (seen) | What it carries | How YAAC shows it |
| --- | --- | --- | --- |
| `<environment_context>` (`<cwd>`, `<shell>`, `<current_date>`, `<timezone>`, `<filesystem>`) | response_item user (227) | The environment | `context` "Environment" |
| `# AGENTS.md instructions for <dir>` + `<INSTRUCTIONS>` | response_item user (202) | AGENTS.md | `context` "Instructions (AGENTS.md)", heading included |
| `<user_instructions>` | binary (older form) | User instructions | `context` |
| `<user_shell_command>` (`<command>`, `<result>`) | response_item user (2) | A `!` command the user ran | `command` "!cmd" + `context` "Shell output" |
| `<turn_aborted>` | response_item developer (20) | The user interrupted the turn | `context` "Turn interrupted"; a message that is only this element: notice `interrupted` "Interrupted", as Claude Code's marker (D167). The app-server reports a stopped turn by its status (`interrupted`), which the fold shows as "· interrupted". |
| `<subagent_notification>`, `<agent_message_board_notification>` | binary | Sub-agent notices | `context` |
| `<codex_internal_context>`, `<objective>`, `<untrusted_objective>`, `<skill>`, `<image_resize_notice>` | response_item user (121, 84, 3), binary | Internal context, a goal, a skill, an image note | `context` (`objective`, `skill` only as the whole message) |
| `<send_user_message_question_reply>` | userMessage (1) | The user's answer to the agent's question | Its inside, as the user's text |
| `text_elements` with placeholder `[Pasted Content N chars]` | schema (`TextElement`); none in the owner's sessions | Where the composer put a long paste | **Verify:** the range's text, when it is not the placeholder itself, becomes a `pasted` part |
| `hookPrompt` item (`fragments[].text`) | schema | A hook's added prompt | Notice "A hook added a prompt.", the fragments as body |

Developer-role fragments (`<permissions>`, `<collaboration_mode>`, `<skills_instructions>`, …)
never reach a `userMessage` and are not listed.

### YAAC's own wrappers

| Text | Written by | How YAAC shows it read back from a harness |
| --- | --- | --- |
| `[Task from … — via YAAC …]`, `[Message from …]`, `[Result from …]`, `[Notice from YAAC about …]` … `[End of …]`; `[YAAC introduction]` | `mesh/format.ts` (`frameMessages`): the input of a turn the mesh starts | A notice from YAAC, "Task from ‹label› · via YAAC", the bodies (frames off) as its body. YAAC's journal already shows such a turn as `meshReceived`. |
| `[YAAC handoff] …` up to `---`, before the user's first message | `session/handoff.ts` (`preamble`) | `context` "YAAC handoff note"; the user's text after it is theirs |

The fork's handoff transcript (`renderTranscript`) carries typed text, pastes and commands, never
`context` parts.

## Agent calls (verified 2026-11-22)

What the harnesses' own agent tools look like, for D150. Shapes only — counted over the stored
transcripts on the development machine (`~/.claude/projects/**/*.jsonl`, 1,985 files;
`~/.codex/sessions/**`, 155 files) and read from the installed types (Claude Code 2.1.330,
Agent SDK 0.3.329 `sdk-tools.d.ts`; Codex 0.170.0 `app-server generate-ts`). No content was
copied.

**Claude Code** (built-in tool names in the SDK's `BUILTIN_TOOL_NAMES` include `Agent`,
`SendMessage`, `SubagentHandback`, `TaskStop`, `ListAgents`):

- `Agent` input: `description`, `prompt`, `subagent_type?`, `model?` (`sonnet`, `opus`,
  `haiku`, `fable`), `effort?`, `run_in_background?` (stored as a boolean or the string
  `"true"`), `name?`, `isolation?`. Result text: a background launch starts "Async agent
  launched successfully." and contains `agentId: <id>`; a foreground one is the answer.
  `tool_use_result`: `{status: "async_launched", agentId, description, prompt, outputFile,
  resolvedModel, …}` or `{status: "completed", agentId, content: [{type: "text", text}],
  totalToolUseCount, usage, …}` (`AgentOutput`).
- `SendMessage` input: `{to, message, summary?}`; older transcripts also carry
  `{type: "message", recipient, recipient_kind: "agent" | "main" | "name" | "session",
  content}`. `to: "main"` is a subagent writing to the conversation that started it. Result
  text is JSON: `{success, message, pin?: {id, name, ref}, resumedAgentId?, msg_id?}` with
  `message` one of "Message queued for delivery to <id> at its next tool round. …",
  "Resuming agent <short id>", "Message queued for the main conversation's next turn.".
- `TaskStop` input `{task_id}`; result JSON `{message, task_id, task_type, command}` with
  `task_type` `local_agent` (an agent), `local_bash` or `local_workflow`. Only `local_agent`
  is communication.
- `SubagentHandback` input `{message}`; result `{success, message: "Report delivered to your
  caller."}`. It runs inside the subagent's own transcript.
- YAAC's tools arrive as `mcp__yaac__<tool>`; their results are text YAAC wrote.

**Codex**: the model calls `collaboration/spawn_agent` (`{message, task_name, model?,
reasoning_effort?, fork_turns?}`), `send_message` and `followup_task` (`{target, message}`),
`wait_agent`, `interrupt_agent`, `list_agents`. The app-server presents each as a
`collabAgentToolCall` thread item: `{tool: "spawnAgent" | "sendInput" | "resumeAgent" |
"wait" | "closeAgent" | "sendMessage" | "followupTask" | "interruptAgent" | "listAgents",
status: "inProgress" | "completed" | "failed" | "interrupted", senderThreadId,
receiverThreadIds, prompt, model, reasoningEffort, agentsStates: {<thread id>: {status:
"pendingInit" | "running" | "interrupted" | "completed" | "errored" | "shutdown" |
"notFound", message}}}`. The receiver ids are Codex thread ids. (`wait` with `cell_id` in
rollouts is the code-mode exec wait, not an agent wait.) YAAC's tools arrive as
`mcpToolCall` items with `server: "yaac"`.

## YAAC peer delivery (verified 2026-11-23)

Scratch homes and local synthetic model endpoints exercised Codex CLI 0.170.0 and
Claude Code 2.1.330 with Agent SDK 0.3.329.

- **Codex.** An experimental app-server `turn/start` with `input: []` and
  `toolOutput: { name: "send_message_to_thread", namespace: "codex_app", output, retain: true }`
  returns the active turn id while busy and a new turn id while idle. The model saw a
  `function_call_output` at the subsequent boundary. A YAAC address in the
  `codex_delegation` envelope's `source_thread_id` worked without a local sender thread.
- **Claude Code.** SDK input with `priority: "next"`, peer origin, and a
  `cross-session-message` envelope appeared in the next model request, following the
  original tool round or final reply. The SDK emitted `command_lifecycle` frames carrying the
  input's UUID as `command_uuid`, with `queued`, `started`, and `completed` states. The
  bundled SDK implements `interrupt({ cancelQueued: true })` even though the public declaration
  omits the option. The provider checks for the CLI's `interrupt_cancel_queued_v1` capability
  before requesting cancellation when the user stops a turn with pending peer input. Sending
  a prompting-class peer to a bypass session produced `peer_message_hold`, and the model
  did not see the message. Explicit inbound refusal produced lifecycle state `refused`.
  YAAC leaves the sender's permission class unset and keeps Claude Code's inbound policy
  in effect. A held or refused command never falls back to a framed prompt.

## Subagents (verified 2026-11-22)

What each harness exposes of the sub-agents it runs itself (D151). Checked against Claude Code
2.1.330 / Agent SDK 0.3.329 (`sdk.d.ts`, `sdk.mjs`, and read-only SDK calls on the owner's
stored sessions — shapes and counts only) and codex-cli 0.170.0 (`app-server generate-ts`,
`~/.codex/state_5.sqlite` opened read-only). No live session was touched.

**Claude Code**

- **Storage.** `~/.claude/projects/<dir>/<sessionId>/subagents/agent-<agentId>.jsonl`, one
  per subagent, beside `agent-<agentId>.meta.json`. Workflow agents sit one folder down
  (`subagents/workflows/<run>/`) with the run's `journal.jsonl` (`started`, `result`,
  `failed` lines by `agentId`). The owner's store: ~1800 transcripts in 13 project folders,
  ~270 direct (Agent tool) and the rest workflow agents. Agent ids are 17 hex digits after `a`.
- **Lines** carry `isSidechain: true`, `agentId`, and `sessionId` = the parent session.
- **Sidecar** keys: `agentType`, `description`, `model`, `effort`, `finished`, `stoppedByUser`,
  `toolUseId` (the parent's Agent tool call), `requestShape` (`background`/`foreground`),
  `spawnDepth`, `parentAgentId` (nested), `worktreePath`/`worktreeBranch`; workflow agents
  have `workflowPhase` and no `toolUseId`. 27 of the direct ones have neither `finished` nor
  `stoppedByUser` (their process went away). Not exposed by the SDK; its own
  `importSessionToStore` carries it along.
- **SDK.** `listSubagents(sessionId)` returns the agent ids (it walks `subagents/`
  recursively, workflow agents included); `getSubagentMessages(sessionId, agentId)` returns
  `SessionMessage`s whose `parent_tool_use_id` is the Agent tool call (a string) and
  `parent_agent_id` null for a top-level subagent; with a wrong session it returns `[]`. The
  transcript is appended while the subagent runs, so it is readable then.
- **Parent transcript.** The Agent tool's `toolUseResult` has `agentId`, `status`
  (`async_launched`, …), `description`, `prompt`, `resolvedModel`, `outputFile`,
  `canContinueAgent`.
- **Live stream.** `system/task_started` (`task_id`, `tool_use_id`, `description`,
  `subagent_type`, `task_type: "local_agent"`, `prompt`, `parent_task_id`, `run_id`),
  `task_progress`, `task_updated` (`patch.status`: `completed`, `failed`, `killed`, …),
  `task_notification` (`status`: `completed`/`failed`/`stopped`, `summary`). Messages from
  inside carry `parent_tool_use_id`, and (newer) `agent_id`, documented as "the `task_id` of
  its task events": the task id is the agent id. By default only tool use/result blocks are
  forwarded; `forwardSubagentText: true` forwards its text and thinking too.
- **Continuing one.** Only from inside the parent: its `SendMessage` tool (`canContinueAgent`).
  `SDKUserMessage` has no field that addresses a subagent ("subagent-addressed messages are out
  of scope" in the interrupt receipt). So YAAC shows them read-only.
- `claude agents --json` lists *sessions* (interactive and background), not subagents; those
  are ordinary threads already.

**Codex**

- A sub-agent is a thread. In the installed app-server 0.171.0 schema, `Thread.source` is
  `{ subagent: { thread_spawn: { parent_thread_id,
  depth, agent_path, agent_nickname, agent_role } } }` (other kinds: `review`, `compact`,
  `memory_consolidation`, `other` — no parent). `Thread.agentNickname`, `agentRole` too.
- `thread/list` returns them when `sourceKinds` includes `subAgentThreadSpawn` (yaacd always
  sent every kind; until D151 they were listed as top-level programmatic threads). They read
  with `thread/items/list` like any thread.
- The owner's store: 125 sub-agent threads (`source` `{"subagent":{"thread_spawn":…}}`), 100
  archived; `thread_spawn_edges(parent_thread_id, child_thread_id, status)` has 125 rows (not
  exposed by the app-server).
- Live: `thread/started` with the sub-agent's thread; the parent's
  `collabAgentToolCall` items (`spawnAgent`, `sendInput`, `wait`, `closeAgent`, …) name it in
  `receiverThreadIds` with `agentsStates`; `subAgentActivity` items (`started`, `interacted`,
  `interrupted`, `completed`) also exist.
- Continuing one from outside: `turn/start` on its thread id is not refused by the schema, but
  the spawning agent drives it; YAAC does not send to one (a second writer). Not tried live.


## Background work and re-opens (verified 2026-11-22)

Observed with throwaway sessions (temporary project folders, `persistSession: false` or
deleted afterwards) against Claude Code 2.1.330 through the Agent SDK 0.3.329, for D160:

- **`background_tasks_changed` is the signal for "background work is running".** A `system`
  message carrying the full set of live background tasks (`task_id`, `run_id`, `task_type`
  such as `local_agent` or `local_bash`, `subagent_type`, `description`, `parent_task_id`,
  `ambient`), sent whenever the set changes. Seen for an `Agent` call with
  `run_in_background: true` (`local_agent`), a background `Bash` (`local_bash`), and a
  subagent's own background shell (with `parent_task_id`); the set was `[]` once all had
  ended. The types say to replace the set with each payload and to start from empty for each
  process (nothing is sent at start-up). The SDK has no call that returns the set; the
  `background_tasks` lists exist only on the `Stop`/`SubagentStop` hook inputs.
- The edges come too: `task_started` (`is_backgrounded: true`, `tool_use_id`), `task_progress`,
  `task_updated` (`patch.status`), `task_notification` (`status`, `summary`, `output_file`).
  The `Agent` tool result says "Async agent launched successfully … agentId: …" at once; the
  parent's turn then ends (`result`) while the subagent runs.
- **When a background task ends, Claude Code starts a turn by itself** (a fresh `system/init`,
  assistant messages, a `result`) with no user message — the provider's `external` turn. A
  subagent whose own background shell is still running can be "completed" and later resumed
  as a new run of the same `task_id` (the set briefly goes `[]` and back).
- **A background subagent's permission request reaches `canUseTool`** (with `agentID`) while
  the parent has no turn running.
- **Bypass on a live query.** With `allowDangerouslySkipPermissions: true` and
  `permissionMode: "default"`, a `Bash` call still reached `canUseTool`;
  `setPermissionMode("bypassPermissions")` succeeded and the next `Bash` call did not ask;
  `setPermissionMode("default")` made it ask again. Without the option,
  `setPermissionMode("bypassPermissions")` is refused: "Cannot set permission mode to
  bypassPermissions because the session was not launched with --dangerously-skip-permissions".
- **As root, the option stops Claude Code from starting.** The CLI checks
  `permissionMode === "bypassPermissions" || allowDangerouslySkipPermissions` at start-up and
  exits with "--dangerously-skip-permissions cannot be used with root/sudo privileges" when
  the user id is 0, unless `IS_SANDBOX=1` (read from the installed binary, not run as root).
- `additionalDirectories` is fixed at open. The SDK's other ways to add a folder do not fit:
  `register_repo_root` takes only a subfolder of the cwd or of a folder given at launch, and
  `addDirectories` is a permission update returned from `canUseTool`.
- End to end through the provider (idle timeout 8 s, Haiku): a background subagent running
  ~50 s kept its process through 12 s of parent idleness, a pasted image, a switch to
  `default` and back to bypass (no new process, no approval in the bypass turn); the process
  was closed for idleness 8 s after the subagent's own closing turn. Killing the process while
  the subagent ran gave the notice "Claude Code's process ended while background work was
  running in it; this stopped: subagent "sleeper"."


## Rate limits and accounts (verified 2026-11-22)

What each harness says about the account it runs as and its plan limits (D161). Read from
`codex app-server generate-ts` (codex-cli 0.170.0) and the Agent SDK's `sdk.d.ts` (0.3.329),
then observed with read-only calls against the owner's sign-ins in a throwaway folder: a private
`codex app-server --listen stdio://` (`account/read`, `account/rateLimits/read`), and a Claude
Code 2.1.330 query with no prompt (`accountInfo()`, the usage report) plus one one-word turn with
no tools and `persistSession: false` (for `rate_limit_event`). No live session was touched.
Values below are as shaped, not as observed.

**Codex**

- `account/read { refreshToken? }` → `{ account, userId, requiresOpenaiAuth }`. `account` is
  `{ type: "chatgpt", email, planType, workspaceName }` | `{ type: "apiKey" }` |
  `{ type: "amazonBedrock", … }` | `{ type: "gcp" }`. `PlanType` is an open list (`free`, `plus`,
  `pro`, `team`, `business`, `enterprise`, … `unknown`). The live answer also carried a
  `workspaceRouting` object that the generated bindings do not declare; YAAC does not read it.
- `account/rateLimits/read { supportsLunaReserve?, excludeResetCreditDetails? }` →
  `{ ordinaryUsageAllowed, rateLimits: RateLimitSnapshot, rateLimitsByLimitId: { [limitId]:
  RateLimitSnapshot } | null, rateLimitResetCredits, accountId, rateLimitUpsell }`.
  `excludeResetCreditDetails: true` is documented for background polls.
- `RateLimitSnapshot` = `{ limitId, limitName, normalModelSlug, primary: RateLimitWindow | null,
  secondary: RateLimitWindow | null, credits, individualLimit, spendControlReached, planType,
  rateLimitReachedType }`; `RateLimitWindow` = `{ usedPercent, windowDurationMins, resetsAt }`.
  Observed: `usedPercent` an integer 0–100, `windowDurationMins` 10080 (a week) for a Pro
  account whose snapshot had only `primary`, `resetsAt` in **Unix seconds**, `limitId` `"codex"`,
  the same snapshot repeated under `rateLimitsByLimitId.codex`, `accountId` a UUID.
- `account/rateLimits/updated { rateLimits: RateLimitSnapshot }` — documented as a **sparse
  rolling update**: merge into the last read; nullable values do not clear what was seen.
- `account/updated { authMode, planType }` arrives right after `initialize` and on sign-in
  changes.

**Claude Code / Agent SDK**

- `Query.accountInfo()` → `AccountInfo { email?, organization?, subscriptionType?, tokenSource?,
  apiKeySource?, apiProvider? }` (`apiProvider`: `firstParty`, `bedrock`, `vertex`, `foundry`,
  …, `gateway`). Observed `subscriptionType: "Claude Ultra"`, `apiProvider: "firstParty"`.
- `Query.usage_EXPERIMENTAL_MAY_CHANGE_DO_NOT_RELY_ON_THIS_API_YET({ skipBehaviors? })` →
  `SDKControlGetUsageResponse { session, subscription_type, rate_limits_available, rate_limits:
  { five_hour?, seven_day?, seven_day_opus?, seven_day_sonnet?, …: { utilization (0–100),
  resets_at (ISO 8601) } | null, model_scoped?, extra_usage?, limits?: [{ kind, group, percent
  (0–100), resets_at, scope?: { model?: { display_name }, surface?: { display_name } },
  severity, is_active }] } | null, behaviors }`. Declared experimental and unstable. Answers on
  a query **without a turn**. Observed: `subscription_type: "max"` (differs from
  `accountInfo().subscriptionType`), `five_hour: null` while no five-hour window was running,
  `limits` rows `weekly_all` and `weekly_scoped` (scope `model.display_name`), and many
  undeclared keys alongside, all null.
- The stream's `SDKRateLimitEvent { type: "rate_limit_event", rate_limit_info: SDKRateLimitInfo,
  uuid, session_id }`, with `SDKRateLimitInfo { status: "allowed" | "allowed_warning" |
  "rejected", resetsAt?, rateLimitType?: "five_hour" | "seven_day" | "seven_day_opus" |
  "seven_day_sonnet" | "seven_day_overage_included" | "overage", utilization?, overage…,
  surpassedThreshold?, … }`. Observed during a one-word turn, after the assistant message:
  `rateLimitType: "seven_day"`, `utilization` a **fraction** (0.87 where the usage report said
  88), `resetsAt` in **Unix seconds**; also an undeclared `unifiedWindows` map, not used.
- Where they come from: plan limits only apply to a first-party claude.ai sign-in; the usage
  report says `rate_limits_available: false` for API keys and cloud providers.

## A subagent's requests (verified 2026-11-22)

Observed for D162 with one throwaway session (a temporary folder, Haiku) against Claude Code
2.1.330 through the Agent SDK 0.3.329, the parent launching one background `general-purpose`
subagent that runs a `Bash` command, `canUseTool` holding the prompt open:

- The parent's turn ends (`result`) right after the `Agent` call; the subagent's `Bash`
  prompt reaches `canUseTool` about half a second later, while **no parent turn is running**.
- **`canUseTool`'s `agentID` is the subagent's `task_id`** — the same id as its
  `task_started`, `background_tasks_changed` entry and its messages' `agent_id`. `toolUseID`
  is the subagent's own `tool_use` id (not the parent's `Agent` call). The types say the same
  (`agentID`: "If running within the context of a sub-agent, the sub-agent's ID"); the hook
  input's `agent_id` is documented as present only inside a subagent (read from sdk.d.ts, not
  run).
- **A user message sent while that prompt is pending is answered at once**: a new
  `system/init`, the reply, a `result`. The prompt stays open (its signal is not aborted) and
  answering it afterwards lets the subagent go on (`tool_result`, then `task_updated`
  `completed`, `task_notification`).
- After the subagent's `task_notification`, Claude Code starts a parent turn by itself to
  report it (the `external` turn of D160) — that one is real work and is shown as a turn.

### Manual context compaction (2026-11-23)

Codex CLI 0.170.0's generated app-server bindings define
`thread/compact/start` with `{ threadId }` and an empty result. The current notification is
a `contextCompaction` item. An isolated app-server emitted the empty reply, then
`turn/started`, then `item/started` for the compaction. The bindings still include
`thread/compacted`, now deprecated
in favor of the item.

The installed Claude Agent SDK 0.3.329 defines `SDKCompactBoundaryMessage` with
`compact_metadata.trigger` of `manual` or `auto`. Its `SDKLocalCommandOutputMessage`
carries local slash command feedback. An isolated Claude Code 2.1.330 stream with no prior
messages emitted a synthetic assistant message, `Error: No messages to compact`, followed by
a result with subtype `success`. YAAC checks for the manual boundary before accepting
a successful SDK result as confirmation of `/compact`.

## Live shell commands (verified 2026-11-23)

- Claude Code 2.1.330 via Agent SDK 0.3.329, in an isolated query against a
  synthetic API, reported a `local_bash` task, paired its `task_started.tool_use_id`
  with the Bash call, and sent a structured `tool_use_result.backgroundTaskId`.
  The corresponding tool result named a regular output file under that session's
  `tasks` directory. The task remained listed after `result`. `stopTask(taskId)`
  was acknowledged. Claude reported the task stopped and sent an empty live set.
- Codex app-server 0.171.0, launched privately with a temporary home and a
  synthetic Responses API, reported the process through
  `thread/backgroundTerminals/list` after the launch turn completed. It emitted a
  later `item/commandExecution/outputDelta` with the original command's item
  and turn ids. `thread/backgroundTerminals/terminate` returned `terminated: true`
  for that thread and process id. The next inventory was empty. The 0.170.0
  source also shows the inventory includes live processes during initial output
  collection, before they have yielded.
