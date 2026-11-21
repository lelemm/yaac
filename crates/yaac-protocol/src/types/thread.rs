//! Threads: summaries, the index, effective capabilities, and thread-level operations
//! (docs/05-threads.md).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{
    Address, Capability, EffortId, EffortInfo, InputPart, InstanceId, ModeId, ModeInfo, ModelId, OpId,
    PageCursor, ProjectId, Support, SyncToken, TargetPath, ThreadId, Timestamp, TurnId,
};
use crate::macros::open_enum;

open_enum! {
    /// What a thread is doing, for the sidebar.
    pub enum ThreadState fallback Unknown {
        /// Nothing running.
        Idle = "idle",
        /// A turn is running.
        Running = "running",
        /// A request (approval, question, form) is pending: an agent is blocked on the user.
        NeedsAttention = "needsAttention",
        /// The last turn failed.
        Failed = "failed",
        /// Blocked on another agent: a `wait` on other threads is in progress.
        Waiting = "waiting",
        /// A thread started by an agent whose last turn completed normally: its work is done.
        Done = "done",
        /// Stopped and held (docs/10-agent-mesh.md#stopping-workers): it does not start again
        /// by itself. `ThreadSummary.hold` says who stopped it.
        Stopped = "stopped",
    }
}

open_enum! {
    /// How a message reaches a thread that is running a turn
    /// (docs/10-agent-mesh.md#queued-or-now).
    pub enum Delivery fallback Unknown {
        /// Wait for the running turn to end.
        Queue = "queue",
        /// Reach it now: steered into the running turn where the harness can steer, otherwise
        /// the running turn is interrupted and a new one starts with the message.
        Now = "now",
    }
}

/// A thread that was stopped and is held: it starts again only when the one who stopped it,
/// or the user, sends it a message, or the user resumes it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThreadHold {
    /// The agent that stopped it; absent when the user did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<Address>,
    /// Who stopped it, for "Stopped by …".
    pub label: String,
    /// When.
    pub at: Timestamp,
}

open_enum! {
    /// Whether a chat's agent may start and message other threads without asking
    /// (docs/10-agent-mesh.md#permission-a-toggle-in-the-chat). A new chat starts on `ask`.
    pub enum AgentsPolicy fallback Unknown {
        /// The first such action in an orchestration raises a `meshPermission` request.
        Ask = "ask",
        /// No prompt; the orchestration runs within the default budget.
        Allow = "allow",
    }
}

// The enum comes from `open_enum!`, which cannot carry `#[default]`.
#[allow(clippy::derivable_impls)]
impl Default for AgentsPolicy {
    fn default() -> Self {
        AgentsPolicy::Ask
    }
}

impl AgentsPolicy {
    /// Whether agents may act without asking. A value this build does not know counts as
    /// `ask`: when unsure, ask.
    pub fn allows(&self) -> bool {
        matches!(self, AgentsPolicy::Allow)
    }
}

open_enum! {
    /// What a [`ProjectRef`] names.
    pub enum ProjectKind fallback Unknown {
        /// A project the user created: `id` names it.
        User = "user",
        /// A folder with discovered threads but no project yet; named after the folder.
        Temporary = "temporary",
    }
}

/// Where a folder sits inside a git repository (docs/05-threads.md#worktrees-that-live-somewhere-else).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RepoLocation {
    /// The repository: its shared (common) git directory. Equal for every checkout of it.
    pub repo: TargetPath,
    /// The folder relative to its checkout's root, `/`-separated: `.` for the root itself.
    pub relative: String,
    /// The checkout's root (the worktree's top-level directory).
    pub root: TargetPath,
    /// The checkout's name when it is not the repository's main checkout (`feature-x`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_name: Option<String>,
}

/// How a thread's checkout differs from origin (docs/07-ui.md#sidebar). Computed once per
/// checkout on the target, not per thread.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GitStatus {
    /// The checked-out branch; absent on a detached head.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// What it is compared with: the branch's upstream, or the remote's default branch when it
    /// has no upstream. Absent when there is nothing to compare with (then only uncommitted
    /// changes are counted).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    /// Files added relative to the base, uncommitted changes included.
    #[serde(default)]
    pub added: u32,
    /// Files modified relative to the base, uncommitted changes included.
    #[serde(default)]
    pub modified: u32,
    /// Files deleted relative to the base, uncommitted changes included.
    #[serde(default)]
    pub deleted: u32,
    /// Commits not pushed.
    #[serde(default)]
    pub ahead: u32,
    /// Commits not pulled.
    #[serde(default)]
    pub behind: u32,
    /// The checkout no longer exists on disk (a deleted worktree); the other fields are the
    /// last ones known.
    #[serde(default)]
    pub removed: bool,
    /// When this was computed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Timestamp>,
}

open_enum! {
    /// Who may act on a thread, **from the point of view of the client receiving it**.
    pub enum Control fallback Unknown {
        /// Nobody is driving it.
        Free = "free",
        /// This client started the running turn.
        Mine = "mine",
        /// Another YAAC client attached to the same `yaacd` is driving it. Read-only.
        OtherClient = "otherClient",
        /// Another program on the target has it (a terminal session, another app). Read-only.
        External = "external",
    }
}

impl Control {
    /// Whether this client may start turns, steer, and answer requests. A control state this
    /// build does not know is treated as in use (read-only): when unsure, it is in use.
    pub fn may_act(&self) -> bool {
        matches!(self, Control::Free | Control::Mine)
    }
}

open_enum! {
    /// How a thread came to exist; drives the sidebar's "hide automated" filter.
    pub enum OriginKind fallback Unknown {
        /// Started by a person in the harness's own interface.
        Interactive = "interactive",
        /// Started from YAAC (by a person or by an agent through the mesh). Always shown.
        Yaac = "yaac",
        /// Everything else (SDK, sub-agent, background worker). Hidden by default.
        Programmatic = "programmatic",
    }
}

/// The project a thread belongs to.
///
/// With `kind` present (every `ThreadSummary` from a current `yaacd`), this is a project the
/// user created (`user`, with its `id`) or a temporary one (`temporary`); `path` is then the
/// project's folder and `displayName` its name — the user's, or the folder path for a
/// temporary project. Without `kind` (`project/resolve`) it is a git identity: `path` is the
/// git common directory, or the directory itself when there is no git.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRef {
    /// The project's folder (or the git identity; see above).
    pub path: TargetPath,
    /// The name to show.
    pub display_name: String,
    /// Normalised `origin` remote URL. Two projects with the same key on different targets are
    /// the same project. Absent: never merged across targets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cross_target_key: Option<String>,
    /// A user project or a temporary one. Absent: a git identity (`project/resolve`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<ProjectKind>,
    /// The user project, for `kind: user`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<ProjectId>,
    /// Where the project's folder sits in a git repository, when it is in one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<RepoLocation>,
}

/// The git worktree containing a working directory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeRef {
    /// Top-level path of the worktree.
    pub path: TargetPath,
    /// Checked-out branch, when on one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
}

/// What the sidebar needs, and what the host cache stores, per thread.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThreadSummary {
    /// The thread.
    pub id: ThreadId,
    /// Its provider instance. Never changes.
    pub instance: InstanceId,
    /// The harness's own conversation id (Codex `Thread.id`, the Claude session id): what a
    /// user copies from a terminal. Opaque; for finding a thread, never for routing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_id: Option<String>,
    /// Harness title, or first prompt, or user override.
    pub title: String,
    /// Short excerpt.
    #[serde(default)]
    pub preview: String,
    /// Working directory.
    pub cwd: TargetPath,
    /// Resolved repository.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<ProjectRef>,
    /// Resolved worktree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<WorktreeRef>,
    /// Where the working directory sits in a git repository; absent outside git.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<RepoLocation>,
    /// How the thread's checkout differs from origin; absent outside git or not yet known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<GitStatus>,
    /// Branch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// The model last applied, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelId>,
    /// What it is doing.
    pub state: ThreadState,
    /// Who may act on it, relative to the receiving client.
    pub control: Control,
    /// Where it is in use when `control` is read-only (a host name, or a program), for
    /// "In use on …".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_holder: Option<String>,
    /// The thread whose history this one began with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forked_from: Option<ThreadId>,
    /// The agent that started this thread, possibly on another target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawned_by: Option<Address>,
    /// The orchestration root this thread belongs to, when it was spawned or messaged by an
    /// agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<Address>,
    /// How many threads this one spawned. The threads themselves are found through the
    /// indexes (each child's `spawnedBy` names its parent, on any target).
    #[serde(default)]
    pub children: u32,
    /// How many threads this one has exchanged messages with, without either spawning the
    /// other.
    #[serde(default)]
    pub peers: u32,
    /// How it came to exist.
    pub origin_kind: OriginKind,
    /// Creation time.
    pub created_at: Timestamp,
    /// Last activity.
    pub updated_at: Timestamp,
    /// Archived.
    #[serde(default)]
    pub archived: bool,
    /// This chat's agents policy. Meaningful on a root thread; workers act under their
    /// root's answer.
    #[serde(default)]
    pub agents: AgentsPolicy,
    /// Set while the thread is stopped and held (state `stopped`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hold: Option<ThreadHold>,
}

/// The model / effort / mode the harness reports it is **actually** using.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AppliedSettings {
    /// Model in use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelId>,
    /// Effort in use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<EffortId>,
    /// Mode in use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<ModeId>,
}

/// What a thread can do right now, computed on the target from the instance, the model, the
/// account, and the thread's state. The UI does not combine flags itself.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThreadEffective {
    /// What can be done in this thread, now.
    #[serde(default)]
    pub capabilities: Vec<Capability>,
    /// Whether the selected model can see images. Decides `image` versus `file` parts.
    pub image_input: Support,
    /// Accepted image media types, when known.
    #[serde(default)]
    pub image_formats: Vec<String>,
    /// Selectable now; empty when effort cannot be changed.
    #[serde(default)]
    pub efforts: Vec<EffortInfo>,
    /// Selectable modes.
    #[serde(default)]
    pub modes: Vec<ModeInfo>,
    /// What the harness is really using.
    #[serde(default)]
    pub applied: AppliedSettings,
}

// ── thread/sync ────────────────────────────────────────────────────────────────────────────

/// `thread/sync`: incremental thread index.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThreadSyncParams {
    /// The last token received; empty for a full listing.
    #[serde(default)]
    pub token: SyncToken,
    /// Upper bound on the size of the page in bytes, chosen by the client.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<u64>,
}

/// `thread/sync` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThreadSyncResult {
    /// Summaries added or changed since the token.
    #[serde(default)]
    pub changed: Vec<ThreadSummary>,
    /// Threads the harness positively reported gone.
    #[serde(default)]
    pub removed: Vec<ThreadId>,
    /// Token to send next time.
    pub token: SyncToken,
    /// More pages follow; call again with `token`.
    #[serde(default)]
    pub more: bool,
    /// This is (the first page of) a full listing: the client replaces its cached index for
    /// this target once the last page arrives, instead of merging.
    #[serde(default)]
    pub full: bool,
    /// Instances whose last scan failed or was incomplete. Their threads stay listed.
    #[serde(default)]
    pub stale_instances: Vec<InstanceId>,
}

/// `thread/updated` notification: one summary changed, pushed while connected.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThreadUpdatedParams {
    /// The new summary.
    pub thread: ThreadSummary,
    /// The index token that includes this change.
    pub token: SyncToken,
}

/// `thread/removed` notification.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThreadRemovedParams {
    /// The thread.
    pub thread_id: ThreadId,
    /// The index token that includes this change.
    pub token: SyncToken,
}

// ── thread/start ───────────────────────────────────────────────────────────────────────────

/// `thread/start`: create a thread **and** send its first message, as one operation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThreadStartParams {
    /// Operation id.
    pub op_id: OpId,
    /// The provider instance. Fixed for the life of the thread.
    pub instance: InstanceId,
    /// Working directory. With `project`, it must be empty or that project's folder: the
    /// thread starts in the root of the project's folder.
    pub cwd: TargetPath,
    /// The project the thread is started in (docs/05-threads.md#projects).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<ProjectId>,
    /// The chat's agents policy from its first message; `ask` when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agents: Option<AgentsPolicy>,
    /// Model; the harness default when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelId>,
    /// Effort.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<EffortId>,
    /// Mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<ModeId>,
    /// The first message.
    pub input: Vec<InputPart>,
}

/// `thread/start` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThreadStartResult {
    /// The new thread.
    pub thread_id: ThreadId,
    /// Its first turn.
    pub turn_id: TurnId,
}

// ── thread/fork ────────────────────────────────────────────────────────────────────────────

open_enum! {
    /// How a fork carries history.
    pub enum ForkKind fallback Unknown {
        /// The harness forks within one conversation store. Lossless.
        Native = "native",
        /// The history is carried as a transcript file.
        Handoff = "handoff",
    }
}

/// `thread/fork`: create a new thread from an existing one, with its first message.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThreadForkParams {
    /// Operation id.
    pub op_id: OpId,
    /// The thread to fork from. Not modified; may be in use elsewhere.
    pub source: ThreadId,
    /// Destination instance, on the same target.
    pub instance: InstanceId,
    /// Model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelId>,
    /// Effort.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<EffortId>,
    /// Mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<ModeId>,
    /// Fork after this turn; the last completed turn when absent. A native fork honours it
    /// only under `threadForkAtTurn`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at_turn: Option<TurnId>,
    /// The first message of the new thread.
    pub input: Vec<InputPart>,
}

/// `thread/fork` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThreadForkResult {
    /// The new thread.
    pub thread_id: ThreadId,
    /// Its first turn.
    pub turn_id: TurnId,
    /// The mechanism used.
    pub fork_kind: ForkKind,
}

/// `thread/forkOnly`: create a new thread from an existing one **without** a first message.
/// The new thread is idle; its first message goes with `turn/start`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThreadForkOnlyParams {
    /// Operation id.
    pub op_id: OpId,
    /// The thread to fork from. Not modified; may be in use elsewhere.
    pub source: ThreadId,
    /// Destination instance, on the same target.
    pub instance: InstanceId,
    /// Model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelId>,
    /// Effort.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<EffortId>,
    /// Mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<ModeId>,
    /// Fork after this turn, as in `thread/fork`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at_turn: Option<TurnId>,
}

/// `thread/forkOnly` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThreadForkOnlyResult {
    /// The new thread.
    pub thread_id: ThreadId,
    /// The mechanism used.
    pub fork_kind: ForkKind,
}

/// `thread/forkPreview`: what a fork would send and where, for the confirmation dialog.
/// Changes nothing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThreadForkPreviewParams {
    /// The thread to fork from.
    pub source: ThreadId,
    /// Destination instance.
    pub instance: InstanceId,
    /// Fork point, as in `thread/fork`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at_turn: Option<TurnId>,
    /// Upper bound on the transcript preview, in bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_preview_bytes: Option<u64>,
}

/// `thread/forkPreview` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThreadForkPreviewResult {
    /// The mechanism that would be used.
    pub fork_kind: ForkKind,
    /// The turn it would cut off after.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cutoff_turn: Option<TurnId>,
    /// Destination account, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination_account: Option<String>,
    /// Size of the full transcript, for a handoff.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_bytes: Option<u64>,
    /// The beginning of the transcript, for a handoff.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_preview: Option<String>,
    /// Attachments that are gone from the target and would be marked as omitted.
    #[serde(default)]
    pub omitted_attachments: Vec<String>,
    /// Why a fork is not possible, when it is not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<String>,
}

// ── rename / archive / delete ─────────────────────────────────────────────────────────────

/// `thread/rename` (under `threadRename`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThreadRenameParams {
    /// Operation id.
    pub op_id: OpId,
    /// The thread.
    pub thread_id: ThreadId,
    /// New title.
    pub title: String,
}

/// `thread/archive` (under `threadArchive`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThreadArchiveParams {
    /// Operation id.
    pub op_id: OpId,
    /// The thread.
    pub thread_id: ThreadId,
}

/// `thread/update`: YAAC's own settings of a thread — not the harness's, and not the host's view
/// preferences (pin, mute, dismiss live in the hub: D92). A patch:
/// absent fields are unchanged. Stored on the target; the change is pushed as `thread/updated`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThreadUpdateParams {
    /// Operation id.
    pub op_id: OpId,
    /// The thread.
    pub thread_id: ThreadId,
    /// The chat's agents policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agents: Option<AgentsPolicy>,
    /// `false`: resume a stopped thread (release its hold) without sending it anything.
    /// `true`: hold it (it starts nothing by itself until released).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held: Option<bool>,
}

/// `thread/update` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThreadUpdateResult {
    /// The summary after the change.
    pub thread: ThreadSummary,
}

/// `thread/delete` (D110): `yaacd` always removes its own record of the thread and its
/// journal; with `alsoInHarness` (offered only under `threadDelete`) it first asks the
/// provider to delete its own session too.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThreadDeleteParams {
    /// Operation id.
    pub op_id: OpId,
    /// The thread.
    pub thread_id: ThreadId,
    /// Also delete the session in the harness (under `threadDelete`). Absent: `false` — only
    /// YAAC's record goes, the harness keeps its session.
    #[serde(default)]
    pub also_in_harness: bool,
}

// ── turns ─────────────────────────────────────────────────────────────────────────────────

/// `turn/start`: send a user message to an existing thread.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TurnStartParams {
    /// Operation id.
    pub op_id: OpId,
    /// The thread.
    pub thread_id: ThreadId,
    /// The message.
    pub input: Vec<InputPart>,
    /// Model override (under `modelSwitch`). Sticks for later turns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelId>,
    /// Effort override (under `effortSwitch`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<EffortId>,
    /// Mode override (under `modes`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<ModeId>,
    /// What to do while a turn is running (docs/07-ui.md#composer). Absent: refuse with
    /// `turn_in_progress`, as before. `queue`: kept by `yaacd` (durably, in order, ahead of
    /// queued agent messages) and delivered when the turn ends. `now`: steered into the
    /// running turn under `steer`, otherwise the turn is interrupted and a new one starts with
    /// it. While the thread waits on a request for the user, only `queue` is accepted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery: Option<Delivery>,
}

open_enum! {
    /// What `turn/start` did.
    pub enum TurnStartOutcome fallback Unknown {
        /// A new turn started (`turnId`).
        Started = "started",
        /// Queued behind the running turn (`queueId`); `turnId` is empty.
        Queued = "queued",
        /// Added to the running turn (`turnId` is that turn).
        Steered = "steered",
        /// The running turn was interrupted and a new one started with the message (`turnId`).
        InterruptedAndStarted = "interruptedAndStarted",
    }
}

/// `turn/start` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TurnStartResult {
    /// The new turn (the running turn when steered; empty when queued).
    pub turn_id: TurnId,
    /// What happened; absent from a `yaacd` that predates `delivery` (then: started).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<TurnStartOutcome>,
    /// The queued message, when queued.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_id: Option<String>,
}

/// A user message waiting in a thread's queue for the running turn to end.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct QueuedMessage {
    /// Its id in the queue.
    pub queue_id: String,
    /// The message.
    pub input: Vec<InputPart>,
    /// When it was queued.
    pub queued_at: Timestamp,
    /// Model override for the turn it starts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelId>,
    /// Effort override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<EffortId>,
    /// Mode override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<ModeId>,
}

/// `turn/queueEdit`: replace a queued message's text and attachments before it is delivered.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TurnQueueEditParams {
    /// Operation id.
    pub op_id: OpId,
    /// The thread.
    pub thread_id: ThreadId,
    /// The queued message.
    pub queue_id: String,
    /// The new message.
    pub input: Vec<InputPart>,
}

/// `turn/queueCancel`: remove a queued message before it is delivered.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TurnQueueCancelParams {
    /// Operation id.
    pub op_id: OpId,
    /// The thread.
    pub thread_id: ThreadId,
    /// The queued message.
    pub queue_id: String,
}

/// `turn/interrupt`. Takes the turn id, so a late Stop can never hit the next turn.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TurnInterruptParams {
    /// The thread.
    pub thread_id: ThreadId,
    /// The turn to stop.
    pub turn_id: TurnId,
    /// Also hold the thread (state `stopped`): queued messages and agent messages wait until
    /// the user sends it a message or resumes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hold: Option<bool>,
}

/// `turn/steer` (under `steer`): add input to the running turn.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TurnSteerParams {
    /// Operation id.
    pub op_id: OpId,
    /// The thread.
    pub thread_id: ThreadId,
    /// The turn the client expects to be running; `turn_mismatch` otherwise.
    pub turn_id: TurnId,
    /// The input to add.
    pub input: Vec<InputPart>,
}

// ── Discovery: caps, older threads, lookup by id, dismiss all ─────────────────────────────

/// The discovery caps of a target (docs/05-threads.md#discovery-is-capped-by-age-and-by-number).
/// A conversation YAAC did not create is indexed only if it was active within `maxAgeDays` and
/// is among the `maxThreads` most recent of its provider instance. 0 turns that cap off.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryCaps {
    /// Whether the caps apply at all.
    pub enabled: bool,
    /// Age cap, in days of inactivity (0: none).
    pub max_age_days: u32,
    /// Number cap, per provider instance (0: none).
    pub max_threads: u32,
}

impl Default for DiscoveryCaps {
    /// The documented defaults: 30 days, 50 per instance.
    fn default() -> Self {
        Self { enabled: true, max_age_days: 30, max_threads: 50 }
    }
}

/// A change to the caps. Absent fields are unchanged.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryCapsUpdate {
    /// See [`DiscoveryCaps`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// See [`DiscoveryCaps`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_age_days: Option<u32>,
    /// See [`DiscoveryCaps`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_threads: Option<u32>,
}

/// `discovery/caps`: read the caps, and change them when `update` is present (then `opId` is
/// required). A change rescans every instance by the new caps.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryCapsParams {
    /// Operation id, required with `update`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub op_id: Option<OpId>,
    /// The change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update: Option<DiscoveryCapsUpdate>,
}

/// `discovery/caps` result, and the `discovery/updated` notification.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryCapsResult {
    /// The caps in force.
    pub caps: DiscoveryCaps,
}

/// `thread/older`: conversations beyond the discovery caps, newest first, a page at a time
/// ("Show older", "Search older threads on …"). Not indexed unless opened, pinned, or active.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThreadOlderParams {
    /// Only this provider instance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<InstanceId>,
    /// Only threads of the project with this folder (`ProjectRef.path`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<TargetPath>,
    /// Only threads whose title contains these words, or whose id starts with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    /// From a previous page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<PageCursor>,
    /// Page size (default 20, at most 100).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// `thread/older` result.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThreadOlderResult {
    /// Summaries, newest first. Their ids work with every thread method.
    #[serde(default)]
    pub threads: Vec<ThreadSummary>,
    /// The next page, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<PageCursor>,
}

/// `thread/lookup`: find a thread by id — the YAAC thread id or the harness's own
/// conversation id — asking the harness directly when the index does not have it, and bringing
/// it into the index.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThreadLookupParams {
    /// The id.
    pub id: String,
    /// Only this provider instance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<InstanceId>,
}

/// `thread/lookup` result.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThreadLookupResult {
    /// The threads with that id (normally one).
    #[serde(default)]
    pub threads: Vec<ThreadSummary>,
}

/// `thread/keep`: the threads the user keeps on the host (pinned or muted there). The discovery
/// caps never cut them. Replaces the previous set; the daemon learns nothing about why.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThreadKeepParams {
    /// Every thread to keep.
    #[serde(default)]
    pub thread_ids: Vec<ThreadId>,
}
