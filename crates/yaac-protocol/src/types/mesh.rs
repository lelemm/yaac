//! The agent mesh as it crosses the protocol (docs/10-agent-mesh.md#in-the-protocol).
//!
//! Direction matters here. `mesh/relay` and `mesh/query` are requests **`yaacd` originates**
//! and the host answers (the one exception to "the host sends requests"); `mesh/query`,
//! `mesh/deliver`, `mesh/receipt`, and `mesh/roster` are sent by the host to a target.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{
    Address, EffortId, InstanceId, MeshPeer, MessageId, ModeId, ModeInfo, ModelId, OpId, ProjectRef, ProviderDescriptor,
    Delivery, TargetId, TargetName, TargetPath, ThreadState, Timestamp, TurnId,
};
use crate::macros::{open_enum, open_union};

open_enum! {
    /// What a mesh message is.
    pub enum MeshMessageKind fallback Unknown {
        /// An agent's message.
        Message = "message",
        /// A worker's final answer to its parent, sent once for the turn the spawn started.
        Result = "result",
        /// YAAC itself speaking. Passive: never wakes a thread; handed over with the next turn
        /// or `wait`.
        Notice = "notice",
    }
}

/// A message between two agents. Sender and body are separate fields all the way to the
/// harness.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MeshMessage {
    /// Unique; every hop deduplicates by it.
    pub id: MessageId,
    /// Stamped by `yaacd` from the sender's attachment ticket, never by the agent. For a
    /// `notice`, the thread the notice is about.
    pub from: Address,
    /// The recipient.
    pub to: Address,
    /// What it is.
    pub kind: MeshMessageKind,
    /// The body. Text only.
    pub text: String,
    /// The message this replies to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_reply_to: Option<MessageId>,
    /// When the sender's `yaacd` stored it.
    pub sent_at: Timestamp,
    /// Position in the sender-to-recipient sequence (starts at 1). The recipient's dispatcher
    /// hands messages over in this order.
    pub seq: u64,
    /// The orchestration root this message belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<Address>,
    /// `queue` (absent) or `now` (docs/10-agent-mesh.md#queued-or-now).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery: Option<Delivery>,
}

open_enum! {
    /// How the recipient handled a peer message.
    pub enum DeliveryHandling fallback Unknown {
        /// Added to the recipient's running turn.
        Steered = "steered",
        /// The harness accepted it for the next boundary of its running turn.
        NextBoundary = "nextBoundary",
        /// The recipient's running turn was interrupted and a new one started with it.
        InterruptedAndDelivered = "interruptedAndDelivered",
        /// The recipient was idle; a turn started with it.
        Delivered = "delivered",
        /// It waits in the mailbox (`reason` says why).
        Queued = "queued",
    }
}

open_enum! {
    /// How far a message got.
    pub enum ReceiptStatus fallback Unknown {
        /// Stored in the recipient's mailbox on its target.
        Delivered = "delivered",
        /// Stored, and waiting; `reason` says for what.
        Queued = "queued",
        /// Could not be delivered at all; `reason` says why.
        Failed = "failed",
        /// The harness may have accepted it; it will not be sent again.
        OutcomeUnknown = "outcomeUnknown",
    }
}

open_enum! {
    /// Why a message is queued or failed.
    pub enum ReceiptReason fallback Unknown {
        /// The recipient's target is offline; the hub holds it.
        TargetOffline = "targetOffline",
        /// No hub is attached to the sender's target; it waits in the outbox.
        HubUnavailable = "hubUnavailable",
        /// The recipient is running a turn.
        RecipientBusy = "recipientBusy",
        /// The recipient is waiting on an approval or question from the user.
        AwaitingUser = "awaitingUser",
        /// Another program holds the recipient (including idle in a terminal).
        InUseElsewhere = "inUseElsewhere",
        /// The orchestration's budget ran out.
        BudgetExhausted = "budgetExhausted",
        /// No such address.
        UnknownAddress = "unknownAddress",
        /// The recipient was deleted.
        ThreadDeleted = "threadDeleted",
        /// A mesh limit was exceeded.
        LimitExceeded = "limitExceeded",
        /// The mesh policy refused it.
        PolicyDenied = "policyDenied",
        /// The recipient's target does not accept messages from other targets.
        RemoteDisallowed = "remoteDisallowed",
        /// The recipient was stopped and is held until its stopper or the user messages it.
        Stopped = "stopped",
        /// `now` was asked toward a thread that is not beneath the sender: delivered as `queue`.
        NotBeneathSender = "notBeneathSender",
    }
}

/// A delivery receipt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Receipt {
    /// How far it got.
    pub status: ReceiptStatus,
    /// Why it is queued or failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<ReceiptReason>,
    /// Text for the agent and the user, shown as-is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// How the harness took the message, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handling: Option<DeliveryHandling>,
}

/// `mesh/relay` (target → hub): carry a message to another target. The hub answers once the
/// message is in its durable relay queue. Retried by the target until answered; the message id
/// makes a repeat harmless.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MeshRelayParams {
    /// The message.
    pub message: MeshMessage,
}

/// `mesh/deliver` (hub → target): put a message into a recipient's mailbox. The target
/// answers once it is on disk. Retried by the hub until answered.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MeshDeliverParams {
    /// The message.
    pub message: MeshMessage,
}

/// Result of `mesh/relay` and `mesh/deliver`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MeshReceiptResult {
    /// How far the message got, as far as the answering side knows.
    pub receipt: Receipt,
}

/// `mesh/receipt` (hub → source target): a relayed message moved on (for example it reached
/// the recipient's mailbox after its target came back), so the sender's `meshSent` item can
/// be updated.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MeshReceiptParams {
    /// The message.
    pub message_id: MessageId,
    /// Its new receipt.
    pub receipt: Receipt,
}

/// A filter for `listThreads`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MeshThreadFilter {
    /// Only threads in the caller's orchestration.
    #[serde(default)]
    pub mine: bool,
    /// Only threads spawned by this thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub children_of: Option<Address>,
    /// Only running threads.
    #[serde(default)]
    pub running: bool,
    /// Only threads of this project (its cross-target key or path).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// Free text matched against title and preview.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Upper bound on results.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// Include idle, archived, and automated threads (by default only active ones).
    #[serde(default)]
    pub include_inactive: bool,
    /// Only threads in this state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<ThreadState>,
    /// The cursor of a previous page (bound to the filter and to a snapshot of the results).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// `listModels`: the instances on the destination and their models.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListModelsQuery {}

/// `listThreads`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListThreadsQuery {
    /// The filter.
    #[serde(default)]
    pub filter: MeshThreadFilter,
}

/// `readThread`: recent messages, or the last answer, of a thread there.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReadThreadQuery {
    /// The thread.
    pub thread: Address,
    /// Only the last agent answer.
    #[serde(default)]
    pub last_answer_only: bool,
    /// Upper bound on the reply in bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<u64>,
}

/// `spawnThread`: start a new thread there, as a child of the caller.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SpawnThreadQuery {
    /// Provider instance, as returned by `listModels`.
    pub instance: InstanceId,
    /// Model, as returned by `listModels`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelId>,
    /// Effort.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<EffortId>,
    /// Working directory on the destination. When absent, the destination resolves "the same
    /// project" from `projectKey`; it confirms the directory exists before starting anything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<TargetPath>,
    /// The caller's project cross-target key (normalised remote URL).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_key: Option<String>,
    /// Title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The first message.
    pub text: String,
    /// Do not send the child's final answer back to the parent.
    #[serde(default)]
    pub detached: bool,
    /// The orchestration root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<Address>,
    /// The new thread's depth of spawning (the caller's depth plus one).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth: Option<u32>,
    /// Every thread above the new one, nearest first (the caller, its parent, …), so the
    /// destination can tell later who is beneath whom.
    #[serde(default)]
    pub ancestors: Vec<Address>,
    /// The mode the worker is asked to run in (`spawn_thread`'s `mode`), as `listModels`
    /// lists it for that instance. Still capped at `parentMode` (D146).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<ModeId>,
    /// The caller's effective mode when it spawned, as its own provider describes it. The
    /// destination maps it onto the worker's provider by `skipsApprovals` and `autonomy`, never
    /// to anything more permissive (D146). Absent: the caller's mode is not known, and the
    /// worker starts in its harness's default mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_mode: Option<Box<ModeInfo>>,
}

/// `findThreads`: find threads across every target by id or fuzzy title. Answered by the hub
/// from its own thread index (no fan-out); the query's `target` is ignored.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FindThreadsQuery {
    /// An id, an id prefix (four characters or more), an address, or words of a title.
    pub query: String,
    /// Only threads of this target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub only_target: Option<TargetId>,
    /// Only threads in this state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<ThreadState>,
    /// The cursor of a previous page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// `listTargets`: the targets of the mesh with counts. Answered by the hub; the query's
/// `target` is ignored.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListTargetsQuery {}

/// `stopThread`: interrupt a thread there. Only for the caller's own workers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StopThreadQuery {
    /// The thread.
    pub thread: Address,
    /// Also every thread beneath it (absent: `true`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtree: Option<bool>,
    /// The agent that asked for the stop, when it is not the caller (a stop passed down a
    /// subtree across targets). Its hold names this agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<Address>,
}

/// `threadStates`: the current state of some threads there (used by `wait`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThreadStatesQuery {
    /// The threads.
    pub threads: Vec<Address>,
}

open_union! {
    /// A tool call that needs another target.
    pub enum MeshQuery tag "tool" {
        /// `list_models` there.
        ListModels(ListModelsQuery) = "listModels",
        /// `list_threads` there.
        ListThreads(ListThreadsQuery) = "listThreads",
        /// `read_thread` there.
        ReadThread(ReadThreadQuery) = "readThread",
        /// `spawn_thread` there.
        SpawnThread(SpawnThreadQuery) = "spawnThread",
        /// `stop_thread` there.
        StopThread(StopThreadQuery) = "stopThread",
        /// Thread states there.
        ThreadStates(ThreadStatesQuery) = "threadStates",
        /// `find_thread` across every target (answered by the hub).
        FindThreads(FindThreadsQuery) = "findThreads",
        /// `list_targets` (answered by the hub).
        ListTargets(ListTargetsQuery) = "listTargets",
    }
}

/// `mesh/query`. Sent by the source `yaacd` to the hub with `target` = destination; the hub
/// forwards the same params to the destination `yaacd` and relays the answer back.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MeshQueryParams {
    /// The calling thread, stamped by its `yaacd`.
    pub caller: Address,
    /// The destination target.
    pub target: TargetId,
    /// The query.
    pub query: MeshQuery,
}

/// A thread as `listThreads` reports it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MeshThreadInfo {
    /// Address and label.
    pub peer: MeshPeer,
    /// State.
    pub state: ThreadState,
    /// Who spawned it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawned_by: Option<Address>,
    /// Working directory.
    pub cwd: TargetPath,
    /// Project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<ProjectRef>,
}

open_enum! {
    /// Who said a transcript entry.
    pub enum TranscriptRole fallback Unknown {
        /// The user.
        User = "user",
        /// The agent.
        Agent = "agent",
        /// Another agent, through the mesh.
        Peer = "peer",
        /// YAAC.
        Notice = "notice",
    }
}

/// One entry of a `readThread` reply.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptEntry {
    /// Who.
    pub role: TranscriptRole,
    /// The text.
    pub text: String,
    /// The sending agent, for `peer`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<Address>,
}

/// One thread's state in a `threadStates` reply.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MeshThreadState {
    /// The thread.
    pub thread: Address,
    /// Its state; absent when it does not exist.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<ThreadState>,
}

/// `listModels` reply.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListModelsReply {
    /// The same live descriptors as the model picker.
    pub providers: Vec<ProviderDescriptor>,
}

/// `listThreads` reply.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListThreadsReply {
    /// Matching threads (one page).
    pub threads: Vec<MeshThreadInfo>,
    /// How many threads matched in all, when the answer is paged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<u32>,
    /// The next page, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// One `findThreads` match.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FoundThreadInfo {
    /// The thread.
    pub thread: MeshThreadInfo,
    /// How it matched: `id`, `nativeId`, `address`, or `title`.
    pub matched: String,
    /// Whether an id matched in full (rather than by prefix).
    #[serde(default)]
    pub exact: bool,
}

/// `findThreads` reply: one page, best first.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FindThreadsReply {
    /// This page.
    pub results: Vec<FoundThreadInfo>,
    /// How many matched in all.
    pub total: u32,
    /// The query was an id prefix that matched several threads.
    #[serde(default)]
    pub ambiguous: bool,
    /// The next page, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// One target in a `listTargets` reply.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TargetCount {
    /// Immutable id.
    pub id: TargetId,
    /// The user's label.
    pub name: TargetName,
    /// Whether the hub is connected to it now.
    pub reachable: bool,
    /// Threads active there (running, waiting, or needing the user), as last known.
    pub active_threads: u32,
}

/// `listTargets` reply.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListTargetsReply {
    /// Every target.
    pub targets: Vec<TargetCount>,
}

/// `readThread` reply.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReadThreadReply {
    /// Entries, oldest first.
    pub entries: Vec<TranscriptEntry>,
    /// Earlier entries were left out.
    #[serde(default)]
    pub truncated: bool,
}

/// `spawnThread` reply.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SpawnThreadReply {
    /// The new thread.
    pub thread: MeshPeer,
    /// Its first turn.
    pub turn_id: TurnId,
    /// One sentence saying which mode it runs in and why ("Runs in Bypass permissions, as its
    /// parent."), for the spawn result. Absent from an older `yaacd`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode_note: Option<String>,
}

/// `stopThread` reply.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StopThreadReply {
    /// One per thread, the asked one first.
    #[serde(default)]
    pub results: Vec<StopResult>,
}

open_enum! {
    /// What a stop did to one thread.
    pub enum StopOutcome fallback Unknown {
        /// Its running turn ended; it is held.
        Stopped = "stopped",
        /// It was not running; it is held.
        WasNotRunning = "wasNotRunning",
        /// Its turn had not ended a few seconds after the interrupt; it is held anyway.
        DidNotRespond = "didNotRespond",
        /// It could not be reached (its machine is offline).
        Unreachable = "unreachable",
    }
}

/// One thread's stop outcome.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StopResult {
    /// The thread.
    pub thread: Address,
    /// What happened.
    pub outcome: StopOutcome,
    /// Its title, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// More, shown as-is (e.g. whether its harness process was ended).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// `threadStates` reply.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThreadStatesReply {
    /// One entry per thread asked about.
    pub states: Vec<MeshThreadState>,
}

open_union! {
    /// `mesh/query` result; its `tool` matches the query's. Failures are ordinary errors
    /// (`target_offline`, `limit_exceeded`, `policy_denied`, …) with a readable message.
    pub enum MeshQueryResult tag "tool" {
        /// `listModels`.
        ListModels(ListModelsReply) = "listModels",
        /// `listThreads`.
        ListThreads(ListThreadsReply) = "listThreads",
        /// `readThread`.
        ReadThread(ReadThreadReply) = "readThread",
        /// `spawnThread`.
        SpawnThread(SpawnThreadReply) = "spawnThread",
        /// `stopThread`.
        StopThread(StopThreadReply) = "stopThread",
        /// `threadStates`.
        ThreadStates(ThreadStatesReply) = "threadStates",
        /// `findThreads`.
        FindThreads(FindThreadsReply) = "findThreads",
        /// `listTargets`.
        ListTargets(ListTargetsReply) = "listTargets",
    }
}

/// One target in the roster.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RosterEntry {
    /// Immutable id.
    pub id: TargetId,
    /// The user's label.
    pub name: TargetName,
    /// Whether the hub is connected to it now.
    pub reachable: bool,
}

/// `mesh/roster` (hub → target): the target's own identity and the list of targets. Sent
/// after `initialize` and whenever it changes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MeshRosterParams {
    /// This target's id.
    pub self_id: TargetId,
    /// This target's name.
    pub self_name: TargetName,
    /// Every target in the mesh, this one included.
    pub targets: Vec<RosterEntry>,
}

open_enum! {
    /// Whether spawning, cross-target sending, and reading outside one's orchestration happen
    /// freely or ask the user first (with an ordinary request card).
    pub enum MeshPolicyMode fallback Unknown {
        /// Allowed (the default).
        Allow = "allow",
        /// Ask the first time in each thread.
        AskFirst = "askFirst",
        /// Always ask.
        AlwaysAsk = "alwaysAsk",
    }
}

/// Local runaway limits, per target.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MeshLimits {
    /// Workers per thread.
    pub workers_per_thread: u32,
    /// Depth of spawning.
    pub spawn_depth: u32,
    /// Threads running at once on this target (a thread blocked in `wait` does not count).
    pub running_threads: u32,
    /// Messages between the same two threads per window.
    pub pair_messages: u32,
    /// That window, in seconds.
    pub pair_window_secs: u32,
    /// After this many seconds undelivered, the sender gets a notice.
    pub undelivered_notice_secs: u32,
}

impl Default for MeshLimits {
    /// The documented defaults (docs/10-agent-mesh.md#local-limits).
    fn default() -> Self {
        Self {
            workers_per_thread: 8,
            spawn_depth: 3,
            running_threads: 6,
            pair_messages: 30,
            pair_window_secs: 600,
            undelivered_notice_secs: 900,
        }
    }
}

/// Allowance per orchestration root, counted across all targets.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MeshBudget {
    /// Turns started by agents (not by a person).
    pub agent_turns: u32,
    /// Threads spawned in total.
    pub threads_spawned: u32,
    /// Wall-clock seconds since a person last acted on the root.
    pub wall_clock_secs: u64,
}

impl Default for MeshBudget {
    /// The documented defaults (docs/10-agent-mesh.md#every-orchestration-has-a-root-and-a-budget).
    fn default() -> Self {
        Self { agent_turns: 100, threads_spawned: 20, wall_clock_secs: 12 * 60 * 60 }
    }
}

/// The mesh policy of one target.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MeshPolicy {
    /// Allow, ask first, or always ask.
    pub mode: MeshPolicyMode,
    /// Whether agents on other targets may start or message threads here at all.
    pub accept_from_other_targets: bool,
    /// Local limits.
    pub limits: MeshLimits,
    /// Budget per orchestration root.
    pub budget: MeshBudget,
}

impl Default for MeshPolicy {
    fn default() -> Self {
        Self {
            mode: MeshPolicyMode::Allow,
            accept_from_other_targets: true,
            limits: MeshLimits::default(),
            budget: MeshBudget::default(),
        }
    }
}

/// A change to some limits. Absent fields are unchanged.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MeshLimitsPatch {
    /// See [`MeshLimits`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workers_per_thread: Option<u32>,
    /// See [`MeshLimits`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawn_depth: Option<u32>,
    /// See [`MeshLimits`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub running_threads: Option<u32>,
    /// See [`MeshLimits`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pair_messages: Option<u32>,
    /// See [`MeshLimits`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pair_window_secs: Option<u32>,
    /// See [`MeshLimits`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undelivered_notice_secs: Option<u32>,
}

/// A change to the budget. Absent fields are unchanged.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MeshBudgetPatch {
    /// See [`MeshBudget`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_turns: Option<u32>,
    /// See [`MeshBudget`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threads_spawned: Option<u32>,
    /// See [`MeshBudget`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wall_clock_secs: Option<u64>,
}

/// A change to the policy. Absent fields are unchanged, so an older client never erases
/// settings it does not know.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MeshPolicyUpdate {
    /// New mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<MeshPolicyMode>,
    /// New remote switch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accept_from_other_targets: Option<bool>,
    /// Limit changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<MeshLimitsPatch>,
    /// Budget changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<MeshBudgetPatch>,
}

/// `mesh/policy`: read the policy, and change it when `update` is present (then `opId` is
/// required).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MeshPolicyParams {
    /// Operation id, required with `update`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub op_id: Option<OpId>,
    /// The change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update: Option<MeshPolicyUpdate>,
}

/// `mesh/policy` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MeshPolicyResult {
    /// The policy now in force.
    pub policy: MeshPolicy,
}

open_enum! {
    /// What `mesh/budget` does.
    pub enum MeshBudgetAction fallback Unknown {
        /// Report the budget.
        Status = "status",
        /// "Continue": grant another allowance and reset the clock.
        Continue = "continue",
    }
}

/// `mesh/budget`: read or continue an orchestration's budget. Sent to the target of the root.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MeshBudgetParams {
    /// Operation id (`continue` changes state).
    pub op_id: OpId,
    /// The orchestration root.
    pub root: Address,
    /// What to do.
    pub action: MeshBudgetAction,
}

/// How much of a budget is used.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MeshBudgetUsage {
    /// Agent-started turns so far.
    pub agent_turns: u32,
    /// Threads spawned so far.
    pub threads_spawned: u32,
    /// Seconds since a person last acted on the root.
    pub wall_clock_secs: u64,
}

/// `mesh/budget` result, and `mesh/budget/changed` (D164).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MeshBudgetResult {
    /// The root.
    pub root: Address,
    /// The current allowance.
    pub allowance: MeshBudget,
    /// What is used of it.
    pub used: MeshBudgetUsage,
    /// Whether it has run out (messages are stored but not dispatched).
    pub exhausted: bool,
    /// Messages of this root stored but held back by the budget: the agent-started turns
    /// waiting for **Continue**. Absent from an older target (then `exhausted` alone says it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held: Option<u32>,
}

/// `mesh/usage` (target → hub → the root's target): what this target has spent against an
/// orchestration root that lives on another target. Counts are **cumulative** for this target
/// and root, so a repeated or reordered report is harmless: the root's target keeps the
/// highest count it has seen from each target.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MeshUsageParams {
    /// The orchestration root; its target owns the budget.
    pub root: Address,
    /// The reporting target. Checked by the hub against the connection it arrived on.
    pub source: TargetId,
    /// Agent-started turns on the reporting target against this root, in total.
    #[serde(default)]
    pub agent_turns: u32,
    /// Threads spawned on the reporting target against this root, in total.
    #[serde(default)]
    pub threads_spawned: u32,
}

/// `mesh/usage` result, from the root's target.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MeshUsageResult {
    /// The root's budget has run out: dispatch nothing more for it until it is continued.
    #[serde(default)]
    pub exhausted: bool,
    /// The root's usage across all targets, when the root's target reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used: Option<MeshBudgetUsage>,
}
