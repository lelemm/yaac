//! The sidebar: a flat list under sticky headers (no tree), virtualised.

use eframe::egui::{self, Margin, Rect, Sense, pos2, vec2};
use yaac_core::attention::AttentionMap;
use yaac_core::sidebar::{NeedsCycle, Order, RowRight, Section, SectionKind, hub_footer, needs_you, sections, title_width};
use yaac_core::thread_menu::{MenuAction, MenuEntry, thread_menu};
use yaac_core::state::Selection;

use crate::app::{Action, Menu, YaacApp};
use crate::icons;
use crate::theme::{self, size};
use crate::widgets;

const ROW: f32 = 28.0;
const ROW2: f32 = 44.0;
const HEAD: f32 = 26.0;
const HEAD_GAP: f32 = 12.0;

enum Line<'a> {
    Head(&'a Section),
    Row(&'a Section, usize),
}

pub(crate) fn show(ui: &mut egui::Ui, app: &mut YaacApp, map: &AttentionMap, now: i64, actions: &mut Vec<Action>) {
    let p = app.p;
    // A fresh install: just the name, a disabled search field, and the footer (screen 07).
    let fresh = yaac_core::first_run::is_first_run(&app.client.state);
    ui.horizontal(|ui| {
        ui.set_min_height(28.0);
        ui.add(egui::Label::new(egui::RichText::new("YAAC").font(theme::bold(17.0))).selectable(false));
        if !fresh {
            ui.add_space(6.0);
            crate::nav::buttons(ui, app, actions, false);
            needs_button(ui, app, map, now, actions);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Collapse to the strip (Ctrl+B).
                let (r, resp) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::click());
                if resp.hovered() || resp.has_focus() {
                    ui.painter().rect_filled(r, 5.0, p.faint);
                }
                icons::sidebar(ui.painter(), r.center(), 16.0, p.text2);
                if resp.on_hover_text(crate::commander::tip(app, "Collapse the sidebar", yaac_core::commands::Cmd::ToggleSidebar)).clicked() {
                    app.ui.sidebar.toggle();
                }
                if widgets::plain(ui, &p, "New", None).on_hover_text(crate::commander::tip(app, "New thread", yaac_core::commands::Cmd::NewThread)).clicked() {
                    actions.push(Action::NewThread);
                }
            });
        }
    });
    ui.add_space(8.0);
    search_field(ui, app, !fresh);
    ui.add_space(6.0);

    // Attached to a real hub, the search goes to the hub (by id or fuzzy title, across every
    // cached target, paged); the demo filters its own list.
    if app.uses_hub_search() {
        let q = app.ui.search.clone();
        app.client.find(&q);
        if app.client.state.find.is_some() {
            find_results(ui, app, map, actions);
            footer(ui, app, actions);
            return;
        }
    }
    let open = yaac_core::sidebar::Open { muted: app.ui.muted_open, archived: app.ui.archived_open };
    let fresh = sections(&app.client.state, map, app.ui.grouping, app.ui.ordering, &app.ui.search, open, now, app.zone);
    // Rows do not jump under the pointer (D116): while it is over the list, or a row's menu
    // is open, a reorder waits until it leaves.
    let pointer = ui.ctx().pointer_hover_pos();
    let hold = app.shots.is_none()
        && (matches!(app.ui.menu, Some(Menu::Context(_))) || pointer.is_some_and(|pt| app.ui.list_rect.is_some_and(|r| r.contains(pt))));
    let secs = app.ui.steady.apply(fresh, hold);
    let mut lines: Vec<(f32, f32, Line<'_>)> = Vec::new();
    let mut y = 0.0;
    for (i, s) in secs.iter().enumerate() {
        if s.header.is_some() {
            if i > 0 {
                y += HEAD_GAP;
            }
            lines.push((y, HEAD, Line::Head(s)));
            y += HEAD;
        }
        for r in 0..s.rows.len() {
            let h = if s.rows[r].line2.is_some() { ROW2 } else { ROW };
            lines.push((y, h, Line::Row(s, r)));
            y += h;
        }
    }
    let total = y;
    let footer_h = if app.demo.is_some() { 74.0 } else { 40.0 };
    let list_h = ui.available_height() - footer_h;
    let selected = app.client.state.selected_thread().cloned();
    let mut sa = egui::ScrollArea::vertical().id_salt("sidebar-list").max_height(list_h).auto_shrink([false, false]);
    if app.ui.sidebar_to_end {
        // Held for the screenshot's settle frames (the area learns its size on the first).
        app.ui.sidebar_to_end = app.shots.is_some();
        sa = sa.vertical_scroll_offset((total - list_h).max(0.0));
    }
    let shown = sa.show_viewport(ui, |ui, view| {
        ui.set_height(total);
        let origin = ui.max_rect().top();
        let left = ui.max_rect().left();
        let width = ui.max_rect().width();
        // Rows, only those in view.
        for (ly, h, line) in &lines {
            if ly + h < view.top() || *ly > view.bottom() {
                continue;
            }
            let rect = Rect::from_min_size(pos2(left, origin + ly), vec2(width, *h));
            match line {
                Line::Head(s) => header(ui, app, s, rect, actions),
                Line::Row(s, r) => row(ui, app, s, *r, rect, selected.as_ref(), now, actions),
            }
        }
        // Sticky header: the section being read stays pinned until the next pushes it away.
        let heads: Vec<(f32, &Section)> = lines.iter().filter_map(|(ly, _, l)| match l { Line::Head(s) => Some((*ly, *s)), _ => None }).collect();
        if let Some(pos) = heads.iter().rposition(|(hy, _)| *hy <= view.top()) {
            let (hy, s) = heads[pos];
            if hy < view.top() {
                let next = heads.get(pos + 1).map_or(f32::INFINITY, |(n, _)| *n - HEAD_GAP);
                let top = view.top().min(next - HEAD);
                let rect = Rect::from_min_size(pos2(left, origin + top), vec2(width, HEAD));
                ui.painter().rect_filled(rect, 0.0, p.panel);
                header(ui, app, s, rect, actions);
            }
        }
    });
    app.ui.list_rect = Some(shown.inner_rect);

    footer(ui, app, actions);
}

/// The Needs-you button (D117): the red exclamation beside the history buttons, with the
/// count, shown only while some thread needs the user. Each press opens the next one.
pub(crate) fn needs_button(ui: &mut egui::Ui, app: &YaacApp, map: &AttentionMap, now: i64, actions: &mut Vec<Action>) {
    let n = needs_you(&app.client.state, map, now).len();
    if n == 0 {
        return;
    }
    let p = app.p;
    let count = ui.fonts_mut(|f| f.layout_no_wrap(n.to_string(), theme::bold(size::SMALL), p.failed));
    let (r, resp) = ui.allocate_exact_size(vec2(26.0 + count.size().x, 26.0), Sense::click());
    if resp.hovered() || resp.has_focus() {
        ui.painter().rect_filled(r, 5.0, p.faint);
    }
    icons::exclaim(ui.painter(), pos2(r.left() + 12.0, r.center().y), 16.0, p.failed);
    ui.painter().galley(pos2(r.left() + 22.0, r.center().y - count.size().y / 2.0), count, p.failed);
    if resp.on_hover_text(crate::commander::tip(app, &NeedsCycle::tooltip(n), yaac_core::commands::Cmd::NeedsNext)).clicked() {
        actions.push(Action::NeedsNext);
    }
}

/// The thread menu (D109) in the shared menu style; returns what was picked.
pub(crate) fn menu_entries(ui: &mut egui::Ui, p: &crate::theme::Palette, entries: &[MenuEntry]) -> Option<MenuAction> {
    let mut picked = None;
    for e in entries {
        match e {
            MenuEntry::Separator => {
                ui.separator();
            }
            MenuEntry::Item(i) => {
                let kind = if !i.enabled {
                    widgets::ItemKind::Disabled
                } else if i.danger {
                    widgets::ItemKind::Danger
                } else {
                    widgets::ItemKind::Normal
                };
                let mut r = widgets::menu_item(ui, p, i.label, None, kind);
                if let Some(why) = i.reason {
                    r = r.on_disabled_hover_text(why);
                }
                if r.clicked() {
                    picked = Some(i.action);
                }
            }
        }
    }
    picked
}

/// Does what a thread-menu entry says.
pub(crate) fn run_menu_action(app: &mut YaacApp, ctx: &egui::Context, key: &yaac_core::ThreadKey, action: MenuAction, actions: &mut Vec<Action>) {
    let k = key.clone();
    match action {
        MenuAction::CopyId => {
            // The harness's own id (what a terminal shows), else YAAC's address.
            let id = app.client.state.summary(key).and_then(|s| s.native_id.clone()).unwrap_or_else(|| key.address().to_string());
            ctx.copy_text(id);
        }
        MenuAction::Rename => {
            let title = app.client.state.summary(key).map(|s| s.title.clone()).unwrap_or_default();
            app.ui.rename = Some((k, title));
        }
        MenuAction::Fork => actions.push(Action::OpenFork(k)),
        MenuAction::Pin => actions.push(Action::SetPinned(k, true)),
        MenuAction::Unpin => actions.push(Action::SetPinned(k, false)),
        MenuAction::Mute => actions.push(Action::OpenMute(k, false)),
        MenuAction::ChangeMute => actions.push(Action::OpenMute(k, true)),
        MenuAction::Unmute => actions.push(Action::SetMuted(k, false)),
        MenuAction::Dismiss => actions.push(Action::Dismiss(k, true)),
        MenuAction::ShowAgain => actions.push(Action::Dismiss(k, false)),
        MenuAction::Archive => actions.push(Action::OpenArchive(k)),
        MenuAction::Unarchive => actions.push(Action::Unarchive(k)),
        MenuAction::Delete => actions.push(Action::OpenDelete(k)),
    }
}

/// A row's context menu, attached to `resp`: opened by a right-click, one at a time. Returns
/// whether it is open (the row then keeps its hover colour, D108).
pub(crate) fn row_menu(ui: &egui::Ui, app: &mut YaacApp, key: &yaac_core::ThreadKey, resp: &egui::Response, now: i64, actions: &mut Vec<Action>) -> bool {
    if resp.secondary_clicked() {
        app.ui.menu = Some(Menu::Context(key.clone()));
    }
    let mut open = app.ui.menu == Some(Menu::Context(key.clone()));
    if !open {
        return false;
    }
    let p = app.p;
    let entries = thread_menu(&app.client.state, key, now);
    let mut picked = None;
    crate::widgets::menu_popup(egui::Popup::from_response(resp), &p)
        .kind(egui::PopupKind::Menu)
        .align(egui::RectAlign::RIGHT_START)
        .open_bool(&mut open)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
        .show(|ui| {
            ui.set_min_width(170.0);
            picked = menu_entries(ui, &p, &entries);
        });
    if let Some(a) = picked {
        run_menu_action(app, ui.ctx(), key, a, actions);
    }
    if !open && app.ui.menu == Some(Menu::Context(key.clone())) {
        app.ui.menu = None;
    }
    open
}

/// The unseen outline (D114): a rounded cornflower border around a row.
pub(crate) fn unseen_outline(ui: &egui::Ui, rect: Rect, p: &crate::theme::Palette) {
    ui.painter().rect_stroke(rect.shrink(1.0), 6.0, egui::Stroke::new(1.5, p.unseen), egui::StrokeKind::Inside);
}

/// The hub's search results: one-line rows, then "More results" while there are pages left.
fn find_results(ui: &mut egui::Ui, app: &mut YaacApp, map: &AttentionMap, actions: &mut Vec<Action>) {
    let p = app.p;
    let Some(f) = app.client.state.find.clone() else { return };
    let list_h = ui.available_height() - if app.demo.is_some() { 74.0 } else { 40.0 };
    egui::ScrollArea::vertical().id_salt("find-list").max_height(list_h).auto_shrink([false, false]).show(ui, |ui| {
        if let Some(e) = &f.error {
            ui.label(egui::RichText::new(e).font(theme::ui(size::SMALL)).color(p.failed));
        }
        ui.label(widgets::secondary(if f.loading && f.results.is_empty() { "Searching…".to_owned() } else { format!("{} found", f.total) }, &p));
        ui.add_space(4.0);
        for key in &f.results {
            let st = &app.client.state;
            let Some(s) = st.summary(key) else { continue };
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::click());
            if st.selected_thread() == Some(key) {
                ui.painter().rect_filled(rect.shrink2(vec2(0.0, 1.0)), 5.0, p.selected);
            } else if resp.hovered() {
                ui.painter().rect_filled(rect.shrink2(vec2(0.0, 1.0)), 5.0, p.faint);
            }
            let mark = yaac_core::attention::display(st, map, key).mark;
            icons::mark(ui.painter(), pos2(rect.left() + 13.0, rect.center().y), 15.0, mark, &p);
            let machine = ui.fonts_mut(|fo| fo.layout_no_wrap(st.target_name(&key.target), theme::ui(size::SMALL), p.text2));
            let rx = rect.right() - 8.0 - machine.size().x;
            ui.painter().galley(pos2(rx, rect.center().y - machine.size().y / 2.0), machine, p.text2);
            let g = widgets::elided(ui, &s.title, &theme::ui(size::UI), p.text, rx - rect.left() - 36.0);
            ui.painter().galley(pos2(rect.left() + 28.0, rect.center().y - g.size().y / 2.0), g, p.text);
            if resp.clicked() {
                actions.push(Action::Open(key.clone()));
            }
        }
        if f.results.is_empty() && !f.loading && f.error.is_none() {
            ui.label(widgets::secondary("No thread matches.", &p));
        }
        if f.cursor.is_some() && !f.loading && widgets::link(ui, &p, "More results", theme::ui(size::SMALL)).clicked() {
            app.client.find_more();
        }
    });
}

fn footer(ui: &mut egui::Ui, app: &mut YaacApp, actions: &mut Vec<Action>) {
    let p = app.p;
    // Footer: Settings, and the quiet hub status.
    ui.add_space(4.0);
    if app.demo.is_some() {
        let d = widgets::plain(ui, &p, "Demo: play the story", None);
        if d.clicked() {
            app.ui.toggle_menu(Menu::Demo);
        }
        demo_menu(ui, app, &d, actions);
    }
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::hover());
    let settings = Rect::from_min_size(rect.min, vec2(96.0, 30.0));
    let resp = ui.interact(settings, egui::Id::unique("settings-button"), Sense::click()).on_hover_text(crate::commander::tip(app, "Settings", yaac_core::commands::Cmd::Settings));
    if resp.hovered() {
        ui.painter().rect_filled(settings, 5.0, p.faint);
    }
    icons::gear(ui.painter(), pos2(settings.left() + 12.0, settings.center().y), 15.0, p.text2);
    let g = ui.fonts_mut(|f| f.layout_no_wrap("Settings".to_owned(), theme::ui(size::UI), p.text));
    ui.painter().galley(pos2(settings.left() + 26.0, settings.center().y - g.size().y / 2.0), g, p.text);
    if resp.clicked() {
        actions.push(Action::Settings(true));
    }
    // The hub as a connection icon, and the targets as counts (D106), right-aligned in what
    // is left beside Settings.
    let f = hub_footer(&app.client.state);
    let mut x = rect.right();
    let cy = rect.center().y;
    let font = theme::ui(size::SMALL);
    let mut tip = Vec::new();
    for (n, up) in [(f.down, false), (f.up, true)] {
        if !up && n == 0 {
            continue;
        }
        let g = ui.fonts_mut(|fo| fo.layout_no_wrap(n.to_string(), font.clone(), p.text2));
        x -= g.size().x;
        ui.painter().galley(pos2(x, cy - g.size().y / 2.0), g, p.text2);
        x -= 9.0;
        icons::arrow_vertical(ui.painter(), pos2(x, cy), 13.0, p.text2, up);
        x -= 14.0;
        tip.push(format!("{n} {}", if up { "connected" } else { "not connected" }));
    }
    x -= 2.0;
    icons::connection(ui.painter(), pos2(x - 8.0, cy), 15.0, p.text2, !f.reachable);
    tip.reverse();
    let hub = if f.reachable { "Hub running" } else { "The hub cannot be reached" };
    let area = Rect::from_min_max(pos2((x - 18.0).max(settings.right()), rect.top()), rect.max);
    ui.interact(area, egui::Id::unique("hub-status"), Sense::hover()).on_hover_text(format!("{hub} · targets: {}", tip.join(", ")));
    // Components older than this build (D135), found as YAAC started: "2 to update", which
    // opens the list in Settings → Advanced. Nothing is drawn while everything is current.
    let n = yaac_core::components::outdated(&app.client.state, crate::BUILD);
    if n > 0 {
        let g = ui.fonts_mut(|fo| fo.layout_no_wrap(format!("{n} to update"), font.clone(), p.failed));
        let right = x - 22.0;
        let r = Rect::from_min_max(pos2(right - g.size().x, cy - g.size().y / 2.0), pos2(right, cy + g.size().y / 2.0));
        if r.left() > settings.right() + 4.0 {
            ui.painter().galley(r.min, g, p.failed);
            let resp = ui.interact(r, egui::Id::unique("components-outdated"), Sense::click()).on_hover_text("Some of YAAC's components are older than this build. Show them");
            if resp.clicked() {
                actions.push(Action::Settings(true));
                actions.push(Action::SettingsPage(yaac_core::settings::Page::Advanced));
            }
        }
    }
}

fn search_field(ui: &mut egui::Ui, app: &mut YaacApp, enabled: bool) {
    let p = app.p;
    let frame = widgets::faint_frame(&p).inner_margin(Margin { left: 30, right: 58, top: 5, bottom: 5 });
    let inner = frame.show(ui, |ui| {
        let te = egui::TextEdit::singleline(&mut app.ui.search)
            .interactive(enabled)
            .frame(egui::Frame::NONE)
            .hint_text("Search…")
            .align(egui::Align2::LEFT_CENTER)
            .min_size(vec2(0.0, 22.0))
            .desired_width(ui.available_width());
        let r = ui.add(te);
        if app.ui.focus_search {
            r.request_focus();
            app.ui.focus_search = false;
        }
        r
    });
    let rect = inner.response.rect;
    icons::search(ui.painter(), pos2(rect.left() + 15.0, rect.center().y), 14.0, p.text2);
    if inner.inner.has_focus() {
        widgets::focus_ring(ui, rect, &p);
    }
    if !enabled {
        return;
    }
    // The grouping button at the right end of the field.
    let b = Rect::from_center_size(pos2(rect.right() - 16.0, rect.center().y), vec2(24.0, 22.0));
    let resp = ui.interact(b, egui::Id::unique("grouping-button"), Sense::click()).on_hover_text("Grouping");
    if resp.hovered() {
        ui.painter().rect_filled(b, 4.0, p.selected);
    }
    for (i, w) in [12.0, 8.0, 4.0].iter().enumerate() {
        let yy = b.center().y - 4.0 + i as f32 * 4.0;
        ui.painter().line_segment([pos2(b.center().x - w / 2.0, yy), pos2(b.center().x + w / 2.0, yy)], egui::Stroke::new(1.5, p.text2));
    }
    if resp.clicked() {
        app.ui.toggle_menu(Menu::Grouping);
    }
    let mut open = app.ui.menu == Some(Menu::Grouping);
    crate::widgets::menu_popup(egui::Popup::from_response(&resp), &p)
        .kind(egui::PopupKind::Menu)
        .open_bool(&mut open)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.set_min_width(180.0);
            ui.label(widgets::secondary("Group by", &p));
            let g = &mut app.ui.grouping;
            ui.checkbox(&mut g.machine, "Machine");
            ui.checkbox(&mut g.project, "Project");
            ui.checkbox(&mut g.worktree, "Worktree");
            ui.add_space(8.0);
            ui.checkbox(&mut g.show_dismissed, "Show dismissed");
        });
    if !open && app.ui.menu == Some(Menu::Grouping) {
        app.ui.menu = None;
    }
    // The ordering button beside it (D116).
    let b = Rect::from_center_size(pos2(rect.right() - 42.0, rect.center().y), vec2(24.0, 22.0));
    let resp = ui.interact(b, egui::Id::unique("ordering-button"), Sense::click()).on_hover_text("Order");
    if resp.hovered() || app.ui.menu == Some(Menu::Ordering) {
        ui.painter().rect_filled(b, 4.0, p.selected);
    }
    icons::order(ui.painter(), b.center(), 14.0, p.text2);
    if resp.clicked() {
        app.ui.toggle_menu(Menu::Ordering);
    }
    let mut open = app.ui.menu == Some(Menu::Ordering);
    crate::widgets::menu_popup(egui::Popup::from_response(&resp), &p)
        .kind(egui::PopupKind::Menu)
        .open_bool(&mut open)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.set_min_width(290.0);
            ui.label(widgets::secondary("Order", &p));
            let o = &mut app.ui.ordering;
            for choice in Order::ALL {
                if ui.radio(o.order == choice, choice.label()).clicked() {
                    o.order = choice;
                }
            }
            ui.add_space(8.0);
            ui.checkbox(&mut o.unseen_first, "Keep unseen finished threads at the top");
            ui.checkbox(&mut o.needs_first, "Keep threads that need you at the top");
            ui.add_space(8.0);
            ui.checkbox(&mut o.running_only, "Show only running");
        });
    if !open && app.ui.menu == Some(Menu::Ordering) {
        app.ui.menu = None;
    }
}

fn header(ui: &mut egui::Ui, app: &mut YaacApp, s: &Section, rect: Rect, actions: &mut Vec<Action>) {
    let p = app.p;
    let Some(h) = &s.header else { return };
    let font = theme::ui(size::SMALL);
    let y = rect.bottom() - 7.0;
    let mut x = rect.left() + 4.0;
    if h.temporary {
        icons::dashed_circle(ui.painter(), pos2(x + 6.0, y - 6.0), 5.0, egui::Stroke::new(1.2, p.text2));
        x += 16.0;
        let resp = ui.interact(rect, egui::Id::unique(("temp-header", s.id.as_str(), rect.top().to_bits())), Sense::click());
        if resp.secondary_clicked() {
            app.ui.menu = Some(Menu::TempProject(s.id.clone()));
        }
        let mut open = app.ui.menu == Some(Menu::TempProject(s.id.clone()));
        if open {
            crate::widgets::menu_popup(egui::Popup::from_response(&resp), &p)
                .kind(egui::PopupKind::Menu)
                .open_bool(&mut open)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
                .show(|ui| {
                    if ui.button("Create project…").clicked()
                        && let Some((target, folder)) = &h.folder
                    {
                        app.ui.create_project = Some(create_project_for(app, target, folder));
                    }
                    // Hide every thread of this folder in the window ("Show dismissed" lists
                    // them again).
                    if ui.button("Dismiss all").clicked() {
                        for r in &s.rows {
                            actions.push(Action::Dismiss(r.key.clone(), true));
                        }
                    }
                });
            if !open && app.ui.menu == Some(Menu::TempProject(s.id.clone())) {
                app.ui.menu = None;
            }
        }
    }
    if matches!(s.kind, SectionKind::Muted | SectionKind::Archived) {
        icons::chevron(ui.painter(), pos2(x + 5.0, y - 6.0), 12.0, !s.collapsed, p.text2);
        x += 16.0;
        let resp = ui.interact(rect, egui::Id::unique(("collapsible-header", s.id.as_str(), rect.top().to_bits())), Sense::click());
        if resp.clicked() {
            if s.kind == SectionKind::Muted {
                app.ui.muted_open = !app.ui.muted_open;
            } else {
                app.ui.archived_open = !app.ui.archived_open;
            }
        }
    }
    let mut rx = rect.right() - 4.0;
    if h.retry {
        let g = ui.fonts_mut(|f| f.layout_no_wrap("Retry".to_owned(), font.clone(), p.text));
        let r = Rect::from_min_size(pos2(rx - g.size().x, y - g.size().y), g.size());
        ui.painter().galley(r.min, g, p.text);
        let retry = ui.interact(r, egui::Id::unique(("retry", s.id.as_str(), rect.top().to_bits())), Sense::click());
        if retry.hovered() || retry.has_focus() {
            ui.painter().hline(r.x_range(), r.bottom(), egui::Stroke::new(1.0, p.text));
        }
        if retry.clicked()
            && let Some(m) = &h.machine
            && let Some(t) = app.client.state.targets.iter().find(|t| t.entry.name.as_str() == m)
        {
            actions.push(Action::Retry(t.entry.id.clone()));
        }
        rx = r.left() - 8.0;
    }
    if let Some((mark, word)) = &h.connection {
        // Keep room for the group's own name: drop the detail when it would crowd it out.
        let full = ui.fonts_mut(|f| f.layout_no_wrap(word.clone(), font.clone(), p.text2)).size().x;
        let machine_w = h.machine.as_ref().map_or(0.0, |m| ui.fonts_mut(|f| f.layout_no_wrap(m.clone(), font.clone(), p.text2)).size().x);
        let word = if rx - full - machine_w - 30.0 - x < 90.0 { word.split(" · ").next().unwrap_or(word).to_owned() } else { word.clone() };
        let g = ui.fonts_mut(|f| f.layout_no_wrap(word, font.clone(), p.text2));
        let w = g.size().x;
        ui.painter().galley(pos2(rx - w, y - g.size().y), g, p.text2);
        rx -= w + 6.0;
        icons::mark(ui.painter(), pos2(rx - 6.0, y - 6.0), 11.0, *mark, &p);
        rx -= 16.0;
    }
    if let Some(m) = &h.machine {
        let g = ui.fonts_mut(|f| f.layout_no_wrap(m.clone(), font.clone(), p.text2));
        let gs = g.size();
        ui.painter().galley(pos2(rx - gs.x, y - gs.y), g, p.text2);
        rx -= gs.x + 6.0;
    }
    if h.older_daemon {
        icons::warning(ui.painter(), pos2(rx - 6.0, y - 6.0), 11.0, p.needs, p.panel);
        rx -= 16.0;
    }
    let color = p.text2;
    let mut job = egui::text::LayoutJob::simple_singleline(h.left.clone(), font, color);
    if let Some(sec) = job.sections.first_mut() {
        sec.format.italics = h.temporary;
    }
    job.wrap = egui::text::TextWrapping { max_width: (rx - x - 8.0).max(10.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    let g = ui.fonts_mut(|f| f.layout_job(job));
    ui.painter().galley(pos2(x, y - g.size().y), g, color);
}

/// The Create project dialog, prefilled from a temporary project's folder.
pub(crate) fn create_project_for(app: &YaacApp, target: &yaac_protocol::TargetId, folder: &str) -> crate::app::CreateProject {
    // Suggest the repository root when a thread in this folder knows its repository.
    let repo_root = app
        .client
        .state
        .threads
        .iter()
        .filter(|(k, r)| &k.target == target && r.summary.cwd.as_str() == folder)
        .find_map(|(_, r)| r.summary.project.as_ref())
        .map(|p| p.path.as_str().trim_end_matches("/.git").to_owned())
        .filter(|root| root != folder);
    crate::app::CreateProject {
        target: target.clone(),
        folder: folder.to_owned(),
        name: yaac_core::projects::suggested_name(folder),
        repo_root,
        for_draft: None,
        browse: None,
    }
}

#[allow(clippy::too_many_arguments, reason = "a row is drawn from all of these")]
pub(crate) fn row(ui: &mut egui::Ui, app: &mut YaacApp, s: &Section, r: usize, rect: Rect, selected: Option<&yaac_core::ThreadKey>, now: i64, actions: &mut Vec<Action>) {
    let p = app.p;
    let row = &s.rows[r];
    let id = egui::Id::unique(("row", s.id.as_str(), row.key.to_string()));
    let resp = ui.interact(rect, id, Sense::click());
    let is_sel = selected == Some(&row.key) && app.client.state.selection == Selection::Thread(row.key.clone());
    let fill_rect = rect.shrink2(vec2(0.0, 1.0));
    // While its menu is open the row keeps its hover colour (D108).
    let menu_open = app.ui.menu == Some(Menu::Context(row.key.clone()));
    if is_sel {
        ui.painter().rect_filled(fill_rect, 5.0, p.selected);
    } else if resp.hovered() || menu_open {
        ui.painter().rect_filled(fill_rect, 5.0, p.faint);
    }
    if row.display.unseen && !is_sel {
        unseen_outline(ui, fill_rect, &p);
    }
    let cy = rect.top() + 14.0;
    icons::thread_mark(ui.painter(), pos2(rect.left() + 13.0, cy), 15.0, &row.display, &p);
    let mut title_x = rect.left() + 28.0;
    if row.display.read_only {
        icons::lock(ui.painter(), pos2(title_x + 5.0, cy), 12.0, p.text2);
        title_x += 14.0;
    }
    // Right side.
    let font = theme::ui(size::SMALL);
    let right_text = match &row.right {
        RowRight::Age(a) => a.clone(),
        RowRight::Machine { name, muted } => if *muted { format!("{name} · muted") } else { name.clone() },
        RowRight::Until(u) => u.clone(),
    };
    let g = ui.fonts_mut(|f| f.layout_no_wrap(right_text, font.clone(), p.text2));
    let rx = rect.right() - 8.0 - g.size().x;
    ui.painter().galley(pos2(rx, cy - g.size().y / 2.0), g, p.text2);
    // The PR badge (D119, on line 1 before the time): the title gives way, never the badge.
    app.ui.pr_visible.push(row.key.clone());
    let mut title_end = rx;
    if let Some(b) = yaac_core::pr::badge_of(&app.client.state, &row.key) {
        title_end = crate::pr_ui::badge(ui, app, &row.key, &b, rx - 6.0, cy, "side", actions) - 2.0;
    }
    if let Some(l2) = &row.line2 {
        // Line 2: the harness logo, the branch, then file counts in git colours, then commits
        // ahead and behind; the workers sit at the right edge, under the time. Every count
        // only when non-zero. Narrow, the branch is ellipsised and every mark stays (D115).
        let y2 = rect.top() + 32.0;
        let mut right = rect.right() - 8.0;
        if l2.workers > 0 {
            let g = ui.fonts_mut(|f| f.layout_no_wrap(l2.workers.to_string(), font.clone(), p.text2));
            right -= g.size().x;
            ui.painter().galley(pos2(right, y2 - g.size().y / 2.0), g, p.text2);
            right -= 16.0;
            icons::people(ui.painter(), pos2(right + 7.0, y2), 13.0, p.text2);
            right -= 6.0;
        }
        let small = theme::ui(size::SMALL - 0.5);
        git_marks(ui, &p, l2, title_x, y2, right, &small);
    }
    let color = if s.kind == SectionKind::Muted { p.text2 } else { p.text };
    let forked = app.client.state.summary(&row.key).is_some_and(|s| s.forked_from.is_some());
    let tw_room = title_width(title_end - title_x, &[if forked { 18.0 } else { 0.0 }], 8.0);
    let tg = widgets::elided(ui, &row.title, &theme::ui(size::UI), color, tw_room.max(1.0));
    let tw = tg.size().x;
    ui.painter().galley(pos2(title_x, cy - tg.size().y / 2.0), tg, color);
    if forked {
        icons::fork(ui.painter(), pos2(title_x + tw + 10.0, cy), 12.0, p.text2);
    }
    if resp.clicked() {
        let needs = row.display.attention.is_some();
        actions.push(if needs { Action::OpenRequest(row.key.clone()) } else { Action::Open(row.key.clone()) });
    }
    let key = row.key.clone();
    row_menu(ui, app, &key, &resp, now, actions);
}

/// A row's git line from `x` to `right` (D115): the harness logo, the branch (ellipsised to
/// the room left), then every non-zero count — which always keeps its place.
pub(crate) fn git_marks(ui: &egui::Ui, p: &crate::theme::Palette, l2: &yaac_core::sidebar::Line2, x0: f32, y2: f32, right: f32, small: &egui::FontId) {
    let count = |n: u32| ui.fonts_mut(|f| f.layout_no_wrap(n.to_string(), small.clone(), p.text2)).size().x;
    let parts: [(u32, egui::Color32, u8, f32); 5] = [
        (l2.added, p.git_added, b'+', 12.0),
        (l2.modified, p.git_modified, b'.', 12.0),
        (l2.deleted, p.git_deleted, b'-', 12.0),
        (l2.ahead, p.text2, b'u', 12.0),
        (l2.behind, p.text2, b'd', 12.0),
    ];
    let widths: Vec<f32> = parts.iter().filter(|q| q.0 > 0).map(|q| q.3 + count(q.0) + 7.0).collect();
    let tail: f32 = widths.iter().sum();
    let mut x = x0;
    crate::agents_ui::harness_mark(ui.painter(), pos2(x + 6.0, y2), 13.0, l2.kind.as_deref(), None, false, p);
    x += 17.0;
    if !l2.branch.is_empty() {
        let room = title_width(right - x, &[13.0, tail], 8.0);
        if room > 12.0 {
            icons::branch(ui.painter(), pos2(x + 5.0, y2), 12.0, p.text2);
            x += 13.0;
            let g = widgets::elided(ui, &l2.branch, small, p.text2, room);
            let gw = g.size().x;
            ui.painter().galley(pos2(x, y2 - g.size().y / 2.0), g, p.text2);
            x += gw + 8.0;
        }
    }
    // The counts never give way: when the row is too narrow even for them, they keep the
    // right end and the branch is what disappears.
    x = x.min(right - tail).max(x0 + 17.0);
    for (n, col, kind, w) in parts {
        if n == 0 {
            continue;
        }
        match kind {
            b'u' => icons::arrow_vertical(ui.painter(), pos2(x + 5.0, y2), 11.0, col, true),
            b'd' => icons::arrow_vertical(ui.painter(), pos2(x + 5.0, y2), 11.0, col, false),
            k => icons::git_square(ui.painter(), pos2(x + 5.0, y2), 12.0, col, k as char),
        }
        x += w;
        let g = ui.fonts_mut(|f| f.layout_no_wrap(n.to_string(), small.clone(), col));
        let gw = g.size().x;
        ui.painter().galley(pos2(x, y2 - g.size().y / 2.0), g, col);
        x += gw + 7.0;
    }
}

fn demo_menu(ui: &mut egui::Ui, app: &mut YaacApp, resp: &egui::Response, actions: &mut Vec<Action>) {
    let mut open = app.ui.menu == Some(Menu::Demo);
    crate::widgets::menu_popup(egui::Popup::from_response(resp), &app.p)
        .kind(egui::PopupKind::Menu)
        .align(egui::RectAlign::TOP_START)
        .open_bool(&mut open)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
        .show(|ui| {
            ui.set_min_width(320.0);
            ui.label(widgets::secondary("Demo: play the next moment of the story", &app.p));
            for s in yaac_core::demo::Scene::ALL {
                if ui.button(s.label()).clicked() {
                    actions.push(Action::Scene(s));
                }
            }
            if ui.button("Stream a reply in this thread").clicked() {
                actions.push(Action::StreamReply);
            }
            if ui.button("Start the story again").clicked() {
                actions.push(Action::DemoReset);
            }
        });
    if !open && app.ui.menu == Some(Menu::Demo) {
        app.ui.menu = None;
    }
    let _ = ui;
}
