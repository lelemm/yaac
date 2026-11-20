//! The one shell: sidebar | thread (header, transcript, composer, control row) | agents.

use eframe::egui::{self, Align2, Margin, vec2};
use yaac_core::agents::{PanelMode, agents_panel};
use yaac_core::attention::AttentionMap;
use yaac_core::state::Selection;
use yaac_protocol::attach::{ConnectionStatus, TargetTransport};

use crate::app::{Action, YaacApp};
use crate::theme::{self, size};
use crate::widgets;

pub(crate) fn show(ui: &mut egui::Ui, app: &mut YaacApp, map: &AttentionMap, now: i64, actions: &mut Vec<Action>, _frame: &mut eframe::Frame) {
    let p = app.p;
    // Fixed regions, split by hand so the layout is exact: sidebar | thread | docked panel.
    let full = ui.max_rect();
    ui.painter().rect_filled(full, 0.0, p.bg);
    let selected = app.client.state.selected_thread().cloned();
    let panel = selected.as_ref().and_then(|k| agents_panel(&app.client.state, map, k));
    let docked = panel.is_some() && app.ui.panel == PanelMode::Docked;
    let side = egui::Rect::from_min_max(full.min, egui::pos2(full.left() + 280.0, full.bottom()));
    let right = if docked { egui::Rect::from_min_max(egui::pos2(full.right() - 300.0, full.top()), full.max) } else { egui::Rect::NOTHING };
    let center = egui::Rect::from_min_max(egui::pos2(side.right(), full.top()), egui::pos2(if docked { right.left() } else { full.right() }, full.bottom()));
    let hair = egui::Stroke::new(1.0, p.border);
    ui.painter().rect_filled(side, 0.0, p.panel);
    ui.painter().vline(side.right(), full.y_range(), hair);
    ui.scope_builder(egui::UiBuilder::new().max_rect(side.shrink2(vec2(12.0, 10.0))).id_salt("sidebar"), |ui| {
        ui.set_clip_rect(side);
        crate::sidebar::show(ui, app, map, now, actions);
    });
    if docked && let Some(panel) = &panel {
        ui.painter().rect_filled(right, 0.0, p.panel);
        ui.painter().vline(right.left(), full.y_range(), hair);
        ui.scope_builder(egui::UiBuilder::new().max_rect(right.shrink2(vec2(14.0, 12.0))).id_salt("agents-docked"), |ui| {
            ui.set_clip_rect(right);
            crate::agents_ui::docked(ui, &p, panel, actions);
        });
    }
    ui.scope_builder(egui::UiBuilder::new().max_rect(center).id_salt("center"), |ui| {
        ui.set_clip_rect(center);
        match app.client.state.selection.clone() {
            Selection::Thread(key) => crate::thread::show(ui, app, map, &key, now, actions),
            Selection::NewThread(id) => crate::composer::draft_view(ui, app, id, actions),
            _ => {
                ui.centered_and_justified(|ui| {
                    ui.label(widgets::secondary("Choose a thread, or start one with New (Ctrl+N).", &p));
                });
            }
        }
    });
    if let Some(panel) = &panel
        && !docked
    {
        crate::agents_ui::floating(ui.ctx(), app, center, panel, actions);
    }

    if app.ui.needs_popup {
        needs_popup(ui.ctx(), app, map, actions);
    }
    wsl_notification(ui.ctx(), app, actions);
    rename_dialog(ui.ctx(), app, actions);
    create_project_dialog(ui.ctx(), app, actions);
}

/// Create project (screen 29): machine (read-only), folder, name; one filled button.
fn create_project_dialog(ctx: &egui::Context, app: &mut YaacApp, actions: &mut Vec<Action>) {
    let p = app.p;
    let Some(mut cp) = app.ui.create_project.take() else { return };
    let machine = app.client.state.target_name(&cp.target);
    let moving = app
        .client
        .state
        .threads
        .iter()
        .filter(|(k, r)| k.target == cp.target && yaac_core::projects::contains(&cp.folder, r.summary.cwd.as_str()) && app.client.state.project_of(k).is_none())
        .count();
    let mut keep = true;
    // The scrim dims the shell behind the dialog.
    egui::Area::new(egui::Id::unique("scrim")).order(egui::Order::Middle).fixed_pos(egui::pos2(0.0, 0.0)).interactable(true).show(ctx, |ui| {
        let r = ctx.content_rect();
        ui.painter().rect_filled(r, 0.0, p.scrim);
        ui.allocate_rect(r, egui::Sense::click());
    });
    egui::Window::new("Create project")
        .collapsible(false)
        .resizable(false)
        .order(egui::Order::Foreground)
        .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
        .frame(egui::Frame::popup(&ctx.global_style()).fill(p.raised).inner_margin(Margin::same(18)))
        .show(ctx, |ui| {
            ui.set_width(420.0);
            let field = |ui: &mut egui::Ui, label: &str| {
                ui.add_space(6.0);
                ui.label(widgets::secondary(label, &p));
            };
            field(ui, "Machine");
            ui.label(&machine);
            field(ui, "Folder");
            ui.add(
                egui::TextEdit::singleline(&mut cp.folder)
                    .font(theme::mono(size::MONO))
                    .frame(egui::Frame::NONE.fill(p.faint).corner_radius(5).inner_margin(Margin::symmetric(8, 5)))
                    .desired_width(f32::INFINITY),
            );
            if let Some(root) = cp.repo_root.clone()
                && root != cp.folder
            {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    if widgets::link(ui, &p, "Use the repository root:", theme::ui(size::SMALL)).clicked() {
                        cp.folder = root.clone();
                    }
                    ui.label(egui::RichText::new(&root).font(theme::mono(size::SMALL)).color(p.text2));
                });
            }
            field(ui, "Name");
            ui.add(
                egui::TextEdit::singleline(&mut cp.name)
                    .frame(egui::Frame::NONE.fill(p.faint).corner_radius(5).inner_margin(Margin::symmetric(8, 5)))
                    .desired_width(f32::INFINITY),
            );
            ui.add_space(8.0);
            let noun = if moving == 1 { "thread" } else { "threads" };
            ui.label(widgets::secondary(format!("{moving} {noun} in this folder will move into the project."), &p));
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let ok = !cp.name.trim().is_empty() && !cp.folder.trim().is_empty();
                let create = widgets::primary(ui, &p, "Create project", Some("⏎"));
                if ok && (create.clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter))) {
                    actions.push(Action::CreateProject(cp.clone()));
                    keep = false;
                }
                if widgets::plain(ui, &p, "Cancel", Some("Esc")).clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    keep = false;
                }
            });
        });
    if keep {
        app.ui.create_project = Some(cp);
    }
}

/// Rename (from a row's context menu): `thread/rename`, under `threadRename`.
fn rename_dialog(ctx: &egui::Context, app: &mut YaacApp, actions: &mut Vec<Action>) {
    let p = app.p;
    let Some((key, mut title)) = app.ui.rename.take() else { return };
    let mut keep = true;
    egui::Window::new("Rename thread")
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
        .frame(egui::Frame::popup(&ctx.global_style()).fill(p.raised).inner_margin(Margin::same(16)))
        .show(ctx, |ui| {
            let te = egui::TextEdit::singleline(&mut title)
                .frame(egui::Frame::NONE.fill(p.faint).corner_radius(5).inner_margin(Margin::symmetric(8, 5)))
                .desired_width(320.0);
            let r = ui.add(te);
            r.request_focus();
            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if widgets::primary(ui, &p, "Rename", Some("⏎")).clicked() || enter {
                    actions.push(Action::Rename(key.clone(), title.clone()));
                    keep = false;
                }
                if widgets::plain(ui, &p, "Cancel", Some("Esc")).clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    keep = false;
                }
            });
        });
    if keep {
        app.ui.rename = Some((key, title));
    }
}

/// Ctrl+J: the Needs-you list from anywhere. ↑ ↓ to move, ⏎ to open (and focus the card).
fn needs_popup(ctx: &egui::Context, app: &mut YaacApp, map: &AttentionMap, actions: &mut Vec<Action>) {
    let p = app.p;
    let list = yaac_core::sidebar::needs_you(&app.client.state, map);
    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown)) {
        app.ui.needs_sel = (app.ui.needs_sel + 1).min(list.len().saturating_sub(1));
    }
    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp)) {
        app.ui.needs_sel = app.ui.needs_sel.saturating_sub(1);
    }
    let enter = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
    egui::Area::new(egui::Id::unique("needs-popup")).order(egui::Order::Foreground).anchor(Align2::CENTER_TOP, vec2(0.0, 80.0)).show(ctx, |ui| {
        egui::Frame::popup(ui.style()).fill(p.raised).inner_margin(Margin::same(10)).show(ui, |ui| {
            ui.set_width(380.0);
            ui.label(egui::RichText::new(format!("Needs you · {}", list.len())).font(theme::bold(size::UI)));
            ui.add_space(6.0);
            if list.is_empty() {
                ui.label(widgets::secondary("Nothing is waiting for you.", &p));
            }
            for (i, e) in list.iter().enumerate() {
                let sel = i == app.ui.needs_sel;
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), egui::Sense::click());
                if sel || resp.hovered() {
                    ui.painter().rect_filled(rect, 5.0, p.selected);
                }
                crate::icons::mark(ui.painter(), egui::pos2(rect.left() + 14.0, rect.center().y), 14.0, yaac_core::attention::Mark::NeedsYou, &p);
                let g = ui.fonts_mut(|f| f.layout_no_wrap(e.title.clone(), theme::ui(size::UI), p.text));
                ui.painter().galley(egui::pos2(rect.left() + 30.0, rect.center().y - g.size().y / 2.0), g, p.text);
                let right = format!("{} · {}{}", e.attention.word(), e.target_name, if e.muted { " · muted" } else { "" });
                let g = ui.fonts_mut(|f| f.layout_no_wrap(right, theme::ui(size::SMALL), p.text2));
                ui.painter().galley(egui::pos2(rect.right() - 8.0 - g.size().x, rect.center().y - g.size().y / 2.0), g, p.text2);
                if resp.clicked() || (sel && enter) {
                    actions.push(Action::OpenRequest(e.key.clone()));
                    app.ui.needs_popup = false;
                }
            }
            ui.add_space(6.0);
            ui.label(widgets::secondary("↑ ↓ to move · ⏎ to open · Ctrl+J", &p));
        });
    });
}

/// A WSL distro that stopped on its own raises a notification; YAAC does not start it.
fn wsl_notification(ctx: &egui::Context, app: &mut YaacApp, actions: &mut Vec<Action>) {
    let p = app.p;
    let down: Vec<_> = app
        .client
        .state
        .targets
        .iter()
        .filter(|t| {
            matches!(t.entry.transport, TargetTransport::Wsl(_))
                && t.entry.enabled
                && t.entry.state.status == ConnectionStatus::Disabled
                && !app.ui.dismissed.contains(&t.entry.id)
        })
        .map(|t| {
            let distro = match &t.entry.transport {
                TargetTransport::Wsl(w) => w.distro.clone(),
                _ => String::new(),
            };
            (t.entry.id.clone(), distro)
        })
        .collect();
    for (id, distro) in down {
        egui::Area::new(egui::Id::unique(("wsl", id.as_str()))).order(egui::Order::Foreground).anchor(Align2::RIGHT_BOTTOM, vec2(-16.0, -16.0)).show(ctx, |ui| {
            egui::Frame::popup(ui.style()).fill(p.raised).inner_margin(Margin::same(14)).show(ui, |ui| {
                ui.set_width(300.0);
                ui.horizontal(|ui| {
                    widgets::icon(ui, 16.0, |pa, c, d| crate::icons::warning(pa, c, d, p.needs, p.raised));
                    ui.label(format!("WSL distro {distro} is not running."));
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if widgets::plain(ui, &p, "Start it", None).clicked() {
                        actions.push(Action::Retry(id.clone()));
                        app.ui.dismissed.insert(id.clone());
                    }
                    if widgets::plain(ui, &p, "Disable its threads", None).clicked() {
                        app.ui.dismissed.insert(id.clone());
                    }
                });
            });
        });
    }
}
