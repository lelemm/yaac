# 03 — The YAAC protocol (host ⇄ `yaacd`)

One protocol, one stream per target. On the host its peer is the [hub](02-architecture.md#processes);
the window reaches targets through the hub, which carries this same protocol to a named target.
Where this doc says "the UI", read "the host side". This doc defines the shape; exact field lists live in the
`yaac-protocol` crate once it exists, and that crate is the source of truth.

**Decided:** this is YAAC's own protocol. It borrows vocabulary from the Agent Client Protocol
(ACP) and from the Codex app-server, but it is not wire-compatible with either, because it must
do three things neither defines: multiplex many provider instances over one stream, replay
losslessly after a disconnect, and carry binary uploads. ACP enters YAAC later as a *provider*
([04 Providers](04-providers.md#adding-a-provider)).

## Design rules

1. **Multiplex everything.** Many provider instances, many threads, and file uploads share the
   stream. No message may block another for long.
2. **Resumable and lossless.** Any connection can drop at any byte. Work continues on the
   target, and after reconnecting the UI receives everything it missed.
3. **Provider-neutral.** The UI never sees a Codex or Claude type. `yaacd` normalises both into
   one item model.
4. **Forward-compatible.** Unknown fields are ignored. Unknown enum values (capabilities, item
   kinds) are preserved and rendered generically, never treated as errors.
5. **Typed once.** Rust types are the source of truth; TypeScript bindings for `yaacd` are
   generated from them and checked in CI. Older protocol versions are **not** kept as copies of
   those types ([09 Compatibility](09-compatibility.md)).
6. **No blind retries.** Anything that could start paid work or change a thread carries an
   operation id, and its outcome is recorded on the target ([idempotency](#idempotency)).

## Framing

After the [bootstrap handshake](02-architecture.md#bootstrap), the stream is a sequence of
frames:

```
┌────────────────┬──────────┬───────────────────────────┐
│ length: u32 LE │ kind: u8 │ payload (length − 1 bytes) │
└────────────────┴──────────┴───────────────────────────┘
```

| Kind | Payload |
| --- | --- |
| `0x01` message | One UTF-8 JSON object. |
| `0x02` data | `stream: u32 LE` followed by raw bytes. Used for uploads and other bulk transfers. |

JSON is the MVP encoding because it is debuggable and fast enough. The kind byte leaves room to
add a binary message encoding later without a breaking change.

### Scheduling and flow control

One stream is one queue. At 1 Mbit/s a 256 KiB frame occupies the wire for about two seconds,
so an approval queued behind a few of them would wait far too long. Three rules prevent that,
and they apply in **both** directions:

- **Three classes.** Control (requests, responses, events) and interactive data (terminal I/O)
  alternate whenever both are ready. Both precede bulk (uploads and history pages). The
  writer asks for one frame at a time. A keystroke waits behind at most one in-flight bulk
  chunk. Pending interactive bytes cap bulk data chunks at 16 KiB even while awaiting credit.
- **Small, adaptive chunks.** Data chunks start at 16 KiB and grow to at most 64 KiB while the
  link keeps up. The hard frame limit is 1 MiB.
- **Credit.** The receiver of a data stream grants credit in bytes; the sender stops when it
  runs out. A slow disk on the target cannot fill memory, and a slow link cannot be flooded.

Known bulk content is paged **by bytes**. History pages target 64 KiB, allowing a single larger
item through so the cursor can advance. A large tool output carries a bounded head and its
true size, with the rest fetched on demand with `item/read`. Image bytes are fetched over
`upload/read`.

### Oversized JSON messages

`initialize` can include `messageChunks: true`. The daemon confirms it in the initialize
result. Once both peers have agreed, a JSON message above the single-frame limit travels in
16 KiB pieces over reserved data stream `0`. The same-build window and hub support this on
their attach link. The framing header and the 1 MiB per-frame limit are unchanged.

After the data frame's stream number, each fragment has a one-byte lane (`0` for control,
`1` for bulk), a four-byte total message size and a four-byte offset, both little-endian,
followed by the UTF-8 message bytes. Fragments in each lane are consecutive and in order.
Each lane can reassemble one message at a time, up to 32 MiB. The receiver parses JSON only
after the last fragment. All fragments of one journal event retain its original position.
Regular data streams and the other lane can interleave with fragments. A reconnect resumes
from the last complete event applied by the receiver.

A peer without the extension still uses one-frame JSON. Oversize RPC results return a size
error. If a journal event exceeds the negotiated size limit, its subscription pauses at
that sequence and records a diagnostic. The original entry remains in the journal, and
other subscriptions stay connected.

## Messages

JSON-RPC 2.0 shapes:

```jsonc
{ "id": 7, "method": "turn/start", "params": { … } }      // request
{ "id": 7, "result": { … } }                              // response
{ "id": 7, "error": { "code": "…", "message": "…", "data": { … } } }
{ "method": "thread/event", "params": { … } }             // notification (no id)
```

These are JSON-RPC 2.0 *shapes*; the `"jsonrpc": "2.0"` member is omitted, and an error's
`code` is a string, not a number. Both are deliberate and part of the frozen core.

The host sends requests; `yaacd` sends responses and notifications. The one exception is the
agent mesh: a target must be able to hand the host a message for another target without being
asked, so `mesh/relay`, `mesh/deliveryReceipt`, and `mesh/query` are requests that `yaacd` originates and the host
answers, with their own id space. Things a harness asks the
user — approve this command, answer this question — are **not** modelled as daemon-to-UI
requests. They are items in the thread, answered with `request/respond`
([requests](#requests-approvals-and-questions)).

## Method catalogue

### Session

| Method | Purpose |
| --- | --- |
| `initialize` | Must be the first message. Each side states the range of protocol versions it speaks; they agree on one ([versioning](#versioning-and-compatibility)). Returns daemon build, state format, and target facts: OS, architecture, hostname, user, home directory. |
| `daemon/status` | Build, protocol range, state format, and what the daemon is busy with: running turns, pending requests, attached clients. Part of the frozen core. |
| `daemon/handover` | Ask the daemon to step aside for a newer build — `when: "idle"` or `when: "now"`. Part of the frozen core. |
| `ping` | Liveness and latency. |
| `diagnostics/get` | Redacted daemon and provider diagnostics, for the "what is wrong with this target" view. Never contains message content. |

### Terminal

A target advertising `terminal` accepts `terminal/shells`, `open`, `attach`, `resize`,
`close` and `list` under the same prefix. Terminal IDs survive connection loss. Each open or
attach returns a fresh connection-local duplex stream and its initial input credit. Host
output credit is granted with `stream/credit` after opening the stream. Output and keystrokes
use raw data frames on the interactive lane. The notifications are `terminal/title` and
`terminal/exited` from the daemon and `terminal/detach` from the host. [Terminal protocol](13-terminal.md#methods-yaac-protocol-additive-feature-terminal)

### Providers

| Method / notification | Purpose |
| --- | --- |
| `provider/list` | All provider instances on this target, each with a full [descriptor](04-providers.md#the-contract): status, backend in use, capabilities, models. |
| `provider/refresh` | Force a re-probe of one instance or all (re-detect install, sign-in, models). |
| `providerInstance/add` · `update` · `remove` | Manage instances: kind, optional name, endpoint settings such as a socket path. |
| → `provider/updated` | An instance's descriptor changed. Carries a monotonically increasing `revision`. |

### Threads

| Method / notification | Purpose |
| --- | --- |
| `thread/sync` | Incremental thread index ([below](#the-thread-index)). |
| `discovery/caps` | Read the target's discovery configuration. An update with `opId` can set `autoDiscover` independently of cap enforcement. A daemon that supports the switch includes `autoDiscover` in its result and in `discovery/updated`. |
| `thread/start` | Create a thread **and send its first message**, as one operation: instance, working directory, model, effort, mode, `fast` (D125), input. Returns the thread id and the first turn id. There is no separate "create empty thread" call: an abandoned draft should leave nothing behind, and one operation is one thing to make idempotent. |
| `thread/read` | A page of items, newest first, with its journal position and attachment presence. Interactive reads set `maxItems`. The first page includes the compact, ordered `prompts` index and outstanding requests outside the page. `around` opens a window at a particular item. `cursor` and `newerCursor` identify the adjacent pages independently of live appends. Interactive reads can set `wholeTurns` to extend the item limit through the boundary turn. If the byte limit cuts it, `continuesTurn` signals that the window should buffer the adjacent chunk before displaying the turn. |
| `item/read` | A further byte range of one large item's output. |
| `thread/forkPreview` | What a fork to a given destination would carry: mechanism, turn it cuts at, sizes, what is left out, and the destination account. For a handoff, the transcript's size and its first `maxPreviewBytes`, rendered from the harness's store exactly as the fork would; `unavailableReason` when it cannot fork (no completed turn yet, no history, destination not usable). Changes nothing. |
| `thread/subscribe` · `unsubscribe` · `ack` | Start or stop live events for a thread, and report how far the client has durably got ([journal and replay](#journal-and-replay)). |
| `thread/fork` | Create a new thread from an existing one, on any instance and model, with its first message ([05 Threads](05-threads.md#fork)). The result's `forkKind` says which mechanism was used. A handoff can only be made this way; `thread/forkOnly` (no first message) is native only. |
| `thread/rename` · `archive` | Gated by capability. |
| `thread/compact` | `{ opId, threadId }` → `{}` once the harness confirms context compaction. Requires `compact`, an idle thread without a pending request, and writer ownership. Queued messages wait. The journal retains prior messages and records the native compaction notice. |
| `thread/delete` | `{ opId, threadId, alsoInHarness }` (D110). Always removes YAAC's record and journal; `alsoInHarness: true` (only under `threadDelete`) first deletes the harness's session too, and nothing is removed if that fails. Absent means `false`. Without it, discovery leaves the session out until it is looked up by id. Followed by `thread/removed`. |
| `thread/move` | `{ opId, threadId, project?, worktree? }` → `{ thread, workingDirectory? }` (D144). Lists the thread under another project of the **same target** (and optionally a checkout of its repository, by root); without `project` the placement is forgotten. `ThreadSummary.placement` carries it. Under the provider's `threadMoveWorkingDirectory` the agent's folder follows from its next turn, and `workingDirectory` names it; otherwise only the placement changes. Refused for an archived thread and one in use elsewhere (`thread_in_use`). Offered under `threadMove`. Followed by `thread/updated`. |
| → `thread/updated` · `thread/removed` | Index changes pushed while connected, including for threads created outside YAAC. |

### Turns

| Method | Purpose |
| --- | --- |
| `command/logs` | `{ threadId, commandId }` → `{ output?, truncated }`. Reads a bounded output tail for a live command on its owner. Absent output means the harness has provided none to YAAC. |
| `command/stop` | `{ opId, threadId, commandId }` → `{}`. Stops one command on its owner when supported and allowed by writer control. Liveness changes arrive through the index. |
| `turn/start` | Send a user message to an existing thread with optional `model`, `effort`, `mode`, `fast`, and `ultrafast` overrides. Speed is validated against the model. It persists for later turns and is reported by `ThreadSummary.fast` and `ThreadSummary.ultrafast`. An explicit Fast or Standard selection clears Ultrafast. Returns the turn id. Rejected with `thread_in_use` when the thread is being driven elsewhere. |
| `turn/shellCommand` | `{ opId, threadId, command, delivery? }` → like `turn/start` (`outcome` `started` or `queued`, `queueId`). Under `userShellCommand` (P344): runs a `!command` the user typed in the thread's composer on the target, in the thread's folder, records it as the harness's own `!` does, and has the agent answer it, as one turn — journaled as a `userMessage` with the one part `!command` (role `command`, `opId`), a `toolCall` with `source: userShell` (streamed output, `exitCode`, `durationMs`), then the answer. `delivery: queue` queues it while a turn runs (the `QueuedMessage` has `shellCommand`); absent, a running turn refuses it (`turn_in_progress`); `now` is `invalid_params`. Without the capability: `capability_missing`. Never reachable from an agent. [13 Terminal](13-terminal.md#command-in-the-composer--the-agent-answers) |
| `turn/interrupt` | Stop a turn. Takes the **turn id**, so a late Stop can never hit the next turn. |
| `turn/steer` | Add input to the running turn. Takes the turn id it expects to be running. Gated by capability. |
| `request/respond` | Answer a pending approval or question. |

### Uploads

| Method | Purpose |
| --- | --- |
| `upload/begin` | Declare name, size, media type, and SHA-256. Returns an `uploadId` and a data `stream` number — or says the content already exists, and no bytes need to be sent. |
| *(data frames)* | The bytes, in order, on that stream, under credit. |
| `upload/end` · `upload/cancel` | Finish (the daemon verifies the hash) or abort (the daemon removes the partial file). |
| `upload/read` | A bounded read of a stored attachment — for showing a thumbnail on a host that did not upload it. |

### Agent mesh

Agents reach each other through tools `yaacd` serves to the harness, not through this
protocol. What crosses this protocol is the part that needs the host: delivery between targets,
and keeping the UI informed. The catalogue (`mesh/relay`, `mesh/deliver`, `mesh/deliveryReceipt`, `mesh/query`,
`mesh/roster`, `mesh/policy`) and its guarantees are in
[10 Agent mesh](10-agent-mesh.md#in-the-protocol). All of it is additive.

### Tunnels and ports

`tunnel/open`, `tunnel/close`, and `preview/ports` forward a TCP port on the target over this
connection for the web preview. Shapes and rules are in [11 Web preview](11-preview.md#tunnel-yaac-protocol).
All of it is additive.

### Later additions

Methods and fields added after this document was drafted — projects, `thread/update`,
`thread/forkOnly`, the permission request, git status and flags on summaries, and the hub's
thread search — are listed with their exact names in
[plan/wire-additions.md](plan/wire-additions.md). All are additive.

### Pull requests

D119–D121: the pull requests of a thread's branch. **All forge work runs on the target**, in
`yaacd`, with the target's own `gh` (GitHub) or `az` (Azure DevOps) and their logins; the host
only opens the URLs it is given. Everything is keyed by thread: `yaacd` knows its checkout and
branch. All additive; a `yaacd` without them answers `method_not_found` and the window shows no
badge.

| Method / notification | Purpose |
| --- | --- |
| `forge/prs` | `{ threadId, refresh? }` → `{ root, branch?, access, defaultBranch?, prs, at?, error? }`. `access` is `{ repo?, capabilities, reason? }`: `repo` (kind, host, slug, web URL, `fromFork`) is absent when the remote is not on a supported forge; `capabilities` (`prList`, `prGet`, `prCreate`, `prChecks`, `prRerun`, `prComments`, `prReply`) are empty with a `reason` when the CLI is missing or signed out. `prs`: number, title, state (`open` · `merged` · `closed`), draft, URL, head, base, combined checks (`none` · `pending` · `passing` · `failing`). Answered from `yaacd`'s cache while fresh; `refresh` asks the forge (rate-limited). |
| `forge/prepare` | `{ threadId, base? }` → what the Create PR modal needs: `access`, `branch`, `defaultBranch`, the remote's `branches`, `base`, a `title` and `body` prefilled from the branch's commits, and `push` (`upstream`, `ahead`, `uncommitted`). |
| `forge/pr/create` | `{ opId, threadId, base, title, body, draft }` → `{ pr, pushed }`. Pushes first (`git push -u`) when the branch has no upstream or unpushed commits; uncommitted changes are never pushed. A create whose outcome cannot be told is `outcome_unknown`, never re-sent. |
| `forge/pr/get` | `{ threadId, number }` → `{ pr, body, reviewers, checks }`. |
| → `forge/prs/changed` | A branch's list changed (after a commit or push in its checkout, or a create); same shape as the `forge/prs` result. |

The forge kind is data for display; the window branches only on the capabilities.

### Rate limits

D161: the plan rate limits of the account each provider instance runs as, for the panel at the
bottom of the sidebar. Reported by instances with the `rateLimits` capability, from the
harness's own account surface ([04](04-providers.md#rate-limits)). Additive: the hub asks only
targets whose provider list has the capability.

| Method / notification | Purpose |
| --- | --- |
| `provider/rateLimits` | `{ refresh? }` → `{ instances: [{ instance, kind, account?, windows, at }] }`. `account` is `{ key, label?, email? }`: `key` is opaque and the same for the same account on every target (a digest, never the harness's identifiers); `label` is the plan as the harness names it; `email` is for the tooltip only. A window is `{ id, label, scope?, usedPercent, resetsAt?, windowMinutes? }` — `usedPercent` in whole percent (0–100), `scope` the model or surface a scoped window counts. Answered from each instance's last report; `refresh` reads the harnesses again, at most every five minutes per instance. |
| → `provider/rateLimits/updated` | One instance's report changed (a harness notification or stream event, or a refresh); one entry of the result above. |

The window gets them from the hub, grouped by account (invariant 14): `hub/rateLimits/get
{ refresh? }` → `{ accounts: [{ key, kind, label?, email?, windows, at, targets, stale }] }`,
and `hub/rateLimits/changed` with the same shape. The same account on two targets is one entry
with the newest report's windows; `targets` lists every target that reported it; `stale` means
no connected target reports it now (the values are the last known, kept across hub restarts).
`refresh` asks each connected target again, at most every five minutes per target.

### Filesystem (read-only)

| Method | Purpose |
| --- | --- |
| `fs/list` | List a directory on the target, for the working-directory picker. |
| `fs/search` | `{ cwd, query }` → `{ entries, truncated }`. Case-insensitive search for files below an absolute target directory. Results carry relative display names and absolute paths. Includes tracked and unignored untracked git files, or searches a bounded directory tree outside a repository. |
| `project/resolve` | For a directory: git root, common directory, worktree, branch, and origin URL. Used for sidebar grouping. |
| `fs/tree` | List a worktree directory with git marks and ignored flags. Requires the target's `fileView` feature. |
| `fs/read` | Read a page of a worktree file. Requires `fileView`. Bulk response. |
| `git/fileDiff` | Read a page of a worktree file's `HEAD` and working-copy versions. Requires `fileView`. Bulk response. |

See [12 Files and diffs](12-files.md#target-methods) for containment, limits, and byte cursors.

### Skills

| Method | Purpose |
| --- | --- |
| `skills/list` | The skills one provider instance offers for a working directory (`{ instance, cwd, forceReload }` → `{ skills: [{ name, description, path, scope?, argumentHint? }], errors? }`). Needs the instance's `skills` capability (`capability_missing` otherwise). Scoped by the target answering, the instance (its harness home) and the folder; disabled skills are left out. Read-only. See [04](04-providers.md#skills-are-a-capability). |

## Input parts

```jsonc
{ "type": "text",  "text": "Why does this test fail?" }
{ "type": "image", "uploadId": "u_01J…" }   // send as real image input
{ "type": "file",  "uploadId": "u_01J…" }   // reference by path on the target
{ "type": "skill", "name": "plan", "path": "/home/me/.codex/skills/plan/SKILL.md" } // a skill named in the text
```

A `skill` part accompanies a text part that names the skill (`$plan`); only the `inlineSkills`
capability sends one. A provider without it ignores the part — the text says it all.

The UI chooses between `image` and `file` from the thread's effective capabilities; `yaacd`
re-checks and rejects an `image` part the model cannot read. The reasoning is in
[06 Attachments](06-attachments.md).

A text part `yaacd` read from a harness (history or the live stream) may carry a **role** — what
the text is when it is not what the user typed — and a **label** (D141, P120). The provider has
already taken the harness's markup off ([harness surfaces](research/harness-surfaces.md#markup-inside-messages)):

```jsonc
{ "type": "text", "text": "INFO batch 1/4 …", "role": "pasted" }                 // the user pasted it
{ "type": "text", "text": "/model opus", "role": "command" }                     // a command they ran
{ "type": "text", "text": "Keep answers short.", "role": "context", "label": "System reminder" }
```

`context` is what the harness added for the model; a window shows it only on request and never
copies it as the user's text. A role a window does not know is shown as ordinary text. A client
never sends a role. A `notice` item may carry a `body`: the plain text behind its one line,
shown when it is expanded.

## The item model

A thread is a list of turns; a turn is a list of items. Every provider's output is normalised
into these item kinds:

| Kind | Content |
| --- | --- |
| `userMessage` | The input parts as sent; `opId`, the operation of the `thread/start` or `turn/start` that sent it (a queued message keeps the one that queued it), so the window can replace its echo of the message in place (D165). |
| `agentMessage` | Markdown text. |
| `reasoning` | Reasoning text or summary, when the provider exposes it. |
| `toolCall` | A tool invocation: `command`, `fileChange`, `mcp`, `webSearch`, `subagent`, or `other`; with title, input, output, and exit status — and, for communication with another agent, an `agentCall` ([10](10-agent-mesh.md#agent-calls-in-the-transcript)), and, when the harness confirmed it loaded a skill's instructions, a `skill` (`{ name }`; the title is then "Skill loaded" or "Read skill instructions" — [04](04-providers.md#skills-are-a-capability)). |
| `plan` | The agent's task list and each step's state. |
| `request` | Something awaiting the user: an approval or a question. |
| `notice` | Out-of-band information: errors, context compaction, model rerouting, rate-limit warnings, a turn the harness recorded as stopped (`interrupted`, D167). |
| `handoff` | Marks history a forked thread inherited from its parent: the source, the mechanism, the cut-off turn and — for a handoff — the frozen transcript (bounded; its end) and the file's path. The first item of a forked thread, before its first turn. |
| `meshSent` · `meshReceived` | A message this agent sent to, or received from, another agent — with the other side's address and a delivery receipt. |
| `meshSpawn` · `meshWait` | Threads this agent started, and a wait on other agents, with their live states. |

Every item has an `id`, a `turnId`, and a `status` (`inProgress`, `completed`, `failed`,
`declined`). Unknown kinds must round-trip and render as a collapsed generic block.

## Requests: approvals and questions

Harnesses stop and ask the user for two kinds of thing, and each harness does it its own way.
YAAC normalises both into `request` items.

**The guarantee (decided): a question or approval is never lost.** The moment a harness asks,
`yaacd` writes the request into the thread's journal — before any UI has seen it. It then
survives the UI being closed, the connection dropping, and `yaacd` restarting. It is delivered
on reconnect, raises the thread's *needs attention* state in the sidebar, and stays visible in
the transcript forever, with its answer.

### Approvals

"May I run this command?" "May I change these files?" "May I have this permission?"

An approval carries what is being asked for — the command and directory, the file changes, the
scope of the grant — and the **decisions the harness actually offers**. These differ: allow
once, allow for the session, allow and remember, deny, deny with a reason. The UI renders the
offered decisions; it does not assume a fixed Allow/Deny pair.

For a command approval (P357), the initial request is journaled and delivered immediately.
The window sends `approval/explain { tool, input, models: { main, fallback } }` through the
request target's existing connection. Each selection identifies a provider instance and model.
The result is `{ text }`, with null on unavailability or timeout. The daemon bounds the input
to 128 KiB and caches successful results in memory by the hash of the tool and input. The
window displays the result only while the matching request is pending. This RPC has no
approval response or journal side effects.

### Questions

"Which of these approaches do you want?" Both harnesses support structured questions, with
different shapes:

| | Codex | Claude Code |
| --- | --- | --- |
| How it arrives | `item/tool/requestUserInput` | the `AskUserQuestion` tool, through the permission callback |
| Per question | `id`, `header`, `question`, optional `options` (label, description), `isOther`, `isSecret` | `question`, `header`, 2–4 `options` (label, description, optional preview), `multiSelect` |
| Answer | question id → list of strings | selected labels per question, or free text |

Normalised:

```jsonc
{ "kind": "request", "requestKind": "question", "blocking": true,
  "questions": [
    { "id": "q1", "header": "Approach", "text": "Which fix do you want?",
      "options": [ { "label": "Patch decoder", "description": "…", "preview": null } ],
      "multiSelect": false,   // may choose several options
      "allowOther": true,     // may type a free-form answer instead
      "secret": false } ] }   // answer must be masked and never cached on the host
```

The answer maps each question id to the chosen labels and/or free text. `yaacd` converts it to
the form the harness wants. A provider that can ask questions declares the `UserQuestions`
capability; one that can ask for approval declares `Approvals`
([04 Providers](04-providers.md#capabilities)).

Schema-driven forms — a harness or an MCP server asking for typed fields (text, number, yes/no,
pick from a list) — use the same request item with `requestKind: "form"`. Until the UI
implements forms, such a request is shown with its text and a note that it must be answered in
the harness itself, rather than being dropped.

### Request states

A request item is always in exactly one of these states:

| State | Meaning |
| --- | --- |
| `pending` | Waiting for an answer. |
| `submitted` | This UI answered; the harness has not confirmed yet. |
| `resolved` | Answered. Records the answer and who gave it: this client, another YAAC client, **elsewhere** (for example in a terminal), or **`yaacd` automatically** — only a tool approval in a thread whose mode never asks (P342), with the mode and the harness's reason recorded. |
| `expired` | No longer answerable: the turn ended, was interrupted, or the agent's process stopped. The question text remains in the transcript. |
| `unknown` | `yaacd` cannot tell whether the answer reached the harness (a crash between sending and confirmation). Shown as such; never re-sent automatically. |

### Tool approvals in a never-ask mode (P342)

A harness can still ask for a tool approval while the thread is in its never-ask mode — Claude
Code keeps some checks in `bypassPermissions`. `yaacd` answers those itself; nothing else.

- **Which requests:** only `requestKind: "approval"` raised while the thread's mode — as the
  provider says the harness was in when it asked — is one the provider marks `skipsApprovals`
  (Claude Code `bypassPermissions`, Codex *Full access*). Never by provider kind. Questions
  (`AskUserQuestion`, `request_user_input`), forms (elicitations), logins and mesh permission
  requests always reach the user. A mode change back to an asking mode restores prompts from the
  next request on; workers keep the mode they were given (D146).
- **Not automatic even then:** an approval the user's own configuration forced (a Claude Code
  `permissions.ask` rule: `matchedAskRule` in `sdk.d.ts` — "the user's stated intent is a human
  prompt" — or, as 2.1.330 actually sends it, `decisionReasonType: "rule"`), one in a thread
  another program holds, or one that offers no allow decision.
- **The answer:** the narrowest grant offered — the first `allow` decision, else
  `allowSession`; never `allowAlways`.
- **Journaled first:** the request record is written to disk pending, then as `submitted` by
  `yaacd`, before the answer is handed over; the item enters the transcript already in that
  state (no window shows it waiting) and becomes `resolved` on the harness's confirmation. If the
  harness refuses the answer, the request goes back to `pending` for the user; a crash between
  hand-over and confirmation leaves it `unknown`, never re-sent.
- **On the wire:** `resolution.by` is `"yaacd"` and `resolution.automatic` names the mode:

```jsonc
"resolution": { "by": "yaacd", "answer": { "requestKind": "approval", "decision": "allow" },
                "automatic": { "mode": "bypassPermissions", "modeName": "Bypass permissions" } }
```

  The same resolution is already on the `submitted` item. Both fields are additive: an older
  window decodes `yaacd` as an unknown answerer and shows the request as resolved elsewhere.

**The harness's reason.** Every approval carries `reason` when the harness gives one, shown on
the card and on the automatic line:

| | Field | Also kept in `details` |
| --- | --- | --- |
| Claude Code | `canUseTool`'s `decisionReason` (else its `description`); `blockedPath` becomes `scope` | `decisionReason`, `decisionReasonType` (sent by 2.1.330, not in `sdk.d.ts`), `description`, `matchedAskRule`, `defaultToNo`, `tool`, `input` |
| Codex | `reason` of `item/commandExecution/requestApproval`, `item/fileChange/requestApproval`, `item/permissions/requestApproval` (and the legacy `execCommandApproval` / `applyPatchApproval`); `grantRoot` becomes `scope` | `commandActions`, `networkApprovalContext` |

Verified 2026-11-25 (Claude Code 2.1.330, SDK 0.3.329, isolated config and a fake API): a query
*opened* in `bypassPermissions` never consults `canUseTool` for `nohup … &`, `setsid`, `sed -i`
or `rm` (the SDK warns `CLAUDE_SDK_CAN_USE_TOOL_SHADOWED`); in `default` each asks — `setsid` with
`decisionReason` "What 'setsid' starts can't be checked before it runs", the others with only
`blockedPath` and `suggestions` (a path outside the working folders). `description` is the
model's own Bash `description` input. The same holds for a query opened in `default` and moved
into bypass with `setPermissionMode` (how YAAC switches) and for a subagent's Bash under bypass.
Under bypass only a `permissions.ask` rule reached `canUseTool`, with `decisionReasonType:
"rule"`, no `matchedAskRule` and no reason text. Codex fields: `codex app-server generate-ts`,
CLI 0.170.0.

Two facts about harness behaviour shape this:

- A Codex daemon delivers the same request to every client attached to that thread, and the
  first answer wins. A card can therefore be resolved by someone else at any moment; the UI
  must show *resolved elsewhere* rather than fail.
- A request can stop being answerable without anyone answering it. `expired` is a normal
  outcome, not an error.

## Thread control: who may act

**Decided:** if a thread is being used somewhere else, YAAC shows it live but does not let the
user interact with it.

Each thread summary and each `thread/subscribe` result carries a control state:

| Control | Meaning | UI |
| --- | --- | --- |
| `free` | Nobody is driving it. | Composer enabled. |
| `mine` | This client started the running turn. | Composer enabled; Stop, steer, and requests are available. |
| `otherClient` | Another YAAC client attached to the same `yaacd` is driving it. | Read-only, live. "In use on *<host>*". |
| `external` | Another program on the target has it — a terminal session, another app. | Read-only. "In use in another program". |

In the read-only states the composer is disabled, and requests are displayed but cannot be
answered from this UI. `turn/start`, `turn/steer`, and `request/respond` are rejected with
`thread_in_use`, so the rule holds even if a UI is out of date. (One exception: a read-only
harness sub-agent's requests are the user's, so `request/respond` on it checks its parent's
control instead of its own, D162/P242.) Control returns to `free` when
the other party finishes or lets go — or, for `external`, when the user **takes it over**
(D140): `thread/takeOver { opId, threadId }`, under the `threadTakeOver` capability, answered
with `{}` (also when nobody else holds it) or refused with `thread_in_use` and the harness's
reason. Where the holder will not let go now, `controlChanged` and the `thread/subscribe`
result carry that reason as `takeOverBlocked`, so the UI can disable **Take over** with it
([05 Threads](05-threads.md#taking-it-over)). How "in use elsewhere" is detected is per provider
([05 Threads](05-threads.md#in-use-elsewhere)).

## Events

Live updates arrive as `thread/event` notifications:

```jsonc
{ "method": "thread/event",
  "params": { "threadId": "…", "epoch": 3, "seq": 412, "event": { "type": "itemDelta", … } } }
```

| Event | Meaning |
| --- | --- |
| `turnStarted` · `turnCompleted` | Turn boundaries. Completion carries status and token usage. |
| `itemStarted` · `itemCompleted` | An item appeared, or reached a final state. |
| `itemDelta` | Text to append to an item's field (`text`, `reasoning`, `output`). |
| `itemUpdated` | A full replacement snapshot of an item. May carry `throughSeq` ([backpressure](#backpressure)). |
| `requestChanged` | A request moved between [states](#request-states). |
| `controlChanged` | The thread's [control](#thread-control-who-may-act) state changed. |
| `settingsChanged` | The model, effort, or mode **the harness actually applied** changed. |
| `usageUpdated` | Token and context usage. |

`yaacd` coalesces deltas *before* assigning sequence numbers — flushing on a short timer or a
size threshold — so a fast-talking model does not become thousands of tiny frames, and every
client sees the same numbering.

## Journal and replay

**Decided:** nothing a thread produces is lost because the host was away.

`yaacd` appends every event of every thread to a per-thread **journal** on disk, whether or not
a client is attached. A journal position is a pair:

- **`epoch`** identifies one continuous journal. It changes only if the journal has to be
  started afresh (corruption, or a state-format change that cannot be migrated).
- **`seq`** increases by one per event within an epoch and is never reused.

```
UI:     thread/subscribe { threadId, epoch: 3, afterSeq: 398 }
yaacd:  { mode: "replay", epoch: 3, fromSeq: 399, headSeq: 431, control: "free" }
            → events 399 … 431, then live events
   or   { mode: "reset",  epoch: 4, headSeq: 12,  control: "free" }
            → rebuild from thread/read, then live events
```

- **Replay is the normal path** after any disconnect, however long.
- **`reset` is the fallback**: the client has no position (first open, or its cache was
  cleared), its epoch is not the current one, or its position predates the journal's start.
- **A reset has no gap.** `thread/read` returns the snapshot together with the exact position
  it is valid at, and its page cursor stays bound to that snapshot. `yaacd` then delivers events
  strictly after that position. Events that arrive while the UI is still paging are neither
  lost nor applied twice.
- **The client commits atomically.** The host cache stores a thread's items and its position
  in one transaction, so a crash cannot leave "data at 410, position at 398" or the reverse.
- `thread/ack { threadId, epoch, seq }`, sent periodically while attached, tells `yaacd` how
  far this client has durably got.

### Keeping the journal bounded

- **Compaction.** Once an item completes, its run of `itemDelta` events is replaced by one
  `itemUpdated` snapshot marked as covering that range of sequence numbers. A client replaying
  across it sees the finished item instead of the deltas — same result, fewer bytes.
- **Trimming.** Events at or below the lowest position acknowledged by known clients are
  dropped. A client not seen for a retention period no longer holds the journal back; if it
  returns, it gets a `reset`.
- **Ceiling.** A per-thread size cap trims the oldest events as a last resort. That can turn a
  replay into a `reset`; it cannot lose conversation content where the harness keeps the
  transcript.

## The thread index

The optional `ThreadSummary.runningCommands` field carries the live process ids, command
text, observation time and stop capability for the owning thread. An optional `controller`
names the same-target thread whose writer permission governs a harness subagent's command.
The list is refreshed from the harness after a daemon restart. The window derives a parent's
cross-target list from the worker links in its indexes.

`yaacd` keeps its own index of every thread on the target and a change counter. The sync token
is a position in that counter — it is YAAC's, not any harness's.

```
UI:     thread/sync { token: "idx3:8841" }
yaacd:  { changed: [ …summaries… ], removed: [ …ids… ], token: "idx3:8912", more: false }
```

- An empty token means a full listing, paged.
- A token from an old index generation gets a full listing; the UI replaces its cache for that
  target.
- `yaacd` builds the index by scanning each instance's listing and listening for its change
  notices. A scan that fails or is incomplete **never removes threads**: the affected instance
  is marked stale, and its threads stay. A thread is removed only when the harness positively
  reports it gone.

## Idempotency

A dropped connection after `thread/start` must not create two threads or pay for two turns.

- Every state-changing request — `thread/start`, `turn/start`, `turn/steer`, `thread/fork`,
  `request/respond` — carries a client-generated `opId`.
- `yaacd` records each `opId` and its outcome on disk before acting and after. Re-sending an
  `opId` returns the recorded outcome instead of acting again.
- If `yaacd` crashed between handing the operation to the harness and recording the result, it
  first tries to reconcile from harness evidence (did a thread or turn appear?). If it cannot
  tell, the outcome is **`outcome_unknown`**: shown to the user, never retried automatically.
  One ambiguous send the user must look at is better than a duplicated paid turn.

## Backpressure

`yaacd` holds a bounded outbound queue per client. If a client falls behind, pending deltas for
an item collapse into one `itemUpdated` snapshot whose `throughSeq` names the last sequence
number it replaces. The client advances its position to `throughSeq`. A slow link costs
smoothness, never correctness, and never unbounded memory on the target.

## Versioning and compatibility

**Decided.** The protocol version is negotiated **once per connection, in the handshake**.
Frames and messages carry no version field.

The two sides do not have to be the same version. Each states the range it can speak, they
agree on the highest version both know, and **the newer side adapts** to it:

```jsonc
// UI → yaacd
{ "id": 1, "method": "initialize",
  "params": { "protocol": { "min": 5, "max": 8 }, "client": { "name": "yaac", "version": "0.9.0", "host": "laptop" } } }

// yaacd → UI
{ "id": 1,
  "result": { "protocol": { "min": 3, "max": 6, "agreed": 6 }, "schemaHash": "9f2c41d0",
              "daemon": { "build": "0.7.2+a1b2c3", "stateFormat": 4 }, "target": { … } } }
```

- `initialize` must be the first message; no other method is valid before it succeeds, except
  the two frozen daemon-management calls.
- A newer UI keeps working against an older daemon, warns that the target will be updated when
  idle, and offers to update it now. It never forces a busy daemon out.
- Each side supports the **last 3 protocol versions** besides its own. Outside that window the
  answer is `protocol_unsupported`, with a specific message about which side to update.
- Most changes never bump the version at all: fields, methods, events, and capabilities are
  added compatibly (design rule 4). The version changes only when an older peer would misread
  something.
- Feature availability is a separate question. Whether a target can do something is discovered
  through capabilities, never by comparing versions.

How the adapting is built — one dialect object per version, the newest as the base, methods
split into small overridable steps — and the exact procedure for making an incompatible change
are in [09 Compatibility](09-compatibility.md). That doc is binding: it also lists what
compatibility code is **not** allowed to look like.

## Errors

Errors carry a stable string `code` for the UI to branch on and a human-readable `message` to
show as-is.

| Code | Meaning |
| --- | --- |
| `protocol_unsupported` | The two sides have no protocol version in common. Says which side is too old. Part of the frozen core. |
| `provider_unavailable` · `needs_auth` | The instance cannot be reached, or the harness is not signed in. |
| `capability_missing` | The instance or model cannot do what was asked. |
| `store_change` | A `providerInstance/update` would lead the instance to another store and lacks `confirmStoreChange`; nothing changed. `data` is `{ current?, next? }`, the two homes (P300, [04](04-providers.md#an-instance-has-a-store-identity)). |
| `thread_not_found` | The thread does not exist on this target. |
| `thread_in_use` | The thread is being driven elsewhere; this client may only watch. |
| `turn_in_progress` · `turn_mismatch` | A turn is already running, or the turn id given is not the running one. |
| `outcome_unknown` | The operation may or may not have taken effect; see [idempotency](#idempotency). |
| `upload_mismatch` | The uploaded bytes do not match the declared hash or size. |
