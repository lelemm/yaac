# 04 — Providers

A **provider kind** is a harness YAAC can drive (`codex`, `claude`). A **provider instance** is
one configured endpoint of that kind on a target. Providers run inside `yaacd`; the Rust side
sees them only through the contract below.

## Kinds, instances, backends

```
provider kind      codex ─────────────────────────────  claude
                     │                                     │
provider instance  "default"        "work"              "default"
(per target)       default socket   /run/…/work.sock
                     │                 │                   │
backend            daemon           daemon              sdk
(chosen at probe)  └ else start it  └ explicit socket
```

- **Several instances of one kind may exist on a target** (decided). The motivating case: more
  than one Codex daemon on the same machine, each on its own socket. Each instance has an
  optional user-given **name** so they can be told apart in the model picker and the sidebar.
- **Every target has implicit default instances.** With no configuration, `yaacd` probes for
  each kind at its standard location. Users add instances only for the non-default cases.
- **Still one connection.** Instances are a target-side concept. `yaacd` holds the N local
  sockets; the host holds one stream.

### Instance configuration

```jsonc
{
  "id": "pi_01J…",              // stable, generated
  "kind": "codex",
  "name": "work",               // optional; shown as "Codex · work"
  "enabled": true,
  "backend": "auto",            // auto | daemon | sdk
  "endpoint": { "socket": "/run/user/1000/codex-work/control.sock" },  // optional
  "env": { "CODEX_HOME": "/home/me/.codex-work" }                      // optional
}
```

Instance configuration is stored **on the target** by `yaacd` and edited from the UI through
`providerInstance/*`. It describes things that exist on that machine, and any host connecting to
the target should see the same instances.

### An instance has a store identity

A socket selects *which process to talk to*. It does not say *which conversations that process
owns*. For Codex those are separate: the session store, configuration, and sign-in come from
the daemon's home directory, and two sockets may share one home or use two different ones.

So when `yaacd` probes an instance it records the instance's **store identity** — for Codex,
the home directory and account the server reports in its `initialize` reply. That identity is
what tells YAAC:

- whether two instances are looking at the same conversations (same store) or different ones;
- whether a thread can be forked natively to another instance (only within one store);
- whether an edited socket path still points at the same store. If it does not, the instance's
  existing threads are **not** silently re-pointed: they are shown as unavailable until the
  user confirms the change.

**Decided:** Claude Code has a single instance per target in the MVP. The contract allows
several — for example separate config directories for two accounts — and that can be added
later without redesign, once it is verified that sessions, sign-in, and the SDK all honour a
separate directory.

### Backend selection

| Kind | Backend |
| --- | --- |
| `codex` | The Codex daemon. **1.** use the one that is running → **2.** otherwise start it, then use it. |
| `claude` | The Claude Agent SDK. (A daemon backend is a reserved slot; see [below](#daemon-backend-reserved).) |

Rules:

- **Decided — Codex runs on its daemon, and only on its daemon.** There is no SDK fallback.
  YAAC requires a Codex version that has the app-server daemon. An older Codex shows as *Codex
  is too old for YAAC — update Codex on this target*, with the version found.
- **Decided — if no Codex daemon is running, `yaacd` starts it** with
  `codex app-server daemon start` (under the instance's `CODEX_HOME` when it has one), so other
  Codex clients on that machine share it too. The `codex` it runs is `YAAC_CODEX_EXECUTABLE`
  when the instance or process sets it, else the first found on `PATH`, in `~/.local/bin`, or
  in `$CODEX_HOME/packages/standalone/current/bin` — where Codex's installer puts it, and which
  a non-login `PATH` leaves out; a folder found off `PATH` is added to the end of the `PATH` it
  runs with (P130). `yaacd` **starts** the daemon; it never stops,
  restarts, or updates it — those can interrupt work that is not YAAC's.
- An instance defined by an **explicit socket** is connect-only. If nothing is listening there
  and the instance has no home to start a daemon from, it is unavailable, and says so. It does
  not fall back to something else that might be a different store.
- The backend in use is reported in the instance's descriptor and shown in the UI.
- Failure reasons are kept distinct, because they need different fixes: not installed, too old,
  not signed in, daemon could not be started, access denied, another writer holds the thread.

The contract still describes backends generally (`Daemon`, `Sdk`), because other providers will
differ and Claude gains a daemon backend when one is supported.

## The contract

What the UI knows about an instance. These are wire types, so they live in `yaac-protocol`:

```rust
pub struct ProviderDescriptor {
    pub instance: InstanceId,
    pub kind: ProviderKind,                 // open string newtype: "codex", "claude", …
    pub name: Option<String>,               // user-given instance name
    pub display_name: String,               // "Codex", or "Codex · work"
    pub backend: BackendKind,               // Daemon | Sdk
    pub harness_version: Option<String>,    // of the process actually serving, not just the CLI
    pub store: StoreIdentity,               // which conversations this instance owns
    pub status: ProviderStatus,             // Ready | NotInstalled | TooOld | NeedsAuth | Unreachable | …
    pub capabilities: Vec<Capability>,      // what this instance can do, on this backend
    pub models: Vec<ModelInfo>,             // fetched from the harness — never hardcoded
    pub modes: Vec<ModeInfo>,               // permission / approval / sandbox modes on offer,
                                            // each with skips_approvals and autonomy
    pub revision: u64,                      // bumps whenever anything above changes
}

pub struct ModelInfo {
    pub id: String,                         // what the harness expects back
    pub display_name: String,
    pub description: Option<String>,
    pub is_default: bool,
    pub hidden: bool,
    pub capabilities: Vec<ModelCapability>, // each Supported, Unsupported, or Unknown
    pub efforts: Vec<EffortInfo>,           // ordered, as reported by the harness
    pub default_effort: Option<EffortId>,
    pub context_window: Option<u32>,
}

pub struct EffortId(pub String);            // open: "low", "high", "xhigh", "ultra", …
```

What a provider implements inside `yaacd` — the TypeScript side:

```ts
interface HarnessProvider {
  probe(): Promise<ProviderDescriptor>;                    // install, sign-in, backend, models
  scanThreads(page: PageRequest): Promise<ThreadPage>;     // for the thread index
  readThread(thread: NativeId, page: PageRequest): Promise<ItemPage>;

  // Creating a thread and sending its first message are one operation, so a draft
  // leaves nothing behind and there is a single thing to make idempotent.
  startThread(params: StartThreadParams, input: InputPart[]): Promise<{ thread: NativeId; turn: TurnId }>;
  startTurn(thread: NativeId, input: InputPart[], overrides: TurnOverrides): Promise<TurnId>;
  interrupt(thread: NativeId, turn: TurnId): Promise<void>;
  respond(thread: NativeId, requestId: string, response: RequestResponse): Promise<void>;

  /** Capabilities and settings for one thread, given its model and current state. */
  effective(thread: NativeId): Promise<ThreadEffective>;
  /** Whether another client or program is currently driving this thread. */
  control(thread: NativeId): Promise<ExternalControl>;

  // Optional — presence must match the declared capabilities.
  forkThread?(thread: NativeId, opts: ForkOptions): Promise<NativeId>;   // native fork only
  steer?(thread: NativeId, turn: TurnId, input: InputPart[]): Promise<void>;
  rename?(thread: NativeId, title: string): Promise<void>;
  archive?(thread: NativeId): Promise<void>;
  remove?(thread: NativeId): Promise<void>;

  readonly events: EventSource<ProviderEvent>;             // normalised items and deltas
  dispose(): Promise<void>;
}
```

`NativeId` is whatever the harness uses to resume that exact conversation
([05 Threads](05-threads.md#identity)). A provider's whole job is translation: harness-native
calls and events on one side, the [YAAC item model](03-protocol.md#the-item-model) on the other.

A provider takes part in a **handoff fork** without any fork-specific code: `yaacd` reads the
source with `readThread`, renders the transcript itself, and creates the destination with an
ordinary `startThread` whose input carries the preamble in its `text` and the transcript's
folder in `readable` — folders the harness must be able to read for the conversation's whole
life. A provider that confines reads (Claude Code) keeps them; one that does not (Codex's
sandbox reads anywhere) may ignore them. `forkThread` is only the native fork
([05 Threads](05-threads.md#fork)).

## Capabilities

A capability is a flag the UI checks before offering a feature.

```rust
#[non_exhaustive]
pub enum Capability {
    // ── Thread lifecycle ────────────────────────────────────────────────────
    ThreadList,          // can enumerate existing conversations on the target
    ThreadRead,          // can read a conversation's history back
    ThreadResume,        // can continue a conversation from an earlier run
    ThreadFork,          // native fork within this provider
    ThreadForkAtTurn,    // native fork from a chosen point, not only the end
    ThreadRename,
    ThreadArchive,
    ThreadDelete,        // the harness can delete its own sessions (`thread/delete { alsoInHarness }`, D110)
    ThreadMove,          // yaacd can list the thread under another project (`thread/move`, D144); yaacd's own
    ThreadMoveWorkingDirectory, // the harness can change a conversation's folder from its next turn (D144)
    FolderTrust,         // the harness can be told a folder is trusted, through a supported interface (D147)
    FollowExternal,      // can show live updates of a thread someone else is driving

    // ── Turn control ────────────────────────────────────────────────────────
    StreamText,          // incremental agent text
    StreamReasoning,     // incremental reasoning text or summaries
    Interrupt,           // stop a running turn
    Steer,               // add input to a running turn
    Compact,             // manual context compaction
    RewindConversation,  // drop later turns from the conversation
    RewindFiles,         // restore files to an earlier point — a different act entirely

    // ── Asking the user ─────────────────────────────────────────────────────
    Approvals,           // asks before running commands or changing files
    UserQuestions,       // can ask the user structured questions
    UserForms,           // can ask for typed fields (schema-driven forms)
    Modes,               // offers selectable permission / approval / sandbox modes

    // ── Mid-thread switching ────────────────────────────────────────────────
    ModelSwitch,         // change model within a thread
    EffortSwitch,        // change effort within a thread

    // ── Input (model-level) ─────────────────────────────────────────────────
    ImageInput,          // the model can see images
    DocumentInput,       // the model accepts documents (PDF) as native input
    AudioInput,

    // ── Reasoning (model-level) ─────────────────────────────────────────────
    Reasoning,           // the model has selectable effort levels
    ReasoningSummary,    // reasoning is visible to the user in some form

    // ── Speed (model-level) ─────────────────────────────────────────────────
    FastMode,            // the harness can run this model's turns in fast mode (D125)

    // ── Reporting ───────────────────────────────────────────────────────────
    TokenUsage,
    ContextUsage,        // how full the context window is
    RateLimits,          // reports its account and plan rate limits (D161)
    ModelList,           // can enumerate models

    // ── Working with other agents ───────────────────────────────────────────
    MeshTools,           // can be given the `yaac` tools: message, spawn, and wait for other agents

    // ── Agent features surfaced in the UI ───────────────────────────────────
    Plan,                // task list updates
    SlashCommands,
    Subagents,
    WebSearch,
    FileDiffs,           // per-turn diff of file changes

    Unknown(String),     // a capability this build does not know; kept, ignored
}
```

On the wire a capability is a string in an array, spelled in camelCase (`meshTools`,
`imageInput`); every other enumerated value is camelCase too, and error codes are snake_case.
An older UI that meets a newer capability keeps it as `Unknown` and moves on.

### Asking the user is a capability

**Decided.** Both harnesses can stop and ask the user for information, each in its own way.
YAAC supports both, and questions must never be lost.

- `UserQuestions` — the provider can ask structured questions. Codex does it with
  `item/tool/requestUserInput`; Claude Code with its `AskUserQuestion` tool. `yaacd` normalises
  both into one question shape ([protocol](03-protocol.md#questions)).
- `Approvals` — the provider asks before acting.
- `UserForms` — typed fields, as used by MCP elicitation. Recognised from day one, rendered
  later; until then such a request is shown, not dropped.

A provider that declares `UserQuestions` must route every question into the thread as a
`request` item, written to the journal before anything else happens to it. A future provider
that *cannot* ask simply does not declare the capability; the UI then says the agent cannot
ask questions in that thread.

### Talking to other agents is a capability

**Decided.** The main purpose of YAAC is that agents in different harnesses can reach each
other ([10 Agent mesh](10-agent-mesh.md)). A provider that declares `MeshTools` accepts, per
thread, an MCP server from `yaacd` plus a short identity block in its instructions. Both MVP
providers declare it. How each harness is given the server is the provider's business:

| Harness | How the `yaac` MCP server is attached | How the identity block is added |
| --- | --- | --- |
| Codex | An MCP server entry in that thread's configuration, launching the `yaacd mcp` bridge | `developerInstructions` on `thread/start` |
| Claude Code | An in-process MCP server (`type: "sdk"`) on that thread's `query()` | an appended system prompt |

A provider must also be able to **start a turn from a message that did not come from the
user** — that is how an incoming agent message wakes an idle thread. Both can: it is an
ordinary turn whose input is the framed message.

### Effective capabilities are computed on the target

What a thread can do depends on four things: the instance and its backend, the selected model,
the account and its policy, and the thread's current state. Three of those four are only known
on the target. So the UI does not combine flags itself — `yaacd` computes the **effective
capabilities of a thread** and sends them with the thread's settings, updating them when the
model changes.

```rust
pub struct ThreadEffective {
    pub capabilities: Vec<Capability>,   // what can be done in this thread, right now
    pub image_input: Support,            // Supported | Unsupported | Unknown
    pub image_formats: Vec<String>,      // media types accepted, when known
    pub efforts: Vec<EffortInfo>,        // selectable now; empty when effort cannot be changed
    pub modes: Vec<ModeInfo>,
    pub applied: AppliedSettings,        // the model / effort / mode the harness is really using
}
```

Things this gets right that a naive union of flags gets wrong:

- **Unknown is a real answer.** A model the harness gave no modality data for is `Unknown`, not
  "yes". The composer handles that case explicitly ([06 Attachments](06-attachments.md)).
- **Showing a control requires the ability to use it.** The effort picker appears on a started
  thread only when the model reports efforts *and* the instance can change effort mid-thread.
- **Applied, not requested.** After a model or effort change the UI shows what the harness
  reports it is using, which can differ from what was asked for (rerouting, policy limits).

**The rule for UI code:** never branch on provider kind (`if kind == "codex"`). Branch on a
capability. If a feature needs a distinction no capability expresses, add a capability.

### Modes and how permissive they are

**Proposed (P150, P151), for D146.** A mode is reported by its provider at runtime, never
listed in UI code. Besides its id, label and `isDefault`, a `ModeInfo` carries two flags about
what it lets the agent do without asking:

- `skipsApprovals` — it never asks. The UI confirms before switching a thread to it and keeps
  it visibly marked while it is active.
- `autonomy` — where it sits on one scale every provider shares, least first: `readOnly`
  (changes nothing), `ask` (anything that changes something asks, beyond the user's own
  rules), `edits` (file edits proceed), `workspace` (edits and commands confined to the
  working directory proceed), `judged` (the harness approves or denies each action itself and
  asks only when unsure), `full` (everything proceeds). Optional and open: absent, or a value
  this build does not know, is *unknown* and is never compared. A mode with `skipsApprovals`
  counts as `full`.

The agent mesh compares modes on these two flags to start a worker in the mode that matches
its parent's ([10](10-agent-mesh.md#the-workers-permission-mode)). Each provider fills them in
from what its harness documents for the mode:

| Harness | Mode → `autonomy` | Source |
| --- | --- | --- |
| Claude Code | `plan` → `readOnly`; `default`, `dontAsk` → `ask`; `acceptEdits` → `edits`; `auto` → `judged`; `bypassPermissions` → `full` (and `skipsApprovals`) | `PermissionMode` in the Agent SDK's `sdk.d.ts` (0.3.329). `dontAsk` acts on exactly what the user's rules allow, as `default` does unasked. |
| Codex | Read only → `ask`; Auto → `workspace`; Full access → `full` (and `skipsApprovals`) | The presets' approval policy and sandbox (`on-request` + `read-only` / `workspace-write`; `never` + `danger-full-access`). |
| Fake | `fake-ask` → `ask`; `fake-auto` → `workspace`; or whatever `FAKE_MODES` says | — |

**Codex reports the mode a thread is really in (P152).** `thread/start`, `thread/resume` and
`thread/fork` answer with the thread's `approvalPolicy`, `approvalsReviewer` and `sandbox`
(verified with `codex app-server generate-ts`, codex-cli 0.170.0). A thread YAAC started with
no mode follows the user's own Codex configuration; when those settings are exactly a preset's
(reviewer `user`, same policy, same sandbox type and network access), `applied.mode` is that
preset — so a thread on a full-access configuration is marked as one. Anything else (a
reviewer other than the user, `untrusted` or granular approval, network the preset lacks) is
no preset: `applied.mode` stays empty, and the mesh treats the thread's mode as unknown.

### Moving a thread is a capability

**Decided (D144); Proposed (P101).** Where YAAC lists a thread is YAAC's own placement
([05](05-threads.md#moving-a-thread-to-another-project)); `threadMove` says the target's
`yaacd` can keep one, and `yaacd` adds it to every instance itself — an older `yaacd` lacks
it, and Move is then offered disabled. Whether the *agent* moves too is the provider's
**`threadMoveWorkingDirectory`**, reported at runtime: with it, `yaacd` sets `cwd` in
`TurnOverrides` on every turn after a move, and the provider applies it, also when it resumes
the conversation.

| Harness | `threadMoveWorkingDirectory` | How |
| --- | --- | --- |
| Codex | yes | `turn/start.cwd` ("override the working directory for this turn and subsequent turns") and `thread/resume.cwd`; verified on codex-cli 0.170.0. Codex's own `threadSection/*` are labels, not folders, and are not used. |
| Claude Code | no | The Agent SDK (0.3.329) has no way to change a session's folder; sessions are stored per project folder and `forkSession` takes no target folder. A move is YAAC's placement only. |
| Fake | yes, unless `FAKE_MOVE_CWD=0` | Records the folder it was handed (`received[].cwd`). |

### Trusting a folder is a capability

**Decided (D147); Proposed (P160–P162).** A provider reports **`folderTrust`** at runtime when
its harness can be told, through an interface the harness supports, that a folder is trusted;
it then implements `trustFolder(folder)`: read the harness's configuration first, write only
when it has no word on the folder, never downgrade or remove, and report `marked`, `trusted`,
`declined` (the user set something else there) or `failed` (with `uncertain` when no answer
came). `yaacd` decides when to call it ([05](05-threads.md#folder-trust)); the instance's
`trustFolders` switch (absent = on) turns it off. A harness's files are never edited by hand.

| Harness | `folderTrust` | How |
| --- | --- | --- |
| Codex | yes | `config/read { includeLayers: true }` → `config.projects["<folder>"].trust_level`; if absent, `config/value/write { keyPath: "projects", value: { "<folder>": { trust_level: "trusted" } }, mergeStrategy: "upsert", expectedVersion: <user layer's version> }` over the connection `yaacd` already has. Verified on codex-cli 0.170.0. |
| Claude Code | no | Trust is `projects["<folder>"].hasTrustDialogAccepted` in `~/.claude.json`. The only programmatic way to set it is the Agent SDK's `workspaceTrust` option, which is not in the SDK's published types and is marked `@internal` in the CLI (2.1.330 / SDK 0.3.329) — not a supported interface, so it is not used. |
| Fake | no | |

### Rate limits

**Decided (D161); Proposed (P230–P233).** A provider reports **`rateLimits`** when its harness
says which account it runs as and how much of that account's plan limits is used. It then
implements `rateLimits({ refresh })`, returning its last report — `{ account, windows, at }` —
and emits `rateLimitsChanged` whenever the harness reports new values. `yaacd` serves
`provider/rateLimits` and `provider/rateLimits/updated` from these
([03](03-protocol.md#rate-limits)); the hub groups them by account across targets and the window
shows them at the bottom of the sidebar ([07](07-ui.md#the-rate-limit-panel)).

The **account key** is `<kind>:` and the first 16 hex digits of a SHA-256 over the kind and the
harness's own account identifiers, so the same account is one key on every target and the
identifiers never leave in clear. The email travels to the window for the tooltip only and is
never logged. A window is `{ id, label, scope?, usedPercent, resetsAt?, windowMinutes? }`, in
whole percent; nothing names a model except a scope the harness labels itself.

| Harness | Account | Windows | When read |
| --- | --- | --- | --- |
| Codex | `account/read` → `account.email`, `account.planType`, `userId`; the key is over the rate-limit read's `accountId` (the ChatGPT account) and `userId` (else the email). Label: the plan, capitalised ("Pro"). | `account/rateLimits/read { excludeResetCreditDetails: true }` → `rateLimits.primary` / `secondary` (`usedPercent` 0–100, `windowDurationMins`, `resetsAt` in Unix seconds), labelled from the length (`5h`, `weekly`); any other bucket in `rateLimitsByLimitId` is a scoped window named by its `limitName`. `account/rateLimits/updated` is a sparse update, merged into the last read (nulls clear nothing). | At each probe (connect, sign-in change), on every update notification, and on a refresh. |
| Claude Code | `accountInfo()` → `email`, `organization`, `subscriptionType` (the label, e.g. "Claude Ultra"); keyed over email and organization. A sign-in other than `apiProvider: "firstParty"` (Bedrock, Vertex, …) has no plan limits and no account. | The SDK's usage report, `usage_EXPERIMENTAL_MAY_CHANGE_DO_NOT_RELY_ON_THIS_API_YET({ skipBehaviors: true })`: the server's `rate_limits.limits[]` rows (`kind` `session` → `5h`, `weekly_all` → `weekly`, `weekly_scoped` with its `scope` display name; `percent` 0–100; `resets_at` ISO 8601), else the named buckets (`five_hour`, `seven_day`, …). During turns, the stream's `rate_limit_event` (`rateLimitType`, `utilization` as a 0–1 fraction, `resetsAt` in Unix seconds) updates the window it names. The usage call is experimental by the SDK's own word: it is optional, and without it the stream alone keeps the windows. | On the probe's query (no prompt is sent), from every turn's stream, and on a refresh — then with a short read-only query of its own, only after 15 minutes without news. |
| Fake | — | — | — |

No polling: `yaacd` reads a harness again only when asked to refresh, and at most every five
minutes per instance; the window asks only while it shows the panel
([07](07-ui.md#the-rate-limit-panel)).

### Fast mode is a model capability

**Decided (D125); the rules below are Proposed (P54–P56).** Fast mode runs a turn faster, at a
higher cost. Whether a model has it is a **model-level capability**, `fastMode`, on its
descriptor (`ModelInfo.capabilities`), as each provider reads it from its harness at runtime —
never from a list in source. Unknown is not yes: a model whose harness says nothing is not
offered fast mode. A thread's effective capabilities include `fastMode` while its model has it
(`yaacd` adds it from the descriptor; providers only report it per model).

Fast mode is a **setting of the thread**, kept by `yaacd` in the thread's record: `turn/start`
and `thread/start` carry `fast` (absent: unchanged); it sticks for later turns, like `model`,
and is reported as `ThreadSummary.fast` — which therefore also says the running turn was
started in fast mode. `yaacd` resolves it for every turn it starts (a client's `fast`, else the
thread's setting) and hands the provider `fast: true` only when the turn's model has
`fastMode`; a turn on a model without it runs at standard speed and the setting goes off.
`fast: true` for a model without it is refused (`invalid_params`). A queued message keeps its
own `fast` and applies it when delivered.

| Harness | What it reports per model | What `fast` becomes |
| --- | --- | --- |
| Codex | `Model.serviceTiers[]` (`{ id, name, description }`): the model has fast mode when a tier is named "Fast" (or has the id `fast` / `priority`); a listed but empty `serviceTiers` is `unsupported`; no field at all (older Codex) is `unknown` | `turn/start.serviceTierForTurn`: that tier's id when on, `"default"` when off — for the turn only, so the thread's own tier in Codex is never changed. Nothing is sent for a model without a fast tier. |
| Claude Code | `supportsFastMode` on `supportedModels()` (absent: `unknown`) | Claude Code's `fastMode` setting in the query's flag layer: `settings: { fastMode: true }` when a query opens, `applyFlagSettings({ fastMode })` on a live query (re-opened with `resume` if refused). Setting it is also the Agent SDK's opt-in to fast mode. When the result reports `fast_mode_state` other than `on` for a turn asked to be fast, a notice in the transcript says so, with Claude Code's reason. |

## Models are never hardcoded

**Decided.** No model id, model name, effort level, or model-to-capability table appears in YAAC
source code — not in the UI, not in `yaacd`.

- Models come from the harness at probe time: Codex `model/list` (every page of it), Claude
  `supportedModels()`.
- Effort levels are whatever each model reports, in the order reported. They are opaque strings
  to YAAC (`EffortId`). Codex and Claude do not use the same set, and both sets have grown.
- Per-model capabilities come from the same listing wherever the harness provides them (Codex
  reports `inputModalities` per model). Where it does not, the answer is `Unknown`.

### Caching the model list

The host caches each instance's descriptor so the composer and model picker are usable the
moment the app opens, before any connection is up. The cache is a convenience, never an
authority:

1. **On startup:** render from cache; mark the data as unconfirmed.
2. **On connect:** `provider/list` returns fresh descriptors. The cache entry is replaced
   whenever the `revision` differs. This happens on **every** connect, unconditionally.
3. **While connected:** `yaacd` re-probes on a timer, on harness-version change, on account
   change, and on `provider/refresh`; it pushes `provider/updated` when anything differs.
4. **A cached model that is gone** from the fresh list is not silently replaced. A thread that
   was using it shows the model as unavailable and asks for a new choice before the next send.
   (A model is only declared gone after a *complete* listing, never after a partial page.)
5. **A model unknown to the cache** can never be blocked by it: the cache does not validate
   anything. `yaacd` validates the model against the live descriptor at send time.

### When a provider cannot list models

Both MVP providers can. A future provider that cannot reports no `ModelList` capability, and
the model picker then offers "harness default" plus a free-text model id. It never falls back
to a built-in list, and such a model's abilities are `Unknown`.

## Codex

Verified against `codex-cli` 0.170.0 and a running daemon at 0.171.0. Details:
[harness surfaces](research/harness-surfaces.md#codex).

### Daemon backend

`yaacd` connects to the daemon's control socket — the default one, or the path configured on
the instance — and speaks the Codex app-server JSON-RPC protocol over it.

Facts that shape the adapter:

- **The control socket speaks WebSocket**, not line-delimited JSON. `yaacd` opens a WebSocket
  over the Unix socket directly. (`codex app-server proxy` only relays bytes; it does not
  translate.) A private `stdio://` server, by contrast, uses JSON lines.
- **The handshake** is `initialize` (with client info), then an `initialized` notification. The
  reply reports the server's home directory and platform — the source of the instance's
  [store identity](#an-instance-has-a-store-identity) — and its version only inside the
  `userAgent` string. The account comes from `account/read`. The daemon can be a newer
  version than the `codex` on `PATH`; `harness_version` is the server's.
- **The conversation key is `Thread.id`.** `Thread.sessionId` is shared by every thread in a
  session tree and must not be used as an identity.
- **Each client connection is independent and shares one runtime.** `thread/start`,
  `thread/resume`, and `thread/fork` subscribe the calling connection to that thread's events.
  `thread/list` and `thread/read` do not. There is no "resume from sequence N" in Codex — which
  is why YAAC keeps [its own journal](03-protocol.md#journal-and-replay).
- **A thread with no subscribers may be unloaded once idle.** `yaacd` stays subscribed to every
  thread with a running turn, whether or not a UI is attached.
- **Requests go to every subscribed connection; the first answer wins.** A request can also be
  cleared by a turn ending. See [request states](03-protocol.md#request-states).
- **`thread/list` filters by default.** With no source filter it returns interactive sessions
  only, and archived threads need a separate query. Building the full index means asking for
  the source kinds explicitly.
- **Per-turn `model` and `effort` stick** — they apply to that turn and those after it. That is
  the native form of model switching.
- **`turn/start` on a busy thread joins the running turn** rather than queuing a new one, and
  returns that turn's id. A successful call therefore does not mean a new turn began. YAAC
  never relies on Codex to queue: `yaacd` holds its own per-thread queue and sends only when
  the thread is idle and held ([10 Agent mesh](10-agent-mesh.md#one-dispatcher-per-thread)).
- **`turn/interrupt` needs the turn id; `turn/steer` needs the expected turn id.**
- **`thread/revert` changes conversation history only.** It does not restore files — hence two
  separate rewind capabilities.

| YAAC | Codex app-server |
| --- | --- |
| probe | `initialize` + `initialized`, `model/list` (paged, `includeHidden`), `account/read` |
| thread index | `thread/list` (cursor, `sourceKinds`, `archived`), `thread/started`, `thread/name/updated`, `thread/archived`, `thread/deleted`, `thread/status/changed` |
| history | `thread/read`, `thread/turns/list`, `thread/items/list` |
| start / resume | `thread/start` (`cwd`, `model`), `thread/resume` |
| native fork | `thread/fork` (`threadId`, optional `lastTurnId` — inclusive, not a turn in progress — `model`, `cwd`, `approvalPolicy`/`sandbox`, `excludeTurns`); result `{ thread, model, reasoningEffort, … }`. Verified against 0.171.0 (`generate-ts`). |
| turn | `turn/start` (`input`, `model`, `effort`), `turn/steer` (`expectedTurnId`), `turn/interrupt` (`turnId`) |
| streaming | `item/started`, `item/agentMessage/delta`, `item/reasoning/summaryTextDelta`, `item/reasoning/textDelta`, `item/commandExecution/outputDelta`, `item/completed`, `turn/completed` |
| approvals | `item/commandExecution/requestApproval`, `item/fileChange/requestApproval`, `item/permissions/requestApproval`; cleared by `serverRequest/resolved` |
| questions | `item/tool/requestUserInput` → `UserQuestions`; `mcpServer/elicitation/request` → `UserForms` |
| plan, diff, usage | `turn/plan/updated`, `turn/diff/updated`, `thread/tokenUsage/updated`, `account/rateLimits/read` |
| images | input part `localImage { path }`; per-model `inputModalities` |
| rename / archive / delete | `thread/name/set`, `thread/archive`, `thread/delete` (`{ threadId }` → `{}`, and a `thread/deleted { threadId }` notification — verified against `codex app-server generate-ts` of codex-cli 0.170.0, 2026-11-20) |

This backend supports essentially the whole capability list.

The protocol types come from Codex itself (`codex app-server generate-ts`). Because the daemon
can be newer than the types `yaacd` was built with, the adapter tolerates unknown fields,
methods, and notifications rather than failing on them.

### Why there is no SDK backend for Codex

**Decided.** The Codex SDK is a thin wrapper around `codex exec`: it cannot list threads, read
history, fork, list models, ask for approval interactively, or ask the user a question. A
fallback that loses all of that would be a second, much poorer product to design, test, and
explain. YAAC relies on a Codex version with the daemon instead, and `yaacd` does not include
the Codex SDK at all.

## Claude

Verified against Claude Code 2.1.329 and `@anthropic-ai/claude-agent-sdk` 0.3.329. Details:
[harness surfaces](research/harness-surfaces.md#claude-code).

### SDK backend

**Decided:** in the MVP Claude runs on the SDK backend only, as a single instance per target.
One `query()` in streaming-input mode per active thread; the SDK runs the Claude Code process as a child of `yaacd`.

| YAAC | Claude Agent SDK |
| --- | --- |
| probe | `supportedModels()`, `accountInfo()`, `supportedCommands()` on a query |
| thread index | `listSessions()` (across projects, worktree-aware), `getSessionInfo()` |
| history | `getSessionMessages()` |
| start / resume | `query({ options: { cwd, model, sessionId } })`, `options.resume` |
| native fork | `forkSession(sessionId, { upToMessageId?, title?, dir? })` → `{ sessionId }` (`upToMessageId` inclusive; omitted = full copy); verified against SDK 0.3.329 |
| turn | push an `SDKUserMessage` into the input stream; `interrupt()` |
| streaming | `includePartialMessages` stream events → deltas; assistant and tool-result messages → items |
| approvals | `canUseTool` callback → approval `request` items |
| questions | the `AskUserQuestion` tool arriving through `canUseTool` → question `request` items; `onUserDialog` for other dialog kinds |
| model / effort / mode switch | `setModel()`, `applyFlagSettings({ effortLevel })`, `setPermissionMode()` |
| images | image content blocks in the user message (`yaacd` reads the uploaded file) |
| attachments readable | the upload store is passed in `additionalDirectories`; so is a handoff fork's transcript folder (`ProviderInput.readable`), kept for the session's whole life |
| in use elsewhere | `claude agents --json` lists live sessions with their session ids |
| rename / archive / delete | `renameSession()`, `archiveSession()`, `deleteSession(sessionId, { dir? })` (verified in the installed Agent SDK 0.3.329) |
| usage | result messages, `getContextUsage()` |

Notes:

- Each active Claude thread is a live process on the target, and a measured one is not small:
  **about 229 MB while idle**. `yaacd` closes a thread's process after five idle minutes and
  resumes it on demand, and keeps at most four alive at once (least recently used closed
  first, never one with a running turn or an open request). Both limits are configurable.
  This eviction is central to the target-side memory budget.
- The modes offered are the SDK's permission modes, **including the one that bypasses all
  permission checks**. That one is reported with `skipsApprovals` set, so the UI asks for
  confirmation before switching to it ([07 UI](07-ui.md#the-control-row-under-the-composer)).
  Each carries its `autonomy` ([above](#modes-and-how-permissive-they-are)).
- The Claude process is a child of `yaacd`. If `yaacd` dies, so does a turn in progress, and
  any request it was waiting on becomes `expired` — still visible, with its full text.
- `supportedModels()` reports effort levels per model but has no image-input flag. The Claude
  provider reports image input as `Supported` at the provider level. That is a statement about
  the harness, not a model table, and it is the only place such a default exists.
- **Verify (spike S6):** that `applyFlagSettings({ effortLevel })` takes effect on a live query
  and is not overridden by policy; otherwise effort changes re-open the query with `resume`.

### Daemon backend (reserved)

Claude Code does run a local daemon — it supervises background sessions (`claude --bg`,
`claude agents`). As of the versions above there is **no documented interface for a third-party
client to drive a turn through it**: the CLI offers listing, attach (a terminal UI), logs, stop,
and respawn.

The daemon backend therefore remains a declared slot in the contract with no implementation, to
be filled when a supported interface exists. Sessions the Claude daemon supervises are still
listed. One that is currently running there is [in use elsewhere](05-threads.md#in-use-elsewhere)
and so read-only in YAAC; once it is idle it can be continued through the SDK.

## Adding a provider

1. Implement `HarnessProvider` in `daemon/src/providers/<kind>/`.
2. Register the kind and its default-instance probe.
3. Declare capabilities truthfully. Anything not declared is not offered in the UI.
4. Map the harness's events onto the item model. Unmappable output becomes a `toolCall` of type
   `other` or a `notice`; it is never dropped.

No Rust change is required unless the provider needs a capability that does not exist yet.

**Decided:** ACP comes to YAAC as a provider, after the MVP — one generic adapter that can drive
any agent speaking the Agent Client Protocol. It is also the best test of whether this contract
is really provider-neutral.
