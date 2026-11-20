//! What the window knows about a thread beyond its summary, learned from its journal events:
//! open requests, waits on other agents, and sent messages still queued. Small, kept for every
//! thread whose events the window has seen, whether or not its transcript is held.

use std::collections::BTreeMap;

use yaac_protocol::{
    Address, Item, ItemBody, ItemId, ItemStatus, JournalPosition, MeshPeer, Receipt, ReceiptStatus, RequestBody,
    RequestState, Seq, ThreadEvent, ThreadEventParams, TurnId,
};

/// What a request asks for, as one word for lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RequestKindWord {
    /// An approval.
    Approval,
    /// A question.
    Question,
    /// A form.
    Form,
    /// Permission to work with other agents (a seam: see [`crate::permission`]).
    Permission,
    /// A kind this build does not know.
    Other,
}

impl RequestKindWord {
    /// From a request body.
    pub fn of(body: &RequestBody) -> Self {
        match body {
            RequestBody::Approval(_) => Self::Approval,
            RequestBody::MeshPermission(_) => Self::Permission,
            RequestBody::Question(_) => Self::Question,
            RequestBody::Form(_) => Self::Form,
            RequestBody::Unknown(_) if crate::permission::parse(body).is_some() => Self::Permission,
            RequestBody::Unknown(_) => Self::Other,
        }
    }

    /// "approval", "question", …
    pub fn word(self) -> &'static str {
        match self {
            Self::Approval => "approval",
            Self::Question => "question",
            Self::Form => "form",
            Self::Permission => "permission",
            Self::Other => "request",
        }
    }
}

/// Whether a request in this state still holds the agent and still concerns the user.
/// `submitted` counts until the agent acknowledges it; `unknown` counts because the user must
/// check it. `resolved` and `expired` do not.
pub fn request_is_open(state: &RequestState) -> bool {
    matches!(state, RequestState::Pending | RequestState::Submitted | RequestState::Unknown)
}

/// A request item, as hinted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestHint {
    /// What it asks.
    pub kind: RequestKindWord,
    /// Its state.
    pub state: RequestState,
    /// The turn it belongs to.
    pub turn: TurnId,
}

/// A message this thread sent, while it is not delivered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SentHint {
    /// The recipient.
    pub to: MeshPeer,
    /// The receipt.
    pub receipt: Receipt,
}

/// Per-thread hints.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Hints {
    /// Ordering stamp: newer than the summary's when the hints were updated after it.
    pub stamp: u64,
    /// Last applied journal position.
    pub position: Option<JournalPosition>,
    /// Events up to here are a replay of the past (no arrival notices).
    pub replay_until: Option<Seq>,
    /// Every request item seen.
    pub requests: BTreeMap<ItemId, RequestHint>,
    /// `meshWait` items still in progress, with the threads each waits on (empty: messages).
    pub waits: BTreeMap<ItemId, Vec<Address>>,
    /// Sent messages that are queued or failed.
    pub sent: BTreeMap<ItemId, SentHint>,
    /// The running turn, when seen.
    pub running_turn: Option<TurnId>,
}

/// What applying an event to the hints revealed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HintChange {
    /// The event was new and applied.
    pub applied: bool,
    /// A request newly became pending, live (not a replay).
    pub arrived: Option<RequestKindWord>,
}

impl Hints {
    /// Whether any request is open.
    pub fn open_request(&self) -> Option<RequestKindWord> {
        self.requests.values().find(|r| request_is_open(&r.state)).map(|r| r.kind)
    }

    /// Whether the thread is blocked in a wait on other agents.
    pub fn waiting(&self) -> bool {
        !self.waits.is_empty()
    }

    /// The threads the open waits are on; an empty list inside means "any message".
    pub fn wait_targets(&self) -> impl Iterator<Item = &Vec<Address>> {
        self.waits.values()
    }

    /// Rebuild from a snapshot of items at a position.
    pub fn rebuild(&mut self, items: &[Item], position: JournalPosition, stamp: u64) {
        self.requests.clear();
        self.waits.clear();
        self.sent.clear();
        for item in items {
            self.observe_item(item);
        }
        self.position = Some(position);
        self.replay_until = Some(position.seq);
        self.stamp = stamp;
    }

    /// Apply one event, deduplicated by position.
    pub fn apply(&mut self, event: &ThreadEventParams, stamp: u64) -> HintChange {
        if let Some(pos) = self.position
            && (event.epoch != pos.epoch || event.position_after().seq <= pos.seq)
        {
            return HintChange::default();
        }
        let live = self.replay_until.is_none_or(|until| event.seq > until) && self.position.is_some();
        self.position = Some(event.position_after());
        let mut change = HintChange { applied: true, arrived: None };
        match &event.event {
            ThreadEvent::TurnStarted(e) => self.running_turn = Some(e.turn_id.clone()),
            ThreadEvent::TurnCompleted(e) => {
                if self.running_turn.as_ref() == Some(&e.turn_id) {
                    self.running_turn = None;
                }
            }
            ThreadEvent::ItemStarted(e) | ThreadEvent::ItemCompleted(e) => {
                let was_open = self.requests.get(&e.item.id).is_some_and(|r| request_is_open(&r.state));
                self.observe_item(&e.item);
                if live
                    && !was_open
                    && let Some(r) = self.requests.get(&e.item.id)
                    && r.state == RequestState::Pending
                {
                    change.arrived = Some(r.kind);
                }
            }
            ThreadEvent::ItemUpdated(e) => self.observe_item(&e.item),
            ThreadEvent::RequestChanged(e) => {
                if let Some(r) = self.requests.get_mut(&e.item_id) {
                    r.state = e.state.clone();
                }
            }
            ThreadEvent::ItemDelta(_)
            | ThreadEvent::ControlChanged(_)
            | ThreadEvent::SettingsChanged(_)
            | ThreadEvent::UsageUpdated(_)
            | ThreadEvent::Unknown(_) => return HintChange { applied: true, arrived: None },
        }
        self.stamp = stamp;
        change
    }

    /// Record what an item says about the thread.
    pub fn observe_item(&mut self, item: &Item) {
        match &item.body {
            ItemBody::Request(r) => {
                self.requests.insert(
                    item.id.clone(),
                    RequestHint { kind: RequestKindWord::of(&r.body), state: r.state.clone(), turn: item.turn_id.clone() },
                );
            }
            ItemBody::MeshWait(w) => {
                if item.status == ItemStatus::InProgress {
                    self.waits.insert(item.id.clone(), w.targets.iter().map(|t| t.peer.address.clone()).collect());
                } else {
                    self.waits.remove(&item.id);
                }
            }
            ItemBody::MeshSent(s) => {
                if s.receipt.status == ReceiptStatus::Delivered {
                    self.sent.remove(&item.id);
                } else {
                    self.sent.insert(item.id.clone(), SentHint { to: s.to.clone(), receipt: s.receipt.clone() });
                }
            }
            _ => {}
        }
    }
}
