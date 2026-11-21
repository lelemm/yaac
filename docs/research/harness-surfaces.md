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
