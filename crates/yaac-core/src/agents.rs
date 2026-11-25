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

/// The pill shows at most this many agent icons (D168); a short window shows fewer
/// ([`pill_cap`]). Beyond it, only the agents that are alive (or finished unseen) are drawn and
/// every other one is counted in one "more" item at the bottom.
pub const PILL_MAX: usize = 7;

/// The fewest icons the pill is ever cut to.
pub const PILL_MIN: usize = 2;

/// How many icons fit when the pill may be `room` points tall, given the points one icon
/// takes and the points of everything else (cluster glyphs, hairlines, the "more" item,
/// margins): from [`PILL_MIN`] to [`PILL_MAX`].
pub fn pill_cap(room: f32, per_icon: f32, fixed: f32) -> usize {
    let fit = ((room - fixed) / per_icon).floor();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss, reason = "clamped to a small range first")]
    let fit = fit.clamp(PILL_MIN as f32, PILL_MAX as f32) as usize;
    fit
}

/// How much an agent is doing now, most urgent first (D168): the pill, the card and the docked
/// panel list a relation's agents in this order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Liveness {
    /// Blocked on the user (an approval, a question, a budget).
    NeedsYou,
    /// A turn is running.
    Running,
    /// Waiting on another agent.
    Waiting,
    /// Its work is done, or its last turn failed.
    Finished,
    /// Nothing running (stopped agents included).
    Idle,
    /// Its machine is offline or switched off.
    Stopped,
}

impl Liveness {
    /// The liveness of a row, from its mark.
    pub fn of(row: &AgentRow) -> Liveness {
        if row.display.attention.is_some() {
            return Liveness::NeedsYou;
        }
        match row.display.mark {
            Mark::NeedsYou | Mark::Paused => Liveness::NeedsYou,
            Mark::Running => Liveness::Running,
            Mark::Waiting => Liveness::Waiting,
            Mark::Done | Mark::Failed => Liveness::Finished,
            Mark::Idle => Liveness::Idle,
            Mark::Offline | Mark::Off => Liveness::Stopped,
        }
    }

    /// Needs you, running or waiting.
    pub fn alive(self) -> bool {
        self <= Liveness::Waiting
    }
}

/// Whether a row is always worth an icon: alive, or finished while nobody looked (D114).
pub fn prominent(row: &AgentRow) -> bool {
    Liveness::of(row).alive() || row.display.unseen
}

/// The sort key of D168: liveness, then unseen before seen, then most recent first.
fn order_key(row: &AgentRow) -> (Liveness, bool, std::cmp::Reverse<i64>) {
    (Liveness::of(row), !row.display.unseen, std::cmp::Reverse(row.updated))
}

/// Rows in D168's order (stable: equal keys keep their place).
pub fn by_liveness<'a>(rows: impl IntoIterator<Item = &'a AgentRow>) -> Vec<&'a AgentRow> {
    let mut v: Vec<&AgentRow> = rows.into_iter().collect();
    v.sort_by_key(|r| order_key(r));
    v
}

/// What the pill's "more" item stands for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Overflow {
    /// Agents without an icon.
    pub hidden: usize,
    /// Of them, alive (needs you, running, waiting): only when alive agents alone pass the cap.
    pub alive: usize,
    /// Of them, blocked on the user (the item then carries the needs-you mark).
    pub needs_you: usize,
}

/// What the pill draws: its clusters (D118), each in D168's order, and the "more" item.
#[derive(Clone, Debug, PartialEq)]
pub struct PillLayout<'a> {
    /// The clusters with at least one icon.
    pub clusters: Vec<Cluster<'a>>,
    /// Everyone else.
    pub more: Overflow,
}

impl PillLayout<'_> {
    /// Icons drawn.
    pub fn icons(&self) -> usize {
        self.clusters.iter().map(|c| c.rows.len()).sum()
    }

    /// Whether a row has an icon.
    pub fn shows(&self, row: &AgentRow) -> bool {
        self.clusters.iter().any(|c| c.rows.iter().any(|r| std::ptr::eq(*r, row)))
    }
}

/// The pill for at most `cap` icons (D168). When everyone fits, everyone is drawn. Otherwise
/// the parent keeps its icon, then the prominent agents (alive, or finished unseen) in D168's
/// order across all relations fill what is left; finished and idle agents are never drawn
/// then, only counted in the "more" item.
pub fn pill(panel: &AgentsPanel, cap: usize) -> PillLayout<'_> {
    let all = relation_clusters(panel).map(|mut cluster| {
        cluster.rows.sort_by_key(|row| order_key(row));
        cluster
    }).collect::<Vec<_>>();
    let total: usize = all.iter().map(|c| c.rows.len()).sum();
    if total <= cap {
        return PillLayout { clusters: all, more: Overflow::default() };
    }
    let parent = all.iter().find(|c| c.relation == Relation::SpawnedBy).and_then(|c| c.rows.first().copied());
    let mut keep: Vec<&AgentRow> = parent.into_iter().collect();
    let rest = by_liveness(all.iter().filter(|c| c.relation != Relation::SpawnedBy).flat_map(|c| c.rows.iter().copied()).filter(|r| prominent(r)));
    keep.extend(rest.into_iter().take(cap.saturating_sub(keep.len())));
    let kept = |r: &AgentRow| keep.iter().any(|k| std::ptr::eq(*k, r));
    let mut more = Overflow::default();
    for r in all.iter().flat_map(|c| c.rows.iter()).filter(|r| !kept(r)) {
        more.hidden += 1;
        let l = Liveness::of(r);
        more.alive += usize::from(l.alive());
        more.needs_you += usize::from(l == Liveness::NeedsYou);
    }
    let clusters = all
        .into_iter()
        .map(|c| Cluster { relation: c.relation, rows: c.rows.into_iter().filter(|r| kept(r)).collect() })
        .filter(|c| !c.rows.is_empty())
        .collect();
    PillLayout { clusters, more }
}

/// The text of the "more" item, in one line: "more (23)", or "more (23 · 4 alive)" when
/// alive agents are among the hidden.
pub fn more_label(more: &Overflow) -> String {
    if more.alive > 0 { format!("more ({} · {} alive)", more.hidden, more.alive) } else { format!("more ({})", more.hidden) }
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
    /// Its rows in the presentation's order.
    pub rows: Vec<&'a AgentRow>,
}

fn relation_clusters(panel: &AgentsPanel) -> impl Iterator<Item = Cluster<'_>> {
    let parent: Vec<&AgentRow> = match &panel.spawned_by {
        SpawnedBy::Agent(p) => vec![&**p],
        SpawnedBy::You => Vec::new(),
    };
    [(Relation::SpawnedBy, parent), (Relation::Workers, panel.workers.iter().collect()), (Relation::InContact, panel.contacts.iter().collect())]
        .into_iter()
        .filter(|(_, rows)| !rows.is_empty())
        .map(|(relation, rows)| Cluster { relation, rows })
}

/// Completed and unread first, other states next, completed and read last.
fn list_order_key(row: &AgentRow) -> (u8, std::cmp::Reverse<i64>) {
    let group = match (row.display.mark, row.display.unseen) {
        (Mark::Done, true) => 0,
        (Mark::Done, false) => 2,
        _ => 1,
    };
    (group, std::cmp::Reverse(row.updated))
}

/// Rows grouped by relation in card and dock order, newest activity first within each tier.
pub fn clusters(panel: &AgentsPanel) -> Vec<Cluster<'_>> {
    relation_clusters(panel).map(|mut cluster| {
        cluster.rows.sort_by_key(|row| list_order_key(row));
        cluster
    }).collect()
}

/// The floating card adds siblings before the usual groups. A sibling who is also a contact
/// appears once in this view, under Siblings.
pub fn card_groups(panel: &AgentsPanel) -> Vec<(&'static str, Vec<&AgentRow>)> {
    let siblings = sibling_rows(panel);
    let mut groups = Vec::new();
    if !siblings.is_empty() {
        groups.push(("Siblings", siblings));
    }
    for cluster in docked_clusters(panel) {
        groups.push((cluster.relation.heading(), cluster.rows));
    }
    groups
}

pub fn sibling_rows(panel: &AgentsPanel) -> Vec<&AgentRow> {
    let mut siblings: Vec<_> = panel.siblings.iter().collect();
    siblings.sort_by_key(|row| (list_order_key(row), row.key.as_ref()));
    siblings
}

/// The usual groups, with rows already shown under Siblings omitted.
pub fn docked_clusters(panel: &AgentsPanel) -> Vec<Cluster<'_>> {
    let siblings: BTreeSet<_> = panel.siblings.iter().filter_map(|row| row.key.as_ref()).collect();
    clusters(panel).into_iter().filter_map(|mut cluster| {
        cluster.rows.retain(|row| row.key.as_ref().is_none_or(|key| !siblings.contains(key)));
        (!cluster.rows.is_empty()).then_some(cluster)
    }).collect()
}

/// One entry of the docked panel's list under a heading.
#[derive(Clone, Debug, PartialEq)]
pub enum DockEntry<'a> {
    /// A row.
    Row(&'a AgentRow),
    /// The fold over the quiet rows: "12 finished" (or "12 inactive" when some are idle or
    /// offline), listed under it when it is open.
    Fold {
        /// The rows it holds.
        rows: Vec<&'a AgentRow>,
        /// "finished" or "inactive".
        word: &'static str,
    },
}

/// The docked panel folds the quiet rows of a heading once there are at least this many.
pub const DOCK_FOLD_MIN: usize = 4;

/// Keep the list order when collapsing an inactive tail.
pub fn docked_entries<'a>(cluster: &Cluster<'a>) -> Vec<DockEntry<'a>> {
    let rows = &cluster.rows;
    let tail = rows.iter().rposition(|row| prominent(row)).map_or(0, |i| i + 1);
    if rows.len() - tail < DOCK_FOLD_MIN {
        return rows.iter().copied().map(DockEntry::Row).collect();
    }
    let mut entries: Vec<_> = rows[..tail].iter().copied().map(DockEntry::Row).collect();
    let inactive = rows[tail..].to_vec();
    let word = if inactive.iter().all(|row| Liveness::of(row) == Liveness::Finished) { "finished" } else { "inactive" };
    entries.push(DockEntry::Fold { rows: inactive, word });
    entries
}

/// One row of the panel.
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
    /// The open thread's own row in Siblings (P341).
    pub is_self: bool,
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
    /// What kind of agent it is, when it was started by another (D151): "Claude subagent ·
    /// Explore" for one of the harness's own, "YAAC worker" for one the mesh started.
    pub origin: Option<String>,
    /// One of the harness's own sub-agents (D151), not a YAAC worker.
    pub subagent: bool,
    /// When it last changed (seconds since the epoch; 0 when unknown).
    pub updated: i64,
}

impl AgentRow {
    /// The thread this row can open.
    pub fn open_key(&self) -> Option<&ThreadKey> {
        self.key.as_ref().filter(|_| !self.is_self)
    }

    /// Compact last-activity age, shared by the Related agents views.
    pub fn activity_age(&self, now: i64) -> Option<String> {
        (self.updated != 0).then(|| crate::time::short_age(now.saturating_sub(self.updated)))
    }
}

/// "Claude subagent · Explore" for a harness's own sub-agent (D151), "YAAC worker" for one
/// the mesh started, nothing for a thread nobody spawned. From the summary, never the kind.
pub fn origin_label(state: &AppState, key: &ThreadKey) -> Option<String> {
    let s = state.summary(key)?;
    match &s.subagent {
        Some(sub) => {
            let provider = state.provider_of(key).map_or_else(|| "Harness".to_owned(), |p| p.display_name.clone());
            let mut label = format!("{provider} subagent");
            if let Some(t) = sub.agent_type.as_deref().filter(|t| !t.is_empty()) {
                label.push_str(" · ");
                label.push_str(t);
            }
            Some(label)
        }
        None => s.spawned_by.is_some().then(|| "YAAC worker".to_owned()),
    }
}

/// Who started the thread.
#[derive(Clone, Debug, PartialEq)]
pub enum SpawnedBy {
    /// A person.
    You,
    /// Another agent.
    Agent(Box<AgentRow>),
}

/// The orchestration's budget line (P352): both limits, and which one ran out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BudgetLine {
    /// Agent-started turns used.
    pub turns_used: u32,
    /// Agent-started turns allowed.
    pub turn_limit: u32,
    /// Seconds since a person last acted, as the root's target last reported it.
    pub since_acted_secs: u64,
    /// When a person last acted (seconds since the epoch), when the target says; the time
    /// since is then counted from it, so it stays current between reports.
    pub acted_at: Option<i64>,
    /// The time limit in seconds since a person last acted; `None` = no time limit (P351).
    pub time_limit_secs: Option<u64>,
    /// The budget ran out (D164): what ran out ([`BudgetLine::reached`]) and **Continue**.
    pub exhausted: bool,
    /// The root's target can change the time limit (`Capability::BudgetTimeLimit`).
    pub can_set_time_limit: bool,
    /// The orchestration's root, which **Continue** and the time limit act on.
    pub root: yaac_protocol::Address,
}

/// Which of a budget's limits ran out (P352).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RanOut {
    /// The agent-started turns.
    Turns,
    /// The time since a person last acted.
    Time,
    /// Exhausted, though neither limit is met here (an older target, a report in between).
    Unknown,
}

/// One choice of the duration's menu (P352).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimeLimitChoice {
    /// "+4 h".
    pub label: String,
    /// What it makes the limit: "limit 18 h" or "no time limit".
    pub detail: String,
    /// The new limit to send; `None` removes it.
    pub wall_clock_secs: Option<u64>,
}

/// The menu on the budget line's duration (P352).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimeLimitMenu {
    /// "Set time limit…" without a limit, "Extend time limit…" with one.
    pub title: &'static str,
    /// +1 h, +4 h, +12 h, +24 h, and "Remove time limit" when there is one.
    pub choices: Vec<TimeLimitChoice>,
    /// Why the menu cannot change anything (an older `yaacd` on the root's target).
    pub unavailable: Option<&'static str>,
}

/// The hours the duration's menu offers to add (P352).
pub const TIME_LIMIT_STEPS: [u32; 4] = [1, 4, 12, 24];

/// "3 h", "25 min", "< 1 min": a span, for the budget line.
pub fn span(secs: u64) -> String {
    match secs {
        0..60 => "< 1 min".to_owned(),
        60..3600 => format!("{} min", secs / 60),
        _ if secs.is_multiple_of(3600) || secs >= 10 * 3600 => format!("{} h", secs / 3600),
        _ => format!("{} h {} min", secs / 3600, secs % 3600 / 60),
    }
}

impl BudgetLine {
    /// Seconds since a person last acted, at `now` (seconds since the epoch).
    pub fn since_acted(&self, now: i64) -> u64 {
        self.acted_at
            .and_then(|at| u64::try_from(now.saturating_sub(at)).ok())
            .map_or(self.since_acted_secs, |since| since.max(self.since_acted_secs))
    }

    /// Seconds until the duration's text next changes, when it counts on by itself (the
    /// target said when a person acted): when to draw it again, and not before.
    pub fn next_change(&self, now: i64) -> Option<u64> {
        self.acted_at?;
        let since = self.since_acted(now);
        let step = if since < 10 * 3600 { 60 } else { 3600 };
        Some(step - since % step)
    }

    /// "41 of 100 agent turns".
    pub fn turns_text(&self) -> String {
        format!("{} of {} agent turns", self.turns_used, self.turn_limit)
    }

    /// The duration, which has the time-limit menu: "14 h since you acted (limit 12 h)" or
    /// "14 h since you acted · no time limit".
    pub fn time_text(&self, now: i64) -> String {
        let since = span(self.since_acted(now));
        match self.time_limit_secs {
            Some(limit) => format!("{since} since you acted (limit {})", span(limit)),
            None => format!("{since} since you acted · no time limit"),
        }
    }

    /// The whole line: "41 of 100 agent turns · 14 h since you acted (limit 12 h)".
    pub fn text(&self, now: i64) -> String {
        format!("{} · {}", self.turns_text(), self.time_text(now))
    }

    /// Which limit ran out, when the budget has, in the order the root's target checks them.
    pub fn ran_out(&self) -> Option<RanOut> {
        if !self.exhausted {
            None
        } else if self.turns_used >= self.turn_limit {
            Some(RanOut::Turns)
        } else if self.time_limit_secs.is_some() {
            Some(RanOut::Time)
        } else {
            Some(RanOut::Unknown)
        }
    }

    /// Which limit ran out, in words: "100 of 100 turns used" or "Paused: 12 h without you";
    /// "Agent budget reached" when it cannot be told which.
    pub fn reached(&self) -> Option<String> {
        Some(match (self.ran_out()?, self.time_limit_secs) {
            (RanOut::Turns, _) => format!("{} of {} turns used", self.turns_used, self.turn_limit),
            (RanOut::Time, Some(limit)) => format!("Paused: {} without you", span(limit)),
            _ => "Agent budget reached".to_owned(),
        })
    }

    /// The limit `hours` more gives: on top of the limit, or of the time already passed when
    /// that is more (or there is no limit), counted from the next whole hour.
    pub fn added(&self, now: i64, hours: u32) -> u64 {
        let base = self.time_limit_secs.unwrap_or(0).max(self.since_acted(now)).div_ceil(3600) * 3600;
        base.saturating_add(u64::from(hours) * 3600).min(yaac_protocol::MeshBudget::NO_TIME_LIMIT - 1)
    }

    /// The menu on the duration (P352). Changing the limit is not a person acting: the
    /// clock runs on (only **Continue** resets it).
    pub fn time_menu(&self, now: i64) -> TimeLimitMenu {
        let title = if self.time_limit_secs.is_some() { "Extend time limit…" } else { "Set time limit…" };
        let mut choices: Vec<_> = TIME_LIMIT_STEPS
            .iter()
            .map(|&h| {
                let secs = self.added(now, h);
                TimeLimitChoice { label: format!("+{h} h"), detail: format!("limit {}", span(secs)), wall_clock_secs: Some(secs) }
            })
            .collect();
        if self.time_limit_secs.is_some() {
            choices.push(TimeLimitChoice { label: "Remove time limit".to_owned(), detail: "no time limit".to_owned(), wall_clock_secs: None });
        }
        let unavailable = (!self.can_set_time_limit).then_some("The yaacd that keeps this budget is too old to change its time limit. Update it to change the limit.");
        TimeLimitMenu { title, choices, unavailable }
    }
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
    /// Other indexed children of the same parent, shown in the floating card.
    pub siblings: Vec<AgentRow>,
    /// Workers, as a flattened tree.
    pub workers: Vec<AgentRow>,
    /// Direct workers.
    pub direct_workers: usize,
    /// In contact with.
    pub contacts: Vec<AgentRow>,
    /// The rail.
    pub rail: Rail,
}

pub(crate) fn model_label(state: &AppState, key: &ThreadKey) -> Option<String> {
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
        is_self: false,
        title: s.title.clone(),
        provider: state.provider_of(key).map(|p| p.display_name.clone()),
        model: model_label(state, key),
        target: state.target_name(&key.target),
        display: d,
        activity,
        depth,
        git: crate::sidebar::line2_of(state, key),
        origin: origin_label(state, key),
        subagent: s.subagent.is_some(),
        updated: crate::time::secs(&s.updated_at).unwrap_or(0),
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
        is_self: false,
        title: peer.title.clone().unwrap_or_else(|| peer.address.to_string()),
        provider: peer.provider.clone(),
        model: peer.model.as_ref().map(ToString::to_string),
        target,
        display: ThreadDisplay { mark, attention: None, read_only: false, unseen: false, fast: false, ultrafast: false },
        activity: mark.word().to_owned(),
        depth: 0,
        git: None,
        origin: None,
        subagent: false,
        updated: 0,
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
        parent.into_iter().chain(self.workers.iter_mut()).chain(self.contacts.iter_mut()).chain(self.siblings.iter_mut())
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
        let mut kids: Vec<ThreadKey> = children.get(parent).cloned().unwrap_or_default();
        if let Some(summary) = state.key_of(parent).as_ref().and_then(|k| state.summary(k)) {
            kids.extend(summary.linked_children.iter().flatten().filter_map(|address| state.key_of(address)));
        }
        kids.sort_by_key(|k| state.summary(k).map(|s| s.created_at.clone()));
        for k in kids {
            if !seen.insert(k.clone()) {
                continue;
            }
            if let Some(row) = row_for(state, map, &k, depth) {
                out.push(row);
            }
            walk(state, map, children, &k.address(), depth.saturating_add(1), out, seen);
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

    // The parent address still identifies siblings while the parent's own thread is missing.
    // Exclude the parent and our workers even if the index contains cyclic links.
    let mut siblings: Vec<_> = s.spawned_by.as_ref().and_then(|parent| children.get(parent))
        .into_iter().flatten()
        .filter(|sibling| !seen.contains(*sibling) && Some(*sibling) != parent_key.as_ref())
        .filter_map(|sibling| row_for(state, map, sibling, 0))
        .collect();
    if !siblings.is_empty() && let Some(mut row) = row_for(state, map, key, 0) {
        row.is_self = true;
        siblings.push(row);
    }

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
        turns_used: b.used.agent_turns,
        turn_limit: b.allowance.agent_turns,
        since_acted_secs: b.used.wall_clock_secs,
        acted_at: b.acted_at.as_ref().and_then(crate::time::secs),
        time_limit_secs: b.allowance.time_limited().then_some(b.allowance.wall_clock_secs),
        exhausted: b.exhausted,
        can_set_time_limit: b
            .root
            .target()
            .and_then(|t| state.target(&t))
            .and_then(|t| t.entry.state.daemon.as_ref())
            .is_some_and(|d| d.budget_time_limit),
        root: b.root.clone(),
    });
    let root = root_addr.filter(|r| r != &me && Some(r) != s.spawned_by.as_ref()).map(|r| {
        let k = state.key_of(&r);
        let title = k.as_ref().and_then(|k| state.summary(k)).map_or_else(|| r.to_string(), |s| s.title.clone());
        (title, k)
    });

    let rail = Rail { count: usize::from(s.spawned_by.is_some()) + direct_workers + contacts.len() };
    let flows = flows(state, key);
    let mut panel = AgentsPanel { budget, root, spawned_by, siblings, workers, direct_workers, contacts, rail };
    if let SpawnedBy::Agent(p) = &mut panel.spawned_by {
        p.role = Role::Parent;
    }
    for c in &mut panel.contacts {
        c.role = Role::Contact;
    }
    for row in panel.rows_mut() {
        if let Some(k) = &row.key {
            row.flow = flows.iter().find(|(a, _)| a == &k.address()).map(|(_, f)| *f);
        }
    }
    Some(panel)
}
