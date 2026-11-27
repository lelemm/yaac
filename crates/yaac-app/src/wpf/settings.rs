//! Settings and first run projected from the same app state as the egui window.

use super::*;
use crate::settings::{RevokePanel, SshPanel};
use yaac_core::attention::Mark;
use yaac_core::commands::{self as keys, Scope};
use yaac_core::components::{self, Component, ComponentStatus};
use yaac_core::display::{Host, Renderer, in_use_line, offered};
use yaac_core::first_run;
use yaac_core::keys::{Chord, MouseButton, canonical_key, format_chords};
use yaac_core::prefs::SendKey;
use yaac_core::settings::{self as model, DistroPicker, SshAction};
use yaac_core::ssh_setup::{
    Credential, Phase, Revoke, RevokePhase, Secret, SetupRecord, SignInMethod, SshConn, SshSetup,
    StepMark, form_ready,
};
use yaac_core::state::HelloOutcome;
use yaac_protocol::attach::{ConnectionStatus, SshTransport, TargetTransport, WslTransport};
use yaac_protocol::{InstanceId, TargetId};

const REPOSITORY: &str = "https://github.com/lelemm/yaac";

fn action(id: &str, label: &str, enabled: bool) -> Value {
    json!({ "id": id, "label": label, "enabled": enabled })
}

fn with(mut value: Value, props: Value) -> Value {
    if let (Some(value), Value::Object(props)) = (value.as_object_mut(), props) {
        value.extend(props);
    }
    value
}

fn args(mut action: Value, values: Value) -> Value {
    action["values"] = values;
    action
}

fn primary(action: Value) -> Value {
    with(action, json!({ "kind": "primary", "default": true }))
}

fn submit(action: Value, clear: bool) -> Value {
    with(action, json!({ "submit": true, "clear": clear }))
}

fn cancel(id: &str, title: &str) -> Value {
    with(
        action(id, title, true),
        json!({ "cancel": true, "detail": "Esc" }),
    )
}

fn toggle(id: &str, title: &str, on: bool, enabled: bool) -> Value {
    with(
        action(id, title, enabled),
        json!({ "kind": "toggle", "checked": on }),
    )
}

fn field(id: &str, label: &str, value: &str, placeholder: &str) -> Value {
    json!({ "id": id, "label": label, "value": value, "placeholder": placeholder })
}

fn choice(id: &str, label: &str, value: Option<&str>, action: &str, options: Vec<Value>) -> Value {
    let mut field =
        json!({ "id": id, "label": label, "type": "choice", "value": value, "action": action });
    field["options"] = Value::Array(options);
    field
}

fn section(id: &str, title: &str, rows: Vec<Value>) -> Value {
    let mut section = json!({ "id": id, "title": title });
    section["rows"] = Value::Array(rows);
    section
}

fn tone(mark: Mark) -> &'static str {
    match mark {
        Mark::Done => "success",
        Mark::NeedsYou | Mark::Paused => "warning",
        Mark::Failed => "error",
        Mark::Running | Mark::Waiting => "accent",
        _ => "normal",
    }
}

fn settings_target<'a>(app: &'a YaacApp, target: Option<&'a TargetId>) -> Option<&'a TargetId> {
    let state = &app.client.state;
    target.filter(|t| state.target(t).is_some()).or_else(|| {
        state
            .targets
            .iter()
            .find(|t| t.connected())
            .or_else(|| state.targets.first())
            .map(|t| &t.entry.id)
    })
}

pub(super) fn upkeep(app: &mut YaacApp, target: Option<&TargetId>) {
    let mut actions = Vec::new();
    if let Some(s) = &mut app.ui.set.ssh
        && let Phase::Ready(record) = &s.setup.phase
        && !s.recorded
    {
        let conn = &s.setup.conn;
        let id = s
            .existing
            .clone()
            .unwrap_or_else(|| TargetId::new(conn.name.clone()));
        if s.existing.is_some() {
            actions.push(Action::TargetEnabled(id.clone(), true));
        } else {
            let transport = SshTransport {
                host: conn.host.clone(),
                port: (conn.port != 22).then_some(conn.port),
                user: Some(conn.user.clone()),
            };
            actions.push(Action::AddTarget(
                conn.name.clone(),
                TargetTransport::Ssh(transport),
            ));
        }
        actions.push(Action::SetRecord(id, record.clone()));
        s.recorded = true;
    }
    actions.extend(
        std::mem::take(&mut app.ui.set.records)
            .into_iter()
            .map(|(target, record)| Action::SetRecord(target, record)),
    );
    if app.ui.settings
        && app.ui.settings_page == Page::Providers
        && let Some(target) = settings_target(app, target).cloned()
        && let Some(record) = app
            .client
            .state
            .targets
            .iter_mut()
            .find(|t| t.entry.id == target && t.connected() && t.instance_configs.is_none())
    {
        record.instance_configs = Some(Vec::new());
        app.client.list_instances(&target);
    }
    if !actions.is_empty() {
        app.apply(actions);
    }
}

pub(super) fn view(app: &YaacApp, target: Option<&TargetId>) -> Value {
    if let HelloOutcome::WindowOutdated { message } = &app.client.state.hub.outcome {
        return json!({
            "mode": "updateNeeded", "theme": app.ui.theme.id(), "title": "Update YAAC", "detail": message,
            "footer": app.hub_machine().map_or_else(
                || "A newer YAAC is already running on this computer.".to_owned(),
                |pc| format!("The hub on {pc} is a different YAAC build."),
            )
        });
    }
    if !app.ui.settings {
        if first_run::is_first_run(&app.client.state)
            && app.client.state.selection == Selection::None
        {
            return with(welcome(app), json!({ "theme": app.ui.theme.id() }));
        }
        return json!({ "mode": "hidden" });
    }
    let page = app.ui.settings_page;
    let content = match page {
        Page::Targets => targets(app),
        Page::Providers => providers(app, target),
        Page::Skills => crate::catalog_settings::wpf_view(app),
        Page::Appearance => appearance(app),
        Page::Window => window(app),
        Page::Android => android(app),
        Page::Terminal => terminal_settings(app),
        Page::Keyboard => keyboard(app),
        Page::Approvals => {
            let choices = model::explanation_choices(&app.client.state);
            let models = &app.client.state.hub.explanations;
            let fields: Vec<_> = [("main", "Main model", &models.main), ("fallback", "Fallback model", &models.fallback)].into_iter().map(|(slot, label, selected)| {
                let value = serde_json::to_string(selected).unwrap();
                let mut options = vec![action("null", "None", true)];
                options.extend(choices.iter().map(|(v, name)| action(&serde_json::to_string(v).unwrap(), name, true)));
                if let Some(v) = selected && !choices.iter().any(|(c, _)| c == v) {
                    options.push(action(&value, &format!("{} · {}", v.instance, v.model), false));
                }
                choice(slot, label, Some(&value), &format!("approvals.{slot}"), options)
            }).collect();
            json!({ "sections": [section("models", "Explain commands with", vec![json!({"id":"models","fields":fields})])] })
        }
        Page::Notifications => {
            let prefs = app.client.state.hub.notifications;
            // These are the hub's: on the phone they also govern the desktop's, and the phone's
            // own notifications follow the same switches (P379). Say so, and where to silence
            // only the phone.
            let android = app.ui.platform == yaac_core::input::Platform::Android;
            let enabled = if android { "Show request notifications (on the desktop and this phone)" } else { "Show desktop request notifications" };
            let mut rows = Vec::new();
            if android {
                let pc = app.hub_machine().unwrap_or_else(|| "the PC".into());
                rows.push(json!({"id":"shared","detail": format!(
                    "These settings belong to the hub on {pc}: they apply to its desktop notifications and to this phone's alike. To silence only this phone, turn off YAAC's notifications in Android's settings. The sound choice applies to the desktop; the phone uses its own notification sound.")}));
            }
            rows.extend([
                json!({"id":"options","actions":[
                    toggle("notifications.enabled",enabled,prefs.enabled,true),
                    toggle("notifications.questions","Include questions",prefs.include_questions,true),
                    toggle("notifications.sound","Play a sound",prefs.sound,true)]}),
                json!({"id":"sound","label":"Sound","fields":[{"id":"soundChoice","type":"radio","action":"notifications.choice",
                    "value":prefs.sound_choice,"options":[action("reminder","Reminder",true),action("default","Default",true)]}]})
            ]);
            json!({ "sections": [section("notifications", "Notifications", rows)] })
        }
        Page::Advanced => advanced(app),
        Page::About => about(app),
    };
    with(
        content,
        json!({ "mode": "settings", "page": page.label().to_ascii_lowercase(), "theme": app.ui.theme.id() }),
    )
}

fn welcome(app: &YaacApp) -> Value {
    let v = first_run::first_run(&app.client.state);
    let mut rows: Vec<_> = v
        .providers
        .iter()
        .enumerate()
        .map(|(i, provider)| {
            json!({
                "id": format!("provider-{i}"), "label": provider.name, "status": provider.word,
                "detail": provider.hint, "tone": if provider.ready { "success" } else { "warning" }
            })
        })
        .collect();
    if v.checking {
        rows.insert(
            0,
            json!({ "id": "checking", "detail": "Checking what is installed…" }),
        );
    }
    let mut actions = Vec::new();
    if let Some(target) = v.target.filter(|_| !v.providers.is_empty()) {
        actions.push(args(
            action("providers.refresh", "Refresh", true),
            json!({ "target": target }),
        ));
    }
    actions.push(primary(with(
        action("first.start", "Start a thread", v.can_start),
        json!({
            "detail": if v.can_start { "Enter" } else { "No provider is ready yet." }
        }),
    )));
    // On a client of another machine's hub (the phone), "this computer" is that machine.
    let pc = app.hub_machine();
    let computer = v.other_name.clone().or_else(|| pc.as_ref().map(|pc| format!("{pc} (the hub)"))).unwrap_or_else(|| "This computer".into());
    let ssh_detail = pc.as_ref().map_or_else(|| "Run agents on another machine over SSH.".to_owned(), |pc| format!("Run agents on another machine over SSH; {pc} connects to it."));
    let (wsl_title, wsl_detail) = pc.as_ref().map_or_else(
        || ("Turn on WSL".to_owned(), "Run agents inside a Linux distro on this PC.".to_owned()),
        |pc| (format!("Turn on WSL on {pc}"), format!("Run agents inside a Linux distro on {pc}.")),
    );
    json!({
        "mode": "firstRun", "title": "YAAC",
        "detail": "Run your coding agents on any machine, and let them work together.",
        "footer": "Agents keep running when you close this window.",
        "sections": [
            { "id": "computer", "title": computer, "rows": rows, "actions": actions },
            { "id": "ssh", "title": "Add a server", "detail": ssh_detail,
              "actions": [action("ssh.add", "Add server…", true)] },
            { "id": "wsl", "title": wsl_title, "detail": wsl_detail,
              "actions": [with(action("first.wsl", "Turn on…", app.host_windows), json!({ "detail": if app.host_windows { "" } else { "Windows only" } }))] }
        ]
    })
}

fn targets(app: &YaacApp) -> Value {
    let state = &app.client.state;
    let v = model::targets_view(state);
    let mut sections = Vec::new();
    let mut header = vec![action("ssh.import", "Import from SSH config", true)];
    let add = action("ssh.add", "Add SSH server", true);
    header.push(if app.ui.set.ssh.is_some() || app.ui.set.revoke.is_some() {
        add
    } else {
        with(add, json!({ "kind": "primary" }))
    });

    if let Some(hosts) = &app.ui.set.ssh_config_hosts {
        let options = hosts
            .iter()
            .map(|host| {
                args(
                    action("ssh.importHost", host, true),
                    json!({ "host": host }),
                )
            })
            .enumerate()
            .map(|(index, mut item)| {
                item["id"] = json!(format!("ssh.importHost.{index}"));
                item
            })
            .collect::<Vec<_>>();
        sections.push(json!({
            "id": "sshImport", "kind": "panel", "title": "Import from SSH config",
            "detail": if hosts.is_empty() { "No hosts were found in ~/.ssh/config." } else { "Choose a host to fill in the form." },
            "actions": options
        }));
    }
    if let Some(panel) = &app.ui.set.ssh {
        sections.push(ssh_panel(panel));
    }
    if let Some(panel) = &app.ui.set.revoke {
        sections.push(revoke_panel(app, panel));
    }

    let mut local = Vec::new();
    if let Some((id, on)) = &v.local
        && let Some(record) = state.target(id)
    {
        let status = match record.entry.state.status {
            ConnectionStatus::Connected => "Connected",
            ConnectionStatus::Connecting => "Connecting…",
            ConnectionStatus::Disabled => "Off",
            _ => "Offline",
        };
        let label = app.hub_machine().map_or_else(|| "This computer".to_owned(), |pc| format!("{pc} (the hub)"));
        local.push(json!({ "id": id, "label": label, "status": status, "detail": record.entry.state.detail,
            "actions": [args(toggle("target.enabled", "Enabled", *on, true), json!({ "target": id }))] }));
    }
    sections.push(section("local", "Local", local));

    let mut ssh = Vec::new();
    if v.ssh.is_empty() {
        ssh.push(json!({ "id": "empty", "detail": "No SSH servers yet." }));
    }
    for row in v.ssh {
        let context = json!({ "target": row.id });
        let mut actions = Vec::new();
        if let Some(on) = row.switch {
            actions.push(args(
                toggle("target.enabled", "Enabled", on, true),
                context.clone(),
            ));
        }
        actions.extend(row.actions.iter().map(|a| {
            args(
                match a {
                    SshAction::Retry => action("target.retry", "Retry", true),
                    SshAction::SetUp => action("ssh.setupExisting", "Set up…", true),
                    SshAction::Revoke => action("ssh.revoke", "Revoke", true),
                    SshAction::RevokeUnavailable(why) => with(
                        action("ssh.revoke", "Revoke", false),
                        json!({ "detail": why }),
                    ),
                    SshAction::Remove => action("target.remove", "Remove", true),
                },
                context.clone(),
            )
        }));
        ssh.push(
            json!({ "id": format!("ssh:{}", row.id), "label": row.name, "status": row.line,
            "detail": row.address, "tone": tone(row.mark), "actions": actions }),
        );
        if app.ui.set.remove_confirm.as_ref() == Some(&row.id) {
            ssh.push(json!({ "id": format!("confirm:{}", row.id),
                "label": format!("YAAC installed a key on {}. Revoke it before removing?", row.name), "layout": "form",
                "actions": [
                    args(action("ssh.revoke", "Revoke first", true), context.clone()),
                    args(action("target.removeOnly", "Remove only", true), context),
                    cancel("target.removeCancel", "Cancel")
                ]
            }));
        }
    }
    ssh.push(json!({ "id": "legend", "detail": model::LEGEND }));
    sections.push(section("ssh", "SSH", ssh));
    if app.host_windows {
        sections.push(wsl(app, v.wsl));
    }
    json!({
        "sections": sections, "actions": header,
        "closeAllowed": app.ui.set.ssh.is_none() && app.ui.set.revoke.is_none()
    })
}

fn wsl(app: &YaacApp, targets: Vec<model::WslRow>) -> Value {
    let state = &app.client.state;
    let on = state.wsl_on;
    let mut rows = vec![
        json!({ "id": "switch", "label": "Run agents inside Linux distros on this PC",
        "actions": [toggle("wsl.enabled", if on { "On" } else { "Off" }, on, true)] }),
    ];
    if !on {
        rows.push(json!({ "id": "off", "detail": "Off. Turn it on to run agents inside a Linux distro on this PC." }));
        return section("wsl", "WSL", rows);
    }
    rows.extend(targets.into_iter().map(|row| {
        let context = json!({ "target": row.id });
        json!({ "id": format!("wsl:{}", row.id), "label": format!("{} · {}", row.distro, row.user),
            "status": if row.enabled { row.note.as_deref().unwrap_or("") } else { "Off" }, "tone": tone(row.mark),
            "actions": [args(toggle("target.enabled", "Enabled", row.enabled, true), context.clone()), args(action("target.remove", "Remove", true), context)]
        })
    }));
    let (distro, note, ready) = match model::distro_picker(state) {
        DistroPicker::Loading => (
            json!({ "id": "distro", "label": "Distro", "type": "choice", "enabled": false, "placeholder": "Loading…" }),
            String::new(),
            false,
        ),
        DistroPicker::Choose { options, selected } => {
            let choices = options.iter().map(|option| with(action(&option.name, &option.label, true), json!({
                "detail": if option.added { format!("{} · already added", option.state) } else { option.state.to_owned() }
            }))).collect();
            (
                with(
                    choice(
                        "distro",
                        "Distro",
                        selected.as_deref(),
                        "wsl.choose",
                        choices,
                    ),
                    json!({ "required": true, "placeholder": model::CHOOSE_DISTRO }),
                ),
                String::new(),
                true,
            )
        }
        DistroPicker::Typed { message } => (
            with(
                field("distro", "Distro", "", "Type the distro's name"),
                json!({ "required": true }),
            ),
            format!(
                "The installed distros could not be listed ({message}). Type the distro's name."
            ),
            true,
        ),
    };
    rows.push(json!({ "id": "add", "layout": "inline", "detail": note,
        "fields": [distro, field("user", "User (optional)", "", "Distro default")],
        "actions": [action("wsl.refresh", "Refresh", true), submit(action("wsl.add", "Add", ready), true)]
    }));
    rows.push(json!({ "id": "note", "detail": "Adding a distro again with another user opens a second session to it." }));
    section("wsl", "WSL", rows)
}

fn ssh_panel(panel: &SshPanel) -> Value {
    let phase = &panel.setup.phase;
    let editable = matches!(phase, Phase::Form | Phase::Unreached(_));
    let input = |id, label, value, placeholder| {
        with(
            field(id, label, value, placeholder),
            json!({ "enabled": editable }),
        )
    };
    let mut rows = vec![json!({
        "id": "connection", "layout": "form", "resetKey": panel.existing,
        "fields": [
            input("name", "Name", &panel.name, "devbox"),
            input("user", "User", &panel.user, "user"),
            with(input("host", "Host", &panel.host, "host.example.com"), json!({ "required": true })),
            with(input("port", "Port", &panel.port, "22"), json!({ "required": true, "validation": "port" }))
        ],
        "actions": if editable {
            vec![submit(primary(action("ssh.setup", "Setup", true)), false), cancel("ssh.close", "Cancel")]
        } else { vec![] }
    })];
    rows.extend(panel.setup.steps.iter().cloned().chain(panel.setup.current()).enumerate().map(|(index, step)| {
        let tone = match step.mark {
            StepMark::Done => "success",
            StepMark::Current => "accent",
            StepMark::Failed => "error",
            StepMark::Hollow => "normal",
        };
        json!({ "id": format!("step-{index}"), "status": step.text, "detail": step.detail, "tone": tone })
    }));
    match phase {
        Phase::Form | Phase::Working => {}
        Phase::Unreached(message) => {
            rows.push(json!({ "id": "error", "status": message, "tone": "error" }));
        }
        Phase::ConfirmHost(key) => {
            rows.push(json!({ "id": "fingerprint", "label": format!("This server's key fingerprint · {}", key.algorithm),
                "layout": "code", "detail": key.fingerprint }));
            rows.push(json!({ "id": "hostConfirm", "detail": "Check it against what your server administrator gave you.", "layout": "form",
                "actions": [primary(action("ssh.confirmHost", "This is the right server", true)), cancel("ssh.close", "Cancel")] }));
        }
        Phase::SignIn { error } => {
            let method = match panel.method {
                SignInMethod::Password => "password",
                SignInMethod::KeyFile => "key",
            };
            let mut fields = vec![with(
                choice(
                    "method",
                    "Sign-in method",
                    Some(method),
                    "ssh.method",
                    vec![
                        action("password", "Password", true),
                        action("key", "Key file", true),
                    ],
                ),
                json!({ "type": "radio" }),
            )];
            if panel.method == SignInMethod::Password {
                fields.push(
                    json!({ "id": "password", "label": "Password", "type": "password", "show": true,
                    "detail": "Used once. YAAC does not save it." }),
                );
            } else {
                fields.extend([
                    with(field("keyPath", "Key file", &panel.key_path, "Choose a key file"), json!({ "type": "file", "required": true })),
                    json!({ "id": "passphrase", "label": "Passphrase (optional)", "type": "password",
                        "detail": "YAAC uses the file and passphrase once to install its own key." })
                ]);
            }
            if let Some(error) = error {
                rows.push(json!({ "id": "authError", "status": error, "tone": "error", "detail": "Nothing was changed on the server." }));
            }
            rows.push(json!({ "id": "signIn", "label": "Sign in", "detail": panel.setup.next_preview(), "layout": "form",
                "fields": fields,
                "actions": [submit(primary(action("ssh.signIn", "Connect", true)), false), cancel("ssh.close", "Cancel")]
            }));
        }
        Phase::Ready(record) => {
            let name = &panel.setup.conn.name;
            let mut actions = vec![primary(action("ssh.close", "Done", true))];
            let detail = if let Some(key) = record.installed() {
                actions.push(action("ssh.revokeReady", "Revoke", true));
                format!(
                    "YAAC can now connect to {name} without a password. YAAC added one line marked `{}` to `~/.ssh/authorized_keys`. Revoke removes that line.",
                    key.comment
                )
            } else {
                format!("Password-less access to {name} was already in place.")
            };
            rows.push(json!({ "id": "ready", "status": "Ready.", "tone": "success", "detail": detail, "layout": "form", "actions": actions }));
        }
        Phase::Failed { cleaned_up } => {
            if !cleaned_up && let Some(key) = panel.setup.leftover() {
                rows.push(json!({ "id": "leftover", "label": "YAAC's key may still be on the server. To remove it by hand, delete this line:",
                    "layout": "code", "detail": key.line,
                    "actions": [with(action("ssh.copy", "Copy", true), json!({ "kind": "copy", "text": key.line }))] }));
            }
            rows.push(json!({ "id": "failed", "layout": "form",
                "actions": [with(action("ssh.tryAgain", "Try again", true), json!({ "kind": "primary" })), cancel("ssh.close", "Close")] }));
        }
    }
    let title = if matches!(phase, Phase::Failed { .. }) {
        "Could not finish"
    } else {
        "Add SSH server"
    };
    with(section("sshSetup", title, rows), json!({ "kind": "panel" }))
}

fn revoke_panel(app: &YaacApp, panel: &RevokePanel) -> Value {
    let name = &panel.revoke.conn.name;
    let (title, rows) = match &panel.revoke.phase {
        RevokePhase::Working => (
            "Revoke",
            vec![
                json!({ "id": "working", "status": format!("Removing YAAC's key from {name}…"), "tone": "accent" }),
            ],
        ),
        RevokePhase::Revoked => {
            let detail = match app.client.state.ssh_records.get(&panel.target) {
                Some(SetupRecord::RemovedByHand { date }) => {
                    format!("{name} · Not set up · removed by hand on {date}")
                }
                Some(SetupRecord::Revoked { date }) => {
                    format!("{name} · Not set up · YAAC's key was removed on {date}")
                }
                _ => format!("{name} · Not set up"),
            };
            (
                "Revoked",
                vec![json!({ "id": "revoked", "status": detail,
                    "detail": "Revoke removes the key YAAC installed.", "actions": [cancel("ssh.revokeClose", "Close")]
                })],
            )
        }
        RevokePhase::CouldNotRevoke { reason } => (
            "Could not revoke",
            vec![
                json!({ "id": "reason", "status": reason, "tone": "error",
                "detail": format!("YAAC's key remains on {name}. YAAC has kept its copy so it can finish this later.") }),
                json!({ "id": "key", "label": format!("To remove it by hand, delete this line from ~/.ssh/authorized_keys on {name}:"), "layout": "code",
                "detail": panel.revoke.key.line,
                "actions": [with(action("ssh.copy", "Copy", true), json!({ "kind": "copy", "text": panel.revoke.key.line }))] }),
                json!({ "id": "actions", "layout": "form",
                "actions": [with(action("ssh.revokeRetry", "Try again", true), json!({ "kind": "primary" })),
                    action("ssh.revokeRemoved", "I removed it myself", true), cancel("ssh.revokeClose", "Close")] }),
            ],
        ),
    };
    with(
        section("sshRevoke", title, rows),
        json!({ "kind": "panel" }),
    )
}

fn target_options(app: &YaacApp) -> Vec<Value> {
    app.client
        .state
        .targets
        .iter()
        .map(|t| action(t.entry.id.as_str(), t.entry.name.as_str(), true))
        .collect()
}

fn providers(app: &YaacApp, target: Option<&TargetId>) -> Value {
    let state = &app.client.state;
    let target = settings_target(app, target);
    let refresh = target.and_then(|id| state.target(id)).map(|target| {
        with(
            args(
                action("providers.refresh", "Refresh", target.connected()),
                json!({ "target": target.entry.id }),
            ),
            json!({ "detail": "Re-check all providers and models on this target" }),
        )
    });
    let mut sections = vec![section(
        "target",
        "",
        vec![json!({
            "id": "selection", "layout": "inline",
            "fields": [choice("target", "Target", target.map(TargetId::as_str), "providers.target", target_options(app))],
            "actions": refresh.into_iter().collect::<Vec<_>>()
        })],
    )];
    if let Some(record) = target.and_then(|id| state.target(id)) {
        let pending = app.client.session_discovery_pending(&record.entry.id);
        if let Some(on) = record.session_discovery {
            let toggle = args(
                toggle("providers.sessionDiscovery", "Automatic session discovery", on, record.connected() && !pending),
                json!({ "target": record.entry.id }),
            );
            sections.push(section("session-discovery", "", vec![json!({
                "id": "automatic", "layout": "form",
                "detail": "Scan this target for sessions started in its harnesses.",
                "actions": [toggle],
            })]));
        } else if record.connected() {
            sections.push(section("session-discovery", "", vec![json!({
                "id": "unavailable",
                "detail": if pending { "Reading session discovery…" } else { "Automatic session discovery settings require an updated target daemon." },
            })]));
        }
    }
    let Some(v) = target.and_then(|t| model::providers_view(state, t)) else {
        return json!({ "sections": sections, "detail": "No target is connected." });
    };
    let target = &v.target;
    let mut rows = Vec::new();
    for row in &v.rows {
        let context = json!({ "target": target, "instance": row.instance });
        let mut actions = vec![
            args(
                toggle("provider.enabled", "Enabled", row.enabled, !v.offline),
                context.clone(),
            ),
            args(
                action("providers.refresh", "Refresh", !v.offline),
                context.clone(),
            ),
        ];
        if row.editable {
            actions.extend([
                args(action("provider.edit", "Edit", !v.offline), context.clone()),
                args(
                    action("provider.remove", "Remove", !v.offline),
                    context.clone(),
                ),
            ]);
        }
        let mut facts = row.facts.clone();
        if let Some(socket) = &row.socket {
            facts.push(format!("`{socket}`"));
        }
        rows.push(json!({ "id": format!("{}:{}", target, row.instance), "label": row.name, "status": row.status,
            "detail": facts.join(" · "), "tone": tone(row.mark), "actions": actions }));
        if let Some((edit_target, instance, name, socket)) = &app.ui.set.edit
            && edit_target == target
            && instance == &row.instance
        {
            rows.push(json!({ "id": format!("edit:{}:{}", target, row.instance), "layout": "inline",
                "fields": [field("name", "Name (optional)", name, "Name"), field("socket", "Socket (optional)", socket, "/path/to/control.sock")],
                "actions": [
                    submit(args(action("provider.save", "Save", !v.offline), context.clone()), false),
                    cancel("provider.cancel", "Cancel")
                ]
            }));
        }
        // The harness home (P300): the folder in use, Change, the inline field, and the
        // confirmation when the new folder is another store.
        if let Some(home) = &row.home {
            let editing = app.ui.set.home_edit.as_ref().filter(|(t, i, _)| t == target && i == &row.instance);
            if let Some((_, _, text)) = editing {
                let mut acts = vec![submit(args(action("provider.homeSave", "Save", !v.offline), context.clone()), false)];
                if home.set.is_some() {
                    acts.push(args(action("provider.homeDefault", "Use default", !v.offline), context.clone()));
                }
                acts.push(cancel("provider.homeCancel", "Cancel"));
                rows.push(json!({ "id": format!("home-edit:{}:{}", target, row.instance), "layout": "inline",
                    "detail": "Empty: the harness default. Threads stay with the store they belong to.",
                    "fields": [field("home", "Harness home", text, &home.placeholder())],
                    "actions": acts }));
            } else {
                rows.push(json!({ "id": format!("home:{}:{}", target, row.instance), "label": "Harness home",
                    "detail": home.line(),
                    "actions": [args(action("provider.homeEdit", "Change", !v.offline), context.clone())] }));
            }
            if let Some(c) = state.home_confirm.as_ref().filter(|c| &c.target == target && c.instance == row.instance) {
                rows.push(json!({ "id": format!("home-confirm:{}:{}", target, row.instance), "label": "Another store",
                    "status": c.text(), "tone": "warning",
                    "actions": [primary(action("provider.homeConfirm", "Change home", !v.offline)), action("provider.homeKeep", "Keep current", true)] }));
            }
        }
        let mut prefs = vec![args(
            toggle(
                "provider.outside",
                "Offer YAAC tools to sessions started outside YAAC",
                row.outside_tools,
                !v.offline,
            ),
            context.clone(),
        )];
        if let Some(on) = row.trust_folders {
            prefs.push(args(
                toggle(
                    "provider.trust",
                    "Trust folders YAAC works in",
                    on,
                    !v.offline,
                ),
                context.clone(),
            ));
        }
        rows.push(json!({ "id": format!("prefs:{}:{}", target, row.instance), "layout": "form", "actions": prefs }));
        if !row.model_guidance.is_empty() {
            let context = json!({ "target": target, "instance": row.instance });
            let expanded = app.ui.set.model_guidance_panel.as_ref() == Some(&(target.clone(), row.instance.clone()));
            rows.push(json!({ "id": format!("model-guidance:{target}:{}", row.instance), "label": "Model guidance",
                "actions": [args(action("provider.bestUsePanel", if expanded { "Hide" } else { "Show" }, true), context)] }));
            if expanded {
                for model in &row.model_guidance {
                    let context = json!({ "target": target, "instance": row.instance, "model": model.id,
                        "expectedModel": model.specific });
                    let id = format!("best-use:{target}:{}:{}", row.instance, model.id);
                    let dimmed = model.enabled == Some(false);
                    let mut acts = Vec::new();
                    if let Some(on) = model.enabled {
                        acts.push(args(toggle("provider.modelEnabled", "Model enabled", on, !v.offline), context.clone()));
                    }
                    if let Some(edit) = app.ui.set.model_guidance_edit.as_ref().filter(|edit| edit.matches(target, &row.instance, model)) {
                        acts.push(submit(args(action("provider.bestUseSave", "Save", !v.offline), context), true));
                        acts.push(cancel("provider.bestUseCancel", "Cancel"));
                        rows.push(json!({ "id": format!("{id}:edit"), "label": model.name, "layout": "form", "dimmed": dimmed,
                            "fields": [with(field("bestUse", "Best used as", &edit.text, "Best used as"), json!({ "required": true }))],
                            "actions": acts }));
                    } else {
                        if model.guidance_editable {
                            acts.push(args(action("provider.bestUseEdit", "Edit", !v.offline), context.clone()));
                            if model.custom() {
                                acts.push(args(action("provider.bestUseReset", "Reset to generated", !v.offline), context));
                            }
                        }
                        rows.push(json!({ "id": id, "label": model.name, "status": model.provenance(), "dimmed": dimmed,
                            "detail": model.guidance_editable.then(|| format!("Best used as: {}", model.summary())), "actions": acts }));
                    }
                }
            }
        }
    }
    sections.push(section("instances", "Instances", rows));

    let active_kind = app
        .ui
        .set
        .add_kind
        .as_ref()
        .filter(|kind| v.kinds.contains(kind))
        .or_else(|| v.kinds.first());
    let choices = v
        .kinds
        .iter()
        .map(|kind| action(kind.as_str(), &model::kind_label(state, kind), true))
        .collect();
    let mut add_fields = vec![
        with(choice("kind", "Kind", active_kind.map(|kind| kind.as_str()), "provider.kind", choices), json!({ "required": true })),
        with(field("name", "Name (optional)", "", "Name"), json!({ "validation": "any" })),
        with(field("socket", "Socket (optional)", "", "/path/to/control.sock"), json!({ "validation": "any" })),
    ];
    // The harness home (P300), for a kind whose instances report `configurableHome`.
    if active_kind.is_some_and(|k| v.home_kinds.contains(k)) {
        add_fields.push(with(field("home", "Harness home (optional)", "", "Harness default"), json!({ "validation": "any" })));
    }
    sections.push(section("add", "Add instance", vec![json!({
        "id": format!("add:{}", target), "layout": "inline", "detail": "A second instance of a kind needs a name, its own socket, or its own harness home.",
        "fields": add_fields,
        "actions": [submit(args(action("provider.add", "Add", !v.offline && active_kind.is_some()), json!({ "target": target })), true)]
    })]));
    for other in state.targets.iter().filter(|t| &t.entry.id != target) {
        if let Some(view) = model::providers_view(state, &other.entry.id) {
            let needs: Vec<_> = view.rows.iter().filter(|r| r.mark != Mark::Done).map(|row| json!({
                "id": row.instance, "label": row.name, "status": row.status, "tone": tone(row.mark)
            })).collect();
            if !needs.is_empty() {
                sections.push(section(
                    other.entry.id.as_str(),
                    &format!("{} · needs attention", view.target_name),
                    needs,
                ));
            }
        }
    }
    json!({
        "detail": if v.offline { format!("{} is not connected · showing what was last read.", v.target_name) } else { String::new() },
        "sections": sections
    })
}


fn appearance(app: &YaacApp) -> Value {
    let theme = app.ui.theme.id();
    let default_size = crate::theme::size::UI;
    let mut choices = vec![action("default", "YAAC default", true)];
    choices.extend(app.fonts.choices.iter().map(|choice| json!({
        "id": choice.id, "label": choice.label, "enabled": true, "fontPreview": choice.native
    })));
    let value = |role: crate::fonts::Role| {
        let current = role.get(&app.ui.fonts);
        if app.fonts.choices.iter().any(|choice| choice.id == current) { current } else { "default" }
    };
    json!({ "sections": [
        { "id": "theme", "title": "Theme", "rows": [
            { "id": "theme", "fields": [{ "id": "theme", "type": "radio", "value": theme, "action": "appearance.theme",
                "options": [action("system", "Follow the system", true), action("dark", "Dark", true), action("light", "Light", true), action("custom", "Custom", true)] }] }
        ] },
        { "id": "font", "title": "Font size", "detail": "Scales the whole window; agent prose stays the largest text.", "rows": [
            { "id": "font", "label": format!("{:.1} pt", app.ui.font_size), "actions": [
                action("appearance.smaller", "Smaller", app.ui.font_size > 11.0),
                action("appearance.larger", "Larger", app.ui.font_size < 18.0),
                action("appearance.reset", "Reset", (app.ui.font_size - default_size).abs() > 0.01)
            ] }
        ] },
        { "id": "font-families", "title": "Fonts", "detail": (!app.fonts.catalog_ready).then_some("Loading installed fonts…"), "rows": [
            { "id": "normal-font", "layout": "inline", "fontSample": format!("{}\n{}", crate::fonts::SAMPLE, crate::fonts::NERD_SAMPLE),
                "fields": [{"id": "font", "label": "Normal font", "type": "choice", "sample": format!("{}\n{}", crate::fonts::SAMPLE, crate::fonts::NERD_SAMPLE), "value": value(crate::fonts::Role::Normal), "action": "appearance.normalFont", "options": choices}],
                "actions": [action("appearance.importNormal", "Import font…", true)]
            },
            { "id": "code-font", "layout": "inline", "fontSample": format!("{}\n{}", crate::fonts::CODE_SAMPLE, crate::fonts::NERD_SAMPLE), "codeFontSample": true,
                "fields": [{"id": "font", "label": "Code font", "type": "choice", "sample": format!("{}\n{}", crate::fonts::CODE_SAMPLE, crate::fonts::NERD_SAMPLE), "codeFont": true, "value": value(crate::fonts::Role::Code), "action": "appearance.codeFont", "options": choices}],
                "actions": [action("appearance.importCode", "Import font…", true)]
            },
            { "id": "font-error", "label": app.fonts.error }
        ] },
        {"id":"custom-themes","title":"Custom themes","detail":app.theme_import.error,
         "actions":[action("appearance.importTheme","Import JSON…",true)],
         "cards":app.ui.themes.entries.values().map(|t| {
            let mut card = app.theme_import.wpf_card(t, app.ui.theme == ThemeChoice::Custom && app.ui.themes.selected == t.name);
            // The phone cannot render the desktop's offscreen demo; it draws the preview itself
            // from the theme's colours, with its own components (the same map as `customPalette`).
            if app.ui.platform == yaac_core::input::Platform::Android {
                card["palette"] = json!(crate::theme::Palette::custom(t).wpf_colors());
                card["status"] = json!("");
            }
            card
        }).collect::<Vec<_>>()}
    ] })
}

fn host(app: &YaacApp) -> Host {
    if app.host_windows {
        Host::Windows
    } else {
        Host::current()
    }
}

fn window(app: &YaacApp) -> Value {
    if app.ui.platform == yaac_core::input::Platform::Android {
        return android_window(app);
    }
    let host = host(app);
    let current = app.display.file.renderer;
    let options = offered(host, crate::display::built())
        .into_iter()
        .map(|r| action(r.id(), r.label(host), true))
        .collect();
    let mut rows = vec![json!({
        "id": "renderer", "detail": "A change applies when YAAC starts again.",
        "fields": [choice("renderer", "Renderer", Some(current.id()), "window.renderer", options)]
    })];
    rows.push(json!({ "id": "current", "detail": in_use_line(&app.display.in_use, host) }));
    if app.display.restart_needed() {
        rows.push(json!({ "id": "restart", "status": "Applies when YAAC starts again." }));
    }
    if let Some(crashed) = app.display.file.crashed {
        rows.push(json!({ "id": "crashed", "detail": format!("{} is skipped until you try it again.", crashed.label(host)),
            "actions": [action("window.retryRenderer", "Try it again", true)] }));
    }
    let tray_enabled = app.shell.has_tray();
    json!({
        "sections": [
            section("close", "Closing the window", vec![json!({
                "id": "tray",
                "detail": if tray_enabled {
                    "Closing the window hides it in the system tray. Click the tray icon to open it again. Its menu has Quit. The hub keeps running either way."
                } else {
                    "Closing the window quits YAAC. The hub keeps running."
                },
                "actions": [toggle("window.tray", "Keep running in the tray when closed", tray_enabled && app.ui.tray_on_close, tray_enabled)]
            })]),
            section("display", "Display", rows),
            section("voice", "Voice", vec![voice_language_row(app)]),
        ]
    })
}

/// The option id for "Same as this device" (an empty id would read as no value).
const SAME_AS_DEVICE_ID: &str = "device";

/// Settings → Window → Voice call language (docs/plan/voice-call.md § Language). The phone keeps
/// its own choice (ui/Settings.kt), so only the desktop shows this row.
fn voice_language_row(app: &YaacApp) -> Value {
    use yaac_core::voice_language::{CHOICES, SAME_AS_DEVICE, label};
    let device = crate::voice_language::device();
    let same = match device.as_deref() {
        Some(tag) => format!("{SAME_AS_DEVICE} ({})", label(tag)),
        None => SAME_AS_DEVICE.to_owned(),
    };
    let mut options = vec![action(SAME_AS_DEVICE_ID, &same, true)];
    options.extend(CHOICES.iter().map(|(tag, name)| action(tag, name, true)));
    let chosen = &app.ui.voice_language;
    if !chosen.is_empty() && !CHOICES.iter().any(|(tag, _)| tag == chosen) {
        options.push(action(chosen, chosen, true));
    }
    let value = if chosen.is_empty() { SAME_AS_DEVICE_ID } else { chosen.as_str() };
    json!({
        "id": "voiceLanguage",
        "detail": "The language voice calls speak and dictation listens for. Code identifiers stay as they are.",
        "fields": [choice("voiceLanguage", "Voice call language", Some(value), "window.voiceLanguage", options)]
    })
}

/// Settings → Android: whether the hub accepts the Android client over Tailscale (P377). The
/// setting is the hub's; in the Android app the page says so, naming the hub's computer.
fn android(app: &YaacApp) -> Value {
    let hub = &app.client.state.hub;
    let detail = if app.ui.platform == yaac_core::input::Platform::Android {
        let pc = app.hub_machine().unwrap_or_else(|| "the PC".into());
        format!("This setting belongs to the hub on {pc}. With it on, the hub accepts this phone over Tailscale, only from devices signed in to the same Tailscale account. Nothing is paired or stored. Turning it off ends this phone's connection.")
    } else {
        "The phone attaches to this computer's hub over Tailscale, only from devices signed in to the same Tailscale account. Nothing is paired or stored.".into()
    };
    json!({
        "sections": [section("tailscale", "Tailscale", vec![
            json!({ "id": "tailscale", "layout": "form", "detail": detail,
                "actions": [toggle("android.tailscale", "Allow the Android client over Tailscale", hub.tailscale_attach.unwrap_or(false), hub.tailscale_attach.is_some())]
            }),
            json!({ "id": "tailscaleStatus", "status": hub.tailscale_status })
        ])]
    })
}

/// Settings → Window in the Android app: the tray setting is **Keep running in the
/// background** (the same preference; its ongoing notification is the tray icon), and the
/// renderer is Android's own, so it is one line saying so, with nothing to choose.
fn android_window(app: &YaacApp) -> Value {
    json!({
        "sections": [
            section("close", "Leaving the app", vec![json!({
                "id": "tray",
                "detail": "YAAC stays attached in the background, with an ongoing notification. The notification has Open and Quit; Quit ends this app, never the hub.",
                "actions": [toggle("window.tray", "Keep running in the background", app.ui.tray_on_close, true)]
            })]),
            section("display", "Display", vec![
                json!({ "id": "current", "detail": "Drawn by Android (Jetpack Compose)." })
            ]),
        ]
    })
}

fn terminal_settings(app: &YaacApp) -> Value {
    let prefs = &app.ui.terminal;
    let controls = [
        ("copyOnSelect", "Copy on select", prefs.copy_on_select),
        ("ctrlVToShell", "Ctrl+V goes to the shell", prefs.ctrl_v_to_shell),
        ("confirmPaste", "Ask before pasting several lines", prefs.confirm_multiline_paste),
    ];
    let rows = controls.into_iter().map(|(id, label, enabled)| json!({
        "id": id, "actions": [toggle(&format!("terminal.{id}"),label,enabled,true)]
    })).collect();
    json!({ "sections": [section("clipboard","Clipboard and selection",rows)] })
}

fn keyboard(app: &YaacApp) -> Value {
    let bindings = &app.ui.bindings;
    let conflict = &app.ui.set.keys.conflict;
    let sections: Vec<_> = keys::Group::ALL.into_iter().map(|group| {
        let mut rows = Vec::new();
        for d in keys::COMMANDS.iter().filter(|d| d.group == group) {
            let chords = bindings.chords(d.cmd);
            let mut actions = Vec::new();
            let global = d.scope == Scope::Global;
            let mut shortcut = json!({ "id": "chord", "type": "shortcut", "value": format_chords(&chords) });
            if global {
                shortcut["action"] = json!(format!("keys.record/{}", d.id));
                let default = format_chords(&keys::default_chords(d));
                let reset_description = if default.is_empty() { "Back to no key".to_owned() } else { format!("Back to {default}") };
                let params = json!({ "command": d.id });
                actions.extend([
                    args(action("keys.clear", "Clear", !chords.is_empty()), params.clone()),
                    args(with(action("keys.reset", "Reset", bindings.is_changed(d.cmd)), json!({ "detail": reset_description })), params)
                ]);
            } else if let Scope::Local(where_) = d.scope {
                shortcut["detail"] = json!(where_);
                shortcut["placeholder"] = json!("—");
            }
            if global || matches!(d.cmd, keys::Cmd::NextTerminal | keys::Cmd::PrevTerminal) {
                actions.push(args(toggle("keys.inTerminal", "Works in the terminal", bindings.works_in_terminal(d.cmd), true),
                    json!({ "command": d.id })));
            }
            rows.push(json!({
                "id": d.id, "label": d.name, "detail": d.explain, "fields": [shortcut], "actions": actions
            }));
            if let Some((command, chord, other)) = conflict
                && *command == d.cmd
            {
                let other = keys::def_of(*other);
                let (text, actions) = match other.scope {
                    Scope::Global => (format!("{chord} is used by {}.", other.name),
                        vec![with(action("keys.replace", "Replace", true), json!({ "kind": "primary" })), cancel("keys.cancel", "Cancel")]),
                    Scope::Local(desc) => (format!("{chord} is used by {} {desc}. Choose another key.", other.name), vec![cancel("keys.cancel", "Cancel")])
                };
                rows.push(json!({ "id": format!("conflict:{}", d.id), "status": text, "tone": "warning", "actions": actions }));
            }
            if let Some(note) = chords.iter().find_map(keys::os_note) {
                rows.push(json!({ "id": format!("note:{}", d.id), "detail": note }));
            }
        }
        section(group.label(), group.label(), rows)
    }).collect();
    json!({
        "detail": app.ui.input.pick(
            "Every command is also in the Commander. Click a key to record a new one. Esc cancels and Backspace clears. Keys without Ctrl or Alt act only when no text field has focus.",
            "Every command is also in the Commander. With a hardware keyboard, tap a key to record a new one. Esc cancels and Backspace clears. Keys without Ctrl or Alt act only when no text field has focus.",
        ),
        "actions": [action("keys.resetAll", "Reset all", !bindings.overrides.0.is_empty() || !bindings.in_terminal.is_empty())],
        "sections": sections
    })
}

fn advanced(app: &YaacApp) -> Value {
    let state = &app.client.state;
    let target = app
        .ui
        .set
        .diag_target
        .as_ref()
        .filter(|id| state.target(id).is_some())
        .or_else(|| state.targets.first().map(|t| &t.entry.id));
    let diag = if let Some(target) = target {
        let mut rows = vec![json!({ "id": "target", "fields": [
            choice("target", "Target", Some(target.as_str()), "diag.target", target_options(app))
        ] })];
        rows.extend(
            model::diagnostics(state, target)
                .into_iter()
                .enumerate()
                .map(|(index, (field, value))| {
                    json!({
                        "id": format!("diagnostic-{index}"), "label": field, "detail": value
                    })
                }),
        );
        rows
    } else {
        vec![json!({ "id": "empty", "detail": "No targets." })]
    };
    json!({ "sections": [
        section("send", "Send key", vec![json!({ "id": "send", "fields": [{
            "id": "sendKey", "type": "radio", "action": "advanced.sendKey",
            "value": if app.ui.send_key == SendKey::Enter { "enter" } else { "ctrlEnter" },
            "options": [action("enter", "Enter sends · Shift+Enter is a new line", true),
                action("ctrlEnter", "Ctrl+Enter sends · Enter is a new line", true)]
        }] })]),
        section("hub", "Hub", vec![
            json!({ "id": "hub", "layout": "form",
                "detail": "Agents on different machines keep working together while this window is closed. Off: the hub exits a while after the last window closes and nothing is running.",
                "actions": [toggle("advanced.always", "Start the hub at login and keep it running", state.hub.always.unwrap_or(true), true)]
            }),
            json!({ "id": "error", "status": state.hub.last_error, "tone": "error" })
        ]),
        section("cache", "Cache", vec![json!({
            "id": "cache", "detail": "Thread lists and transcripts are read again from each target. Drafts are kept.",
            "actions": [action("advanced.clearCache", "Clear cache", true)]
        })]),
        section("diagnostics", "Diagnostics", diag)
    ] })
}

fn component_row(component: Component, id: &str, release: bool, detail: Option<&str>) -> Value {
    let (status, tone) = match component.status {
        ComponentStatus::Current => (
            (if release {
                "this YAAC is up to date"
            } else {
                "up to date"
            })
            .to_owned(),
            "normal",
        ),
        ComponentStatus::Outdated(why) if release => (why, "warning"),
        ComponentStatus::Outdated(why) => (format!("needs updating · {why}"), "warning"),
        ComponentStatus::Unknown(why) => (why, "normal"),
    };
    let build = component.build.unwrap_or_else(|| "—".to_owned());
    json!({ "id": id, "label": format!("{} · {}", component.name, build), "status": status, "tone": tone, "detail": detail })
}

fn about(app: &YaacApp) -> Value {
    use crate::updater::Phase;
    let (version, id) = crate::BUILD.split_once('+').unwrap_or((crate::BUILD, ""));
    let build = if !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()) {
        format!("Build {id}")
    } else {
        format!("Development build ({id})")
    };
    let diagnostic = crate::troubleshoot::choice(&app.client.state);
    let info = with(
        section("build", &format!("YAAC {version}"), Vec::new()),
        json!({
            "detail": format!("{build} · {}", crate::BUILD),
            "actions": [
                with(action("about.releases", "Releases", true), json!({ "kind": "link" })),
                with(action("about.source", "Source", true), json!({ "kind": "link" })),
                with(action("about.troubleshoot", "Troubleshoot", diagnostic.is_some() || app.troubleshoot.open),
                    json!({ "detail": if diagnostic.is_none() { crate::troubleshoot::UNAVAILABLE } else { "" } }))
            ]
        }),
    );
    let mut components: Vec<_> = components::components(&app.client.state, crate::BUILD)
        .into_iter()
        .enumerate()
        .map(|(i, c)| component_row(c, &format!("component-{i}"), false, None))
        .collect();
    let last_checked = if app.updater.refreshing {
        Some("Checking for a release…".into())
    } else { app.updater.last_checked.map(|when| {
        let local = chrono::DateTime::<chrono::Local>::from(when);
        format!("Last checked {}", local.format("%Y-%m-%d %H:%M"))
    }) };
    components.push(component_row(
        components::release(&app.updater.check, crate::BUILD),
        "release",
        true,
        last_checked.as_deref(),
    ));
    match &app.updater.phase {
        Phase::Idle => {}
        Phase::Available(found) => components.push(json!({
            "id": "update", "label": format!("YAAC {} is available", found.manifest.version),
            "actions": [with(action("update.download", "Download", true), json!({ "kind": "primary" }))]
        })),
        Phase::Downloading { found, done, total } => components.push(json!({
            "id": "update", "label": format!("Downloading YAAC {}…", found.manifest.version),
            "fields": [{ "id": "progress", "type": "progress", "value": (*total > 0).then(|| (100.0 * *done as f64 / *total as f64).min(100.0)) }]
        })),
        Phase::Ready(pending) => components.push(json!({
            "id": "update", "label": format!("YAAC {} is downloaded", pending.version),
            "actions": [with(action("update.install", "Restart to update", true), json!({ "kind": "primary" }))]
        })),
        Phase::Failed { message, .. } => components.push(json!({
            "id": "update", "status": message, "tone": "error",
            "actions": [action("update.download", "Try again", true)]
        }))
    }
    if let Some(notice) = &app.updater.notice {
        components.push(json!({ "id": "updateNotice", "status": notice, "tone": "warning" }));
    }
    let components = with(section("components", "Components", components), json!({
        "inlineActions": true,
        "actions": [with(action("about.refresh", "Refresh", true),
            json!({ "detail": "Check target connections and the latest release now" }))]
    }));
    let mut sections = vec![info, components];
    if let Some(pc) = host_update_section(app) {
        sections.push(pc);
    }
    if app.troubleshoot.open {
        let panel = &app.troubleshoot;
        let pending = panel.pending_label();
        let status = pending.map(|label| format!("Diagnosing with {label}…"))
            .or_else(|| panel.error.clone())
            .unwrap_or_else(|| diagnostic.as_ref().map_or(crate::troubleshoot::UNAVAILABLE, |c| c.label.as_str()).to_owned());
        let mut rows = vec![json!({
            "id": "symptom", "layout": "form",
            "status": status, "tone": if panel.error.is_some() { "error" } else { "" },
            "fields": [{
                "id": "symptom", "type": "textarea", "label": "Symptom", "value": panel.symptom,
                "placeholder": "What is happening?", "height": 90, "required": true,
                "enabled": pending.is_none(), "maxLength": crate::troubleshoot::MAX_SYMPTOM
            }],
            "actions": [with(action("about.diagnose", "Diagnose", diagnostic.is_some() && pending.is_none()),
                json!({ "kind": "primary", "submit": true }))]
        })];
        if !panel.report.is_empty() {
            rows.push(json!({
                "id": "diagnostic-report", "layout": "form", "resetKey": panel.report_version.to_string(),
                "fields": [{
                    "id": "report", "type": "textarea", "label": "Developer handoff",
                    "value": panel.report, "height": 290
                }],
                "actions": [with(action("about.copyReport", "Copy report", true), json!({ "copyField": "report" }))]
            }));
        }
        sections.push(with(section("troubleshoot", "Troubleshoot", rows),
            json!({ "detail": crate::troubleshoot::GUIDANCE })));
    }
    json!({ "sections": sections })
}

/// A phone newer than its PC's hub: **Update YAAC on <pc> to <build>**, and how that update
/// stands (`hub/update/request`, docs/install.md#updating-from-the-phone). Only for a hub on
/// another computer, and only while the phone's build is the newer release.
fn host_update_section(app: &YaacApp) -> Option<Value> {
    host_update_view(&app.remote_pc()?, crate::BUILD, &app.client.state.hub)
}

fn host_update_view(pc: &str, own: &str, hub: &yaac_core::state::HubState) -> Option<Value> {
    use yaac_core::state::HostUpdatePhase as P;
    let offered = yaac_core::update::offers_host_update(own, hub.build.as_deref());
    let update = hub.host_update.as_ref().filter(|u| u.requested_here);
    if !offered && update.is_none() {
        return None;
    }
    let number = |b: &str| yaac_core::update::build_number(b).map_or_else(|| b.to_owned(), |n| n.to_string());
    let label = format!("Update YAAC on {pc} to build {}", number(own));
    let button = || with(action("pc.update", &label, true), json!({
        "kind": "primary",
        "confirm": {
            "title": format!("Update YAAC on {pc}?"),
            "text": format!("{pc} downloads YAAC {own} from GitHub and installs it. YAAC closes there, \
                and running turns keep going on the targets. Its hub and window start again on the new build, \
                and this phone reconnects by itself."),
            "ok": "Update"
        }
    }));
    let mut row = json!({ "id": "pc", "label": format!("YAAC on {pc}"),
        "detail": format!("{pc} runs YAAC {}; this phone runs {own}.", hub.build.as_deref().unwrap_or("?")) });
    match update.map(|u| &u.phase) {
        None => row["actions"] = json!([button()]),
        Some(P::Requesting) => row["status"] = json!(format!("Asking {pc} to update…")),
        Some(P::Downloading { done, total }) => {
            row["status"] = json!(format!("{pc} is downloading YAAC {own}…"));
            row["fields"] = json!([{ "id": "progress", "type": "progress", "value": (*total > 0).then(|| (100.0 * *done as f64 / *total as f64).min(100.0)) }]);
        }
        Some(P::Installing) => row["status"] = json!(format!("{pc} is installing YAAC {own}. It closes YAAC and starts it again.")),
        Some(P::Restarting) => row["status"] = json!(format!("{pc} is updating… This phone reconnects when it is back.")),
        Some(P::Other(phase)) => row["status"] = json!(format!("{pc}: {phase}")),
        Some(P::Failed(why)) => {
            row["status"] = json!(why);
            row["tone"] = json!("error");
            if offered {
                row["actions"] = json!([button()]);
            }
        }
    }
    Some(section("pc", &format!("YAAC on {pc}"), vec![row]))
}

pub(super) fn command(
    app: &mut YaacApp,
    settings_selection: &mut Option<TargetId>,
    name: &str,
    p: &Value,
) -> Result<(), String> {
    if name != "settings.action" {
        return Err("Unknown settings command.".into());
    }
    let id = string(p, "id");
    let values = &p["values"];
    if id.starts_with("catalog.") {
        return crate::catalog_settings::wpf_command(app, id, values);
    }
    let action = match id {
        "first.start" if first_run::first_run(&app.client.state).can_start => Action::StartFirstThread,
        "first.wsl" if app.host_windows => {
            app.apply(vec![
                Action::WslOn(true),
                Action::SettingsPage(Page::Targets),
            ]);
            return Ok(());
        }
        "ssh.add" => {
            app.ui.set.ssh = Some(SshPanel::new(app.now(), None));
            app.ui.set.revoke = None;
            if !app.ui.settings || app.ui.settings_page != Page::Targets {
                app.apply(vec![Action::SettingsPage(Page::Targets)]);
            }
            return Ok(());
        }
        "target.enabled" => Action::TargetEnabled(target_id(app, values)?, checked(values)?),
        "target.retry" => Action::Retry(target_id(app, values)?),
        "target.remove" => {
            let target = target_id(app, values)?;
            if app
                .client
                .state
                .ssh_records
                .get(&target)
                .and_then(SetupRecord::installed)
                .is_some()
            {
                app.ui.set.remove_confirm = Some(target);
                return Ok(());
            }
            Action::RemoveTarget(target)
        }
        "target.removeOnly" => {
            let target = target_id(app, values)?;
            if app.ui.set.remove_confirm.as_ref() != Some(&target) {
                return Err("The removal confirmation has closed.".into());
            }
            app.ui.set.remove_confirm = None;
            Action::RemoveTarget(target)
        }
        "target.removeCancel" => {
            app.ui.set.remove_confirm = None;
            return Ok(());
        }
        "wsl.enabled" if app.host_windows => Action::WslOn(checked(values)?),
        "wsl.refresh" if app.host_windows && app.client.state.wsl_on => Action::LoadDistros,
        "wsl.choose" if app.host_windows && app.client.state.wsl_on => {
            let distro = string(values, "distro");
            if !app
                .client
                .state
                .wsl_distros
                .list
                .iter()
                .any(|d| d.name == distro)
            {
                return Err("Choose an installed distribution.".into());
            }
            app.client.choose_wsl_distro(Some(distro.to_owned()));
            return Ok(());
        }
        "wsl.add" if app.host_windows && app.client.state.wsl_on => {
            let distro = string(values, "distro").trim();
            if distro.is_empty()
                || match model::distro_picker(&app.client.state) {
                    DistroPicker::Loading => true,
                    DistroPicker::Choose { options, .. } => {
                        !options.iter().any(|option| option.name == distro)
                    }
                    DistroPicker::Typed { .. } => false,
                }
            {
                return Err("Choose a distribution to add.".into());
            }
            let user = string(values, "user").trim();
            let name = if user.is_empty() {
                format!("wsl: {distro}")
            } else {
                format!("wsl: {distro} ({user})")
            };
            let transport = WslTransport {
                distro: distro.to_owned(),
                user: (!user.is_empty()).then(|| user.to_owned()),
            };
            app.client.choose_wsl_distro(None);
            Action::AddTarget(name, TargetTransport::Wsl(transport))
        }
        "providers.target" => {
            *settings_selection = Some(target_id(app, values)?);
            return Ok(());
        }
        "providers.sessionDiscovery" => Action::SessionDiscovery(target_id(app, values)?, checked(values)?),
        "providers.refresh" => {
            let target = connected_target(app, values)?;
            let instance = string(values, "instance");
            let instance = if instance.is_empty() {
                None
            } else {
                Some(instance_id(app, &target, values)?)
            };
            Action::RefreshProviders(target, instance)
        }
        "provider.kind" => {
            let target = settings_target(app, settings_selection.as_ref())
                .ok_or("No target is selected.")?;
            app.ui.set.add_kind = Some(provider_kind(app, target, values)?);
            return Ok(());
        }
        "provider.add" => {
            let target = connected_target(app, values)?;
            let kind = provider_kind(app, &target, values)?;
            let name = string(values, "name").trim();
            let socket = string(values, "socket").trim();
            // The harness home (P300): only for a kind whose instances report it.
            let home_kind = model::providers_view(&app.client.state, &target).is_some_and(|v| v.home_kinds.contains(&kind));
            let home = if home_kind { string(values, "home").trim() } else { "" };
            if name.is_empty() && socket.is_empty() && home.is_empty() {
                return Err("Supply a name, socket or harness home for the instance.".into());
            }
            Action::AddInstance(target, kind, name.to_owned(), socket.to_owned(), home.to_owned())
        }
        "provider.cancel" => {
            app.ui.set.edit = None;
            return Ok(());
        }
        "provider.bestUsePanel" => {
            let target = target_id(app, values)?;
            let instance = instance_id(app, &target, values)?;
            let scope = (target, instance);
            app.ui.set.model_guidance_panel = (app.ui.set.model_guidance_panel.as_ref() != Some(&scope)).then_some(scope);
            app.ui.set.model_guidance_edit = None;
            return Ok(());
        }
        "provider.bestUseCancel" => {
            app.ui.set.model_guidance_edit = None;
            return Ok(());
        }
        "provider.modelEnabled" => {
            let target = connected_target(app, values)?;
            let instance = instance_id(app, &target, values)?;
            let provider = app.client.state.target(&target)
                .and_then(|t| t.providers.iter().find(|p| p.instance == instance))
                .filter(|p| p.capabilities.contains(&yaac_protocol::Capability::ModelAvailability))
                .ok_or_else(|| "This provider instance has no model switch.".to_owned())?;
            let model = yaac_protocol::ModelId::new(string(values, "model"));
            if provider.find_model(&model).is_none() {
                return Err("This provider instance no longer lists that model.".into());
            }
            let enabled = values["checked"].as_bool().ok_or_else(|| "Expected a model switch value.".to_owned())?;
            Action::ModelEnabled(target, instance, model, enabled)
        }
        "provider.explainsApprovals" => {
            let target = connected_target(app, values)?;
            let instance = instance_id(app, &target, values)?;
            let model = yaac_protocol::ModelId::new(string(values, "model"));
            let enabled = values["checked"].as_bool().ok_or_else(|| "Expected an explanation model switch value.".to_owned())?;
            Action::ApprovalExplanationModel(target, instance, enabled.then_some(model))
        }
        "provider.bestUseEdit" | "provider.bestUseSave" | "provider.bestUseReset" => {
            let target = connected_target(app, values)?;
            let instance = instance_id(app, &target, values)?;
            let model = model::providers_view(&app.client.state, &target)
                .and_then(|view| view.rows.into_iter().find(|row| row.instance == instance))
                .and_then(|row| row.model_guidance.into_iter().find(|m| {
                    m.id.as_str() == string(values, "model") && m.specific.as_str() == string(values, "expectedModel")
                }))
                .ok_or_else(|| "The model changed while you were editing. Reopen its current guidance.".to_owned())?;
            if id == "provider.bestUseEdit" {
                app.ui.set.model_guidance_edit = Some(crate::settings::ModelGuidanceEdit::new(&target, &instance, &model));
                return Ok(());
            }
            let summary = if id == "provider.bestUseSave" {
                let text = string(values, "bestUse").trim();
                if !crate::settings::valid_model_guidance(text) {
                    return Err("Enter a model recommendation or reset it to generated guidance.".into());
                }
                Some(text.to_owned())
            } else {
                None
            };
            app.ui.set.model_guidance_edit = None;
            Action::ModelBestUse(target, instance, model.id, model.specific, summary)
        }
        "provider.homeCancel" => {
            app.ui.set.home_edit = None;
            return Ok(());
        }
        "provider.homeConfirm" => Action::ConfirmInstanceHome,
        "provider.homeKeep" => Action::CancelInstanceHome,
        "provider.homeEdit" => {
            let target = connected_target(app, values)?;
            let instance = instance_id(app, &target, values)?;
            let home = model::providers_view(&app.client.state, &target)
                .and_then(|v| v.rows.into_iter().find(|row| row.instance == instance))
                .and_then(|row| row.home)
                .ok_or("This instance has no harness home to set.")?;
            app.ui.set.home_edit = Some((target, instance, home.set.unwrap_or_default()));
            return Ok(());
        }
        "provider.homeSave" | "provider.homeDefault" => {
            let target = connected_target(app, values)?;
            let instance = instance_id(app, &target, values)?;
            if !app.ui.set.home_edit.as_ref().is_some_and(|(t, i, _)| t == &target && i == &instance) {
                return Err("This harness home is not being edited.".into());
            }
            app.ui.set.home_edit = None;
            let home = if id == "provider.homeSave" { string(values, "home").trim().to_owned() } else { String::new() };
            Action::InstanceHome(target, instance, home)
        }
        "provider.edit" => {
            let target = connected_target(app, values)?;
            let instance = instance_id(app, &target, values)?;
            let config = app
                .client
                .state
                .target(&target)
                .and_then(|record| record.instance_configs.as_ref())
                .and_then(|configs| configs.iter().find(|c| c.id == instance && !c.implicit))
                .ok_or("This provider cannot be edited.")?;
            let name = config.name.clone().unwrap_or_default();
            let socket = config
                .endpoint
                .as_ref()
                .and_then(|e| e.socket.as_ref())
                .map(ToString::to_string)
                .unwrap_or_default();
            app.ui.set.edit = Some((target, instance, name, socket));
            return Ok(());
        }
        "provider.enabled" | "provider.outside" | "provider.trust" | "provider.save"
        | "provider.remove" => provider_command(app, id, values)?,
        "diag.target" => {
            app.ui.set.diag_target = Some(target_id(app, values)?);
            return Ok(());
        }
        "appearance.importTheme" => { app.import_theme(); return Ok(()); }
        "appearance.selectTheme" => { app.select_theme(string(values,"name"))?; return Ok(()); }
        "appearance.theme" => Action::Theme(match string(values, "theme") {
            "custom" => ThemeChoice::Custom,
            "dark" => ThemeChoice::Dark,
            "light" => ThemeChoice::Light,
            "system" => ThemeChoice::System,
            _ => return Err("Choose a theme.".into()),
        }),
        "appearance.normalFont" | "appearance.codeFont" => {
            let choice = string(values, "font");
            let role = if id == "appearance.normalFont" { crate::fonts::Role::Normal } else { crate::fonts::Role::Code };
            if choice == "default" || app.fonts.choices.iter().any(|font| font.id == choice) {
                role.set(&mut app.ui.fonts, if choice == "default" { String::new() } else { choice.to_owned() });
            }
            return Ok(());
        }
        "appearance.importNormal" | "appearance.importCode" => {
            let role = if id == "appearance.importNormal" { crate::fonts::Role::Normal } else { crate::fonts::Role::Code };
            app.import_font(role);
            return Ok(());
        }
        "appearance.smaller" | "appearance.larger" | "appearance.reset" => {
            app.ui.font_size = match id {
                "appearance.smaller" => (app.ui.font_size - 0.5).max(11.0),
                "appearance.larger" => (app.ui.font_size + 0.5).min(18.0),
                _ => crate::theme::size::UI,
            };
            return Ok(());
        }
        "terminal.copyOnSelect" | "terminal.ctrlVToShell" | "terminal.confirmPaste" => {
            let on = checked(values)?;
            match id {
                "terminal.copyOnSelect" => app.ui.terminal.copy_on_select = on,
                "terminal.ctrlVToShell" => app.ui.terminal.ctrl_v_to_shell = on,
                _ => app.ui.terminal.confirm_multiline_paste = on,
            }
            return Ok(());
        }
        "android.tailscale" => {
            app.client.set_tailscale_attach(checked(values)?);
            return Ok(());
        }
        "window.tray" if app.shell.has_tray() || app.ui.platform == yaac_core::input::Platform::Android => {
            app.ui.tray_on_close = checked(values)?;
            return Ok(());
        }
        "window.voiceLanguage" => {
            let picked = string(values, "voiceLanguage");
            app.ui.voice_language = if picked == SAME_AS_DEVICE_ID {
                String::new()
            } else {
                yaac_core::voice_language::normalize(picked).ok_or("This language is unavailable.")?
            };
            return Ok(());
        }
        "window.renderer" | "window.retryRenderer" => {
            let renderer = if id == "window.retryRenderer" {
                app.display.file.renderer
            } else {
                Renderer::from_id(string(values, "renderer"))
                    .filter(|r| r.offered(host(app), crate::display::built()))
                    .ok_or("This renderer is unavailable.")?
            };
            app.display.choose(renderer);
            return Ok(());
        }
        "approvals.main" | "approvals.fallback" => {
            let slot = id.trim_start_matches("approvals.");
            let value: Option<yaac_protocol::ExplanationModel> = serde_json::from_str(string(values, slot)).map_err(|_| "Unknown model.")?;
            if value.as_ref().is_some_and(|v| !model::explanation_choices(&app.client.state).iter().any(|(c, _)| c == v)) { return Err("Model unavailable.".into()); }
            let mut models = app.client.state.hub.explanations.clone();
            if slot == "main" { models.main = value; } else { models.fallback = value; }
            app.client.set_explanations(models);
            return Ok(());
        }
        "advanced.sendKey" => {
            app.ui.send_key = match string(values, "sendKey") {
                "enter" => SendKey::Enter,
                "ctrlEnter" => SendKey::CtrlEnter,
                _ => return Err("Choose a send key.".into()),
            };
            return Ok(());
        }
        "notifications.enabled" | "notifications.questions" | "notifications.sound" | "notifications.choice" => {
            let mut prefs = app.client.state.hub.notifications;
            match id {
                "notifications.enabled" => prefs.enabled = checked(values)?,
                "notifications.questions" => prefs.include_questions = checked(values)?,
                "notifications.sound" => prefs.sound = checked(values)?,
                _ => prefs.sound_choice = serde_json::from_value(json!(string(values, "soundChoice"))).map_err(|_| "Unknown sound.")?,
            }
            app.client.set_notifications(prefs);
            return Ok(());
        }
        "advanced.always" => Action::SetAlways(checked(values)?),
        "advanced.clearCache" => Action::ClearCache,
        "about.releases" => Action::OpenUrl(format!("{REPOSITORY}/releases")),
        "about.source" => Action::OpenUrl(REPOSITORY.to_owned()),
        "about.refresh" => Action::RefreshComponents,
        "about.troubleshoot" => {
            app.troubleshoot.open = !app.troubleshoot.open;
            return Ok(());
        }
        "about.diagnose" => {
            app.troubleshoot.symptom = string(values, "symptom").to_owned();
            app.troubleshoot.start(&mut app.client, "WPF", app.host_windows);
            return Ok(());
        }
        "update.download" => {
            app.updater.download();
            return Ok(());
        }
        "pc.update" if app.remote_pc().is_some() => {
            app.client.request_host_update();
            return Ok(());
        }
        "update.install" => {
            if let Err(error) = app.install_update() {
                app.updater.notice = Some(error);
            }
            return Ok(());
        }
        _ if id.starts_with("keys.") => return keyboard_command(app, id, values),
        _ if id.starts_with("ssh.") => return ssh_command(app, id, values),
        _ => return Err("This settings action is unavailable.".into()),
    };
    app.apply(vec![action]);
    Ok(())
}

fn checked(values: &Value) -> Result<bool, String> {
    values["checked"]
        .as_bool()
        .ok_or_else(|| "The switch value is missing.".into())
}

fn target_id(app: &YaacApp, values: &Value) -> Result<TargetId, String> {
    let target = TargetId::from(string(values, "target"));
    app.client
        .state
        .target(&target)
        .map(|_| target)
        .ok_or_else(|| "The target is unavailable.".into())
}

fn connected_target(app: &YaacApp, values: &Value) -> Result<TargetId, String> {
    let target = target_id(app, values)?;
    if app
        .client
        .state
        .target(&target)
        .is_some_and(|record| record.connected())
    {
        Ok(target)
    } else {
        Err("The target is offline.".into())
    }
}

fn instance_id(app: &YaacApp, target: &TargetId, values: &Value) -> Result<InstanceId, String> {
    let instance = InstanceId::from(string(values, "instance"));
    if app.client.state.target(target).is_some_and(|record| {
        record
            .providers
            .iter()
            .any(|provider| provider.instance == instance)
    }) {
        Ok(instance)
    } else {
        Err("The provider instance is unavailable.".into())
    }
}

fn provider_kind(
    app: &YaacApp,
    target: &TargetId,
    values: &Value,
) -> Result<yaac_protocol::ProviderKind, String> {
    model::providers_view(&app.client.state, target)
        .and_then(|view| {
            view.kinds
                .into_iter()
                .find(|kind| kind.as_str() == string(values, "kind"))
        })
        .ok_or_else(|| "Choose an available provider kind.".into())
}

fn provider_command(app: &mut YaacApp, id: &str, values: &Value) -> Result<Action, String> {
    let target = connected_target(app, values)?;
    let instance = instance_id(app, &target, values)?;
    let row = model::providers_view(&app.client.state, &target)
        .and_then(|v| v.rows.into_iter().find(|row| row.instance == instance))
        .ok_or("The provider instance is unavailable.")?;
    match id {
        "provider.enabled" => Ok(Action::InstanceEnabled(target, instance, checked(values)?)),
        "provider.outside" => Ok(Action::OutsideTools(target, instance, checked(values)?)),
        "provider.trust" if row.trust_folders.is_some() => {
            Ok(Action::TrustFolders(target, instance, checked(values)?))
        }
        "provider.remove" if row.editable => Ok(Action::RemoveInstance(target, instance)),
        "provider.save"
            if row.editable
                && app
                    .ui
                    .set
                    .edit
                    .as_ref()
                    .is_some_and(|(t, i, _, _)| t == &target && i == &instance) =>
        {
            app.ui.set.edit = None;
            Ok(Action::UpdateInstance(
                target,
                instance,
                string(values, "name").to_owned(),
                string(values, "socket").to_owned(),
            ))
        }
        _ => Err("This provider action is unavailable.".into()),
    }
}

fn keyboard_command(app: &mut YaacApp, id: &str, values: &Value) -> Result<(), String> {
    if let Some(id) = id.strip_prefix("keys.record/") {
        let def = keys::by_id(id)
            .filter(|d| d.scope == Scope::Global)
            .ok_or("This shortcut is fixed.")?;
        let key = string(values, "key");
        if key.is_empty() {
            app.ui.bindings.clear(def.cmd);
            app.ui.set.keys.conflict = None;
            return Ok(());
        }
        let pressed = |name| values[name].as_bool().unwrap_or(false);
        let chord = match key {
            "MouseBack" => Chord::mouse(MouseButton::Back),
            "MouseForward" => Chord::mouse(MouseButton::Forward),
            key => Chord::key(
                pressed("ctrl") || pressed("command"),
                pressed("shift"),
                pressed("alt"),
                &canonical_key(key).ok_or("This key is unavailable.")?,
            ),
        };
        crate::keyboard::record(app, def.cmd, chord);
        return Ok(());
    }
    match id {
        "keys.inTerminal" => {
            let def = keys::by_id(string(values, "command")).ok_or("Unknown command.")?;
            app.ui.bindings.set_in_terminal(def.cmd, checked(values)?);
        }
        "keys.clear" | "keys.reset" => {
            let def = keys::by_id(string(values, "command"))
                .filter(|d| d.scope == Scope::Global)
                .ok_or("This shortcut is fixed.")?;
            if id == "keys.clear" {
                app.ui.bindings.clear(def.cmd);
            } else {
                app.ui.bindings.reset(def.cmd);
            }
            app.ui.set.keys.conflict = None;
        }
        "keys.resetAll" => {
            app.ui.bindings.reset_all();
            app.ui.set.keys = crate::keyboard::KeysUi::default();
        }
        "keys.cancel" => app.ui.set.keys.conflict = None,
        "keys.replace" => {
            let Some((cmd, chord, _)) = app.ui.set.keys.conflict.clone() else {
                return Ok(());
            };
            if app
                .ui
                .bindings
                .conflict(&chord, cmd)
                .is_some_and(|other| keys::def_of(other).scope != Scope::Global)
            {
                return Err("This shortcut is fixed.".into());
            }
            app.ui.bindings.assign(cmd, chord, true);
            app.ui.set.keys.conflict = None;
        }
        _ => return Err("This keyboard action is unavailable.".into()),
    }
    Ok(())
}

fn ssh_command(app: &mut YaacApp, id: &str, values: &Value) -> Result<(), String> {
    match id {
        "ssh.import" => app.ui.set.ssh_config_hosts = Some(ssh_config_hosts()),
        id if id.starts_with("ssh.importHost.") => {
            let host = string(values, "host");
            if !app
                .ui
                .set
                .ssh_config_hosts
                .as_ref()
                .is_some_and(|hosts| hosts.iter().any(|listed| listed == host))
            {
                return Err("This SSH config host is unavailable.".into());
            }
            let mut panel = SshPanel::new(app.now(), None);
            panel.name = host.to_owned();
            panel.host = host.to_owned();
            app.ui.set.ssh = Some(panel);
            app.ui.set.revoke = None;
            app.ui.set.ssh_config_hosts = None;
        }
        "ssh.setupExisting" => {
            let target = target_id(app, values)?;
            let record = app
                .client
                .state
                .target(&target)
                .ok_or("The target is unavailable.")?;
            let TargetTransport::Ssh(transport) = &record.entry.transport else {
                return Err("Choose an SSH target.".into());
            };
            app.ui.set.ssh = Some(SshPanel::new(
                app.now(),
                Some((target, transport, record.entry.name.as_str())),
            ));
            app.ui.set.revoke = None;
        }
        "ssh.setup" => {
            let host = string(values, "host").trim();
            let port = string(values, "port").trim();
            if !form_ready(host, port) {
                return Err("Fill in the host and a valid port.".into());
            }
            let now = app.now();
            let s = app
                .ui
                .set
                .ssh
                .as_mut()
                .filter(|s| matches!(s.setup.phase, Phase::Form | Phase::Unreached(_)))
                .ok_or("The setup form has closed.")?;
            s.name = string(values, "name").trim().to_owned();
            s.host = host.to_owned();
            s.port = port.to_owned();
            s.user = string(values, "user").trim().to_owned();
            let conn = SshConn {
                name: if s.name.is_empty() {
                    s.host.clone()
                } else {
                    s.name.clone()
                },
                host: s.host.clone(),
                port: s.port.parse().map_err(|_| "Fill in a valid port.")?,
                user: s.user.clone(),
            };
            s.setup = SshSetup::new(conn, now).in_zone(now, app.zone);
            s.setup.start(app.ssh.as_mut());
        }
        "ssh.confirmHost" => {
            let s = app
                .ui
                .set
                .ssh
                .as_mut()
                .filter(|s| matches!(s.setup.phase, Phase::ConfirmHost(_)))
                .ok_or("The fingerprint confirmation has closed.")?;
            s.setup.confirm_host(app.ssh.as_mut());
        }
        "ssh.method" | "ssh.signIn" => {
            let s = app
                .ui
                .set
                .ssh
                .as_mut()
                .filter(|s| matches!(s.setup.phase, Phase::SignIn { .. }))
                .ok_or("The sign-in form has closed.")?;
            s.method = match string(values, "method") {
                "password" => SignInMethod::Password,
                "key" => SignInMethod::KeyFile,
                _ => return Err("Choose how to sign in.".into()),
            };
            if id == "ssh.signIn" {
                let credential = match s.method {
                    SignInMethod::Password => {
                        Credential::Password(secret(string(values, "password")))
                    }
                    SignInMethod::KeyFile => {
                        let path = string(values, "keyPath").trim();
                        if path.is_empty() {
                            return Err("Choose a key file.".into());
                        }
                        s.key_path = path.to_owned();
                        let passphrase = string(values, "passphrase");
                        Credential::KeyFile {
                            path: path.to_owned(),
                            passphrase: (!passphrase.is_empty()).then(|| secret(passphrase)),
                        }
                    }
                };
                s.setup.sign_in(app.ssh.as_mut(), credential);
            }
        }
        "ssh.tryAgain" => {
            let s = app
                .ui
                .set
                .ssh
                .as_mut()
                .filter(|s| matches!(s.setup.phase, Phase::Failed { .. }))
                .ok_or("Setup has already moved on.")?;
            s.setup.try_again();
        }
        "ssh.close" => {
            if app
                .ui
                .set
                .ssh
                .as_ref()
                .is_some_and(|s| !matches!(s.setup.phase, Phase::Working))
            {
                app.ui.set.ssh = None;
            }
        }
        "ssh.revoke" => {
            let target = target_id(app, values)?;
            let record = app
                .client
                .state
                .target(&target)
                .ok_or("The target is unavailable.")?;
            let TargetTransport::Ssh(transport) = &record.entry.transport else {
                return Err("Choose an SSH target.".into());
            };
            let key = app
                .client
                .state
                .ssh_records
                .get(&target)
                .and_then(SetupRecord::installed)
                .ok_or("YAAC installed no key on this server.")?
                .clone();
            let conn = SshConn {
                name: record.entry.name.to_string(),
                host: transport.host.clone(),
                port: transport.port.unwrap_or(22),
                user: transport.user.clone().unwrap_or_default(),
            };
            start_revoke(app, target, conn, key);
        }
        "ssh.revokeReady" => {
            let s = app
                .ui
                .set
                .ssh
                .as_ref()
                .ok_or("The setup panel has closed.")?;
            let Phase::Ready(record) = &s.setup.phase else {
                return Err("This setup is still in progress.".into());
            };
            let key = record
                .installed()
                .ok_or("YAAC installed no key on this server.")?
                .clone();
            let conn = s.setup.conn.clone();
            let target = s
                .existing
                .clone()
                .unwrap_or_else(|| TargetId::new(conn.name.clone()));
            start_revoke(app, target, conn, key);
        }
        "ssh.revokeClose" => {
            if app
                .ui
                .set
                .revoke
                .as_ref()
                .is_some_and(|r| r.revoke.phase != RevokePhase::Working)
            {
                app.ui.set.revoke = None;
            }
        }
        "ssh.revokeRetry" | "ssh.revokeRemoved" => {
            let panel = app
                .ui
                .set
                .revoke
                .as_mut()
                .filter(|r| matches!(r.revoke.phase, RevokePhase::CouldNotRevoke { .. }))
                .ok_or("This revoke attempt has closed.")?;
            if id == "ssh.revokeRetry" {
                panel.revoke.retry(app.ssh.as_mut());
            } else {
                let record = panel.revoke.removed_by_hand();
                let target = panel.target.clone();
                app.apply(vec![Action::SetRecord(target, record)]);
            }
        }
        "ssh.copy" => {
            return Err("Clipboard is unavailable. Select the key line to copy it.".into());
        }
        _ => return Err("This SSH action is unavailable.".into()),
    }
    Ok(())
}

fn secret(value: &str) -> Secret {
    let mut out = Secret::new();
    for c in value.chars() {
        out.push(c);
    }
    out
}

fn start_revoke(
    app: &mut YaacApp,
    target: TargetId,
    conn: SshConn,
    key: yaac_core::ssh_setup::InstalledKey,
) {
    let now = app.now();
    let revoke = Revoke::start(conn, key, now, app.ssh.as_mut()).in_zone(now, app.zone);
    app.ui.set.revoke = Some(RevokePanel { target, revoke });
    app.ui.set.ssh = None;
    app.ui.set.remove_confirm = None;
}

fn ssh_config_hosts() -> Vec<String> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
    let Some(config) = home.and_then(|h| {
        std::fs::read_to_string(std::path::Path::new(&h).join(".ssh").join("config")).ok()
    }) else {
        return Vec::new();
    };
    let mut result = Vec::new();
    for line in config.lines() {
        let line = line.split('#').next().unwrap_or_default();
        let mut parts = line.split_whitespace();
        if parts
            .next()
            .is_some_and(|key| key.eq_ignore_ascii_case("host"))
        {
            for alias in parts {
                if !alias.contains(['*', '?', '!']) && !result.iter().any(|seen| seen == alias) {
                    result.push(alias.to_owned());
                }
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn on_android_the_notification_page_says_the_settings_are_the_hubs() {
        let ctx = egui::Context::default();
        let mut app = crate::app::tests::demo_app(&ctx);
        app.ui.settings = true;
        app.ui.settings_page = Page::Notifications;
        let desktop = view(&app, None);
        assert_eq!(desktop["sections"][0]["rows"][0]["actions"][0]["label"], "Show desktop request notifications");
        super::super::dispatch(&mut app, &ctx, &mut None, "shell.platform", &json!({ "platform": "android" })).unwrap();
        let phone = view(&app, None);
        let rows = phone["sections"][0]["rows"].as_array().unwrap();
        assert!(rows[0]["detail"].as_str().unwrap().contains("Android's settings"));
        assert_eq!(rows[1]["actions"][0]["label"], "Show request notifications (on the desktop and this phone)");
        assert_eq!(rows[1]["actions"][0]["id"], "notifications.enabled", "the same hub setting");
    }

    #[test]
    fn on_android_the_window_page_offers_running_in_the_background_and_no_renderer_choice() {
        let ctx = egui::Context::default();
        let mut app = crate::app::tests::demo_app(&ctx);
        let desktop = window(&app);
        super::super::dispatch(&mut app, &ctx, &mut None, "shell.platform", &json!({ "platform": "android" })).unwrap();
        let android = window(&app);
        assert_ne!(desktop, android);
        let tray = &android["sections"][0]["rows"][0];
        assert_eq!(tray["actions"][0]["label"], "Keep running in the background");
        assert_eq!(tray["actions"][0]["enabled"], true);
        assert!(tray["detail"].as_str().unwrap().contains("Open and Quit"));
        let display = android["sections"][1]["rows"].as_array().unwrap();
        assert_eq!(display.len(), 1);
        assert!(display[0].get("fields").is_none(), "nothing to choose");
        // The same preference as the desktop's tray toggle.
        command(&mut app, &mut None, "settings.action", &json!({ "id": "window.tray", "values": { "checked": false } })).unwrap();
        assert!(!app.ui.tray_on_close);
        super::super::dispatch(&mut app, &ctx, &mut None, "shell.platform", &json!({ "platform": "desktop" })).unwrap();
        assert_eq!(window(&app)["sections"][0], {
            let mut back = crate::app::tests::demo_app(&ctx);
            back.ui.tray_on_close = false;
            window(&back)["sections"][0].clone()
        });
    }

    #[test]
    fn the_tailscale_toggle_is_on_the_android_page_not_the_window_page() {
        let ctx = egui::Context::default();
        let mut app = crate::app::tests::demo_app(&ctx);
        assert_eq!(Page::ALL[Page::ALL.iter().position(|p| *p == Page::Window).unwrap() + 1], Page::Android);
        let toggles = |v: &Value| v.to_string().contains("tailscale");
        assert!(!toggles(&window(&app)), "Window no longer holds it");
        app.client.state.hub.tailscale_attach = Some(true);
        app.ui.settings = true;
        app.ui.settings_page = Page::Android;
        let desktop = view(&app, None);
        assert_eq!(desktop["page"], "android");
        let row = &desktop["sections"][0]["rows"][0];
        assert_eq!(row["actions"][0]["id"], "android.tailscale");
        assert_eq!(row["actions"][0]["label"], "Allow the Android client over Tailscale");
        assert_eq!(row["actions"][0]["enabled"], true);
        assert!(!row["detail"].as_str().unwrap().contains("belongs to the hub"));
        command(&mut app, &mut None, "settings.action", &json!({ "id": "android.tailscale", "values": { "checked": false } })).unwrap();
        assert_eq!(app.client.state.hub.tailscale_attach, Some(false));
        // On the phone it is the same hub setting, and the page says whose it is.
        super::super::dispatch(&mut app, &ctx, &mut None, "shell.platform", &json!({ "platform": "android" })).unwrap();
        assert!(!toggles(&window(&app)));
        let phone = view(&app, None);
        let row = &phone["sections"][0]["rows"][0];
        assert_eq!(row["actions"][0]["id"], "android.tailscale");
        assert!(row["detail"].as_str().unwrap().contains("This setting belongs to the hub on"));
    }

    #[test]
    fn touch_input_changes_only_the_texts_that_name_a_mechanic() {
        let ctx = egui::Context::default();
        let mut app = crate::app::tests::demo_app(&ctx);
        let pointer = keyboard(&app);
        assert!(pointer["detail"].as_str().unwrap().contains("Click a key"));
        super::super::dispatch(&mut app, &ctx, &mut None, "shell.inputMode", &json!({ "mode": "touch" })).unwrap();
        let touch = keyboard(&app);
        assert!(touch["detail"].as_str().unwrap().contains("With a hardware keyboard, tap a key"));
        assert_eq!(pointer["sections"], touch["sections"]);
        super::super::dispatch(&mut app, &ctx, &mut None, "shell.inputMode", &json!({ "mode": "pointer" })).unwrap();
        assert_eq!(keyboard(&app), pointer);
    }

    #[test]
    fn keyboard_switch_updates_the_shortcut_dispatched_by_a_focused_terminal() {
        let mut app = crate::app::tests::demo_app(&egui::Context::default());
        let allows = |app: &YaacApp, id: &str| super::super::navigation::shortcuts(app).into_iter()
            .find(|binding| binding["id"] == id).unwrap()["inTerminal"] == true;
        assert!(!allows(&app, "commander"));
        assert!(allows(&app, "toggle-terminal-panel"));
        keyboard_command(&mut app, "keys.inTerminal", &json!({"command":"commander", "checked":true})).unwrap();
        assert!(allows(&app, "commander"));
    }

    #[test]
    fn model_guidance_editor_sends_the_specific_model_and_preserves_the_text() {
        let mut app = crate::app::tests::demo_app(&egui::Context::default());
        let local = TargetId::new("local");
        let target = app.client.state.targets.iter_mut().find(|target| target.entry.id == local).unwrap();
        let provider = target.providers.iter_mut().find(|provider| !provider.models.is_empty()).unwrap();
        provider.capabilities.push(yaac_protocol::Capability::ModelBestUse);
        let model = &mut provider.models[0];
        model.resolved_model = Some(yaac_protocol::ModelId::new("specific-model"));
        model.best_use = Some(yaac_protocol::ModelBestUse { summary: "Small tests".into(), custom: true });
        let identity = json!({ "target": local, "instance": provider.instance, "model": model.id, "expectedModel": "specific-model" });
        let mut selected = Some(local.clone());
        command(&mut app, &mut selected, "settings.action", &json!({
            "id": "provider.bestUsePanel", "values": identity,
        })).unwrap();
        let view = providers(&app, Some(&local));
        let rows = view["sections"].as_array().unwrap().iter().find(|section| section["id"] == "instances").unwrap()["rows"].as_array().unwrap();
        let row = rows.iter().find(|row| row["detail"] == "Best used as: Small tests").unwrap();
        assert_eq!(row["status"], "User-edited");
        command(&mut app, &mut selected, "settings.action", &row["actions"][0]).unwrap();
        assert_eq!(app.ui.set.model_guidance_edit.as_ref().unwrap().text, "Small tests");
        let mut values = identity;
        values["bestUse"] = json!("Review focused changes");
        command(&mut app, &mut selected, "settings.action", &json!({ "id": "provider.bestUseSave", "values": values })).unwrap();
        assert!(app.demo.as_ref().unwrap().requests().iter().any(|(target, method, params)| {
            target.as_ref() == Some(&local) && method == yaac_protocol::methods::names::PROVIDER_MODEL_BEST_USE_SET
                && params["expectedModel"] == "specific-model" && params["summary"] == "Review focused changes"
                && params["opId"].is_string()
        }));
    }

    #[test]
    fn the_pc_update_button_shows_only_for_a_phone_newer_than_its_hub() {
        use yaac_core::state::{HostUpdate, HostUpdatePhase, HubState};
        let hub = |build: &str| HubState { build: Some(build.into()), ..HubState::default() };
        let phone = "0.1.0+1060";
        assert!(host_update_view("desk", phone, &hub(phone)).is_none(), "the same build");
        assert!(host_update_view("desk", phone, &hub("0.1.0+1070")).is_none(), "an older phone");
        assert!(host_update_view("desk", "0.1.0+dev.ab", &hub("0.1.0+1050")).is_none(), "a development phone");
        assert!(host_update_view("desk", phone, &HubState::default()).is_none(), "the hub's build unknown");
        let view = host_update_view("desk", phone, &hub("0.1.0+1050")).unwrap();
        let button = &view["rows"][0]["actions"][0];
        assert_eq!(button["id"], "pc.update");
        assert_eq!(button["label"], "Update YAAC on desk to build 1060");
        assert_eq!(button["confirm"]["ok"], "Update");
        // While it runs: the state, no button; after the drop, "updating…".
        let mut state = hub("0.1.0+1050");
        state.host_update = Some(HostUpdate { build: phone.into(), phase: HostUpdatePhase::Downloading { done: 1, total: 4 }, requested_here: true });
        let view = host_update_view("desk", phone, &state).unwrap();
        assert!(view["rows"][0]["actions"].is_null());
        assert_eq!(view["rows"][0]["fields"][0]["value"], 25.0);
        state.host_update.as_mut().unwrap().phase = HostUpdatePhase::Restarting;
        let view = host_update_view("desk", phone, &state).unwrap();
        assert!(view["rows"][0]["status"].as_str().unwrap().contains("desk is updating"));
        // Failed: why, and the button again.
        state.host_update.as_mut().unwrap().phase = HostUpdatePhase::Failed("GitHub could not be reached".into());
        let view = host_update_view("desk", phone, &state).unwrap();
        assert_eq!(view["rows"][0]["tone"], "error");
        assert_eq!(view["rows"][0]["actions"][0]["id"], "pc.update");
    }

    #[test]
    fn off_model_stays_dimmed_and_can_be_turned_on_from_settings() {
        let mut app = crate::app::tests::demo_app(&egui::Context::default());
        let local = TargetId::new("local");
        let target = app.client.state.targets.iter_mut().find(|target| target.entry.id == local).unwrap();
        let provider = target.providers.iter_mut().find(|provider| !provider.models.is_empty()).unwrap();
        provider.capabilities.push(yaac_protocol::Capability::ModelAvailability);
        let off = provider.models.remove(0);
        let identity = json!({ "target": local, "instance": provider.instance, "model": off.id });
        provider.disabled_models.push(off);

        let mut selected = Some(local.clone());
        command(&mut app, &mut selected, "settings.action", &json!({
            "id": "provider.bestUsePanel", "values": identity,
        })).unwrap();
        let view = providers(&app, Some(&local));
        let rows = view["sections"].as_array().unwrap().iter()
            .find(|section| section["id"] == "instances").unwrap()["rows"].as_array().unwrap();
        let row = rows.iter().find(|row| row["dimmed"] == true).unwrap();
        let toggle = &row["actions"][0];
        assert_eq!(toggle["id"], "provider.modelEnabled");
        assert_eq!(toggle["checked"], false);
        assert_eq!(toggle["enabled"], true);

        let mut values = identity;
        values["checked"] = json!(true);
        command(&mut app, &mut selected, "settings.action", &json!({
            "id": "provider.modelEnabled", "values": values,
        })).unwrap();
        assert!(app.demo.as_ref().unwrap().requests().iter().any(|(target, method, params)| {
            target.as_ref() == Some(&local) && method == yaac_protocol::methods::names::PROVIDER_INSTANCE_SET_MODEL_ENABLED
                && params["instance"] == values["instance"] && params["model"] == values["model"]
                && params["enabled"] == true && params["opId"].is_string()
        }));
    }

    #[test]
    fn session_discovery_switch_updates_only_the_selected_target() {
        let mut app = crate::app::tests::demo_app(&egui::Context::default());
        let local = TargetId::new("local");
        let remote = TargetId::new("devbox");
        let toggle = |app: &YaacApp, target: &TargetId| {
            providers(app, Some(target))["sections"][1]["rows"][0]["actions"][0].clone()
        };
        assert_eq!(toggle(&app, &local)["id"], "providers.sessionDiscovery");
        assert_eq!(toggle(&app, &local)["checked"], true);
        assert_eq!(toggle(&app, &remote)["checked"], true);

        command(&mut app, &mut Some(local.clone()), "settings.action", &json!({
            "id": "providers.sessionDiscovery", "values": { "target": local, "checked": false },
        })).unwrap();
        assert_eq!(toggle(&app, &local)["enabled"], false, "update is pending");
        app.client.pump();
        assert_eq!(toggle(&app, &local)["checked"], false);
        assert_eq!(toggle(&app, &remote)["checked"], true);
    }

    #[test]
    fn about_diagnosis_routes_to_the_ready_remote_harness_and_shows_the_report() {
        use yaac_protocol::attach::ConnectionStatus;
        use yaac_protocol::{Capability, EffortId, EffortInfo, ModelId, ProviderStatus};

        let mut app = crate::app::tests::demo_app(&egui::Context::default());
        for target in &mut app.client.state.targets {
            target.entry.state.status = ConnectionStatus::Offline;
        }
        let button = |app: &YaacApp| about(app)["sections"][0]["actions"][2]["enabled"].clone();
        assert_eq!(button(&app), false);
        let remote = app.client.state.targets.iter_mut()
            .find(|target| target.entry.id.as_str() == "devbox").unwrap();
        remote.entry.state.status = ConnectionStatus::Connected;
        let provider = remote.providers.first_mut().unwrap();
        provider.status = ProviderStatus::Ready;
        provider.capabilities.push(Capability::Troubleshoot);
        let model = provider.models.first_mut().unwrap();
        model.id = ModelId::new("fixture-model-id");
        model.display_name = "Opus 5.5".into();
        model.hidden = false;
        model.efforts = vec![EffortInfo {
            id: EffortId::new("fixture-effort-id"), display_name: Some("Medium".into()), description: None,
        }];
        assert_eq!(button(&app), true);
        let mut target = None;
        command(&mut app, &mut target, "settings.action", &json!({ "id": "about.troubleshoot" })).unwrap();
        command(&mut app, &mut target, "settings.action", &json!({
            "id": "about.diagnose", "values": { "symptom": "The window is blank." }
        })).unwrap();
        assert!(app.demo.as_ref().unwrap().requests().iter().any(|(target, method, params)| {
            target.as_ref().is_some_and(|id| id.as_str() == "devbox")
                && method == yaac_protocol::methods::names::DIAGNOSTICS_TROUBLESHOOT
                && params["model"] == "fixture-model-id"
                && params["effort"] == "fixture-effort-id"
                && params["symptom"] == "The window is blank."
        }));
        app.troubleshoot.report = "Developer report".into();
        assert_eq!(about(&app)["sections"][2]["rows"][1]["fields"][0]["value"], "Developer report");
    }

    #[test]
    fn about_refresh_rechecks_enabled_offline_targets_without_enabling_others() {
        use yaac_protocol::attach::{self, ConnectionStatus};
        let mut app = crate::app::tests::demo_app(&egui::Context::default());
        let offline = TargetId::new("devbox");
        for target in &mut app.client.state.targets {
            target.entry.state.status = if target.entry.id.as_str() == "local" {
                ConnectionStatus::Connected
            } else {
                target.entry.enabled = target.entry.id == offline;
                ConnectionStatus::Offline
            };
        }

        let view = about(&app);
        let section = &view["sections"][1];
        assert_eq!(section["inlineActions"], true);
        let refresh = &section["actions"][0];
        assert_eq!(refresh["id"], "about.refresh");
        let before = app.demo.as_ref().unwrap().requests().len();
        command(&mut app, &mut None, "settings.action", refresh).unwrap();
        let requests = app.demo.as_ref().unwrap().requests();
        let messages: Vec<_> = requests[before..].iter()
            .map(|(_, method, params)| (method.as_str(), params.clone())).collect();
        assert_eq!(messages, [
            (yaac_core::client::hub_names::TARGETS_RECONNECT, json!({ "id": offline })),
            (attach::names::TARGETS_LIST, json!({})),
        ]);
    }

    #[test]
    fn refresh_is_available_when_the_local_provider_list_is_empty() {
        let mut app = crate::app::tests::demo_app(&egui::Context::default());
        let local = TargetId::new("local");
        let target = app.client.state.targets.iter_mut().find(|t| t.entry.id == local).unwrap();
        target.providers.clear();

        let view = providers(&app, Some(&local));
        let refresh = &view["sections"][0]["rows"][0]["actions"][0];
        assert_eq!(refresh["id"], "providers.refresh");
        assert_eq!(refresh["enabled"], true);
        assert_eq!(refresh["values"], json!({ "target": local }));

        command(&mut app, &mut Some(local.clone()), "settings.action", refresh).unwrap();
        assert!(app.demo.as_ref().unwrap().requests().iter().any(|(target, method, params)| {
            target.as_ref() == Some(&local) && method == yaac_protocol::methods::names::PROVIDER_REFRESH
                && params.get("instance").is_none()
        }));
    }
}
