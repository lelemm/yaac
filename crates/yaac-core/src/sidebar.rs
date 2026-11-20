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

/// The three grouping checkboxes, plus "Show archived".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Grouping {
    /// Group by machine (target).
    pub machine: bool,
    /// Group by project.
    pub project: bool,
    /// Group by worktree.
    pub worktree: bool,
    /// Archived threads are listed too.
    pub show_archived: bool,
}

impl Default for Grouping {
    fn default() -> Self {
        // Each row carries its branch, so sections are per project by default.
        Self { machine: true, project: true, worktree: false, show_archived: false }
    }
}

/// What the right end of a row shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowRight {
    /// A relative time ("2h").
    Age(String),
    /// The machine, in the Needs-you section; `muted` adds the word "muted".
    Machine {
        /// Machine name.
        name: String,
        /// The thread is muted.
        muted: bool,
    },
}

/// The second line of a row: what the thread is doing in git. The window draws each part
/// with an icon; every count is shown only when it is not zero.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line2 {
    /// The branch.
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
    /// Archived (only listed with "Show archived").
    pub archived: bool,
}

/// What a section is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SectionKind {
    /// "Needs you · N", first.
    NeedsYou,
    /// "Pinned · N", second.
    Pinned,
    /// A group of the chosen grouping.
    Group,
    /// "Muted · N", last, collapsed unless opened.
    Muted,
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
pub fn needs_you(state: &AppState, map: &AttentionMap) -> Vec<NeedsYouEntry> {
    let mut out: Vec<NeedsYouEntry> = map
        .own
        .iter()
        .filter_map(|(key, a)| {
            let s = state.summary(key)?;
            Some(NeedsYouEntry {
                key: key.clone(),
                title: s.title.clone(),
                attention: *a,
                target_name: state.target_name(&key.target),
                muted: state.is_muted(key),
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
            (ConnectionStatus::Disabled, Some(d)) => d.clone(),
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
    // A linked worktree is named by its checkout; the main checkout by its branch.
    s.repo
        .as_ref()
        .and_then(|r| r.worktree_name.clone())
        .or_else(|| s.git.as_ref().and_then(|g| g.branch.clone()))
        .or_else(|| s.worktree.as_ref().and_then(|w| w.branch.clone()))
        .or_else(|| s.branch.clone())
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

/// The whole list. `filter` matches title and preview, case-insensitively; `muted_open`
/// expands the Muted section.
pub fn sections(
    state: &AppState,
    map: &AttentionMap,
    grouping: Grouping,
    filter: &str,
    muted_open: bool,
    now: i64,
) -> Vec<Section> {
    let needle = filter.trim().to_lowercase();
    let selected = state.selected_thread();
    let matches = |s: &ThreadSummary| {
        (grouping.show_archived || !s.archived)
            && (needle.is_empty() || s.title.to_lowercase().contains(&needle) || s.preview.to_lowercase().contains(&needle))
    };
    let age = |key: &ThreadKey| {
        RowRight::Age(state.summary(key).and_then(|s| time::secs(&s.updated_at)).map(|u| time::short_age(now - u)).unwrap_or_default())
    };
    let row = |key: &ThreadKey, right: RowRight, two_lines: bool| -> ThreadRow {
        let s = state.summary(key);
        let line2 = if two_lines {
            s.and_then(|s| s.git.as_ref().map(|g| (s, g))).map(|(s, g)| Line2 {
                branch: g.branch.clone().or_else(|| s.branch.clone()).unwrap_or_default(),
                added: g.added,
                modified: g.modified,
                deleted: g.deleted,
                ahead: g.ahead,
                behind: g.behind,
                workers: s.children,
            })
        } else {
            None
        };
        ThreadRow {
            key: key.clone(),
            title: s.map(|s| s.title.clone()).unwrap_or_default(),
            display: display(state, map, key),
            right,
            line2,
            selected: selected == Some(key),
            muted: state.is_muted(key),
            archived: s.is_some_and(|s| s.archived),
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

    // Needs you: one-line rows, the machine on the right.
    let needs: Vec<NeedsYouEntry> =
        needs_you(state, map).into_iter().filter(|e| state.summary(&e.key).is_some_and(&matches)).collect();
    if !needs.is_empty() {
        let rows: Vec<ThreadRow> = needs
            .iter()
            .map(|e| row(&e.key, RowRight::Machine { name: e.target_name.clone(), muted: e.muted }, false))
            .collect();
        out.push(Section {
            id: "needs".into(),
            kind: SectionKind::NeedsYou,
            header: Some(simple_header(format!("Needs you · {}", rows.len()))),
            collapsed: false,
            count: rows.len(),
            rows,
        });
    }

    // Pinned, then groups, then muted.
    let mut pinned: Vec<ThreadKey> = Vec::new();
    let mut groups: BTreeMap<String, Group> = BTreeMap::new();
    let mut muted: Vec<ThreadKey> = Vec::new();
    for (key, rec) in &state.threads {
        let s = &rec.summary;
        if !matches(s) {
            continue;
        }
        if state.is_muted(key) {
            // A blocked muted thread is listed under Needs you only.
            if map.of(key).is_none() {
                muted.push(key.clone());
            }
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
    if !pinned.is_empty() {
        newest_first(&mut pinned);
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
    for (_, (_, keys)) in &mut groups {
        newest_first(keys);
    }
    groups.sort_by(|a, b| {
        let la = a.1.1.first().map_or(0, |k| updated(state, k));
        let lb = b.1.1.first().map_or(0, |k| updated(state, k));
        lb.cmp(&la).then_with(|| a.0.cmp(&b.0))
    });
    for (id, (header, keys)) in groups {
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
            collapsed: !muted_open,
            rows: if muted_open { muted.iter().map(|k| row(k, age(k), false)).collect() } else { Vec::new() },
            count,
        });
    }
    out
}

/// "Hub running · 3 of 4 targets": connected of enabled targets.
pub fn hub_status(state: &AppState) -> String {
    if !state.hub.attached {
        return "Hub not attached".to_owned();
    }
    let enabled = state.targets.iter().filter(|t| t.entry.enabled).count();
    let connected = state.targets.iter().filter(|t| t.entry.enabled && t.connected()).count();
    format!("Hub running · {connected} of {enabled} targets")
}
