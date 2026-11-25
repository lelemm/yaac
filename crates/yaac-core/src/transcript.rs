//! One thread's items, kept in journal order, with the journal position they are valid at.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use yaac_protocol::{
    ContextUsage, Control, DeltaField, Item, ItemBody, ItemId, JournalPosition, QueueChange, QueuedMessage, RequestId,
    RequestState, Seq, ThreadEffective, ThreadEvent, ThreadEventParams, TurnId, TurnOrigin, UploadId,
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
    /// Outstanding requests that may be outside the latest page.
    pub required: Vec<Item>,
    /// Events received while paging.
    pub buffered: Vec<ThreadEventParams>,
    /// Attachments reported gone.
    pub missing: BTreeSet<UploadId>,
    /// Items in later wire pages can already include live events since the first page.
    pub(crate) covered_items: HashMap<ItemId, Seq>,
}

/// The open (or recently open) thread's transcript.
#[derive(Clone, Debug, Default)]
pub struct Transcript {
    items: Vec<Item>,
    index: HashMap<ItemId, usize>,
    revs: Vec<u64>,
    covered_items: HashMap<ItemId, Seq>,
    covered_through: Seq,
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
    /// Latest completed turn observed by this subscription.
    pub last_completed_turn: Option<TurnId>,
    /// Effective capabilities and applied settings.
    pub effective: Option<ThreadEffective>,
    /// Context usage, when reported.
    pub context: Option<ContextUsage>,
    /// Uploads reported as no longer on the target.
    pub missing_uploads: BTreeSet<UploadId>,
    /// Who started each turn, when seen live.
    pub turn_origins: BTreeMap<TurnId, TurnOrigin>,
    /// When each turn started and how it ended, when seen live (D156).
    pub turns: BTreeMap<TurnId, crate::turns::TurnTimes>,
    /// A reset being loaded.
    pub loading: Option<ResetLoad>,
    /// Cursor immediately before the loaded history window.
    pub older_cursor: Option<String>,
    /// Whether a request for an older page is in flight.
    pub loading_older: bool,
    /// Cursor and request state at the newer edge of a historical window.
    pub newer_cursor: Option<String>,
    pub loading_newer: bool,
    /// Full prompt metadata, independent of the loaded rows. None for older targets.
    pub prompts: Option<Vec<yaac_protocol::TimelinePrompt>>,
    /// Newer settled rows were left behind to keep the window bounded.
    pub history_away: bool,
    /// Answers this window gave.
    pub local_answers: BTreeMap<RequestId, LocalAnswer>,
    /// User messages queued behind the running turn, in delivery order (D166): from the
    /// subscription's snapshot, then its `queueChanged` events.
    pub queued: Vec<QueuedMessage>,
    /// Queued messages with an edit, cancel or Send now of this window in flight.
    pub queue_busy: BTreeSet<String>,
    /// Messages this window sent whose own item has not arrived, oldest first (D165). Each
    /// has its echo among the items.
    pending: Vec<crate::sending::Pending>,
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

    /// Insert or replace an item. A sent message's own item takes the place of its echo
    /// (D165).
    pub fn upsert(&mut self, item: Item, rev: u64) {
        let echo = if self.index.contains_key(&item.id) { None } else { self.pending.iter().position(|p| p.matches(&item)) };
        if let Some(n) = echo {
            let p = self.pending.remove(n);
            if let Some(i) = self.index.remove(&p.item) {
                self.index.insert(item.id.clone(), i);
                self.items[i] = item;
                self.revs[i] = rev;
                self.revision = rev;
                return;
            }
        }
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

    /// Replace every item (a snapshot), oldest first. The echoes of sends still in flight stay
    /// at the end, unless the snapshot already holds their message.
    pub fn replace_all(&mut self, mut items: Vec<Item>, rev: u64) {
        self.pending.retain(|p| !items.iter().any(|i| p.matches(i)));
        items.extend(self.pending.iter().map(crate::sending::Pending::item));
        self.index = items.iter().enumerate().map(|(i, item)| (item.id.clone(), i)).collect();
        self.revs = vec![rev; items.len()];
        self.items = items;
        self.revision = rev;
    }

    // ── Sent messages before their own item (D165) ───────────────────────────────────────

    /// Show a message being sent: its echo at the end of the transcript, in the same frame.
    pub fn add_pending(&mut self, pending: crate::sending::Pending, rev: u64) {
        let item = pending.item();
        self.pending.push(pending);
        self.upsert(item, rev);
    }

    /// The sends whose item has not arrived, oldest first.
    pub fn pending(&self) -> &[crate::sending::Pending] {
        &self.pending
    }

    /// The send an echo stands for.
    pub fn pending_of(&self, item: &ItemId) -> Option<&crate::sending::Pending> {
        self.pending.iter().find(|p| &p.item == item)
    }

    /// The send in flight under this op id.
    pub fn pending_op(&self, op: &yaac_protocol::OpId) -> Option<&crate::sending::Pending> {
        self.pending.iter().find(|p| &p.op == op)
    }

    /// Change a send (its state, its op id for a Retry); its echo follows.
    pub fn update_pending(&mut self, op: &yaac_protocol::OpId, rev: u64, f: impl FnOnce(&mut crate::sending::Pending)) -> bool {
        let Some(p) = self.pending.iter_mut().find(|p| &p.op == op) else { return false };
        f(p);
        let item = p.item();
        if let Some(&i) = self.index.get(&item.id) {
            self.items[i] = item;
            self.revs[i] = rev;
        }
        self.revision = rev;
        true
    }

    /// Once a send's turn is known: when its message is already here (it came before the
    /// answer, without an op id), the echo goes.
    pub fn settle_pending(&mut self, op: &yaac_protocol::OpId, rev: u64) {
        let here = self.pending_op(op).is_some_and(|p| self.items.iter().any(|i| p.matches(i)));
        if here {
            self.remove_pending(op, rev);
        }
    }

    /// Take a send and its echo out of the transcript (Edit, Dismiss, or a message that went
    /// to the queue instead).
    pub fn remove_pending(&mut self, op: &yaac_protocol::OpId, rev: u64) -> Option<crate::sending::Pending> {
        let n = self.pending.iter().position(|p| &p.op == op)?;
        let p = self.pending.remove(n);
        if let Some(i) = self.index.remove(&p.item) {
            self.items.remove(i);
            self.revs.remove(i);
            for v in self.index.values_mut() {
                if *v > i {
                    *v -= 1;
                }
            }
        }
        self.revision = rev;
        Some(p)
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
        let position = event.position_after();
        self.position = Some(position);
        if !self.covered_items.is_empty() {
            let item = match &event.event {
                ThreadEvent::ItemStarted(e) | ThreadEvent::ItemCompleted(e) => Some(&e.item.id),
                ThreadEvent::ItemUpdated(e) => Some(&e.item.id),
                ThreadEvent::ItemDelta(e) => Some(&e.item_id),
                ThreadEvent::RequestChanged(e) => Some(&e.item_id),
                _ => None,
            };
            let covered = item.and_then(|id| self.covered_items.get(id)).is_some_and(|&seq| position.seq <= seq);
            if position.seq >= self.covered_through {
                self.covered_items.clear();
            }
            if covered { return false; }
        }
        self.apply_body(&event.event, rev);
        self.trim_history();
        self.revision = rev;
        true
    }

    fn apply_body(&mut self, event: &ThreadEvent, rev: u64) {
        match event {
            ThreadEvent::TurnStarted(e) => {
                self.active_turn = Some(e.turn_id.clone());
                self.turn_origins.insert(e.turn_id.clone(), e.origin.clone());
                self.turns.entry(e.turn_id.clone()).or_default().started = e.at.clone();
            }
            ThreadEvent::TurnCompleted(e) => {
                self.last_completed_turn = Some(e.turn_id.clone());
                self.turns.entry(e.turn_id.clone()).or_default().ended =
                    Some(crate::turns::TurnEnd { status: e.status.clone(), at: e.at.clone(), error: e.error.clone() });
                if self.active_turn.as_ref() == Some(&e.turn_id) {
                    self.active_turn = None;
                }
            }
            ThreadEvent::ItemStarted(e) | ThreadEvent::ItemCompleted(e) => self.apply_item(&e.item, rev),
            ThreadEvent::ItemUpdated(e) => self.apply_item(&e.item, rev),
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
                    } else if e.state == RequestState::Pending {
                        // Back to waiting (an automatic approval the harness did not take,
                        // P342): nobody has answered it.
                        request.resolution = None;
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
            ThreadEvent::QueueChanged(e) => self.apply_queue(&e.change, &e.entry),
            // SEAM (window package): the stopped state.
            ThreadEvent::HoldChanged(_) | ThreadEvent::Unknown(_) => {}
        }
    }

    fn apply_item(&mut self, item: &Item, rev: u64) {
        if let Some(prompts) = &mut self.prompts
            && let Some(prompt) = yaac_protocol::TimelinePrompt::of(item, false)
        {
            if let Some(known) = prompts.iter_mut().find(|p| p.item_id == prompt.item_id) {
                *known = prompt;
            } else {
                prompts.push(prompt);
            }
        }
        if self.history_away && self.newer_cursor.is_some()
            && !self.index.contains_key(&item.id)
            && !matches!(&item.body, ItemBody::Request(_))
            && !self.pending.iter().any(|p| p.matches(item))
        {
            return;
        }
        self.upsert(item.clone(), rev);
    }

    /// The queue as the subscription reported it (absent: empty).
    pub fn set_queue(&mut self, queued: Option<Vec<QueuedMessage>>, rev: u64) {
        self.queued = queued.unwrap_or_default();
        self.queue_busy.retain(|id| self.queued.iter().any(|q| &q.queue_id == id));
        self.revision = rev;
    }

    /// One change to the queue. Idempotent, so a replay that overlaps the snapshot ends in the
    /// same queue: an entry is replaced in place by its id, or added at the end.
    fn apply_queue(&mut self, change: &QueueChange, entry: &QueuedMessage) {
        match change {
            QueueChange::Added | QueueChange::Updated => match self.queued.iter_mut().find(|q| q.queue_id == entry.queue_id) {
                Some(q) => *q = entry.clone(),
                None => self.queued.push(entry.clone()),
            },
            QueueChange::Removed | QueueChange::Delivered => {
                self.queued.retain(|q| q.queue_id != entry.queue_id);
                self.queue_busy.remove(&entry.queue_id);
            }
            QueueChange::Unknown(_) => {}
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
        self.loading_older = false;
        self.loading_newer = false;
    }

    pub fn cancel_reset(&mut self, rev: u64) {
        if let Some(load) = self.loading.take() {
            for event in load.buffered {
                self.apply(&event, rev);
            }
        }
    }

    /// Fits the visible history into a bounded window while preserving live requests and drafts.
    pub fn trim_history(&mut self) {
        self.trim_history_from(self.history_away, None);
    }

    fn trim_history_from(&mut self, newest: bool, overlap: Option<&TurnId>) -> bool {
        const WINDOW: usize = 360;
        if self.items.len() <= WINDOW || self.loading_older || self.loading_newer {
            return false;
        }
        let protected = |item: &Item| {
            item.status == yaac_protocol::ItemStatus::InProgress
                || matches!(&item.body, ItemBody::Request(r) if matches!(r.state, RequestState::Pending | RequestState::Submitted))
                || self.pending.iter().any(|p| p.item == item.id)
        };
        let mut start = 0;
        let mut groups: Vec<_> = self.items.chunk_by(|left, right| left.turn_id == right.turn_id)
            .map(|group| {
                let range = start..start + group.len();
                start = range.end;
                range
            }).collect();
        if newest { groups.reverse(); }
        let mut remaining = self.items.len();
        let mut dropping = BTreeSet::new();
        for group in groups {
            if Some(&self.items[group.start].turn_id) == overlap || remaining - group.len() < WINDOW {
                break;
            }
            if self.items[group.clone()].iter().any(protected) {
                continue;
            }
            remaining -= group.len();
            dropping.extend(group);
        }
        // Optimistic sends have local IDs. Only target items can anchor the next page.
        let can_anchor = |item: &&Item| !crate::sending::is_echo(&item.id);
        if !newest && !dropping.is_empty()
            && self.older_cursor.as_deref().is_none_or(|c| c.starts_with("history:"))
            && let Some(anchor) = dropping.last().and_then(|i| self.items[i + 1..].iter().find(can_anchor))
        {
            self.older_cursor = Some(format!("history:{}", anchor.id));
        }
        if newest && self.prompts.is_some()
            && let Some(anchor) = dropping.first().and_then(|&i| self.items[..i].iter().rev().find(can_anchor))
        {
            self.newer_cursor = Some(format!("history-newer:{}", anchor.id));
        }
        if dropping.is_empty() { return false; }
        self.retain_history(|i| !dropping.contains(&i));
        true
    }

    fn retain_history(&mut self, keep: impl Fn(usize) -> bool) {
        let mut index = 0;
        self.items.retain(|_| { let yes = keep(index); index += 1; yes });
        index = 0;
        self.revs.retain(|_| { let yes = keep(index); index += 1; yes });
        self.index = self.items.iter().enumerate().map(|(i, item)| (item.id.clone(), i)).collect();
    }

    /// Prepend an older page without overwriting records already updated by the live stream.
    pub fn prepend_history(&mut self, items: Vec<Item>, rev: u64) {
        let mut older: Vec<_> = items.into_iter().rev().filter(|item| !self.index.contains_key(&item.id)).collect();
        if older.is_empty() {
            self.trim_history();
            return;
        }
        let overlap = self.items.first().map(|item| item.turn_id.clone());
        let count = older.len();
        older.append(&mut self.items);
        self.items = older;
        self.revs.splice(..0, std::iter::repeat_n(rev, count));
        self.index = self.items.iter().enumerate().map(|(i, item)| (item.id.clone(), i)).collect();
        self.revision = rev;
        if self.trim_history_from(true, overlap.as_ref()) {
            self.history_away = true;
        }
    }

    pub fn append_history(&mut self, anchor: &ItemId, items: Vec<Item>, rev: u64) {
        let Some(edge) = self.index_of(anchor) else { return };
        let overlap = self.items[edge].turn_id.clone();
        let items: Vec<_> = items.into_iter().rev().filter(|item| !self.index.contains_key(&item.id)).collect();
        let count = items.len();
        self.items.splice(edge + 1..edge + 1, items);
        self.revs.splice(edge + 1..edge + 1, std::iter::repeat_n(rev, count));
        self.index = self.items.iter().enumerate().map(|(i, item)| (item.id.clone(), i)).collect();
        self.revision = rev;
        self.trim_history_from(false, Some(&overlap));
    }

    /// Finish a reset with the complete snapshot (`pages` newest first) and its position; then
    /// apply the held events that are strictly after it.
    pub fn finish_reset(&mut self, position: JournalPosition, rev: u64) {
        let Some(load) = self.loading.take() else { return };
        let mut items = load.pages;
        items.reverse();
        items.extend(load.required);
        self.replace_all(items, rev);
        self.missing_uploads = load.missing;
        self.position = Some(position);
        self.covered_items = load.covered_items;
        self.covered_through = self.covered_items.values().copied().max().unwrap_or_default();
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
