//! What sits around messages in the transcript: the user's bubble (D155), a message's time
//! shown while it is hovered (D154), and the line a finished turn folds into (D156).

use eframe::egui::{self, Margin, Rect, Sense, UiBuilder, pos2, vec2};
use yaac_core::activity::Block;
use yaac_core::time::Zone;
use yaac_core::transcript::Transcript;
use yaac_protocol::ItemBody;

use crate::icons;
use crate::theme::{self, Palette, size};

/// The widest a user's bubble gets, as a share of the column. It sits at the column's right
/// edge and hugs a short message.
pub(crate) const BUBBLE_SHARE: f32 = 0.8;
/// The bubble's inner margins.
pub(crate) const BUBBLE_MARGIN: Margin = Margin::symmetric(14, 10);
/// The line kept free under every message for its time, so showing it moves nothing.
pub(crate) const STAMP_LINE: f32 = 18.0;
/// The "Worked for" line's height.
pub(crate) const WORKED_LINE: f32 = 24.0;

/// The space a block keeps under itself for its time.
pub(crate) fn stamp_space(b: &Block) -> f32 {
    if matches!(b, Block::User { .. } | Block::Task { .. } | Block::Prose { .. }) { STAMP_LINE } else { 0.0 }
}

/// The user's bubble: the `user_bg` fill, right-aligned in the column, as wide as its text
/// (`natural`, from [`crate::markdown::natural_width`]) up to [`BUBBLE_SHARE`] of the column;
/// a message with code, a table, a paste or files takes the full share.
pub(crate) fn bubble<R>(ui: &mut egui::Ui, p: &Palette, natural: Option<f32>, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let column = ui.available_width();
    let margin = f32::from(BUBBLE_MARGIN.left + BUBBLE_MARGIN.right);
    let max = (column * BUBBLE_SHARE).max(column.min(240.0));
    // A little slack so a measured line never wraps on rounding.
    let inner = natural.map_or(max - margin, |w| (w.ceil() + 2.0).min(max - margin)).max(16.0);
    let top = ui.cursor().min.y;
    let right = ui.max_rect().right();
    let rect = Rect::from_min_max(pos2(right - inner - margin, top), pos2(right, ui.max_rect().bottom().max(top + 1.0)));
    ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
        egui::Frame::NONE
            .fill(p.user_bg)
            .corner_radius(8)
            .inner_margin(BUBBLE_MARGIN)
            .show(ui, |ui| {
                ui.set_width(inner);
                add(ui)
            })
            .inner
    })
    .inner
}

/// When a message was written: the user's when sent, the agent's when finished.
pub(crate) fn message_time(b: &Block, t: &Transcript) -> Option<i64> {
    let (Block::User { index } | Block::Task { index } | Block::Prose { index }) = b else { return None };
    let item = t.items().get(*index)?;
    let at = match &item.body {
        ItemBody::AgentMessage(_) if item.status == yaac_protocol::ItemStatus::InProgress => return None,
        ItemBody::AgentMessage(_) => item.completed_at.as_ref().or(item.started_at.as_ref()),
        ItemBody::MeshReceived(r) => item.started_at.as_ref().or(Some(&r.message.sent_at)),
        _ => item.started_at.as_ref().or(item.completed_at.as_ref()),
    };
    at.and_then(yaac_core::time::secs)
}

/// A message's time, small and secondary, in the line kept free under it (`drawn` is the
/// message's own rectangle). The user's sits at the right, under the bubble; the agent's at
/// the left.
pub(crate) fn stamp(ui: &egui::Ui, p: &Palette, drawn: Rect, at: i64, now: i64, zone: Zone, right: bool) -> Rect {
    let text = yaac_core::time::message_time(now, at, zone);
    let g = ui.fonts_mut(|f| f.layout_no_wrap(text, theme::ui(size::SMALL - 0.5), p.text2));
    let y = drawn.bottom() + (STAMP_LINE - g.size().y) / 2.0;
    let x = if right { drawn.right() - g.size().x - 2.0 } else { drawn.left() + 2.0 };
    let r = Rect::from_min_size(pos2(x, y), g.size());
    ui.painter().galley(r.min, g, p.text2);
    r
}

/// "Worked for 26s ›": the folded work of a finished turn, a quiet line that opens it. A
/// failed turn's line is in the failed colour and its error stays under it.
pub(crate) fn worked_line(ui: &mut egui::Ui, p: &Palette, text: &str, failed: bool, error: Option<&str>, expanded: bool) -> egui::Response {
    let font = theme::ui(size::SMALL + 0.5);
    let color = if failed { p.failed } else { p.text2 };
    let g = ui.fonts_mut(|f| f.layout_no_wrap(text.to_owned(), font.clone(), color));
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), WORKED_LINE), Sense::hover());
    let hit = Rect::from_min_size(row.min, vec2(g.size().x + 24.0, row.height()));
    let resp = ui.interact(hit, ui.scope_id().with(("worked", text)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
    let ink = if resp.hovered() && !failed { p.text } else { color };
    let y = row.center().y - g.size().y / 2.0;
    let gw = g.size().x;
    ui.painter().galley_with_override_text_color(pos2(row.left(), y), g, ink);
    icons::chevron(ui.painter(), pos2(row.left() + gw + 10.0, row.center().y), 11.0, expanded, ink);
    if resp.has_focus() {
        crate::widgets::focus_ring(ui, hit, p);
    }
    if let Some(e) = error {
        ui.label(egui::RichText::new(e).font(theme::ui(size::SMALL + 0.5)).color(p.failed));
    }
    resp
}
