//! Settings: a full-window page with its own navigation (Targets, Providers, Appearance,
//! Window, Advanced) and "Back to threads". No thread list, no agents panel, no composer.

use egui::{self, Margin, RichText, pos2, vec2};
use yaac_core::attention::Mark;
use yaac_core::components::ComponentStatus;
use yaac_core::settings::{CHOOSE_DISTRO, DistroOption, DistroPhase, DistroPicker, LEGEND, Page, SshAction, diagnostics, distro_picker, kind_label, providers_view, targets_view};
use yaac_core::ssh_setup::{Credential, Phase, Revoke, RevokePhase, Secret, SetupRecord, SignInMethod, SshConn, SshSetup, SshSetupBackend, StepLine, StepMark, form_ready};
use yaac_protocol::attach::{ConnectionStatus, SshTransport, TargetTransport, WslTransport};
use yaac_protocol::{InstanceId, ModelId, TargetId};

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
            setup: SshSetup::new(conn, now).in_zone(now, crate::app::host_zone()),
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
    /// The Add form's harness home (P300).
    pub add_home: String,
    pub edit: Option<(TargetId, InstanceId, String, String)>,
    /// An instance's harness home being edited (P300).
    pub home_edit: Option<(TargetId, InstanceId, String)>,
    pub model_guidance_panel: Option<(TargetId, InstanceId)>,
    pub model_guidance_edit: Option<ModelGuidanceEdit>,
    pub wsl_distro: String,
    pub wsl_user: String,
    pub diag_target: Option<TargetId>,
    pub ssh_config_hosts: Option<Vec<String>>,
    /// Records the setup and revoke flows produced, to store.
    pub records: Vec<(TargetId, SetupRecord)>,
    /// Settings → Keyboard: recording a chord, and a conflict to resolve.
    pub keys: crate::keyboard::KeysUi,
}

#[derive(Debug)]
pub(crate) struct ModelGuidanceEdit {
    pub target: TargetId,
    pub instance: InstanceId,
    pub model: ModelId,
    pub specific: ModelId,
    pub text: String,
}

impl ModelGuidanceEdit {
    pub(crate) fn new(target: &TargetId, instance: &InstanceId, model: &yaac_core::settings::ModelGuidanceRow) -> Self {
        Self { target: target.clone(), instance: instance.clone(), model: model.id.clone(), specific: model.specific.clone(),
            text: model.best_use.as_ref().map_or_else(String::new, |hint| hint.summary.clone()) }
    }

    pub(crate) fn matches(&self, target: &TargetId, instance: &InstanceId, model: &yaac_core::settings::ModelGuidanceRow) -> bool {
        &self.target == target && &self.instance == instance && self.model == model.id && self.specific == model.specific
    }
}

pub(crate) fn valid_model_guidance(text: &str) -> bool {
    !text.trim().is_empty()
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
    // "Back to threads" sits in the window's title row (D104); the page itself starts below
    // it, clear of the window buttons above its right side.
    let inset = crate::titlebar::HEIGHT;
    let nav_content = egui::Rect::from_min_max(nav.min + vec2(12.0, 7.0), nav.max - vec2(12.0, 12.0));
    ui.scope_builder(egui::UiBuilder::new().max_rect(nav_content).id_salt("settings-nav"), |ui| {
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
    let content = egui::Rect::from_min_max(pos2(nav.right() + 1.0, full.top() + inset), full.max);
    ui.scope_builder(egui::UiBuilder::new().max_rect(content).id_salt("settings-content"), |ui| {
        egui::ScrollArea::vertical().id_salt(("settings-scroll", app.ui.settings_page as u8)).auto_shrink([false, false]).show(ui, |ui| {
            egui::Frame::NONE.inner_margin(Margin { left: 36, right: 36, top: 22, bottom: 30 }).show(ui, |ui| {
                ui.set_max_width(940.0f32.min(ui.available_width()));
                match app.ui.settings_page {
                    Page::Targets => targets(ui, app, actions),
                    Page::Providers => providers(ui, app, actions),
                    Page::Skills => crate::catalog_settings::show(ui, app),
                    Page::Appearance => appearance(ui, app, actions),
                    Page::Window => window(ui, app),
                    Page::Android => android(ui, app),
                    Page::Keyboard => crate::keyboard::page(ui, app),
                    Page::Terminal => terminal(ui, app),
                    Page::Approvals => {
                        ui.heading("Explain commands with");
                        let options = yaac_core::settings::explanation_choices(&app.client.state);
                        let mut models = app.client.state.hub.explanations.clone();
                        for (label, slot) in [("Main model", &mut models.main), ("Fallback model", &mut models.fallback)] {
                            let selected = options.iter().find(|(v, _)| Some(v) == slot.as_ref()).map(|(_, name)| name.clone())
                                .or_else(|| slot.as_ref().map(|v| format!("{} · {}", v.instance, v.model)));
                            egui::ComboBox::from_label(label).selected_text(selected.as_deref().unwrap_or("None")).show_ui(ui, |ui| {
                                ui.selectable_value(slot, None, "None");
                                for (v, name) in &options { ui.selectable_value(slot, Some(v.clone()), name); }
                            });
                        }
                        if models != app.client.state.hub.explanations { app.client.set_explanations(models); }
                    }
                    Page::Notifications => {
                        use yaac_protocol::attach::NotificationSound;
                        title_row(ui, "Notifications", |_| {});
                        let mut prefs = app.client.state.hub.notifications;
                        ui.checkbox(&mut prefs.enabled, "Show desktop request notifications");
                        ui.checkbox(&mut prefs.include_questions, "Include questions");
                        ui.checkbox(&mut prefs.sound, "Play a sound");
                        ui.horizontal(|ui| {
                            ui.label("Sound");
                            ui.radio_value(&mut prefs.sound_choice, NotificationSound::Reminder, "Reminder");
                            ui.radio_value(&mut prefs.sound_choice, NotificationSound::Default, "Default");
                        });
                        if prefs != app.client.state.hub.notifications { app.client.set_notifications(prefs); }
                    }
                    Page::Hubs => hubs_page(ui, app, actions),
                    Page::Advanced => advanced(ui, app, actions),
                    Page::About => about(ui, app, actions),
                }
            });
        });
    });
    let busy = app.ui.menu.is_some() || app.ui.set.ssh.is_some() || app.ui.set.revoke.is_some() || app.ui.set.keys.conflict.is_some() || app.ui.commander.is_some();
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
            ui.label(widgets::secondary(if hosts.is_empty() { "No hosts were found in ~/.ssh/config." } else { "Hosts in ~/.ssh/config · choose one to fill in the form:" }, &p));
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
    if let Some((id, enabled)) = &v.local
        && let Some(target) = app.client.state.target(id)
    {
        ui.horizontal(|ui| {
            ui.set_min_height(30.0);
            ui.label("This computer");
            let status = match target.entry.state.status {
                ConnectionStatus::Connected => "Connected",
                ConnectionStatus::Connecting => "Connecting…",
                ConnectionStatus::Disabled => "Off",
                _ => "Offline",
            };
            ui.label(widgets::secondary(status, &p));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::switch(ui, &p, *enabled).on_hover_text(if *enabled { "Disable" } else { "Enable" }).clicked() {
                    actions.push(Action::TargetEnabled(id.clone(), !enabled));
                }
            });
        });
        if let Some(detail) = &target.entry.state.detail {
            ui.label(widgets::secondary(detail, &p));
        }
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
            wsl_add_row(ui, app, actions);
            ui.label(widgets::secondary("Adding a distro again with another user opens a second session to it.", &p));
        }
    }
}

/// The Add row of the WSL section: a dropdown of the installed distros (from the hub), a
/// refresh button, the optional user, and Add. When the hub cannot list them, the distro is
/// typed instead, so the user is never stuck.
fn wsl_add_row(ui: &mut egui::Ui, app: &mut YaacApp, actions: &mut Vec<Action>) {
    let p = app.p;
    if app.client.state.wsl_distros.phase == DistroPhase::NotAsked {
        actions.push(Action::LoadDistros);
    }
    let picker = distro_picker(&app.client.state);
    let mut picked: Option<String> = None;
    ui.horizontal(|ui| {
        let chosen = match &picker {
            DistroPicker::Loading => {
                widgets::selector(ui, &p, "Loading…", false);
                None
            }
            DistroPicker::Choose { options, selected } => {
                let r = widgets::selector(ui, &p, selected.as_deref().unwrap_or(CHOOSE_DISTRO), true);
                if r.clicked() {
                    app.ui.toggle_menu(Menu::WslDistro);
                }
                let mut open = app.ui.menu == Some(Menu::WslDistro);
                widgets::menu_popup(egui::Popup::from_response(&r), &p)
                    .kind(egui::PopupKind::Menu)
                    .open_bool(&mut open)
                    .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
                    .show(|ui| {
                        // A fixed width: the rows fill what they are given.
                        ui.set_width(320.0);
                        for o in options {
                            if distro_row(ui, &p, o, selected.as_deref() == Some(o.name.as_str())).clicked() {
                                picked = Some(o.name.clone());
                            }
                        }
                    });
                if !open && app.ui.menu == Some(Menu::WslDistro) {
                    app.ui.menu = None;
                }
                selected.clone()
            }
            DistroPicker::Typed { .. } => {
                widgets::field(ui, &p, &mut app.ui.set.wsl_distro, "Distro", 160.0, false);
                Some(app.ui.set.wsl_distro.trim().to_owned()).filter(|d| !d.is_empty())
            }
        };
        if widgets::icon_button(ui, &p, 14.0, "List the installed distros again", icons::refresh).clicked() {
            actions.push(Action::LoadDistros);
        }
        ui.add_space(6.0);
        widgets::field(ui, &p, &mut app.ui.set.wsl_user, "User (optional)", 200.0, false);
        match chosen {
            Some(distro) => {
                if widgets::plain(ui, &p, "Add", None).clicked() {
                    let user = Some(app.ui.set.wsl_user.trim().to_owned()).filter(|u| !u.is_empty());
                    let name = match &user {
                        Some(u) => format!("wsl: {distro} ({u})"),
                        None => format!("wsl: {distro}"),
                    };
                    actions.push(Action::AddTarget(name, TargetTransport::Wsl(WslTransport { distro, user })));
                    app.client.choose_wsl_distro(None);
                    app.ui.set.wsl_distro.clear();
                    app.ui.set.wsl_user.clear();
                }
            }
            None => widgets::unavailable(ui, &p, "Add", if matches!(picker, DistroPicker::Typed { .. }) { "Type a distro's name" } else { "Choose a distro first" }),
        }
    });
    if let Some(name) = picked {
        app.client.choose_wsl_distro(Some(name));
    }
    if let DistroPicker::Typed { message } = &picker {
        ui.label(widgets::secondary(format!("The installed distros could not be listed ({message}). Type the distro's name."), &p));
    }
}

/// One distro in the dropdown: its name ("Ubuntu · default") and, on the right, whether it
/// runs and whether it already has a connection.
fn distro_row(ui: &mut egui::Ui, p: &Palette, o: &DistroOption, selected: bool) -> egui::Response {
    let (resp, rect) = widgets::menu_row(ui, p, selected, true);
    let font = theme::ui(size::UI);
    let g = ui.fonts_mut(|f| f.layout_no_wrap(o.label.clone(), font.clone(), p.text));
    ui.painter().galley(pos2(rect.left() + 10.0, rect.center().y - g.size().y / 2.0), g, p.text);
    let right = if o.added { format!("{} · already added", o.state) } else { o.state.to_owned() };
    let g = ui.fonts_mut(|f| f.layout_no_wrap(right, theme::ui(size::SMALL), p.text2));
    ui.painter().galley(pos2(rect.right() - 10.0 - g.size().x, rect.center().y - g.size().y / 2.0), g, p.text2);
    resp
}

fn start_revoke(app: &mut YaacApp, id: &TargetId) {
    let Some(t) = app.client.state.target(id) else { return };
    let TargetTransport::Ssh(s) = &t.entry.transport else { return };
    let Some(key) = app.client.state.ssh_records.get(id).and_then(|r| r.installed()).cloned() else { return };
    let conn = SshConn { name: t.entry.name.to_string(), host: s.host.clone(), port: s.port.unwrap_or(22), user: s.user.clone().unwrap_or_default() };
    let now = app.now();
    let revoke = Revoke::start(conn, key, now, app.ssh.as_mut()).in_zone(now, app.zone);
    app.ui.set.revoke = Some(RevokePanel { target: id.clone(), revoke });
    app.ui.set.ssh = None;
}

/// The step list: done, hollow, failed, and the current step.
fn steps(ui: &mut egui::Ui, p: &Palette, lines: &[StepLine], current: Option<&StepLine>) {
    for l in lines.iter().chain(current) {
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
        if panel.existing.is_some() {
            // The connection already exists (its row's "Set up…"): start using it.
            actions.push(Action::TargetEnabled(id.clone(), true));
        } else {
            let transport = TargetTransport::Ssh(SshTransport { host: conn.host.clone(), port: (conn.port != 22).then_some(conn.port), user: Some(conn.user.clone()) });
            actions.push(Action::AddTarget(conn.name, transport));
        }
        // SEAM: the record is window-local (keyed by the name, which the demo hub uses as id).
        actions.push(Action::SetRecord(id, rec.clone()));
        panel.recorded = true;
    }
    let title = match &panel.setup.phase {
        Phase::Failed { .. } => "Could not finish",
        _ => "Add SSH server",
    };
    panel_frame(ui, &p, |ui| {
        ui.label(RichText::new(title).font(theme::bold(size::SECTION + 1.0)));
        ui.add_space(8.0);
        let editable = matches!(panel.setup.phase, Phase::Form | Phase::Unreached(_));
        // Name and Host are the widest (a host name is often long), User medium, Port narrow.
        let SshPanel { name, user, host, port, .. } = &mut panel;
        widgets::form(
            ui,
            &p,
            vec![
                vec![
                    widgets::Field { label: "Name", text: name, hint: "devbox", width: widgets::FieldWidth::Fill, mono: false },
                    widgets::Field { label: "User", text: user, hint: "user", width: widgets::FieldWidth::Fill, mono: false },
                ],
                vec![
                    widgets::Field { label: "Host", text: host, hint: "host.example.com", width: widgets::FieldWidth::Fill, mono: false },
                    widgets::Field { label: "Port", text: port, hint: "22", width: widgets::FieldWidth::Chars(6), mono: false },
                ],
            ],
            editable,
        );
        ui.add_space(10.0);
        steps(ui, &p, &panel.setup.steps, panel.setup.current().as_ref());
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
                            panel.setup = SshSetup::new(panel.conn(), now).in_zone(now, crate::app::host_zone());
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
                        widgets::secret_field(ui, &p, &mut panel.password, "Password", panel.show, 280.0, pid);
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
                        widgets::secret_field(ui, &p, &mut panel.passphrase, "Password", false, 280.0, egui::Id::unique("ssh-passphrase"));
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
                            let revoke = Revoke::start(panel.setup.conn.clone(), key.clone(), now, app.ssh.as_mut()).in_zone(now, crate::app::host_zone());
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
                    SetupRecord::RemovedByHand { date } => format!("Not set up · removed by hand on {date}"),
                    SetupRecord::Revoked { date } => format!("Not set up · YAAC's key was removed on {date}"),
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
                ui.add_space(4.0);
                ui.spacing_mut().item_spacing.x = 0.0;
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
            // One filled button per screen: plain when the setup panel above has its own.
            let other_primary = app.ui.set.ssh.is_some();
            ui.horizontal(|ui| {
                let again = if other_primary { widgets::plain(ui, &p, "Try again", None) } else { widgets::primary(ui, &p, "Try again", None) };
                if again.clicked() {
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
        .or_else(|| {
            st.targets.iter().find(|t| t.connected())
                .or_else(|| st.targets.first())
                .map(|t| t.entry.id.clone())
        });
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
        let refresh = ui.add_enabled_ui(!v.offline, |ui| {
            widgets::plain(ui, &p, "Refresh", None).on_hover_text("Re-check all providers and models on this target")
        }).inner;
        if refresh.clicked() {
            actions.push(Action::RefreshProviders(target.clone(), None));
        }
        let r = widgets::selector(ui, &p, &v.target_name, true);
        if r.clicked() {
            app.ui.toggle_menu(Menu::SettingsTarget);
        }
        let mut open = app.ui.menu == Some(Menu::SettingsTarget);
        let names: Vec<(TargetId, String)> = app.client.state.targets.iter().map(|t| (t.entry.id.clone(), t.entry.name.to_string())).collect();
        crate::widgets::menu_popup(egui::Popup::from_response(&r), &p).kind(egui::PopupKind::Menu).open_bool(&mut open).close_behavior(egui::PopupCloseBehavior::CloseOnClick).show(|ui| {
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
        ui.label(widgets::secondary(format!("{} is not connected · showing what was last read.", v.target_name), &p));
    }
    if let Some(record) = app.client.state.target(&target) {
        let pending = app.client.session_discovery_pending(&target);
        if let Some(on) = record.session_discovery {
            ui.horizontal(|ui| {
                let toggle = ui.add_enabled_ui(record.connected() && !pending, |ui| widgets::switch(ui, &p, on)).inner;
                if toggle.clicked() {
                    actions.push(Action::SessionDiscovery(target.clone(), !on));
                }
                ui.label("Automatic session discovery");
            });
            ui.label(widgets::secondary("Scan this target for sessions started in its harnesses.", &p));
        } else if record.connected() {
            let detail = if pending { "Reading session discovery…" } else { "Automatic session discovery settings require an updated target daemon." };
            ui.label(widgets::secondary(detail, &p));
        }
    }
    ui.add_space(8.0);
    for row in &v.rows {
        instance_row(ui, app, &target, row, actions);
        model_guidance(ui, app, &target, row, v.offline, actions);
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
        crate::widgets::menu_popup(egui::Popup::from_response(&r), &p).kind(egui::PopupKind::Menu).open_bool(&mut open).close_behavior(egui::PopupCloseBehavior::CloseOnClick).show(|ui| {
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
        // The harness home (P300), for a kind whose instances report `configurableHome`.
        let home_kind = app.ui.set.add_kind.as_ref().is_some_and(|k| v.home_kinds.contains(k));
        if home_kind {
            widgets::field(ui, &p, &mut app.ui.set.add_home, "Harness home (optional)", 220.0, true);
        }
        let home = if home_kind { app.ui.set.add_home.trim().to_owned() } else { String::new() };
        let ok = app.ui.set.add_kind.is_some() && (!app.ui.set.add_name.trim().is_empty() || !app.ui.set.add_socket.trim().is_empty() || !home.is_empty());
        if ok {
            if widgets::plain(ui, &p, "Add", None).clicked()
                && let Some(k) = app.ui.set.add_kind.clone()
            {
                actions.push(Action::AddInstance(target.clone(), k, app.ui.set.add_name.clone(), app.ui.set.add_socket.clone(), home));
                app.ui.set.add_name.clear();
                app.ui.set.add_socket.clear();
                app.ui.set.add_home.clear();
            }
        } else {
            widgets::unavailable(ui, &p, "Add", "");
        }
    });
    ui.label(widgets::secondary("A second instance of a kind needs a name, its own socket, or its own harness home.", &p));

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
        ui.label(widgets::secondary(format!("{name} · needs attention"), &p));
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
            ui.allocate_ui_with_layout(vec2(250.0, 30.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.set_width(250.0);
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
            });
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
    if let Some(home) = &row.home {
        harness_home(ui, app, target, row, home, actions);
    }
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
        ui.label(widgets::secondary("· edits this harness's own configuration", &p));
    });
    if let Some(on) = row.trust_folders {
        ui.horizontal(|ui| {
            ui.add_space(24.0);
            if widgets::switch(ui, &p, on).clicked() {
                actions.push(Action::TrustFolders(target.clone(), row.instance.clone(), !on));
            }
            ui.label("Trust folders YAAC works in");
            ui.label(widgets::secondary("· marks a project's folder trusted in this harness when YAAC starts a thread there", &p));
        });
    }
}

fn model_guidance(ui: &mut egui::Ui, app: &mut YaacApp, target: &TargetId,
    row: &yaac_core::settings::InstanceRow, offline: bool, actions: &mut Vec<Action>) {
    if row.model_guidance.is_empty() { return; }
    let p = app.p;
    let scope = (target.clone(), row.instance.clone());
    let expanded = app.ui.set.model_guidance_panel.as_ref() == Some(&scope);
    ui.indent(("model-guidance", target, &row.instance), |ui| {
        if widgets::disclosure_row(ui, &p, "Model guidance", expanded, ui.scope_id().with("models")) {
            app.ui.set.model_guidance_panel = (!expanded).then_some(scope);
            app.ui.set.model_guidance_edit = None;
        }
        if !expanded { return; }
        for model in &row.model_guidance {
            ui.push_id(&model.id, |ui| {
                let dimmed = model.enabled == Some(false);
                ui.horizontal(|ui| {
                    if let Some(on) = model.enabled {
                        let toggle = ui.add_enabled_ui(!offline, |ui| widgets::switch(ui, &p, on)).inner;
                        if toggle.on_hover_text(if on { "Turn model off" } else { "Turn model on" }).clicked() {
                            actions.push(Action::ModelEnabled(target.clone(), row.instance.clone(), model.id.clone(), !on));
                        }
                    }
                    ui.label(RichText::new(&model.name).font(theme::bold(size::UI)).color(if dimmed { p.text2 } else { p.text }));
                });
                if !model.guidance_editable { return; }
                let editing = app.ui.set.model_guidance_edit.as_ref()
                    .is_some_and(|edit| edit.matches(target, &row.instance, model));
                if !editing {
                    ui.label(RichText::new(format!("Best used as: {}", model.summary())).color(if dimmed { p.text2 } else { p.text }));
                }
                ui.horizontal_wrapped(|ui| {
                    if !model.provenance().is_empty() {
                        ui.label(widgets::secondary(model.provenance(), &p));
                    }
                    if !editing && ui.add_enabled(!offline, egui::Button::new("Edit")).clicked() {
                        app.ui.set.model_guidance_edit = Some(ModelGuidanceEdit::new(target, &row.instance, model));
                    }
                    if !editing && model.custom() && ui.add_enabled(!offline, egui::Button::new("Reset to generated")).clicked() {
                        actions.push(Action::ModelBestUse(target.clone(), row.instance.clone(), model.id.clone(), model.specific.clone(), None));
                    }
                });
                if editing && let Some(edit) = &mut app.ui.set.model_guidance_edit {
                    let mut close = false;
                    ui.horizontal(|ui| {
                        widgets::field(ui, &p, &mut edit.text, "Best used as", (ui.available_width() - 150.0).clamp(120.0, 520.0), false);
                        if ui.add_enabled(!offline && valid_model_guidance(&edit.text), egui::Button::new("Save")).clicked() {
                            actions.push(Action::ModelBestUse(target.clone(), row.instance.clone(), model.id.clone(), model.specific.clone(), Some(edit.text.trim().to_owned())));
                            close = true;
                        }
                        if widgets::plain(ui, &p, "Cancel", None).clicked() { close = true; }
                    });
                    if close { app.ui.set.model_guidance_edit = None; }
                }
                ui.add_space(4.0);
            });
        }
    });
}

/// An instance's harness home (P300): the folder in use, **Change**, an inline field whose
/// placeholder is the folder in use, and the confirmation when the new folder is another store.
fn harness_home(ui: &mut egui::Ui, app: &mut YaacApp, target: &TargetId, row: &yaac_core::settings::InstanceRow, home: &yaac_core::settings::HomeField, actions: &mut Vec<Action>) {
    let p = app.p;
    let editing = app.ui.set.home_edit.as_ref().is_some_and(|(t, i, _)| t == target && i == &row.instance);
    if !editing {
        ui.horizontal(|ui| {
            ui.add_space(24.0);
            ui.label(RichText::new(home.line()).font(theme::mono(size::SMALL)).color(p.text2));
            if widgets::plain(ui, &p, "Change", None).on_hover_text("Where this harness keeps its configuration, sessions and sign-in").clicked() {
                app.ui.set.home_edit = Some((target.clone(), row.instance.clone(), home.set.clone().unwrap_or_default()));
            }
        });
    }
    if let Some((t, i, text)) = &mut app.ui.set.home_edit
        && t == target
        && i == &row.instance
    {
        let mut done = false;
        ui.horizontal(|ui| {
            ui.add_space(24.0);
            ui.label("Harness home");
            widgets::field(ui, &p, text, &home.placeholder(), 320.0, true);
            if widgets::plain(ui, &p, "Save", None).clicked() {
                actions.push(Action::InstanceHome(t.clone(), i.clone(), text.trim().to_owned()));
                done = true;
            }
            if home.set.is_some() && widgets::plain(ui, &p, "Use default", None).clicked() {
                actions.push(Action::InstanceHome(t.clone(), i.clone(), String::new()));
                done = true;
            }
            if widgets::plain(ui, &p, "Cancel", None).clicked() {
                done = true;
            }
        });
        ui.horizontal(|ui| {
            ui.add_space(24.0);
            ui.label(widgets::secondary("Empty: the harness default. Threads stay with the store they belong to.", &p));
        });
        if done {
            app.ui.set.home_edit = None;
        }
    }
    let confirm = app.client.state.home_confirm.clone().filter(|c| &c.target == target && c.instance == row.instance);
    if let Some(c) = confirm {
        ui.horizontal_wrapped(|ui| {
            ui.add_space(24.0);
            widgets::icon(ui, 16.0, |pa, col, d| crate::icons::warning(pa, col, d, p.needs, p.faint));
            ui.add(egui::Label::new(c.text()).wrap());
        });
        ui.horizontal(|ui| {
            ui.add_space(24.0);
            if widgets::plain(ui, &p, "Change home", None).clicked() {
                actions.push(Action::ConfirmInstanceHome);
            }
            if widgets::plain(ui, &p, "Keep current", None).clicked() {
                actions.push(Action::CancelInstanceHome);
            }
        });
    }
}

// ── Appearance and Advanced ─────────────────────────────────────────────────────────────

fn appearance(ui: &mut egui::Ui, app: &mut YaacApp, actions: &mut Vec<Action>) {
    let p = app.p;
    title_row(ui, "Appearance", |_| {});
    widgets::heading(ui, "Theme");
    for (choice, label) in [(ThemeChoice::System, "Follow the system"), (ThemeChoice::Dark, "Dark"), (ThemeChoice::Light, "Light"), (ThemeChoice::Custom, "Custom")] {
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
    widgets::heading(ui, "Fonts");
    if !app.fonts.catalog_ready {
        ui.label(widgets::secondary("Loading installed fonts…", &p));
    }
    use crate::fonts::{Role, SAMPLE, CODE_SAMPLE, NERD_SAMPLE};
    for (role, id, label, sample, font) in [
        (Role::Normal, "normal-font", "Normal font", SAMPLE, theme::ui(size::UI)),
        (Role::Code, "code-font", "Code font", CODE_SAMPLE, theme::mono(size::MONO)),
    ] {
        let current = role.get(&app.ui.fonts);
        let selected = app.fonts.choices.iter().find(|choice| choice.id == current);
        ui.label(label);
        let (choice, import, hover) = ui.horizontal(|ui| {
            let mut choice = current.to_owned();
            let mut hover = None;
            egui::ComboBox::from_id_salt(id).width(235.0).selected_text(selected.map_or("YAAC default", |choice| &choice.label))
                .show_ui(ui, |ui| {
                    let mut option = |ui: &mut egui::Ui, key: &str, label: &str| {
                        let response = ui.selectable_value(&mut choice, key.to_owned(), label);
                        if response.hovered() {
                            hover = Some(key.to_owned());
                            if let Some(preview) = app.fonts.preview_font(role, key, font.size) {
                                response.on_hover_ui(|ui| {
                                    ui.label(egui::RichText::new(format!("{sample}\n{NERD_SAMPLE}")).font(preview));
                                });
                            }
                        }
                    };
                    option(ui, "", "YAAC default");
                    for item in &app.fonts.choices { option(ui, &item.id, &item.label); }
                });
            (choice, ui.button("Import font…").clicked(), hover)
        }).inner;
        if choice != current {
            role.set(&mut app.ui.fonts, choice);
            ui.ctx().request_repaint();
        }
        if import { app.import_font(role); }
        if let Some(key) = hover { app.fonts.preview(role, &key, ui.ctx()); }
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.label(egui::RichText::new(format!("{sample}\n{NERD_SAMPLE}")).font(font));
        });
    }
    if let Some(error) = &app.fonts.error {
        ui.label(egui::RichText::new(error).color(p.failed));
    }

    widgets::heading(ui, "Custom themes");
    if ui.button("Import JSON…").clicked() { app.import_theme(); }
    if let Some(error) = &app.theme_import.error { ui.colored_label(p.failed,error); }
    let mut picked = None;
    ui.horizontal_wrapped(|ui| {
        for theme in app.ui.themes.entries.values() {
            ui.allocate_ui_with_layout(egui::vec2(264.0, 170.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                let selected = app.ui.theme == ThemeChoice::Custom && app.ui.themes.selected == theme.name;
                let preview = app.theme_import.card(ui, theme);
                let label = ui.selectable_label(selected, &theme.name);
                if preview.clicked() || label.clicked() {
                    picked = Some(theme.name.clone());
                }
            });
        }
    });
    if let Some(name) = picked { let _ = app.select_theme(&name); }

}

// ── Android: the Android client over Tailscale (P377) ──────────────────────────────────

fn android(ui: &mut egui::Ui, app: &mut YaacApp) {
    let p = app.p;
    title_row(ui, "Android", |_| {});
    let allowed = app.client.state.hub.tailscale_attach;
    ui.horizontal(|ui| {
        let on = allowed.unwrap_or(false);
        if allowed.is_some() {
            if widgets::switch(ui, &p, on).clicked() {
                app.client.set_tailscale_attach(!on);
            }
        } else {
            ui.add_enabled_ui(false, |ui| widgets::switch(ui, &p, false));
        }
        ui.label("Allow the Android client over Tailscale");
    });
    ui.label(widgets::secondary("The phone attaches to this computer's hub over Tailscale, only from devices signed in to the same Tailscale account. Nothing is paired or stored.", &p));
    if let Some(status) = &app.client.state.hub.tailscale_status {
        ui.label(RichText::new(status).font(theme::ui(size::SMALL)).color(p.text));
    }
}

// ── Window: the tray and the renderer ──────────────────────────────────────────────────

fn window(ui: &mut egui::Ui, app: &mut YaacApp) {
    use yaac_core::display::{Host, Renderer, in_use_line, offered};
    let p = app.p;
    title_row(ui, "Window", |_| {});
    widgets::heading(ui, "Closing the window");
    let supported = crate::tray::Shell::supported();
    ui.horizontal(|ui| {
        let on = app.ui.tray_on_close && supported;
        if supported {
            if widgets::switch(ui, &p, on).clicked() {
                app.ui.tray_on_close = !app.ui.tray_on_close;
            }
        } else {
            ui.add_enabled_ui(false, |ui| widgets::switch(ui, &p, false));
        }
        ui.label("Keep running in the tray when closed");
    });
    let note = if supported {
        "Closing the window hides it in the system tray. Click the tray icon to open it again; its menu has Quit. The hub keeps running either way."
    } else {
        "The tray icon is available on Windows. Here, closing the window closes it; the hub keeps running."
    };
    ui.label(widgets::secondary(note, &p));

    voice_language(ui, app);

    widgets::heading(ui, "Display");
    let host = Host::current();
    let choices = offered(host, crate::display::built());
    let current = app.display.file.renderer;
    ui.horizontal(|ui| {
        ui.label("Renderer");
        let r = widgets::selector(ui, &p, current.label(host), true);
        if r.clicked() {
            app.ui.toggle_menu(Menu::Renderer);
        }
        let mut open = app.ui.menu == Some(Menu::Renderer);
        let mut picked: Option<Renderer> = None;
        crate::widgets::menu_popup(egui::Popup::from_response(&r), &p).kind(egui::PopupKind::Menu).open_bool(&mut open).close_behavior(egui::PopupCloseBehavior::CloseOnClick).show(|ui| {
            for c in &choices {
                if ui.selectable_label(*c == current, c.label(host)).clicked() {
                    picked = Some(*c);
                }
            }
        });
        if !open && app.ui.menu == Some(Menu::Renderer) {
            app.ui.menu = None;
        }
        if let Some(c) = picked {
            app.display.choose(c);
        }
    });
    if app.display.restart_needed() {
        ui.label(RichText::new("Applies when YAAC starts again.").font(theme::ui(size::SMALL)).color(p.text));
    } else {
        ui.label(widgets::secondary("A change applies when YAAC starts again. If the renderer cannot start, YAAC falls back to software rendering and says so here.", &p));
    }
    ui.add_space(6.0);
    let u = &app.display.in_use;
    let line = in_use_line(u, host);
    let color = if u.fell_back.is_some() || u.software { p.text } else { p.text2 };
    ui.label(RichText::new(line).font(theme::ui(size::UI)).color(color));
    if let Some(crashed) = app.display.file.crashed {
        ui.horizontal(|ui| {
            ui.label(widgets::secondary(format!("{} is skipped until you try it again.", crashed.label(host)), &p));
            if widgets::plain(ui, &p, "Try it again", None).clicked() {
                app.display.choose(current);
            }
        });
    }
}

/// Settings → Window → Voice call language (docs/plan/voice-call.md § Language).
fn voice_language(ui: &mut egui::Ui, app: &mut YaacApp) {
    use yaac_core::voice_language::{CHOICES, SAME_AS_DEVICE, label};
    let p = app.p;
    widgets::heading(ui, "Voice");
    let same = match crate::voice_language::device() {
        Some(tag) => format!("{SAME_AS_DEVICE} ({})", label(&tag)),
        None => SAME_AS_DEVICE.to_owned(),
    };
    let current = if app.ui.voice_language.is_empty() { same.clone() } else { label(&app.ui.voice_language).to_owned() };
    ui.horizontal(|ui| {
        ui.label("Voice call language");
        egui::ComboBox::from_id_salt("voice-language").selected_text(current).show_ui(ui, |ui| {
            ui.selectable_value(&mut app.ui.voice_language, String::new(), same);
            for (tag, name) in CHOICES {
                ui.selectable_value(&mut app.ui.voice_language, (*tag).to_owned(), *name);
            }
        });
    });
    ui.label(widgets::secondary("The language voice calls speak and dictation listens for. Code identifiers stay as they are.", &p));
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
    #[cfg(feature = "hub")]
    {
        reclaim_offer(ui, app, actions);
        transfer_section(ui, app, actions);
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
}

/// The repository, where releases are published (D136).
const REPOSITORY: &str = "https://github.com/lelemm/yaac";

/// About: this YAAC's version and build (D135), the hub's, the latest release, and every
/// component checked as YAAC started.
fn about(ui: &mut egui::Ui, app: &mut YaacApp, actions: &mut Vec<Action>) {
    let p = app.p;
    title_row(ui, "About", |_| {});
    ui.add_space(6.0);
    let (version, id) = crate::BUILD.split_once('+').unwrap_or((crate::BUILD, ""));
    ui.label(RichText::new(format!("YAAC {version}")).font(theme::bold(size::TITLE)));
    let build = if id.bytes().all(|b| b.is_ascii_digit()) && !id.is_empty() {
        format!("Build {id}")
    } else {
        format!("Development build ({id})")
    };
    ui.label(widgets::secondary(format!("{build} · {}", crate::BUILD), &p));
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if widgets::link(ui, &p, "Releases", theme::ui(size::UI)).clicked() {
            crate::browser::open(&format!("{REPOSITORY}/releases"));
        }
        ui.label(widgets::secondary("·", &p));
        if widgets::link(ui, &p, "Source", theme::ui(size::UI)).clicked() {
            crate::browser::open(REPOSITORY);
        }
        let enabled = crate::troubleshoot::choice(&app.client.state).is_some();
        if ui.add_enabled(enabled || app.troubleshoot.open, egui::Button::new("Troubleshoot"))
            .on_disabled_hover_text(crate::troubleshoot::UNAVAILABLE).clicked()
        {
            app.troubleshoot.open = !app.troubleshoot.open;
        }
    });
    // Every component against this window's build (D135): checked as YAAC starts — the hub at
    // `hub/hello`, each target's yaacd when the hub connects to it.
    ui.add_space(18.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new("Components").font(theme::bold(size::SECTION)));
        if widgets::plain(ui, &p, "Refresh", None)
            .on_hover_text("Check target connections and the latest release now")
            .clicked()
        {
            actions.push(Action::RefreshComponents);
        }
    });
    ui.add_space(6.0);
    egui::Grid::new("components").num_columns(3).spacing(vec2(18.0, 6.0)).show(ui, |ui| {
        for c in yaac_core::components::components(&app.client.state, crate::BUILD) {
            ui.label(widgets::secondary(&c.name, &p));
            ui.label(c.build.as_deref().unwrap_or("—"));
            match &c.status {
                ComponentStatus::Current => ui.label(widgets::secondary("up to date", &p)),
                ComponentStatus::Outdated(why) => ui.label(RichText::new(format!("needs updating · {why}")).color(p.failed)),
                ComponentStatus::Unknown(why) => ui.label(widgets::secondary(why, &p)),
            };
            ui.end_row();
        }
        let c = yaac_core::components::release(&app.updater.check, crate::BUILD);
        ui.label(widgets::secondary(&c.name, &p));
        ui.label(c.build.as_deref().unwrap_or("—"));
        ui.vertical(|ui| {
            match &c.status {
                ComponentStatus::Current => ui.label(widgets::secondary("this YAAC is up to date", &p)),
                ComponentStatus::Outdated(why) => ui.label(RichText::new(why).color(p.failed)),
                ComponentStatus::Unknown(why) => ui.label(widgets::secondary(why, &p)),
            };
            if app.updater.refreshing {
                ui.label(widgets::secondary("Checking for a release…", &p));
            } else if let Some(checked_at) = app.updater.last_checked {
                let local = chrono::DateTime::<chrono::Local>::from(checked_at);
                ui.label(widgets::secondary(format!("Last checked {}", local.format("%Y-%m-%d %H:%M")), &p));
            }
        });
        ui.end_row();
    });
    if app.troubleshoot.open {
        troubleshoot(ui, app);
    }
}

fn troubleshoot(ui: &mut egui::Ui, app: &mut YaacApp) {
    use crate::troubleshoot::{GUIDANCE, MAX_SYMPTOM, UNAVAILABLE, choice};

    ui.add_space(18.0);
    widgets::heading(ui, "Troubleshoot");
    ui.label(GUIDANCE);
    let selection = choice(&app.client.state);
    let panel = &mut app.troubleshoot;
    let pending = panel.pending_label().is_some();
    ui.add_enabled(!pending, egui::TextEdit::multiline(&mut panel.symptom)
        .hint_text("What is happening?")
        .desired_width(f32::INFINITY)
        .desired_rows(4)
        .char_limit(MAX_SYMPTOM));
    if ui.add_enabled(selection.is_some() && !pending && !panel.symptom.trim().is_empty(),
        egui::Button::new("Diagnose")).clicked()
    {
        panel.start(&mut app.client, "egui", app.host_windows);
    }
    if let Some(label) = panel.pending_label() {
        ui.label(format!("Diagnosing with {label}…"));
    } else if let Some(error) = &panel.error {
        ui.colored_label(app.p.failed, error);
    } else {
        ui.label(selection.as_ref().map_or(UNAVAILABLE, |s| s.label.as_str()));
    }
    if !panel.report.is_empty() {
        ui.add_space(8.0);
        ui.label("Developer handoff");
        ui.add(egui::TextEdit::multiline(&mut panel.report)
            .desired_width(f32::INFINITY)
            .desired_rows(14));
        if ui.button("Copy report").clicked() {
            ui.ctx().copy_text(panel.report.clone());
        }
    }
}

/// Settings → Terminal (P339, docs/13-terminal.md#clipboard-and-selection).
fn terminal(ui: &mut egui::Ui, app: &mut YaacApp) {
    let p = app.p;
    widgets::heading(ui, "Clipboard");
    let rows: [(&str, &str, fn(&mut yaac_core::prefs::TerminalPrefs) -> &mut bool); 3] = [
        ("Copy on select", "Selecting text in a terminal copies it at once.", |t| &mut t.copy_on_select),
        ("Ctrl+V goes to the shell", "Ctrl+V reaches the shell (bash's literal next); paste with Ctrl+Shift+V or right-click.", |t| &mut t.ctrl_v_to_shell),
        ("Ask before pasting several lines", "Without bracketed paste each pasted line runs as it arrives; ask first.", |t| &mut t.confirm_multiline_paste),
    ];
    for (label, note, field) in rows {
        ui.horizontal(|ui| {
            let on = *field(&mut app.ui.terminal);
            if widgets::switch(ui, &p, on).clicked() {
                *field(&mut app.ui.terminal) = !on;
            }
            ui.label(label);
        });
        ui.label(widgets::secondary(note, &p));
        ui.add_space(10.0);
    }
}

// ── Hubs (P412) and moving the hub (docs/15-hub-transfer.md) ────────────────────────────

/// **Start the hub here again**, with its confirmation, when the hub moved away and does not
/// answer.
#[cfg(feature = "hub")]
pub(crate) fn reclaim_offer(ui: &mut egui::Ui, app: &mut YaacApp, actions: &mut Vec<Action>) {
    let p = app.p;
    let Some(line) = app.reclaim_line() else {
        app.ui.reclaim_confirm = false;
        return;
    };
    ui.add_space(6.0);
    ui.label(RichText::new(line).font(theme::ui(size::SMALL)).color(p.needs));
    if app.ui.reclaim_confirm {
        ui.label(widgets::secondary(app.reclaim_confirm_text(), &p));
        ui.horizontal(|ui| {
            if widgets::primary(ui, &p, "Start here", None).clicked() {
                app.ui.reclaim_confirm = false;
                actions.push(Action::ReclaimHub);
            }
            if widgets::plain(ui, &p, "Cancel", None).clicked() {
                app.ui.reclaim_confirm = false;
            }
        });
    } else if widgets::primary(ui, &p, "Start the hub here again", None).clicked() {
        app.ui.reclaim_confirm = true;
    }
}

/// Settings → Advanced → Hub → Transfer hub to….
#[cfg(feature = "hub")]
fn transfer_section(ui: &mut egui::Ui, app: &mut YaacApp, actions: &mut Vec<Action>) {
    let p = app.p;
    let view = app.transfer_view();
    ui.add_space(10.0);
    ui.label(RichText::new("Transfer hub to…").font(theme::bold(size::UI)));
    ui.label(widgets::secondary("Move the hub, with its threads, drafts and messages, to a Linux machine on your Tailscale network. Windows and phones follow it there.", &p));
    match &view.choices {
        Err(line) => {
            ui.label(widgets::secondary(line.as_str(), &p));
        }
        Ok(choices) => {
            let picked = view.picked.as_ref().and_then(|t| choices.iter().find(|c| &c.target == t)).map_or("Choose a machine", |c| c.label.as_str());
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("transfer-target").selected_text(picked).width(260.0).show_ui(ui, |ui| {
                    for c in choices {
                        let text = match &c.reason {
                            Some(why) if !c.eligible => format!("{} — {why}", c.label),
                            _ => c.label.clone(),
                        };
                        let r = ui.add_enabled(c.eligible, egui::Button::selectable(view.picked.as_ref() == Some(&c.target), text));
                        if r.clicked() {
                            actions.push(Action::TransferTarget(c.target.clone()));
                            app.ui.transfer_confirm = false;
                        }
                    }
                });
                if ui.add_enabled_ui(view.can_start && !app.ui.transfer_confirm, |ui| widgets::primary(ui, &p, "Transfer hub…", None)).inner.clicked() {
                    app.ui.transfer_confirm = true;
                }
                if widgets::plain(ui, &p, "Refresh", None).clicked() {
                    actions.push(Action::TransferCandidates);
                }
            });
        }
    }
    if app.ui.transfer_confirm {
        match (&view.confirm, view.can_start) {
            (Some((title, text)), true) => {
                egui::Frame::NONE.fill(p.faint).corner_radius(6.0).inner_margin(Margin::same(12)).show(ui, |ui| {
                    ui.label(RichText::new(title).font(theme::bold(size::UI)));
                    ui.label(widgets::secondary(text.as_str(), &p));
                    ui.horizontal(|ui| {
                        if widgets::primary(ui, &p, "Transfer", None).clicked() {
                            app.ui.transfer_confirm = false;
                            actions.push(Action::TransferStart);
                        }
                        if widgets::plain(ui, &p, "Cancel", None).clicked() {
                            app.ui.transfer_confirm = false;
                        }
                    });
                });
            }
            _ => app.ui.transfer_confirm = false,
        }
    }
    if let Some(progress) = &view.progress {
        ui.add_space(4.0);
        ui.label(RichText::new(&progress.label).font(theme::ui(size::SMALL)).color(if progress.failed { p.failed } else { p.text }));
        if progress.running {
            match progress.percent {
                Some(pct) => { ui.add(egui::ProgressBar::new((pct / 100.0) as f32).desired_width(320.0).show_percentage()); }
                None => { ui.add(egui::Spinner::new().size(14.0)); }
            }
        }
        if let Some(m) = &progress.message {
            ui.label(RichText::new(m).font(theme::ui(size::SMALL)).color(if progress.failed { p.failed } else { p.text2 }));
        }
    }
}

/// Settings → Hubs: the saved hubs, switch, retry, remove, add.
fn hubs_page(ui: &mut egui::Ui, app: &mut YaacApp, actions: &mut Vec<Action>) {
    let p = app.p;
    title_row(ui, "Hubs", |_| {});
    #[cfg(feature = "hub")]
    if app.desk_hubs.is_some() {
        ui.label(widgets::secondary("This window is attached to one hub at a time. Switching detaches from the current hub; its turns keep running.", &p));
        ui.add_space(8.0);
        for h in app.hub_rows() {
            widgets::faint_frame(&p).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    hub_badge(ui, &p, &h.initial, h.selected);
                    ui.vertical(|ui| {
                        let name = if h.selected { format!("{}  ✓", h.name) } else { h.name.clone() };
                        ui.label(RichText::new(name).font(theme::bold(size::UI)));
                        ui.label(widgets::secondary(format!("{} · {}", h.address, h.status), &p));
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if app.ui.hub_remove_confirm == Some(h.index) {
                            if widgets::plain(ui, &p, "Cancel", None).clicked() {
                                app.ui.hub_remove_confirm = None;
                            }
                            if widgets::primary(ui, &p, "Remove", None).clicked() {
                                app.ui.hub_remove_confirm = None;
                                actions.push(Action::RemoveHub(h.index));
                            }
                            ui.label(widgets::secondary("Forget this hub? The hub itself is not touched.", &p));
                        } else {
                            let remove = ui.add_enabled_ui(h.removable, |ui| widgets::plain(ui, &p, "Remove", None)).inner;
                            if remove.on_disabled_hover_text("Switch to another hub first.").clicked() {
                                app.ui.hub_remove_confirm = Some(h.index);
                            }
                            if h.retry && widgets::plain(ui, &p, "Retry", None).clicked() {
                                actions.push(Action::RetryHub);
                            }
                            if !h.selected && widgets::primary(ui, &p, "Switch to this hub", None).clicked() {
                                actions.push(Action::SelectHub(h.index));
                            }
                        }
                    });
                });
            });
            ui.add_space(6.0);
        }
        reclaim_offer(ui, app, actions);
        widgets::heading(ui, "Add a hub");
        ui.label(widgets::secondary("A hub on another computer, by its Tailscale name or address. That hub must allow clients over Tailscale (Settings → Android on that computer).", &p));
        let mut add = false;
        ui.horizontal(|ui| {
            let (address, label) = &mut app.ui.hub_add;
            let r = widgets::field(ui, &p, address, "desk or desk.tailnet.ts.net:47380", 280.0, false);
            add |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            widgets::field(ui, &p, label, "Label (optional)", 180.0, false);
            add |= widgets::primary(ui, &p, "Add hub", None).clicked();
        });
        if add && !app.ui.hub_add.0.trim().is_empty() {
            let (address, label) = std::mem::take(&mut app.ui.hub_add);
            actions.push(Action::AddHub(address, label));
        }
        if let Some(e) = app.hub_add_error() {
            ui.label(RichText::new(e).font(theme::ui(size::SMALL)).color(p.failed));
        }
        return;
    }
    let _ = actions;
    ui.label(widgets::secondary("Hubs are kept by the app on this device.", &p));
}

/// The small accent square with a hub's initial (the badge left of the YAAC mark).
pub(crate) fn hub_badge(ui: &mut egui::Ui, p: &crate::theme::Palette, initial: &str, filled: bool) -> egui::Response {
    let font = theme::bold(if initial.chars().count() > 1 { 9.5 } else { 11.0 });
    let (rect, resp) = ui.allocate_exact_size(vec2(20.0, 20.0), egui::Sense::click());
    if filled {
        ui.painter().rect_filled(rect, 4.0, p.primary);
        ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, initial, font, p.on_primary);
    } else {
        ui.painter().rect_stroke(rect, 4.0, egui::Stroke::new(1.0, p.text2), egui::StrokeKind::Inside);
        ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, initial, font, p.text2);
    }
    resp
}
