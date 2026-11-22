//! Moving threads to another project or worktree (D139) — **on the same machine**.
//!
//! Where a thread is listed is YAAC's own placement, kept by `yaacd` on the target
//! (`thread/move`), apart from the harness's working directory. Where the thread's provider
//! has `threadMoveWorkingDirectory` the agent also works in the new folder from its next turn;
//! without it only the placement changes. Everything here reads capabilities and summaries,
//! never the provider's kind.
//!
//! Three ways in, one model: the Move dialog (a thread's menu, the selection's menu, the
//! Commander), and dropping rows on another group of the sidebar ([`drop_on`]).

use std::collections::BTreeSet;

use yaac_protocol::{Capability, TargetId, ThreadState, ThreadSummary};

use crate::key::ThreadKey;
use crate::projects::Project;
use crate::sidebar::{Grouping, Section, SectionKind};
use crate::state::AppState;
use crate::thread_menu::can;

/// The hint over another machine's group while rows are dragged.
pub const OTHER_MACHINE: &str = "A thread cannot move to another machine";

/// Where threads go: a project of their machine and, optionally, one checkout of its
/// repository (its root). `None`: the project's own folder.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Destination {
    /// The project's id on its target.
    pub project: String,
    /// A checkout's root, when one was chosen.
    pub worktree: Option<String>,
}

/// One worktree a project offers: a checkout of its repository that YAAC knows on that
/// machine — the project's own, and those its threads are in or were moved to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorktreeChoice {
    /// The checkout's root.
    pub root: String,
    /// What the worktree level calls it: a linked checkout's name, or the branch.
    pub label: String,
    /// It is the project's own checkout.
    pub own: bool,
}

fn sep(path: &str) -> char {
    if path.contains('\\') && !path.contains('/') { '\\' } else { '/' }
}

fn basename(path: &str) -> &str {
    path.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().unwrap_or(path)
}

/// `root` joined with a `/`-separated relative path ("." is the root itself).
fn join(root: &str, relative: &str) -> String {
    if relative == "." || relative.is_empty() {
        return root.to_owned();
    }
    let s = sep(root);
    let rel: String = relative.split('/').filter(|c| !c.is_empty() && *c != ".").collect::<Vec<_>>().join(&s.to_string());
    format!("{}{s}{rel}", root.trim_end_matches(['/', '\\']))
}

/// The projects of a machine, by name.
pub fn projects_on<'a>(state: &'a AppState, target: &TargetId) -> Vec<&'a Project> {
    let mut v: Vec<&Project> = state.projects.iter().filter(|p| &p.target == target).collect();
    v.sort_by(|a, b| crate::sidebar::natural_cmp(&a.name, &b.name).then_with(|| a.wire_id.cmp(&b.wire_id)));
    v
}

fn project<'a>(state: &'a AppState, target: &TargetId, id: &str) -> Option<&'a Project> {
    state.projects.iter().find(|p| &p.target == target && p.wire_id == id)
}

/// The checkout a thread is listed in: where it was moved, else its own.
fn checkout_of(s: &ThreadSummary) -> Option<String> {
    s.placement.as_ref().and_then(|p| p.worktree.as_ref()).or_else(|| s.repo.as_ref().map(|r| &r.root)).map(ToString::to_string)
}

/// The label of the checkout a thread is listed in.
fn checkout_label(s: &ThreadSummary) -> Option<String> {
    if let Some(p) = &s.placement
        && p.worktree.is_some()
    {
        return p.worktree_name.clone().or_else(|| p.worktree.as_ref().map(|w| basename(w.as_str()).to_owned()));
    }
    s.repo
        .as_ref()
        .and_then(|r| r.worktree_name.clone())
        .or_else(|| s.git.as_ref().and_then(|g| g.branch.clone()))
        .or_else(|| s.worktree.as_ref().and_then(|w| w.branch.clone()))
        .or_else(|| s.branch.clone())
}

/// The worktrees a project offers, its own checkout first. Empty outside git.
pub fn worktrees(state: &AppState, p: &Project) -> Vec<WorktreeChoice> {
    let Some(repo) = &p.repo else { return Vec::new() };
    let mut out: Vec<WorktreeChoice> = Vec::new();
    let mut seen = BTreeSet::new();
    let mut push = |out: &mut Vec<WorktreeChoice>, root: String, label: Option<String>, own: bool| {
        if root.is_empty() || !seen.insert(root.clone()) {
            return;
        }
        let label = label.unwrap_or_else(|| basename(&root).to_owned());
        out.push(WorktreeChoice { root, label, own });
    };
    // The project's own checkout, named like the sidebar names it: its checkout name, or the
    // branch a thread there reports.
    let own_label = repo.checkout.clone().or_else(|| {
        state
            .threads
            .iter()
            .filter(|(k, _)| k.target == p.target)
            .find(|(_, r)| checkout_of(&r.summary).as_deref() == Some(repo.root.as_str()))
            .and_then(|(_, r)| checkout_label(&r.summary))
    });
    push(&mut out, repo.root.clone(), own_label, true);
    let mut others: Vec<(String, Option<String>)> = Vec::new();
    for (k, r) in &state.threads {
        if k.target != p.target {
            continue;
        }
        let s = &r.summary;
        let same_repo = s.repo.as_ref().is_some_and(|tr| tr.repo.as_str() == repo.repo);
        let placed_here = s.placement.as_ref().is_some_and(|pl| pl.project.as_str() == p.wire_id && pl.worktree.is_some());
        if placed_here {
            others.push((s.placement.as_ref().and_then(|pl| pl.worktree.as_ref()).map(ToString::to_string).unwrap_or_default(), checkout_label(s)));
        } else if same_repo && let Some(tr) = &s.repo {
            others.push((tr.root.to_string(), checkout_label(s)));
        }
    }
    others.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
    for (root, label) in others {
        push(&mut out, root, label, false);
    }
    out
}

/// The folder the agent works in after a move to `dest` (under `threadMoveWorkingDirectory`):
/// the project's folder, or the same place in the chosen checkout.
pub fn folder_for(p: &Project, worktree: Option<&str>) -> String {
    match (worktree, &p.repo) {
        (Some(root), Some(repo)) => join(root, &repo.relative),
        _ => p.folder.clone(),
    }
}

/// Where a thread is listed now, as a destination (`None`: a temporary project).
pub fn current(state: &AppState, key: &ThreadKey) -> Option<Destination> {
    let s = state.summary(key)?;
    let p = state.project_of(key)?;
    // A checkout counts only when it is one of the project's repository.
    let worktree = match (&s.placement, &p.repo, &s.repo) {
        (Some(pl), _, _) => pl.worktree.as_ref().map(ToString::to_string),
        (None, Some(pr), Some(tr)) if tr.repo.as_str() == pr.repo => Some(tr.root.to_string()),
        _ => None,
    };
    Some(Destination { project: p.wire_id.clone(), worktree })
}

/// Whether a thread listed at `now` is already at `dest` (a destination without a worktree is
/// the project's own checkout).
fn already(state: &AppState, target: &TargetId, now: Option<&Destination>, dest: &Destination) -> bool {
    let Some(now) = now else { return false };
    if now.project != dest.project {
        return false;
    }
    let own = project(state, target, &dest.project).and_then(|p| p.repo.as_ref()).map(|r| r.root.clone());
    let a = now.worktree.clone().or_else(|| own.clone());
    let b = dest.worktree.clone().or(own);
    a == b
}

/// Why a thread cannot be moved now; `None`: it can.
pub fn blocked(state: &AppState, key: &ThreadKey) -> Option<&'static str> {
    let Some(s) = state.summary(key) else { return Some("This thread is no longer listed") };
    if s.archived {
        return Some("An archived thread cannot be moved");
    }
    if !can(state, key, &Capability::ThreadMove) {
        return Some("This machine's yaacd cannot move threads yet; update it");
    }
    let connected = state.target(&key.target).is_some_and(|t| t.connected());
    if !connected || state.stopped_distro(&key.target).is_some() {
        return Some("Its machine is not connected");
    }
    if !state.control(key).may_act() {
        return Some("This thread is in use elsewhere");
    }
    None
}

/// Whether a move changes this thread's working directory: its provider can, and the folder
/// differs from the one it works in.
pub fn changes_folder(state: &AppState, key: &ThreadKey, folder: &str) -> bool {
    can(state, key, &Capability::ThreadMoveWorkingDirectory) && state.summary(key).is_some_and(|s| s.cwd.as_str().trim_end_matches(['/', '\\']) != folder.trim_end_matches(['/', '\\']))
}

fn threads(n: usize) -> String {
    if n == 1 { "1 thread".to_owned() } else { format!("{n} threads") }
}

// ── The Move dialog ──────────────────────────────────────────────────────────────────────

/// A project in the dialog's dropdown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectChoice {
    /// Its id on the target.
    pub id: String,
    /// Its name.
    pub name: String,
    /// Its folder (the row's second line).
    pub folder: String,
}

/// The Move dialog: Project (the machine's projects), Worktree (optional: the project's
/// checkouts), what will happen to the working directory, Move and Cancel. The machine never
/// changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MoveDialog {
    /// The threads it moves (those that can be moved), in the order given.
    pub keys: Vec<ThreadKey>,
    /// Their machine.
    pub target: TargetId,
    /// "Move “Fix frame decoder”" or "Move 3 threads".
    pub title: String,
    /// The chosen project (its id).
    pub project: Option<String>,
    /// The chosen checkout's root; `None`: the project's own folder.
    pub worktree: Option<String>,
    /// "1 of 3 is in use elsewhere and stays where it is", when some are left out.
    pub skipped: Option<String>,
    /// Why nothing can be moved (the dialog then offers only Close).
    pub blocked: Option<String>,
}

impl MoveDialog {
    /// The dialog for `keys` (one thread, or a selection in the order shown).
    pub fn new(state: &AppState, keys: &[ThreadKey]) -> Self {
        let machines: BTreeSet<&TargetId> = keys.iter().map(|k| &k.target).collect();
        let target = keys.first().map(|k| k.target.clone()).unwrap_or_else(|| TargetId::new(""));
        let title = match keys {
            [one] => format!("Move “{}”", state.summary(one).map_or_else(|| "this thread".to_owned(), |s| s.title.clone())),
            _ => format!("Move {}", threads(keys.len())),
        };
        let mut d = Self { keys: Vec::new(), target: target.clone(), title, project: None, worktree: None, skipped: None, blocked: None };
        if keys.is_empty() {
            d.blocked = Some("Nothing to move".into());
            return d;
        }
        if machines.len() > 1 {
            d.blocked = Some(format!("These threads are on {} machines. {OTHER_MACHINE}; select threads of one machine.", machines.len()));
            return d;
        }
        let mut skips: Vec<&'static str> = Vec::new();
        for k in keys {
            match blocked(state, k) {
                None => d.keys.push(k.clone()),
                Some(why) => skips.push(why),
            }
        }
        if d.keys.is_empty() {
            d.blocked = Some(skips.first().copied().unwrap_or("Nothing can be moved").to_owned());
            return d;
        }
        if !skips.is_empty() {
            let why = skips[0].trim_start_matches("This thread is ").trim_start_matches("This ");
            d.skipped = Some(format!("{} of {} cannot be moved ({}); {} where {} {}", skips.len(), keys.len(), why.to_lowercase(), if skips.len() == 1 { "it stays" } else { "they stay" }, if skips.len() == 1 { "it" } else { "they" }, if skips.len() == 1 { "is" } else { "are" }));
        }
        let list = projects_on(state, &target);
        if list.is_empty() {
            d.blocked = Some(format!("{} has no projects yet. Create one first.", state.target_name(&target)));
            return d;
        }
        // Preselect the first project that is not where the (first) thread is now.
        let now = current(state, &d.keys[0]);
        d.project = list.iter().find(|p| now.as_ref().is_none_or(|n| n.project != p.wire_id)).or(list.first()).map(|p| p.wire_id.clone());
        d
    }

    /// The machine's projects, for the dropdown.
    pub fn projects(&self, state: &AppState) -> Vec<ProjectChoice> {
        projects_on(state, &self.target).into_iter().map(|p| ProjectChoice { id: p.wire_id.clone(), name: p.name.clone(), folder: p.folder.clone() }).collect()
    }

    fn chosen<'a>(&self, state: &'a AppState) -> Option<&'a Project> {
        self.project.as_deref().and_then(|id| project(state, &self.target, id))
    }

    /// The chosen project's name.
    pub fn project_label(&self, state: &AppState) -> String {
        self.chosen(state).map_or_else(|| "Choose a project".to_owned(), |p| p.name.clone())
    }

    /// Choose a project (the worktree goes back to the project's own folder).
    pub fn set_project(&mut self, id: &str) {
        if self.project.as_deref() != Some(id) {
            self.project = Some(id.to_owned());
            self.worktree = None;
        }
    }

    /// The chosen project's worktrees (empty outside git: the Worktree field is then off).
    pub fn worktrees(&self, state: &AppState) -> Vec<WorktreeChoice> {
        self.chosen(state).map(|p| worktrees(state, p)).unwrap_or_default()
    }

    /// The Worktree field's text.
    pub fn worktree_label(&self, state: &AppState) -> String {
        let list = self.worktrees(state);
        if list.is_empty() {
            return "None · not in git".to_owned();
        }
        match &self.worktree {
            None => list.iter().find(|w| w.own).map_or_else(|| "The project's own folder".to_owned(), |w| format!("{} · the project's own", w.label)),
            Some(root) => list.iter().find(|w| &w.root == root).map_or_else(|| basename(root).to_owned(), |w| w.label.clone()),
        }
    }

    /// Choose a worktree (`None`, or the project's own checkout: the project's folder).
    pub fn set_worktree(&mut self, state: &AppState, root: Option<&str>) {
        let own = self.worktrees(state).into_iter().find(|w| w.own).map(|w| w.root);
        self.worktree = root.filter(|r| Some(*r) != own.as_deref()).map(str::to_owned);
    }

    /// Where the threads go.
    pub fn destination(&self) -> Option<Destination> {
        Some(Destination { project: self.project.clone()?, worktree: self.worktree.clone() })
    }

    /// The folder the agents work in after the move, where their harness can change it.
    pub fn folder(&self, state: &AppState) -> Option<String> {
        self.chosen(state).map(|p| folder_for(p, self.worktree.as_deref()))
    }

    /// The moves Move sends: every thread not already there.
    pub fn moves(&self, state: &AppState) -> Vec<(ThreadKey, Destination)> {
        let Some(dest) = self.destination() else { return Vec::new() };
        self.keys.iter().filter(|k| !already(state, &self.target, current(state, k).as_ref(), &dest)).map(|k| (k.clone(), dest.clone())).collect()
    }

    /// Move is offered: something can be moved and is not already there.
    pub fn can_move(&self, state: &AppState) -> bool {
        self.blocked.is_none() && !self.moves(state).is_empty()
    }

    /// What will happen to the working directory — the capability sentence. One thread:
    /// "The agent will work in <path> from its next turn" or "Only where YAAC lists it
    /// changes; the agent keeps working in <old path>".
    pub fn sentence(&self, state: &AppState) -> String {
        if let Some(b) = &self.blocked {
            return b.clone();
        }
        let Some(folder) = self.folder(state) else { return String::new() };
        let moves = self.moves(state);
        if moves.is_empty() {
            return if self.keys.len() == 1 { "It is already there.".to_owned() } else { "They are already there.".to_owned() };
        }
        let follow: Vec<&ThreadKey> = moves.iter().map(|(k, _)| k).filter(|k| changes_folder(state, k, &folder)).collect();
        let running = moves.iter().any(|(k, _)| follow.contains(&k) && state.summary(k).is_some_and(|s| matches!(s.state, ThreadState::Running | ThreadState::Waiting | ThreadState::NeedsAttention)));
        let tail = if running { " A running turn finishes where it is." } else { "" };
        if let [(k, _)] = moves.as_slice() {
            let cwd = state.summary(k).map(|s| s.cwd.to_string()).unwrap_or_default();
            return if follow.is_empty() {
                format!("Only where YAAC lists it changes; the agent keeps working in {cwd}.")
            } else {
                format!("The agent will work in {folder} from its next turn.{tail}")
            };
        }
        let (n, m) = (follow.len(), moves.len() - follow.len());
        match (n, m) {
            (0, _) => "Only where YAAC lists them changes; the agents keep working in their folders.".to_owned(),
            (_, 0) => format!("The agents will work in {folder} from their next turn.{tail}"),
            _ => format!(
                "{n} {} will work in {folder} from {} next turn. For {m}, only where YAAC lists {} changes; {} keep{} {} folder.{tail}",
                if n == 1 { "agent" } else { "agents" },
                if n == 1 { "its" } else { "their" },
                if m == 1 { "it" } else { "them" },
                if m == 1 { "it" } else { "they" },
                if m == 1 { "s" } else { "" },
                if m == 1 { "its" } else { "their" },
            ),
        }
    }

    /// Whether the move changes some agent's working directory.
    pub fn changes_folder(&self, state: &AppState) -> bool {
        let Some(folder) = self.folder(state) else { return false };
        self.moves(state).iter().any(|(k, _)| changes_folder(state, k, &folder))
    }
}

// ── Drag and drop ────────────────────────────────────────────────────────────────────────

/// Rows can be dragged to another group only when the grouping has a project or worktree
/// level: grouped by machine alone (or not at all) there is nothing to move to.
pub fn can_drag(grouping: Grouping) -> bool {
    grouping.project || grouping.worktree
}

/// What dropping dragged rows on a section does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Drop {
    /// Nothing: not a group (Pinned, Muted, Archived), or the rows' own group.
    Nothing,
    /// Not allowed: the reason, shown as the hint with the not-allowed cursor.
    Refused(String),
    /// Move these threads there. `label` names the group ("api", "yaac · feature-x");
    /// `confirm`: some agent's working directory changes, so ask first.
    Move {
        /// The threads, in the order given (those already there and those that cannot be
        /// moved are left out).
        keys: Vec<ThreadKey>,
        /// Where.
        to: Destination,
        /// The group's name.
        label: String,
        /// Ask first: a working directory changes.
        confirm: bool,
    },
}

/// What dropping `dragged` on `section` does, under `grouping`. Drop on a group = move to that
/// group's project (and checkout when grouping by worktree), on the same machine only.
pub fn drop_on(state: &AppState, grouping: Grouping, section: &Section, dragged: &[ThreadKey]) -> Drop {
    if !can_drag(grouping) || section.kind != SectionKind::Group || dragged.is_empty() {
        return Drop::Nothing;
    }
    let machines: BTreeSet<&TargetId> = dragged.iter().map(|k| &k.target).collect();
    let [machine] = machines.into_iter().collect::<Vec<_>>()[..] else { return Drop::Refused(OTHER_MACHINE.to_owned()) };
    // Every dragged row already in this group: nothing happens.
    if dragged.iter().all(|k| crate::sidebar::section_of(state, grouping, k).as_deref() == Some(section.id.as_str())) {
        return Drop::Nothing;
    }
    let rows: Vec<&ThreadKey> = section.rows.iter().map(|r| &r.key).filter(|k| &k.target == machine).collect();
    if rows.is_empty() {
        return Drop::Refused(OTHER_MACHINE.to_owned());
    }
    let mut projects = BTreeSet::new();
    for k in &rows {
        match state.project_of(k) {
            Some(p) => {
                projects.insert(p.wire_id.clone());
            }
            None => return Drop::Refused("This folder is not a project yet; create a project for it first".to_owned()),
        }
    }
    if projects.len() != 1 {
        return Drop::Refused("This group holds several projects; drop on one of them".to_owned());
    }
    let Some(pid) = projects.into_iter().next() else { return Drop::Nothing };
    let Some(p) = project(state, machine, &pid) else { return Drop::Nothing };
    let mut label = p.name.clone();
    let mut worktree = None;
    if grouping.worktree && p.repo.is_some() {
        let roots: BTreeSet<Option<String>> = rows.iter().map(|k| state.summary(k).and_then(checkout_of)).collect();
        if roots.len() != 1 {
            return Drop::Refused("This group holds several checkouts; drop on one of them".to_owned());
        }
        if let Some(Some(root)) = roots.into_iter().next() {
            let wl = rows.iter().find_map(|k| state.summary(k).and_then(checkout_label)).unwrap_or_else(|| basename(&root).to_owned());
            label = format!("{label} · {wl}");
            let own = p.repo.as_ref().map(|r| r.root.clone());
            worktree = (Some(&root) != own.as_ref()).then_some(root);
        }
    }
    let to = Destination { project: pid, worktree };
    let mut keys = Vec::new();
    let mut why = None;
    for k in dragged {
        if crate::sidebar::section_of(state, grouping, k).as_deref() == Some(section.id.as_str()) || already(state, machine, current(state, k).as_ref(), &to) {
            continue;
        }
        match blocked(state, k) {
            None => keys.push(k.clone()),
            Some(w) => why = why.or(Some(w)),
        }
    }
    if keys.is_empty() {
        return why.map_or(Drop::Nothing, |w| Drop::Refused(w.to_owned()));
    }
    let folder = folder_for(p, to.worktree.as_deref());
    let confirm = keys.iter().any(|k| changes_folder(state, k, &folder));
    Drop::Move { keys, to, label, confirm }
}

/// The confirmation of a drop that changes a working directory: "Move 2 threads to api?" and
/// the same sentence as the Move dialog.
pub fn drop_dialog(state: &AppState, keys: &[ThreadKey], to: &Destination) -> MoveDialog {
    let mut d = MoveDialog::new(state, keys);
    d.project = Some(to.project.clone());
    d.worktree = to.worktree.clone();
    d
}

// ── What a move leaves behind ────────────────────────────────────────────────────────────

/// The bottom bar after a move that needed no confirmation: "Moved 2 threads to api · Undo".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MoveNotice {
    /// "Moved 2 threads to api".
    pub text: String,
    /// Undo: each thread back where it was (`None`: where its folder puts it).
    pub undo: Vec<(ThreadKey, Option<Destination>)>,
}

/// What Undo sends for a thread: its placement before the move (`None`: it had none, so the
/// placement is forgotten and its folder decides again).
pub fn undo_of(state: &AppState, key: &ThreadKey) -> Option<Destination> {
    let s = state.summary(key)?;
    let pl = s.placement.as_ref()?;
    Some(Destination { project: pl.project.to_string(), worktree: pl.worktree.as_ref().map(ToString::to_string) })
}

/// The notice for moving `keys` to the group or project called `label`.
pub fn notice(state: &AppState, keys: &[ThreadKey], label: &str) -> MoveNotice {
    let text = match keys {
        [one] => format!("Moved “{}” to {label}", state.summary(one).map_or_else(|| "the thread".to_owned(), |s| s.title.clone())),
        _ => format!("Moved {} to {label}", threads(keys.len())),
    };
    MoveNotice { text, undo: keys.iter().map(|k| (k.clone(), undo_of(state, k))).collect() }
}

/// The name of a destination: "api", or "api · feature-x" with a worktree.
pub fn label(state: &AppState, target: &TargetId, to: &Destination) -> String {
    let Some(p) = project(state, target, &to.project) else { return to.project.clone() };
    match &to.worktree {
        None => p.name.clone(),
        Some(root) => {
            let wl = worktrees(state, p).into_iter().find(|w| &w.root == root).map_or_else(|| basename(root).to_owned(), |w| w.label);
            format!("{} · {wl}", p.name)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_join_with_the_targets_separator() {
        assert_eq!(join("/srv/wt", "."), "/srv/wt");
        assert_eq!(join("/srv/wt/", "services/billing"), "/srv/wt/services/billing");
        assert_eq!(join("C:\\wt", "services/billing"), "C:\\wt\\services\\billing");
        assert_eq!(basename("/srv/wt/hotfix/"), "hotfix");
    }
}
