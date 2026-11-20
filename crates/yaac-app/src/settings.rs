//! Settings: a full-window page with its own navigation (Targets, Providers, Appearance,
//! Advanced) and "Back to threads". No thread list, no agents panel, no composer.

use eframe::egui::{self, Margin, RichText, pos2, vec2};
use yaac_core::attention::Mark;
use yaac_core::settings::{LEGEND, Page, SshAction, diagnostics, kind_label, providers_view, targets_view};
use yaac_core::ssh_setup::{Credential, Phase, Revoke, RevokePhase, Secret, SetupRecord, SignInMethod, SshConn, SshSetup, SshSetupBackend, StepLine, StepMark, form_ready};
use yaac_protocol::attach::{SshTransport, TargetTransport, WslTransport};
use yaac_protocol::{InstanceId, TargetId};

use crate::app::{Action, Menu, YaacApp};
use crate::icons;
use crate::theme::{self, Palette, ThemeChoice, size};
use crate::widgets;

/// The "Add SSH server" panel (screens 14, 22, 15, 16). It owns the password: a zeroised
/// buffer handed to the backend once, on Connect.
#[derive(Debug)]
pub(crate) struct SshPanel {
    pub setup: SshSetup,
    pub name: String,
    pub host: String,
    pub port: String,
    pub user: String,
    pub method: SignInMethod,
    pub password: Secret,
    pub show: bool,
    pub key_path: String,
    pub passphrase: Secret,
    /// Set up again an existing connection (its row's "Set up…").
    pub existing: Option<TargetId>,
    /// The result was recorded (target added, record stored).
    pub recorded: bool,
}

impl SshPanel {
    pub(crate) fn new(now: i64, existing: Option<(TargetId, &SshTransport, &str)>) -> Self {
        let user_default = std::env::var("USER").or_else(|_| std::env::var("USERNAME")).unwrap_or_default();
        let (name, host, port, user, id) = match existing {
            Some((id, t, name)) => (
                name.to_owned(),
                t.host.clone(),
                t.port.unwrap_or(22).to_string(),
                t.user.clone().unwrap_or(user_default),
                Some(id),
            ),
            None => (String::new(), String::new(), "22".to_owned(), user_default, None),
        };
        let conn = SshConn { name: name.clone(), host: host.clone(), port: 22, user: user.clone() };
        Self {
            setup: SshSetup::new(conn, now),
            name,
            host,
            port,
            user,
            method: SignInMethod::Password,
            password: Secret::new(),
            show: false,
            key_path: String::new(),
            passphrase: Secret::new(),
            existing: id,
            recorded: false,
        }
    }

    fn conn(&self) -> SshConn {
        let name = if self.name.trim().is_empty() { self.host.trim().to_owned() } else { self.name.trim().to_owned() };
        SshConn { name, host: self.host.trim().to_owned(), port: self.port.trim().parse().unwrap_or(22), user: self.user.trim().to_owned() }
    }
}

/// A revoke in progress (screen 16).
#[derive(Debug)]
pub(crate) struct RevokePanel {
    pub target: TargetId,
    pub revoke: Revoke,
}

/// The settings pages' own state.
#[derive(Debug, Default)]
pub(crate) struct SettingsUi {
    pub ssh: Option<SshPanel>,
    pub revoke: Option<RevokePanel>,
    /// Remove was pressed on a connection YAAC set up: offer Revoke first.
    pub remove_confirm: Option<TargetId>,
    pub providers_target: Option<TargetId>,
    pub add_kind: Option<yaac_protocol::ProviderKind>,
    pub add_name: String,
    pub add_socket: String,
    pub edit: Option<(TargetId, InstanceId, String, String)>,
    pub wsl_distro: String,
    pub wsl_user: String,
    pub diag_target: Option<TargetId>,
    pub ssh_config_hosts: Option<Vec<String>>,
    /// Records the setup and revoke flows produced, to store.
    pub records: Vec<(TargetId, SetupRecord)>,
}

impl SettingsUi {
    /// Apply what the SSH backend reported. Returns whether anything changed.
    pub(crate) fn pump_ssh(&mut self, backend: &mut dyn SshSetupBackend) -> bool {
        let mut any = false;
        if let Some(s) = &mut self.ssh {
            any |= s.setup.pump(backend);
        }
        if let Some(r) = &mut self.revoke {
            if let Some(rec) = r.revoke.pump(backend) {
                self.records.push((r.target.clone(), rec));
                any = true;
            }
            any |= r.revoke.phase != RevokePhase::Working;
        }
        any
    }
}

fn mark_for(m: StepMark) -> Mark {
    match m {
        StepMark::Done => Mark::Done,
        StepMark::Current => Mark::Running,
        StepMark::Hollow => Mark::Idle,
        StepMark::Failed => Mark::Failed,
    }
}

pub(crate) fn show(ui: &mut egui::Ui, app: &mut YaacApp, actions: &mut Vec<Action>) {
    let p = app.p;
    for (t, r) in std::mem::take(&mut app.ui.set.records) {
        actions.push(Action::SetRecord(t, r));
    }
    let full = ui.max_rect();
    ui.painter().rect_filled(full, 0.0, p.bg);
    let nav = egui::Rect::from_min_max(full.min, pos2(full.left() + 220.0, full.bottom()));
    ui.painter().rect_filled(nav, 0.0, p.panel);
    ui.painter().vline(nav.right(), full.y_range(), egui::Stroke::new(1.0, p.border));
    ui.scope_builder(egui::UiBuilder::new().max_rect(nav.shrink2(vec2(12.0, 12.0))).id_salt("settings-nav"), |ui| {
        let back = widgets::plain(ui, &p, "     Back to threads", Some("Esc"));
        icons::arrow_left(ui.painter(), pos2(back.rect.left() + 18.0, back.rect.center().y), 14.0, p.text);
        if back.clicked() {
            actions.push(Action::Settings(false));
        }
        ui.add_space(14.0);
        for page in Page::ALL {
            let sel = page == app.ui.settings_page;
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), egui::Sense::click());
            if sel {
                ui.painter().rect_filled(rect, 5.0, p.selected);
            } else if resp.hovered() {
                ui.painter().rect_filled(rect, 5.0, p.faint);
            }
            let g = ui.fonts_mut(|f| f.layout_no_wrap(page.label().to_owned(), theme::ui(size::UI + 0.5), p.text));
            ui.painter().galley(pos2(rect.left() + 12.0, rect.center().y - g.size().y / 2.0), g, p.text);
            if resp.clicked() {
                actions.push(Action::SettingsPage(page));
            }
        }
    });
    let content = egui::Rect::from_min_max(pos2(nav.right() + 1.0, full.top()), full.max);
    ui.scope_builder(egui::UiBuilder::new().max_rect(content).id_salt("settings-content"), |ui| {
        egui::ScrollArea::vertical().id_salt(("settings-scroll", app.ui.settings_page as u8)).auto_shrink([false, false]).show(ui, |ui| {
            egui::Frame::NONE.inner_margin(Margin { left: 36, right: 36, top: 22, bottom: 30 }).show(ui, |ui| {
                ui.set_max_width(940.0f32.min(ui.available_width()));
                match app.ui.settings_page {
                    Page::Targets => targets(ui, app, actions),
                    Page::Providers => providers(ui, app, actions),
                    Page::Appearance => appearance(ui, app, actions),
                    Page::Advanced => advanced(ui, app, actions),
                }
            });
        });
    });
    let busy = app.ui.menu.is_some() || app.ui.set.ssh.is_some() || app.ui.set.revoke.is_some();
    if !busy && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
        actions.push(Action::Settings(false));
    }
}

/// A fixed-width cell, left-aligned (names line up in a column).
fn cell(ui: &mut egui::Ui, width: f32, text: RichText) {
    ui.allocate_ui_with_layout(vec2(width, 22.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.set_width(width);
        ui.add(egui::Label::new(text).truncate());
    });
}

fn title_row(ui: &mut egui::Ui, title: &str, right: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).font(theme::bold(size::TITLE + 3.0)));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), right);
    });
}

// ── Targets ─────────────────────────────────────────────────────────────────────────────

fn targets(ui: &mut egui::Ui, app: &mut YaacApp, actions: &mut Vec<Action>) {
    let p = app.p;
    let panel_open = app.ui.set.ssh.is_some() || app.ui.set.revoke.is_some();
    let mut open_add = false;
    let mut import = false;
    title_row(ui, "Targets", |ui| {
        if widgets::plain(ui, &p, "Import from SSH config", None).clicked() {
            import = true;
        }
        let add = if panel_open { widgets::plain(ui, &p, "Add SSH server", None) } else { widgets::primary(ui, &p, "Add SSH server", None) };
        if add.clicked() {
            open_add = true;
        }
    });
    if open_add {
        app.ui.set.ssh = Some(SshPanel::new(app.now(), None));
        app.ui.set.revoke = None;
    }
    if import {
        app.ui.set.ssh_config_hosts = Some(read_ssh_config_hosts());
    }
    if let Some(hosts) = app.ui.set.ssh_config_hosts.clone() {
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(widgets::secondary(if hosts.is_empty() { "No hosts were found in ~/.ssh/config." } else { "Hosts in ~/.ssh/config — choose one to fill in the form:" }, &p));
            for h in &hosts {
                if widgets::link(ui, &p, h, theme::ui(size::UI)).clicked() {
                    let mut panel = SshPanel::new(app.now(), None);
                    panel.name = h.clone();
                    panel.host = h.clone();
                    app.ui.set.ssh = Some(panel);
                    app.ui.set.ssh_config_hosts = None;
                }
            }
        });
    }
    if app.ui.set.ssh.is_some() {
        ssh_panel(ui, app, actions);
    }
    if app.ui.set.revoke.is_some() {
        revoke_panel(ui, app, actions);
    }
    let v = targets_view(&app.client.state);

    widgets::heading(ui, "Local");
    if let Some((id, enabled)) = &v.local {
        ui.horizontal(|ui| {
            ui.set_min_height(30.0);
            ui.label("This computer");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::switch(ui, &p, *enabled).on_hover_text(if *enabled { "Disable" } else { "Enable" }).clicked() {
                    actions.push(Action::TargetEnabled(id.clone(), !enabled));
                }
            });
        });
    }

    widgets::heading(ui, "SSH");
    if v.ssh.is_empty() {
        ui.label(widgets::secondary("No SSH servers yet.", &p));
    }
    for row in &v.ssh {
        let mut unavailable_reason = None;
        ui.horizontal(|ui| {
            ui.set_min_height(34.0);
            cell(ui, 110.0, RichText::new(&row.name).font(theme::bold(size::UI)));
            widgets::mark(ui, &p, row.mark, 16.0);
            ui.label(&row.line);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Actions sit in a fixed column, so the switches line up.
                ui.allocate_ui_with_layout(vec2(250.0, 30.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.set_width(250.0);
                for a in row.actions.iter().rev() {
                    match a {
                        SshAction::Remove => {
                            if widgets::plain(ui, &p, "Remove", None).clicked() {
                                let installed = app.client.state.ssh_records.get(&row.id).and_then(|r| r.installed()).is_some();
                                if installed {
                                    app.ui.set.remove_confirm = Some(row.id.clone());
                                } else {
                                    actions.push(Action::RemoveTarget(row.id.clone()));
                                }
                            }
                        }
                        SshAction::Revoke => {
                            if widgets::plain(ui, &p, "Revoke", None).clicked() {
                                start_revoke(app, &row.id);
                            }
                        }
                        SshAction::RevokeUnavailable(reason) => {
                            widgets::unavailable(ui, &p, "Revoke", "");
                            unavailable_reason = Some(reason.clone());
                        }
                        SshAction::Retry => {
                            if widgets::plain(ui, &p, "Retry", None).clicked() {
                                actions.push(Action::Retry(row.id.clone()));
                            }
                        }
                        SshAction::SetUp => {
                            if widgets::plain(ui, &p, "Set up…", None).clicked() {
                                let t = app.client.state.target(&row.id).map(|t| (t.entry.transport.clone(), t.entry.name.to_string()));
                                if let Some((TargetTransport::Ssh(s), name)) = t {
                                    app.ui.set.ssh = Some(SshPanel::new(app.now(), Some((row.id.clone(), &s, &name))));
                                }
                            }
                        }
                    }
                }
                });
                if let Some(on) = row.switch {
                    ui.add_space(12.0);
                    if widgets::switch(ui, &p, on).on_hover_text(if on { "Disable" } else { "Enable" }).clicked() {
                        actions.push(Action::TargetEnabled(row.id.clone(), !on));
                    }
                }
            });
        });
        if let Some(r) = unavailable_reason {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                ui.label(widgets::secondary(r, &p));
            });
        }
        if app.ui.set.remove_confirm.as_ref() == Some(&row.id) {
            ui.horizontal(|ui| {
                ui.add_space(116.0);
                ui.label(widgets::secondary(format!("YAAC installed a key on {}. Revoke it before removing?", row.name), &p));
                if widgets::plain(ui, &p, "Revoke first", None).clicked() {
                    start_revoke(app, &row.id);
                    app.ui.set.remove_confirm = None;
                }
                if widgets::plain(ui, &p, "Remove only", None).clicked() {
                    actions.push(Action::RemoveTarget(row.id.clone()));
                    app.ui.set.remove_confirm = None;
                }
                if widgets::plain(ui, &p, "Cancel", None).clicked() {
                    app.ui.set.remove_confirm = None;
                }
            });
        }
    }
    ui.add_space(8.0);
    ui.label(widgets::secondary(LEGEND, &p));

    if app.host_windows {
        let on = app.client.state.wsl_on;
        ui.add_space(18.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("WSL").font(theme::bold(size::SECTION)));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::switch(ui, &p, on).on_hover_text("Run agents inside Linux distros on this PC").clicked() {
                    actions.push(Action::WslOn(!on));
                }
            });
        });
        ui.add_space(6.0);
        if !on {
            ui.label(widgets::secondary("Off. Turn it on to run agents inside a Linux distro on this PC.", &p));
        } else {
            for r in &v.wsl {
                ui.horizontal(|ui| {
                    ui.set_min_height(32.0);
                    cell(ui, 110.0, RichText::new(&r.distro).font(theme::bold(size::UI)));
                    cell(ui, 110.0, widgets::secondary(&r.user, &p));
                    if !r.enabled {
                        widgets::mark(ui, &p, Mark::Off, 15.0);
                        ui.label(widgets::secondary("off", &p));
                    }
                    if let Some(n) = &r.note {
                        ui.label(widgets::secondary(n, &p));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if widgets::plain(ui, &p, "Remove", None).clicked() {
                            actions.push(Action::RemoveTarget(r.id.clone()));
                        }
                        ui.add_space(12.0);
                        if widgets::switch(ui, &p, r.enabled).clicked() {
                            actions.push(Action::TargetEnabled(r.id.clone(), !r.enabled));
                        }
                    });
                });
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                // SEAM: the installed distros (`wsl.exe -l -q`) are not offered by the hub yet;
                // the distro is typed.
                widgets::field(ui, &p, &mut app.ui.set.wsl_distro, "Distro", 160.0, false);
                widgets::field(ui, &p, &mut app.ui.set.wsl_user, "User (optional)", 200.0, false);
                let ok = !app.ui.set.wsl_distro.trim().is_empty();
                if ok {
                    if widgets::plain(ui, &p, "Add", None).clicked() {
                        let distro = app.ui.set.wsl_distro.trim().to_owned();
                        let user = Some(app.ui.set.wsl_user.trim().to_owned()).filter(|u| !u.is_empty());
                        let name = match &user {
                            Some(u) => format!("wsl: {distro} ({u})"),
                            None => format!("wsl: {distro}"),
                        };
                        actions.push(Action::AddTarget(name, TargetTransport::Wsl(WslTransport { distro, user })));
                        app.ui.set.wsl_distro.clear();
                        app.ui.set.wsl_user.clear();
                    }
                } else {
                    widgets::unavailable(ui, &p, "Add", "");
                }
            });
            ui.label(widgets::secondary("Adding a distro again with another user opens a second session to it.", &p));
        }
    }
}

fn start_revoke(app: &mut YaacApp, id: &TargetId) {
    let Some(t) = app.client.state.target(id) else { return };
    let TargetTransport::Ssh(s) = &t.entry.transport else { return };
    let Some(key) = app.client.state.ssh_records.get(id).and_then(|r| r.installed()).cloned() else { return };
    let conn = SshConn { name: t.entry.name.to_string(), host: s.host.clone(), port: s.port.unwrap_or(22), user: s.user.clone().unwrap_or_default() };
    let now = app.now();
    let revoke = Revoke::start(conn, key, now, app.ssh.as_mut());
    app.ui.set.revoke = Some(RevokePanel { target: id.clone(), revoke });
    app.ui.set.ssh = None;
}

/// The step list: done, hollow, failed, and the current step.
fn steps(ui: &mut egui::Ui, p: &Palette, lines: &[StepLine], current: Option<StepLine>) {
    for l in lines.iter().chain(current.as_ref()) {
        ui.horizontal(|ui| {
            widgets::mark(ui, p, mark_for(l.mark), 15.0);
            let text = if l.mark == StepMark::Current { RichText::new(&l.text).font(theme::bold(size::UI)) } else { RichText::new(&l.text) };
            ui.label(text);
        });
        if let Some(d) = &l.detail {
            ui.horizontal(|ui| {
                ui.add_space(22.0);
                ui.label(RichText::new(d).font(theme::ui(size::SMALL)).color(if l.mark == StepMark::Failed { p.failed } else { p.text2 }));
            });
        }
    }
}

fn panel_frame<R>(ui: &mut egui::Ui, p: &Palette, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.add_space(12.0);
    egui::Frame::NONE
        .fill(p.panel)
        .corner_radius(6)
        .inner_margin(Margin::same(18))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

fn ssh_panel(ui: &mut egui::Ui, app: &mut YaacApp, actions: &mut Vec<Action>) {
    let p = app.p;
    let now = app.now();
    let Some(mut panel) = app.ui.set.ssh.take() else { return };
    let mut keep = true;
    // Record the outcome once: add the connection (or keep the existing one) and what YAAC did.
    if let Phase::Ready(rec) = &panel.setup.phase
        && !panel.recorded
    {
        let conn = panel.setup.conn.clone();
        let id = panel.existing.clone().unwrap_or_else(|| TargetId::new(conn.name.clone()));
        if panel.existing.is_none() {
            let transport = TargetTransport::Ssh(SshTransport { host: conn.host.clone(), port: (conn.port != 22).then_some(conn.port), user: Some(conn.user.clone()) });
            actions.push(Action::AddTarget(conn.name.clone(), transport));
        }
        // SEAM: the record is window-local (keyed by the name, which the demo hub uses as id).
        actions.push(Action::SetRecord(id, rec.clone()));
        panel.recorded = true;
    }
    let title = match &panel.setup.phase {
        Phase::Failed { .. } => "Could not finish",
        _ if panel.existing.is_some() => "Set up SSH server",
        _ => "Add SSH server",
    };
    panel_frame(ui, &p, |ui| {
        ui.label(RichText::new(title).font(theme::bold(size::SECTION + 1.0)));
        ui.add_space(8.0);
        let editable = matches!(panel.setup.phase, Phase::Form | Phase::Unreached(_));
        egui::Grid::new("ssh-form").num_columns(4).spacing(vec2(10.0, 8.0)).show(ui, |ui| {
            ui.label(widgets::secondary("Name", &p));
            if editable { widgets::field(ui, &p, &mut panel.name, "devbox", 200.0, false); } else { ui.label(&panel.name); }
            ui.label(widgets::secondary("User", &p));
            if editable { widgets::field(ui, &p, &mut panel.user, "user", 140.0, false); } else { ui.label(&panel.user); }
            ui.end_row();
            ui.label(widgets::secondary("Host", &p));
            if editable { widgets::field(ui, &p, &mut panel.host, "host.example.com", 200.0, false); } else { ui.label(&panel.host); }
            ui.label(widgets::secondary("Port", &p));
            if editable { widgets::field(ui, &p, &mut panel.port, "22", 60.0, false); } else { ui.label(&panel.port); }
            ui.end_row();
        });
        ui.add_space(10.0);
        steps(ui, &p, &panel.setup.steps, panel.setup.current());
        let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
        let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
        match panel.setup.phase.clone() {
            Phase::Form | Phase::Unreached(_) => {
                if let Phase::Unreached(e) = &panel.setup.phase {
                    ui.label(RichText::new(e).color(p.failed));
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let ready = form_ready(&panel.host, &panel.port);
                    if ready {
                        if widgets::primary(ui, &p, "Setup", Some("Enter")).clicked() || enter {
                            panel.setup = SshSetup::new(panel.conn(), now);
                            panel.setup.start(app.ssh.as_mut());
                        }
                    } else {
                        widgets::unavailable(ui, &p, "Setup", "Fill in the host and port.");
                    }
                    if widgets::plain(ui, &p, "Cancel", Some("Esc")).clicked() || esc {
                        keep = false;
                    }
                });
            }
            Phase::Working => {}
            Phase::ConfirmHost(k) => {
                ui.add_space(8.0);
                egui::Frame::NONE.fill(p.faint).corner_radius(5).inner_margin(Margin::same(12)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(widgets::secondary("This server's key fingerprint", &p));
                    ui.label(RichText::new(&k.fingerprint).font(theme::mono(size::MONO + 0.5)));
                    ui.label(widgets::secondary(&k.algorithm, &p));
                    ui.add_space(4.0);
                    ui.label("Check it against what your server administrator gave you.");
                });
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let ok = widgets::primary(ui, &p, "This is the right server", Some("Enter"));
                    if !ok.has_focus() && ui.memory(|m| m.focused().is_none()) {
                        ok.request_focus();
                    }
                    if ok.clicked() || enter {
                        panel.setup.confirm_host(app.ssh.as_mut());
                    }
                    if widgets::plain(ui, &p, "Cancel", Some("Esc")).clicked() || esc {
                        keep = false;
                    }
                });
            }
            Phase::SignIn { error } => {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.radio(panel.method == SignInMethod::Password, "Password").clicked() {
                        panel.method = SignInMethod::Password;
                    }
                    if ui.radio(panel.method == SignInMethod::KeyFile, "Key file").clicked() {
                        panel.method = SignInMethod::KeyFile;
                    }
                });
                ui.add_space(4.0);
                let pid = egui::Id::unique("ssh-password");
                if panel.method == SignInMethod::Password {
                    ui.horizontal(|ui| {
                        widgets::secret_field(ui, &p, &mut panel.password, panel.show, 280.0, pid);
                        if widgets::plain(ui, &p, if panel.show { "Hide" } else { "Show" }, None).clicked() {
                            panel.show = !panel.show;
                        }
                    });
                    ui.label(widgets::secondary("Used once. YAAC does not save it.", &p));
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        widgets::unavailable(ui, &p, "Key file…", "");
                        widgets::unavailable(ui, &p, "Passphrase, if the key has one", "");
                    });
                } else {
                    ui.horizontal(|ui| {
                        widgets::field(ui, &p, &mut panel.key_path, "/path/to/key.pem", 280.0, true);
                        ui.label(widgets::secondary("Key file", &p));
                    });
                    ui.horizontal(|ui| {
                        widgets::secret_field(ui, &p, &mut panel.passphrase, false, 280.0, egui::Id::unique("ssh-passphrase"));
                        ui.label(widgets::secondary("Passphrase, if the key has one", &p));
                    });
                    ui.label(widgets::secondary("Used once to set up YAAC's own key. YAAC does not keep the file or the passphrase.", &p));
                    ui.add_space(6.0);
                    widgets::unavailable(ui, &p, "Password", "");
                }
                if let Some(e) = &error {
                    ui.add_space(4.0);
                    ui.label(RichText::new(e).color(p.failed));
                    ui.label(widgets::secondary("Nothing was changed on the server.", &p));
                }
                ui.add_space(8.0);
                ui.label(widgets::secondary(panel.setup.next_preview(), &p));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if widgets::primary(ui, &p, "Connect", Some("Enter")).clicked() || enter {
                        let credential = match panel.method {
                            SignInMethod::Password => Credential::Password(panel.password.take()),
                            SignInMethod::KeyFile => Credential::KeyFile {
                                path: panel.key_path.clone(),
                                passphrase: (!panel.passphrase.is_empty()).then(|| panel.passphrase.take()),
                            },
                        };
                        panel.show = false;
                        panel.setup.sign_in(app.ssh.as_mut(), credential);
                    }
                    if widgets::plain(ui, &p, "Cancel", Some("Esc")).clicked() || esc {
                        keep = false;
                    }
                });
            }
            Phase::Ready(rec) => {
                ui.add_space(10.0);
                let name = panel.setup.conn.name.clone();
                ui.horizontal(|ui| {
                    widgets::mark(ui, &p, Mark::Done, 18.0);
                    ui.label(RichText::new("Ready.").font(theme::bold(size::UI)));
                    let text = match rec {
                        SetupRecord::InstalledByYaac { .. } => format!("YAAC can now connect to {name} without a password. Your password was not saved."),
                        _ => format!("Password-less access to {name} was already in place. YAAC installed nothing."),
                    };
                    ui.label(text);
                });
                if let SetupRecord::InstalledByYaac { key, .. } = &rec {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        ui.label(widgets::secondary("YAAC added one line to ", &p));
                        ui.label(RichText::new("~/.ssh/authorized_keys").font(theme::mono(size::SMALL)).color(p.text2));
                        ui.label(widgets::secondary(" on the server, marked ", &p));
                        ui.label(RichText::new(&key.comment).font(theme::mono(size::SMALL)).color(p.text2));
                        ui.label(widgets::secondary(". Revoke removes exactly that line.", &p));
                    });
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if widgets::primary(ui, &p, "Done", Some("Enter")).clicked() || enter {
                        keep = false;
                    }
                    if matches!(rec, SetupRecord::InstalledByYaac { .. }) && widgets::plain(ui, &p, "Revoke", None).clicked() {
                        let id = panel.existing.clone().unwrap_or_else(|| TargetId::new(name.clone()));
                        if let SetupRecord::InstalledByYaac { key, .. } = &rec {
                            let revoke = Revoke::start(panel.setup.conn.clone(), key.clone(), now, app.ssh.as_mut());
                            app.ui.set.revoke = Some(RevokePanel { target: id, revoke });
                        }
                        keep = false;
                    }
                });
            }
            Phase::Failed { cleaned_up } => {
                if !cleaned_up && let Some(k) = panel.setup.leftover() {
                    ui.label(RichText::new("YAAC's key may still be on the server. To remove it by hand, delete this line:").color(p.needs));
                    ui.label(RichText::new(&k.line).font(theme::mono(size::SMALL)));
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if widgets::primary(ui, &p, "Try again", None).clicked() {
                        panel.setup.try_again();
                    }
                    if widgets::plain(ui, &p, "Close", Some("Esc")).clicked() || esc {
                        keep = false;
                    }
                });
            }
        }
    });
    if keep {
        app.ui.set.ssh = Some(panel);
    }
}

fn revoke_panel(ui: &mut egui::Ui, app: &mut YaacApp, actions: &mut Vec<Action>) {
    let p = app.p;
    let Some(mut panel) = app.ui.set.revoke.take() else { return };
    let mut keep = true;
    let name = panel.revoke.conn.name.clone();
    panel_frame(ui, &p, |ui| match panel.revoke.phase.clone() {
        RevokePhase::Working => {
            ui.horizontal(|ui| {
                widgets::mark(ui, &p, Mark::Running, 16.0);
                ui.label(format!("Removing YAAC's key from {name}…"));
            });
        }
        RevokePhase::Revoked => {
            ui.label(RichText::new("Revoked").font(theme::bold(size::SECTION + 1.0)));
            ui.add_space(6.0);
            let line = app
                .client
                .state
                .ssh_records
                .get(&panel.target)
                .map(|r| match r {
                    SetupRecord::RemovedByHand { date } => format!("Not set up — removed by hand on {date}"),
                    SetupRecord::Revoked { date } => format!("Not set up — YAAC's key was removed on {date}"),
                    _ => "Not set up".to_owned(),
                })
                .unwrap_or_default();
            ui.horizontal(|ui| {
                ui.label(RichText::new(&name).font(theme::bold(size::UI)));
                widgets::mark(ui, &p, Mark::Off, 15.0);
                ui.label(line);
            });
            ui.label(widgets::secondary("Only YAAC's own key was removed. Your other keys were not touched.", &p));
            ui.add_space(8.0);
            if widgets::plain(ui, &p, "Close", Some("Esc")).clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                keep = false;
            }
        }
        RevokePhase::CouldNotRevoke { .. } => {
            ui.label(RichText::new("Could not revoke").font(theme::bold(size::SECTION + 1.0)));
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                widgets::icon(ui, 16.0, |pa, c, d| icons::warning(pa, c, d, p.needs, p.panel));
                ui.label(RichText::new(format!("{name} could not be reached")).font(theme::bold(size::UI)));
                ui.label(", so YAAC's key is still on that server.");
            });
            ui.label("YAAC has kept its copy of the key so it can finish this later.");
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.label(widgets::secondary("To remove it by hand, delete this line from ", &p));
                ui.label(RichText::new("~/.ssh/authorized_keys").font(theme::mono(size::SMALL)).color(p.text2));
                ui.label(widgets::secondary(format!(" on {name}:"), &p));
            });
            let line = panel.revoke.key.line.clone();
            egui::Frame::NONE.fill(p.faint).corner_radius(5).inner_margin(Margin::same(10)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.add(egui::Label::new(RichText::new(&line).font(theme::mono(size::MONO))).wrap_mode(egui::TextWrapMode::Wrap).selectable(true));
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    if widgets::plain(ui, &p, "Copy", None).clicked() {
                        ui.ctx().copy_text(line.clone());
                    }
                });
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if widgets::primary(ui, &p, "Try again", None).clicked() {
                    panel.revoke.retry(app.ssh.as_mut());
                }
                if widgets::plain(ui, &p, "I removed it myself", None).clicked() {
                    let rec = panel.revoke.removed_by_hand();
                    actions.push(Action::SetRecord(panel.target.clone(), rec));
                }
                if widgets::plain(ui, &p, "Close", None).clicked() {
                    keep = false;
                }
            });
        }
    });
    if keep {
        app.ui.set.revoke = Some(panel);
    }
}

/// Host aliases in `~/.ssh/config` (no wildcards), to pre-fill the form.
fn read_ssh_config_hosts() -> Vec<String> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
    let Some(text) = home.and_then(|h| std::fs::read_to_string(std::path::Path::new(&h).join(".ssh").join("config")).ok()) else { return Vec::new() };
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("Host ").or_else(|| line.strip_prefix("host ")) {
            for h in rest.split_whitespace() {
                if !h.contains(['*', '?', '!']) && !out.iter().any(|x| x == h) {
                    out.push(h.to_owned());
                }
            }
        }
    }
    out
}

// ── Providers ───────────────────────────────────────────────────────────────────────────

fn providers(ui: &mut egui::Ui, app: &mut YaacApp, actions: &mut Vec<Action>) {
    let p = app.p;
    let st = &app.client.state;
    let target = app
        .ui
        .set
        .providers_target
        .clone()
        .filter(|t| st.target(t).is_some())
        .or_else(|| st.targets.iter().find(|t| t.connected()).map(|t| t.entry.id.clone()));
    let Some(target) = target else {
        title_row(ui, "Providers", |_| {});
        ui.label(widgets::secondary("No target is connected.", &p));
        return;
    };
    if st.target(&target).is_some_and(|t| t.instance_configs.is_none() && t.connected()) {
        // Asked once per target; the answer fills the socket paths and what can be edited.
        app.client.list_instances(&target);
        if let Some(t) = app.client.state.targets.iter_mut().find(|t| t.entry.id == target) {
            t.instance_configs = Some(Vec::new());
        }
    }
    let Some(v) = providers_view(&app.client.state, &target) else { return };
    title_row(ui, "Providers", |ui| {
        let r = widgets::selector(ui, &p, &v.target_name, true);
        if r.clicked() {
            app.ui.toggle_menu(Menu::SettingsTarget);
        }
        let mut open = app.ui.menu == Some(Menu::SettingsTarget);
        let names: Vec<(TargetId, String)> = app.client.state.targets.iter().map(|t| (t.entry.id.clone(), t.entry.name.to_string())).collect();
        egui::Popup::from_response(&r).kind(egui::PopupKind::Menu).open_bool(&mut open).close_behavior(egui::PopupCloseBehavior::CloseOnClick).show(|ui| {
            ui.set_min_width(160.0);
            for (id, n) in &names {
                if ui.selectable_label(id == &target, n).clicked() {
                    app.ui.set.providers_target = Some(id.clone());
                }
            }
        });
        if !open && app.ui.menu == Some(Menu::SettingsTarget) {
            app.ui.menu = None;
        }
        ui.label(widgets::secondary("Target", &p));
    });
    if v.offline {
        ui.label(widgets::secondary(format!("{} is not connected — showing what was last read.", v.target_name), &p));
    }
    ui.add_space(8.0);
    for row in &v.rows {
        instance_row(ui, app, &target, row, actions);
    }
    // Add an instance.
    widgets::heading(ui, "Add instance");
    if app.ui.set.add_kind.as_ref().is_none_or(|k| !v.kinds.contains(k)) {
        app.ui.set.add_kind = v.kinds.first().cloned();
    }
    ui.horizontal(|ui| {
        let label = app.ui.set.add_kind.as_ref().map_or_else(|| "Kind".to_owned(), |k| kind_label(&app.client.state, k));
        let r = widgets::selector(ui, &p, &label, true);
        if r.clicked() {
            app.ui.toggle_menu(Menu::InstanceKind);
        }
        let mut open = app.ui.menu == Some(Menu::InstanceKind);
        let kinds: Vec<_> = v.kinds.iter().map(|k| (k.clone(), kind_label(&app.client.state, k))).collect();
        egui::Popup::from_response(&r).kind(egui::PopupKind::Menu).open_bool(&mut open).close_behavior(egui::PopupCloseBehavior::CloseOnClick).show(|ui| {
            for (k, l) in &kinds {
                if ui.selectable_label(app.ui.set.add_kind.as_ref() == Some(k), l).clicked() {
                    app.ui.set.add_kind = Some(k.clone());
                }
            }
        });
        if !open && app.ui.menu == Some(Menu::InstanceKind) {
            app.ui.menu = None;
        }
        widgets::field(ui, &p, &mut app.ui.set.add_name, "Name (optional)", 160.0, false);
        widgets::field(ui, &p, &mut app.ui.set.add_socket, "/path/to/control.sock", 260.0, true);
        let ok = app.ui.set.add_kind.is_some() && (!app.ui.set.add_name.trim().is_empty() || !app.ui.set.add_socket.trim().is_empty());
        if ok {
            if widgets::plain(ui, &p, "Add", None).clicked()
                && let Some(k) = app.ui.set.add_kind.clone()
            {
                actions.push(Action::AddInstance(target.clone(), k, app.ui.set.add_name.clone(), app.ui.set.add_socket.clone()));
                app.ui.set.add_name.clear();
                app.ui.set.add_socket.clear();
            }
        } else {
            widgets::unavailable(ui, &p, "Add", "");
        }
    });
    ui.label(widgets::secondary("A second instance of a kind needs a name or its own socket.", &p));

    // Instances that need attention on other targets.
    let others: Vec<(String, Vec<yaac_core::settings::InstanceRow>)> = app
        .client
        .state
        .targets
        .iter()
        .filter(|t| t.entry.id != target)
        .filter_map(|t| {
            let v = providers_view(&app.client.state, &t.entry.id)?;
            let rows: Vec<_> = v.rows.into_iter().filter(|r| r.mark != Mark::Done).collect();
            (!rows.is_empty()).then(|| (t.entry.name.to_string(), rows))
        })
        .collect();
    for (name, rows) in others {
        ui.add_space(22.0);
        ui.label(widgets::secondary(format!("{name} — needs attention"), &p));
        ui.add_space(4.0);
        for r in &rows {
            ui.horizontal(|ui| {
                widgets::mark(ui, &p, r.mark, 16.0);
                ui.label(RichText::new(&r.name).font(theme::bold(size::UI)));
                widgets::rich_inline(ui, &p, &r.status, p.text, size::UI);
            });
        }
    }
}

fn instance_row(ui: &mut egui::Ui, app: &mut YaacApp, target: &TargetId, row: &yaac_core::settings::InstanceRow, actions: &mut Vec<Action>) {
    let p = app.p;
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        widgets::mark(ui, &p, row.mark, 16.0);
        ui.label(RichText::new(&row.name).font(theme::bold(size::UI + 0.5)));
        ui.add_space(4.0);
        widgets::rich_inline(ui, &p, &row.status, p.text, size::UI);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if row.editable {
                if widgets::plain(ui, &p, "Remove", None).clicked() {
                    actions.push(Action::RemoveInstance(target.clone(), row.instance.clone()));
                }
                if widgets::plain(ui, &p, "Edit", None).clicked() {
                    let name = row.name.split(" · ").nth(1).unwrap_or_default().to_owned();
                    app.ui.set.edit = Some((target.clone(), row.instance.clone(), name, row.socket.clone().unwrap_or_default()));
                }
            }
            if widgets::plain(ui, &p, "Refresh", None).on_hover_text("Re-check this instance and its models").clicked() {
                actions.push(Action::RefreshProviders(target.clone(), Some(row.instance.clone())));
            }
            ui.add_space(8.0);
            if widgets::switch(ui, &p, row.enabled).on_hover_text(if row.enabled { "Disable this instance" } else { "Enable this instance" }).clicked() {
                actions.push(Action::InstanceEnabled(target.clone(), row.instance.clone(), !row.enabled));
            }
        });
    });
    ui.horizontal_wrapped(|ui| {
        ui.add_space(24.0);
        ui.label(widgets::secondary(row.facts.join(" · "), &p));
        if let Some(s) = &row.socket {
            ui.label(widgets::secondary("·", &p));
            ui.label(RichText::new(s).font(theme::mono(size::SMALL)).color(p.text2));
        }
    });
    if let Some((t, i, name, socket)) = &mut app.ui.set.edit
        && t == target
        && i == &row.instance
    {
        let mut done = false;
        ui.horizontal(|ui| {
            ui.add_space(24.0);
            widgets::field(ui, &p, name, "Name (optional)", 160.0, false);
            widgets::field(ui, &p, socket, "/path/to/control.sock", 260.0, true);
            if widgets::plain(ui, &p, "Save", None).clicked() {
                actions.push(Action::UpdateInstance(t.clone(), i.clone(), name.clone(), socket.clone()));
                done = true;
            }
            if widgets::plain(ui, &p, "Cancel", None).clicked() {
                done = true;
            }
        });
        if done {
            app.ui.set.edit = None;
        }
    }
    ui.horizontal(|ui| {
        ui.add_space(24.0);
        if widgets::switch(ui, &p, row.outside_tools).clicked() {
            actions.push(Action::OutsideTools(target.clone(), row.instance.clone(), !row.outside_tools));
        }
        ui.label("Offer YAAC tools to sessions started outside YAAC");
        ui.label(widgets::secondary("— edits this harness's own configuration", &p));
    });
}

// ── Appearance and Advanced ─────────────────────────────────────────────────────────────

fn appearance(ui: &mut egui::Ui, app: &mut YaacApp, actions: &mut Vec<Action>) {
    let p = app.p;
    title_row(ui, "Appearance", |_| {});
    widgets::heading(ui, "Theme");
    for (choice, label) in [(ThemeChoice::System, "Follow the system"), (ThemeChoice::Dark, "Dark"), (ThemeChoice::Light, "Light")] {
        if ui.radio(app.ui.theme == choice, label).clicked() {
            actions.push(Action::Theme(choice));
        }
    }
    widgets::heading(ui, "Font size");
    ui.horizontal(|ui| {
        if widgets::plain(ui, &p, "Smaller", None).clicked() {
            app.ui.font_size = (app.ui.font_size - 0.5).max(11.0);
        }
        ui.label(format!("{:.1} pt", app.ui.font_size));
        if widgets::plain(ui, &p, "Larger", None).clicked() {
            app.ui.font_size = (app.ui.font_size + 0.5).min(18.0);
        }
        if (app.ui.font_size - size::UI).abs() > 0.01 && widgets::plain(ui, &p, "Reset", None).clicked() {
            app.ui.font_size = size::UI;
        }
    });
    ui.label(widgets::secondary("Scales the whole window; agent prose stays the largest text.", &p));
}

fn advanced(ui: &mut egui::Ui, app: &mut YaacApp, actions: &mut Vec<Action>) {
    let p = app.p;
    title_row(ui, "Advanced", |_| {});
    widgets::heading(ui, "Send key");
    use yaac_core::prefs::SendKey;
    if ui.radio(app.ui.send_key == SendKey::Enter, "Enter sends · Shift+Enter is a new line").clicked() {
        app.ui.send_key = SendKey::Enter;
    }
    if ui.radio(app.ui.send_key == SendKey::CtrlEnter, "Ctrl+Enter sends · Enter is a new line").clicked() {
        app.ui.send_key = SendKey::CtrlEnter;
    }
    widgets::heading(ui, "Hub");
    let always = app.client.state.hub.always.unwrap_or(true);
    ui.horizontal(|ui| {
        if widgets::switch(ui, &p, always).clicked() {
            actions.push(Action::SetAlways(!always));
        }
        ui.label("Start the hub at login and keep it running");
    });
    ui.label(widgets::secondary("Agents on different machines keep working together while this window is closed. Off: the hub exits a while after the last window closes and nothing is running.", &p));
    if let Some(e) = &app.client.state.hub.last_error {
        ui.label(RichText::new(e).font(theme::ui(size::SMALL)).color(p.failed));
    }
    widgets::heading(ui, "Cache");
    ui.horizontal(|ui| {
        if widgets::plain(ui, &p, "Clear cache", None).clicked() {
            actions.push(Action::ClearCache);
        }
        ui.label(widgets::secondary("Thread lists and transcripts are read again from each target. Drafts are never touched.", &p));
    });
    widgets::heading(ui, "Diagnostics");
    let target = app.ui.set.diag_target.clone().filter(|t| app.client.state.target(t).is_some()).or_else(|| app.client.state.targets.first().map(|t| t.entry.id.clone()));
    let Some(target) = target else {
        ui.label(widgets::secondary("No targets.", &p));
        return;
    };
    ui.horizontal_wrapped(|ui| {
        let names: Vec<(TargetId, String)> = app.client.state.targets.iter().map(|t| (t.entry.id.clone(), t.entry.name.to_string())).collect();
        for (id, n) in names {
            let sel = id == target;
            if ui.selectable_label(sel, n).clicked() {
                app.ui.set.diag_target = Some(id);
            }
        }
    });
    ui.add_space(6.0);
    egui::Grid::new("diag").num_columns(2).spacing(vec2(18.0, 6.0)).show(ui, |ui| {
        for (k, v) in diagnostics(&app.client.state, &target) {
            ui.label(widgets::secondary(k, &p));
            widgets::rich_inline(ui, &p, &v, p.text, size::UI);
            ui.end_row();
        }
    });
    ui.label(widgets::secondary(format!("This window: YAAC {} · hub {}", crate::BUILD, app.client.state.hub.build.clone().unwrap_or_else(|| "not attached".into())), &p));
}
