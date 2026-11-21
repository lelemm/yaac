//! First run (screen 07), and the page shown when this window is older than the hub.

use eframe::egui::{self, Margin, RichText};
use yaac_core::attention::Mark;
use yaac_core::settings::Page;

use crate::app::{Action, YaacApp};
use crate::theme::{self, Palette, size};
use crate::widgets;

fn card<R>(ui: &mut egui::Ui, p: &Palette, width: f32, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::NONE
        .fill(p.panel)
        .corner_radius(6)
        .inner_margin(Margin::same(18))
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.set_width(width);
                ui.set_min_height(190.0);
                add(ui)
            })
            .inner
        })
        .inner
}

/// The centre of a fresh install: what YAAC is, this computer's providers, and the one next
/// step.
pub(crate) fn show(ui: &mut egui::Ui, app: &mut YaacApp, actions: &mut Vec<Action>) {
    let p = app.p;
    let v = yaac_core::first_run::first_run(&app.client.state);
    if ui.input(|i| i.key_pressed(egui::Key::Enter)) && v.can_start && ui.memory(|m| m.focused().is_none()) {
        actions.push(Action::NewThread);
    }
    let avail = ui.available_size();
    let card_w = ((avail.x - 200.0) / 3.0).clamp(200.0, 280.0);
    let total_w = card_w * 3.0 + 2.0 * 16.0 + 3.0 * 36.0;
    ui.add_space((avail.y * 0.16).max(24.0));
    ui.vertical_centered(|ui| {
        ui.label(RichText::new("YAAC").font(theme::bold(30.0)));
        ui.add_space(6.0);
        ui.label(RichText::new("Run your coding agents on any machine, and let them work together.").font(theme::ui(size::PROSE)).color(p.text2));
    });
    ui.add_space(30.0);
    ui.horizontal(|ui| {
        ui.add_space(((avail.x - total_w) / 2.0).max(0.0));
        ui.spacing_mut().item_spacing.x = 16.0;
        ui.style_mut().spacing.item_spacing.y = 6.0;
        card(ui, &p, card_w, |ui| {
            ui.label(RichText::new("This computer").font(theme::bold(size::SECTION)));
            ui.add_space(8.0);
            if v.checking {
                ui.label(widgets::secondary("Checking what is installed…", &p));
            }
            for line in &v.providers {
                ui.horizontal(|ui| {
                    widgets::mark(ui, &p, if line.ready { Mark::Done } else { Mark::NeedsYou }, 15.0);
                    ui.label(format!("{} · {}", line.name, line.word));
                });
                if let Some(h) = &line.hint {
                    ui.horizontal(|ui| {
                        ui.add_space(20.0);
                        widgets::rich_inline(ui, &p, h, p.text2, size::SMALL);
                    });
                }
            }
            if !v.providers.is_empty() {
                ui.horizontal(|ui| {
                    ui.add_space(14.0);
                    if widgets::plain(ui, &p, "Refresh", None).on_hover_text("Check this computer again").clicked()
                        && let Some(t) = &v.local
                    {
                        actions.push(Action::RefreshProviders(t.clone(), None));
                    }
                });
            }
            ui.add_space(8.0);
            if v.can_start {
                let r = widgets::primary(ui, &p, "Start a thread", Some("Enter"));
                if r.clicked() {
                    actions.push(Action::NewThread);
                }
            } else {
                widgets::unavailable(ui, &p, "Start a thread", "no provider is ready yet");
            }
        });
        card(ui, &p, card_w, |ui| {
            ui.label(RichText::new("Add a server").font(theme::bold(size::SECTION)));
            ui.add_space(8.0);
            ui.label(RichText::new("Run agents on another machine over SSH.").color(p.text2));
            ui.add_space(12.0);
            if widgets::plain(ui, &p, "Add server…", None).clicked() {
                actions.push(Action::SettingsPage(Page::Targets));
                app.ui.set.ssh = Some(crate::settings::SshPanel::new(app.now(), None));
            }
        });
        card(ui, &p, card_w, |ui| {
            ui.label(RichText::new("Turn on WSL").font(theme::bold(size::SECTION)));
            ui.add_space(8.0);
            ui.label(RichText::new("Run agents inside a Linux distro on this PC.").color(p.text2));
            ui.add_space(12.0);
            if app.host_windows {
                if widgets::plain(ui, &p, "Turn on…", None).clicked() {
                    actions.push(Action::WslOn(true));
                    actions.push(Action::SettingsPage(Page::Targets));
                }
            } else {
                widgets::unavailable(ui, &p, "Turn on…", "Windows only");
            }
        });
    });
    ui.add_space(26.0);
    ui.vertical_centered(|ui| {
        ui.label(widgets::secondary("Agents keep running when you close this window.", &p));
    });
}

/// This window is older than the running hub: say so, and attach to nothing.
pub(crate) fn update_needed(ui: &mut egui::Ui, p: &Palette, message: &str) {
    let r = ui.max_rect();
    ui.painter().rect_filled(r, 0.0, p.bg);
    ui.add_space(r.height() * 0.3);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new("Update YAAC").font(theme::bold(size::TITLE + 4.0)));
        ui.add_space(8.0);
        ui.label(message);
        ui.add_space(4.0);
        ui.label(widgets::secondary("A newer YAAC is already running on this computer. This window will not attach to it; nothing was changed.", p));
        ui.add_space(4.0);
    });
}
