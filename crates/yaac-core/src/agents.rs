//! The agents panel: Spawned by, Workers, In contact with — or hidden.

use std::collections::BTreeSet;

use yaac_protocol::{Address, ItemBody, MeshPeer, ReceiptReason, ReceiptStatus};

use crate::attention::{AttentionMap, Mark, ThreadDisplay, display};
use crate::key::ThreadKey;
use crate::state::AppState;

/// Data moving between the open thread and an agent, right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    /// This thread is sending to it (up arrow).
    Sending,
    /// This thread is receiving from it (down arrow).
    Receiving,
}

/// Where an agent sits relative to the open thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// The parent.
    Parent,
    /// A worker (at any depth).
    Worker,
    /// A contact.
    Contact,
}

/// The three states of the panel, smallest first (docs/07-ui.md#the-agents-panel).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PanelMode {
    /// A narrow pill floating over the right edge of the thread (default).
    #[default]
    Pill,
    /// The pill widened into a floating card (hover or keyboard focus).
    Card,
    /// Pinned as a fixed panel; the thread narrows.
    Docked,
}

/// What happened to the panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelInput {
    /// The pointer entered it, or it took keyboard focus.
    Enter,
    /// The pointer left and it has no focus.
    Leave,
    /// The dock button.
    Dock,
    /// The undock button.
    Undock,
}

impl PanelMode {
    /// The next state.
    pub fn next(self, input: PanelInput) -> PanelMode {
        match (self, input) {
            (PanelMode::Pill, PanelInput::Enter) => PanelMode::Card,
            (PanelMode::Card, PanelInput::Leave) => PanelMode::Pill,
            (_, PanelInput::Dock) => PanelMode::Docked,
            (PanelMode::Docked, PanelInput::Undock) => PanelMode::Pill,
            (m, _) => m,
        }
    }

    /// Whether the panel takes a layout column.
    pub fn takes_column(self) -> bool {
        self == PanelMode::Docked
    }
}

/// The pill shows at most this many icons; beyond it, the first seven and "+N".
pub const PILL_MAX: usize = 8;

/// The icons the pill shows, and how many are hidden behind "+N".
pub fn pill_icons(panel: &AgentsPanel) -> (Vec<&AgentRow>, usize) {
    let all: Vec<&AgentRow> = panel.all().collect();
    if all.len() > PILL_MAX {
        let shown = all[..PILL_MAX - 1].to_vec();
        let hidden = all.len() - shown.len();
        (shown, hidden)
    } else {
        (all, 0)
    }
}

/// How a related agent stands to the open thread (D118): the three groups every state of
/// the panel shows, in this order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Relation {
    /// The agent that started this thread.
    SpawnedBy,
    /// Workers this thread started (at any depth).
    Workers,
    /// Agents it is in contact with.
    InContact,
}

impl Relation {
    /// The heading in the card and the docked panel.
    pub fn heading(self) -> &'static str {
        match self {
            Relation::SpawnedBy => "Spawned by",
            Relation::Workers => "Workers",
            Relation::InContact => "In contact with",
        }
    }
}

/// One relation's rows.
#[derive(Clone, Debug, PartialEq)]
pub struct Cluster<'a> {
    /// Which relation.
    pub relation: Relation,
    /// Its rows, in order.
    pub rows: Vec<&'a AgentRow>,
    /// Rows the pill had no room for (shown as "+N" after the cluster).
    pub more: usize,
}

/// The panel's rows grouped by relation (D118) — the one grouping the card, the docked
/// panel and the pill all draw. Empty groups are left out. With `cap`, at most that many
/// icons are kept in all (the pill), each cluster keeping at least one.
pub fn clusters(panel: &AgentsPanel, cap: Option<usize>) -> Vec<Cluster<'_>> {
    let parent: Vec<&AgentRow> = match &panel.spawned_by {
        SpawnedBy::Agent(p) => vec![&**p],
        SpawnedBy::You => Vec::new(),
    };
    let mut out: Vec<Cluster<'_>> = [(Relation::SpawnedBy, parent), (Relation::Workers, panel.workers.iter().collect()), (Relation::InContact, panel.contacts.iter().collect())]
        .into_iter()
        .filter(|(_, rows)| !rows.is_empty())
        .map(|(relation, rows)| Cluster { relation, rows, more: 0 })
        .collect();
    if let Some(cap) = cap {
        // Each icon takes a slot, and so does a cluster's "+N". Trim the largest cluster
        // first, never below one icon.
        let slots = |out: &[Cluster<'_>]| out.iter().map(|c| c.rows.len() + usize::from(c.more > 0)).sum::<usize>();
        while slots(&out) > cap {
            let Some(c) = out.iter_mut().filter(|c| c.rows.len() > 1).max_by_key(|c| c.rows.len()) else { break };
            c.rows.pop();
            c.more += 1;
        }
    }
    out
}

/// One row of the panel (always a link when `key` is known).
#[derive(Clone, Debug, PartialEq)]
pub struct AgentRow {
    /// Parent, worker, or contact.
    pub role: Role,
    /// The provider kind, for the harness icon (display only — never branched on).
    pub kind: Option<String>,
    /// The last thing it did, one line (ellipsised by the widget).
    pub last_action: String,
    /// Data in flight with the open thread.
    pub flow: Option<Flow>,
    /// Finished: drawn dimmed.
    pub finished: bool,
    /// The thread, when it is in the index.
    pub key: Option<ThreadKey>,
    /// Title.
    pub title: String,
    /// Provider display name.
    pub provider: Option<String>,
    /// Model label.
    pub model: Option<String>,
    /// Target name.
    pub target: String,
    /// Mark and attention.
    pub display: ThreadDisplay,
    /// Latest activity, one line.
    pub activity: String,
    /// Tree depth (workers of workers).
    pub depth: u8,
    /// Its git line (branch and counts), as in the sidebar.
    pub git: Option<crate::sidebar::Line2>,
}

/// Who started the thread.
#[derive(Clone, Debug, PartialEq)]
pub enum SpawnedBy {
    /// A person.
    You,
    /// Another agent.
    Agent(Box<AgentRow>),
}

/// The orchestration's budget line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BudgetLine {
    /// "14 of 100 agent turns · 4 of 20 threads".
    pub text: String,
    /// The budget ran out.
    pub exhausted: bool,
}

/// The collapsed rail: the relationship count, nothing else.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rail {
    /// Relationships.
    pub count: usize,
}

/// The panel.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentsPanel {
    /// Budget of the orchestration this thread belongs to.
    pub budget: Option<BudgetLine>,
    /// "root: …" when the root is not the parent.
    pub root: Option<(String, Option<ThreadKey>)>,
    /// Spawned by.
    pub spawned_by: SpawnedBy,
    /// Workers, as a flattened tree.
    pub workers: Vec<AgentRow>,
    /// Direct workers.
    pub direct_workers: usize,
    /// In contact with.
    pub contacts: Vec<AgentRow>,
    /// The rail.
    pub rail: Rail,
}

fn model_label(state: &AppState, key: &ThreadKey) -> Option<String> {
    let s = state.summary(key)?;
    let m = s.model.as_ref()?;
    Some(state.provider_of(key).and_then(|p| p.models.iter().find(|x| &x.id == m)).map_or_else(|| m.to_string(), |x| x.display_name.clone()))
}

/// A row for an indexed thread.
pub fn row_for(state: &AppState, map: &AttentionMap, key: &ThreadKey, depth: u8) -> Option<AgentRow> {
    let s = state.summary(key)?;
    let d = display(state, map, key);
    let activity = match d.attention {
        Some(a) => format!("needs you · {}", a.word()),
        None => s.preview.clone(),
    };
    let last_action = match d.attention {
        Some(crate::attention::Attention::BudgetReached) => "Paused · the budget ran out".to_owned(),
        Some(a) => format!("Waiting for your {}", a.word()),
        None if !s.preview.is_empty() => s.preview.clone(),
        None => d.mark.word().to_owned(),
    };
    Some(AgentRow {
        role: Role::Worker,
        kind: state.provider_of(key).map(|p| p.kind.to_string()),
        last_action,
        flow: None,
        finished: d.mark == Mark::Done,
        key: Some(key.clone()),
        title: s.title.clone(),
        provider: state.provider_of(key).map(|p| p.display_name.clone()),
        model: model_label(state, key),
        target: state.target_name(&key.target),
        display: d,
        activity,
        depth,
        git: crate::sidebar::line2_of(state, key),
    })
}

fn row_for_peer(state: &AppState, map: &AttentionMap, peer: &MeshPeer) -> AgentRow {
    if let Some(key) = state.key_of(&peer.address)
        && let Some(row) = row_for(state, map, &key, 0)
    {
        return row;
    }
    let target = peer.target_name.clone().unwrap_or_else(|| peer.address.target().map_or_else(String::new, |t| state.target_name(&t)));
    let mark = if peer.reachable == Some(false) { Mark::Offline } else { Mark::Idle };
    AgentRow {
        role: Role::Contact,
        kind: None,
        last_action: mark.word().to_owned(),
        flow: None,
        finished: false,
        key: None,
        title: peer.title.clone().unwrap_or_else(|| peer.address.to_string()),
        provider: peer.provider.clone(),
        model: peer.model.as_ref().map(ToString::to_string),
        target,
        display: ThreadDisplay { mark, attention: None, read_only: false, unseen: false },
        activity: mark.word().to_owned(),
        depth: 0,
        git: None,
    }
}

impl AgentsPanel {
    /// Every row: the parent first, then workers, then contacts.
    pub fn all(&self) -> impl Iterator<Item = &AgentRow> {
        let parent = match &self.spawned_by {
            SpawnedBy::Agent(p) => Some(&**p),
            SpawnedBy::You => None,
        };
        parent.into_iter().chain(self.workers.iter()).chain(self.contacts.iter())
    }

    fn rows_mut(&mut self) -> impl Iterator<Item = &mut AgentRow> {
        let parent = match &mut self.spawned_by {
            SpawnedBy::Agent(p) => Some(&mut **p),
            SpawnedBy::You => None,
        };
        parent.into_iter().chain(self.workers.iter_mut()).chain(self.contacts.iter_mut())
    }

    /// Whether any arrow is showing (the only reason the panel asks for repaints).
    pub fn in_flight(&self) -> bool {
        self.all().any(|r| r.flow.is_some())
    }
}

/// Messages in flight between `key` and other agents: a `meshSent` of this thread still in
/// progress (sending), or a `meshSent` to this thread still in progress in the other agent's
/// transcript, or a `meshReceived` still in progress here (receiving).
pub fn flows(state: &AppState, key: &ThreadKey) -> Vec<(Address, Flow)> {
    let me = key.address();
    let mut out = Vec::new();
    if let Some(t) = state.transcripts.get(key) {
        for item in t.items() {
            if item.status != yaac_protocol::ItemStatus::InProgress {
                continue;
            }
            match &item.body {
                ItemBody::MeshSent(s) => out.push((s.to.address.clone(), Flow::Sending)),
                ItemBody::MeshReceived(r) => out.push((r.from.address.clone(), Flow::Receiving)),
                _ => {}
            }
        }
    }
    for (other, t) in &state.transcripts {
        if other == key {
            continue;
        }
        for item in t.items() {
            if item.status == yaac_protocol::ItemStatus::InProgress
                && let ItemBody::MeshSent(s) = &item.body
                && s.to.address == me
            {
                out.push((other.address(), Flow::Receiving));
            }
        }
    }
    out
}

/// The orchestration root of a thread: its summary's `root`, or itself when it has a budget
/// or workers.
pub fn root_of(state: &AppState, key: &ThreadKey) -> Option<Address> {
    let s = state.summary(key)?;
    if let Some(r) = &s.root {
        return Some(r.clone());
    }
    let me = key.address();
    (state.budgets.contains_key(&me) || s.children > 0).then_some(me)
}

/// The panel for a thread, or `None` when it has no parent, no workers, and no contacts — the
/// panel is then hidden entirely.
pub fn agents_panel(state: &AppState, map: &AttentionMap, key: &ThreadKey) -> Option<AgentsPanel> {
    let s = state.summary(key)?;
    let me = key.address();
    let children = state.children_index();

    // Workers, depth-first.
    let mut workers = Vec::new();
    fn walk(
        state: &AppState,
        map: &AttentionMap,
        children: &std::collections::BTreeMap<Address, Vec<ThreadKey>>,
        parent: &Address,
        depth: u8,
        out: &mut Vec<AgentRow>,
        seen: &mut BTreeSet<ThreadKey>,
    ) {
        let mut kids: Vec<&ThreadKey> = children.get(parent).map(|v| v.iter().collect()).unwrap_or_default();
        kids.sort_by_key(|k| state.summary(k).map(|s| s.created_at.clone()));
        for k in kids {
            if !seen.insert(k.clone()) {
                continue;
            }
            if let Some(row) = row_for(state, map, k, depth) {
                out.push(row);
            }
            walk(state, map, children, &k.address(), depth + 1, out, seen);
        }
    }
    let mut seen = BTreeSet::from([key.clone()]);
    walk(state, map, &children, &me, 0, &mut workers, &mut seen);
    let direct_workers = workers.iter().filter(|w| w.depth == 0).count();

    // Parent.
    let parent_key = s.spawned_by.as_ref().and_then(|a| state.key_of(a));
    let spawned_by = match (&s.spawned_by, &parent_key) {
        (None, _) => SpawnedBy::You,
        (Some(_), Some(pk)) => row_for(state, map, pk, 0).map_or(SpawnedBy::You, |r| SpawnedBy::Agent(Box::new(r))),
        (Some(addr), None) => SpawnedBy::Agent(Box::new(row_for_peer(
            state,
            map,
            &MeshPeer { address: addr.clone(), title: None, provider: None, model: None, target_name: None, reachable: None },
        ))),
    };

    // Contacts: peers in mesh items that are neither the parent nor a worker, and that exist.
    let mut contacts: Vec<AgentRow> = Vec::new();
    let mut known: BTreeSet<Address> = BTreeSet::new();
    known.insert(me.clone());
    if let Some(p) = &s.spawned_by {
        known.insert(p.clone());
    }
    for w in &workers {
        if let Some(k) = &w.key {
            known.insert(k.address());
        }
    }
    if let Some(t) = state.transcripts.get(key) {
        for item in t.items() {
            let peer = match &item.body {
                ItemBody::MeshSent(sent) => {
                    let gone = sent.receipt.status == ReceiptStatus::Failed
                        && matches!(sent.receipt.reason, Some(ReceiptReason::ThreadDeleted | ReceiptReason::UnknownAddress));
                    if gone {
                        continue;
                    }
                    &sent.to
                }
                ItemBody::MeshReceived(r) => &r.from,
                _ => continue,
            };
            if known.insert(peer.address.clone()) {
                contacts.push(row_for_peer(state, map, peer));
            }
        }
    }

    if s.spawned_by.is_none() && workers.is_empty() && contacts.is_empty() && s.peers == 0 {
        return None;
    }

    let root_addr = root_of(state, key);
    let budget = root_addr.as_ref().and_then(|r| state.budgets.get(r)).map(|b| BudgetLine {
        text: format!(
            "{} of {} agent turns · {} of {} threads",
            b.used.agent_turns, b.allowance.agent_turns, b.used.threads_spawned, b.allowance.threads_spawned
        ),
        exhausted: b.exhausted,
    });
    let root = root_addr.filter(|r| r != &me && Some(r) != s.spawned_by.as_ref()).map(|r| {
        let k = state.key_of(&r);
        let title = k.as_ref().and_then(|k| state.summary(k)).map_or_else(|| r.to_string(), |s| s.title.clone());
        (title, k)
    });

    let rail = Rail { count: usize::from(s.spawned_by.is_some()) + direct_workers + contacts.len() };
    let mut panel = AgentsPanel { budget, root, spawned_by, workers, direct_workers, contacts, rail };
    if let SpawnedBy::Agent(p) = &mut panel.spawned_by {
        p.role = Role::Parent;
    }
    for c in &mut panel.contacts {
        c.role = Role::Contact;
    }
    let flows = flows(state, key);
    for row in panel.rows_mut() {
        if let Some(k) = &row.key {
            row.flow = flows.iter().find(|(a, _)| a == &k.address()).map(|(_, f)| *f);
        }
    }
    Some(panel)
}
