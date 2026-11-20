//! Threads: summaries, the index, effective capabilities, and thread-level operations
//! (docs/05-threads.md).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{
    Address, Capability, EffortId, EffortInfo, InputPart, InstanceId, ModeId, ModeInfo, ModelId, OpId,
    Support, SyncToken, TargetPath, ThreadId, Timestamp, TurnId,
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
    }
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

/// The repository a thread's working directory belongs to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRef {
    /// Identifies the project on its target: the git common directory, or the working
    /// directory itself when there is no git.
    pub path: TargetPath,
    /// The repository directory name, for display.
    pub display_name: String,
    /// Normalised `origin` remote URL. Two projects with the same key on different targets are
    /// the same project. Absent: never merged across targets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cross_target_key: Option<String>,
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
    /// Working directory.
    pub cwd: TargetPath,
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

/// `thread/delete` (under `threadDelete`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThreadDeleteParams {
    /// Operation id.
    pub op_id: OpId,
    /// The thread.
    pub thread_id: ThreadId,
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
}

/// `turn/start` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TurnStartResult {
    /// The new turn.
    pub turn_id: TurnId,
}

/// `turn/interrupt`. Takes the turn id, so a late Stop can never hit the next turn.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TurnInterruptParams {
    /// The thread.
    pub thread_id: ThreadId,
    /// The turn to stop.
    pub turn_id: TurnId,
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
