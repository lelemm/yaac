//! Confirming a mode that bypasses approvals (screen 30; docs/07-ui.md, the Mode row).
//!
//! Choosing a mode the provider marks `skipsApprovals` does not apply it: it opens a
//! confirmation naming the thread, the machine, and the folder. Only "Bypass permissions"
//! applies it; Cancel (the default, also Esc) does nothing. The answer is asked for every time
//! and never remembered. Which modes count is decided only by `skips_approvals` — never by a
//! mode id or a provider kind.

use yaac_protocol::{ModeId, ModeInfo};

use crate::key::ThreadKey;
use crate::state::{AppState, Selection};

/// Whose mode is being chosen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModeTarget {
    /// A started thread (its next message).
    Thread(ThreadKey),
    /// A new-thread draft.
    Draft(u64),
}

/// A mode choice waiting for the user's confirmation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingModeConfirm {
    /// Source mode as offered when the scope confirmation opened.
    pub chosen: Option<ModeInfo>,
    /// Snapshot shown to the user. New workers require a fresh confirmation.
    pub workers: Vec<ModeRecipient>,
    /// Whether this thread also needs an interruption.
    pub interrupt_root: bool,
    /// Whose mode.
    pub target: ModeTarget,
    /// The mode chosen.
    pub mode: ModeId,
}

/// One destination and the interruption the user was shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModeRecipient {
    pub key: ThreadKey,
    pub interrupt: bool,
    pub label: String,
}

/// Match spawn's preference order: closest safe autonomy, then the same id, the default,
/// and finally the provider's first choice at that rank. The daemon rechecks at handoff.
pub(crate) fn equivalent<'a>(source: &ModeInfo, modes: &'a [ModeInfo], same_instance: bool) -> Option<&'a ModeInfo> {
    if same_instance { return modes.iter().find(|m| m.id == source.id); }
    let rank = |m: &ModeInfo| if m.skips_approvals { Some(5) } else { m.autonomy.as_ref()?.rank() };
    let ceiling = rank(source)?;
    modes.iter().rev().filter_map(|m| {
        let level = rank(m)?;
        (level <= ceiling).then_some(((level, m.id == source.id, m.is_default), m))
    }).max_by_key(|(score, _)| *score).map(|(_, mode)| mode)
}

fn interrupted(state: &AppState, key: &ThreadKey, root: &ThreadKey, source: &ModeInfo) -> bool {
    let Some(row) = state.summary(key) else { return false };
    if row.subagent.as_ref().and_then(|s| s.read_only.as_ref()).is_some() { return false }
    let current = row.permission_mode.as_ref().or_else(|| state.transcripts.get(key)?.effective.as_ref()?.applied.mode.as_ref());
    if let Some(current) = current {
        let same = key.target == root.target && state.summary(root).is_some_and(|s| s.instance == row.instance);
        let modes = modes_for(state, &ModeTarget::Thread(key.clone()));
        if equivalent(source, &modes, same).is_none_or(|desired| &desired.id == current) { return false }
    }
    let active = row.turn_active.unwrap_or_else(|| crate::queue::running(state, key)
        || matches!(row.state, yaac_protocol::ThreadState::Running | yaac_protocol::ThreadState::NeedsAttention));
    let live = state.transcripts.get(key).and_then(|t| t.effective.as_ref()).map(|e| &e.capabilities)
        .or_else(|| state.provider_of(key).map(|p| &p.capabilities))
        .is_some_and(|caps| caps.contains(&yaac_protocol::Capability::ModeChangeLive));
    active && !live
}

pub(crate) fn tree(state: &AppState, target: &ModeTarget, mode: &ModeId, modes: &[ModeInfo]) -> Option<PendingModeConfirm> {
    let ModeTarget::Thread(key) = target else { return None };
    let current = state.next_settings.get(key).and_then(|s| s.mode.as_ref())
        .or_else(|| state.transcripts.get(key)?.effective.as_ref()?.applied.mode.as_ref())
        .or_else(|| state.summary(key)?.permission_mode.as_ref());
    if !modes.iter().any(|m| m.skips_approvals && (&m.id == mode || Some(&m.id) == current)) { return None }
    let source = modes.iter().find(|m| &m.id == mode)?;
    let workers: Vec<_> = state.threads.iter().filter(|(k,_)| crate::copies::depth_below(state, k, key).is_some())
        .map(|(k,row)| {
            let interrupt = interrupted(state, k, key, source);
            let offline = !state.target(&k.target).is_some_and(|t| t.connected());
            let status = if row.summary.subagent.as_ref().and_then(|s| s.read_only.as_ref()).is_some() { " · controlled by its parent" } else if offline && interrupt { " · offline · interrupt and resume if still running" }
                else if interrupt { " · interrupt and resume" } else if offline { " · offline" } else { "" };
            ModeRecipient { key: k.clone(), interrupt,
                label: format!("{} · {}{}", row.summary.title, state.target_name(&k.target), status) }
        }).collect();
    (!workers.is_empty()).then(|| PendingModeConfirm { target: target.clone(), mode: mode.clone(), chosen: Some(source.clone()), workers, interrupt_root: interrupted(state, key, key, source) })
}

/// What choosing a mode does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModePick {
    /// Apply it now.
    Apply(ModeTarget, ModeId),
    /// Ask first.
    Confirm(PendingModeConfirm),
}

/// Decide what choosing `mode` does, from the modes on offer.
pub fn pick(modes: &[ModeInfo], target: ModeTarget, mode: ModeId) -> ModePick {
    if modes.iter().any(|m| m.id == mode && m.skips_approvals) {
        ModePick::Confirm(PendingModeConfirm { target, mode, chosen: None, workers: Vec::new(), interrupt_root: false })
    } else {
        ModePick::Apply(target, mode)
    }
}

/// The dialog of screen 30.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfirmView {
    /// "Let this agent act without asking?"
    pub title: String,
    /// The mode's name as the provider gives it ("bypass permissions"), set in bold in the body.
    pub mode_name: String,
    /// "In **bypass permissions** mode the agent runs commands and changes files without asking
    /// you first." — split around the mode name.
    pub body: (String, String),
    /// "Thread: Fix frame decoder", "Machine: devbox", "Folder: /home/dev/yaac".
    pub lines: Vec<String>,
    /// "This applies to this thread only. You can switch back at any time."
    pub footer: String,
    /// The filled button, in the failed colour: the mode's name, capitalised.
    pub confirm_label: String,
}

/// The modes on offer for a mode target.
pub fn modes_for(state: &AppState, target: &ModeTarget) -> Vec<ModeInfo> {
    match target {
        ModeTarget::Thread(key) => state
            .transcripts
            .get(key)
            .and_then(|t| t.effective.as_ref())
            .map(|e| e.modes.clone())
            .filter(|m| !m.is_empty())
            .or_else(|| state.provider_of(key).map(|p| p.modes.clone()))
            .unwrap_or_default(),
        ModeTarget::Draft(id) => state
            .new_threads
            .iter()
            .find(|d| d.id == *id)
            .and_then(|d| {
                let t = state.target(d.target.as_ref()?)?;
                let inst = d.instance.clone().or_else(|| {
                    t.providers.iter().find(|p| p.status == yaac_protocol::ProviderStatus::Ready).map(|p| p.instance.clone())
                })?;
                t.provider(&inst).map(|p| p.modes.clone())
            })
            .unwrap_or_default(),
    }
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// The dialog for a pending confirmation.
pub fn view(state: &AppState, pending: &PendingModeConfirm) -> ConfirmView {
    let modes = modes_for(state, &pending.target);
    let chosen = pending.chosen.as_ref().or_else(|| modes.iter().find(|m| m.id == pending.mode));
    let mode_name = chosen.map_or_else(|| pending.mode.to_string(), |m| m.display_name.clone());
    let (thread, target, folder) = match &pending.target {
        ModeTarget::Thread(key) => {
            let s = state.summary(key);
            (s.map_or_else(|| key.thread.to_string(), |s| s.title.clone()), key.target.clone(), s.map(|s| s.cwd.to_string()))
        }
        ModeTarget::Draft(id) => {
            let d = state.new_threads.iter().find(|d| d.id == *id);
            (
                "New thread".to_owned(),
                d.and_then(|d| d.target.clone()).unwrap_or_default(),
                d.and_then(|d| d.cwd.as_ref().map(ToString::to_string)),
            )
        }
    };
    let mut lines = vec![format!("Thread: {thread}"), format!("Machine: {}", state.target_name(&target))];
    if let Some(f) = folder {
        lines.push(format!("Folder: {f}"));
    }
    if !pending.workers.is_empty() {
        let skips = chosen.is_some_and(|m| m.skips_approvals);
        lines.push(format!("{} workers under this thread (all levels)", pending.workers.len()));
        let count = pending.workers.iter().filter(|r| r.interrupt).count();
        if count > 0 {
            let names = pending.workers.iter().filter(|r| r.interrupt).map(|r|
                state.summary(&r.key).map_or(r.key.thread.as_str(), |s| s.title.as_str())).collect::<Vec<_>>().join(", ");
            lines.push(format!("{count} running {} will be interrupted for a moment and resumed in the new mode: {names}.",
                if count == 1 { "worker" } else { "workers" }));
        }
        if pending.interrupt_root { lines.push("This thread will also be interrupted and resumed.".to_owned()); }
        return ConfirmView { title: format!("Change mode for {thread}?"), mode_name,
            body: ("In ".into(), if skips { " mode the agent runs commands and changes files without asking you first." } else { " mode the harness applies its approval rules." }.into()),
            confirm_label: "Only this thread".into(), lines, footer: "Apply this mode to the workers as well?".into() };
    }
    ConfirmView {
        title: "Let this agent act without asking?".to_owned(),
        body: ("In ".to_owned(), " mode the agent runs commands and changes files without asking you first.".to_owned()),
        confirm_label: capitalise(&mode_name),
        mode_name,
        lines,
        footer: "This applies to this thread only. You can switch back at any time.".to_owned(),
    }
}

/// Whether a pending confirmation still makes sense (its thread or draft is still the one
/// shown); the window drops it otherwise.
pub fn still_relevant(state: &AppState, pending: &PendingModeConfirm) -> bool {
    match (&pending.target, &state.selection) {
        (ModeTarget::Thread(k), Selection::Thread(s)) => k == s,
        (ModeTarget::Draft(d), Selection::NewThread(s)) => d == s,
        _ => false,
    }
}
