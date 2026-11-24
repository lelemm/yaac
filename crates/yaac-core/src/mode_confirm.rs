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
    /// Whose mode.
    pub target: ModeTarget,
    /// The mode chosen.
    pub mode: ModeId,
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
        ModePick::Confirm(PendingModeConfirm { target, mode })
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
    let mode_name = modes.iter().find(|m| m.id == pending.mode).map_or_else(|| pending.mode.to_string(), |m| m.display_name.clone());
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
