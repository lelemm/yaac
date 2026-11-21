//! The transcript as the user reads it (docs/design/screens.md, "The transcript: the
//! conversation first"): the user's messages and the agent's prose at full size; everything
//! else is **activity** — one muted line each, collapsed by default, consecutive lines sitting
//! together as a run, a long run folded into one summary line. What needs the user is never
//! collapsed: requests and the budget notice are the only cards.
//!
//! Expansion is view state kept in [`Expansion`]; this module decides what each item becomes.

use std::collections::BTreeSet;

use yaac_protocol::{
    Item, ItemBody, ItemId, ItemStatus, MeshPeer, NoticeKind, ReceiptReason, ReceiptStatus, ThreadState, ToolKind, TurnId,
};

use crate::diff::{FileStat, TurnTotals};

use crate::attention::{AttentionMap, Mark, display};
use crate::key::ThreadKey;
use crate::state::AppState;

/// Runs longer than this fold into one summary line.
pub const FOLD_AFTER: usize = 6;

/// Which lines and runs the user opened, per thread.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Expansion {
    /// Activity lines shown expanded.
    pub lines: BTreeSet<(ThreadKey, ItemId)>,
    /// Folded runs opened (keyed by their first item).
    pub runs: BTreeSet<(ThreadKey, ItemId)>,
    /// One file of a file-change item, shown as its diff (item, file index).
    pub files: BTreeSet<(ThreadKey, ItemId, usize)>,
    /// A long diff with "show the rest" opened (item, file index).
    pub rest: BTreeSet<(ThreadKey, ItemId, usize)>,
    /// A turn's "show all": every diff of that turn expanded.
    pub turns: BTreeSet<(ThreadKey, TurnId)>,
}

impl Expansion {
    /// Toggle one activity line.
    pub fn toggle_line(&mut self, key: &ThreadKey, id: &ItemId) {
        let k = (key.clone(), id.clone());
        if !self.lines.remove(&k) {
            self.lines.insert(k);
        }
    }

    /// Toggle one file's diff.
    pub fn toggle_file(&mut self, key: &ThreadKey, id: &ItemId, file: usize) {
        let k = (key.clone(), id.clone(), file);
        if !self.files.remove(&k) {
            self.files.insert(k);
        }
    }

    /// Open the rest of a long diff.
    pub fn open_rest(&mut self, key: &ThreadKey, id: &ItemId, file: usize) {
        self.rest.insert((key.clone(), id.clone(), file));
    }

    /// Toggle a turn's "show all". Closing it also closes the diffs opened one by one in that
    /// turn, so the turn returns to its collapsed form.
    pub fn toggle_turn(&mut self, key: &ThreadKey, turn: &TurnId, items: &[ItemId]) {
        let k = (key.clone(), turn.clone());
        if !self.turns.remove(&k) {
            self.turns.insert(k);
        } else {
            self.files.retain(|(tk, id, _)| tk != key || !items.contains(id));
        }
    }

    /// Toggle a folded run.
    pub fn toggle_run(&mut self, key: &ThreadKey, first: &ItemId) {
        let k = (key.clone(), first.clone());
        if !self.runs.remove(&k) {
            self.runs.insert(k);
        }
    }
}

/// The small mark at the start of an activity line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineMark {
    /// Reasoning.
    Thought,
    /// A tool call.
    Tool,
    /// A message sent to another agent (→).
    Sent,
    /// A message received from another agent (←).
    Received,
    /// Workers started.
    Workers,
    /// Waiting on other agents.
    Waiting,
    /// A notice.
    Notice,
    /// The plan.
    Plan,
    /// Something this build does not know.
    Other,
}

/// One muted activity line.
#[derive(Clone, Debug, PartialEq)]
pub struct ActivityLine {
    /// Index of the item in the transcript.
    pub index: usize,
    /// The item.
    pub id: ItemId,
    /// Its mark.
    pub mark: LineMark,
    /// The short description.
    pub text: String,
    /// Something went wrong: not muted, failed mark and colour.
    pub failed: bool,
    /// Still under way (spinner).
    pub in_progress: bool,
    /// Expanded in place.
    pub expanded: bool,
    /// The item just navigated to (highlighted).
    pub highlighted: bool,
    /// A file of a file change: its index in the item's diff and its counts. The line reads
    /// "Edited src/frame.rs" followed by `+12 −3`.
    pub diff: Option<DiffPart>,
}

/// One file of a file-change item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiffPart {
    /// The file's index in the item's diff.
    pub file: usize,
    /// Lines added.
    pub added: u32,
    /// Lines removed.
    pub removed: u32,
}

/// A block of the transcript.
#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    /// The user's message, full size.
    User {
        /// Item index.
        index: usize,
    },
    /// The message that set a spawned thread's task, full size where a user message would
    /// be, labelled "From …".
    Task {
        /// Item index.
        index: usize,
    },
    /// Agent prose, full size.
    Prose {
        /// Item index.
        index: usize,
    },
    /// A card that needs (or needed) the user: a request, or the budget notice.
    Card {
        /// Item index.
        index: usize,
    },
    /// Inherited history of a fork, with its divider.
    Handoff {
        /// Item index.
        index: usize,
    },
    /// A run of activity lines, sitting tight together.
    Run {
        /// The lines.
        lines: Vec<ActivityLine>,
    },
    /// The end of a turn that changed several files: "Changed 3 files  +40 −12 · show all".
    TurnChanges {
        /// The turn.
        turn: TurnId,
        /// Files and lines.
        totals: TurnTotals,
        /// "show all" is open.
        expanded: bool,
        /// The file-change items of the turn.
        items: Vec<ItemId>,
    },
    /// A long run, folded: "12 steps · 9 commands, 3 messages".
    Folded {
        /// The first item (the run's identity).
        first: ItemId,
        /// The summary.
        text: String,
        /// It contains a failure.
        failed: bool,
        /// How many lines it holds.
        count: usize,
    },
}

fn peer_title(p: &MeshPeer) -> String {
    p.title.clone().unwrap_or_else(|| p.address.to_string())
}

fn clip(text: &str, max: usize) -> String {
    let one = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= max {
        return one;
    }
    let mut s: String = one.chars().take(max).collect();
    s.push('…');
    s
}

/// Backticks are markdown, not text, in a one-line description.
fn plain(s: &str) -> String {
    s.replace('`', "")
}

/// The short receipt of a sent message: "delivered", "queued, staging is offline",
/// "failed — that thread was deleted on staging".
pub fn short_receipt(sent: &yaac_protocol::MeshSentItem) -> String {
    let target = sent.to.target_name.clone().unwrap_or_default();
    let r = &sent.receipt;
    let why = match &r.reason {
        Some(ReceiptReason::TargetOffline) => format!("{target} is offline"),
        Some(ReceiptReason::ThreadDeleted) => format!("that thread was deleted on {target}"),
        Some(ReceiptReason::UnknownAddress) => "there is no such thread".to_owned(),
        Some(ReceiptReason::RecipientBusy) => "the recipient is busy".to_owned(),
        Some(ReceiptReason::AwaitingUser) => "the recipient is waiting for you".to_owned(),
        Some(ReceiptReason::InUseElsewhere) => "another program holds it".to_owned(),
        Some(ReceiptReason::BudgetExhausted) => "the budget ran out".to_owned(),
        Some(ReceiptReason::HubUnavailable) => "no hub is running".to_owned(),
        Some(ReceiptReason::LimitExceeded) => "a limit was exceeded".to_owned(),
        Some(ReceiptReason::PolicyDenied) => "the policy refused it".to_owned(),
        Some(ReceiptReason::RemoteDisallowed) => format!("{target} accepts no messages from other machines"),
        Some(ReceiptReason::Stopped) => "the thread was stopped".to_owned(),
        Some(ReceiptReason::NotBeneathSender) => "it was queued: the thread is not beneath the sender".to_owned(),
        Some(ReceiptReason::Unknown(s)) => s.clone(),
        None => String::new(),
    };
    match &r.status {
        ReceiptStatus::Delivered => match &r.detail {
            Some(d) => format!("delivered, {d}"),
            None => "delivered".to_owned(),
        },
        ReceiptStatus::Queued => {
            let w = r.detail.clone().unwrap_or(why);
            if w.is_empty() { "queued".to_owned() } else { format!("queued, {w}") }
        }
        ReceiptStatus::Failed => {
            let w = r.detail.clone().unwrap_or(why);
            if w.is_empty() { "failed".to_owned() } else { format!("failed — {w}") }
        }
        ReceiptStatus::Unknown(s) => s.clone(),
    }
}

/// The activity line an item becomes, or `None` when it is not activity.
fn line_of(state: &AppState, map: &AttentionMap, index: usize, item: &Item) -> Option<ActivityLine> {
    let in_progress = item.status == ItemStatus::InProgress;
    let (mark, text, failed) = match &item.body {
        ItemBody::Reasoning(_) => {
            let took = item
                .started_at
                .as_ref()
                .zip(item.completed_at.as_ref())
                .and_then(|(a, b)| Some(crate::time::secs(b)? - crate::time::secs(a)?))
                .filter(|s| *s >= 0);
            let text = match (in_progress, took) {
                (true, _) => "Thinking…".to_owned(),
                (false, Some(s)) => format!("Thought for {}", crate::time::duration(s)),
                (false, None) => "Thought".to_owned(),
            };
            (LineMark::Thought, text, false)
        }
        ItemBody::ToolCall(t) => {
            let failed = item.status == ItemStatus::Failed || t.exit_code.is_some_and(|c| c != 0);
            let mut text = plain(&t.title);
            if failed {
                match t.exit_code {
                    Some(c) if c != 0 => text.push_str(&format!(" — failed (exit {c})")),
                    _ => text.push_str(" — failed"),
                }
            }
            if item.status == ItemStatus::Declined {
                text.push_str(" — declined");
            }
            (LineMark::Tool, text, failed)
        }
        ItemBody::MeshSent(s) => {
            let failed = s.receipt.status == ReceiptStatus::Failed;
            let receipt = if in_progress { "sending…".to_owned() } else { short_receipt(s) };
            (LineMark::Sent, format!("{} · {receipt}", peer_title(&s.to)), failed)
        }
        ItemBody::MeshReceived(r) => (LineMark::Received, format!("{} · {}", peer_title(&r.from), clip(&plain(&r.message.text), 60)), false),
        ItemBody::MeshSpawn(s) => {
            let n = s.threads.len();
            let finished = s
                .threads
                .iter()
                .filter(|t| match state.key_of(&t.peer.address) {
                    Some(k) => matches!(display(state, map, &k).mark, Mark::Done | Mark::Idle),
                    None => matches!(t.state, ThreadState::Idle),
                })
                .count();
            let failed = s.threads.iter().any(|t| t.error.is_some());
            let noun = if n == 1 { "worker" } else { "workers" };
            (LineMark::Workers, format!("Started {n} {noun} · {finished} of {n} finished"), failed)
        }
        ItemBody::MeshWait(w) => {
            let names: Vec<String> = w.targets.iter().map(|t| peer_title(&t.peer)).collect();
            let what = if names.is_empty() { "a message".to_owned() } else { names.join(", ") };
            let text = if in_progress { format!("Waiting for {what}…") } else { format!("Waited for {what}") };
            (LineMark::Waiting, text, false)
        }
        ItemBody::Notice(n) if n.notice_kind != NoticeKind::BudgetExhausted => {
            (LineMark::Notice, n.text.clone(), n.notice_kind == NoticeKind::Error)
        }
        ItemBody::Plan(p) => {
            let done = p.steps.iter().filter(|s| s.state == yaac_protocol::PlanStepState::Completed).count();
            (LineMark::Plan, format!("Plan · {done} of {} done", p.steps.len()), false)
        }
        ItemBody::Unknown(u) => (LineMark::Other, format!("{} (not shown by this version)", u.tag("kind")), false),
        _ => return None,
    };
    Some(ActivityLine { index, id: item.id.clone(), mark, text, failed, in_progress, expanded: false, highlighted: false, diff: None })
}

/// The files of a file-change item whose output is a diff.
pub fn file_stats(item: &Item) -> Option<Vec<FileStat>> {
    match &item.body {
        ItemBody::ToolCall(t) if t.tool == ToolKind::FileChange && item.status != ItemStatus::Failed => crate::diff::stats(&t.output),
        _ => None,
    }
}

/// A turn that changed at least this many files ends with a summary line. (One file's own
/// line already says everything the summary would.)
pub const TURN_SUMMARY_FILES: usize = 2;

fn fold_summary(lines: &[ActivityLine]) -> String {
    let commands = lines.iter().filter(|l| l.mark == LineMark::Tool).count();
    let messages = lines.iter().filter(|l| matches!(l.mark, LineMark::Sent | LineMark::Received)).count();
    let mut parts = Vec::new();
    if commands > 0 {
        parts.push(format!("{commands} {}", if commands == 1 { "command" } else { "commands" }));
    }
    if messages > 0 {
        parts.push(format!("{messages} {}", if messages == 1 { "message" } else { "messages" }));
    }
    if parts.is_empty() {
        format!("{} steps", lines.len())
    } else {
        format!("{} steps · {}", lines.len(), parts.join(", "))
    }
}

/// The blocks of a thread's transcript.
pub fn blocks(state: &AppState, map: &AttentionMap, key: &ThreadKey, expansion: &Expansion) -> Vec<Block> {
    let Some(t) = state.transcripts.get(key) else { return Vec::new() };
    let spawned = state.summary(key).is_some_and(|s| s.spawned_by.is_some());
    let focus = state.focus_item.as_ref().filter(|(k, _)| k == key).map(|(_, id)| id);
    let mut out = Vec::new();
    let mut run: Vec<ActivityLine> = Vec::new();
    let flush = |run: &mut Vec<ActivityLine>, out: &mut Vec<Block>| {
        if run.is_empty() {
            return;
        }
        let lines = std::mem::take(run);
        let first = lines[0].id.clone();
        let opened = expansion.runs.contains(&(key.clone(), first.clone()))
            || lines.iter().any(|l| l.highlighted || l.expanded);
        if lines.len() > FOLD_AFTER && !opened {
            out.push(Block::Folded {
                text: fold_summary(&lines),
                failed: lines.iter().any(|l| l.failed),
                count: lines.len(),
                first,
            });
        } else {
            out.push(Block::Run { lines });
        }
    };
    let mut seen_prompt = false;
    // The turn being read, with the files it changed so far.
    let mut turn: Option<(TurnId, TurnTotals, Vec<ItemId>)> = None;
    let end_turn = |turn: &mut Option<(TurnId, TurnTotals, Vec<ItemId>)>, run: &mut Vec<ActivityLine>, out: &mut Vec<Block>| {
        if let Some((id, totals, items)) = turn.take()
            && totals.files >= TURN_SUMMARY_FILES
        {
            flush(run, out);
            let expanded = expansion.turns.contains(&(key.clone(), id.clone()));
            out.push(Block::TurnChanges { turn: id, totals, expanded, items });
        }
    };
    for (index, item) in t.items().iter().enumerate() {
        if turn.as_ref().is_none_or(|(id, _, _)| *id != item.turn_id) {
            end_turn(&mut turn, &mut run, &mut out);
            turn = Some((item.turn_id.clone(), TurnTotals::default(), Vec::new()));
        }
        let block = match &item.body {
            ItemBody::UserMessage(_) => {
                seen_prompt = true;
                Some(Block::User { index })
            }
            ItemBody::MeshReceived(_) if spawned && !seen_prompt => {
                seen_prompt = true;
                Some(Block::Task { index })
            }
            ItemBody::AgentMessage(_) => Some(Block::Prose { index }),
            ItemBody::Request(_) => Some(Block::Card { index }),
            ItemBody::Notice(n) if n.notice_kind == NoticeKind::BudgetExhausted => Some(Block::Card { index }),
            ItemBody::Handoff(_) => Some(Block::Handoff { index }),
            _ => None,
        };
        match block {
            Some(b) => {
                flush(&mut run, &mut out);
                out.push(b);
            }
            None => {
                if let Some(files) = file_stats(item) {
                    // A file change: one line per file, each its own diff.
                    let highlighted = focus == Some(&item.id);
                    let all = expansion.turns.contains(&(key.clone(), item.turn_id.clone()));
                    for (n, f) in files.iter().enumerate() {
                        if let Some((_, totals, items)) = &mut turn {
                            totals.add(f);
                            if !items.contains(&item.id) {
                                items.push(item.id.clone());
                            }
                        }
                        let expanded = all || (highlighted && n == 0) || expansion.files.contains(&(key.clone(), item.id.clone(), n));
                        run.push(ActivityLine {
                            index,
                            id: item.id.clone(),
                            mark: LineMark::Tool,
                            text: f.title.clone(),
                            failed: false,
                            in_progress: item.status == ItemStatus::InProgress,
                            expanded,
                            highlighted,
                            diff: Some(DiffPart { file: n, added: f.added, removed: f.removed }),
                        });
                    }
                } else if let Some(mut line) = line_of(state, map, index, item) {
                    line.highlighted = focus == Some(&item.id);
                    line.expanded = line.highlighted || expansion.lines.contains(&(key.clone(), item.id.clone()));
                    run.push(line);
                }
            }
        }
    }
    end_turn(&mut turn, &mut run, &mut out);
    flush(&mut run, &mut out);
    out
}
