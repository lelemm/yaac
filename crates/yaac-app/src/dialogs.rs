//! Dialogs over the shell: the fork dialog (screen 11) and the confirmation of a mode that
//! bypasses approvals (screen 30). Each dims the shell with the scrim and has one filled button.

use eframe::egui::{self, Align2, Margin, RichText, vec2};
use yaac_core::fork::ForkChoice;

use crate::app::{Action, Menu, YaacApp};
use crate::icons;
use crate::theme::{self, Palette, size};
use crate::widgets;

/// The scrim behind a dialog: dims the shell and swallows clicks.
pub(crate) fn scrim(ctx: &egui::Context, p: &Palette, id: &str) {
    egui::Area::new(egui::Id::unique(("scrim", id))).order(egui::Order::Middle).fixed_pos(egui::pos2(0.0, 0.0)).interactable(true).show(ctx, |ui| {
        let r = ctx.content_rect();
        ui.painter().rect_filled(r, 0.0, p.scrim);
        ui.allocate_rect(r, egui::Sense::click());
    });
}

fn window(title: &str, p: &Palette, ctx: &egui::Context) -> egui::Window<'static> {
    egui::Window::new(title.to_owned())
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .order(egui::Order::Foreground)
        .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
        .frame(egui::Frame::popup(&ctx.global_style()).fill(p.raised).inner_margin(Margin::same(20)))
}

/// Screen 30: "Let this agent act without asking?" Cancel is the focused default (Esc
/// cancels); Enter does not confirm; the confirming action is the one filled button, in the
/// failed colour.
pub(crate) fn mode_confirm(ctx: &egui::Context, app: &mut YaacApp, actions: &mut Vec<Action>) {
    let p = app.p;
    let Some(pending) = app.client.state.mode_confirm.clone() else { return };
    if !yaac_core::mode_confirm::still_relevant(&app.client.state, &pending) {
        actions.push(Action::CancelMode);
        return;
    }
    let v = yaac_core::mode_confirm::view(&app.client.state, &pending);
    scrim(ctx, &p, "mode");
    // Esc cancels; Enter is swallowed so it can never confirm.
    let esc = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
    ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
    if esc {
        actions.push(Action::CancelMode);
        return;
    }
    window("mode-confirm", &p, ctx).show(ctx, |ui| {
        ui.set_width(440.0);
        ui.horizontal(|ui| {
            widgets::icon(ui, 20.0, |pa, c, d| icons::warning(pa, c, d, p.needs, p.raised));
            ui.label(RichText::new(&v.title).font(theme::bold(size::SECTION + 1.5)));
        });
        ui.add_space(10.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.label(&v.body.0);
            ui.label(RichText::new(&v.mode_name).font(theme::bold(size::UI)));
            ui.label(&v.body.1);
        });
        ui.add_space(8.0);
        for l in &v.lines {
            ui.label(widgets::secondary(l, &p));
        }
        ui.add_space(8.0);
        ui.label(widgets::secondary(&v.footer, &p));
        ui.add_space(14.0);
        ui.horizontal(|ui| {
            let cancel = widgets::plain(ui, &p, "Cancel", Some("Esc"));
            if cancel.clicked() {
                actions.push(Action::CancelMode);
            }
            ui.add_space(8.0);
            let confirm = danger(ui, &p, &v.confirm_label);
            if confirm.clicked() {
                actions.push(Action::ConfirmMode);
            }
            // Cancel is the default: it holds the focus unless the user tabbed to the other.
            if !confirm.has_focus() {
                if !cancel.has_focus() {
                    cancel.request_focus();
                }
                widgets::focus_ring(ui, cancel.rect, &p);
            }
        });
    });
}

/// The filled button in the failed colour.
fn danger(ui: &mut egui::Ui, p: &Palette, label: &str) -> egui::Response {
    let font = theme::bold(size::UI);
    let g = ui.fonts_mut(|f| f.layout_no_wrap(label.to_owned(), font, p.on_primary));
    let (rect, resp) = ui.allocate_exact_size(vec2(g.size().x + 24.0, 28.0), egui::Sense::click());
    let fill = if resp.hovered() { p.failed.gamma_multiply(0.9) } else { p.failed };
    ui.painter().rect_filled(rect, 5.0, fill);
    ui.painter().galley(egui::pos2(rect.left() + 12.0, rect.center().y - g.size().y / 2.0), g, p.on_primary);
    if resp.has_focus() {
        widgets::focus_ring(ui, rect, p);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Screen 11: fork this thread.
pub(crate) fn fork(ctx: &egui::Context, app: &mut YaacApp, actions: &mut Vec<Action>) {
    let p = app.p;
    let Some(mut d) = app.ui.fork.take() else { return };
    let Some(v) = yaac_core::fork::view(&app.client.state, &d.key, &d.choice) else { return };
    let mut keep = true;
    scrim(ctx, &p, "fork");
    window("fork", &p, ctx).show(ctx, |ui| {
        ui.set_width(520.0);
        ui.label(RichText::new("Fork this thread").font(theme::bold(size::SECTION + 2.0)));
        ui.add_space(10.0);
        let label = |ui: &mut egui::Ui, t: &str| {
            ui.add_space(6.0);
            ui.label(widgets::secondary(t, &p));
        };
        // The body scrolls; the title and the buttons always show (a handoff's preview and
        // first message make it taller than a small window).
        let body_max = (ctx.content_rect().height() - 200.0).max(200.0);
        egui::ScrollArea::vertical().max_height(body_max).auto_shrink([false, true]).show(ui, |ui| {
            label(ui, "From");
            ui.label(&v.from);
            ui.label(widgets::secondary(&v.at, &p));
            label(ui, "Continue with");
            let (inst, model) = v.chosen.clone().unwrap_or_default();
            let r = widgets::selector_pair(ui, &p, &inst, &model, true);
            if r.clicked() {
                app.ui.toggle_menu(Menu::ForkDest);
            }
            let mut open = app.ui.menu == Some(Menu::ForkDest);
            let mut picked: Option<ForkChoice> = None;
            crate::widgets::menu_popup(egui::Popup::from_response(&r), &p).kind(egui::PopupKind::Menu).open_bool(&mut open).close_behavior(egui::PopupCloseBehavior::CloseOnClick).show(|ui| {
                ui.set_min_width(300.0);
                for dest in &v.destinations {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&dest.name).font(theme::bold(size::UI)));
                        ui.label(widgets::secondary(if dest.native { "native" } else { "handoff" }, &p));
                    });
                    if let Some(reason) = &dest.reason {
                        widgets::unavailable(ui, &p, &dest.name, reason);
                        continue;
                    }
                    for (id, name, default) in &dest.models {
                        let sel = d.choice.instance == dest.instance && d.choice.model.as_ref().map_or(*default, |m| m == id);
                        let text = if *default { format!("{name}   default") } else { name.clone() };
                        if ui.selectable_label(sel, text).clicked() {
                            picked = Some(ForkChoice { instance: dest.instance.clone(), model: Some(id.clone()) });
                        }
                    }
                    ui.add_space(4.0);
                }
            });
            if !open && app.ui.menu == Some(Menu::ForkDest) {
                app.ui.menu = None;
            }
            if let Some(c) = picked {
                app.client.fork_preview(&d.key, &c.instance);
                d.choice = c;
            }
            if let Some(l) = &v.instance_line {
                ui.label(widgets::secondary(l, &p));
            }
            ui.add_space(12.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.label(RichText::new(&v.result_word).font(theme::bold(size::UI)));
                ui.label(&v.result_text);
            });
            if let Some(r) = &v.reason {
                ui.label(widgets::secondary(r, &p));
            }
            ui.add_space(10.0);
            ui.horizontal_wrapped(|ui| {
                ui.label(widgets::secondary("Carried over:", &p));
                ui.label(&v.carried);
            });
            if let Some(s) = &v.size {
                ui.label(widgets::secondary(s, &p));
            }
            if let Some(n) = &v.not_carried {
                ui.horizontal_wrapped(|ui| {
                    ui.label(widgets::secondary("Not carried over:", &p));
                    ui.label(n);
                });
            }
            if let Some(n) = &v.notice {
                ui.add_space(10.0);
                egui::Frame::NONE.fill(p.faint).corner_radius(5).inner_margin(Margin::symmetric(10, 8)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        widgets::icon(ui, 15.0, |pa, c, dd| icons::info(pa, c, dd, p.text2));
                        ui.label(n);
                    });
                });
            }
            if v.preview.is_some() || v.preview_loading {
                ui.add_space(10.0);
                let r = ui.horizontal(|ui| {
                    widgets::icon(ui, 12.0, |pa, c, dd| icons::chevron(pa, c, dd, d.preview_open, p.text2));
                    widgets::link(ui, &p, "Preview transcript", theme::ui(size::UI))
                });
                if r.inner.clicked() {
                    d.preview_open = !d.preview_open;
                }
                if d.preview_open {
                    egui::Frame::NONE.fill(p.faint).corner_radius(5).inner_margin(Margin::same(10)).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        egui::ScrollArea::vertical().max_height(150.0).show(ui, |ui| match &v.preview {
                            Some(t) => {
                                ui.label(RichText::new(t).font(theme::mono(size::SMALL)).color(p.text2));
                            }
                            None => {
                                ui.label(widgets::secondary("Reading the transcript…", &p));
                            }
                        });
                    });
                }
            }
            if let Some(u) = &v.unavailable {
                ui.add_space(8.0);
                ui.label(RichText::new(u).color(p.failed));
            }
            if v.handoff && v.unavailable.is_none() {
                // A handoff creates the new conversation with its first message: it is written
                // here, and sent after the preamble that points the agent at the transcript.
                label(ui, "First message to the new agent");
                ui.add(egui::TextEdit::multiline(&mut d.message).desired_rows(3).desired_width(f32::INFINITY).hint_text("What should it do next?"));
            }
        });
        ui.add_space(14.0);
        // Enter confirms a native fork; a handoff is confirmed only with the button.
        let enter = !v.handoff && ui.input(|i| i.key_pressed(egui::Key::Enter)) && app.ui.menu.is_none();
        let esc = ui.input(|i| i.key_pressed(egui::Key::Escape)) && app.ui.menu.is_none();
        let empty = v.handoff && d.message.trim().is_empty();
        ui.horizontal(|ui| {
            if v.unavailable.is_some() {
                widgets::unavailable(ui, &p, &v.confirm, "");
            } else if v.confirm_waits {
                widgets::unavailable(ui, &p, &v.confirm, "reading the transcript…");
            } else if empty {
                widgets::unavailable(ui, &p, &v.confirm, "write the first message");
            } else if widgets::primary(ui, &p, &v.confirm, (!v.handoff).then_some("Enter")).clicked() || enter {
                let message = v.handoff.then(|| d.message.clone());
                actions.push(Action::Fork(d.key.clone(), d.choice.clone(), message));
                keep = false;
            }
            if widgets::plain(ui, &p, "Cancel", Some("Esc")).clicked() || esc {
                keep = false;
            }
        });
    });
    if keep {
        app.ui.fork = Some(d);
    }
}

/// The Mute dialog (D90): until I unmute it (default), for a duration, or until a date and
/// time — all in the host's local time. One filled button, Mute (Enter); Cancel (Esc). A
/// moment in the past cannot be chosen: Mute is unavailable with the reason beside it.
pub(crate) fn mute(ctx: &egui::Context, app: &mut YaacApp, actions: &mut Vec<Action>) {
    use yaac_core::mute::{DurationUnit, MuteChoice, MuteOutcome};
    let p = app.p;
    let now = app.now();
    let zone = app.zone;
    let Some(mut d) = app.ui.mute.take() else { return };
    scrim(ctx, &p, "mute");
    let esc = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
    if esc {
        return;
    }
    let outcome = d.outcome_in(now, zone);
    let enter = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
    let mut done = false;
    window("mute", &p, ctx).show(ctx, |ui| {
        ui.set_width(420.0);
        ui.label(RichText::new(&d.title).font(theme::bold(size::SECTION + 1.5)));
        ui.add_space(12.0);
        ui.spacing_mut().item_spacing.y = 8.0;
        let radio = |ui: &mut egui::Ui, d: &mut yaac_core::mute::MuteDialog, c: MuteChoice, label: &str| {
            let r = ui.radio(d.choice == c, label);
            if r.clicked() {
                d.choice = c;
            }
        };
        radio(ui, &mut d, MuteChoice::Forever, "Until I unmute it");
        ui.horizontal(|ui| {
            radio(ui, &mut d, MuteChoice::For, "For");
            let r = widgets::field(ui, &p, &mut d.amount, "2", 44.0, false);
            if r.gained_focus() || r.changed() {
                d.choice = MuteChoice::For;
            }
            // A selector (a word and a chevron), not a boxed combo.
            let sel = widgets::selector(ui, &p, d.unit.label(), true);
            crate::widgets::menu_popup(egui::Popup::menu(&sel), &p).show(|ui| {
                for u in DurationUnit::ALL {
                    if ui.selectable_label(u == d.unit, u.label()).clicked() {
                        d.unit = u;
                        d.choice = MuteChoice::For;
                    }
                }
            });
        });
        if let Some(prev) = d.for_preview(now, zone) {
            ui.horizontal(|ui| {
                ui.add_space(26.0);
                ui.label(widgets::secondary(prev, &p));
            });
        }
        ui.horizontal(|ui| {
            radio(ui, &mut d, MuteChoice::Until, "Until");
            // The date is a field with a calendar button (D112); the calendar is a dropdown,
            // never part of the dialog's own layout.
            let r = widgets::field(ui, &p, &mut d.date_text, "2026-11-21", 92.0, true);
            if r.changed() {
                d.date_typed();
            } else if r.gained_focus() {
                d.choice = MuteChoice::Until;
            }
            let cal = widgets::icon_button(ui, &p, 16.0, "Choose a date", icons::calendar);
            if cal.clicked() {
                d.calendar_open = !d.calendar_open;
                d.choice = MuteChoice::Until;
                d.month = (d.date.0, d.date.1);
            }
            ui.add_space(6.0);
            let r = widgets::field(ui, &p, &mut d.time, "14:30", 64.0, true);
            if r.gained_focus() || r.changed() {
                d.choice = MuteChoice::Until;
            }
            if d.calendar_open {
                let mut open = true;
                widgets::menu_popup(egui::Popup::from_response(&cal), &p)
                    .kind(egui::PopupKind::Popup)
                    .id(egui::Id::unique("mute-calendar"))
                    .open_bool(&mut open)
                    .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                    .show(|ui| month_grid(ui, &p, &mut d, now, zone));
                if !open {
                    d.calendar_open = false;
                }
            }
        });
        ui.add_space(4.0);
        ui.label(widgets::secondary("The agent keeps working. If it gets blocked it still comes to the top of its group.", &p));
        ui.add_space(10.0);
        ui.horizontal(|ui| match &outcome {
            MuteOutcome::Mute(until) => {
                if widgets::primary(ui, &p, "Mute", Some("Enter")).clicked() || enter {
                    actions.push(Action::MuteUntil(d.key.clone(), *until));
                    done = true;
                }
                if widgets::plain(ui, &p, "Cancel", Some("Esc")).clicked() {
                    done = true;
                }
            }
            MuteOutcome::Unavailable(why) => {
                widgets::unavailable(ui, &p, "Mute", why);
                if widgets::plain(ui, &p, "Cancel", Some("Esc")).clicked() {
                    done = true;
                }
            }
        });
    });
    if !done {
        app.ui.mute = Some(d);
    }
}

/// The calendar dropdown in the calm style: the month and two arrows, weekday initials, six
/// rows of days; past days unavailable, the chosen day on the selected-row tint.
fn month_grid(ui: &mut egui::Ui, p: &Palette, d: &mut yaac_core::mute::MuteDialog, now: i64, zone: yaac_core::time::Zone) {
    use yaac_core::time;
    let cell = vec2(30.0, 24.0);
    let cell_size = cell;
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing = vec2(0.0, 2.0);
        ui.horizontal(|ui| {
            let arrow = |ui: &mut egui::Ui, forward: bool| {
                let (r, resp) = ui.allocate_exact_size(cell, egui::Sense::click());
                if resp.hovered() || resp.has_focus() {
                    ui.painter().rect_filled(r.shrink(1.0), 5.0, p.faint);
                }
                icons::nav(ui.painter(), r.center(), 14.0, forward, p.text);
                resp
            };
            let back = arrow(ui, false);
            ui.add_sized(vec2(cell.x * 5.0, cell.y), egui::Label::new(RichText::new(time::month_title(d.month.0, d.month.1)).font(theme::bold(size::UI))));
            let fwd = arrow(ui, true);
            if back.clicked() {
                d.shift_month(-1);
            }
            if fwd.clicked() {
                d.shift_month(1);
            }
        });
        ui.horizontal(|ui| {
            for w in ["M", "T", "W", "T", "F", "S", "S"] {
                ui.add_sized(cell, egui::Label::new(widgets::secondary(w, p)));
            }
        });
        // Always six week rows: the dropdown never changes size between months (D112).
        for week in d.month_cells() {
            ui.horizontal(|ui| {
                for cell in week {
                    let (r, resp) = ui.allocate_exact_size(cell_size, egui::Sense::click());
                    let Some(day) = cell else { continue };
                    let past = d.day_is_past(day, now, zone);
                    let chosen = d.date == (d.month.0, d.month.1, day);
                    if chosen {
                        ui.painter().rect_filled(r.shrink(1.0), 5.0, p.selected);
                    } else if resp.hovered() && !past {
                        ui.painter().rect_filled(r.shrink(1.0), 5.0, p.faint);
                    }
                    let c = if past { p.unavailable } else { p.text };
                    ui.painter().text(r.center(), Align2::CENTER_CENTER, day.to_string(), theme::ui(size::SMALL + 0.5), c);
                    if resp.clicked() && !past {
                        d.pick_day(day);
                    }
                }
            });
        }
    });
}

/// D110: "Delete <title>?" with "Also delete the session from <harness>" (checked by
/// default) when the harness can delete its own sessions. The confirming button is the one
/// filled button, in the failed colour; Cancel is the focused default and Esc cancels.
pub(crate) fn delete(ctx: &egui::Context, app: &mut YaacApp, actions: &mut Vec<Action>) {
    let p = app.p;
    let Some(mut d) = app.ui.delete.take() else { return };
    scrim(ctx, &p, "delete");
    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
        return;
    }
    ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
    let mut keep = true;
    window("delete", &p, ctx).show(ctx, |ui| {
        ui.set_width(440.0);
        ui.label(RichText::new(&d.title).font(theme::bold(size::SECTION + 1.5)));
        ui.add_space(10.0);
        if let Some(label) = d.harness_label.clone() {
            ui.checkbox(&mut d.also_in_harness, label);
            ui.add_space(6.0);
        }
        ui.label(widgets::secondary(d.body(), &p));
        ui.add_space(14.0);
        ui.horizontal(|ui| {
            let cancel = widgets::plain(ui, &p, "Cancel", Some("Esc"));
            if cancel.clicked() {
                keep = false;
            }
            ui.add_space(8.0);
            let confirm = danger(ui, &p, "Delete");
            if confirm.clicked() {
                actions.push(Action::Delete(d.key.clone(), d.also_in_harness()));
                keep = false;
            }
            if !confirm.has_focus() && !ui.ctx().memory(|m| m.focused().is_some()) {
                cancel.request_focus();
            }
        });
    });
    if keep {
        app.ui.delete = Some(d);
    }
}

/// D111: "Archive <title>?" — Archive (Enter) or Cancel (Esc).
pub(crate) fn archive(ctx: &egui::Context, app: &mut YaacApp, actions: &mut Vec<Action>) {
    let p = app.p;
    let Some(d) = app.ui.archive.take() else { return };
    scrim(ctx, &p, "archive");
    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
        return;
    }
    let enter = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
    let mut keep = true;
    window("archive", &p, ctx).show(ctx, |ui| {
        ui.set_width(420.0);
        ui.label(RichText::new(&d.title).font(theme::bold(size::SECTION + 1.5)));
        ui.add_space(10.0);
        ui.label(widgets::secondary(d.body, &p));
        ui.add_space(14.0);
        ui.horizontal(|ui| {
            if widgets::primary(ui, &p, "Archive", Some("Enter")).clicked() || enter {
                actions.push(Action::Archive(d.key.clone()));
                keep = false;
            }
            if widgets::plain(ui, &p, "Cancel", Some("Esc")).clicked() {
                keep = false;
            }
        });
    });
    if keep {
        app.ui.archive = Some(d);
    }
}
