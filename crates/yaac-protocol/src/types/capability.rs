//! Capabilities: what an instance, a model, or a thread can do. On the wire a capability is a
//! plain string; one this build does not know is kept as [`Capability::Unknown`].

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::macros::open_enum;

open_enum! {
    /// A flag the UI checks before offering a feature. Branch on these, never on provider kind.
    pub enum Capability fallback Unknown {
        // Thread lifecycle
        /// Can enumerate existing conversations on the target.
        ThreadList = "threadList",
        /// Can read a conversation's history back.
        ThreadRead = "threadRead",
        /// Can continue a conversation from an earlier run.
        ThreadResume = "threadResume",
        /// Native fork within this provider.
        ThreadFork = "threadFork",
        /// Native fork from a chosen point, not only the end.
        ThreadForkAtTurn = "threadForkAtTurn",
        /// Can rename a thread.
        ThreadRename = "threadRename",
        /// Can archive a thread.
        ThreadArchive = "threadArchive",
        /// The harness can delete its own sessions: `thread/delete` may be asked to delete the
        /// session there too (`alsoInHarness`, D110). Without it YAAC removes only its record.
        ThreadDelete = "threadDelete",
        /// A thread another program holds can be taken over (`thread/takeOver`, D140): YAAC
        /// continues it here without stopping the other program.
        ThreadTakeOver = "threadTakeOver",
        /// `yaacd` can list this instance's threads under another project or worktree on the
        /// same target (`thread/move`, D144). YAAC's own placement; says nothing about the
        /// harness.
        ThreadMove = "threadMove",
        /// The harness can change a conversation's working directory from its next turn: a
        /// `thread/move` then also moves where the agent works (Codex: `cwd` on `turn/start`
        /// and `thread/resume`). Without it a move changes only where YAAC lists the thread.
        ThreadMoveWorkingDirectory = "threadMoveWorkingDirectory",
        /// Can show live updates of a thread someone else is driving.
        FollowExternal = "followExternal",
        /// The harness can be told, through an interface it supports, that a folder is trusted
        /// (Codex: `projects."<folder>".trust_level` via its app-server's `config/value/write`).
        /// `yaacd` then marks the folders of the threads YAAC starts in a user's project, so the
        /// harness's own interface does not ask about them again (D147), unless the instance's
        /// `trustFolders` is off. Without it the harness keeps its own behaviour.
        FolderTrust = "folderTrust",
        // Turn control
        /// Incremental agent text.
        StreamText = "streamText",
        /// Incremental reasoning text or summaries.
        StreamReasoning = "streamReasoning",
        /// Stop a running turn.
        Interrupt = "interrupt",
        /// Add input to a running turn.
        Steer = "steer",
        /// Manual context compaction.
        Compact = "compact",
        /// Drop later turns from the conversation.
        RewindConversation = "rewindConversation",
        /// Restore files to an earlier point — a different act entirely.
        RewindFiles = "rewindFiles",
        // Asking the user
        /// Asks before running commands or changing files.
        Approvals = "approvals",
        /// Can ask the user structured questions.
        UserQuestions = "userQuestions",
        /// Can ask for typed fields (schema-driven forms).
        UserForms = "userForms",
        /// Offers selectable permission / approval / sandbox modes.
        Modes = "modes",
        // Mid-thread switching
        /// Change model within a thread.
        ModelSwitch = "modelSwitch",
        /// Change effort within a thread.
        EffortSwitch = "effortSwitch",
        // Input (model-level)
        /// The model can see images.
        ImageInput = "imageInput",
        /// The model accepts documents (PDF) as native input.
        DocumentInput = "documentInput",
        /// The model accepts audio input.
        AudioInput = "audioInput",
        // Reasoning (model-level)
        /// The model has selectable effort levels.
        Reasoning = "reasoning",
        /// Reasoning is visible to the user in some form.
        ReasoningSummary = "reasoningSummary",
        // Speed (model-level)
        /// Fast mode (D125): the harness can run this model's turns faster, at a higher cost.
        /// Reported per model from what the harness lists; a model without it, or with it
        /// unknown, is never offered fast mode. In a thread's effective capabilities: the
        /// thread's current model has it.
        FastMode = "fastMode",
        // Reporting
        /// Reports token usage.
        TokenUsage = "tokenUsage",
        /// Reports how full the context window is.
        ContextUsage = "contextUsage",
        /// Reports rate limits.
        RateLimits = "rateLimits",
        /// Can enumerate models.
        ModelList = "modelList",
        // Working with other agents
        /// Can be given the `yaac` tools: message, spawn, and wait for other agents.
        MeshTools = "meshTools",
        // Agent features surfaced in the UI
        /// Task list updates.
        Plan = "plan",
        /// Offers slash commands.
        SlashCommands = "slashCommands",
        /// Runs sub-agents of its own.
        Subagents = "subagents",
        /// Lists the sub-agents it runs itself as threads of their own, each linked to the
        /// thread that started it (`ThreadSummary.subagent` and `spawnedBy`, D151): a Claude
        /// Code subagent, a Codex sub-agent.
        SubagentThreads = "subagentThreads",
        /// Can search the web.
        WebSearch = "webSearch",
        /// Per-turn diff of file changes.
        FileDiffs = "fileDiffs",
    }
}

open_enum! {
    /// Three-valued support. `unknown` is a real answer — the harness gave no information —
    /// and is never treated as "yes".
    pub enum Support fallback Other {
        /// Known to work.
        Supported = "supported",
        /// Known not to work.
        Unsupported = "unsupported",
        /// The harness gave no information.
        Unknown = "unknown",
    }
}

impl Support {
    /// Whether this is a definite yes. A value this build does not know counts as unknown.
    pub fn is_supported(&self) -> bool {
        matches!(self, Support::Supported)
    }

    /// Whether the answer is unknown (including a value this build does not know).
    pub fn is_unknown(&self) -> bool {
        matches!(self, Support::Unknown | Support::Other(_))
    }
}

/// A model-level capability with its three-valued support.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ModelCapability {
    /// The capability.
    pub capability: Capability,
    /// Whether this model has it.
    pub support: Support,
}

/// Whether `capabilities` contains `wanted`.
pub fn has_capability(capabilities: &[Capability], wanted: &Capability) -> bool {
    capabilities.iter().any(|c| c == wanted)
}
