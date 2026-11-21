//! Acting on several threads at once (D138): the sidebar's multi-selection, which threads a
//! bulk Archive or Mute can act on and why the others are skipped, and the one report of how
//! a bulk Archive went.
//!
//! The selection is window-local UI state, kept apart from the open thread: a plain click
//! opens a thread and clears it; Ctrl+click toggles a row; Shift+click selects the range from
//! the anchor in the order the rows are shown (across groups). It is keyed by [`ThreadKey`],
//! so it survives a reorder, and it keeps only threads that are still listed — nothing is ever
//! acted on that the user cannot see.
//!
//! Eligibility reads capabilities and the thread's state, never the provider's kind.

use std::collections::BTreeSet;

use yaac_protocol::{Capability, ThreadState};

use crate::key::ThreadKey;
use crate::state::AppState;

/// How a row was clicked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Click {
    /// A plain click: open the thread, clear the selection.
    Plain,
    /// Ctrl+click: add or remove this row.
    Toggle,
    /// Shift+click: the rows from the anchor to this one, in the order shown.
    Range,
}

/// The sidebar's multi-selection.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ListSelection {
    keys: BTreeSet<ThreadKey>,
    /// Where a range starts: the row last plain- or Ctrl-clicked.
    anchor: Option<ThreadKey>,
    /// Where a range ends: the row Shift+clicked, or moved to with Shift+↑/↓.
    head: Option<ThreadKey>,
}

fn range(order: &[ThreadKey], a: &ThreadKey, b: &ThreadKey) -> Option<BTreeSet<ThreadKey>> {
    let i = order.iter().position(|k| k == a)?;
    let j = order.iter().position(|k| k == b)?;
    let (lo, hi) = (i.min(j), i.max(j));
    Some(order[lo..=hi].iter().cloned().collect())
}

impl ListSelection {
    /// Nothing selected.
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// How many rows are selected.
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// Whether a row is selected.
    pub fn contains(&self, key: &ThreadKey) -> bool {
        self.keys.contains(key)
    }

    /// The selected threads in the order they are shown (`order`).
    pub fn in_order(&self, order: &[ThreadKey]) -> Vec<ThreadKey> {
        order.iter().filter(|k| self.keys.contains(*k)).cloned().collect()
    }

    /// A click on `key`; `order` is every listed row in display order, `open` the open
    /// thread. Returns whether the thread is to be opened (a plain click).
    pub fn click(&mut self, order: &[ThreadKey], key: &ThreadKey, click: Click, open: Option<&ThreadKey>) -> bool {
        match click {
            Click::Plain => {
                self.keys.clear();
                self.anchor = Some(key.clone());
                self.head = Some(key.clone());
                true
            }
            Click::Toggle => {
                if !self.keys.remove(key) {
                    self.keys.insert(key.clone());
                }
                self.anchor = Some(key.clone());
                self.head = Some(key.clone());
                false
            }
            Click::Range => {
                // Without an anchor the range starts at the open thread, as in a file list.
                let anchor = self.anchor.clone().filter(|a| order.contains(a)).or_else(|| open.filter(|o| order.contains(o)).cloned());
                let a = anchor.unwrap_or_else(|| key.clone());
                self.keys = range(order, &a, key).unwrap_or_else(|| std::iter::once(key.clone()).collect());
                self.anchor = Some(a);
                self.head = Some(key.clone());
                false
            }
        }
    }

    /// Shift+↑ (`-1`) or Shift+↓ (`1`) with focus in the list: the range's end moves one row,
    /// starting from the open thread when nothing was clicked yet.
    pub fn extend(&mut self, order: &[ThreadKey], delta: isize, open: Option<&ThreadKey>) {
        if order.is_empty() {
            return;
        }
        let listed = |k: &Option<ThreadKey>| k.clone().filter(|k| order.contains(k));
        let start = listed(&self.head).or_else(|| listed(&self.anchor)).or_else(|| open.filter(|o| order.contains(o)).cloned());
        let Some(start) = start else {
            // Nothing to start from: the first (or last) row alone.
            let k = if delta < 0 { order[order.len() - 1].clone() } else { order[0].clone() };
            self.keys = std::iter::once(k.clone()).collect();
            self.anchor = Some(k.clone());
            self.head = Some(k);
            return;
        };
        let anchor = listed(&self.anchor).unwrap_or_else(|| start.clone());
        let at = order.iter().position(|k| *k == start).unwrap_or(0);
        let last = order.len() - 1;
        let to = at.saturating_add_signed(delta).min(last);
        let head = order[to].clone();
        self.keys = range(order, &anchor, &head).unwrap_or_default();
        self.anchor = Some(anchor);
        self.head = Some(head);
    }

    /// Ctrl+A with focus in the list: every listed row.
    pub fn select_all(&mut self, order: &[ThreadKey]) {
        self.keys = order.iter().cloned().collect();
        self.anchor = order.first().cloned();
        self.head = order.last().cloned();
    }

    /// Esc, Clear selection, or after a bulk action: the next range starts from the open
    /// thread again.
    pub fn clear(&mut self) {
        self.keys.clear();
        self.anchor = None;
        self.head = None;
    }

    /// Keep only what is still listed (`order`): a thread that left the list — removed,
    /// archived, filtered out, or in a section that was collapsed — leaves the selection.
    pub fn retain_listed(&mut self, order: &[ThreadKey]) {
        if self.keys.is_empty() && self.anchor.is_none() {
            return;
        }
        let listed: BTreeSet<&ThreadKey> = order.iter().collect();
        self.keys.retain(|k| listed.contains(k));
        if self.anchor.as_ref().is_some_and(|a| !listed.contains(a)) {
            self.anchor = None;
        }
        if self.head.as_ref().is_some_and(|h| !listed.contains(h)) {
            self.head = None;
        }
    }
}

/// What a bulk action does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BulkKind {
    /// Archive (`thread/archive`, one request per thread).
    Archive,
    /// Mute (one `hub/view/setMany`).
    Mute,
    /// Unmute (one `hub/view/setMany`).
    Unmute,
}

impl BulkKind {
    fn verb(self) -> &'static str {
        match self {
            BulkKind::Archive => "archived",
            BulkKind::Mute => "muted",
            BulkKind::Unmute => "unmuted",
        }
    }
}

/// Why a thread is left out of a bulk action.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Skip {
    /// The thread is not known here any more.
    Unknown,
    /// Already archived (Archive), or archived and so not muted (Mute).
    Archived,
    /// Its harness cannot archive threads (no `threadArchive` capability).
    NoArchive,
    /// Its machine is not connected (or its WSL distro is stopped).
    Offline,
    /// A turn is running.
    Running,
    /// It is at work, waiting on another agent.
    Waiting,
    /// It is waiting on the user (a question or an approval must not be archived away).
    NeedsYou,
    /// It is in use elsewhere (read-only here).
    InUse,
    /// Unmute: it is not muted.
    NotMuted,
}

impl Skip {
    /// "2 are running", "1 is in use elsewhere".
    pub fn phrase(self, n: usize) -> String {
        let one = n == 1;
        let is = if one { "is" } else { "are" };
        match self {
            Skip::Unknown => format!("{n} {is} no longer listed"),
            Skip::Archived => format!("{n} {is} already archived"),
            Skip::NoArchive => format!("{n} cannot be archived by {}", if one { "its harness" } else { "their harness" }),
            Skip::Offline => format!("{n} {is} on a machine that is not connected"),
            Skip::Running => format!("{n} {is} running"),
            Skip::Waiting => format!("{n} {is} waiting on another agent"),
            Skip::NeedsYou => format!("{n} {is} waiting for you"),
            Skip::InUse => format!("{n} {is} in use elsewhere"),
            Skip::NotMuted => format!("{n} {is} not muted"),
        }
    }
}

/// Which of the chosen threads a bulk action acts on, and why the others are skipped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BulkPlan {
    /// What it does.
    pub kind: BulkKind,
    /// The threads it acts on, in the order given.
    pub eligible: Vec<ThreadKey>,
    /// The others, each with its reason.
    pub skipped: Vec<(ThreadKey, Skip)>,
}

/// Whether the thread can do `cap` (effective capabilities when known, else its provider's).
fn can(state: &AppState, key: &ThreadKey, cap: &Capability) -> bool {
    crate::thread_menu::can(state, key, cap)
}

fn archive_skip(state: &AppState, key: &ThreadKey) -> Option<Skip> {
    let Some(s) = state.summary(key) else { return Some(Skip::Unknown) };
    if s.archived {
        return Some(Skip::Archived);
    }
    if !can(state, key, &Capability::ThreadArchive) {
        return Some(Skip::NoArchive);
    }
    let connected = state.target(&key.target).is_some_and(|t| t.connected());
    if !connected || state.stopped_distro(&key.target).is_some() {
        return Some(Skip::Offline);
    }
    if s.state == ThreadState::Running {
        return Some(Skip::Running);
    }
    if s.state == ThreadState::Waiting {
        return Some(Skip::Waiting);
    }
    if s.state == ThreadState::NeedsAttention {
        return Some(Skip::NeedsYou);
    }
    if !state.control(key).may_act() {
        return Some(Skip::InUse);
    }
    None
}

/// The plan for `kind` over `keys` at `now`.
pub fn plan(state: &AppState, keys: &[ThreadKey], kind: BulkKind, now: i64) -> BulkPlan {
    let mut eligible = Vec::new();
    let mut skipped = Vec::new();
    for key in keys {
        let skip = match kind {
            BulkKind::Archive => archive_skip(state, key),
            BulkKind::Mute => match state.summary(key) {
                None => Some(Skip::Unknown),
                Some(s) if s.archived => Some(Skip::Archived),
                Some(_) => None,
            },
            BulkKind::Unmute => match state.summary(key) {
                None => Some(Skip::Unknown),
                Some(_) if !state.is_muted(key, now) => Some(Skip::NotMuted),
                Some(_) => None,
            },
        };
        match skip {
            Some(why) => skipped.push((key.clone(), why)),
            None => eligible.push(key.clone()),
        }
    }
    BulkPlan { kind, eligible, skipped }
}

impl BulkPlan {
    /// How many threads were chosen.
    pub fn total(&self) -> usize {
        self.eligible.len() + self.skipped.len()
    }

    /// The skip reasons with their counts, in a fixed order.
    pub fn reasons(&self) -> Vec<(Skip, usize)> {
        let mut out: std::collections::BTreeMap<Skip, usize> = std::collections::BTreeMap::new();
        for (_, s) in &self.skipped {
            *out.entry(*s).or_default() += 1;
        }
        out.into_iter().collect()
    }

    /// "3 of 5 can be archived; 2 are running" — `None` when nothing is skipped.
    pub fn skipped_line(&self) -> Option<String> {
        if self.skipped.is_empty() {
            return None;
        }
        let why: Vec<String> = self.reasons().into_iter().map(|(s, n)| s.phrase(n)).collect();
        let head = if self.eligible.is_empty() {
            format!("None of these {} can be {}", self.total(), self.kind.verb())
        } else {
            format!("{} of {} can be {}", self.eligible.len(), self.total(), self.kind.verb())
        };
        Some(format!("{head}; {}", why.join("; ")))
    }
}

/// "3 threads", "1 thread".
pub fn threads(n: usize) -> String {
    if n == 1 { "1 thread".to_owned() } else { format!("{n} threads") }
}

/// The bulk Archive confirmation (D111, once for all).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BulkArchiveDialog {
    /// Which threads it archives, and which it skips.
    pub plan: BulkPlan,
    /// "Archive 3 threads?"
    pub title: String,
    /// What it does.
    pub body: &'static str,
    /// "3 of 5 can be archived; 2 are running", when some are skipped.
    pub skipped: Option<String>,
}

impl BulkArchiveDialog {
    /// The dialog for `keys` (in the order shown).
    pub fn new(state: &AppState, keys: &[ThreadKey], now: i64) -> Self {
        let plan = plan(state, keys, BulkKind::Archive, now);
        let title = if plan.eligible.is_empty() { format!("{} cannot be archived", threads(plan.total())) } else { format!("Archive {}?", threads(plan.eligible.len())) };
        let skipped = plan.skipped_line();
        Self { title, body: "They move to Archived, and lose their pins and mutes. Unarchive brings each back.", skipped, plan }
    }

    /// Something can be archived (the Archive button is offered).
    pub fn can_archive(&self) -> bool {
        !self.plan.eligible.is_empty()
    }
}

/// How one thread's `thread/archive` ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Archived.
    Ok,
    /// Refused, with the target's message.
    Failed(String),
    /// The answer was lost: it may or may not be archived, and it is not asked again.
    Uncertain,
}

/// The one report of a bulk Archive, once every answer is in — only when something did not
/// simply succeed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BulkReport {
    /// "2 could not be archived: billing-service, CI integration · uncertain: Fix frame decoder"
    pub text: String,
    /// One line per thread that did not succeed (the hover text).
    pub detail: Vec<String>,
}

/// The report for a bulk Archive's outcomes (title, outcome); `None` when every one succeeded.
pub fn archive_report(outcomes: &[(String, Outcome)]) -> Option<BulkReport> {
    let failed: Vec<&(String, Outcome)> = outcomes.iter().filter(|(_, o)| matches!(o, Outcome::Failed(_))).collect();
    let uncertain: Vec<&String> = outcomes.iter().filter(|(_, o)| *o == Outcome::Uncertain).map(|(t, _)| t).collect();
    if failed.is_empty() && uncertain.is_empty() {
        return None;
    }
    let mut parts = Vec::new();
    if !failed.is_empty() {
        let names: Vec<&str> = failed.iter().map(|(t, _)| t.as_str()).collect();
        parts.push(format!("{} could not be archived: {}", failed.len(), names.join(", ")));
    }
    if !uncertain.is_empty() {
        let names: Vec<&str> = uncertain.iter().map(|t| t.as_str()).collect();
        parts.push(format!("uncertain: {}", names.join(", ")));
    }
    let mut detail = Vec::new();
    for (t, o) in outcomes {
        match o {
            Outcome::Failed(why) => detail.push(format!("{t}: {why}")),
            Outcome::Uncertain => detail.push(format!("{t}: YAAC lost contact; it may or may not be archived and is not asked again. Check the list once the target answers.")),
            Outcome::Ok => {}
        }
    }
    Some(BulkReport { text: parts.join(" · "), detail })
}
