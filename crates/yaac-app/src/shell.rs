//! The one shell: sidebar | thread (header, transcript, composer, control row) | agents.

use eframe::egui::{self, Align2, Margin, vec2};
use yaac_core::agents::{PanelMode, agents_panel};
use yaac_core::attention::AttentionMap;
use yaac_core::state::Selection;

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
    // A pane tab in full screen hides the chat; the full-screen Chat tab shows it.
    let full_browser = selected.as_ref().is_some_and(|k| !app.browsers.chat_visible(k));
    let panel = selected.as_ref().filter(|_| !full_browser).and_then(|k| agents_panel(&app.client.state, map, k)).filter(|p| p.all().next().is_some());
    let docked = panel.is_some() && app.ui.panel == PanelMode::Docked && !narrow;
    // With a side pane open the Related agents panel belongs to the chat column, left of the
    // pane (docs/12-files.md#where-the-pane-sits); the thread view leaves room for it.
    let pane = selected.as_ref().is_some_and(|k| app.browsers.areas.contains_key(k));
    app.ui.chat_dock_width = (docked && pane).then_some(app.ui.agents_width);
    app.ui.chat_dock = None;
    app.ui.chat_column = None;
    app.ui.chat_agents = pane && panel.is_some();
    // The sidebar's own width, or the strip (collapsed, or a window too narrow for it).
    let side_w = app.ui.sidebar.shown(narrow);
    let strip = side_w <= yaac_core::strip::STRIP_WIDTH;
    let side = egui::Rect::from_min_max(full.min, egui::pos2(full.left() + side_w, full.bottom()));
    let right = if docked && !pane { egui::Rect::from_min_max(egui::pos2(full.right() - app.ui.agents_width.min(full.width() * 0.4), full.top()), full.max) } else { egui::Rect::NOTHING };
    let center = egui::Rect::from_min_max(egui::pos2(side.right(), full.top()), egui::pos2(if docked && !pane { right.left() } else { full.right() }, full.bottom()));
    let hair = egui::Stroke::new(1.0, p.border);
    ui.painter().rect_filled(side, 0.0, p.panel);
    // The window's top row is its title bar (D104): the sidebar's and the thread's first rows
    // sit in it. Only the docked agents panel starts below it (the window buttons are above).
    let below = |r: egui::Rect| egui::Rect::from_min_max(egui::pos2(r.left(), r.top() + crate::titlebar::HEIGHT), r.max);
    app.ui.threads_overlay = false;
    // What this window shows, for the hub's unseen mark (D114): the open thread, while the
    // thread view is on screen.
    let showing: Vec<yaac_core::ThreadKey> = match &app.client.state.selection {
        Selection::Thread(k) if !app.ui.settings && !full_browser => vec![k.clone()],
        _ => Vec::new(),
    };
    app.client.set_showing(showing);
    if strip {
        crate::strip::show(ui, app, map, now, side, actions);
    } else {
        ui.scope_builder(egui::UiBuilder::new().max_rect(egui::Rect::from_min_max(side.min + vec2(12.0, 7.0), side.max - vec2(12.0, 10.0))).id_salt("sidebar"), |ui| {
            ui.set_clip_rect(side);
            crate::sidebar::show(ui, app, map, now, actions);
        });
    }
    let draw_dock = |ui: &mut egui::Ui, app: &mut YaacApp, panel: &yaac_core::agents::AgentsPanel, rect: egui::Rect, content: egui::Rect, actions: &mut Vec<Action>| {
        ui.painter().rect_filled(rect, 0.0, p.panel);
        let hovered = ui.scope_builder(egui::UiBuilder::new().max_rect(content.shrink2(vec2(14.0, 12.0))).id_salt("agents-docked"), |ui| {
            ui.set_clip_rect(rect);
            crate::agents_ui::docked(ui, app, panel, actions)
        }).inner;
        crate::agents_ui::dock_hover(app, ui.ctx(), ui.rect_contains_pointer(content), hovered);
    };
    if docked && !pane && let Some(panel) = &panel {
        draw_dock(ui, app, panel, right, below(right), actions);
    }
    ui.scope_builder(egui::UiBuilder::new().max_rect(center).id_salt("center"), |ui| {
        ui.set_clip_rect(center);
        match app.client.state.selection.clone() {
            Selection::Thread(key) => crate::thread::show(ui, app, map, &key, now, actions),
            Selection::NewThread(id) => crate::composer::draft_view(ui, app, id, actions),
            _ if yaac_core::first_run::is_first_run(&app.client.state) => crate::first_run::show(ui, app, actions),
            _ => {
                ui.centered_and_justified(|ui| {
                    ui.label(widgets::secondary(empty_workspace(app), &p));
                });
            }
        }
    });
    // With a side pane, the docked panel goes where the thread view left room for it.
    let dock = app.ui.chat_dock.filter(|_| docked && pane);
    match (&panel, dock) {
        (Some(panel), Some(dock)) => draw_dock(ui, app, panel, dock, dock, actions),
        _ if !(docked && !pane) => crate::agents_ui::dock_hover(app, ui.ctx(), false, None),
        _ => {}
    }
    let browser = selected.as_ref().and_then(|k| app.browsers.areas.get(k));
    let chat = match app.ui.chat_column {
        Some(column) if browser.is_some() => column,
        _ => below(center),
    };
    if !full_browser && (browser.is_none()
        || ui.input(|i| i.pointer.hover_pos().is_some_and(|p| chat.contains(p)))) {
        drop_target(ui, app, chat, actions);
    }
    // The hairlines, painted after the thread view (whose panels fill their own background)
    // and snapped to one pixel column, so each is one unbroken line the full height.
    let snap = |x: f32| ui.painter().round_to_pixel_center(x);
    ui.painter().vline(snap(side.right()), full.y_range(), hair);
    if docked && !pane {
        ui.painter().vline(snap(right.left()), full.y_range(), hair);
        agents_edge(ui, app, right);
    }
    if let Some(dock) = dock {
        let x = ui.painter().round_to_pixel_center(dock.left());
        ui.painter().vline(x, dock.y_range(), hair);
        agents_edge(ui, app, dock);
    }
    if !narrow {
        resize_edge(ui, app, side);
    }
    if let Some(panel) = &panel
        && !(docked && !pane)
        && dock.is_none()
    {
        // Inside the chat column when a side pane is open (never over the pane).
        let column = if pane { app.ui.chat_column.map(|c| egui::Rect::from_min_max(egui::pos2(c.left(), center.top()), c.max)) } else { Some(center) };
        if let Some(column) = column {
            crate::agents_ui::floating(ui.ctx(), app, column, panel, actions);
        }
    }

    wsl_notification(ui.ctx(), app, actions);
    rename_dialog(ui.ctx(), app, actions);
    create_project_dialog(ui.ctx(), app, actions);
    crate::dialogs::fork(ui.ctx(), app, actions);
    crate::dialogs::delete(ui.ctx(), app, actions);
    crate::dialogs::archive(ui.ctx(), app, actions);
    crate::dialogs::update_now(ui.ctx(), app, actions);
    crate::dialogs::bulk_archive(ui.ctx(), app, actions);
    crate::dialogs::move_dialog(ui.ctx(), app, actions);
}

pub(crate) fn window_error_notice(ctx: &egui::Context, app: &YaacApp, width: f32, actions: &mut Vec<Action>) {
    let Some(error) = app.client.errors.iter().rfind(|e| e.key.is_none()) else { return };
    egui::Area::new(egui::Id::unique("window-error")).order(egui::Order::Foreground)
        .anchor(Align2::CENTER_BOTTOM, vec2(0.0, -5.0)).show(ctx, |ui| {
            egui::Frame::popup(ui.style()).fill(app.p.bg).show(ui, |ui| {
                ui.set_max_width(600.0_f32.min(width));
                error_notice(ui, app, error, actions);
            });
        });
}

pub(crate) fn error_notice(ui: &mut egui::Ui, app: &YaacApp, error: &yaac_core::client::ClientError, actions: &mut Vec<Action>) {
    let p = app.p;
    ui.push_id(error.id, |ui| {
        ui.horizontal(|ui| {
            if widgets::icon_button(ui, &p, 12.0, "Dismiss notice", crate::icons::close).clicked() {
                actions.push(Action::DismissError(error.id, error.key.clone()));
            }
            ui.label(egui::RichText::new(&error.text).font(theme::ui(size::SMALL)).color(p.failed));
        });
    });
}

pub(crate) fn empty_workspace(app: &YaacApp) -> String {
    let hub = &app.client.state.hub;
    if hub.attached {
        format!("Choose a thread, or start one with {}.", crate::commander::tip(app, "New", yaac_core::commands::Cmd::NewThread))
    } else {
        hub.last_error.clone().unwrap_or_else(|| "Attaching to the hub…".to_owned())
    }
}


/// Create project (screen 29): machine (read-only), folder, name; one filled button.
fn create_project_dialog(ctx: &egui::Context, app: &mut YaacApp, actions: &mut Vec<Action>) {
    let p = app.p;
    let Some(mut cp) = app.ui.create_project.take() else { return };
    let machine = app.client.state.target_name(&cp.target);
    let moving = project_thread_count(app, &cp.target, &cp.folder);
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

pub(crate) fn project_thread_count(app: &YaacApp, target: &yaac_protocol::TargetId, folder: &str) -> usize {
    app.client.state.threads.iter().filter(|(key, record)| {
        &key.target == target && yaac_core::projects::contains(folder, record.summary.cwd.as_str())
            && app.client.state.project_of(key).is_none()
    }).count()
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

/// A WSL distro that stopped on its own raises a notification; YAAC does not start it.
pub(crate) fn stopped_wsl_notifications(app: &YaacApp) -> Vec<(yaac_protocol::TargetId, String)> {
    app
        .client
        .state
        .targets
        .iter()
        .filter_map(|t| {
            let id = &t.entry.id;
            if app.ui.dismissed.contains(id) {
                return None;
            }
            app.client.state.stopped_distro(id)?;
            yaac_core::header::composer_connection_notice(&app.client.state, id).map(|notice| (id.clone(), notice))
        })
        .collect()
}

fn wsl_notification(ctx: &egui::Context, app: &mut YaacApp, actions: &mut Vec<Action>) {
    let p = app.p;
    for (id, notice) in stopped_wsl_notifications(app) {
        egui::Area::new(egui::Id::unique(("wsl", id.as_str()))).order(egui::Order::Foreground).anchor(Align2::RIGHT_BOTTOM, vec2(-16.0, -16.0)).show(ctx, |ui| {
            egui::Frame::popup(ui.style()).fill(p.raised).inner_margin(Margin::same(14)).show(ui, |ui| {
                ui.set_width(300.0);
                ui.horizontal(|ui| {
                    widgets::icon(ui, 16.0, |pa, c, d| crate::icons::warning(pa, c, d, p.needs, p.raised));
                    ui.label(notice);
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
