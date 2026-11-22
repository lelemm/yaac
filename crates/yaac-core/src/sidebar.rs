//! The sidebar (docs/design/screens.md, revision 5): **one flat list** of one-line thread rows
//! under quiet sticky section headers. No tree. "Needs you · N" comes first, "Muted · N" last
//! (collapsed). Grouping is any combination of Machine, Project, and Worktree; whatever is
//! ticked composes the header text, and with nothing ticked there are no headers, newest
//! first (docs/05-threads.md#sidebar-grouping, flattened).

use std::collections::BTreeMap;

use yaac_protocol::attach::ConnectionStatus;
use yaac_protocol::{TargetId, ThreadSummary};

use crate::attention::{Attention, AttentionMap, Mark, ThreadDisplay, display, target_mark};
use crate::key::ThreadKey;
use crate::state::AppState;
use crate::time;

/// The three grouping checkboxes, plus "Show dismissed". (Archived threads have their own
/// section, D93; there is no "Show archived" switch.)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Grouping {
    /// Group by machine (target).
    pub machine: bool,
    /// Group by project.
    pub project: bool,
    /// Group by worktree.
    pub worktree: bool,
    /// Dismissed threads are listed too.
    pub show_dismissed: bool,
}

impl Default for Grouping {
    fn default() -> Self {
        // Each row carries its branch, so sections are per project by default.
        Self { machine: true, project: true, worktree: false, show_dismissed: false }
    }
}

/// Which collapsed sections the user opened.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Open {
    /// Muted.
    pub muted: bool,
    /// Archived.
    pub archived: bool,
}

/// How rows are ordered inside each group (D116).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Order {
    /// Most recent message first (the default).
    #[default]
    LastMessage,
    /// By title, A to Z (case-insensitive, numbers in natural order).
    AToZ,
    /// By title, Z to A.
    ZToA,
}

impl Order {
    /// All, in menu order.
    pub const ALL: [Order; 3] = [Order::LastMessage, Order::AToZ, Order::ZToA];

    /// The menu label.
    pub fn label(self) -> &'static str {
        match self {
            Order::LastMessage => "Last message",
            Order::AToZ => "A → Z",
            Order::ZToA => "Z → A",
        }
    }
}

/// The ordering menu's choices (D116): a window preference, like the grouping.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Ordering {
    /// The order inside each group.
    pub order: Order,
    /// Keep unseen finished threads (D114) at the top of their group.
    pub unseen_first: bool,
    /// Keep threads that need the user at the top of their group.
    pub needs_first: bool,
    /// List only running threads (D129) — and, when the two choices above are on, the
    /// threads they float. The open thread always stays listed.
    pub running_only: bool,
}

impl Default for Ordering {
    fn default() -> Self {
        Self { order: Order::LastMessage, unseen_first: true, needs_first: true, running_only: false }
    }
}

/// Under "Show only running" (D129): running threads (and those waiting on another agent,
/// which are at work), plus whatever the float choices keep at the top.
fn shown_when_running_only(state: &AppState, map: &AttentionMap, key: &ThreadKey, ordering: Ordering) -> bool {
    let d = display(state, map, key);
    matches!(d.mark, Mark::Running | Mark::Waiting)
        || (ordering.needs_first && d.attention.is_some())
        || (ordering.unseen_first && d.unseen)
}

/// Compares titles case-insensitively, runs of digits by their value ("t2" before "t10").
pub fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering as O;
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return O::Less,
            (Some(_), None) => return O::Greater,
            (Some(c), Some(d)) if c.is_ascii_digit() && d.is_ascii_digit() => {
                let take = |it: &mut std::iter::Peekable<std::str::Chars<'_>>| {
                    let mut n = String::new();
                    while let Some(c) = it.peek().copied().filter(char::is_ascii_digit) {
                        n.push(c);
                        it.next();
                    }
                    n
                };
                let (m, n) = (take(&mut x), take(&mut y));
                let (mt, nt) = (m.trim_start_matches('0'), n.trim_start_matches('0'));
                let o = mt.len().cmp(&nt.len()).then_with(|| mt.cmp(nt));
                if o != O::Equal {
                    return o;
                }
            }
            (Some(c), Some(d)) => {
                let o = c.to_lowercase().cmp(d.to_lowercase());
                if o != O::Equal {
                    return o;
                }
                x.next();
                y.next();
            }
        }
    }
}

/// What the right end of a row shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowRight {
    /// A relative time ("2h").
    Age(String),
    /// The machine; `muted` adds the word "muted".
    Machine {
        /// Machine name.
        name: String,
        /// The thread is muted.
        muted: bool,
    },
    /// When a timed mute ends ("until 14:30", "until Sat", "until 3 Dec"), in the Muted
    /// section, in the host's local time.
    Until(String),
}

/// The second line of a row: what the thread is doing in git. The window draws each part
/// with an icon; every count is shown only when it is not zero.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line2 {
    /// The provider kind, for the harness logo that starts the line (display only — never
    /// branched on).
    pub kind: Option<String>,
    /// The branch; empty for a folder outside git (the line is then the logo alone).
    pub branch: String,
    /// Files added relative to origin.
    pub added: u32,
    /// Files modified relative to origin.
    pub modified: u32,
    /// Files deleted relative to origin.
    pub deleted: u32,
    /// Commits not pushed.
    pub ahead: u32,
    /// Commits not yet pulled.
    pub behind: u32,
    /// Direct workers.
    pub workers: u32,
}

/// One row: line 1 is a state mark, the title, and one thing on the right; line 2 (when the
/// folder is in git, outside Needs you and Muted) is the branch and compact counts.
#[derive(Clone, Debug, PartialEq)]
pub struct ThreadRow {
    /// The thread.
    pub key: ThreadKey,
    /// Title.
    pub title: String,
    /// Mark and attention.
    pub display: ThreadDisplay,
    /// Right-aligned secondary text.
    pub right: RowRight,
    /// The second line.
    pub line2: Option<Line2>,
    /// This is the open thread.
    pub selected: bool,
    /// Muted (drawn in secondary text in the Muted section).
    pub muted: bool,
    /// Archived (listed only under Archived).
    pub archived: bool,
    /// Dismissed (listed only with "Show dismissed").
    pub dismissed: bool,
}

/// What a section is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SectionKind {
    /// "Pinned · N", second.
    Pinned,
    /// A group of the chosen grouping.
    Group,
    /// "Muted · N", collapsed unless opened.
    Muted,
    /// "Archived · N", the last, collapsed unless opened.
    Archived,
}

/// A sticky section header: project · worktree on the left, the machine on the right.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    /// "yaac", "yaac · feature/protocol", "Needs you · 2", "Muted · 4", or a folder path.
    pub left: String,
    /// A temporary project (a folder with threads but no project yet): italic, dashed mark,
    /// and "Create project…" on right-click.
    pub temporary: bool,
    /// The folder of a temporary project, for Create project.
    pub folder: Option<(TargetId, String)>,
    /// The machine.
    pub machine: Option<String>,
    /// The machine's connection, shown only when it is not connected: mark and word.
    pub connection: Option<(Mark, String)>,
    /// The machine's daemon is older (a small warning mark).
    pub older_daemon: bool,
    /// Offer Retry.
    pub retry: bool,
}

/// A section of the list.
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    /// Stable id (for collapsing and sticky headers).
    pub id: String,
    /// What it is.
    pub kind: SectionKind,
    /// Its header; `None` when nothing is ticked.
    pub header: Option<Header>,
    /// Collapsed to its header.
    pub collapsed: bool,
    /// Rows (empty when collapsed).
    pub rows: Vec<ThreadRow>,
    /// Rows it holds, collapsed or not.
    pub count: usize,
}

/// One entry of the Needs-you list (also Ctrl+J).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NeedsYouEntry {
    /// The thread.
    pub key: ThreadKey,
    /// Its title.
    pub title: String,
    /// Why.
    pub attention: Attention,
    /// Its target's name.
    pub target_name: String,
    /// Muted (still listed: a blocked thread is never hidden).
    pub muted: bool,
}

fn updated(state: &AppState, key: &ThreadKey) -> i64 {
    state.summary(key).and_then(|s| time::secs(&s.updated_at)).unwrap_or(0)
}

/// Every thread waiting on the user, from all machines, ignoring grouping; most recent first.
/// A muted (or dismissed) thread that gets blocked is still listed; an archived one is listed
/// only under Archived.
pub fn needs_you(state: &AppState, map: &AttentionMap, now: i64) -> Vec<NeedsYouEntry> {
    let mut out: Vec<NeedsYouEntry> = map
        .own
        .iter()
        .filter_map(|(key, a)| {
            let s = state.summary(key).filter(|s| !s.archived)?;
            Some(NeedsYouEntry {
                key: key.clone(),
                title: s.title.clone(),
                attention: *a,
                target_name: state.target_name(&key.target),
                muted: state.is_muted(key, now),
            })
        })
        .collect();
    out.sort_by(|a, b| updated(state, &b.key).cmp(&updated(state, &a.key)).then_with(|| a.title.cmp(&b.title)));
    out
}

fn connection(state: &AppState, target: &TargetId) -> (Option<(Mark, String)>, bool, bool) {
    let Some(t) = state.target(target) else { return (None, false, false) };
    let st = &t.entry.state;
    let older = st.daemon.as_ref().is_some_and(|d| d.older);
    if t.entry.enabled && st.status == ConnectionStatus::Connected {
        return (None, older, false);
    }
    let word = if !t.entry.enabled {
        "off".to_owned()
    } else {
        match (&st.status, &st.detail) {
            (ConnectionStatus::Disabled | ConnectionStatus::Stopped, Some(d)) => d.clone(),
            (ConnectionStatus::Disabled, None) => "off".to_owned(),
            (s, Some(d)) => format!("{} · {d}", s.as_str()),
            (s, None) => s.as_str().to_owned(),
        }
    };
    let retry = t.entry.enabled && st.status == ConnectionStatus::Offline;
    (Some((target_mark(state, target), word)), older, retry)
}

fn branch_of(state: &AppState, key: &ThreadKey, s: &ThreadSummary) -> Option<String> {
    let _ = (state, key);
    // Where the user moved it (D144): the checkout it was placed in.
    if let Some(pl) = &s.placement
        && let Some(root) = &pl.worktree
    {
        return pl.worktree_name.clone().or_else(|| root.as_str().trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().map(str::to_owned));
    }
    // A linked worktree is named by its checkout; the main checkout by its branch.
    s.repo
        .as_ref()
        .and_then(|r| r.worktree_name.clone())
        .or_else(|| s.git.as_ref().and_then(|g| g.branch.clone()))
        .or_else(|| s.worktree.as_ref().and_then(|w| w.branch.clone()))
        .or_else(|| s.branch.clone())
}

/// A row's second line: the harness logo, then (in git) the branch and the counts.
pub fn line2_of(state: &AppState, key: &ThreadKey) -> Option<Line2> {
    let s = state.summary(key)?;
    let kind = state.provider_of(key).map(|p| p.kind.to_string());
    Some(match &s.git {
        Some(g) => Line2 {
            kind,
            branch: g.branch.clone().or_else(|| s.branch.clone()).unwrap_or_default(),
            added: g.added,
            modified: g.modified,
            deleted: g.deleted,
            ahead: g.ahead,
            behind: g.behind,
            workers: s.children,
        },
        None => Line2 { kind, branch: String::new(), added: 0, modified: 0, deleted: 0, ahead: 0, behind: 0, workers: s.children },
    })
}

/// D115: how a narrow row shares its width. Every mark keeps its room (`marks`, in points,
/// plus `gaps`); the title — or the branch — gets what is left and is ellipsised, down to
/// nothing.
pub fn title_width(row_width: f32, marks: &[f32], gaps: f32) -> f32 {
    (row_width - marks.iter().sum::<f32>() - gaps).max(0.0)
}

/// The section a thread is listed under, and its header.
fn group_of(state: &AppState, key: &ThreadKey, s: &ThreadSummary, g: Grouping) -> (String, Option<Header>) {
    let mut id = String::from("g");
    let mut left: Vec<String> = Vec::new();
    let mut temporary = false;
    let mut folder = None;
    if g.project {
        match state.project_of(key) {
            Some(p) => {
                id.push_str(&format!("|p:{}", p.id));
                left.push(p.name.clone());
            }
            None => {
                id.push_str(&format!("|t:{}:{}", key.target, s.cwd));
                left.push(s.cwd.to_string());
                temporary = true;
                folder = Some((key.target.clone(), s.cwd.to_string()));
            }
        }
    }
    if g.worktree
        && let Some(b) = branch_of(state, key, s)
    {
        id.push_str(&format!("|b:{b}"));
        left.push(b);
    }
    if g.machine {
        id.push_str(&format!("|m:{}", key.target));
    }
    if !(g.machine || g.project || g.worktree) {
        return (id, None);
    }
    let name = state.target_name(&key.target);
    let (connection, older, retry) = if g.machine { connection(state, &key.target) } else { (None, false, false) };
    let header = if left.is_empty() {
        Header { left: name, machine: None, connection, older_daemon: older, retry, temporary, folder }
    } else {
        Header { left: left.join(" · "), machine: g.machine.then_some(name), connection, older_daemon: older, retry, temporary, folder }
    };
    (id, Some(header))
}

/// The section id that holds a thread, for scrolling to it.
pub fn section_of(state: &AppState, grouping: Grouping, key: &ThreadKey) -> Option<String> {
    let s = state.summary(key)?;
    Some(group_of(state, key, s, grouping).0)
}

/// A section's header and its threads.
type Group = (Option<Header>, Vec<ThreadKey>);

/// The whole list. `filter` matches title and preview, case-insensitively; `open` expands
/// the Muted and Archived sections. Times are in the host's `zone`.
#[allow(clippy::too_many_arguments, reason = "the sidebar is a function of all of these")]
pub fn sections(state: &AppState, map: &AttentionMap, grouping: Grouping, ordering: Ordering, filter: &str, open: Open, now: i64, zone: time::Zone) -> Vec<Section> {
    let needle = filter.trim().to_lowercase();
    let selected = state.selected_thread();
    let text_matches = |s: &ThreadSummary| needle.is_empty() || s.title.to_lowercase().contains(&needle) || s.preview.to_lowercase().contains(&needle);
    let matches = |key: &ThreadKey, s: &ThreadSummary| !s.archived && (grouping.show_dismissed || !state.is_dismissed(key)) && text_matches(s);
    let age = |key: &ThreadKey| {
        RowRight::Age(state.summary(key).and_then(|s| time::secs(&s.updated_at)).map(|u| time::short_age(now - u)).unwrap_or_default())
    };
    let row = |key: &ThreadKey, right: RowRight, two_lines: bool| -> ThreadRow {
        let s = state.summary(key);
        // Every row outside Muted and Archived has a second line: the harness logo, then (in
        // git) the branch and the counts.
        let line2 = if two_lines { line2_of(state, key) } else { None };
        ThreadRow {
            key: key.clone(),
            title: s.map(|s| s.title.clone()).unwrap_or_default(),
            display: display(state, map, key),
            right,
            line2,
            selected: selected == Some(key),
            muted: state.is_muted(key, now),
            archived: s.is_some_and(|s| s.archived),
            dismissed: state.is_dismissed(key),
        }
    };
    let simple_header = |left: String| Header {
        left,
        machine: None,
        connection: None,
        older_daemon: false,
        retry: false,
        temporary: false,
        folder: None,
    };

    let mut out = Vec::new();

    // Pinned, then groups, then muted. There is no Needs-you section (D116): a thread that
    // needs the user floats to the top of its own group, and the title row's button (D117)
    // goes to it.
    let mut pinned: Vec<ThreadKey> = Vec::new();
    let mut groups: BTreeMap<String, Group> = BTreeMap::new();
    let mut muted: Vec<ThreadKey> = Vec::new();
    let mut archived: Vec<ThreadKey> = Vec::new();
    for (key, rec) in &state.threads {
        let s = &rec.summary;
        if s.archived {
            if text_matches(s) && !ordering.running_only {
                archived.push(key.clone());
            }
            continue;
        }
        if !matches(key, s) && !(map.of(key).is_some() && text_matches(s)) {
            continue;
        }
        if ordering.running_only && selected != Some(key) && !shown_when_running_only(state, map, key, ordering) {
            continue;
        }
        // A blocked thread is never hidden: a muted (or dismissed) one that needs the user is
        // listed in its group until answered.
        if state.is_muted(key, now) && map.of(key).is_none() {
            muted.push(key.clone());
            continue;
        }
        if state.is_pinned(key) {
            pinned.push(key.clone());
            continue;
        }
        let (id, header) = group_of(state, key, s, grouping);
        groups.entry(id).or_insert_with(|| (header, Vec::new())).1.push(key.clone());
    }
    let newest_first = |v: &mut Vec<ThreadKey>| v.sort_by(|a, b| updated(state, b).cmp(&updated(state, a)).then_with(|| a.cmp(b)));
    // The chosen order, with the floated threads on top: needs you, then unseen (D116).
    let ordered = |v: &mut Vec<ThreadKey>| {
        let title = |k: &ThreadKey| state.summary(k).map(|s| s.title.as_str()).unwrap_or("");
        let float = |k: &ThreadKey| {
            let d = display(state, map, k);
            if ordering.needs_first && d.attention.is_some() {
                0
            } else if ordering.unseen_first && d.unseen {
                1
            } else {
                2
            }
        };
        v.sort_by(|a, b| {
            float(a).cmp(&float(b)).then_with(|| match ordering.order {
                Order::LastMessage => updated(state, b).cmp(&updated(state, a)),
                Order::AToZ => natural_cmp(title(a), title(b)),
                Order::ZToA => natural_cmp(title(b), title(a)),
            })
            .then_with(|| a.cmp(b))
        });
    };
    if !pinned.is_empty() {
        ordered(&mut pinned);
        let rows: Vec<ThreadRow> = pinned.iter().map(|k| row(k, age(k), true)).collect();
        out.push(Section {
            id: "pinned".into(),
            kind: SectionKind::Pinned,
            header: Some(simple_header(format!("Pinned · {}", rows.len()))),
            collapsed: false,
            count: rows.len(),
            rows,
        });
    }
    let mut groups: Vec<(String, Group)> = groups.into_iter().collect();
    // Groups follow their most recent thread, whatever the order inside them.
    let latest = |keys: &[ThreadKey]| keys.iter().map(|k| updated(state, k)).max().unwrap_or(0);
    groups.sort_by(|a, b| latest(&b.1.1).cmp(&latest(&a.1.1)).then_with(|| a.0.cmp(&b.0)));
    for (id, (header, mut keys)) in groups {
        ordered(&mut keys);
        let rows: Vec<ThreadRow> = keys.iter().map(|k| row(k, age(k), true)).collect();
        out.push(Section { id, kind: SectionKind::Group, header, collapsed: false, count: rows.len(), rows });
    }
    if !muted.is_empty() {
        newest_first(&mut muted);
        let count = muted.len();
        out.push(Section {
            id: "muted".into(),
            kind: SectionKind::Muted,
            header: Some(simple_header(format!("Muted · {count}"))),
            collapsed: !open.muted,
            rows: if open.muted {
                muted
                    .iter()
                    .map(|k| {
                        let right = match state.muted_until(k, now) {
                            Some(u) => RowRight::Until(time::until_label(now, u, zone)),
                            None => age(k),
                        };
                        row(k, right, false)
                    })
                    .collect()
            } else {
                Vec::new()
            },
            count,
        });
    }
    if !archived.is_empty() {
        newest_first(&mut archived);
        let count = archived.len();
        out.push(Section {
            id: "archived".into(),
            kind: SectionKind::Archived,
            header: Some(simple_header(format!("Archived · {count}"))),
            collapsed: !open.archived,
            rows: if open.archived { archived.iter().map(|k| row(k, age(k), false)).collect() } else { Vec::new() },
            count,
        });
    }
    out
}

/// The footer's hub line (D106): a connection icon — crossed out while the hub cannot be
/// reached — and the targets as counts only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HubFooter {
    /// The window is attached to the hub.
    pub reachable: bool,
    /// Enabled targets that are connected.
    pub up: usize,
    /// Enabled targets that are not (hidden when 0).
    pub down: usize,
}

/// The footer's hub line, from the state.
pub fn hub_footer(state: &AppState) -> HubFooter {
    let enabled = state.targets.iter().filter(|t| t.entry.enabled).count();
    let up = state.targets.iter().filter(|t| t.entry.enabled && t.connected()).count();
    HubFooter { reachable: state.hub.attached, up, down: enabled - up }
}

/// Rows must not jump under the pointer (D116): while the user is pointing into the list (or
/// has a row's menu open), a reorder is held back — each section keeps the order it was last
/// shown in, threads new to it go at its end, threads gone from it go — and the chosen order
/// returns once the pointer leaves the list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Steady {
    shown: BTreeMap<String, Vec<ThreadKey>>,
}

impl Steady {
    /// The sections to draw: `fresh` as computed, or, while `hold`, in the order last shown.
    pub fn apply(&mut self, mut fresh: Vec<Section>, hold: bool) -> Vec<Section> {
        if hold {
            for s in &mut fresh {
                let Some(before) = self.shown.get(&s.id) else { continue };
                let rank = |k: &ThreadKey| before.iter().position(|b| b == k).unwrap_or(usize::MAX);
                // A stable sort: new threads keep their fresh order among themselves, at the end.
                s.rows.sort_by_key(|r| rank(&r.key));
            }
        }
        self.shown = fresh.iter().map(|s| (s.id.clone(), s.rows.iter().map(|r| r.key.clone()).collect())).collect();
        fresh
    }
}

/// The Needs-you button (D117): each press opens the next thread that needs the user,
/// cycling through those still unresolved, most recent first. A resolved thread leaves the
/// cycle; with none left the button is not drawn.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NeedsCycle {
    last: Option<ThreadKey>,
}

impl NeedsCycle {
    /// The thread to open now, given the threads that need the user (as [`needs_you`] lists
    /// them); `None` when there are none.
    pub fn next(&mut self, waiting: &[NeedsYouEntry]) -> Option<ThreadKey> {
        if waiting.is_empty() {
            self.last = None;
            return None;
        }
        let at = self.last.as_ref().and_then(|l| waiting.iter().position(|e| &e.key == l)).map_or(0, |i| (i + 1) % waiting.len());
        let key = waiting[at].key.clone();
        self.last = Some(key.clone());
        Some(key)
    }

    /// The button's tooltip: "Next thread that needs you (N)".
    pub fn tooltip(count: usize) -> String {
        format!("Next thread that needs you ({count})")
    }
}
