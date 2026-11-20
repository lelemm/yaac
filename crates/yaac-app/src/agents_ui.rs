//! The agents panel in its three states: a pill floating over the thread's right edge (the
//! default), a card on hover, or docked as a fixed panel. Arrows show only while a message is
//! in flight, and are static: an idle fleet costs no CPU.

use eframe::egui::{self, Align2, Margin, Rect, Sense, Stroke, pos2, vec2};
use yaac_core::agents::{AgentRow, AgentsPanel, Flow, PanelInput, PanelMode, SpawnedBy, pill_icons};
use yaac_core::attention::Mark;
use yaac_core::hover::Region;

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
    let w = Stroke::new(1.6, p.text);
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
    let (strip, card_rect) = floating_regions(thread, rows.len(), icons_shown.len() + usize::from(more > 0));
    let width = if card { CARD_W } else { PILL_W };
    let row_h = if card { CARD_ROW } else { PILL_ROW };
    let pos = if card { card_rect.min } else { strip.min };
    let mut any_focus = false;
    let resp = egui::Area::new(egui::Id::unique("agents-floating")).order(egui::Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
        let radius = if card { 12 } else { 22 };
        egui::Frame::popup(ui.style()).fill(p.raised).corner_radius(radius).inner_margin(Margin::same(if card { 10 } else { 6 })).show(ui, |ui| {
            ui.set_width(width - if card { 20.0 } else { 12.0 });
            if card {
                ui.set_min_height(card_rect.height() - 20.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Agents").font(theme::bold(size::UI)));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let d = widgets::plain(ui, &p, "Dock", None).on_hover_text("Pin to the side");
                        any_focus |= d.has_focus();
                        if d.clicked() {
                            actions.push(Action::Panel(PanelInput::Dock));
                        }
                    });
                });
                for row in &rows {
                    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click());
                    any_focus |= resp.has_focus();
                    if resp.hovered() || resp.has_focus() {
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
                        any_focus |= resp.has_focus();
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
    let _ = resp;
    if app.shots.is_some() {
        return;
    }
    // Open and close from fixed regions and a short delay (yaac_core::hover), never from the
    // area's own outline, which grows and shrinks with the state.
    let pointer = ctx.pointer_hover_pos().map(|p| (p.x, p.y));
    let region = |r: Rect| Region::new((r.left(), r.top()), (r.right(), r.bottom()));
    let now = ctx.input(|i| i.time);
    let step = app.ui.hover.step(pointer, region(strip), region(card_rect), any_focus, now);
    if let Some(s) = step.wake_in {
        // The pending close: one scheduled repaint, not a running loop.
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(s));
    }
    match (app.ui.panel, step.open) {
        (PanelMode::Pill, true) => {
            actions.push(Action::Panel(PanelInput::Enter));
            ctx.request_repaint();
        }
        (PanelMode::Card, false) => {
            actions.push(Action::Panel(PanelInput::Leave));
            ctx.request_repaint();
        }
        _ => {}
    }
}

const PILL_W: f32 = 44.0;
const PILL_ROW: f32 = 32.0;
const CARD_W: f32 = 300.0;
const CARD_ROW: f32 = 44.0;

/// The pill's strip and the card's region, from the layout alone: both share the thread's
/// right edge (less a 12 pt margin) and are centred on the same line.
pub(crate) fn floating_regions(thread: Rect, rows: usize, pill_slots: usize) -> (Rect, Rect) {
    let right = thread.right() - 12.0;
    let cy = thread.center().y;
    let pill_h = pill_slots as f32 * PILL_ROW + 16.0;
    let card_h = rows as f32 * CARD_ROW + 44.0 + 20.0;
    let strip = Rect::from_min_max(pos2(right - PILL_W, cy - pill_h / 2.0), pos2(right, cy + pill_h / 2.0));
    let card = Rect::from_min_max(pos2(right - CARD_W, cy - card_h / 2.0), pos2(right, cy + card_h / 2.0));
    (strip, card)
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

#[cfg(test)]
mod tests {
    //! The real widget on a headless egui context, with the demo fleet and simulated pointer
    //! positions: the card must not flicker, and must not keep the window repainting.
    use eframe::egui::{Event, Pos2, RawInput, ViewportId, vec2};
    use yaac_core::agents::agents_panel;
    use yaac_core::attention::AttentionMap;

    use super::*;

    fn app(ctx: &egui::Context) -> YaacApp {
        let launch = crate::Launch {
            demo: Some(yaac_core::demo::Scenario::Fleet),
            theme: crate::theme::ThemeChoice::Dark,
            shots: None,
            host_windows: false,
            #[cfg(feature = "hub")]
            attach: None,
        };
        YaacApp::from_ctx(ctx, launch)
    }

    /// One frame with the pointer at `p`; returns the panel mode after it and the repaint
    /// delay egui was asked for.
    fn frame(ctx: &egui::Context, app: &mut YaacApp, thread: Rect, p: Option<Pos2>, t: f64) -> (PanelMode, std::time::Duration) {
        // An event only when the pointer moves (a frame without input is what an idle window
        // sees).
        let last: Option<Pos2> = ctx.input(|i| i.pointer.latest_pos());
        let events = match p {
            Some(p) if last != Some(p) => vec![Event::PointerMoved(p)],
            Some(_) => vec![],
            None => vec![Event::PointerGone],
        };
        let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 1000.0))), events, time: Some(t), ..Default::default() };
        let mut actions = Vec::new();
        let mut out = ctx.run_ui(input, |ui| {
            let map = AttentionMap::compute(&app.client.state);
            let key = app.client.state.selected_thread().cloned().unwrap();
            let panel = agents_panel(&app.client.state, &map, &key).unwrap();
            floating(ui.ctx(), app, thread, &panel, &mut actions);
        });
        out.textures_delta.clear();
        app.apply(actions);
        (app.ui.panel, out.viewport_output[&ViewportId::ROOT].repaint_delay)
    }

    #[test]
    fn the_card_stays_open_while_the_pointer_moves_over_it_and_closes_once_after_leaving() {
        let ctx = egui::Context::default();
        let mut app = app(&ctx);
        let thread = Rect::from_min_max(pos2(280.0, 0.0), pos2(1600.0, 1000.0));
        let panel = {
            let map = AttentionMap::compute(&app.client.state);
            agents_panel(&app.client.state, &map, app.client.state.selected_thread().unwrap()).unwrap()
        };
        let (icons, more) = pill_icons(&panel);
        let (strip, card) = floating_regions(thread, panel.all().count(), icons.len() + usize::from(more > 0));
        // Idle, pointer far away: the pill, and (once egui's first-frame layout passes are
        // done) nothing keeps the window awake.
        for i in 0..10 {
            frame(&ctx, &mut app, thread, Some(pos2(600.0, 500.0)), f64::from(i) * 0.1 - 2.0);
        }
        let (mode, delay) = frame(&ctx, &mut app, thread, Some(pos2(600.0, 500.0)), 0.0);
        assert_eq!(mode, PanelMode::Pill);
        assert!(delay > std::time::Duration::from_secs(60), "idle repaints: {delay:?}");
        // Onto the pill: the card opens.
        let (mode, _) = frame(&ctx, &mut app, thread, Some(strip.center()), 0.1);
        assert_eq!(mode, PanelMode::Card);
        // Across the card, frame by frame, including over parts far from the pill: it never
        // closes (the owner's flicker).
        let mut t = 0.1;
        for i in 0..40 {
            t += 0.016;
            let f = i as f32 / 39.0;
            let p = pos2(card.left() + 4.0 + (card.width() - 8.0) * (1.0 - f), card.top() + 4.0 + (card.height() - 8.0) * f);
            let (mode, _) = frame(&ctx, &mut app, thread, Some(p), t);
            assert_eq!(mode, PanelMode::Card, "closed at {p:?}");
        }
        // Resting over the card: open, and idle (no repaint pending) once settled.
        let mut d = std::time::Duration::ZERO;
        for k in 0..5 {
            let (mode, delay) = frame(&ctx, &mut app, thread, Some(card.center()), t + 0.1 * f64::from(k) + 0.1);
            assert_eq!(mode, PanelMode::Card);
            d = delay;
        }
        assert!(d > std::time::Duration::from_secs(60), "{d:?}");
        t += 0.6;
        // Out: still open for a quarter of a second, with one scheduled repaint to close it.
        let (mode, _) = frame(&ctx, &mut app, thread, Some(pos2(600.0, 500.0)), t + 0.01);
        assert_eq!(mode, PanelMode::Card);
        // (The frame of the move and egui's hover-out pass repaint for themselves; after that
        // the window is woken only by the pending close.)
        frame(&ctx, &mut app, thread, Some(pos2(600.0, 500.0)), t + 0.05);
        let (mode, delay) = frame(&ctx, &mut app, thread, Some(pos2(600.0, 500.0)), t + 0.1);
        assert_eq!(mode, PanelMode::Card);
        assert!(delay <= std::time::Duration::from_millis(250) && delay > std::time::Duration::ZERO, "{delay:?}");
        let (mode, _) = frame(&ctx, &mut app, thread, Some(pos2(600.0, 500.0)), t + 0.2);
        assert_eq!(mode, PanelMode::Card);
        let (mode, _) = frame(&ctx, &mut app, thread, Some(pos2(600.0, 500.0)), t + 0.3);
        assert_eq!(mode, PanelMode::Pill);
        // Closed and idle again: once egui's own short layout and fade passes are over, no
        // repaint is pending.
        let mut delay = std::time::Duration::ZERO;
        for i in 0..6 {
            delay = frame(&ctx, &mut app, thread, Some(pos2(600.0, 500.0)), t + 0.4 + f64::from(i) * 0.2).1;
        }
        assert!(delay > std::time::Duration::from_secs(60), "{delay:?}");
    }
}
