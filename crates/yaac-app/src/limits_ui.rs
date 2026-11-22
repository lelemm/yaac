//! The rate-limit panel (D161), docked at the bottom of the sidebar above Settings, and its
//! tiny form in the collapsed strip. The model is `yaac_core::limits`; this draws it.
//!
//! Each account is a group: the provider's logo, then one segmented bar per window with
//! "55% · 2d" beside it, in the provider's colour — like the logos (D98), an exception to the
//! monochrome rule, and like them looked up from the kind as data, never branched on. Near the
//! limit the text is the failed colour. Groups flow left to right and wrap. The countdown
//! repaints at most once a minute, when its text would change.

use std::time::Duration;

use eframe::egui::{self, Color32, Rect, Sense, pos2, vec2};
use yaac_core::limits::{Group, Panel, Segment};

use crate::app::YaacApp;
use crate::theme::{self, Palette, size};

/// Provider kind → the panel's colour. Data, looked up. Claude's is its logo's brand orange
/// (D98); Codex's mark has no colour of its own, so the panel uses a blue chosen to reach 3:1
/// on the sidebar in both themes (3.8:1 on #F5F6F8, 4.1:1 on #1B1E24). A kind not listed uses
/// the secondary text colour.
const COLOURS: &[(&str, Color32)] = &[("claude", Color32::from_rgb(0xD9, 0x77, 0x57)), ("codex", Color32::from_rgb(0x34, 0x78, 0xF6))];

const SEG_W: f32 = 5.0;
const SEG_H: f32 = 4.0;
const SEG_GAP: f32 = 1.5;
const BAR_W: f32 = SEG_W * 5.0 + SEG_GAP * 4.0;
const LINE: f32 = 14.0;
const LOGO: f32 = 14.0;
const GROUP_GAP: f32 = 8.0;
const PAD: f32 = 6.0;

fn colour(kind: &str, p: &Palette) -> Color32 {
    COLOURS.iter().find(|(k, _)| *k == kind).map_or(p.text2, |(_, c)| *c)
}

/// The panel at `now`, and the countdown's repaint scheduled. `None`: nothing to show.
fn model(ui: &egui::Ui, app: &mut YaacApp, now: i64) -> Option<Panel> {
    app.client.rate_limits_shown(now);
    let state = &app.client.state;
    let names = |t: &yaac_protocol::TargetId| state.target(t).map_or_else(|| t.to_string(), |r| r.entry.name.to_string());
    let panel = yaac_core::limits::panel(&state.rate_limits.accounts, &names, now, app.zone);
    if panel.groups.is_empty() {
        return None;
    }
    if let Some(secs) = yaac_core::limits::next_change(&state.rate_limits.accounts, now) {
        ui.ctx().request_repaint_after(Duration::from_secs(u64::try_from(secs).unwrap_or(60)));
    }
    Some(panel)
}

fn text_width(ui: &egui::Ui, text: &str) -> f32 {
    ui.fonts_mut(|f| f.layout_no_wrap(text.to_owned(), theme::ui(size::SMALL), Color32::WHITE).size().x)
}

fn group_size(ui: &egui::Ui, g: &Group) -> egui::Vec2 {
    let text = g.bars.iter().map(|b| text_width(ui, &b.text)).fold(0.0, f32::max);
    let lines = g.bars.len().max(1) as f32;
    vec2(LOGO + 4.0 + BAR_W + 4.0 + text, (lines * LINE).max(LOGO))
}

/// Where each group goes in a panel `width` wide, and the panel's height.
fn layout(ui: &egui::Ui, panel: &Panel, width: f32) -> (Vec<Rect>, f32) {
    let (mut x, mut y, mut row_h) = (0.0_f32, 0.0_f32, 0.0_f32);
    let mut rects: Vec<Rect> = Vec::new();
    let mut row_start = 0;
    // Each group is centred on its row's height.
    let centre = |rects: &mut [Rect], row_h: f32| {
        for r in rects {
            *r = r.translate(vec2(0.0, (row_h - r.height()) / 2.0));
        }
    };
    for g in &panel.groups {
        let s = group_size(ui, g);
        if x > 0.0 && x + s.x > width {
            centre(&mut rects[row_start..], row_h);
            row_start = rects.len();
            y += row_h + PAD;
            x = 0.0;
            row_h = 0.0;
        }
        rects.push(Rect::from_min_size(pos2(x, y), s));
        x += s.x + GROUP_GAP;
        row_h = row_h.max(s.y);
    }
    centre(&mut rects[row_start..], row_h);
    (rects, y + row_h + PAD * 2.0)
}

/// The panel's height in a sidebar `width` wide (0 when hidden). Asked before the list is
/// sized, so the list ends above it.
pub(crate) fn height(ui: &egui::Ui, app: &mut YaacApp, now: i64, width: f32) -> f32 {
    model(ui, app, now).map_or(0.0, |panel| layout(ui, &panel, width).1)
}

fn segments(painter: &egui::Painter, left: f32, cy: f32, segs: &[Segment; 5], tone: Color32, p: &Palette) {
    for (i, s) in segs.iter().enumerate() {
        let r = Rect::from_min_size(pos2(left + i as f32 * (SEG_W + SEG_GAP), cy - SEG_H / 2.0), vec2(SEG_W, SEG_H));
        let fill = match s {
            Segment::Full => tone,
            Segment::Partial => tone.gamma_multiply(0.45),
            Segment::Empty => p.text2.gamma_multiply(0.22),
        };
        painter.rect_filled(r, 1.0, fill);
    }
}

fn draw_group(ui: &egui::Ui, g: &Group, rect: Rect, p: &Palette, force_tip: bool) {
    let painter = ui.painter();
    let dim = |c: Color32| if g.stale { c.gamma_multiply(0.55) } else { c };
    let tone = dim(colour(g.kind.as_str(), p));
    crate::agents_ui::harness_mark(painter, pos2(rect.left() + LOGO / 2.0, rect.center().y), LOGO, Some(g.kind.as_str()), None, g.stale, p);
    let block = g.bars.len() as f32 * LINE;
    let top = rect.center().y - block / 2.0;
    let left = rect.left() + LOGO + 4.0;
    for (i, b) in g.bars.iter().enumerate() {
        let cy = top + LINE * (i as f32 + 0.5);
        segments(painter, left, cy, &b.segments, tone, p);
        let colour = if b.near { dim(p.failed) } else { tone };
        let galley = ui.fonts_mut(|f| f.layout_no_wrap(b.text.clone(), theme::ui(size::SMALL), colour));
        painter.galley(pos2(left + BAR_W + 4.0, cy - galley.size().y / 2.0), galley, colour);
    }
    let resp = ui.interact(rect, egui::Id::unique(("rate-limits", &g.key)), Sense::hover());
    if force_tip {
        resp.show_tooltip_text(&g.tooltip);
    } else {
        resp.on_hover_text(&g.tooltip);
    }
}

/// Draws the panel in the next `height` points of the sidebar.
pub(crate) fn show(ui: &mut egui::Ui, app: &mut YaacApp, now: i64) {
    let Some(panel) = model(ui, app, now) else { return };
    let p = app.p;
    let width = ui.available_width();
    let (rects, h) = layout(ui, &panel, width);
    let (area, _) = ui.allocate_exact_size(vec2(width, h), Sense::hover());
    // Set apart from the list by space alone (no rules inside the sidebar).
    for (g, r) in panel.groups.iter().zip(rects) {
        let force = app.ui.limits_tip_force.as_deref() == Some(g.key.as_str());
        draw_group(ui, g, r.translate(area.min.to_vec2() + vec2(2.0, PAD)), &p, force);
    }
}

/// The collapsed strip's form: per account, the logo and one thin bar for its most used
/// window, stacked above Settings. Returns the height used (drawn upwards from `bottom`).
pub(crate) fn strip(ui: &mut egui::Ui, app: &mut YaacApp, now: i64, cx: f32, bottom: f32) -> f32 {
    const CELL: f32 = 30.0;
    let Some(panel) = model(ui, app, now) else { return 0.0 };
    let p = app.p;
    let mut y = bottom;
    for g in panel.groups.iter().rev() {
        let cell = Rect::from_min_max(pos2(cx - 20.0, y - CELL), pos2(cx + 20.0, y));
        crate::agents_ui::harness_mark(ui.painter(), pos2(cx, cell.top() + 10.0), 15.0, Some(g.kind.as_str()), None, g.stale, &p);
        if let Some(b) = g.bars.get(g.tightest) {
            let tone = if g.stale { colour(g.kind.as_str(), &p).gamma_multiply(0.55) } else { colour(g.kind.as_str(), &p) };
            let tone = if b.near { p.failed } else { tone };
            // Thinner and narrower than the panel's bar: five 4×3 segments.
            let w = 4.0 * 5.0 + 1.5 * 4.0;
            for (i, s) in b.segments.iter().enumerate() {
                let r = Rect::from_min_size(pos2(cx - w / 2.0 + i as f32 * 5.5, cell.top() + 21.0), vec2(4.0, 3.0));
                let fill = match s {
                    Segment::Full => tone,
                    Segment::Partial => tone.gamma_multiply(0.45),
                    Segment::Empty => p.text2.gamma_multiply(0.22),
                };
                ui.painter().rect_filled(r, 0.5, fill);
            }
            let tip = format!("{} {}\n\n{}", b.label, b.text, g.tooltip);
            ui.interact(cell, egui::Id::unique(("strip-rate-limits", &g.key)), Sense::hover()).on_hover_text(tip);
        }
        y -= CELL;
    }
    bottom - y
}
