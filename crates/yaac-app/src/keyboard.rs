//! Settings → Keyboard (D131): every command, grouped as in the catalogue, with its keys. Click
//! a key to record a new chord (Esc cancels, Backspace or Delete clears), reset one row, or
//! reset all. A chord another command has is flagged, naming it, until it is resolved with
//! Replace or Cancel. Only differences from the defaults are stored (`keyBindings`).

use eframe::egui::{self, Event, Key, RichText};
use yaac_core::commands::{self as cmds, COMMANDS, Cmd, Group, Scope};
use yaac_core::keys::{Chord, MouseButton};

use crate::app::YaacApp;
use crate::theme::{self, size};
use crate::widgets;

/// The page's own state.
#[derive(Clone, Debug, Default)]
pub(crate) struct KeysUi {
    /// The command whose next chord is being recorded.
    pub recording: Option<Cmd>,
    /// A recorded chord another command has: (for, chord, the other).
    pub conflict: Option<(Cmd, Chord, Cmd)>,
}

/// While recording: the next key press (or mouse side button) is the chord. Esc cancels,
/// Backspace or Delete alone clears the command's keys.
pub(crate) fn capture(app: &mut YaacApp, ctx: &egui::Context) {
    let Some(cmd) = app.ui.set.keys.recording else { return };
    let mut got: Option<Option<Chord>> = None;
    let mut cancel = false;
    ctx.input_mut(|i| {
        i.events.retain(|e| {
            if got.is_some() || cancel {
                return true;
            }
            match e {
                Event::Key { key, pressed: true, modifiers, .. } => {
                    let plain = !modifiers.ctrl && !modifiers.command && !modifiers.alt && !modifiers.shift;
                    if *key == Key::Escape && plain {
                        cancel = true;
                    } else if matches!(key, Key::Backspace | Key::Delete) && plain {
                        got = Some(None);
                    } else {
                        got = Some(Some(crate::commander::chord_of(*key, *modifiers)));
                    }
                    false
                }
                Event::PointerButton { button: egui::PointerButton::Extra1, pressed: true, .. } => {
                    got = Some(Some(Chord::mouse(MouseButton::Back)));
                    false
                }
                Event::PointerButton { button: egui::PointerButton::Extra2, pressed: true, .. } => {
                    got = Some(Some(Chord::mouse(MouseButton::Forward)));
                    false
                }
                // Typed text is not a chord.
                Event::Text(_) => false,
                _ => true,
            }
        });
    });
    if cancel {
        app.ui.set.keys.recording = None;
        return;
    }
    match got {
        None => {}
        Some(None) => {
            app.ui.bindings.clear(cmd);
            app.ui.set.keys.recording = None;
        }
        Some(Some(chord)) => {
            app.ui.set.keys.recording = None;
            record(app, cmd, chord);
        }
    }
}

/// Give a recorded chord to a command, or flag the command that already has it.
pub(crate) fn record(app: &mut YaacApp, cmd: Cmd, chord: Chord) {
    match app.ui.bindings.conflict(&chord, cmd) {
        Some(other) => app.ui.set.keys.conflict = Some((cmd, chord, other)),
        None => {
            app.ui.bindings.assign(cmd, chord, false);
            app.ui.set.keys.conflict = None;
        }
    }
}

fn where_local(scope: Scope) -> Option<&'static str> {
    match scope {
        Scope::Global => None,
        Scope::Local(w) => Some(w),
    }
}

/// The page.
pub(crate) fn page(ui: &mut egui::Ui, app: &mut YaacApp) {
    let p = app.p;
    ui.horizontal(|ui| {
        ui.label(RichText::new("Keyboard").font(theme::bold(size::TITLE + 3.0)));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let changed = !app.ui.bindings.overrides.0.is_empty();
            if ui.add_enabled(changed, egui::Button::new("Reset all")).clicked() {
                app.ui.bindings.reset_all();
                app.ui.set.keys = KeysUi::default();
            }
        });
    });
    ui.add_space(4.0);
    ui.label(widgets::secondary(
        "Every command is also in the Commander. Click a key to record a new one: Esc cancels, Backspace clears. Keys without Ctrl or Alt act only when no text field has focus.",
        &p,
    ));
    for g in Group::ALL {
        widgets::heading(ui, g.label());
        for d in COMMANDS.iter().filter(|d| d.group == g) {
            row(ui, app, d);
        }
    }
}

fn row(ui: &mut egui::Ui, app: &mut YaacApp, d: &'static cmds::CommandDef) {
    let p = app.p;
    let cmd = d.cmd;
    let chords = app.ui.bindings.chords(cmd);
    let recording = app.ui.set.keys.recording == Some(cmd);
    let local = where_local(d.scope);
    ui.horizontal(|ui| {
        ui.set_min_height(40.0);
        ui.vertical(|ui| {
            ui.set_width((ui.available_width() - 300.0).max(200.0));
            ui.add_space(3.0);
            ui.label(RichText::new(d.name).font(theme::ui(size::UI)));
            ui.label(widgets::secondary(d.explain, &p).font(theme::ui(size::SMALL)));
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if let Some(w) = local {
                // Fixed: it works only where it is.
                ui.label(widgets::secondary(w, &p).font(theme::ui(size::SMALL)));
                ui.add_space(6.0);
                key_chip(ui, &p, &cmds::Bindings::default().label(cmd), false, false);
                return;
            }
            let changed = app.ui.bindings.is_changed(cmd);
            if ui.add_enabled(changed, egui::Button::new(RichText::new("Reset").font(theme::ui(size::SMALL))).frame(false)).on_hover_text(format!("Back to {}", default_text(d))).clicked() {
                app.ui.bindings.reset(cmd);
                app.ui.set.keys.conflict = None;
            }
            if ui.add_enabled(!chords.is_empty(), egui::Button::new(RichText::new("Clear").font(theme::ui(size::SMALL))).frame(false)).clicked() {
                app.ui.bindings.clear(cmd);
                app.ui.set.keys.conflict = None;
            }
            ui.add_space(4.0);
            let text = if recording {
                "Press a key…  Esc cancels".to_owned()
            } else if chords.is_empty() {
                "Add a key".to_owned()
            } else {
                app.ui.bindings.label(cmd)
            };
            let r = key_chip(ui, &p, &text, true, recording);
            if r.clicked() {
                app.ui.set.keys.recording = if recording { None } else { Some(cmd) };
                app.ui.set.keys.conflict = None;
            }
        });
    });
    // The chord recorded for this row belongs to another command.
    if let Some((for_cmd, chord, other)) = app.ui.set.keys.conflict.clone()
        && for_cmd == cmd
    {
        let other_local = where_local(cmds::def_of(other).scope);
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            crate::widgets::icon(ui, 14.0, |pa, c, dd| crate::icons::warning(pa, c, dd, p.needs, p.bg));
            let msg = match other_local {
                Some(w) => format!("{chord} is used by {} {w}; choose another key.", cmds::def_of(other).name),
                None => format!("{chord} is used by {}.", cmds::def_of(other).name),
            };
            ui.label(RichText::new(msg).font(theme::ui(size::SMALL)));
            if other_local.is_none() && widgets::primary(ui, &p, "Replace", None).clicked() {
                app.ui.bindings.assign(cmd, chord.clone(), true);
                app.ui.set.keys.conflict = None;
            }
            if widgets::plain(ui, &p, "Cancel", None).clicked() {
                app.ui.set.keys.conflict = None;
            }
        });
    }
    // A chord the system may take first.
    if let Some(note) = chords.iter().find_map(cmds::os_note) {
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            crate::widgets::icon(ui, 13.0, |pa, c, dd| crate::icons::info(pa, c, dd, p.text2));
            ui.add(egui::Label::new(widgets::secondary(note, &p).font(theme::ui(size::SMALL))).wrap());
        });
    }
    ui.add_space(2.0);
}

fn default_text(d: &cmds::CommandDef) -> String {
    let t = yaac_core::keys::format_chords(&cmds::default_chords(d));
    if t.is_empty() { "no key".to_owned() } else { t }
}

/// A key drawn as a key cap: outlined, small text; `active` while recording.
fn key_chip(ui: &mut egui::Ui, p: &crate::theme::Palette, text: &str, clickable: bool, active: bool) -> egui::Response {
    let g = ui.fonts_mut(|f| f.layout_no_wrap(text.to_owned(), theme::ui(size::SMALL), p.text));
    let sz = egui::vec2(g.size().x + 18.0, 24.0).max(egui::vec2(76.0, 24.0));
    let (rect, resp) = ui.allocate_exact_size(sz, if clickable { egui::Sense::click() } else { egui::Sense::hover() });
    let fill = if active {
        p.selected
    } else if clickable && (resp.hovered() || resp.has_focus()) {
        p.faint
    } else {
        egui::Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, 5.0, fill);
    let stroke = if active { egui::Stroke::new(1.5, p.focus) } else { egui::Stroke::new(1.0, p.border) };
    ui.painter().rect_stroke(rect, 5.0, stroke, egui::StrokeKind::Inside);
    let color = if clickable { p.text } else { p.text2 };
    ui.painter().galley(egui::pos2(rect.center().x - g.size().x / 2.0, rect.center().y - g.size().y / 2.0), g, color);
    resp
}
