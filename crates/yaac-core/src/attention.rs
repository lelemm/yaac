//! Attention and display state: what mark and word a thread shows, everywhere.
//!
//! Needing the user outranks every other state. A thread's own attention comes from the
//! requests still open on it (its hints, when they are fresher than its summary; otherwise
//! the summary's `needsAttention`) or from its orchestration's budget holding back an
//! agent-started turn while the thread is idle ([`budget_mark`], D164). It then
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
    /// Play triangle (the symbol alone, D122).
    Running,
    /// Red exclamation.
    NeedsYou,
    /// Pause (an orchestration's budget ran out — it also needs the user).
    Paused,
    /// Hourglass.
    Waiting,
    /// Check.
    Done,
    /// Cross.
    Failed,
    /// Nothing drawn.
    Idle,
    /// Nothing drawn: unreachable (the row is dimmed).
    Offline,
    /// Nothing drawn: disabled (the row is dimmed).
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
    // blocked thread: its question is still the user's.) A harness sub-agent's requests follow
    // its parent's writer (D162).
    if !state.control(key).may_act() && !state.may_answer(key) {
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
    if budget_mark(state, key) == Some(BudgetMark::Paused) {
        return Some(Attention::BudgetReached);
    }
    None
}

/// What a root thread shows of its orchestration's used-up budget (D164).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BudgetMark {
    /// The quiet note — "Agent budget reached" and **Continue** — beside the thread's real
    /// state: its own turn runs, or nothing an agent started is held back.
    Reached,
    /// ❚❚ "Paused · budget reached": the thread is idle and an agent-started turn is held back.
    Paused,
}

/// Whether the orchestration rooted at this thread has used up its budget, and how loudly to
/// say it. A budget only stops mesh actions (agent-started turns, spawns); it never hides what
/// the thread itself is doing, so a running thread keeps its spinner and gets the quiet note.
pub fn budget_mark(state: &AppState, key: &ThreadKey) -> Option<BudgetMark> {
    let b = state.budgets.get(&key.address()).filter(|b| b.exhausted)?;
    // An older target does not say what it holds back: then the budget alone says it.
    let held = b.held.is_none_or(|n| n > 0);
    let busy = matches!(base_mark(state, key), Mark::Running | Mark::Waiting);
    Some(if held && !busy { BudgetMark::Paused } else { BudgetMark::Reached })
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
    /// Its last turn ended while nobody looked (D114): the cornflower outline. Not drawn
    /// while the thread needs the user, is paused, failed, or running — those marks already
    /// say more, and win where both would be drawn.
    pub unseen: bool,
    /// Running in fast mode (D125), as the target reports it: the spinner is yellow and
    /// turns a little faster. Only ever set beside the running mark.
    pub fast: bool,
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
    let unseen = state.is_unseen(key) && unseen_shows(mark);
    let fast = mark == Mark::Running && state.summary(key).is_some_and(|s| s.fast);
    ThreadDisplay { mark, attention, read_only, unseen, fast }
}

/// Whether the unseen outline is drawn beside this mark (D114's precedence): needs you,
/// paused, failed and running take precedence.
pub fn unseen_shows(mark: Mark) -> bool {
    !matches!(mark, Mark::NeedsYou | Mark::Paused | Mark::Failed | Mark::Running)
}

/// The mark from the thread's own state, without attention.
fn base_mark(state: &AppState, key: &ThreadKey) -> Mark {
    if let Some(t) = state.target(&key.target) {
        if !t.entry.enabled || matches!(t.entry.state.status, ConnectionStatus::Disabled | ConnectionStatus::Stopped) {
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
            ConnectionStatus::Disabled | ConnectionStatus::Stopped => Mark::Off,
            ConnectionStatus::Incompatible => Mark::Failed,
            _ => Mark::Offline,
        },
        None => Mark::Offline,
    }
}
