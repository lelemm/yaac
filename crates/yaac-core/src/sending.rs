//! A sent message before its own item arrives (D165): the window shows it at once, at the end
//! of the transcript, as the user's bubble in a quiet "Sending…" state, instead of waiting for
//! the round trip window → hub → yaacd → harness → item. It is an item of the transcript like
//! any other (so it is drawn through the normal block path), with an id of its own
//! ([`echo_id`]), and it carries the operation id of its send (D159). When the real
//! `userMessage` arrives with that op id it takes the echo's place — the same slot, so nothing
//! jumps and nothing is shown twice.
//!
//! A send that fails keeps its bubble with the error and **Edit** (the text and attachments
//! back in the composer) and **Retry** (a new operation id, only when the user asks — no blind
//! retries, invariant 10). An outcome YAAC cannot know says so, and is never sent again.

use serde_json::Value;
use yaac_protocol::{InputPart, Item, ItemBody, ItemId, ItemStatus, OpId, TurnId, UserMessageItem};

use crate::state::Draft;

/// Where a sent message is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SendState {
    /// On its way: "Sending…".
    Sending,
    /// `yaacd` took it; its own item has not arrived (an older `yaacd` stamps no op id, or the
    /// harness reports the message itself). Drawn as an ordinary bubble.
    Sent,
    /// Refused or not delivered; nothing was sent. The text is why.
    Failed(String),
    /// The connection was lost before the answer: it may or may not have arrived. Never sent
    /// again on its own.
    Unknown,
}

/// A message this window sent whose own item has not arrived.
#[derive(Clone, Debug, PartialEq)]
pub struct Pending {
    /// The operation id of the send now in flight (a Retry has a new one).
    pub op: OpId,
    /// The echo's item id, stable across Retries.
    pub item: ItemId,
    /// What was sent, as the bubble shows it.
    pub parts: Vec<InputPart>,
    /// The composer's content, for Edit.
    pub draft: Draft,
    /// The method and its params (with `opId`), for Retry.
    pub method: String,
    /// The request's params.
    pub params: Value,
    /// Where it is.
    pub state: SendState,
    /// The turn `yaacd` named in its answer, when it did.
    pub turn: Option<TurnId>,
}

/// The id of the echo of the send with this op id.
pub fn echo_id(op: &OpId) -> ItemId {
    ItemId::new(format!("pending:{}", op.as_str()))
}

/// Whether an item is an echo, not something the target journaled.
pub fn is_echo(id: &ItemId) -> bool {
    id.as_str().starts_with("pending:")
}

impl Pending {
    /// A send in flight.
    pub fn new(op: OpId, parts: Vec<InputPart>, draft: Draft, method: &str, params: Value) -> Self {
        Self { item: echo_id(&op), op, parts, draft, method: method.to_owned(), params, state: SendState::Sending, turn: None }
    }

    /// The echo as an item: the user's message, in a turn of its own until the real one comes.
    pub fn item(&self) -> Item {
        Item {
            id: self.item.clone(),
            turn_id: TurnId::new(self.item.as_str()),
            status: ItemStatus::InProgress,
            body: ItemBody::UserMessage(UserMessageItem { parts: self.parts.clone(), op_id: Some(self.op.clone()) }),
            started_at: None,
            completed_at: None,
        }
    }

    /// Whether `item` is this send's own message: by op id, or — from a `yaacd` that does not
    /// stamp one, or a message the harness reports itself — by the same parts in the turn the
    /// send went to, once that turn is known.
    pub fn matches(&self, item: &Item) -> bool {
        let ItemBody::UserMessage(u) = &item.body else { return false };
        if is_echo(&item.id) {
            return false;
        }
        match &u.op_id {
            Some(op) => *op == self.op,
            None => {
                matches!(self.state, SendState::Sent | SendState::Sending) && self.turn.as_ref() == Some(&item.turn_id) && u.parts == self.parts
            }
        }
    }

    /// The line under the bubble, when there is one.
    pub fn note(&self) -> Option<String> {
        match &self.state {
            SendState::Sending => Some("Sending…".to_owned()),
            SendState::Sent => None,
            SendState::Failed(why) => Some(format!("Not sent: {why}")),
            SendState::Unknown => Some("YAAC lost contact while sending this. It may or may not have arrived; it will not be sent again.".to_owned()),
        }
    }

    /// Whether Retry is offered: only for a send that certainly did not arrive.
    pub fn can_retry(&self) -> bool {
        matches!(self.state, SendState::Failed(_))
    }

    /// Whether Edit and Dismiss are offered.
    pub fn settled(&self) -> bool {
        matches!(self.state, SendState::Failed(_) | SendState::Unknown)
    }
}
