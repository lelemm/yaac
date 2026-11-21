//! The one shell: sidebar | thread (header, transcript, composer, control row) | agents.

use eframe::egui::{self, Align2, Margin, vec2};
use yaac_core::agents::{PanelMode, agents_panel};
use yaac_core::attention::AttentionMap;
use yaac_core::state::Selection;
use yaac_protocol::attach::{ConnectionStatus, TargetTransport};

use crate::app::{Action, YaacApp};
use crate::theme::{self, size};
use crate::widgets;

/// Below this width (in points) the sidebar folds to the strip and a docked agents panel
/// folds back to the pill (screen 21).
pub(crate) const NARROW: f32 = 1000.0;

pub(crate) fn show(ui: &mut egui::Ui, app: &mut YaacApp, map: &AttentionMap, now: i64, actions: &mut Vec<Action>, _frame: &mut eframe::Frame) {
    let p = app.p;
    // Fixed regions, split by hand so the layout is exact: sidebar | thread | docked panel.
    // The screenshot runner can pin the layout to a smaller window (screen 21) when the
    // display cannot resize the real one.
    let full = match app.ui.layout_size {
        Some(sz) => egui::Rect::from_min_size(ui.max_rect().min, sz),
        None => ui.max_rect(),
    };
    ui.painter().rect_filled(full, 0.0, p.bg);
    let narrow = full.width() < NARROW;
    let selected = app.client.state.selected_thread().cloned();
    // No related agent known in the index: no panel (not an empty pill).
    let panel = selected.as_ref().and_then(|k| agents_panel(&app.client.state, map, k)).filter(|p| p.all().next().is_some());
    let docked = panel.is_some() && app.ui.panel == PanelMode::Docked && !narrow;
    // The sidebar's own width, or the strip (collapsed, or a window too narrow for it).
    let side_w = app.ui.sidebar.shown(narrow);
    let strip = side_w <= yaac_core::strip::STRIP_WIDTH;
    let side = egui::Rect::from_min_max(full.min, egui::pos2(full.left() + side_w, full.bottom()));
    let right = if docked { egui::Rect::from_min_max(egui::pos2(full.right() - app.ui.agents_width.min(full.width() * 0.4), full.top()), full.max) } else { egui::Rect::NOTHING };
    let center = egui::Rect::from_min_max(egui::pos2(side.right(), full.top()), egui::pos2(if docked { right.left() } else { full.right() }, full.bottom()));
    let hair = egui::Stroke::new(1.0, p.border);
    ui.painter().rect_filled(side, 0.0, p.panel);
    // The window's top row is its title bar (D104): the sidebar's and the thread's first rows
    // sit in it. Only the docked agents panel starts below it (the window buttons are above).
    let below = |r: egui::Rect| egui::Rect::from_min_max(egui::pos2(r.left(), r.top() + crate::titlebar::HEIGHT), r.max);
    let mut badge = None;
    app.ui.threads_overlay = false;
    if strip {
        badge = Some(crate::strip::show(ui, app, map, now, side, actions));
    } else {
        ui.scope_builder(egui::UiBuilder::new().max_rect(egui::Rect::from_min_max(side.min + vec2(12.0, 7.0), side.max - vec2(12.0, 10.0))).id_salt("sidebar"), |ui| {
            ui.set_clip_rect(side);
            crate::sidebar::show(ui, app, map, now, actions);
        });
    }
    if docked && let Some(panel) = &panel {
        ui.painter().rect_filled(right, 0.0, p.panel);
        ui.scope_builder(egui::UiBuilder::new().max_rect(below(right).shrink2(vec2(14.0, 12.0))).id_salt("agents-docked"), |ui| {
            ui.set_clip_rect(right);
            crate::agents_ui::docked(ui, &p, panel, actions);
        });
    }
    ui.scope_builder(egui::UiBuilder::new().max_rect(center).id_salt("center"), |ui| {
        ui.set_clip_rect(center);
        match app.client.state.selection.clone() {
            Selection::Thread(key) => crate::thread::show(ui, app, map, &key, now, actions),
            Selection::NewThread(id) => crate::composer::draft_view(ui, app, id, actions),
            _ if yaac_core::first_run::is_first_run(&app.client.state) => crate::first_run::show(ui, app, actions),
            _ => {
                let text = match &app.client.state.hub.last_error {
                    Some(e) if !app.client.state.hub.attached => e.clone(),
                    _ if !app.client.state.hub.attached => "Attaching to the hub…".to_owned(),
                    _ => "Choose a thread, or start one with New (Ctrl+N).".to_owned(),
                };
                ui.centered_and_justified(|ui| {
                    ui.label(widgets::secondary(text, &p));
                });
            }
        }
    });
    drop_target(ui, app, below(center), actions);
    // The hairlines, painted after the thread view (whose panels fill their own background)
    // and snapped to one pixel column, so each is one unbroken line the full height.
    let snap = |x: f32| ui.painter().round_to_pixel_center(x);
    ui.painter().vline(snap(side.right()), full.y_range(), hair);
    if docked {
        ui.painter().vline(snap(right.left()), full.y_range(), hair);
        agents_edge(ui, app, right);
    }
    if !narrow {
        resize_edge(ui, app, side);
    }
    if let Some(panel) = &panel
        && !docked
    {
        crate::agents_ui::floating(ui.ctx(), app, center, panel, actions);
    }

    if app.ui.needs_popup {
        needs_popup(ui.ctx(), app, map, badge, actions);
    }
    wsl_notification(ui.ctx(), app, actions);
    rename_dialog(ui.ctx(), app, actions);
    create_project_dialog(ui.ctx(), app, actions);
    crate::dialogs::fork(ui.ctx(), app, actions);
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
    crate::dialogs::scrim(ctx, &p, "create-project");
    egui::Window::new("Create project")
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .order(egui::Order::Foreground)
        .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
        .frame(egui::Frame::popup(&ctx.global_style()).fill(p.raised).inner_margin(Margin::same(18)))
        .show(ctx, |ui| {
            ui.set_width(420.0);
            ui.label(egui::RichText::new("Create project").font(theme::bold(size::SECTION + 1.0)));
            let field = |ui: &mut egui::Ui, label: &str| {
                ui.add_space(6.0);
                ui.label(widgets::secondary(label, &p));
            };
            field(ui, "Machine");
            ui.label(&machine);
            field(ui, "Folder");
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut cp.folder)
                        .font(theme::mono(size::MONO))
                        .frame(egui::Frame::NONE.fill(p.faint).corner_radius(5).inner_margin(Margin::symmetric(8, 5)))
                        .desired_width(ui.available_width() - 90.0),
                );
                if widgets::plain(ui, &p, "Browse…", None).clicked() {
                    let start = cp.folder.trim().trim_end_matches('/').to_owned();
                    actions.push(Action::FsList(cp.target.clone(), start.clone()));
                    cp.browse = if cp.browse.is_some() { None } else { Some(start) };
                }
            });
            if cp.browse.is_some() {
                folder_browser(ui, app, &mut cp, actions);
            }
            ui.label(widgets::secondary("Prefilled from the thread's folder. Shorten it if the project is a parent folder.", &p));
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
                let create = widgets::primary(ui, &p, "Create project", Some("Enter"));
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

/// The folder browser in the Create project dialog, backed by the target's `fs/list`.
fn folder_browser(ui: &mut egui::Ui, app: &YaacApp, cp: &mut crate::app::CreateProject, actions: &mut Vec<Action>) {
    use yaac_core::state::FsListingState;
    let p = app.p;
    let Some(path) = cp.browse.clone() else { return };
    egui::Frame::NONE.fill(p.faint).corner_radius(5).inner_margin(Margin::same(8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        match app.client.state.fs_listings.get(&(cp.target.clone(), path.clone())) {
            None | Some(FsListingState::Loading) => {
                ui.label(widgets::secondary("Reading the folder…", &p));
            }
            Some(FsListingState::Failed(e)) => {
                ui.label(egui::RichText::new(e).color(p.failed));
            }
            Some(FsListingState::Ready(l)) => {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(l.path.as_str()).font(theme::mono(size::SMALL)));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if widgets::link(ui, &p, "Use this folder", theme::ui(size::SMALL)).clicked() {
                            cp.folder = format!("{}/", l.path.as_str().trim_end_matches('/'));
                            cp.name = yaac_core::projects::suggested_name(l.path.as_str());
                            cp.browse = None;
                        }
                    });
                });
                egui::ScrollArea::vertical().max_height(160.0).show(ui, |ui| {
                    let mut go: Option<String> = None;
                    if let Some(parent) = &l.parent
                        && ui.selectable_label(false, "Parent folder").clicked()
                    {
                        go = Some(parent.to_string());
                    }
                    for e in l.entries.iter().filter(|e| e.kind == yaac_protocol::FsEntryKind::Directory) {
                        if ui.selectable_label(false, &e.name).clicked() {
                            go = Some(e.path.to_string());
                        }
                    }
                    if l.truncated {
                        ui.label(widgets::secondary("Only the first entries are shown.", &p));
                    }
                    if let Some(g) = go {
                        actions.push(Action::FsList(cp.target.clone(), g.clone()));
                        cp.browse = Some(g);
                    }
                });
            }
        }
    });
}

/// Rename (from a row's context menu): `thread/rename`, under `threadRename`.
fn rename_dialog(ctx: &egui::Context, app: &mut YaacApp, actions: &mut Vec<Action>) {
    let p = app.p;
    let Some((key, mut title)) = app.ui.rename.take() else { return };
    let mut keep = true;
    egui::Window::new("Rename thread")
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
        .frame(egui::Frame::popup(&ctx.global_style()).fill(p.raised).inner_margin(Margin::same(16)))
        .show(ctx, |ui| {
            ui.label(egui::RichText::new("Rename thread").font(theme::bold(size::SECTION + 1.0)));
            ui.add_space(6.0);
            let te = egui::TextEdit::singleline(&mut title)
                .frame(egui::Frame::NONE.fill(p.faint).corner_radius(5).inner_margin(Margin::symmetric(8, 5)))
                .desired_width(320.0);
            let r = ui.add(te);
            r.request_focus();
            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if widgets::primary(ui, &p, "Rename", Some("Enter")).clicked() || enter {
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
fn needs_popup(ctx: &egui::Context, app: &mut YaacApp, map: &AttentionMap, badge: Option<egui::Rect>, actions: &mut Vec<Action>) {
    let p = app.p;
    let list = yaac_core::sidebar::needs_you(&app.client.state, map, app.now());
    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown)) {
        app.ui.needs_sel = (app.ui.needs_sel + 1).min(list.len().saturating_sub(1));
    }
    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp)) {
        app.ui.needs_sel = app.ui.needs_sel.saturating_sub(1);
    }
    let enter = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
    let area = egui::Area::new(egui::Id::unique("needs-popup")).order(egui::Order::Foreground);
    // Attached to the rail's badge in a narrow window; centred near the top otherwise.
    let area = match badge {
        Some(b) => area.fixed_pos(egui::pos2(b.right() + 8.0, b.top())),
        None => area.anchor(Align2::CENTER_TOP, vec2(0.0, 80.0)),
    };
    area.show(ctx, |ui| {
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
            ui.label(widgets::secondary("Up and Down to move · Enter to open · Ctrl+J", &p));
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
                // One action (D96): starting it asks the hub to connect, which boots the distro.
                // Turning the connection off lives only in Settings → Targets.
                ui.horizontal(|ui| {
                    if widgets::plain(ui, &p, "Start it", None).clicked() {
                        actions.push(Action::Retry(id.clone()));
                        app.ui.dismissed.insert(id.clone());
                    }
                });
            });
        });
    }
}

/// Files dragged over the window attach to the open composer (docs/06-attachments.md): while
/// they hover, the thread view shows a quiet "Drop to attach"; dropped anywhere — transcript or
/// composer — they become chips. Nothing happens where there is no composer to take them.
fn drop_target(ui: &mut egui::Ui, app: &mut YaacApp, area: egui::Rect, actions: &mut Vec<Action>) {
    let Some(d) = app.open_composer() else { return };
    let (hovering, dropped) = ui.input(|i| {
        let paths: Vec<String> = i.raw.dropped_files.iter().map(|f| f.path()).filter(|p| p.is_absolute()).map(|p| p.to_string_lossy().into_owned()).collect();
        (!i.raw.hovered_files.is_empty(), paths)
    });
    if !dropped.is_empty() {
        actions.push(Action::AttachPaths(d, dropped));
    }
    if !(hovering || app.ui.drop_preview) {
        return;
    }
    let p = app.p;
    let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("drop-target")));
    // Leave the hairline beside the sidebar visible.
    let area = egui::Rect::from_min_max(egui::pos2(area.left() + 1.0, area.top()), area.max);
    painter.rect_filled(area, 0.0, p.bg.gamma_multiply(0.7));
    let inner = area.shrink2(vec2(28.0, 20.0));
    painter.rect_filled(inner, 10.0, p.faint);
    let font = theme::ui(size::SECTION);
    let g = ui.fonts_mut(|f| f.layout_no_wrap("Drop to attach".to_owned(), font, p.text));
    let total = 26.0 + g.size().x;
    let left = inner.center().x - total / 2.0;
    crate::icons::paperclip(&painter, egui::pos2(left + 9.0, inner.center().y), 20.0, p.text2);
    painter.galley(egui::pos2(left + 26.0, inner.center().y - g.size().y / 2.0), g, p.text);
}

/// The hairline beside the docked agents panel resizes it (D105), between its minimum and
/// maximum width; the width is remembered.
fn agents_edge(ui: &mut egui::Ui, app: &mut YaacApp, right: egui::Rect) {
    let edge = egui::Rect::from_min_max(egui::pos2(right.left() - 3.0, right.top()), egui::pos2(right.left() + 3.0, right.bottom()));
    let resp = ui.interact(edge, egui::Id::unique("agents-edge"), egui::Sense::drag()).on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
    if resp.drag_started() {
        app.ui.agents_drag = Some(right.width());
    }
    if resp.dragged()
        && let (Some(start), Some(origin), Some(now)) = (app.ui.agents_drag, ui.input(|i| i.pointer.press_origin()), ui.input(|i| i.pointer.interact_pos()))
    {
        app.ui.agents_width = yaac_core::prefs::agents_width(start - (now.x - origin.x));
    }
    if resp.drag_stopped() {
        app.ui.agents_drag = None;
    }
    if resp.hovered() || resp.dragged() {
        ui.painter().vline(right.left(), right.y_range(), egui::Stroke::new(2.0, app.p.focus));
    }
}

/// The hairline beside the sidebar is its resize handle: drag it between the minimum and the
/// maximum width; well below the minimum the sidebar collapses to the strip, and dragging out
/// of the strip brings it back (yaac_core::strip).
fn resize_edge(ui: &mut egui::Ui, app: &mut YaacApp, side: egui::Rect) {
    let edge = egui::Rect::from_min_max(egui::pos2(side.right() - 3.0, side.top()), egui::pos2(side.right() + 3.0, side.bottom()));
    let resp = ui.interact(edge, egui::Id::unique("sidebar-edge"), egui::Sense::drag()).on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
    if resp.drag_started() {
        app.ui.sidebar_drag = Some(side.width());
    }
    if resp.dragged()
        && let (Some(start), Some(origin), Some(now)) = (app.ui.sidebar_drag, ui.input(|i| i.pointer.press_origin()), ui.input(|i| i.pointer.interact_pos()))
    {
        app.ui.sidebar.drag_to(start + now.x - origin.x);
    }
    if resp.drag_stopped() {
        app.ui.sidebar_drag = None;
    }
    if resp.hovered() || resp.dragged() {
        ui.painter().vline(side.right(), side.y_range(), egui::Stroke::new(2.0, app.p.focus));
    }
}
