//! Subscribing, reading, and acknowledging a thread's journal
//! (docs/03-protocol.md#journal-and-replay).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{
    ContextUsage, Control, Epoch, InputPart, Item, ItemBody, ItemId, JournalPosition, PageCursor, QueuedMessage, Seq, TextRole, ThreadEffective, ThreadHold, ThreadId, Timestamp, TokenUsage, TurnId,
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
    /// Upper bound on the page size in bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<u64>,
    /// Optional item limit for an interactive history window, with a stable older cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_items: Option<u32>,
    /// Open a history window at this item.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub around: Option<ItemId>,
    /// Keep turn boundaries in an interactive history page. The byte limit still applies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub whole_turns: Option<bool>,
}

/// Compact metadata for one prompt on the full-thread timeline.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TimelinePrompt {
    pub item_id: ItemId,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Timestamp>,
}

impl TimelinePrompt {
    /// Derive metadata without retaining or serializing the prompt's body.
    pub fn of(item: &Item, first_task: bool) -> Option<Self> {
        let text = match &item.body {
            ItemBody::UserMessage(message) => {
                let typed = message.parts.iter().find_map(|part| match part {
                    InputPart::Text(part) if part.role != Some(TextRole::Context) =>
                        part.text.lines().map(str::trim).find(|line| !line.is_empty()),
                    _ => None,
                });
                if typed.is_none()
                    && !message.parts.iter().any(|part| matches!(part, InputPart::Image(_) | InputPart::File(_)))
                    && message.parts.iter().any(|part| matches!(part, InputPart::Text(text) if text.role == Some(TextRole::Context)))
                {
                    return None;
                }
                typed.unwrap_or("Attachments")
            }
            ItemBody::MeshReceived(received) if first_task => &received.message.text,
            _ => return None,
        };
        let line = text.lines().map(str::trim).find(|line| !line.is_empty()).unwrap_or("");
        let line = line.trim_start_matches('#').split_whitespace().collect::<Vec<_>>().join(" ");
        let title = if line.chars().count() > 80 {
            line.chars().take(79).chain(['…']).collect()
        } else {
            line
        };
        Some(Self { item_id: item.id.clone(), title, at: item.started_at.clone().or_else(|| item.completed_at.clone()) })
    }

    pub fn index(items: &[Item], spawned: bool) -> Vec<Self> {
        let mut first_task = spawned;
        items.iter().filter_map(|item| {
            let prompt = Self::of(item, first_task);
            if matches!(&item.body, ItemBody::MeshReceived(_) | ItemBody::UserMessage(_)) {
                first_task = false;
            }
            prompt
        }).collect()
    }
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
    /// Outstanding requests outside the latest page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_items: Option<Vec<Item>>,
    /// The journal position the snapshot is valid at. Same on every page of one snapshot.
    pub position: JournalPosition,
    /// Cursor for the next (older) page; absent on the last page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<PageCursor>,
    /// Cursor for the adjacent newer page in an interactive window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub newer_cursor: Option<PageCursor>,
    /// The byte limit cut a turn at the requested edge. Read the adjacent page before showing it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continues_turn: Option<bool>,
    /// Full timeline metadata, oldest first. Sent on the first interactive page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompts: Option<Vec<TimelinePrompt>>,
    /// Presence of the attachments referenced by the items on this page.
    #[serde(default)]
    pub uploads: Vec<UploadPresence>,
    /// Context window usage at the snapshot's position, when known. On the first page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<ContextUsage>,
}

/// An interactive page from a cached transcript, in the same direction and order as the target.
pub fn cached_history_page(items: &[Item], p: &ThreadReadParams) -> Result<CachedHistoryPage, &'static str> {
    let cursor = p.cursor.as_ref().map(PageCursor::as_str);
    let newer = cursor.and_then(|c| c.strip_prefix("history-newer:"));
    let older = cursor.and_then(|c| c.strip_prefix("history:"));
    let anchor = p.around.as_ref().map(ItemId::as_str).or(newer).or(older);
    let offset = match anchor {
        Some(id) => items.iter().position(|item| item.id.as_str() == id).ok_or("the message is not cached")?,
        None => items.len(),
    };
    let forward = newer.is_some() || p.around.is_some();
    let mut start = offset + usize::from(newer.is_some() && p.around.is_none());
    if p.whole_turns == Some(true) && p.around.is_some() {
        while start > 0 && items[start - 1].turn_id == items[offset].turn_id {
            start -= 1;
        }
    }
    let mut out: Vec<Item> = Vec::new();
    let mut continues_turn = false;
    let mut bytes = 0;
    let limit = p.max_bytes.unwrap_or(64 * 1024).clamp(1024, 64 * 1024);
    let max = p.max_items.unwrap_or(200).clamp(1, 200) as usize;
    let source: Box<dyn Iterator<Item = &Item>> = if forward {
        Box::new(items[start..].iter())
    } else {
        Box::new(items[..start].iter().rev())
    };
    for item in source {
        let same_turn = p.whole_turns == Some(true) && out.last().is_some_and(|last| last.turn_id == item.turn_id);
        if out.len() >= max && !same_turn { break; }
        let len = serde_json::to_vec(item).map_or(0, |v| v.len() as u64);
        if !out.is_empty() && bytes + len > limit {
            continues_turn = same_turn;
            break;
        }
        bytes += len;
        out.push(item.clone());
    }
    if forward { out.reverse(); }
    let (first, end) = if forward { (start, start + out.len()) } else { (start - out.len(), start) };
    let older = out.last().filter(|_| first > 0).map(|item| PageCursor::new(format!("history:{}", item.id)));
    let newer = out.first().filter(|_| end < items.len()).map(|item| PageCursor::new(format!("history-newer:{}", item.id)));
    Ok(CachedHistoryPage { items: out, older, newer, continues_turn })
}

/// A page sliced from a cached interactive history window.
#[derive(Debug)]
pub struct CachedHistoryPage {
    /// Items, newest first.
    pub items: Vec<Item>,
    /// Cursor immediately before this page.
    pub older: Option<PageCursor>,
    /// Cursor immediately after this page.
    pub newer: Option<PageCursor>,
    /// The last item in the requested direction belongs to an incomplete turn.
    pub continues_turn: bool,
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
