//! Live thread events (`thread/event`), with journal positions (docs/03-protocol.md#events).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{
    Control, Epoch, Item, ItemId, QueuedMessage, RequestId, RequestResolution, RequestState, Seq,
    ThreadEffective, ThreadHold, ThreadId, Timestamp, TurnId,
};
use crate::macros::{open_enum, open_union};

open_enum! {
    /// How a turn ended.
    pub enum TurnStatus fallback Unknown {
        /// Finished normally.
        Completed = "completed",
        /// Stopped by `turn/interrupt` or the harness.
        Interrupted = "interrupted",
        /// Interrupted by a daemon update ("Update now"). Never silent.
        InterruptedByUpdate = "interruptedByUpdate",
        /// Failed.
        Failed = "failed",
    }
}

open_enum! {
    /// Who started a turn.
    pub enum TurnOrigin fallback Unknown {
        /// A person, through a YAAC client.
        User = "user",
        /// A message from another agent (the mesh dispatcher).
        Mesh = "mesh",
        /// Outside YAAC (a terminal, another app).
        External = "external",
    }
}

open_enum! {
    /// The item field an `itemDelta` appends to.
    pub enum DeltaField fallback Unknown {
        /// `agentMessage.text`.
        Text = "text",
        /// `reasoning.reasoning`.
        Reasoning = "reasoning",
        /// `toolCall.output`.
        Output = "output",
    }
}

/// Token counts. Absent counts are not reported by the harness.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsage {
    /// Input tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<u64>,
    /// Of which served from cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_input: Option<u64>,
    /// Output tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<u64>,
    /// Of which reasoning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<u64>,
    /// Total.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
}

/// How full the context window is.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ContextUsage {
    /// Tokens in use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used: Option<u64>,
    /// Window size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<u64>,
}

/// `turnStarted`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TurnStartedEvent {
    /// The turn.
    pub turn_id: TurnId,
    /// Who started it.
    pub origin: TurnOrigin,
    /// When.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Timestamp>,
}

/// `turnCompleted`: carries status and token usage.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TurnCompletedEvent {
    /// The turn.
    pub turn_id: TurnId,
    /// How it ended.
    pub status: TurnStatus,
    /// Token usage of the turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<TokenUsage>,
    /// Error text, for a failed turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// When.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Timestamp>,
}

/// `itemStarted` / `itemCompleted`: an item appeared, or reached a final state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ItemEvent {
    /// The item, in full.
    pub item: Item,
}

/// `itemDelta`: text to append to one field of an item.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ItemDeltaEvent {
    /// The item.
    pub item_id: ItemId,
    /// The field.
    pub field: DeltaField,
    /// Text to append.
    pub delta: String,
}

/// `itemUpdated`: a full replacement snapshot of an item.
///
/// With `throughSeq`, this one event replaces every event from the envelope's `seq` through
/// `throughSeq` (journal compaction, or backpressure collapse); the client advances its
/// position to `throughSeq`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ItemUpdatedEvent {
    /// The item, in full.
    pub item: Item,
    /// The last sequence number this snapshot replaces.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub through_seq: Option<Seq>,
}

/// `requestChanged`: a request moved between states.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RequestChangedEvent {
    /// The request item.
    pub item_id: ItemId,
    /// The request.
    pub request_id: RequestId,
    /// The new state.
    pub state: RequestState,
    /// How it was resolved, once `resolved`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<RequestResolution>,
}

/// `controlChanged`: who may act on the thread changed (relative to the receiving client).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ControlChangedEvent {
    /// The new control state.
    pub control: Control,
    /// Where it is in use, for the read-only states.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub holder: Option<String>,
    /// When `control` is `external` and the holder will not let YAAC take the thread over now
    /// (D140), why — shown on a disabled **Take over**. Absent: it can be taken over (under
    /// `threadTakeOver`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub take_over_blocked: Option<String>,
    /// The running turn, when there is one (needed for Stop).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_turn: Option<TurnId>,
}

/// `settingsChanged`: what the harness actually applied changed; carries the recomputed
/// effective capabilities (which include the applied settings).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SettingsChangedEvent {
    /// The thread's effective capabilities and applied settings, now.
    pub effective: ThreadEffective,
}

/// `usageUpdated`: token and context usage.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UsageUpdatedEvent {
    /// Cumulative token usage of the thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<TokenUsage>,
    /// Context window usage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<ContextUsage>,
}

open_enum! {
    /// What happened to a queued user message.
    pub enum QueueChange fallback Unknown {
        /// Queued.
        Added = "added",
        /// Edited.
        Updated = "updated",
        /// Cancelled (or dropped with its thread).
        Removed = "removed",
        /// Delivered: its turn started (`turnId`); it now appears as a user message.
        Delivered = "delivered",
    }
}

/// `queueChanged`: the thread's queue of user messages changed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct QueueChangedEvent {
    /// What happened.
    pub change: QueueChange,
    /// The message as it is now.
    pub entry: QueuedMessage,
    /// For `delivered`: the turn it started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
}

/// `holdChanged`: the thread was stopped and held, or released.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HoldChangedEvent {
    /// The hold; absent when released.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hold: Option<ThreadHold>,
}

open_union! {
    /// A thread event.
    pub enum ThreadEvent tag "type" {
        /// A turn began.
        TurnStarted(TurnStartedEvent) = "turnStarted",
        /// A turn ended.
        TurnCompleted(TurnCompletedEvent) = "turnCompleted",
        /// An item appeared.
        ItemStarted(ItemEvent) = "itemStarted",
        /// An item reached a final state.
        ItemCompleted(ItemEvent) = "itemCompleted",
        /// Text appended to an item.
        ItemDelta(ItemDeltaEvent) = "itemDelta",
        /// An item replaced in full.
        ItemUpdated(ItemUpdatedEvent) = "itemUpdated",
        /// A request changed state.
        RequestChanged(RequestChangedEvent) = "requestChanged",
        /// Control changed.
        ControlChanged(ControlChangedEvent) = "controlChanged",
        /// Applied settings / effective capabilities changed.
        SettingsChanged(SettingsChangedEvent) = "settingsChanged",
        /// Usage changed.
        UsageUpdated(UsageUpdatedEvent) = "usageUpdated",
        /// The queue of user messages changed.
        QueueChanged(QueueChangedEvent) = "queueChanged",
        /// The thread was held or released.
        HoldChanged(HoldChangedEvent) = "holdChanged",
    }
}

/// `thread/event` notification: one journaled event, with its position.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThreadEventParams {
    /// The thread.
    pub thread_id: ThreadId,
    /// The journal.
    pub epoch: Epoch,
    /// The event's sequence number (the first one it covers, for a compacted `itemUpdated`).
    pub seq: Seq,
    /// The event.
    pub event: ThreadEvent,
}

impl ThreadEventParams {
    /// The position a client has reached after applying this event: `throughSeq` for a
    /// snapshot that replaces a range, `seq` otherwise.
    pub fn position_after(&self) -> super::JournalPosition {
        let seq = match &self.event {
            ThreadEvent::ItemUpdated(ItemUpdatedEvent { through_seq: Some(through), .. }) => {
                (*through).max(self.seq)
            }
            _ => self.seq,
        };
        super::JournalPosition { epoch: self.epoch, seq }
    }
}
