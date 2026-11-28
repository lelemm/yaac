//! The settings pages as data (screens 13 and 17; docs/07-ui.md#settings): targets with their
//! state lines and actions, and provider instances per target. Nothing here branches on
//! provider kind.

use std::collections::BTreeSet;

use yaac_protocol::attach::{ConnectionStatus, TargetTransport};
use yaac_protocol::{BackendKind, Capability, InstanceId, ProviderKind, ProviderStatus, TargetId};

use crate::attention::Mark;
use crate::ssh_setup::SetupRecord;
use crate::state::{AppState, TargetRecord};

/// The settings pages.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Page {
    /// Targets (13).
    #[default]
    Targets,
    /// Providers (17).
    Providers,
    /// The installation-owned skill catalog.
    Skills,
    /// Appearance.
    Appearance,
    /// Window: the tray and the renderer (D127, D128).
    Window,
    /// Android: whether the hub accepts the Android client over Tailscale (P377).
    Android,
    /// Keyboard: every command's keys (D131).
    Keyboard,
    /// Terminal: copy on select, Ctrl+V, the multi-line paste question (P339).
    Terminal,
    /// Desktop request notifications.
    Notifications,
    Approvals,
    /// Hubs: the hubs this window can attach to (P412, desktop).
    Hubs,
    /// Advanced.
    Advanced,
    /// About: this YAAC's version and build, its components, the latest release.
    About,
}

impl Page {
    /// Every page, in navigation order.
    pub const ALL: [Page; 13] = [Page::Targets, Page::Providers, Page::Skills, Page::Approvals, Page::Appearance, Page::Window, Page::Android, Page::Keyboard, Page::Terminal, Page::Notifications, Page::Hubs, Page::Advanced, Page::About];

    /// The navigation label.
    pub fn label(self) -> &'static str {
        match self {
            Page::Targets => "Targets",
            Page::Providers => "Providers",
            Page::Skills => "Skills",
            Page::Appearance => "Appearance",
            Page::Window => "Window",
            Page::Android => "Android",
            Page::Keyboard => "Keyboard",
            Page::Terminal => "Terminal",
            Page::Approvals => "Approvals",
            Page::Notifications => "Notifications",
            Page::Hubs => "Hubs",
            Page::Advanced => "Advanced",
            Page::About => "About",
        }
    }
}

/// An action on an SSH row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SshAction {
    /// Reconnect now.
    Retry,
    /// Run Setup again.
    SetUp,
    /// Remove the key YAAC installed.
    Revoke,
    /// Revoke is not possible, and why ("YAAC installed nothing on this server").
    RevokeUnavailable(String),
    /// Forget the connection (offering Revoke first when it applies).
    Remove,
}

/// One SSH connection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SshRow {
    /// The target.
    pub id: TargetId,
    /// Its name.
    pub name: String,
    /// "user@host:port".
    pub address: String,
    /// The state mark.
    pub mark: Mark,
    /// "Ready — set up by YAAC on 2026-11-20".
    pub line: String,
    /// The enable switch is shown (a connection that is set up).
    pub switch: Option<bool>,
    /// Actions, in order.
    pub actions: Vec<SshAction>,
}

/// One WSL connection: a distro and an optional user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WslRow {
    /// The target.
    pub id: TargetId,
    /// The distro.
    pub distro: String,
    /// The user, or "(default)".
    pub user: String,
    /// Enabled.
    pub enabled: bool,
    /// "second session to this distro".
    pub note: Option<String>,
    /// The mark.
    pub mark: Mark,
}

/// The Targets page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetsView {
    /// This computer and whether it is enabled.
    pub local: Option<(TargetId, bool)>,
    /// SSH connections.
    pub ssh: Vec<SshRow>,
    /// WSL connections.
    pub wsl: Vec<WslRow>,
}

/// The one-line legend under the SSH list.
pub const LEGEND: &str = "Disable keeps the connection but stops using it. Remove forgets it. Revoke removes the key YAAC installed on the server.";

fn mark_of(t: &TargetRecord) -> Mark {
    match (&t.entry.state.status, t.entry.enabled) {
        (_, false) | (ConnectionStatus::Disabled | ConnectionStatus::Stopped, _) => Mark::Off,
        (ConnectionStatus::Connected, _) => Mark::Done,
        (ConnectionStatus::Incompatible, _) => Mark::Failed,
        _ => Mark::Offline,
    }
}

fn ssh_row(t: &TargetRecord, record: Option<&SetupRecord>) -> Option<SshRow> {
    let TargetTransport::Ssh(ssh) = &t.entry.transport else { return None };
    let mut address = ssh.host.clone();
    if let Some(u) = &ssh.user {
        address = format!("{u}@{address}");
    }
    if let Some(p) = ssh.port {
        address = format!("{address}:{p}");
    }
    let connected = t.connected();
    let offline_detail = || {
        let d = t.entry.state.detail.clone().unwrap_or_default();
        if d.is_empty() { "Offline".to_owned() } else { format!("Offline · {d}") }
    };
    let state_word = |suffix: String| -> String {
        if !t.entry.enabled {
            format!("Off · {suffix}")
        } else if connected {
            format!("Ready · {suffix}")
        } else {
            format!("{} · {suffix}", offline_detail())
        }
    };
    let mut actions = Vec::new();
    let retry = t.entry.enabled && !connected;
    let (mark, line, switch) = match record {
        Some(SetupRecord::InstalledByYaac { date, .. }) => {
            if retry {
                actions.push(SshAction::Retry);
            }
            actions.push(SshAction::Revoke);
            (mark_of(t), state_word(format!("set up by YAAC on {date}")), Some(t.entry.enabled))
        }
        Some(SetupRecord::AlreadyInPlace { .. }) | None => {
            if retry {
                actions.push(SshAction::Retry);
            }
            actions.push(SshAction::RevokeUnavailable("YAAC installed nothing on this server".to_owned()));
            let line = if connected || !t.entry.enabled {
                state_word("access was already in place".to_owned())
            } else {
                offline_detail()
            };
            (mark_of(t), line, Some(t.entry.enabled))
        }
        Some(SetupRecord::NotSetUp) => {
            actions.push(SshAction::SetUp);
            (Mark::Off, "Not set up".to_owned(), None)
        }
        Some(SetupRecord::Revoked { date }) => {
            actions.push(SshAction::SetUp);
            (Mark::Off, format!("Not set up · YAAC's key was removed on {date}"), None)
        }
        Some(SetupRecord::RemovedByHand { date }) => {
            actions.push(SshAction::SetUp);
            (Mark::Off, format!("Not set up · removed by hand on {date}"), None)
        }
    };
    actions.push(SshAction::Remove);
    Some(SshRow { id: t.entry.id.clone(), name: t.entry.name.to_string(), address, mark, line, switch, actions })
}

/// The Targets page.
pub fn targets_view(state: &AppState) -> TargetsView {
    let mut local = None;
    let mut ssh = Vec::new();
    let mut wsl = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for t in &state.targets {
        match &t.entry.transport {
            TargetTransport::Local(_) if local.is_none() => local = Some((t.entry.id.clone(), t.entry.enabled)),
            TargetTransport::Ssh(_) => ssh.extend(ssh_row(t, state.ssh_records.get(&t.entry.id))),
            TargetTransport::Wsl(w) => {
                let second = !seen.insert(w.distro.clone());
                wsl.push(WslRow {
                    id: t.entry.id.clone(),
                    distro: w.distro.clone(),
                    user: w.user.clone().unwrap_or_else(|| "(default)".to_owned()),
                    enabled: t.entry.enabled,
                    note: second.then(|| "second session to this distro".to_owned()),
                    mark: mark_of(t),
                });
            }
            _ => {}
        }
    }
    TargetsView { local, ssh, wsl }
}

// ── WSL: the installed distros ──────────────────────────────────────────────────────────

/// One installed WSL distro, as the hub lists it (`hub/wsl/distros`). The window never runs
/// `wsl.exe` itself.
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WslDistro {
    /// Its name, as `wsl.exe -d` takes it.
    pub name: String,
    /// Windows' default distro.
    #[serde(default)]
    pub default: bool,
    /// Running now.
    #[serde(default)]
    pub running: bool,
    /// WSL version (1 or 2), when known.
    #[serde(default)]
    pub version: Option<u8>,
}

/// Where asking the hub for the installed distros stands.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum DistroPhase {
    /// Not asked since the window started.
    #[default]
    NotAsked,
    /// Asked; no answer yet.
    Loading,
    /// Answered with [`WslDistros::list`].
    Ready,
    /// Listing failed; the hub's message. The distro is typed instead.
    Failed(String),
}

/// The installed distros and the one chosen in the Add row.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WslDistros {
    /// Where the last request stands.
    pub phase: DistroPhase,
    /// The last list the hub gave.
    pub list: Vec<WslDistro>,
    /// The distro chosen in the dropdown (`None`: nothing chosen).
    pub selected: Option<String>,
}

/// Distro names compare without case, as Windows does.
fn same_distro(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

fn has_target(targets: &[TargetRecord], distro: &str) -> bool {
    targets.iter().any(|t| matches!(&t.entry.transport, TargetTransport::Wsl(w) if same_distro(&w.distro, distro)))
}

/// The distro to select once a list arrives: the one already chosen, if it is still
/// installed; else Windows' default distro, unless a WSL target for it exists already (for
/// any user) — then nothing, so the user picks another.
pub fn choose_distro(list: &[WslDistro], targets: &[TargetRecord], current: Option<&str>) -> Option<String> {
    if let Some(c) = current.and_then(|c| list.iter().find(|d| same_distro(&d.name, c))) {
        return Some(c.name.clone());
    }
    list.iter().find(|d| d.default).filter(|d| !has_target(targets, &d.name)).map(|d| d.name.clone())
}

/// One entry of the distro dropdown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DistroOption {
    /// The distro's name.
    pub name: String,
    /// "Ubuntu · default".
    pub label: String,
    /// "running", "stopped".
    pub state: &'static str,
    /// A WSL target for it exists already. Still selectable: another user opens a second
    /// session to it.
    pub added: bool,
}

/// The placeholder of the dropdown when nothing is chosen.
pub const CHOOSE_DISTRO: &str = "Choose a distro";

/// The distro control of the WSL Add row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DistroPicker {
    /// The hub is listing them: the dropdown says "Loading…".
    Loading,
    /// A dropdown of the installed distros, with the one chosen (`None`: [`CHOOSE_DISTRO`]).
    Choose {
        /// Every installed distro, in the order `wsl.exe` lists them.
        options: Vec<DistroOption>,
        /// The chosen one.
        selected: Option<String>,
    },
    /// No list to choose from: the distro is typed, and this says why.
    Typed {
        /// The hub's message, or "No distros were found".
        message: String,
    },
}

/// The distro control of the WSL Add row, from what the hub last said.
pub fn distro_picker(state: &AppState) -> DistroPicker {
    let d = &state.wsl_distros;
    match &d.phase {
        DistroPhase::NotAsked | DistroPhase::Loading => DistroPicker::Loading,
        DistroPhase::Failed(m) => DistroPicker::Typed { message: m.clone() },
        DistroPhase::Ready if d.list.is_empty() => DistroPicker::Typed { message: "No distros were found".to_owned() },
        DistroPhase::Ready => DistroPicker::Choose {
            options: d
                .list
                .iter()
                .map(|x| DistroOption {
                    name: x.name.clone(),
                    label: if x.default { format!("{} · default", x.name) } else { x.name.clone() },
                    state: if x.running { "running" } else { "stopped" },
                    added: has_target(&state.targets, &x.name),
                })
                .collect(),
            selected: d.selected.clone(),
        },
    }
}

/// One provider instance on the Providers page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstanceRow {
    /// The instance.
    pub instance: InstanceId,
    /// "Codex · work".
    pub name: String,
    /// Done when ready, needs-you otherwise.
    pub mark: Mark,
    /// "Ready", or "Not signed in — run `claude` in a terminal on this computer".
    pub status: String,
    /// Facts in order: version and backend, socket, how the daemon was found, models, account.
    pub facts: Vec<String>,
    /// The socket path, shown in monospace, when the instance has an explicit one.
    pub socket: Option<String>,
    /// The instance has a stored configuration the user can edit and remove.
    pub editable: bool,
    /// Its configuration is enabled.
    pub enabled: bool,
    /// The "Offer YAAC tools to sessions started outside YAAC" switch.
    pub outside_tools: bool,
    /// The "Trust folders YAAC works in" switch (D147): shown only when the instance reports
    /// `folderTrust`; on unless its stored configuration turns it off.
    pub trust_folders: Option<bool>,
    /// The "Harness home" setting (P300): shown only when the instance reports
    /// `configurableHome`.
    pub home: Option<HomeField>,
    /// Available model guidance editors on this instance.
    pub model_guidance: Vec<ModelGuidanceRow>,
}

/// The model identity and current guidance shown in Settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelGuidanceRow {
    /// The model alias or ID displayed by the harness.
    pub id: yaac_protocol::ModelId,
    /// Its stable identity for an override.
    pub specific: yaac_protocol::ModelId,
    /// The model name.
    pub name: String,
    /// Generated or user-edited recommendation.
    pub best_use: Option<yaac_protocol::ModelBestUse>,
    /// Whether this provider supports editing model guidance.
    pub guidance_editable: bool,
    /// Whether the model is on, when the target supports the switch.
    pub enabled: Option<bool>,
    /// Whether the model explains approvals, when this choice is offered.
    pub explains_approvals: Option<bool>,
}

impl ModelGuidanceRow {
    /// The current recommendation, including the empty state.
    pub fn summary(&self) -> &str {
        self.best_use.as_ref().map_or("No guidance yet", |hint| &hint.summary)
    }

    /// Whether a manual edit is active.
    pub fn custom(&self) -> bool {
        self.best_use.as_ref().is_some_and(|hint| hint.custom)
    }

    /// Where the displayed guidance came from.
    pub fn provenance(&self) -> &str {
        match &self.best_use {
            Some(hint) if hint.custom => "User-edited",
            Some(_) => "Generated",
            None => "",
        }
    }
}

/// An instance's harness home on the Providers page (P300).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HomeField {
    /// The home the instance's configuration sets; `None`: the harness default.
    pub set: Option<String>,
    /// The home in use now, as the instance reports it: the field's placeholder.
    pub effective: Option<String>,
}

impl HomeField {
    /// The placeholder of an empty field: the folder in use, or the default in words.
    pub fn placeholder(&self) -> String {
        self.effective.as_ref().map_or_else(|| "Harness default".to_owned(), |e| format!("Default · {e}"))
    }

    /// The one-line summary under the instance: "Home · /home/me/.codex (default)".
    pub fn line(&self) -> String {
        match (&self.set, &self.effective) {
            (Some(s), _) => format!("Home · {s}"),
            (None, Some(e)) => format!("Home · {e} (default)"),
            (None, None) => "Home · harness default".to_owned(),
        }
    }
}

/// A harness-home change that leads to another store, waiting for the user (P300).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HomeConfirm {
    /// The target.
    pub target: TargetId,
    /// The instance.
    pub instance: InstanceId,
    /// The home asked for; `None`: back to the harness default.
    pub home: Option<String>,
    /// The home in use now, as the target said.
    pub current: Option<String>,
    /// The home asked for, as the target resolved it.
    pub next: Option<String>,
}

impl HomeConfirm {
    /// The question.
    pub fn text(&self) -> String {
        let next = self.next.clone().or_else(|| self.home.clone()).unwrap_or_else(|| "the harness default".to_owned());
        let current = self.current.clone().unwrap_or_else(|| "the current home".to_owned());
        format!(
            "{next} is another store than {current}. This instance's threads stay with {current} and show as unavailable until the home is changed back. Change the home?"
        )
    }
}

/// The Providers page for one target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProvidersView {
    /// The target.
    pub target: TargetId,
    /// Its name.
    pub target_name: String,
    /// Not connected: the rows come from the cache.
    pub offline: bool,
    /// Instances.
    pub rows: Vec<InstanceRow>,
    /// The kinds the Add form offers: those this target's (and every known target's) yaacd
    /// reports. Never a list in the window.
    pub kinds: Vec<ProviderKind>,
    /// The kinds whose instances report `configurableHome` (P300): the Add form offers a
    /// harness home for them.
    pub home_kinds: Vec<ProviderKind>,
}

fn status_word(s: &ProviderStatus) -> &'static str {
    match s {
        ProviderStatus::Ready => "Ready",
        ProviderStatus::NeedsAuth => "Not signed in",
        ProviderStatus::NotInstalled => "Not installed",
        ProviderStatus::TooOld => "Too old",
        ProviderStatus::Unreachable => "Unreachable",
        ProviderStatus::StartFailed => "Could not start",
        ProviderStatus::AccessDenied => "Access denied",
        ProviderStatus::Disabled => "Disabled",
        ProviderStatus::Probing => "Checking…",
        ProviderStatus::Unknown(_) => "Unknown state",
    }
}

/// The Providers page for `target`.
pub fn providers_view(state: &AppState, target: &TargetId) -> Option<ProvidersView> {
    let t = state.target(target)?;
    let configs = t.instance_configs.as_deref().unwrap_or(&[]);
    let rows = t
        .providers
        .iter()
        .map(|p| {
            let config = configs.iter().find(|c| c.id == p.instance);
            let socket = config.and_then(|c| c.endpoint.as_ref()).and_then(|e| e.socket.as_ref()).map(ToString::to_string);
            let mut facts = Vec::new();
            let version = p.harness_version.clone().unwrap_or_default();
            match &p.backend {
                BackendKind::Daemon => facts.push(format!("daemon {version}").trim().to_owned()),
                BackendKind::Sdk => {
                    if !version.is_empty() {
                        facts.push(version);
                    }
                    facts.push("SDK".to_owned());
                }
                BackendKind::Unknown(b) => facts.push(format!("{b} {version}").trim().to_owned()),
            }
            // SEAM (wire field missing): whether yaacd found the daemon running or started it
            // is not in `ProviderDescriptor`; shown once the descriptor carries it.
            if p.models_complete && !p.models.is_empty() {
                let n = p.models.iter().filter(|m| !m.hidden).count();
                facts.push(format!("{n} model{}", if n == 1 { "" } else { "s" }));
            }
            if let Some(a) = &p.store.account {
                facts.push(format!("account: {a}"));
            }
            let status = match (&p.status, &p.status_message) {
                (ProviderStatus::Ready, _) | (_, None) => status_word(&p.status).to_owned(),
                (_, Some(m)) => format!("{} · {m}", status_word(&p.status)),
            };
            let guidance_editable = p.capabilities.contains(&Capability::ModelBestUse);
            let availability = p.capabilities.contains(&Capability::ModelAvailability);
            let explanations = p.capabilities.contains(&Capability::CommandExplanations);
            let mut model_guidance = Vec::new();
            if guidance_editable || availability || explanations {
                model_guidance.extend(p.models.iter().map(|model| (model, true))
                    .chain(p.disabled_models.iter().map(|model| (model, false)))
                    .map(|(model, enabled)| ModelGuidanceRow {
                        id: model.id.clone(),
                        specific: model.resolved_model.as_ref().unwrap_or(&model.id).clone(),
                        name: model.display_name.clone(),
                        best_use: model.best_use.clone(),
                        guidance_editable,
                        enabled: availability.then_some(enabled),
                        explains_approvals: (explanations && !model.hidden).then(|| p.approval_explanation_model.as_ref() == Some(&model.id)),
                    }));
                if availability {
                    model_guidance.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
                }
            }
            InstanceRow {
                instance: p.instance.clone(),
                name: p.display_name.clone(),
                mark: if p.status == ProviderStatus::Ready { Mark::Done } else if p.status == ProviderStatus::Disabled { Mark::Off } else { Mark::NeedsYou },
                status,
                facts,
                socket,
                editable: config.is_some_and(|c| !c.implicit),
                enabled: config.is_none_or(|c| c.enabled),
                outside_tools: state.outside_tools.contains(&(target.clone(), p.instance.clone())),
                trust_folders: p.capabilities.contains(&Capability::FolderTrust).then(|| config.is_none_or(|c| c.trust_folders != Some(false))),
                home: p.capabilities.contains(&Capability::ConfigurableHome).then(|| HomeField {
                    set: config.and_then(|c| c.home.as_ref()).map(ToString::to_string),
                    effective: p.store.home.as_ref().map(ToString::to_string),
                }),
                model_guidance,
            }
        })
        .collect();
    let mut kinds: Vec<ProviderKind> = Vec::new();
    for k in t.providers.iter().map(|p| &p.kind).chain(state.targets.iter().flat_map(|t| t.providers.iter().map(|p| &p.kind))) {
        if !kinds.contains(k) {
            kinds.push(k.clone());
        }
    }
    let mut home_kinds: Vec<ProviderKind> = Vec::new();
    for p in t.providers.iter().filter(|p| p.capabilities.contains(&Capability::ConfigurableHome)) {
        if !home_kinds.contains(&p.kind) {
            home_kinds.push(p.kind.clone());
        }
    }
    Some(ProvidersView { target: target.clone(), target_name: t.entry.name.to_string(), offline: !t.connected(), rows, kinds, home_kinds })
}

/// A kind's label for the Add form: the display name of an instance of that kind without its
/// instance name, else the kind as the target spells it.
pub fn kind_label(state: &AppState, kind: &ProviderKind) -> String {
    state
        .targets
        .iter()
        .flat_map(|t| t.providers.iter())
        .find(|p| &p.kind == kind && p.name.is_none())
        .map_or_else(|| kind.as_str().to_owned(), |p| p.display_name.clone())
}

/// The diagnostics of one target: what the connection and each provider reported.
pub fn diagnostics(state: &AppState, target: &TargetId) -> Vec<(String, String)> {
    let Some(t) = state.target(target) else { return Vec::new() };
    let mut out = vec![
        ("Connection".to_owned(), t.entry.state.status.as_str().to_owned()),
        ("Enabled".to_owned(), if t.entry.enabled { "yes" } else { "no" }.to_owned()),
    ];
    if let Some(d) = &t.entry.state.detail {
        out.push(("Detail".to_owned(), d.clone()));
    }
    if let Some(d) = &t.entry.state.daemon {
        let mut v = d.build.clone();
        if d.older {
            v.push_str(" · older than this YAAC; it will be updated when idle");
        }
        out.push(("yaacd".to_owned(), v));
        if !d.schema_hash_matches {
            out.push(("Schema".to_owned(), "does not match this build".to_owned()));
        }
    }
    out.push(("Threads indexed".to_owned(), state.threads.keys().filter(|k| &k.target == target).count().to_string()));
    if !t.stale_instances.is_empty() {
        out.push(("Incomplete scan".to_owned(), t.stale_instances.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")));
    }
    for p in &t.providers {
        let mut v = status_word(&p.status).to_owned();
        if let Some(m) = &p.status_message {
            v.push_str(" · ");
            v.push_str(m);
        }
        out.push((p.display_name.clone(), v));
    }
    out
}

/// Explanations run on the card's target. An instance/model pair appears once across
/// connected targets that currently offer it.
pub fn explanation_choices(state: &AppState) -> Vec<(yaac_protocol::ExplanationModel, String)> {
    let mut choices = Vec::new();
    for target in state.targets.iter().filter(|t| t.connected()) {
        for provider in &target.providers {
            if provider.status != ProviderStatus::Ready || !provider.capabilities.contains(&Capability::CommandExplanations) { continue; }
            for model in provider.models.iter().filter(|m| !m.hidden) {
                let value = yaac_protocol::ExplanationModel { instance: provider.instance.clone(), model: model.id.clone() };
                if !choices.iter().any(|(v, _)| v == &value) {
                    choices.push((value, format!("{} · {}", provider.display_name, model.display_name)));
                }
            }
        }
    }
    choices
}
