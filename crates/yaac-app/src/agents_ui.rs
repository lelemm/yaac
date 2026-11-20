//! The agents panel in its three states: a pill floating over the thread's right edge (the
//! default), a card on hover, or docked as a fixed panel. Arrows show only while a message is
//! in flight, and are static: an idle fleet costs no CPU.

use eframe::egui::{self, Align2, Margin, Rect, Sense, Stroke, pos2, vec2};
use yaac_core::agents::{AgentRow, AgentsPanel, Flow, PanelInput, PanelMode, SpawnedBy, pill_icons};
use yaac_core::attention::Mark;

use crate::app::{Action, YaacApp};
use crate::icons;
use crate::theme::{self, Palette, size};
use crate::widgets;

/// A harness glyph.
#[derive(Clone, Copy, Debug)]
enum Glyph {
    Hexagon,
    Spark,
}

/// Glyphs by provider kind: data, looked up — not a branch in UI logic. A kind not listed gets
/// its initial in a circle.
const GLYPHS: &[(&str, Glyph)] = &[("codex", Glyph::Hexagon), ("claude", Glyph::Spark)];

fn harness_icon(painter: &egui::Painter, c: egui::Pos2, d: f32, row: &AgentRow, p: &Palette) {
    let color = if row.finished { p.text2.gamma_multiply(0.6) } else { p.text };
    let glyph = row.kind.as_deref().and_then(|k| GLYPHS.iter().find(|(name, _)| *name == k).map(|(_, g)| *g));
    match glyph {
        Some(Glyph::Spark) => icons::spark(painter, c, d * 0.9, color),
        Some(Glyph::Hexagon) => {
            let r = d * 0.42;
            let pts: Vec<egui::Pos2> = (0..=6)
                .map(|i| {
                    let a = std::f32::consts::FRAC_PI_6 + i as f32 * std::f32::consts::FRAC_PI_3;
                    c + vec2(a.cos(), a.sin()) * r
                })
                .collect();
            painter.line(pts, Stroke::new(1.6, color));
            painter.circle_filled(c, d * 0.1, color);
        }
        None => {
            painter.circle_stroke(c, d * 0.42, Stroke::new(1.4, color));
            let initial = row.provider.as_deref().or(row.kind.as_deref()).and_then(|s| s.chars().next()).unwrap_or('?');
            painter.text(c, Align2::CENTER_CENTER, initial.to_uppercase().to_string(), theme::bold(d * 0.5), color);
        }
    }
    if row.display.attention.is_some() {
        icons::warning(painter, c + vec2(d * 0.38, d * 0.32), d * 0.5, p.needs, p.raised);
    }
}

fn arrow(painter: &egui::Painter, c: egui::Pos2, flow: Flow, p: &Palette) {
    let s = 4.0;
    let w = Stroke::new(1.6, p.accent);
    match flow {
        Flow::Sending => {
            painter.line_segment([pos2(c.x, c.y + s), pos2(c.x, c.y - s)], w);
            painter.line(vec![pos2(c.x - 3.0, c.y - s + 3.0), pos2(c.x, c.y - s), pos2(c.x + 3.0, c.y - s + 3.0)], w);
        }
        Flow::Receiving => {
            painter.line_segment([pos2(c.x, c.y - s), pos2(c.x, c.y + s)], w);
            painter.line(vec![pos2(c.x - 3.0, c.y + s - 3.0), pos2(c.x, c.y + s), pos2(c.x + 3.0, c.y + s - 3.0)], w);
        }
    }
}

/// The pill, or the card while hovered. Floats over the thread; takes no layout column.
pub(crate) fn floating(ctx: &egui::Context, app: &mut YaacApp, thread: Rect, panel: &AgentsPanel, actions: &mut Vec<Action>) {
    let p = app.p;
    let card = app.ui.panel == PanelMode::Card;
    let (icons_shown, more) = pill_icons(panel);
    let rows: Vec<&AgentRow> = panel.all().collect();
    let width = if card { 300.0 } else { 44.0 };
    let count = if card { rows.len() } else { icons_shown.len() + usize::from(more > 0) };
    let row_h = if card { 44.0 } else { 32.0 };
    let height = count as f32 * row_h + if card { 44.0 } else { 16.0 };
    let pos = pos2(thread.right() - width - 12.0, thread.center().y - height / 2.0);
    let resp = egui::Area::new(egui::Id::unique("agents-floating")).order(egui::Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
        let radius = if card { 12 } else { 22 };
        egui::Frame::popup(ui.style()).fill(p.raised).corner_radius(radius).inner_margin(Margin::same(if card { 10 } else { 6 })).show(ui, |ui| {
            ui.set_width(width - if card { 20.0 } else { 12.0 });
            if card {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Agents").font(theme::bold(size::UI)));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if widgets::plain(ui, &p, "Dock", None).on_hover_text("Pin to the side").clicked() {
                            actions.push(Action::Panel(PanelInput::Dock));
                        }
                    });
                });
                for row in &rows {
                    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click());
                    if resp.hovered() {
                        ui.painter().rect_filled(r, 6.0, p.faint);
                    }
                    harness_icon(ui.painter(), pos2(r.left() + 16.0, r.center().y), 20.0, row, &p);
                    let text_w = r.width() - 60.0;
                    let name_color = if row.finished { p.text2 } else { p.text };
                    let g = widgets::elided(ui, &row.title, &theme::bold(size::UI), name_color, text_w);
                    ui.painter().galley(pos2(r.left() + 34.0, r.top() + 5.0), g, name_color);
                    let g = widgets::elided(ui, &row.last_action, &theme::ui(size::SMALL), p.text2, text_w);
                    ui.painter().galley(pos2(r.left() + 34.0, r.top() + 24.0), g, p.text2);
                    if let Some(f) = row.flow {
                        arrow(ui.painter(), pos2(r.right() - 12.0, r.center().y), f, &p);
                    }
                    if resp.clicked()
                        && let Some(k) = &row.key
                    {
                        actions.push(Action::Open(k.clone()));
                    }
                }
            } else {
                ui.vertical_centered(|ui| {
                    for row in &icons_shown {
                        let (r, resp) = ui.allocate_exact_size(vec2(32.0, row_h), Sense::click());
                        harness_icon(ui.painter(), r.center(), 22.0, row, &p);
                        if let Some(f) = row.flow {
                            arrow(ui.painter(), pos2(r.right() - 1.0, r.top() + 8.0), f, &p);
                        }
                        if resp.clicked()
                            && let Some(k) = &row.key
                        {
                            actions.push(Action::Open(k.clone()));
                        }
                    }
                    if more > 0 {
                        ui.label(widgets::secondary(format!("+{more}"), &p));
                    }
                });
            }
        });
    });
    let hovered = resp.response.contains_pointer() || ctx.memory(|m| m.focused().is_some_and(|f| resp.response.layer_id.id == f));
    match (app.ui.panel, hovered) {
        (PanelMode::Pill, true) => actions.push(Action::Panel(PanelInput::Enter)),
        (PanelMode::Card, false) if app.shots.is_none() => actions.push(Action::Panel(PanelInput::Leave)),
        _ => {}
    }
}

fn docked_row(ui: &mut egui::Ui, p: &Palette, row: &AgentRow, actions: &mut Vec<Action>) {
    ui.horizontal(|ui| {
        ui.add_space(f32::from(row.depth) * 16.0);
        ui.spacing_mut().item_spacing.x = 6.0;
        let (r, _) = ui.allocate_exact_size(vec2(16.0, 18.0), Sense::hover());
        icons::mark(ui.painter(), r.center(), 14.0, row.display.mark, p);
        match &row.key {
            Some(k) => {
                if widgets::link(ui, p, &row.title, theme::ui(size::UI)).clicked() {
                    actions.push(Action::Open(k.clone()));
                }
            }
            None => {
                ui.label(&row.title);
            }
        }
        if row.display.mark != Mark::Idle {
            ui.label(egui::RichText::new(row.display.mark.word()).font(theme::ui(size::SMALL)).color(p.mark(row.display.mark)));
        }
        if let Some(f) = row.flow {
            let (r, _) = ui.allocate_exact_size(vec2(12.0, 16.0), Sense::hover());
            arrow(ui.painter(), r.center(), f, p);
        }
    });
    let mut parts: Vec<String> = Vec::new();
    parts.extend(row.provider.clone());
    parts.extend(row.model.clone());
    parts.push(row.target.clone());
    ui.horizontal(|ui| {
        ui.add_space(f32::from(row.depth) * 16.0 + 22.0);
        ui.label(widgets::secondary(parts.join(" · "), p));
    });
    if !row.activity.is_empty() {
        ui.horizontal(|ui| {
            ui.add_space(f32::from(row.depth) * 16.0 + 22.0);
            let w = ui.available_width();
            let g = widgets::elided(ui, &row.activity, &theme::ui(size::SMALL), p.text2, w);
            ui.label(g);
        });
    }
    ui.add_space(6.0);
}

/// Docked: rows under three quiet headings; no boxes, no rules.
pub(crate) fn docked(ui: &mut egui::Ui, p: &Palette, panel: &AgentsPanel, actions: &mut Vec<Action>) {
    ui.spacing_mut().interact_size.y = 16.0;
    ui.spacing_mut().item_spacing.y = 2.0;
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Agents").font(theme::bold(size::SECTION + 1.0)));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let r = widgets::plain(ui, p, "Undock", None).on_hover_text("Back to the pill");
            if r.clicked() {
                actions.push(Action::Panel(PanelInput::Undock));
            }
        });
    });
    if let Some(b) = &panel.budget {
        ui.horizontal(|ui| {
            if b.exhausted {
                widgets::mark(ui, p, Mark::Paused, 14.0);
            }
            let c = if b.exhausted { p.needs } else { p.text2 };
            ui.label(egui::RichText::new(&b.text).font(theme::ui(size::SMALL)).color(c));
        });
    }
    ui.add_space(14.0);
    egui::ScrollArea::vertical().id_salt("agents-docked").auto_shrink([false, false]).show(ui, |ui| {
        heading(ui, p, "Spawned by", None);
        if let Some((title, _)) = &panel.root {
            ui.label(widgets::secondary(format!("root: {title}"), p));
        }
        match &panel.spawned_by {
            SpawnedBy::You => {
                ui.label(widgets::secondary("started by you", p));
                ui.add_space(6.0);
            }
            SpawnedBy::Agent(row) => docked_row(ui, p, row, actions),
        }
        ui.add_space(10.0);
        heading(ui, p, "Workers", Some(panel.direct_workers));
        for w in &panel.workers {
            docked_row(ui, p, w, actions);
        }
        ui.add_space(10.0);
        heading(ui, p, "In contact with", Some(panel.contacts.len()));
        for c in &panel.contacts {
            docked_row(ui, p, c, actions);
        }
    });
}

fn heading(ui: &mut egui::Ui, p: &Palette, text: &str, n: Option<usize>) {
    let t = match n {
        Some(n) => format!("{text} ({n})"),
        None => text.to_owned(),
    };
    ui.label(egui::RichText::new(t).font(theme::bold(size::SMALL)).color(p.text2));
    ui.add_space(4.0);
}
