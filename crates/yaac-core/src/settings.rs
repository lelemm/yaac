//! The settings pages as data (screens 13 and 17; docs/07-ui.md#settings): targets with their
//! state lines and actions, and provider instances per target. Nothing here branches on
//! provider kind.

use std::collections::BTreeSet;

use yaac_protocol::attach::{ConnectionStatus, TargetTransport};
use yaac_protocol::{BackendKind, InstanceId, ProviderKind, ProviderStatus, TargetId};

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
    /// Appearance.
    Appearance,
    /// Advanced.
    Advanced,
}

impl Page {
    /// Every page, in navigation order.
    pub const ALL: [Page; 4] = [Page::Targets, Page::Providers, Page::Appearance, Page::Advanced];

    /// The navigation label.
    pub fn label(self) -> &'static str {
        match self {
            Page::Targets => "Targets",
            Page::Providers => "Providers",
            Page::Appearance => "Appearance",
            Page::Advanced => "Advanced",
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
            }
        })
        .collect();
    let mut kinds: Vec<ProviderKind> = Vec::new();
    for k in t.providers.iter().map(|p| &p.kind).chain(state.targets.iter().flat_map(|t| t.providers.iter().map(|p| &p.kind))) {
        if !kinds.contains(k) {
            kinds.push(k.clone());
        }
    }
    Some(ProvidersView { target: target.clone(), target_name: t.entry.name.to_string(), offline: !t.connected(), rows, kinds })
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
