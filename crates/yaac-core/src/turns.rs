//! A finished turn collapses into one line (D156): everything the agent did between the
//! prompt and its final message folds into **"Worked for 26s ›"**, with the final message
//! shown in full under it. While the turn runs, or while it waits on the user, nothing is
//! folded. Clicking the line opens the turn's work again; that is view state, kept in
//! [`Opened`] like the other expansions.
//!
//! This module works on the blocks [`crate::activity::blocks`] already made, so the rules of
//! activity lines stay in one place and this one only decides what a finished turn hides.

use std::collections::{BTreeMap, BTreeSet};

use yaac_protocol::{Item, ItemBody, ItemId, ItemStatus, NoticeKind, RequestState, Timestamp, TurnId, TurnStatus};

use crate::activity::{Block, Expansion};
use crate::key::ThreadKey;
use crate::transcript::Transcript;

/// What the window saw of a turn's start and end (`turnStarted`, `turnCompleted`). A turn read
/// from a snapshot has neither; its end is then inferred from its items.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TurnTimes {
    /// When it started.
    pub started: Option<Timestamp>,
    /// How and when it ended.
    pub ended: Option<TurnEnd>,
}

/// A turn's end, as `turnCompleted` reported it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TurnEnd {
    /// How it ended.
    pub status: TurnStatus,
    /// When.
    pub at: Option<Timestamp>,
    /// The error, for a failed turn.
    pub error: Option<String>,
}

/// How a finished turn ended, as the "Worked for" line says it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Finished normally.
    Completed,
    /// Stopped by the user, the harness or an update.
    Interrupted,
    /// Failed.
    Failed,
}

/// A finished turn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finished {
    /// How it ended.
    pub outcome: Outcome,
    /// Its wall-clock duration, when the times are known.
    pub secs: Option<i64>,
    /// The error of a failed turn, shown under the line even while it is collapsed.
    pub error: Option<String>,
}

/// The turns whose work the user opened again, per thread (window view state).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Opened {
    turns: BTreeSet<(ThreadKey, TurnId)>,
}

impl Opened {
    /// Whether a turn's work is shown.
    pub fn is_open(&self, key: &ThreadKey, turn: &TurnId) -> bool {
        self.turns.contains(&(key.clone(), turn.clone()))
    }

    /// Open or close a turn's work.
    pub fn toggle(&mut self, key: &ThreadKey, turn: &TurnId) {
        let k = (key.clone(), turn.clone());
        if !self.turns.remove(&k) {
            self.turns.insert(k);
        }
    }

    /// Open every turn of a thread ("Expand all tool lines").
    pub fn open_all(&mut self, key: &ThreadKey, t: &Transcript) {
        for item in t.items() {
            self.turns.insert((key.clone(), item.turn_id.clone()));
        }
    }

    /// Close every turn of a thread ("Collapse all tool lines").
    pub fn close_all(&mut self, key: &ThreadKey) {
        self.turns.retain(|(k, _)| k != key);
    }
}

/// "26s", "4m 12s", "1h 3m", "2d 4h": a turn's wall-clock duration.
pub fn worked_duration(seconds: i64) -> String {
    let s = seconds.max(0);
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        let (m, r) = (s / 60, s % 60);
        if r == 0 { format!("{m}m") } else { format!("{m}m {r}s") }
    } else if s < 86_400 {
        let (h, m) = (s / 3600, (s % 3600) / 60);
        if m == 0 { format!("{h}h") } else { format!("{h}h {m}m") }
    } else {
        let (d, h) = (s / 86_400, (s % 86_400) / 3600);
        if h == 0 { format!("{d}d") } else { format!("{d}d {h}h") }
    }
}

/// The line a finished turn folds into: "Worked for 26s", "Worked for 26s · interrupted",
/// "Worked for 26s · failed". Under a second, or with no times, it is just "Worked".
pub fn worked_text(f: &Finished) -> String {
    let mut s = match f.secs {
        Some(s) if s >= 1 => format!("Worked for {}", worked_duration(s)),
        _ => "Worked".to_owned(),
    };
    match f.outcome {
        Outcome::Completed => {}
        Outcome::Interrupted => s.push_str(" · interrupted"),
        Outcome::Failed => s.push_str(" · failed"),
    }
    s
}

/// An item that still waits on the user: a pending or submitted request. A turn holding one
/// is never finished.
fn waits_on_user(item: &Item) -> bool {
    matches!(&item.body, ItemBody::Request(r) if matches!(r.state, RequestState::Pending | RequestState::Submitted))
}

/// Whether a turn is finished, and how it ended. `None` while it runs or waits on the user.
pub fn finished(t: &Transcript, turn: &TurnId) -> Option<Finished> {
    if t.active_turn.as_ref() == Some(turn) {
        return None;
    }
    let items: Vec<&Item> = t.items().iter().filter(|i| &i.turn_id == turn).collect();
    if items.is_empty() || items.iter().any(|i| waits_on_user(i)) {
        return None;
    }
    // Something still under way (a command, streaming prose, a wait on another agent). A
    // request's own status does not count: its state says whether it waits.
    if items.iter().any(|i| i.status == ItemStatus::InProgress && !matches!(i.body, ItemBody::Request(_))) {
        return None;
    }
    let times = t.turns.get(turn);
    let end = times.and_then(|x| x.ended.as_ref());
    if end.is_none() {
        // No end seen: the newest turn counts as finished only once the transcript is loaded
        // and no turn is running.
        let last = t.items().last().map(|i| &i.turn_id);
        if last == Some(turn) && (!t.is_loaded() || t.active_turn.is_some()) {
            return None;
        }
    }
    let notice_error = items.iter().rev().find_map(|i| match &i.body {
        ItemBody::Notice(n) if n.notice_kind == NoticeKind::Error => Some(crate::message::one_line(&n.text)),
        _ => None,
    });
    let interrupted_notice = items.iter().any(|i| matches!(&i.body, ItemBody::Notice(n) if n.notice_kind == NoticeKind::InterruptedByUpdate));
    let outcome = match end.map(|e| &e.status) {
        Some(TurnStatus::Completed) => Outcome::Completed,
        Some(TurnStatus::Interrupted | TurnStatus::InterruptedByUpdate) => Outcome::Interrupted,
        Some(TurnStatus::Failed) => Outcome::Failed,
        Some(TurnStatus::Unknown(_)) | None if notice_error.is_some() => Outcome::Failed,
        Some(TurnStatus::Unknown(_)) | None if interrupted_notice => Outcome::Interrupted,
        Some(TurnStatus::Unknown(_)) | None => Outcome::Completed,
    };
    let error = match outcome {
        Outcome::Failed => end.and_then(|e| e.error.clone()).map(|e| crate::message::one_line(&e)).or(notice_error),
        _ => None,
    };
    // Wall-clock: from the turn's start (or its first timed item) to its end (or the last
    // time any of its items reports).
    let secs = |ts: &Timestamp| crate::time::secs(ts);
    let start = times.and_then(|x| x.started.as_ref()).and_then(secs).or_else(|| items.iter().find_map(|i| i.started_at.as_ref().and_then(secs)));
    let stop = end.and_then(|e| e.at.as_ref()).and_then(secs).or_else(|| {
        items.iter().filter_map(|i| i.completed_at.as_ref().or(i.started_at.as_ref()).and_then(secs)).max()
    });
    let secs = start.zip(stop).map(|(a, b)| b - a).filter(|s| *s >= 0);
    Some(Finished { outcome, secs, error })
}

/// The turn a block belongs to. A fork's inherited history belongs to none: it is never
/// folded.
pub fn block_turn<'a>(b: &Block, t: &'a Transcript) -> Option<&'a TurnId> {
    let of = |i: usize| t.items().get(i).map(|x| &x.turn_id);
    match b {
        Block::User { index } | Block::Task { index } | Block::Prose { index } | Block::Card { index } => of(*index),
        Block::Handoff { .. } => None,
        Block::Run { lines } => lines.first().and_then(|l| of(l.index)),
        Block::Folded { first, .. } => t.index_of(first).and_then(of),
        Block::TurnChanges { turn, .. } => t.items().iter().find(|i| &i.turn_id == turn).map(|i| &i.turn_id),
        Block::Worked { turn, .. } => t.items().iter().find(|i| &i.turn_id == turn).map(|i| &i.turn_id),
    }
}

/// The items a block shows.
fn block_ids<'a>(b: &'a Block, t: &'a Transcript) -> Vec<&'a ItemId> {
    match b {
        Block::User { index } | Block::Task { index } | Block::Prose { index } | Block::Card { index } | Block::Handoff { index } => {
            t.items().get(*index).map(|i| &i.id).into_iter().collect()
        }
        Block::Run { lines } => lines.iter().map(|l| &l.id).collect(),
        Block::Folded { first, .. } => vec![first],
        Block::TurnChanges { items, .. } => items.iter().collect(),
        Block::Worked { .. } => Vec::new(),
    }
}

/// Blocks that stay visible in a collapsed turn: the prompt, what needs the user, and the
/// summary of the files it changed.
fn always_shown(b: &Block, t: &Transcript) -> bool {
    match b {
        Block::User { .. } | Block::Task { .. } | Block::Handoff { .. } | Block::TurnChanges { .. } => true,
        Block::Card { index } => t.items().get(*index).is_some_and(|i| match &i.body {
            ItemBody::Request(_) => waits_on_user(i),
            // The budget notice needs the user.
            _ => true,
        }),
        _ => false,
    }
}

/// Fold every finished turn of a transcript's blocks into its "Worked for" line and its final
/// message. A turn stays open when the user opened it, when its files' "show all" is open, or
/// when the item just navigated to (`focus`) is inside its work.
pub fn collapse(t: &Transcript, key: &ThreadKey, blocks: Vec<Block>, opened: &Opened, expansion: &Expansion, focus: Option<&ItemId>) -> Vec<Block> {
    let mut out = Vec::with_capacity(blocks.len());
    let mut cache: BTreeMap<TurnId, Option<Finished>> = BTreeMap::new();
    let mut i = 0;
    while i < blocks.len() {
        let Some(turn) = block_turn(&blocks[i], t).cloned() else {
            out.push(blocks[i].clone());
            i += 1;
            continue;
        };
        // The consecutive blocks of this turn.
        let mut end = i + 1;
        while end < blocks.len() && block_turn(&blocks[end], t) == Some(&turn) {
            end += 1;
        }
        let group = &blocks[i..end];
        let fin = cache.entry(turn.clone()).or_insert_with(|| finished(t, &turn)).clone();
        i = end;
        let Some(fin) = fin else {
            out.extend(group.iter().cloned());
            continue;
        };
        let last_prose = group.iter().rposition(|b| matches!(b, Block::Prose { .. }));
        let lead = group.iter().take_while(|b| matches!(b, Block::User { .. } | Block::Task { .. })).count();
        let work: Vec<usize> = (lead..group.len()).filter(|&n| Some(n) != last_prose && !always_shown(&group[n], t)).collect();
        if work.is_empty() && fin.outcome == Outcome::Completed {
            out.extend(group.iter().cloned());
            continue;
        }
        let open = opened.is_open(key, &turn)
            || expansion.turns.contains(&(key.clone(), turn.clone()))
            || focus.is_some_and(|f| work.iter().any(|&n| block_ids(&group[n], t).contains(&f)));
        out.extend(group[..lead].iter().cloned());
        out.push(Block::Worked { text: worked_text(&fin), failed: fin.outcome == Outcome::Failed, error: fin.error.clone(), expanded: open, turn: turn.clone() });
        for (n, b) in group.iter().enumerate().skip(lead) {
            if open || !work.contains(&n) {
                out.push(b.clone());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo::build::{agent as say, approval, decision, notice, reasoning, tool, with_state};
    use crate::time::timestamp;
    use yaac_protocol::{DecisionKind, Epoch, InputPart, JournalPosition, Seq, TargetId, ThreadId, ToolKind, UserMessageItem};

    fn key() -> ThreadKey {
        ThreadKey::new(TargetId::new("local"), ThreadId::new("t"))
    }

    fn item(id: &str, turn: &str, status: ItemStatus, body: ItemBody, at: Option<(i64, i64)>) -> Item {
        Item {
            id: ItemId::new(id),
            turn_id: TurnId::new(turn),
            status,
            body,
            started_at: at.map(|(a, _)| timestamp(a)),
            completed_at: at.map(|(_, b)| timestamp(b)),
        }
    }

    fn user(id: &str, turn: &str, at: i64) -> Item {
        item(id, turn, ItemStatus::Completed, ItemBody::UserMessage(UserMessageItem { parts: vec![InputPart::text("Fix it")] }), Some((at, at)))
    }

    fn agent(id: &str, turn: &str, at: i64) -> Item {
        item(id, turn, ItemStatus::Completed, say("Done."), Some((at, at)))
    }

    fn thought(id: &str, turn: &str, a: i64, b: i64) -> Item {
        item(id, turn, ItemStatus::Completed, reasoning("Hm"), Some((a, b)))
    }

    fn command(id: &str, turn: &str, status: ItemStatus) -> Item {
        item(id, turn, status, tool(ToolKind::Command, "Ran `cargo test`", Some(0), "ok"), None)
    }

    fn transcript(items: Vec<Item>) -> Transcript {
        let mut t = Transcript::new();
        t.replace_all(items, 1);
        t.position = Some(JournalPosition { epoch: Epoch(1), seq: Seq(1) });
        t
    }

    fn shown(t: &Transcript, opened: &Opened) -> Vec<Block> {
        let state = state_with(t);
        let map = crate::attention::AttentionMap::compute(&state);
        let bl = crate::activity::blocks(&state, &map, &key(), &Expansion::default());
        collapse(t, &key(), bl, opened, &Expansion::default(), None)
    }

    fn state_with(t: &Transcript) -> crate::state::AppState {
        let mut s = crate::state::AppState::default();
        s.transcripts.insert(key(), t.clone());
        s
    }

    fn kinds(b: &[Block]) -> Vec<String> {
        b.iter()
            .map(|b| match b {
                Block::User { .. } => "user".to_owned(),
                Block::Prose { .. } => "prose".to_owned(),
                Block::Run { lines } => format!("run{}", lines.len()),
                Block::Worked { text, expanded, .. } => format!("{text}{}", if *expanded { " (open)" } else { "" }),
                Block::Card { .. } => "card".to_owned(),
                Block::TurnChanges { .. } => "changes".to_owned(),
                Block::Folded { .. } => "folded".to_owned(),
                Block::Task { .. } => "task".to_owned(),
                Block::Handoff { .. } => "handoff".to_owned(),
            })
            .collect()
    }

    fn worked_turn() -> Vec<Item> {
        vec![
            user("u", "t1", 1000),
            thought("r", "t1", 1001, 1004),
            command("c", "t1", ItemStatus::Completed),
            agent("m1", "t1", 1010),
            command("c2", "t1", ItemStatus::Completed),
            agent("m2", "t1", 1026),
        ]
    }

    #[test]
    fn durations_read_like_a_stopwatch() {
        assert_eq!(worked_duration(0), "0s");
        assert_eq!(worked_duration(26), "26s");
        assert_eq!(worked_duration(60), "1m");
        assert_eq!(worked_duration(252), "4m 12s");
        assert_eq!(worked_duration(3780), "1h 3m");
        assert_eq!(worked_duration(7200), "2h");
        assert_eq!(worked_duration(86_400 + 4 * 3600 + 59), "1d 4h");
        let f = |outcome, secs| worked_text(&Finished { outcome, secs, error: None });
        assert_eq!(f(Outcome::Completed, Some(26)), "Worked for 26s");
        assert_eq!(f(Outcome::Interrupted, Some(26)), "Worked for 26s · interrupted");
        assert_eq!(f(Outcome::Failed, Some(252)), "Worked for 4m 12s · failed");
        assert_eq!(f(Outcome::Completed, None), "Worked");
        assert_eq!(f(Outcome::Completed, Some(0)), "Worked");
    }

    #[test]
    fn a_running_turn_stays_expanded() {
        let mut t = transcript(worked_turn());
        t.active_turn = Some(TurnId::new("t1"));
        assert_eq!(kinds(&shown(&t, &Opened::default())), ["user", "run2", "prose", "run1", "prose"]);
        // Something still under way keeps it open even without a live turn.
        let mut items = worked_turn();
        items.push(command("c3", "t1", ItemStatus::InProgress));
        let t = transcript(items);
        assert!(finished(&t, &TurnId::new("t1")).is_none());
    }

    #[test]
    fn a_completed_turn_is_one_line_and_its_final_message() {
        let t = transcript(worked_turn());
        assert_eq!(kinds(&shown(&t, &Opened::default())), ["user", "Worked for 26s", "prose"]);
        let Block::Prose { index } = shown(&t, &Opened::default())[2] else { panic!() };
        assert_eq!(t.items()[index].id.as_str(), "m2", "the final message, not an interim one");
    }

    #[test]
    fn clicking_the_line_opens_the_work_and_again_closes_it() {
        let t = transcript(worked_turn());
        let mut o = Opened::default();
        o.toggle(&key(), &TurnId::new("t1"));
        assert_eq!(kinds(&shown(&t, &o)), ["user", "Worked for 26s (open)", "run2", "prose", "run1", "prose"]);
        o.toggle(&key(), &TurnId::new("t1"));
        assert_eq!(kinds(&shown(&t, &o)), ["user", "Worked for 26s", "prose"]);
        o.open_all(&key(), &t);
        assert!(o.is_open(&key(), &TurnId::new("t1")));
        o.close_all(&key());
        assert!(!o.is_open(&key(), &TurnId::new("t1")));
    }

    #[test]
    fn the_live_end_gives_the_outcome_and_the_duration() {
        let mut t = transcript(worked_turn());
        t.turns.insert(
            TurnId::new("t1"),
            TurnTimes { started: Some(timestamp(995)), ended: Some(TurnEnd { status: TurnStatus::Interrupted, at: Some(timestamp(1247)), error: None }) },
        );
        assert_eq!(kinds(&shown(&t, &Opened::default()))[1], "Worked for 4m 12s · interrupted");
        t.turns.get_mut(&TurnId::new("t1")).unwrap().ended = Some(TurnEnd { status: TurnStatus::Failed, at: Some(timestamp(1021)), error: Some("model overloaded".into()) });
        let b = shown(&t, &Opened::default());
        let Block::Worked { text, failed, error, .. } = &b[1] else { panic!() };
        assert_eq!(text, "Worked for 26s · failed");
        assert!(*failed);
        assert_eq!(error.as_deref(), Some("model overloaded"), "the error stays visible");
    }

    #[test]
    fn an_error_notice_marks_an_older_turn_failed() {
        let mut items = worked_turn();
        items.push(item("n", "t1", ItemStatus::Completed, notice(NoticeKind::Error, "Rate limited", None), None));
        let f = finished(&transcript(items), &TurnId::new("t1")).unwrap();
        assert_eq!(f.outcome, Outcome::Failed);
        assert_eq!(f.error.as_deref(), Some("Rate limited"));
    }

    #[test]
    fn a_turn_waiting_on_the_user_is_not_finished() {
        let mut items = worked_turn();
        let req = |state| with_state(approval("r1", "rm -rf target", "/w", vec![decision("allow", "Allow", DecisionKind::Allow)]), state, None);
        items.insert(3, item("q", "t1", ItemStatus::InProgress, req(RequestState::Pending), None));
        let t = transcript(items.clone());
        assert!(finished(&t, &TurnId::new("t1")).is_none());
        assert!(kinds(&shown(&t, &Opened::default())).contains(&"card".to_owned()));
        // Answered, it folds with the rest of the work.
        items[3] = item("q", "t1", ItemStatus::Completed, req(RequestState::Resolved), None);
        assert_eq!(kinds(&shown(&transcript(items), &Opened::default())), ["user", "Worked for 26s", "prose"]);
    }

    #[test]
    fn the_newest_turn_without_an_end_waits_for_the_transcript() {
        let mut t = transcript(worked_turn());
        t.position = None;
        assert!(finished(&t, &TurnId::new("t1")).is_none(), "not loaded yet");
        // An older turn is finished once a newer one exists.
        let mut items = worked_turn();
        items.push(user("u2", "t2", 2000));
        let mut t = transcript(items);
        t.active_turn = Some(TurnId::new("t2"));
        assert!(finished(&t, &TurnId::new("t1")).is_some());
        assert!(finished(&t, &TurnId::new("t2")).is_none());
    }

    #[test]
    fn a_turn_with_nothing_to_hide_is_left_alone() {
        let t = transcript(vec![user("u", "t1", 0), agent("m", "t1", 3)]);
        assert_eq!(kinds(&shown(&t, &Opened::default())), ["user", "prose"]);
    }

    #[test]
    fn the_item_navigated_to_opens_its_turn() {
        let t = transcript(worked_turn());
        let state = state_with(&t);
        let map = crate::attention::AttentionMap::compute(&state);
        let bl = crate::activity::blocks(&state, &map, &key(), &Expansion::default());
        let focus = ItemId::new("c2");
        let b = collapse(&t, &key(), bl, &Opened::default(), &Expansion::default(), Some(&focus));
        assert_eq!(kinds(&b)[1], "Worked for 26s (open)");
    }
}
