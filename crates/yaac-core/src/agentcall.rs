//! Agent calls in the transcript (D150): a tool call that is communication with another agent
//! — a YAAC mesh tool, a harness's own sub-agent tool — shown as one calm line with the other
//! agent as a **pill** (harness logo, title, machine), the message or task as a preview, and a
//! status word, instead of the tool's name and its raw JSON.
//!
//! Every pill is resolved here, in one place ([`resolve`]): a YAAC address to its thread; a
//! harness's own sub-agent id to the thread YAAC keeps for it, beneath the calling thread
//! ([`resolve_subagent`], D151); "its parent" to the caller's parent. An unresolved pill is
//! drawn the same way, not clickable, with the call's own name or a short id.
//!
//! YAAC's mesh tools also leave mesh items in the transcript (`meshSent`, `meshReceived`,
//! `meshSpawn`, `meshWait`). Those that are a call's own echo are folded into the call's line
//! ([`Echoes`]) so the conversation between agents reads once.

use std::collections::BTreeSet;

use yaac_protocol::{
    AgentCall, AgentCallKind, AgentCallOutcome, AgentRef, Item, ItemBody, ItemId, ItemStatus, MessageId, ReceiptStatus, ToolCallItem,
};

use crate::attention::{AttentionMap, Mark, display};
use crate::key::ThreadKey;
use crate::state::AppState;
use crate::transcript::Transcript;

/// The other agent of a call, as a pill.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pill {
    /// Its title, the call's name for it, or a short id.
    pub label: String,
    /// Provider kind, for the harness logo (`None`: no logo is known).
    pub kind: Option<String>,
    /// Its machine, when it is not the caller's.
    pub machine: Option<String>,
    /// The thread it opens; `None`: not clickable.
    pub key: Option<ThreadKey>,
    /// The item to land on in that thread (the message's other end), when known.
    pub at: Option<ItemId>,
}

/// What an agent-call line shows, in reading order:
/// `[mark] lead [pill] [pill] after  "preview"  meta · status`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallLine {
    /// What the call does, before the pills ("Started", "Waiting for"); empty for a message
    /// (its arrow says it).
    pub lead: String,
    /// The other agents.
    pub pills: Vec<Pill>,
    /// Words after the pills ("reported back").
    pub after: String,
    /// The message or task, one line (the widget ellipsises it), quoted.
    pub preview: Option<String>,
    /// Quiet facts ("gpt-6.2-sol · medium · background").
    pub meta: Option<String>,
    /// How it went: "delivered", "queued · it is busy", "failed · …", "running".
    pub status: Option<String>,
}

impl CallLine {
    /// The line as plain text (copy, fold summaries, tests).
    pub fn plain(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        let mut head = self.lead.clone();
        let pills: Vec<String> = self.pills.iter().map(Pill::plain).collect();
        if !pills.is_empty() {
            if !head.is_empty() {
                head.push(' ');
            }
            head.push_str(&pills.join(", "));
        }
        if !self.after.is_empty() {
            head.push(' ');
            head.push_str(&self.after);
        }
        parts.push(head.trim().to_owned());
        if let Some(p) = &self.preview {
            parts.push(format!("\u{201c}{p}\u{201d}"));
        }
        if let Some(m) = &self.meta {
            parts.push(m.clone());
        }
        if let Some(s) = &self.status {
            parts.push(s.clone());
        }
        parts.retain(|p| !p.is_empty());
        parts.join(" · ")
    }
}

impl Pill {
    /// "billing-service · devbox".
    pub fn plain(&self) -> String {
        match &self.machine {
            Some(m) => format!("{} · {m}", self.label),
            None => self.label.clone(),
        }
    }
}

/// The agent call of an item, if it is one.
pub fn of(item: &Item) -> Option<(&ToolCallItem, &AgentCall)> {
    match &item.body {
        ItemBody::ToolCall(t) => t.agent_call.as_ref().map(|c| (t, c)),
        _ => None,
    }
}

/// The thread YAAC keeps for a harness's own sub-agent (D151): a thread beneath `caller`
/// whose harness id is `agent_id`. The one place that answers this, so that every pill (and
/// whatever else needs it) agrees.
pub fn resolve_subagent(state: &AppState, caller: &ThreadKey, agent_id: &str) -> Option<ThreadKey> {
    let parent = caller.address();
    state.threads.iter().find_map(|(k, rec)| {
        let s = &rec.summary;
        let beneath = s.spawned_by.as_ref() == Some(&parent);
        // The harness's id for the agent as yaacd reports it (`subagent.agentId`), else its
        // conversation id (a Codex sub-agent's thread id is both).
        let same = s.subagent.as_ref().and_then(|sub| sub.agent_id.as_deref()) == Some(agent_id)
            || s.native_id.as_deref() == Some(agent_id)
            || k.thread.as_str() == agent_id;
        (beneath && same).then(|| k.clone())
    })
}

/// The thread a call's peer is, when YAAC knows it.
pub fn resolve(state: &AppState, caller: &ThreadKey, r: &AgentRef) -> Option<ThreadKey> {
    if let Some(a) = &r.address {
        return state.key_of(a);
    }
    if r.parent {
        return state.summary(caller).and_then(|s| s.spawned_by.as_ref()).and_then(|a| state.key_of(a));
    }
    r.agent_id.as_deref().and_then(|id| resolve_subagent(state, caller, id))
}

/// Eight characters of an id: the thread part of an address.
pub fn short_id(id: &str) -> String {
    let tail = id.rsplit('/').next().unwrap_or(id);
    tail.chars().take(8).collect()
}

/// A peer as a pill.
pub fn pill(state: &AppState, caller: &ThreadKey, r: &AgentRef) -> Pill {
    let key = resolve(state, caller, r);
    let harness_own = r.address.is_none();
    match &key {
        Some(k) => Pill {
            label: state.summary(k).map(|s| s.title.clone()).filter(|t| !t.is_empty()).unwrap_or_else(|| short_id(k.thread.as_str())),
            kind: state.provider_of(k).map(|p| p.kind.to_string()),
            machine: (k.target != caller.target).then(|| state.target_name(&k.target)),
            key,
            at: None,
        },
        None => {
            let label = if r.parent {
                r.name.clone().unwrap_or_else(|| "its parent".to_owned())
            } else {
                r.name.clone().or_else(|| r.agent_id.as_deref().map(short_id)).or_else(|| r.address.as_ref().map(|a| short_id(a.as_str()))).unwrap_or_else(|| "an agent".to_owned())
            };
            // A harness's own sub-agent runs in the caller's harness; an unknown address has
            // no harness YAAC can name.
            let kind = harness_own.then(|| state.provider_of(caller).map(|p| p.kind.to_string())).flatten();
            let machine = r.address.as_ref().and_then(|a| a.target()).filter(|t| *t != caller.target).map(|t| state.target_name(&t));
            Pill { label, kind, machine, key: None, at: None }
        }
    }
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ").replace('`', "")
}

/// The live receipt of the call's own sent message, when its `meshSent` is in the transcript.
fn live_receipt<'a>(t: &'a Transcript, ids: &[MessageId]) -> Option<&'a yaac_protocol::MeshSentItem> {
    t.items().iter().find_map(|i| match &i.body {
        ItemBody::MeshSent(s) if ids.contains(&s.message.id) => Some(s),
        _ => None,
    })
}

fn outcome_word(o: &AgentCallOutcome, detail: Option<&str>) -> String {
    let with = |w: &str| match detail {
        Some(d) if !d.is_empty() => format!("{w} · {d}"),
        _ => w.to_owned(),
    };
    match o {
        AgentCallOutcome::Delivered => "delivered".to_owned(),
        AgentCallOutcome::Queued => with("queued"),
        AgentCallOutcome::Failed => with("failed"),
        AgentCallOutcome::Started => "started".to_owned(),
        AgentCallOutcome::Completed => "finished".to_owned(),
        AgentCallOutcome::Resumed => "resumed".to_owned(),
        AgentCallOutcome::Stopped => "stopped".to_owned(),
        AgentCallOutcome::NotRunning => "was not running".to_owned(),
        AgentCallOutcome::Returned => "returned".to_owned(),
        AgentCallOutcome::NothingYet => "nothing yet".to_owned(),
        AgentCallOutcome::Registered => "registered".to_owned(),
        AgentCallOutcome::Unknown(s) => s.clone(),
    }
}

/// The line an agent call becomes, in thread `caller`.
pub fn line(state: &AppState, map: &AttentionMap, caller: &ThreadKey, t: Option<&Transcript>, item: &Item, call: &AgentCall) -> CallLine {
    let in_progress = item.status == ItemStatus::InProgress;
    let failed = item.status == ItemStatus::Failed || call.outcome == Some(AgentCallOutcome::Failed);
    let mut pills: Vec<Pill> = call.peers.iter().map(|r| pill(state, caller, r)).collect();
    let preview = call.text.as_deref().map(one_line).filter(|s| !s.is_empty());
    let detail = call.detail.as_deref();
    let status_of = |o: &Option<AgentCallOutcome>| -> Option<String> {
        if failed {
            return Some(outcome_word(&AgentCallOutcome::Failed, detail));
        }
        o.as_ref().map(|o| outcome_word(o, detail))
    };
    let mut l = CallLine { lead: String::new(), pills: Vec::new(), after: String::new(), preview: None, meta: None, status: None };
    match &call.kind {
        AgentCallKind::Message => {
            // The other end of the message, to land on when the pill is followed.
            for p in &mut pills {
                if let (Some(k), Some(id)) = (&p.key, call.messages.first()) {
                    p.at = state.transcripts.get(k).and_then(|tt| {
                        tt.items().iter().find_map(|i| match &i.body {
                            ItemBody::MeshReceived(r) if &r.message.id == id => Some(i.id.clone()),
                            _ => None,
                        })
                    });
                }
            }
            l.preview = preview;
            l.status = match t.and_then(|t| live_receipt(t, &call.messages)) {
                // A YAAC message's receipt moves on after the call (queued → delivered).
                Some(s) if !failed => Some(match &s.receipt.status {
                    ReceiptStatus::Delivered => "delivered".to_owned(),
                    _ => crate::activity::short_receipt(s).replacen(", ", " · ", 1),
                }),
                _ if in_progress => Some("sending…".to_owned()),
                _ => status_of(&call.outcome),
            };
            if call.outcome == Some(AgentCallOutcome::Resumed) && !failed {
                let harness_own = call.peers.iter().all(|r| r.address.is_none());
                l.lead = if harness_own { "Resumed subagent" } else { "Resumed" }.to_owned();
                l.status = None;
            }
        }
        AgentCallKind::Spawn => {
            let harness_own = call.peers.iter().all(|r| r.address.is_none());
            l.lead = if harness_own { "Subagent".to_owned() } else if in_progress { "Starting".to_owned() } else { "Started".to_owned() };
            if pills.is_empty() || (harness_own && pills.iter().all(|p| p.key.is_none())) {
                // A harness's sub-agent that is not a thread: the pill is the work's name.
                let label = call.title.clone().or_else(|| pills.first().map(|p| p.label.clone())).unwrap_or_else(|| "an agent".to_owned());
                let kind = state.provider_of(caller).map(|p| p.kind.to_string());
                pills = vec![Pill { label, kind, machine: None, key: None, at: None }];
            } else if let Some(title) = &call.title
                && pills.len() == 1
                && pills[0].key.is_none()
            {
                pills[0].label = title.clone();
            }
            let model = match pills.first().and_then(|p| p.key.as_ref()) {
                Some(k) => crate::agents::model_label(state, k).or_else(|| call.model.clone()),
                None => call.model.clone(),
            };
            let mut meta: Vec<String> = model.into_iter().collect();
            meta.extend(call.effort.clone());
            if call.background == Some(true) {
                meta.push("background".to_owned());
            }
            l.meta = (!meta.is_empty()).then(|| meta.join(" · "));
            // A worker that is a thread says how it is doing; otherwise the call's outcome.
            let live = pills.first().and_then(|p| p.key.as_ref()).map(|k| display(state, map, k).mark).filter(|m| *m != Mark::Idle);
            l.status = if failed {
                status_of(&call.outcome)
            } else if in_progress {
                Some(if harness_own { "running…".to_owned() } else { "starting…".to_owned() })
            } else {
                live.map(|m| m.word().to_owned()).or_else(|| status_of(&call.outcome).filter(|s| s != "started"))
            };
            l.preview = if harness_own { None } else { call.title.is_none().then_some(preview).flatten() };
        }
        AgentCallKind::Wait => {
            match (&call.outcome, in_progress) {
                (_, true) => {
                    l.lead = "Waiting for".to_owned();
                    if pills.is_empty() {
                        l.lead = "Waiting for a message…".to_owned();
                    } else {
                        l.after = "…".to_owned();
                    }
                }
                (Some(AgentCallOutcome::Returned), _) => {
                    l.after = if pills.is_empty() { "Something came back".to_owned() } else { "reported back".to_owned() };
                }
                _ => {
                    l.lead = if pills.is_empty() { "Waited for messages".to_owned() } else { "Waited for".to_owned() };
                    l.status = status_of(&call.outcome);
                }
            }
            if failed {
                l.status = status_of(&call.outcome);
            }
        }
        AgentCallKind::Resume => {
            l.lead = "Resumed".to_owned();
            l.preview = preview;
            l.status = if failed { status_of(&call.outcome) } else { None };
        }
        AgentCallKind::Read => {
            l.lead = "Read".to_owned();
            l.status = if failed { status_of(&call.outcome) } else { None };
        }
        AgentCallKind::Stop => {
            l.lead = if pills.is_empty() { "Stopped every worker".to_owned() } else { "Stopped".to_owned() };
            l.status = if failed || call.outcome == Some(AgentCallOutcome::NotRunning) { status_of(&call.outcome) } else { None };
        }
        AgentCallKind::Notify => {
            l.lead = "Will tell".to_owned();
            l.after = "when this turn ends".to_owned();
            l.status = if failed { status_of(&call.outcome) } else { None };
        }
        AgentCallKind::List => {
            l.lead = "Looked up agents".to_owned();
            l.preview = preview;
            l.status = if failed { status_of(&call.outcome) } else { None };
        }
        AgentCallKind::Unknown(k) => {
            l.lead = k.clone();
            l.preview = preview;
            l.status = status_of(&call.outcome);
        }
    }
    l.pills = pills;
    l
}

/// What an expanded agent-call line shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallDetail {
    /// "Message to", "Task for", "Waited for": heading the pills.
    pub heading: String,
    /// The message, prompt, or task, in full (markdown).
    pub text: Option<String>,
    /// What came back (markdown): a sub-agent's answer, what a wait returned, a transcript.
    pub result: Option<String>,
    /// A full receipt or reason, one line.
    pub status: Option<String>,
    /// The raw call, for the "raw" disclosure: its input as JSON.
    pub raw_input: String,
    /// … and its raw output.
    pub raw_output: String,
}

/// The expanded view of an agent call.
pub fn detail(t: Option<&Transcript>, tc: &ToolCallItem, call: &AgentCall) -> CallDetail {
    let heading = match &call.kind {
        AgentCallKind::Message => "To",
        AgentCallKind::Spawn => "Task for",
        AgentCallKind::Wait => "Waited for",
        AgentCallKind::Resume => "Resumed",
        AgentCallKind::Read => "Read",
        AgentCallKind::Stop => "Stopped",
        AgentCallKind::Notify => "Will tell",
        _ => "With",
    }
    .to_owned();
    let status = match t.and_then(|t| live_receipt(t, &call.messages)) {
        Some(s) => {
            let target = s.to.target_name.clone().unwrap_or_default();
            Some(crate::header::receipt_text(&s.receipt.status, s.receipt.reason.as_ref(), s.receipt.detail.as_deref(), &target))
        }
        None => call.detail.clone(),
    };
    let raw_input = tc.input.as_ref().map(|v| serde_json::to_string_pretty(v).unwrap_or_default()).unwrap_or_default();
    CallDetail { heading, text: call.text.clone(), result: call.result.clone(), status, raw_input, raw_output: tc.output.clone() }
}

/// Mesh items that are an agent call's own echo, in one transcript: hidden, because the
/// call's line already says it.
#[derive(Clone, Debug, Default)]
pub struct Echoes {
    messages: BTreeSet<MessageId>,
    spawned: BTreeSet<yaac_protocol::Address>,
    waits: BTreeSet<yaac_protocol::TurnId>,
}

impl Echoes {
    /// Collects the echoes of every agent call in `items`.
    pub fn of(items: &[Item]) -> Self {
        let mut e = Self::default();
        for item in items {
            let Some((_, call)) = of(item) else { continue };
            e.messages.extend(call.messages.iter().cloned());
            if call.kind == AgentCallKind::Spawn {
                e.spawned.extend(call.peers.iter().filter_map(|r| r.address.clone()));
            }
            // A wait of the mesh (its peers are addresses, or it waits for messages).
            if call.kind == AgentCallKind::Wait && call.peers.iter().all(|r| r.agent_id.is_none()) {
                e.waits.insert(item.turn_id.clone());
            }
        }
        e
    }

    /// Whether this item is an echo.
    pub fn hides(&self, item: &Item) -> bool {
        match &item.body {
            ItemBody::MeshSent(s) => self.messages.contains(&s.message.id),
            ItemBody::MeshReceived(r) => self.messages.contains(&r.message.id),
            ItemBody::MeshSpawn(s) => !s.threads.is_empty() && s.threads.iter().all(|t| self.spawned.contains(&t.peer.address)),
            ItemBody::MeshWait(_) => self.waits.contains(&item.turn_id),
            _ => false,
        }
    }
}
