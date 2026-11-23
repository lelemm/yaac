//! First run (screen 07) and the draft's project picker (screen 08).

use yaac_protocol::attach::{ConnectionStatus, TargetTransport};
use yaac_protocol::{ProviderStatus, TargetId};

use crate::state::AppState;

/// One provider line in the "This computer" card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderLine {
    /// "Codex".
    pub name: String,
    /// Ready to use.
    pub ready: bool,
    /// "ready", "not signed in", "not installed", …
    pub word: String,
    /// What to do, as the target reported it ("Run `claude` in a terminal to sign in"); inline
    /// code in backticks is drawn in monospace.
    pub hint: Option<String>,
}

/// Screen 07.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FirstRunView {
    /// The target shown in the provider card.
    pub target: Option<TargetId>,
    /// The remote or WSL target's name.
    pub other_name: Option<String>,
    /// Its provider instances.
    pub providers: Vec<ProviderLine>,
    /// Still being read from this computer.
    pub checking: bool,
    /// A thread can be started (some instance is ready).
    pub can_start: bool,
}

/// Whether the window is on a fresh install: no thread anywhere, no project, no draft.
pub fn is_first_run(state: &AppState) -> bool {
    state.hub.attached && state.threads.is_empty() && state.projects.is_empty() && state.new_threads.is_empty()
}

fn word(status: &ProviderStatus) -> String {
    match status {
        ProviderStatus::Ready => "ready".into(),
        ProviderStatus::NeedsAuth => "not signed in".into(),
        ProviderStatus::NotInstalled => "not installed".into(),
        ProviderStatus::TooOld => "too old".into(),
        ProviderStatus::Unreachable => "unreachable".into(),
        ProviderStatus::StartFailed => "could not start".into(),
        ProviderStatus::AccessDenied => "access denied".into(),
        ProviderStatus::Disabled => "disabled".into(),
        ProviderStatus::Probing => "checking…".into(),
        ProviderStatus::Unknown(s) => s.clone(),
    }
}

fn has_ready(t: &crate::state::TargetRecord) -> bool {
    t.providers.iter().any(|p| p.status == ProviderStatus::Ready)
}

fn is_local(t: &crate::state::TargetRecord) -> bool {
    matches!(t.entry.transport, TargetTransport::Local(_))
}

/// When no target is chosen, prefer a connected target with a ready provider.
/// Among equally suitable targets, prefer Local.
pub fn preferred_start_target(state: &AppState) -> Option<&TargetId> {
    let connected = || state.targets.iter().filter(|t| t.connected());
    connected()
        .find(|t| is_local(t) && has_ready(t))
        .or_else(|| connected().find(|t| has_ready(t)))
        .or_else(|| connected().find(|t| is_local(t)))
        .or_else(|| connected().next())
        .map(|t| &t.entry.id)
}

/// Screen 07's content. It shows this computer, unless another connected machine has a
/// ready provider and this computer has none.
pub fn first_run(state: &AppState) -> FirstRunView {
    let this = state.targets.iter().find(|t| is_local(t));
    let ready_elsewhere = if this.is_some_and(|t| t.connected() && has_ready(t)) {
        None
    } else {
        state.targets.iter().find(|t| t.connected() && has_ready(t))
    };
    let target = ready_elsewhere.or(this).or_else(|| state.targets.iter().find(|t| t.connected()));
    let providers: Vec<ProviderLine> = target
        .filter(|t| t.connected())
        .map(|t| {
            t.providers
                .iter()
                .map(|p| ProviderLine {
                    name: p.display_name.clone(),
                    ready: p.status == ProviderStatus::Ready,
                    word: word(&p.status),
                    hint: (p.status != ProviderStatus::Ready).then(|| p.status_message.clone()).flatten(),
                })
                .collect()
        })
        .unwrap_or_default();
    FirstRunView {
        target: target.map(|t| t.entry.id.clone()),
        other_name: target.filter(|t| !is_local(t)).map(|t| t.entry.name.to_string()),
        checking: target.is_some_and(|t| t.entry.state.status == ConnectionStatus::Connecting || t.connected() && !t.providers_fresh),
        can_start: providers.iter().any(|p| p.ready),
        providers,
    }
}

/// One project in the picker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickerProject {
    /// Its id.
    pub id: String,
    /// The name the user gave it.
    pub name: String,
    /// Its folder.
    pub folder: String,
    /// The draft's current project.
    pub selected: bool,
}

/// One machine's projects.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickerMachine {
    /// The machine.
    pub target: TargetId,
    /// Its name.
    pub name: String,
    /// "offline", when it is not connected (its projects can still be chosen; the thread
    /// starts once it reconnects — the send is refused until then).
    pub note: Option<String>,
    /// Projects, most recent first.
    pub projects: Vec<PickerProject>,
}

/// The draft's project picker: projects grouped by machine, most recent first (by the latest
/// thread activity in each project; machines by their most recent project), then "New
/// project…" (drawn by the window).
pub fn project_picker(state: &AppState, selected: Option<&str>) -> Vec<PickerMachine> {
    let recency = |pid: &str| -> i64 {
        state
            .threads
            .iter()
            .filter(|(k, _)| state.project_of(k).is_some_and(|p| p.id == pid))
            .filter_map(|(_, r)| crate::time::secs(&r.summary.updated_at))
            .max()
            .unwrap_or(i64::MIN)
    };
    let mut machines: Vec<(i64, PickerMachine)> = Vec::new();
    for t in &state.targets {
        let mut ps: Vec<(i64, PickerProject)> = state
            .projects
            .iter()
            .filter(|p| p.target == t.entry.id)
            .map(|p| {
                (recency(&p.id), PickerProject { id: p.id.clone(), name: p.name.clone(), folder: p.folder.clone(), selected: selected == Some(p.id.as_str()) })
            })
            .collect();
        if ps.is_empty() {
            continue;
        }
        ps.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.name.cmp(&b.1.name)));
        let top = ps.first().map_or(i64::MIN, |p| p.0);
        machines.push((
            top,
            PickerMachine {
                target: t.entry.id.clone(),
                name: t.entry.name.to_string(),
                note: (!t.connected()).then(|| "offline".to_owned()),
                projects: ps.into_iter().map(|p| p.1).collect(),
            },
        ));
    }
    machines.sort_by_key(|m| std::cmp::Reverse(m.0));
    machines.into_iter().map(|m| m.1).collect()
}
