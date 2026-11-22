//! Subscribing, reading, and acknowledging a thread's journal
//! (docs/03-protocol.md#journal-and-replay).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{
    ContextUsage, Control, Epoch, Item, ItemId, JournalPosition, PageCursor, QueuedMessage, Seq, ThreadEffective, ThreadHold, ThreadId, TokenUsage, TurnId,
    UploadId,
};
use crate::macros::open_union;

/// `thread/subscribe`: start live events for a thread, replaying from a position.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThreadSubscribeParams {
    /// The thread.
    pub thread_id: ThreadId,
    /// The client's journal epoch; absent when it has no position.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epoch: Option<Epoch>,
    /// The last sequence number the client has durably applied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_seq: Option<Seq>,
}

/// Replay: events `fromSeq ..= headSeq` follow, then live events.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SubscribeReplay {
    /// The current epoch (the client's).
    pub epoch: Epoch,
    /// First event that will be replayed.
    pub from_seq: Seq,
    /// Last event in the journal now.
    pub head_seq: Seq,
}

/// Reset: rebuild from `thread/read`, then live events strictly after its snapshot position.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SubscribeReset {
    /// The current epoch.
    pub epoch: Epoch,
    /// Last event in the journal now.
    pub head_seq: Seq,
}

open_union! {
    /// How the subscription resumes. A mode this build does not know must be handled as a
    /// reset.
    pub enum SubscribeMode tag "mode" {
        /// The normal path after any disconnect.
        Replay(SubscribeReplay) = "replay",
        /// The fallback: no position, old epoch, or position before the journal's start.
        Reset(SubscribeReset) = "reset",
    }
}

/// `thread/subscribe` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThreadSubscribeResult {
    /// Replay or reset (`mode` and its fields).
    #[serde(flatten)]
    pub mode: SubscribeMode,
    /// Who may act on the thread, relative to this client.
    pub control: Control,
    /// Where it is in use, for the read-only states.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_holder: Option<String>,
    /// When `control` is `external` and the holder will not let YAAC take the thread over now
    /// (D140), why — shown on a disabled **Take over**. Absent: it can be taken over (under
    /// `threadTakeOver`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub take_over_blocked: Option<String>,
    /// The running turn, when there is one (needed for Stop and steer).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_turn: Option<TurnId>,
    /// The thread's effective capabilities and applied settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective: Option<ThreadEffective>,
    /// How full the context window is now, when known (under `contextUsage`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<ContextUsage>,
    /// Cumulative token usage of the thread, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<TokenUsage>,
    /// User messages queued behind the running turn, in delivery order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queued: Option<Vec<QueuedMessage>>,
    /// Set while the thread is stopped and held.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hold: Option<ThreadHold>,
}

/// `thread/unsubscribe`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThreadUnsubscribeParams {
    /// The thread.
    pub thread_id: ThreadId,
}

/// `thread/ack`: how far this client has durably got. Sent periodically while attached.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThreadAckParams {
    /// The thread.
    pub thread_id: ThreadId,
    /// The epoch.
    pub epoch: Epoch,
    /// The last sequence number durably stored by the client.
    pub seq: Seq,
}

/// `thread/read`: one page of items, newest first, from a snapshot bound to a position.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThreadReadParams {
    /// The thread.
    pub thread_id: ThreadId,
    /// Continue from a previous page; absent for the first page (which takes a new snapshot).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<PageCursor>,
    /// Upper bound on the page size in bytes. Pages are bounded by bytes, never by count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<u64>,
}

/// Whether an attachment referenced by the thread is still on the target.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UploadPresence {
    /// The upload.
    pub upload_id: UploadId,
    /// Whether its file still exists (temp folders can be emptied).
    pub present: bool,
}

/// `thread/read` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThreadReadResult {
    /// Items, newest first.
    pub items: Vec<Item>,
    /// The journal position the snapshot is valid at. Same on every page of one snapshot.
    pub position: JournalPosition,
    /// Cursor for the next (older) page; absent on the last page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<PageCursor>,
    /// Presence of the attachments referenced by the items on this page.
    #[serde(default)]
    pub uploads: Vec<UploadPresence>,
    /// Context window usage at the snapshot's position, when known. On the first page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<ContextUsage>,
}

/// `item/read`: a byte range of one item's full output. A tool call's inline output is a
/// bounded head (`outputTruncated`); the target keeps the rest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ItemReadParams {
    /// The thread.
    pub thread_id: ThreadId,
    /// The item.
    pub item_id: ItemId,
    /// Byte offset into the full output.
    #[serde(default)]
    pub offset: u64,
    /// Upper bound on bytes returned. The daemon may return fewer.
    pub max_bytes: u64,
}

/// `item/read` result. Sent in the bulk class.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ItemReadResult {
    /// Whether the full output is still on the target. When false, the other fields are empty.
    pub present: bool,
    /// The text, UTF-8. A range never splits a character: it may end a little early.
    #[serde(default)]
    pub data: String,
    /// The offset `data` starts at.
    #[serde(default)]
    pub offset: u64,
    /// The offset to ask for next.
    #[serde(default)]
    pub next_offset: u64,
    /// Total size of the full output in bytes.
    #[serde(default)]
    pub total_bytes: u64,
    /// Whether bytes remain after this range.
    #[serde(default)]
    pub more: bool,
}
