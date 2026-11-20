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
            if !confirm.has_focus() && !cancel.has_focus() {
                cancel.request_focus();
            }
        });
    });
}

/// The filled button in the failed colour.
fn danger(ui: &mut egui::Ui, p: &Palette, label: &str) -> egui::Response {
    let font = theme::bold(size::UI);
    let g = ui.fonts_mut(|f| f.layout_no_wrap(label.to_owned(), font, p.on_accent));
    let (rect, resp) = ui.allocate_exact_size(vec2(g.size().x + 24.0, 28.0), egui::Sense::click());
    let fill = if resp.hovered() { p.failed.gamma_multiply(0.9) } else { p.failed };
    ui.painter().rect_filled(rect, 5.0, fill);
    ui.painter().galley(egui::pos2(rect.left() + 12.0, rect.center().y - g.size().y / 2.0), g, p.on_accent);
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
        egui::Popup::from_response(&r).kind(egui::PopupKind::Menu).open_bool(&mut open).close_behavior(egui::PopupCloseBehavior::CloseOnClick).show(|ui| {
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
        ui.add_space(14.0);
        let enter = ui.input(|i| i.key_pressed(egui::Key::Enter)) && app.ui.menu.is_none();
        let esc = ui.input(|i| i.key_pressed(egui::Key::Escape)) && app.ui.menu.is_none();
        ui.horizontal(|ui| {
            if v.unavailable.is_none() {
                if widgets::primary(ui, &p, "Fork", Some("Enter")).clicked() || enter {
                    actions.push(Action::Fork(d.key.clone(), d.choice.clone()));
                    keep = false;
                }
            } else {
                widgets::unavailable(ui, &p, "Fork", "");
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
