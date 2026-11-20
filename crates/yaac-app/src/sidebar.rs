//! The sidebar: a flat list under sticky headers (no tree), virtualised.

use eframe::egui::{self, Margin, Rect, Sense, pos2, vec2};
use yaac_core::attention::{AttentionMap, Mark};
use yaac_core::sidebar::{RowRight, Section, SectionKind, hub_status, sections};
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
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("YAAC").font(theme::bold(17.0)));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if widgets::plain(ui, &p, "New", None).on_hover_text("New thread (Ctrl+N)").clicked() {
                actions.push(Action::NewThread);
            }
        });
    });
    ui.add_space(8.0);
    search_field(ui, app);
    ui.add_space(6.0);

    let secs = sections(&app.client.state, map, app.ui.grouping, &app.ui.search, app.ui.muted_open, now);
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
    egui::ScrollArea::vertical().id_salt("sidebar-list").max_height(list_h).auto_shrink([false, false]).show_viewport(ui, |ui, view| {
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
                Line::Row(s, r) => row(ui, app, s, *r, rect, selected.as_ref(), actions),
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
    let resp = ui.interact(settings, egui::Id::unique("settings-button"), Sense::click()).on_hover_text("Settings (Ctrl+,)");
    if resp.hovered() {
        ui.painter().rect_filled(settings, 5.0, p.faint);
    }
    icons::gear(ui.painter(), pos2(settings.left() + 12.0, settings.center().y), 15.0, p.text2);
    let g = ui.fonts_mut(|f| f.layout_no_wrap("Settings".to_owned(), theme::ui(size::UI), p.text));
    ui.painter().galley(pos2(settings.left() + 26.0, settings.center().y - g.size().y / 2.0), g, p.text);
    if resp.clicked() {
        actions.push(Action::Settings(true));
    }
    let status = hub_status(&app.client.state);
    let g = ui.fonts_mut(|f| f.layout_no_wrap(status, theme::ui(size::SMALL), p.text2));
    ui.painter().galley(pos2(rect.right() - g.size().x, rect.center().y - g.size().y / 2.0), g, p.text2);
}

fn search_field(ui: &mut egui::Ui, app: &mut YaacApp) {
    let p = app.p;
    let frame = widgets::faint_frame(&p).inner_margin(Margin { left: 30, right: 32, top: 5, bottom: 5 });
    let inner = frame.show(ui, |ui| {
        let te = egui::TextEdit::singleline(&mut app.ui.search)
            .frame(egui::Frame::NONE)
            .hint_text("Search…")
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
    egui::Popup::from_response(&resp)
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
            ui.checkbox(&mut g.show_archived, "Show archived");
        });
    if !open && app.ui.menu == Some(Menu::Grouping) {
        app.ui.menu = None;
    }
}

fn header(ui: &mut egui::Ui, app: &mut YaacApp, s: &Section, rect: Rect, actions: &mut Vec<Action>) {
    let p = app.p;
    let Some(h) = &s.header else { return };
    let font = theme::ui(size::SMALL);
    let y = rect.bottom() - 7.0;
    let mut x = rect.left() + 4.0;
    if s.kind == SectionKind::NeedsYou {
        icons::mark(ui.painter(), pos2(x + 6.0, y - 6.0), 12.0, Mark::NeedsYou, &p);
        x += 18.0;
    }
    if h.temporary {
        icons::dashed_circle(ui.painter(), pos2(x + 6.0, y - 6.0), 5.0, egui::Stroke::new(1.2, p.text2));
        x += 16.0;
        let resp = ui.interact(rect, egui::Id::unique(("temp-header", s.id.as_str())), Sense::click());
        if resp.secondary_clicked() {
            app.ui.menu = Some(Menu::TempProject(s.id.clone()));
        }
        let mut open = app.ui.menu == Some(Menu::TempProject(s.id.clone()));
        if open {
            egui::Popup::from_response(&resp)
                .kind(egui::PopupKind::Menu)
                .open_bool(&mut open)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
                .show(|ui| {
                    if ui.button("Create project…").clicked()
                        && let Some((target, folder)) = &h.folder
                    {
                        app.ui.create_project = Some(create_project_for(app, target, folder));
                    }
                });
            if !open && app.ui.menu == Some(Menu::TempProject(s.id.clone())) {
                app.ui.menu = None;
            }
        }
    }
    if s.kind == SectionKind::Muted {
        icons::chevron(ui.painter(), pos2(x + 5.0, y - 6.0), 12.0, !s.collapsed, p.text2);
        x += 16.0;
        let resp = ui.interact(rect, egui::Id::unique("muted-header"), Sense::click());
        if resp.clicked() {
            app.ui.muted_open = !app.ui.muted_open;
        }
    }
    let mut rx = rect.right() - 4.0;
    if h.retry {
        let g = ui.fonts_mut(|f| f.layout_no_wrap("Retry".to_owned(), font.clone(), p.link));
        let r = Rect::from_min_size(pos2(rx - g.size().x, y - g.size().y), g.size());
        ui.painter().galley(r.min, g, p.link);
        let retry = ui.interact(r, egui::Id::unique(("retry", s.id.as_str())), Sense::click());
        if retry.hovered() || retry.has_focus() {
            ui.painter().hline(r.x_range(), r.bottom(), egui::Stroke::new(1.0, p.link));
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
    let color = if s.kind == SectionKind::NeedsYou { p.text } else { p.text2 };
    let left_font = if s.kind == SectionKind::NeedsYou { theme::bold(size::SMALL) } else { font };
    let mut job = egui::text::LayoutJob::simple_singleline(h.left.clone(), left_font, color);
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
    }
}

fn row(ui: &mut egui::Ui, app: &mut YaacApp, s: &Section, r: usize, rect: Rect, selected: Option<&yaac_core::ThreadKey>, actions: &mut Vec<Action>) {
    let p = app.p;
    let row = &s.rows[r];
    let id = egui::Id::unique(("row", s.id.as_str(), row.key.to_string()));
    let resp = ui.interact(rect, id, Sense::click());
    let is_sel = selected == Some(&row.key) && app.client.state.selection == Selection::Thread(row.key.clone());
    let fill_rect = rect.shrink2(vec2(0.0, 1.0));
    if is_sel {
        ui.painter().rect_filled(fill_rect, 5.0, p.selected);
    } else if resp.hovered() {
        ui.painter().rect_filled(fill_rect, 5.0, p.faint);
    }
    let cy = rect.top() + 14.0;
    icons::mark(ui.painter(), pos2(rect.left() + 13.0, cy), 15.0, row.display.mark, &p);
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
    };
    let g = ui.fonts_mut(|f| f.layout_no_wrap(right_text, font.clone(), p.text2));
    let rx = rect.right() - 8.0 - g.size().x;
    ui.painter().galley(pos2(rx, cy - g.size().y / 2.0), g, p.text2);
    if let Some(l2) = &row.line2 {
        // Line 2: branch, then file counts in git colours, then commits ahead and behind; the
        // workers sit at the right edge, under the time. Every count only when non-zero.
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
        let count = |n: u32| ui.fonts_mut(|f| f.layout_no_wrap(n.to_string(), small.clone(), p.text2)).size().x;
        let mut tail = 0.0;
        for (n, w) in [(l2.added, 14.0), (l2.modified, 14.0), (l2.deleted, 14.0), (l2.ahead, 13.0), (l2.behind, 13.0)] {
            if n > 0 {
                tail += w + count(n) + 6.0;
            }
        }
        let mut x = title_x;
        icons::branch(ui.painter(), pos2(x + 5.0, y2), 12.0, p.text2);
        x += 13.0;
        let g = widgets::elided(ui, &l2.branch, &small, p.text2, (right - x - tail - 6.0).max(30.0));
        let gw = g.size().x;
        ui.painter().galley(pos2(x, y2 - g.size().y / 2.0), g, p.text2);
        x += gw + 8.0;
        let parts: [(u32, egui::Color32, u8); 5] =
            [(l2.added, p.git_added, b'+'), (l2.modified, p.git_modified, b'.'), (l2.deleted, p.git_deleted, b'-'), (l2.ahead, p.text2, b'u'), (l2.behind, p.text2, b'd')];
        for (n, col, kind) in parts {
            if n == 0 || x > right - 10.0 {
                continue;
            }
            match kind {
                b'u' => icons::arrow_vertical(ui.painter(), pos2(x + 5.0, y2), 11.0, col, true),
                b'd' => icons::arrow_vertical(ui.painter(), pos2(x + 5.0, y2), 11.0, col, false),
                k => icons::git_square(ui.painter(), pos2(x + 5.0, y2), 12.0, col, k as char),
            }
            x += 12.0;
            let g = ui.fonts_mut(|f| f.layout_no_wrap(n.to_string(), small.clone(), col));
            let gw = g.size().x;
            ui.painter().galley(pos2(x, y2 - g.size().y / 2.0), g, col);
            x += gw + 7.0;
        }
    }
    let color = if s.kind == SectionKind::Muted { p.text2 } else { p.text };
    let tg = widgets::elided(ui, &row.title, &theme::ui(size::UI), color, rx - title_x - 8.0);
    ui.painter().galley(pos2(title_x, cy - tg.size().y / 2.0), tg, color);
    if resp.clicked() {
        actions.push(if s.kind == SectionKind::NeedsYou { Action::OpenRequest(row.key.clone()) } else { Action::Open(row.key.clone()) });
    }
    if resp.secondary_clicked() {
        app.ui.menu = Some(Menu::Context(row.key.clone()));
    }
    let mut open = app.ui.menu == Some(Menu::Context(row.key.clone())) && s.kind != SectionKind::NeedsYou;
    if open {
        egui::Popup::from_response(&resp)
            .kind(egui::PopupKind::Menu)
            .align(egui::RectAlign::RIGHT_START)
            .open_bool(&mut open)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
            .show(|ui| {
                ui.set_min_width(140.0);
                let pinned = app.client.state.is_pinned(&row.key);
                if ui.button(if pinned { "Unpin" } else { "Pin" }).clicked() {
                    actions.push(Action::SetPinned(row.key.clone(), !pinned));
                }
                let muted = row.muted;
                if ui.button(if muted { "Unmute" } else { "Mute" }).clicked() {
                    actions.push(Action::SetMuted(row.key.clone(), !muted));
                }
                if ui.button("Archive").clicked() {
                    actions.push(Action::Archive(row.key.clone()));
                }
                if ui.button("Rename").clicked() {
                    app.ui.rename = Some((row.key.clone(), row.title.clone()));
                }
                if ui.button("Fork").clicked() {
                    actions.push(Action::OpenFork(row.key.clone()));
                }
            });
        if !open && app.ui.menu == Some(Menu::Context(row.key.clone())) {
            app.ui.menu = None;
        }
    }
}

fn demo_menu(ui: &mut egui::Ui, app: &mut YaacApp, resp: &egui::Response, actions: &mut Vec<Action>) {
    let mut open = app.ui.menu == Some(Menu::Demo);
    egui::Popup::from_response(resp)
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
