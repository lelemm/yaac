//! The transcript as the user reads it (docs/design/screens.md, "The transcript: the
//! conversation first"): the user's messages and the agent's prose at full size; everything
//! else is **activity** — one muted line each, consecutive lines sitting
//! together as a run, a long run folded into one summary line. What needs the user is never
//! collapsed: requests and the budget notice are the only cards. Reasoning with text opens
//! by default within its run.
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

/// Reasoning with no text is shown ("Thought for 3s") only when it took at least this many
/// seconds; a shorter one is not shown at all (D148).
pub const SILENT_THOUGHT_SHOWN_AFTER: i64 = 2;

/// Which lines and runs the user opened, per thread.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Expansion {
    /// Activity lines shown expanded.
    pub lines: BTreeSet<(ThreadKey, ItemId)>,
    /// Thoughts with text that the user closed.
    collapsed_thoughts: BTreeSet<(ThreadKey, ItemId)>,
    /// Folded runs opened (keyed by their first item).
    pub runs: BTreeSet<(ThreadKey, ItemId)>,
    /// One file of a file-change item, shown as its diff (item, file index).
    pub files: BTreeSet<(ThreadKey, ItemId, usize)>,
    /// A long diff with "show the rest" opened (item, file index).
    pub rest: BTreeSet<(ThreadKey, ItemId, usize)>,
    /// A turn's "show all": every diff of that turn expanded.
    pub turns: BTreeSet<(ThreadKey, TurnId)>,
    /// A pasted block of a user message opened (item, part index).
    pub pasted: BTreeSet<(ThreadKey, ItemId, usize)>,
    /// A user message's "Context" disclosure opened.
    pub context: BTreeSet<(ThreadKey, ItemId)>,
    /// An agent call's "Raw call" disclosure opened (D150).
    pub raw: BTreeSet<(ThreadKey, ItemId)>,
}

impl Expansion {
    /// Toggle one pasted block of a user message.
    pub fn toggle_pasted(&mut self, key: &ThreadKey, id: &ItemId, part: usize) {
        let k = (key.clone(), id.clone(), part);
        if !self.pasted.remove(&k) {
            self.pasted.insert(k);
        }
    }

    /// Toggle a user message's context disclosure.
    pub fn toggle_context(&mut self, key: &ThreadKey, id: &ItemId) {
        let k = (key.clone(), id.clone());
        if !self.context.remove(&k) {
            self.context.insert(k);
        }
    }

    /// Toggle an agent call's raw input and output.
    pub fn toggle_raw(&mut self, key: &ThreadKey, id: &ItemId) {
        let k = (key.clone(), id.clone());
        if !self.raw.remove(&k) {
            self.raw.insert(k);
        }
    }

    /// Toggle one activity line.
    pub fn toggle_line(&mut self, key: &ThreadKey, id: &ItemId) {
        let k = (key.clone(), id.clone());
        if !self.lines.remove(&k) {
            self.lines.insert(k);
        }
    }

    /// Toggle a thought, which starts out expanded.
    pub fn toggle_thought(&mut self, key: &ThreadKey, id: &ItemId) {
        let k = (key.clone(), id.clone());
        if !self.collapsed_thoughts.remove(&k) {
            self.collapsed_thoughts.insert(k);
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
    /// Another agent started by this one (an agent call, D150).
    Spawn,
    /// A stopped agent resumed with a message (an agent call).
    Resumed,
    /// A notice.
    Notice,
    /// The turn was stopped part-way (D167): a quiet stop mark.
    Interrupted,
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
    /// An agent call (D150): the line drawn with pills. `text` is its plain form.
    pub call: Option<crate::agentcall::CallLine>,
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
    /// The user's message, full size — or a `!command` the user ran (P346): its user message,
    /// or a user-run command item no such message took over. [`crate::shell_block::block`]
    /// tells them apart.
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
        /// Transcript index of the last item in the run.
        last_index: usize,
    },
    /// A finished turn's work folded into one line (D156): "Worked for 26s ›". Made by
    /// [`crate::turns::collapse`], never by [`blocks`].
    Worked {
        /// The turn.
        turn: TurnId,
        /// "Worked for 26s · interrupted".
        text: String,
        /// The turn failed: the line keeps the failed mark and colour.
        failed: bool,
        /// A failed turn's error, shown under the line even while collapsed.
        error: Option<String>,
        /// The work is shown under it.
        expanded: bool,
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
            if w.is_empty() { "failed".to_owned() } else { format!("failed · {w}") }
        }
        ReceiptStatus::OutcomeUnknown => crate::header::receipt_text(&r.status, r.reason.as_ref(), r.detail.as_deref(), &target),
        ReceiptStatus::Unknown(s) => s.clone(),
    }
}

/// The activity line an item becomes, or `None` when it is not activity.
pub fn line_of(state: &AppState, map: &AttentionMap, key: &ThreadKey, index: usize, item: &Item) -> Option<ActivityLine> {
    let in_progress = item.status == ItemStatus::InProgress;
    if let Some((_, call)) = crate::agentcall::of(item) {
        use yaac_protocol::{AgentCallKind as K, AgentCallOutcome as O};
        let line = crate::agentcall::line(state, map, key, state.transcripts.get(key), item, call);
        let failed = item.status == ItemStatus::Failed || call.outcome == Some(O::Failed);
        let mark = match (&call.kind, &call.outcome) {
            (K::Message, Some(O::Resumed)) | (K::Resume, _) => LineMark::Resumed,
            (K::Message, _) => LineMark::Sent,
            (K::Spawn, _) => LineMark::Spawn,
            (K::Wait, Some(O::Returned)) if !in_progress => LineMark::Received,
            (K::Wait, _) => LineMark::Waiting,
            _ => LineMark::Tool,
        };
        let text = line.plain();
        return Some(ActivityLine { index, id: item.id.clone(), mark, text, failed, in_progress, expanded: false, highlighted: false, diff: None, call: Some(line) });
    }
    let (mark, text, failed) = match &item.body {
        ItemBody::Reasoning(_) => {
            let took = item
                .started_at
                .as_ref()
                .zip(item.completed_at.as_ref())
                .and_then(|(a, b)| Some(crate::time::secs(b)? - crate::time::secs(a)?))
                .filter(|s| *s >= 0);
            // Reasoning with nothing to read (Codex's hidden reasoning has an empty summary;
            // a thinking block can be empty) is not a line of its own unless it took long
            // enough to explain a pause (D148). Under way, it has no duration yet: it shows
            // only once it has text.
            let has_text = thought_text(item).is_some();
            let text = match (in_progress, took) {
                (true, _) if has_text => "Thinking…".to_owned(),
                (false, Some(s)) if has_text || s >= SILENT_THOUGHT_SHOWN_AFTER => format!("Thought for {}", crate::time::duration(s)),
                (false, None) if has_text => "Thought".to_owned(),
                _ => return None,
            };
            (LineMark::Thought, text, false)
        }
        ItemBody::ToolCall(t) => {
            let failed = item.status == ItemStatus::Failed || t.exit_code.is_some_and(|c| c != 0);
            let mut text = plain(&t.title);
            // A skill's instructions were loaded (the harness confirmed it): say which skill.
            if let Some(skill) = &t.skill {
                text.push_str(&format!(" · {}", skill.name));
            }
            if failed {
                match t.exit_code {
                    Some(c) if c != 0 => text.push_str(&format!(" · failed (exit {c})")),
                    _ => text.push_str(" · failed"),
                }
            }
            if item.status == ItemStatus::Declined {
                text.push_str(" · declined");
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
        ItemBody::Notice(n) if n.notice_kind == NoticeKind::Interrupted => (LineMark::Interrupted, crate::message::one_line(&n.text), false),
        ItemBody::Notice(n) if n.notice_kind != NoticeKind::BudgetExhausted => {
            (LineMark::Notice, crate::message::one_line(&n.text), n.notice_kind == NoticeKind::Error)
        }
        ItemBody::Plan(p) => {
            let done = p.steps.iter().filter(|s| s.state == yaac_protocol::PlanStepState::Completed).count();
            (LineMark::Plan, format!("Plan · {done} of {} done", p.steps.len()), false)
        }
        ItemBody::Unknown(u) => (LineMark::Other, format!("{} (not shown by this version)", u.tag("kind")), false),
        _ => return None,
    };
    Some(ActivityLine { index, id: item.id.clone(), mark, text, failed, in_progress, expanded: false, highlighted: false, diff: None, call: None })
}

/// A shell command that failed, including providers that only report a nonzero exit.
pub fn failed_command(item: &Item) -> bool {
    matches!(&item.body, ItemBody::ToolCall(t) if t.tool == ToolKind::Command
        && (item.status == ItemStatus::Failed || t.exit_code.is_some_and(|code| code != 0)))
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
    let commands = lines.iter().filter(|l| l.mark == LineMark::Tool && l.call.is_none()).count();
    let messages = lines.iter().filter(|l| l.call.is_some() || matches!(l.mark, LineMark::Sent | LineMark::Received)).count();
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
    let emit_run = |lines: Vec<ActivityLine>, out: &mut Vec<Block>, fold: bool| {
        let Some(first) = lines.first().map(|l| l.id.clone()) else { return };
        let opened = expansion.runs.contains(&(key.clone(), first.clone()))
            || lines.iter().any(|l| l.highlighted || l.expanded && l.mark != LineMark::Thought);
        if fold && lines.len() > 1 && !opened {
            out.push(Block::Folded {
                text: fold_summary(&lines),
                failed: lines.iter().any(|l| l.failed),
                count: lines.len(),
                last_index: lines.last().expect("nonempty activity run").index,
                first,
            });
        } else {
            out.push(Block::Run { lines });
        }
    };
    let flush = |run: &mut Vec<ActivityLine>, out: &mut Vec<Block>| {
        // Splitting a long run must keep its smaller groups folded, too.
        let fold = run.len() > FOLD_AFTER;
        if !run.iter().any(|line| line.failed) {
            emit_run(std::mem::take(run), out, fold);
            return;
        }
        let mut group = Vec::new();
        for line in std::mem::take(run) {
            if line.failed && failed_command(&t.items()[line.index]) {
                emit_run(std::mem::take(&mut group), out, fold);
                out.push(Block::Run { lines: vec![line] });
            } else {
                group.push(line);
            }
        }
        emit_run(group, out, fold);
    };
    let mut seen_prompt = false;
    // Mesh items that only echo an agent call's line.
    let echoes = crate::agentcall::Echoes::of(t.items());
    // The items a `!command` block takes over: its toolCall and Claude Code's output (P346).
    let absorbed = crate::shell_block::absorbed(t.items());
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
        if echoes.hides(item) || absorbed[index] {
            continue;
        }
        if turn.as_ref().is_none_or(|(id, _, _)| *id != item.turn_id) {
            end_turn(&mut turn, &mut run, &mut out);
            // A run never spans two turns, so a finished turn can fold on its own (D156).
            flush(&mut run, &mut out);
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
            // A user-run command no `!command` message took over (Codex history): its own
            // command block, where the user's message would be (P346).
            ItemBody::ToolCall(c) if c.source == Some(yaac_protocol::ToolCallSource::UserShell) => {
                seen_prompt = true;
                Some(Block::User { index })
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
                            call: None,
                        });
                    }
                } else if let Some(mut line) = line_of(state, map, key, index, item) {
                    line.highlighted = focus == Some(&item.id);
                    let entry = (key.clone(), item.id.clone());
                    line.expanded = if line.mark == LineMark::Thought {
                        thought_text(item).is_some() && !expansion.collapsed_thoughts.contains(&entry)
                    } else {
                        line.highlighted || expansion.lines.contains(&entry)
                    };
                    run.push(line);
                }
            }
        }
    }
    end_turn(&mut turn, &mut run, &mut out);
    flush(&mut run, &mut out);
    out
}

/// The original Markdown of a thought with text.
pub fn thought_text(item: &Item) -> Option<&str> {
    match &item.body {
        ItemBody::Reasoning(r) if !r.reasoning.trim().is_empty() => Some(&r.reasoning),
        _ => None,
    }
}

/// "Expand all tool lines": every folded run of the thread opens and every activity line in
/// it is expanded (file diffs keep their own state: a turn's "show all" opens those).
pub fn expand_all(state: &AppState, map: &AttentionMap, key: &ThreadKey, expansion: &mut Expansion) {
    expansion.collapsed_thoughts.retain(|(k, _)| k != key);
    for b in blocks(state, map, key, expansion) {
        if let Block::Folded { first, .. } = b {
            expansion.runs.insert((key.clone(), first));
        }
    }
    for b in blocks(state, map, key, expansion) {
        if let Block::Run { lines } = b {
            for l in lines.into_iter().filter(|l| l.diff.is_none() && l.mark != LineMark::Thought) {
                expansion.lines.insert((key.clone(), l.id));
            }
        }
    }
}

/// "Collapse all tool lines": the thread's currently loaded lines, runs and diffs collapse.
pub fn collapse_all(state: &AppState, key: &ThreadKey, expansion: &mut Expansion) {
    if let Some(t) = state.transcripts.get(key) {
        expansion.collapsed_thoughts.extend(t.items().iter()
            .filter(|item| thought_text(item).is_some())
            .map(|item| (key.clone(), item.id.clone())));
    }
    expansion.lines.retain(|(k, _)| k != key);
    expansion.runs.retain(|(k, _)| k != key);
    expansion.files.retain(|(k, _, _)| k != key);
    expansion.rest.retain(|(k, _, _)| k != key);
    expansion.turns.retain(|(k, _)| k != key);
    expansion.raw.retain(|(k, _)| k != key);
}

#[cfg(test)]
mod tests {
    use super::*;
    use yaac_protocol::{ReasoningItem, TargetId, ThreadId};

    /// The lines one reasoning item becomes, alone in a transcript.
    fn lines_of(text: &str, status: ItemStatus, took: Option<i64>) -> Vec<String> {
        let key = ThreadKey::new(TargetId::new("local"), ThreadId::new("t"));
        let item = Item {
            id: ItemId::new("r1"),
            turn_id: TurnId::new("t1"),
            status,
            body: ItemBody::Reasoning(ReasoningItem { reasoning: text.to_owned(), summary: true }),
            started_at: Some(crate::time::timestamp(1_000)),
            completed_at: took.map(|s| crate::time::timestamp(1_000 + s)),
        };
        let mut t = crate::transcript::Transcript::new();
        t.upsert(item, 1);
        let mut state = AppState::default();
        state.transcripts.insert(key.clone(), t);
        let map = AttentionMap::compute(&state);
        blocks(&state, &map, &key, &Expansion::default())
            .into_iter()
            .flat_map(|b| match b {
                Block::Run { lines } => lines.into_iter().map(|l| l.text).collect(),
                _ => Vec::new(),
            })
            .collect()
    }

    #[test]
    fn empty_reasoning_is_not_a_line() {
        assert!(lines_of("", ItemStatus::Completed, Some(0)).is_empty());
        assert!(lines_of("  \n\n ", ItemStatus::Completed, Some(1)).is_empty());
        assert!(lines_of("", ItemStatus::Completed, None).is_empty());
    }

    #[test]
    fn empty_reasoning_that_took_a_while_reads_as_a_pause() {
        assert_eq!(lines_of("", ItemStatus::Completed, Some(SILENT_THOUGHT_SHOWN_AFTER)), ["Thought for 2s"]);
        assert_eq!(lines_of(" ", ItemStatus::Completed, Some(75)), ["Thought for 1m"]);
    }

    #[test]
    fn reasoning_with_text_keeps_its_line() {
        assert_eq!(lines_of("Check the prefix.", ItemStatus::Completed, Some(0)), ["Thought for 0s"]);
        assert_eq!(lines_of("Check the prefix.", ItemStatus::Completed, Some(4)), ["Thought for 4s"]);
        assert_eq!(lines_of("Check the prefix.", ItemStatus::Completed, None), ["Thought"]);
    }

    #[test]
    fn reasoning_under_way_has_no_duration() {
        assert_eq!(lines_of("Check the", ItemStatus::InProgress, None), ["Thinking…"]);
        // Nothing to read yet: no line, and never "Thought for 0s".
        assert!(lines_of("", ItemStatus::InProgress, None).is_empty());
        assert!(lines_of("", ItemStatus::InProgress, Some(0)).is_empty());
    }

    #[test]
    fn a_confirmed_skill_load_names_the_skill_and_other_tools_do_not() {
        use yaac_protocol::{SkillLoad, ToolCallItem, ToolKind};
        let tool = |title: &str, skill: Option<&str>| ToolCallItem {
            tool: ToolKind::Other,
            title: title.to_owned(),
            input: None,
            output: String::new(),
            output_truncated: false,
            output_bytes: None,
            exit_code: None,
            cwd: None,
            agent_call: None,
            skill: skill.map(|n| SkillLoad { name: n.to_owned() }),
            images: vec![],
            source: None,
            duration_ms: None,
        };
        let key = ThreadKey::new(TargetId::new("local"), ThreadId::new("t"));
        let mut t = crate::transcript::Transcript::new();
        for (n, body) in [("a", tool("Skill loaded", Some("review"))), ("b", tool("Read skill instructions", Some("plan"))), ("c", tool("Skill", None))] {
            let item = Item { id: ItemId::new(n), turn_id: TurnId::new("t1"), status: ItemStatus::Completed, body: ItemBody::ToolCall(body), started_at: None, completed_at: None };
            t.upsert(item, 1);
        }
        let mut state = AppState::default();
        state.transcripts.insert(key.clone(), t);
        let map = AttentionMap::compute(&state);
        let lines: Vec<String> = blocks(&state, &map, &key, &Expansion::default())
            .into_iter()
            .flat_map(|b| match b {
                Block::Run { lines } => lines.into_iter().map(|l| l.text).collect(),
                _ => Vec::new(),
            })
            .collect();
        assert_eq!(lines, ["Skill loaded · review", "Read skill instructions · plan", "Skill"]);
    }
}
