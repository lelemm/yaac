//! The item model: every provider's output normalised into one set of kinds
//! (docs/03-protocol.md#the-item-model). Unknown kinds round-trip and render generically.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{
    Address, ForkKind, InputPart, ItemId, MeshMessage, MessageId, ModelId, OpId, Receipt, RequestBody, RequestId,
    RequestResolution, RequestState, TargetPath, ThreadId, ThreadState, Timestamp, TurnId, UploadId,
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
    /// The operation id of the `thread/start` or `turn/start` that sent it (a queued message
    /// keeps the `turn/start` that queued it), when a client sent it through `yaacd`: the
    /// window replaces its "Sending…" echo of that operation with this item, in place (D165).
    /// Absent for anything else, and from an older `yaacd`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub op_id: Option<OpId>,
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

open_enum! {
    /// Who ran a tool call, when it was not the agent.
    pub enum ToolCallSource fallback Unknown {
        /// The user ran it as `!command` in the composer (P344): shown as a command block, not
        /// as one of the agent's tool calls.
        UserShell = "userShell",
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
    /// The call is communication with another agent (D150): what it did, to whom, and how it
    /// went, read from the call by `yaacd`. Absent for every other tool. A window that does
    /// not know this field shows the tool call as an ordinary one, under its readable title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_call: Option<AgentCall>,
    /// The call loaded a skill's instructions, and the harness confirmed it: set by `yaacd`
    /// only on a completed call (Claude Code's `Skill` tool result; for Codex, a successful read
    /// of a `SKILL.md` it listed). The title then reads "Skill loaded" or "Read skill
    /// instructions". Absent for every other tool, and for a call that failed or is not done.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill: Option<SkillLoad>,
    /// Image results stored on this target and read on demand through `upload/read`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ToolImage>,
    /// Who ran it, when not the agent (`userShell`: a `!command`, P344). Absent: the agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<ToolCallSource>,
    /// How long it ran, in milliseconds, when the harness says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

/// A tool image result, with its bytes kept out of the transcript stream.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ToolImage {
    /// Identifies the stored image on this target.
    pub upload_id: UploadId,
    /// Display label.
    pub name: String,
}

/// A skill whose instructions a tool call loaded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SkillLoad {
    /// The skill's name.
    pub name: String,
}

open_enum! {
    /// What an agent call does.
    pub enum AgentCallKind fallback Unknown {
        /// Starts another agent (a YAAC worker, or a sub-agent of the harness's own).
        Spawn = "spawn",
        /// Sends a message (or a follow-up task) to another agent.
        Message = "message",
        /// Resumes another agent that had stopped.
        Resume = "resume",
        /// Waits for other agents, or for messages.
        Wait = "wait",
        /// Reads another agent's transcript without interrupting it.
        Read = "read",
        /// Stops (or interrupts, or closes) other agents.
        Stop = "stop",
        /// Registers that another agent hears when this turn ends.
        Notify = "notify",
        /// Lists or finds agents.
        List = "list",
    }
}

open_enum! {
    /// How an agent call went, as its result says.
    pub enum AgentCallOutcome fallback Unknown {
        /// The message reached the recipient.
        Delivered = "delivered",
        /// The message is stored and will be delivered later (`detail` says why).
        Queued = "queued",
        /// The call failed (`detail` says why).
        Failed = "failed",
        /// The agent was started (and runs on its own).
        Started = "started",
        /// The agent was started, ran, and its answer is `result`.
        Completed = "completed",
        /// A stopped agent was resumed to take the message.
        Resumed = "resumed",
        /// The agents were stopped.
        Stopped = "stopped",
        /// The agent was not running.
        NotRunning = "notRunning",
        /// A wait returned something: messages, results, or finished agents (`result`).
        Returned = "returned",
        /// A wait ran out without anything happening.
        NothingYet = "nothingYet",
        /// `notify_when_done` was registered.
        Registered = "registered",
    }
}

/// The other side of an agent call. At least one of `address`, `agentId`, `name`, or
/// `parent` is set.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentRef {
    /// A YAAC address (a mesh peer). Opaque.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<Address>,
    /// The harness's own id for one of its sub-agents (Claude Code's `agentId`, Codex's
    /// receiver thread id). Resolved by the window against the calling thread's children.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// A name the call gave it (a sub-agent's description or name, a task name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// It is the calling agent's own parent (a sub-agent writing to the conversation that
    /// started it).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub parent: bool,
}

/// Communication with another agent, read from a tool call (D150).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentCall {
    /// What it does.
    pub kind: AgentCallKind,
    /// The other agents, in the call's order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub peers: Vec<AgentRef>,
    /// The message, prompt, or task, in full (markdown).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// A short title the call gave the work (a spawn's title, a sub-agent's description).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The model asked for, as the call named it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The effort asked for, as the call named it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// The spawned agent runs in the background (the caller does not block on it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<bool>,
    /// The machine asked for, by name, for a spawn elsewhere.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// How it went, once the result is in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<AgentCallOutcome>,
    /// Why, in a few words (for queued and failed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// What came back that is worth reading: a sub-agent's answer, what a wait returned, a
    /// read transcript (markdown). The raw result stays in the tool call's `output`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    /// Mesh messages this call sent or handed over (`send_message`'s message, the messages a
    /// `wait` returned): the matching `meshSent` / `meshReceived` items are this call's own.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<MessageId>,
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
        /// A message this thread sent has waited undelivered past the limit. Passive.
        Undelivered = "undelivered",
        /// The thread was stopped and is held ("Stopped by …").
        Stopped = "stopped",
        /// The thread was taken over from another program (D140).
        TakenOver = "takenOver",
        /// The thread was moved to another project or worktree (`thread/move`, D144).
        Moved = "moved",
        /// `yaacd` marked the thread's folder as trusted in the harness, or could not (D147).
        FolderTrust = "folderTrust",
        /// The harness's own record that the turn was stopped part-way (Claude Code's
        /// `[Request interrupted by user]`, Codex's `<turn_aborted>`), D167. A quiet line.
        Interrupted = "interrupted",
    }
}

/// `notice`: out-of-band information.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct NoticeItem {
    /// What it is about.
    pub notice_kind: NoticeKind,
    /// Text to show as-is: one line.
    pub text: String,
    /// The longer text behind it, shown when the notice is expanded (plain text). Absent: the
    /// one line is all there is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
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
    /// When it appeared, as `yaacd` (or the harness) recorded it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<Timestamp>,
    /// When it reached a final state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<Timestamp>,
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
