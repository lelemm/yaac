//! The timeline at the left edge of the transcript (D153): a narrow column of short muted
//! dashes, one per prompt, the ones in view longer and brighter. Hovering a dash shows a small
//! card with that prompt's first line and time; clicking it scrolls there. Nothing here
//! animates: the column is redrawn with the transcript, which repaints only on a cause
//! (a scroll, the pointer, new items).
//!
//! What to draw comes from `yaac_core::timeline`; this module only paints and hit-tests.

use eframe::egui::{self, Rect, Sense, pos2, vec2};
use yaac_core::ThreadKey;
use yaac_core::time::Zone;
use yaac_core::timeline::{self as tl, Entry};
use yaac_protocol::ItemId;

use crate::app::YaacApp;
use crate::theme::{self, Palette, size};

/// The column's width, from the transcript's left edge.
pub(crate) const COLUMN: f32 = 24.0;
/// Room kept free above and below the dashes.
const PAD: f32 = 28.0;
/// A dash's length, and a lit one's.
const DASH: f32 = 7.0;
const DASH_LIT: f32 = 12.0;

/// The timeline's view state.
#[derive(Debug, Default)]
pub(crate) struct TimelineUi {
    /// As last drawn: the thread, each prompt with its top in the transcript, and the view's
    /// top — what "Go to previous / next message" works from.
    pub last: Option<(ThreadKey, Vec<(ItemId, f32)>, f32)>,
    /// Scroll the transcript to this prompt on the next frame.
    pub go_to: Option<ItemId>,
    /// Screenshots: show this dash's card as if it were hovered.
    pub force_hover: Option<usize>,
    /// Screenshots and tests: show this message's time as if it were hovered.
    pub force_stamp: Option<ItemId>,
    /// The dashes as last drawn, in screen coordinates (tests click them).
    pub dashes: Vec<Rect>,
}

/// "Go to previous / next message" (Alt+PageUp / Alt+PageDown): the prompt before or after the
/// view, from what was last drawn.
pub(crate) fn go(app: &mut YaacApp, key: &ThreadKey, forward: bool) {
    let Some((k, entries, view_top)) = &app.ui.timeline.last else { return };
    if k != key {
        return;
    }
    let tops: Vec<f32> = entries.iter().map(|(_, t)| *t).collect();
    let to = if forward { tl::next(&tops, *view_top) } else { tl::prev(&tops, *view_top) };
    if let Some(i) = to {
        app.ui.timeline.go_to = Some(entries[i].0.clone());
    }
}

/// Draw the column at the left of `area` (the transcript's visible rectangle). `tops` are the
/// prompts' tops in the transcript; `view` its visible range. Returns the prompt clicked.
#[allow(clippy::too_many_arguments, reason = "the drawing context of one column")]
pub(crate) fn show(ui: &mut egui::Ui, ui_state: &mut TimelineUi, p: &Palette, area: Rect, entries: &[Entry], tops: &[f32], view: (f32, f32), now: i64, zone: Zone) -> Option<ItemId> {
    ui_state.dashes.clear();
    let col = Rect::from_min_max(pos2(area.left() + 4.0, area.top() + PAD), pos2(area.left() + COLUMN, area.bottom() - PAD));
    let layout = tl::layout(entries.len(), col.height());
    if layout.dashes.is_empty() {
        return None;
    }
    let in_view = tl::visible(tops, view.0, view.1);
    let resp = ui.interact(col, ui.scope_id().with("timeline"), Sense::click());
    let hovered = resp.hover_pos().and_then(|pos| tl::dash_at(&layout, pos.y - col.top())).or(ui_state.force_hover);
    if hovered.is_some() && resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let painter = ui.painter_at(col.expand(2.0));
    let x = col.left() + 4.0;
    for (i, d) in layout.dashes.iter().enumerate() {
        let lit = tl::lit(d, &in_view);
        let hot = hovered == Some(i);
        let (len, color, w) = match (lit, hot) {
            (_, true) => (DASH_LIT, p.text, 2.0),
            (true, false) => (DASH_LIT, p.text, 1.5),
            (false, false) => (DASH, p.text2.gamma_multiply(0.55), 1.5),
        };
        let y = col.top() + d.y;
        painter.hline(x..=x + len, y, egui::Stroke::new(w, color));
        ui_state.dashes.push(Rect::from_center_size(pos2(x + DASH_LIT / 2.0, y), vec2(DASH_LIT + 4.0, layout.pitch.max(3.0))));
    }
    let clicked = if resp.clicked() { hovered.and_then(|i| layout.dashes.get(i)).and_then(|d| entries.get(d.first)).map(|e| e.item.clone()) } else { None };
    if let Some(i) = hovered {
        let d = layout.dashes[i];
        card(ui, p, pos2(col.right() + 6.0, col.top() + d.y), &entries[d.first], d.last - d.first, now, zone);
    }
    clicked
}

/// The hover card: the prompt's first line and when it was sent; "and N more" for a dash
/// that stands for several.
fn card(ui: &egui::Ui, p: &Palette, at: egui::Pos2, e: &Entry, more: usize, now: i64, zone: Zone) {
    let ctx = ui.ctx().clone();
    egui::Area::new(egui::Id::unique("timeline-card")).order(egui::Order::Tooltip).fixed_pos(pos2(at.x, at.y - 24.0)).interactable(false).show(&ctx, |ui| {
        egui::Frame::popup(ui.style()).fill(p.raised).corner_radius(8).inner_margin(egui::Margin::symmetric(10, 7)).show(ui, |ui| {
            ui.set_max_width(320.0);
            let g = crate::widgets::elided(ui, &e.title, &theme::ui(size::UI), p.text, 300.0);
            let (r, _) = ui.allocate_exact_size(g.size(), Sense::hover());
            ui.painter().galley(r.min, g, p.text);
            let mut sub = e.at.map(|a| yaac_core::time::message_time(now, a, zone)).unwrap_or_default();
            if more > 0 {
                if !sub.is_empty() {
                    sub.push_str(" · ");
                }
                sub.push_str(&format!("and {more} more"));
            }
            if !sub.is_empty() {
                ui.label(egui::RichText::new(sub).font(theme::ui(size::SMALL)).color(p.text2));
            }
        });
    });
}
