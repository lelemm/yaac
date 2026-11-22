//! One thread's items, kept in journal order, with the journal position they are valid at.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use yaac_protocol::{
    ContextUsage, Control, DeltaField, Item, ItemBody, ItemId, JournalPosition, RequestId, RequestState,
    ThreadEffective, ThreadEvent, ThreadEventParams, TurnId, TurnOrigin, UploadId,
};

/// What this window answered, kept so the card can say "You chose …" before the harness
/// confirms. Secret answers are stored masked: the clear text never enters the state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalAnswer {
    /// A one-line, already masked summary of the answer.
    pub summary: String,
    /// The decision kind word ("Allowed", "Denied", …) for approvals.
    pub verb: Option<String>,
}

/// This window's `thread/takeOver` (D140): in flight, or refused with the target's reason.
/// Never re-sent on its own; a success clears it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TakeOverRequest {
    /// Sent; no answer yet.
    InFlight,
    /// Not taken over, and why.
    Failed(String),
}

/// A reset in progress: `thread/read` pages being collected, and live events held back until
/// the snapshot is complete (they are strictly after its position).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ResetLoad {
    /// Items collected so far, newest first, as the pages arrive.
    pub pages: Vec<Item>,
    /// Events received while paging.
    pub buffered: Vec<ThreadEventParams>,
    /// Attachments reported gone.
    pub missing: BTreeSet<UploadId>,
}

/// The open (or recently open) thread's transcript.
#[derive(Clone, Debug, Default)]
pub struct Transcript {
    items: Vec<Item>,
    index: HashMap<ItemId, usize>,
    revs: Vec<u64>,
    /// Last applied journal position. `None` until the first snapshot or replayed event.
    pub position: Option<JournalPosition>,
    /// Who may act, as the subscription last reported.
    pub control: Option<Control>,
    /// Where it is in use, for the read-only states.
    pub control_holder: Option<String>,
    /// Why the program holding it will not let YAAC take it over now (D140).
    pub take_over_blocked: Option<String>,
    /// This window's **Take over** request, while it is in flight or after it failed.
    pub take_over: Option<TakeOverRequest>,
    /// The running turn, for Stop.
    pub active_turn: Option<TurnId>,
    /// Effective capabilities and applied settings.
    pub effective: Option<ThreadEffective>,
    /// Context usage, when reported.
    pub context: Option<ContextUsage>,
    /// Uploads reported as no longer on the target.
    pub missing_uploads: BTreeSet<UploadId>,
    /// Who started each turn, when seen live.
    pub turn_origins: BTreeMap<TurnId, TurnOrigin>,
    /// A reset being loaded.
    pub loading: Option<ResetLoad>,
    /// Answers this window gave.
    pub local_answers: BTreeMap<RequestId, LocalAnswer>,
    /// Bumped on every change, for UI caches.
    pub revision: u64,
}

impl Transcript {
    /// An empty transcript.
    pub fn new() -> Self {
        Self::default()
    }

    /// The items, oldest first.
    pub fn items(&self) -> &[Item] {
        &self.items
    }

    /// The revision of the item at `index` (changes whenever the item does).
    pub fn item_rev(&self, index: usize) -> u64 {
        self.revs.get(index).copied().unwrap_or(0)
    }

    /// The item with this id.
    pub fn item(&self, id: &ItemId) -> Option<&Item> {
        self.index.get(id).map(|&i| &self.items[i])
    }

    /// Index of an item.
    pub fn index_of(&self, id: &ItemId) -> Option<usize> {
        self.index.get(id).copied()
    }

    /// Whether the transcript has been loaded (snapshot or replay applied).
    pub fn is_loaded(&self) -> bool {
        self.position.is_some() && self.loading.is_none()
    }

    /// Insert or replace an item.
    pub fn upsert(&mut self, item: Item, rev: u64) {
        match self.index.get(&item.id) {
            Some(&i) => {
                self.items[i] = item;
                self.revs[i] = rev;
            }
            None => {
                self.index.insert(item.id.clone(), self.items.len());
                self.items.push(item);
                self.revs.push(rev);
            }
        }
        self.revision = rev;
    }

    /// Replace every item (a snapshot), oldest first.
    pub fn replace_all(&mut self, items: Vec<Item>, rev: u64) {
        self.index = items.iter().enumerate().map(|(i, item)| (item.id.clone(), i)).collect();
        self.revs = vec![rev; items.len()];
        self.items = items;
        self.revision = rev;
    }

    /// Whether an event at this position is new for this transcript.
    pub fn admits(&self, event: &ThreadEventParams) -> bool {
        match self.position {
            None => true,
            Some(pos) => event.epoch == pos.epoch && event.position_after().seq > pos.seq,
        }
    }

    /// Apply one journaled event. Returns whether anything changed. Events at or before the
    /// current position are ignored (replay overlap), as are events from another epoch (a
    /// reset is needed; the caller re-subscribes).
    pub fn apply(&mut self, event: &ThreadEventParams, rev: u64) -> bool {
        if let Some(load) = &mut self.loading {
            load.buffered.push(event.clone());
            return false;
        }
        if !self.admits(event) {
            return false;
        }
        self.position = Some(event.position_after());
        self.apply_body(&event.event, rev);
        self.revision = rev;
        true
    }

    fn apply_body(&mut self, event: &ThreadEvent, rev: u64) {
        match event {
            ThreadEvent::TurnStarted(e) => {
                self.active_turn = Some(e.turn_id.clone());
                self.turn_origins.insert(e.turn_id.clone(), e.origin.clone());
            }
            ThreadEvent::TurnCompleted(e) => {
                if self.active_turn.as_ref() == Some(&e.turn_id) {
                    self.active_turn = None;
                }
            }
            ThreadEvent::ItemStarted(e) | ThreadEvent::ItemCompleted(e) => self.upsert(e.item.clone(), rev),
            ThreadEvent::ItemUpdated(e) => self.upsert(e.item.clone(), rev),
            ThreadEvent::ItemDelta(e) => {
                if let Some(&i) = self.index.get(&e.item_id) {
                    append_delta(&mut self.items[i].body, &e.field, &e.delta);
                    self.revs[i] = rev;
                }
            }
            ThreadEvent::RequestChanged(e) => {
                if let Some(&i) = self.index.get(&e.item_id)
                    && let ItemBody::Request(request) = &mut self.items[i].body
                {
                    request.state = e.state.clone();
                    if e.resolution.is_some() {
                        request.resolution = e.resolution.clone();
                    }
                    self.revs[i] = rev;
                }
            }
            ThreadEvent::ControlChanged(e) => {
                self.control = Some(e.control.clone());
                self.control_holder = e.holder.clone();
                self.take_over_blocked = e.take_over_blocked.clone();
                self.active_turn = e.active_turn.clone();
            }
            ThreadEvent::SettingsChanged(e) => self.effective = Some(e.effective.clone()),
            ThreadEvent::UsageUpdated(e) => {
                if e.context.is_some() {
                    self.context = e.context.clone();
                }
            }
            // SEAM (window package): the queue strip and the stopped state.
            ThreadEvent::QueueChanged(_) | ThreadEvent::HoldChanged(_) | ThreadEvent::Unknown(_) => {}
        }
    }

    /// Set a request's state locally (optimistic "submitted", or "unknown" after a lost
    /// answer). The harness's own `requestChanged` overrides it when it arrives.
    pub fn set_request_state(&mut self, request_id: &RequestId, state: RequestState, rev: u64) -> bool {
        for (i, item) in self.items.iter_mut().enumerate() {
            if let ItemBody::Request(request) = &mut item.body
                && &request.request_id == request_id
            {
                request.state = state;
                self.revs[i] = rev;
                self.revision = rev;
                return true;
            }
        }
        false
    }

    /// Begin a reset: subsequent events are held until [`Self::finish_reset`].
    pub fn begin_reset(&mut self) {
        self.loading = Some(ResetLoad::default());
    }

    /// Finish a reset with the complete snapshot (`pages` newest first) and its position; then
    /// apply the held events that are strictly after it.
    pub fn finish_reset(&mut self, position: JournalPosition, rev: u64) {
        let Some(load) = self.loading.take() else { return };
        let mut items = load.pages;
        items.reverse();
        self.replace_all(items, rev);
        self.missing_uploads = load.missing;
        self.position = Some(position);
        for event in load.buffered {
            self.apply(&event, rev);
        }
    }
}

fn append_delta(body: &mut ItemBody, field: &DeltaField, delta: &str) {
    match (body, field) {
        (ItemBody::AgentMessage(m), DeltaField::Text) => m.text.push_str(delta),
        (ItemBody::Reasoning(r), DeltaField::Reasoning) => r.reasoning.push_str(delta),
        (ItemBody::ToolCall(t), DeltaField::Output) => t.output.push_str(delta),
        _ => {}
    }
}
