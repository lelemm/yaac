//! Settings: a full-window page with its own navigation. The pages themselves are a later
//! package; Appearance (the theme) works now.

use eframe::egui::{self, Margin};

use crate::app::{Action, YaacApp};
use crate::theme::{self, ThemeChoice, size};
use crate::widgets;

pub(crate) fn show(ui: &mut egui::Ui, app: &mut YaacApp, actions: &mut Vec<Action>) {
    let p = app.p;
    egui::CentralPanel::no_frame().show(ui, |ui| {
        ui.painter().rect_filled(ui.max_rect(), 0.0, p.bg);
        egui::Panel::left("settings-nav")
            .exact_size(220.0)
            .resizable(false)
            .frame(egui::Frame::NONE.fill(p.panel).inner_margin(Margin::symmetric(12, 12)))
            .show(ui, |ui| {
                let back = widgets::plain(ui, &p, "     Back to threads", Some("Esc"));
                crate::icons::arrow_left(ui.painter(), eframe::egui::pos2(back.rect.left() + 18.0, back.rect.center().y), 14.0, p.text);
                if back.clicked() {
                    actions.push(Action::Settings(false));
                }
                ui.add_space(12.0);
                for page in ["Targets", "Providers", "Appearance", "Advanced"] {
                    let sel = page == "Appearance";
                    ui.add(egui::Button::selectable(sel, page));
                }
            });
        egui::CentralPanel::no_frame().show(ui, |ui| {
            egui::Frame::NONE.inner_margin(Margin::same(28)).show(ui, |ui| {
                ui.label(egui::RichText::new("Appearance").font(theme::bold(size::TITLE)));
                ui.add_space(10.0);
                ui.label("Theme");
                for (choice, label) in [(ThemeChoice::System, "Follow the system"), (ThemeChoice::Dark, "Dark"), (ThemeChoice::Light, "Light")] {
                    if ui.radio(app.ui.theme == choice, label).clicked() {
                        actions.push(Action::Theme(choice));
                    }
                }
                ui.add_space(20.0);
                ui.label(widgets::secondary("Targets, Providers, and Advanced arrive in a later package.", &p));
            });
        });
    });
    if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
        actions.push(Action::Settings(false));
    }
}
