//! Back and forward beside the `YAAC` title (decisions D87, D88): two small icon buttons that
//! work like a browser's. A click goes one step; holding a button (about 400 ms) or
//! right-clicking it lists the places in that direction, nearest first, to jump several at
//! once. A button with nowhere to go is drawn in the unavailable colour, does nothing, and has
//! no list.

use egui::{self, Sense, pos2, vec2};
use yaac_core::history::{Entry, Place};

use crate::app::{Action, YaacApp};
use crate::icons;
use crate::theme::{self, Palette, size};

/// Hold this long to open the list.
const HOLD: f64 = 0.4;

/// The two buttons, side by side; `compact` makes them narrower (the collapsed sidebar strip).
pub(crate) fn buttons(ui: &mut egui::Ui, app: &mut YaacApp, actions: &mut Vec<Action>, compact: bool) {
    let p = app.p;
    let exists = |pl: &Place| app.place_exists(pl);
    let can = [app.ui.history.can_back(&exists), app.ui.history.can_forward(&exists)];
    let mut draw = |ui: &mut egui::Ui, forward: bool| {
        let enabled = can[usize::from(forward)];
        let side = if compact { 20.0 } else { 24.0 };
        let (rect, resp) = ui.allocate_exact_size(vec2(side, 24.0), if enabled { Sense::click() } else { Sense::hover() });
        let tip = if forward { crate::commander::tip(app, "Forward", yaac_core::commands::Cmd::Forward) } else { crate::commander::tip(app, "Back", yaac_core::commands::Cmd::Back) } + " · hold for the list";
        let resp = if enabled { resp.on_hover_text(tip) } else { resp };
        if enabled && (resp.hovered() || resp.has_focus() || app.ui.nav_menu == Some(forward)) {
            ui.painter().rect_filled(rect, 5.0, p.faint);
        }
        let color = if enabled { p.text } else { p.unavailable };
        icons::nav(ui.painter(), rect.center(), if compact { 15.0 } else { 16.0 }, forward, color);
        if resp.has_focus() {
            crate::widgets::focus_ring(ui, rect, &p);
        }
        if !enabled {
            return;
        }
        // Hold to open the list.
        if resp.is_pointer_button_down_on() {
            let now = ui.input(|i| i.time);
            let held = ui.input(|i| i.pointer.press_start_time()).map_or(0.0, |s| now - s);
            if held >= HOLD {
                if app.ui.nav_menu != Some(forward) {
                    app.ui.nav_menu = Some(forward);
                    app.ui.nav_sel = 0;
                    app.ui.nav_held = true;
                }
            } else {
                // One wake-up when the hold would open the list; not a running loop.
                ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(HOLD - held));
            }
        }
        if resp.secondary_clicked() {
            app.ui.nav_menu = Some(forward);
            app.ui.nav_sel = 0;
        }
        if resp.clicked() {
            if app.ui.nav_held {
                app.ui.nav_held = false;
            } else {
                actions.push(Action::Nav(if forward { 1 } else { -1 }));
            }
        }
        if app.ui.nav_menu == Some(forward) {
            menu(ui, app, rect, forward, actions);
        }
    };
    // Side by side, also in the strip (D103), where they are narrower.
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = if compact { 0.0 } else { 2.0 };
        draw(ui, false);
        draw(ui, true);
    });
}

/// How an entry reads in the list: a thread's title with its machine; Settings and drafts by
/// name.
pub(crate) fn label(app: &YaacApp, e: &Entry) -> (String, Option<String>) {
    match &e.place {
        Place::Thread(k) => {
            let st = &app.client.state;
            let title = st.summary(k).map_or_else(|| k.thread.to_string(), |s| s.title.clone());
            let machine = st.target(&k.target).map_or_else(|| k.target.to_string(), |t| t.entry.name.to_string());
            (title, Some(machine))
        }
        Place::Settings(page) => (format!("Settings · {}", page.label()), None),
        Place::Draft(_) => ("New thread".to_owned(), None),
    }
}

/// The list under a button: nearest first, at most ten; arrows and Enter choose, Escape or a
/// click elsewhere closes. No outline: a raised surface with a soft shadow, rows with a faint
/// fill under the pointer or the keyboard.
fn menu(ui: &mut egui::Ui, app: &mut YaacApp, button: egui::Rect, forward: bool, actions: &mut Vec<Action>) {
    let p: Palette = app.p;
    let exists = |pl: &Place| app.place_exists(pl);
    let rows: Vec<(isize, String, Option<String>)> =
        app.ui.history.menu(forward, &exists).into_iter().map(|(n, e)| {
            let (t, m) = label(app, e);
            (n, t, m)
        }).collect();
    if rows.is_empty() {
        app.ui.nav_menu = None;
        return;
    }
    let ctx = ui.ctx().clone();
    let (down, up, enter, esc) = ctx.input_mut(|i| {
        (
            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
            i.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
            i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
        )
    });
    if down {
        app.ui.nav_sel = (app.ui.nav_sel + 1).min(rows.len() - 1);
    }
    if up {
        app.ui.nav_sel = app.ui.nav_sel.saturating_sub(1);
    }
    if esc {
        app.ui.nav_menu = None;
        return;
    }
    if enter {
        actions.push(Action::Nav(rows[app.ui.nav_sel.min(rows.len() - 1)].0));
        app.ui.nav_menu = None;
        return;
    }
    let area = egui::Area::new(egui::Id::unique(("nav-menu", forward)))
        .order(egui::Order::Foreground)
        .fixed_pos(pos2(button.left(), button.bottom() + 4.0))
        .show(&ctx, |ui| {
            egui::Frame::popup(ui.style()).fill(p.raised).corner_radius(8).inner_margin(egui::Margin::same(6)).show(ui, |ui| {
                ui.set_width(260.0);
                ui.spacing_mut().item_spacing.y = 0.0;
                for (i, (n, title, machine)) in rows.iter().enumerate() {
                    let h = if machine.is_some() { 40.0 } else { 30.0 };
                    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::click());
                    if resp.hovered() {
                        app.ui.nav_sel = i;
                    }
                    if i == app.ui.nav_sel {
                        ui.painter().rect_filled(r, 5.0, p.faint);
                    }
                    let w = r.width() - 20.0;
                    let g = crate::widgets::elided(ui, title, &theme::ui(size::UI), p.text, w);
                    let top = if machine.is_some() { r.top() + 4.0 } else { r.center().y - g.size().y / 2.0 };
                    ui.painter().galley(pos2(r.left() + 10.0, top), g, p.text);
                    if let Some(m) = machine {
                        let g = crate::widgets::elided(ui, m, &theme::ui(size::SMALL), p.text2, w);
                        ui.painter().galley(pos2(r.left() + 10.0, r.top() + 22.0), g, p.text2);
                    }
                    if resp.clicked() {
                        actions.push(Action::Nav(*n));
                    }
                }
            });
        });
    let pressed_outside = ctx.input(|i| i.pointer.any_pressed())
        && ctx.pointer_interact_pos().is_some_and(|pos| !area.response.rect.contains(pos) && !button.contains(pos));
    if pressed_outside || actions.iter().any(|a| matches!(a, Action::Nav(_))) {
        app.ui.nav_menu = None;
    }
}
