//! The item model: every provider's output normalised into one set of kinds
//! (docs/03-protocol.md#the-item-model). Unknown kinds round-trip and render generically.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{
    Address, ForkKind, InputPart, ItemId, MeshMessage, ModelId, Receipt, RequestBody, RequestId,
    RequestResolution, RequestState, TargetPath, ThreadId, ThreadState, TurnId,
};
use crate::macros::{open_enum, open_union};

open_enum! {
    /// Item status.
    pub enum ItemStatus fallback Unknown {
        /// Still being produced.
        InProgress = "inProgress",
        /// Finished.
        Completed = "completed",
        /// Failed.
        Failed = "failed",
        /// Declined (for example a tool call the user denied).
        Declined = "declined",
    }
}

/// `userMessage`: the input parts as sent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UserMessageItem {
    /// The parts. For a message sent from outside YAAC, the text as the harness stored it.
    pub parts: Vec<InputPart>,
}

/// `agentMessage`: markdown text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentMessageItem {
    /// Markdown. `itemDelta` with field `text` appends here.
    pub text: String,
}

/// `reasoning`: reasoning text or summary, when the provider exposes it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReasoningItem {
    /// The text. `itemDelta` with field `reasoning` appends here.
    pub reasoning: String,
    /// Whether this is a summary rather than the raw reasoning.
    #[serde(default)]
    pub summary: bool,
}

open_enum! {
    /// What kind of tool a tool call is.
    pub enum ToolKind fallback Unknown {
        /// A shell command.
        Command = "command",
        /// A file change.
        FileChange = "fileChange",
        /// An MCP tool.
        Mcp = "mcp",
        /// A web search.
        WebSearch = "webSearch",
        /// A sub-agent of the harness's own.
        Subagent = "subagent",
        /// Anything else. Unmappable output becomes this, never dropped.
        Other = "other",
    }
}

/// `toolCall`: a tool invocation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallItem {
    /// What kind of tool.
    pub tool: ToolKind,
    /// One-line title.
    pub title: String,
    /// The tool's input, as structured data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<Value>,
    /// Output text. `itemDelta` with field `output` appends here. Bounded: a large output is
    /// a head, with `outputTruncated` set.
    #[serde(default)]
    pub output: String,
    /// The output above is a bounded head of a longer one.
    #[serde(default)]
    pub output_truncated: bool,
    /// Full output size in bytes, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_bytes: Option<u64>,
    /// Exit status, for a command.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// Working directory, for a command.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<TargetPath>,
}

open_enum! {
    /// State of a plan step.
    pub enum PlanStepState fallback Unknown {
        /// Not started.
        Pending = "pending",
        /// Under way.
        InProgress = "inProgress",
        /// Done.
        Completed = "completed",
    }
}

/// One step of a plan.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PlanStep {
    /// What the step is.
    pub text: String,
    /// Its state.
    pub state: PlanStepState,
}

/// `plan`: the agent's task list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PlanItem {
    /// Steps, in order.
    pub steps: Vec<PlanStep>,
    /// The agent's explanation, when given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub explanation: Option<String>,
}

/// `request`: something awaiting the user. Journaled before anything else happens to it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RequestItem {
    /// The request, for `request/respond`.
    pub request_id: RequestId,
    /// Whether the agent is blocked until it is answered.
    #[serde(default)]
    pub blocking: bool,
    /// Its state.
    pub state: RequestState,
    /// How it was resolved, once `resolved`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<RequestResolution>,
    /// What it asks for (`requestKind` and its fields).
    #[serde(flatten)]
    pub body: RequestBody,
}

open_enum! {
    /// What a notice is about.
    pub enum NoticeKind fallback Unknown {
        /// An error.
        Error = "error",
        /// A warning.
        Warning = "warning",
        /// Information.
        Info = "info",
        /// The context was compacted.
        Compaction = "compaction",
        /// The harness used a different model than requested.
        ModelRerouted = "modelRerouted",
        /// A rate-limit warning.
        RateLimit = "rateLimit",
        /// The turn was interrupted by a daemon update ("Update now").
        InterruptedByUpdate = "interruptedByUpdate",
        /// An orchestration's mesh budget ran out; `mesh/budget` continues it.
        BudgetExhausted = "budgetExhausted",
        /// A mesh limit was hit.
        MeshLimit = "meshLimit",
        /// A worker spawned beneath this thread needs attention.
        WorkerNeedsAttention = "workerNeedsAttention",
    }
}

/// `notice`: out-of-band information.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct NoticeItem {
    /// What it is about.
    pub notice_kind: NoticeKind,
    /// Text to show as-is.
    pub text: String,
    /// The thread it concerns, when it is about another agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub about: Option<Address>,
    /// Structured detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

/// `handoff`: marks history a forked thread inherited from its parent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HandoffItem {
    /// The thread forked from.
    pub forked_from: ThreadId,
    /// How the history was carried.
    pub fork_kind: ForkKind,
    /// The last turn of the source that was carried.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cutoff_turn: Option<TurnId>,
    /// The frozen transcript (markdown), for a handoff fork; rendered read-only above the
    /// divider. Bounded like any item field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript: Option<String>,
    /// The transcript file on the target, in `yaacd`'s state directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_path: Option<TargetPath>,
}

/// A readable label for another agent: "title · provider · target".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MeshPeer {
    /// Its address. Opaque.
    pub address: Address,
    /// Its title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Provider display name ("Codex · work").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelId>,
    /// Target name, as the user named it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_name: Option<String>,
    /// Whether its target is currently reachable. Absent: not known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reachable: Option<bool>,
}

/// `meshSent`: a message this agent sent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MeshSentItem {
    /// The message.
    pub message: MeshMessage,
    /// The recipient, labelled.
    pub to: MeshPeer,
    /// How far it got. Updated (`itemUpdated`) as it moves.
    pub receipt: Receipt,
}

/// `meshReceived`: a message this agent received. Never styled as a user message.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MeshReceivedItem {
    /// The message.
    pub message: MeshMessage,
    /// The sender, labelled.
    pub from: MeshPeer,
}

/// One thread started by a spawn.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SpawnedThread {
    /// The new thread.
    pub peer: MeshPeer,
    /// Its live state.
    pub state: ThreadState,
    /// Why it could not be started, when it could not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// `meshSpawn`: threads this agent started.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MeshSpawnItem {
    /// The threads, with live states.
    pub threads: Vec<SpawnedThread>,
}

/// One thread a `wait` is watching.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WaitTarget {
    /// The thread.
    pub peer: MeshPeer,
    /// Its live state.
    pub state: ThreadState,
    /// Whether it has finished its turn (or failed, or needs attention) — what the wait is for.
    #[serde(default)]
    pub finished: bool,
}

/// `meshWait`: a wait on other agents.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MeshWaitItem {
    /// The threads waited on.
    #[serde(default)]
    pub targets: Vec<WaitTarget>,
    /// Whether any incoming message also ends the wait.
    #[serde(default)]
    pub for_messages: bool,
}

open_union! {
    /// The kind-specific part of an item.
    pub enum ItemBody tag "kind" {
        /// The input parts as sent.
        UserMessage(UserMessageItem) = "userMessage",
        /// Markdown text.
        AgentMessage(AgentMessageItem) = "agentMessage",
        /// Reasoning text or summary.
        Reasoning(ReasoningItem) = "reasoning",
        /// A tool invocation.
        ToolCall(ToolCallItem) = "toolCall",
        /// The agent's task list.
        Plan(PlanItem) = "plan",
        /// An approval, question, or form.
        Request(RequestItem) = "request",
        /// Out-of-band information.
        Notice(NoticeItem) = "notice",
        /// History inherited by a fork.
        Handoff(HandoffItem) = "handoff",
        /// A message sent to another agent.
        MeshSent(MeshSentItem) = "meshSent",
        /// A message received from another agent.
        MeshReceived(MeshReceivedItem) = "meshReceived",
        /// Threads this agent started.
        MeshSpawn(MeshSpawnItem) = "meshSpawn",
        /// A wait on other agents.
        MeshWait(MeshWaitItem) = "meshWait",
    }
}

/// One item of a turn: common fields plus the kind-specific body (flattened, so `kind` sits
/// next to `id` on the wire).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    /// The item.
    pub id: ItemId,
    /// The turn it belongs to.
    pub turn_id: TurnId,
    /// Its status.
    pub status: ItemStatus,
    /// `kind` and the kind-specific fields.
    #[serde(flatten)]
    pub body: ItemBody,
}

impl Item {
    /// The `kind` string, including for kinds this build does not know.
    pub fn kind(&self) -> &str {
        self.body.tag()
    }
}
