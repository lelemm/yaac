//! Attention and display state: what mark and word a thread shows, everywhere.
//!
//! Needing the user outranks every other state. A thread's own attention comes from the
//! requests still open on it (its hints, when they are fresher than its summary; otherwise
//! the summary's `needsAttention`) or from its orchestration's budget having run out. It then
//! climbs: every ancestor (through `spawnedBy`, across targets) counts the descendants that
//! need the user.

use std::collections::BTreeMap;

use yaac_protocol::attach::ConnectionStatus;
use yaac_protocol::ThreadState;

use crate::hints::RequestKindWord;
use crate::key::ThreadKey;
use crate::state::AppState;

/// The mark a thread or target shows: a distinct shape plus a word.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Mark {
    /// Play triangle in a circle.
    Running,
    /// Warning triangle.
    NeedsYou,
    /// Pause (an orchestration's budget ran out — it also needs the user).
    Paused,
    /// Clock.
    Waiting,
    /// Check in a circle.
    Done,
    /// Cross in a circle.
    Failed,
    /// Hollow circle.
    Idle,
    /// Dashed hollow circle: unreachable.
    Offline,
    /// Dashed hollow circle: disabled.
    Off,
}

impl Mark {
    /// The word shown beside the mark. Idle shows a relative time instead (empty here).
    pub fn word(self) -> &'static str {
        match self {
            Mark::Running => "running",
            Mark::NeedsYou => "needs you",
            Mark::Paused => "paused",
            Mark::Waiting => "waiting",
            Mark::Done => "done",
            Mark::Failed => "failed",
            Mark::Idle => "",
            Mark::Offline => "offline",
            Mark::Off => "off",
        }
    }

    /// Urgency, for "the most urgent state" (lower is more urgent).
    pub fn urgency(self) -> u8 {
        match self {
            Mark::NeedsYou => 0,
            Mark::Paused => 1,
            Mark::Failed => 2,
            Mark::Running => 3,
            Mark::Waiting => 4,
            Mark::Offline => 5,
            Mark::Done => 6,
            Mark::Idle => 7,
            Mark::Off => 8,
        }
    }
}

/// Why a thread needs the user.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Attention {
    /// A request is open.
    Request(RequestKindWord),
    /// The summary says so; the kind is not known yet.
    Unspecified,
    /// The orchestration rooted here ran out of budget.
    BudgetReached,
}

impl Attention {
    /// "approval", "question", "budget reached", "needs you".
    pub fn word(self) -> &'static str {
        match self {
            Attention::Request(kind) => kind.word(),
            Attention::Unspecified => "needs you",
            Attention::BudgetReached => "budget reached",
        }
    }
}

/// A thread's own attention, ignoring descendants.
///
/// A thread held by another program or client has none here: the user cannot answer from this
/// window, so it is not "needs you" in this window (its cards say "answer in the other
/// program" instead).
pub fn own_attention(state: &AppState, key: &ThreadKey) -> Option<Attention> {
    let rec = state.threads.get(key)?;
    // Held by another program: not "needs you" here. (A stopped WSL distro does not hide a
    // blocked thread: its question is still the user's.)
    if !state.control(key).may_act() {
        return None;
    }
    let from_requests = match state.hints.get(key) {
        Some(h) if h.stamp > rec.stamp => h.open_request().map(Attention::Request),
        Some(h) if rec.summary.state == ThreadState::NeedsAttention => {
            Some(h.open_request().map_or(Attention::Unspecified, Attention::Request))
        }
        _ if rec.summary.state == ThreadState::NeedsAttention => Some(Attention::Unspecified),
        _ => None,
    };
    if from_requests.is_some() {
        return from_requests;
    }
    let address = key.address();
    if state.budgets.get(&address).is_some_and(|b| b.exhausted) {
        return Some(Attention::BudgetReached);
    }
    None
}

/// Attention for every thread at once.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AttentionMap {
    /// Threads that need the user.
    pub own: BTreeMap<ThreadKey, Attention>,
}

impl AttentionMap {
    /// Compute from the state (recomputed from the requests still open, every time).
    pub fn compute(state: &AppState) -> Self {
        let mut own = BTreeMap::new();
        for key in state.threads.keys() {
            if let Some(a) = own_attention(state, key) {
                own.insert(key.clone(), a);
            }
        }
        Self { own }
    }

    /// The thread's own attention.
    pub fn of(&self, key: &ThreadKey) -> Option<Attention> {
        self.own.get(key).copied()
    }

}

/// How a thread is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThreadDisplay {
    /// The mark.
    pub mark: Mark,
    /// Its own attention.
    pub attention: Option<Attention>,
    /// Held by another program or client: read-only here.
    pub read_only: bool,
}

/// The display of a thread.
pub fn display(state: &AppState, map: &AttentionMap, key: &ThreadKey) -> ThreadDisplay {
    let attention = map.of(key);
    let read_only = state.threads.contains_key(key) && !state.control(key).may_act();
    let mark = match attention {
        Some(Attention::BudgetReached) => Mark::Paused,
        Some(_) => Mark::NeedsYou,
        None => base_mark(state, key),
    };
    ThreadDisplay { mark, attention, read_only }
}

/// The mark from the thread's own state, without attention.
fn base_mark(state: &AppState, key: &ThreadKey) -> Mark {
    if let Some(t) = state.target(&key.target) {
        if !t.entry.enabled || t.entry.state.status == ConnectionStatus::Disabled {
            return Mark::Off;
        }
        if t.entry.state.status == ConnectionStatus::Offline {
            return Mark::Offline;
        }
    }
    let Some(s) = state.summary(key) else { return Mark::Idle };
    let hints = state.hints.get(key);
    // The target reports waiting (on another agent) and done (a spawned thread whose work
    // completed) itself; the window does not derive them.
    match &s.state {
        ThreadState::Running => Mark::Running,
        ThreadState::NeedsAttention if hints.is_some_and(|h| h.running_turn.is_some()) => Mark::Running,
        ThreadState::Waiting => Mark::Waiting,
        ThreadState::Done => Mark::Done,
        ThreadState::Failed => Mark::Failed,
        _ => Mark::Idle,
    }
}

/// The mark of a target's connection.
pub fn target_mark(state: &AppState, target: &yaac_protocol::TargetId) -> Mark {
    match state.target(target) {
        Some(t) if !t.entry.enabled => Mark::Off,
        Some(t) => match t.entry.state.status {
            ConnectionStatus::Connected => Mark::Done,
            ConnectionStatus::Connecting => Mark::Running,
            ConnectionStatus::Disabled => Mark::Off,
            ConnectionStatus::Incompatible => Mark::Failed,
            _ => Mark::Offline,
        },
        None => Mark::Offline,
    }
}
