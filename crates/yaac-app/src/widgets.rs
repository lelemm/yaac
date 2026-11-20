//! Small widgets in the house style: no outlines; one filled button per screen; plain-text
//! buttons with a faint fill on hover; selectors as a word and a chevron; a 2 px focus ring.

use eframe::egui::{self, Color32, CornerRadius, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};
use yaac_core::attention::Mark;

use crate::icons;
use crate::theme::{self, Palette, size};

/// The 2 px accent focus ring.
pub(crate) fn focus_ring(ui: &Ui, rect: Rect, p: &Palette) {
    ui.painter().rect_stroke(rect.expand(2.0), CornerRadius::same(6), Stroke::new(2.0, p.accent), StrokeKind::Outside);
}

fn text_size(ui: &Ui, text: &str, font: &egui::FontId) -> Vec2 {
    ui.fonts_mut(|f| f.layout_no_wrap(text.to_owned(), font.clone(), Color32::WHITE)).size()
}

/// The primary action: the one filled button on a screen. `key` is its shortcut hint.
pub(crate) fn primary(ui: &mut Ui, p: &Palette, label: &str, key: Option<&str>) -> Response {
    button(ui, p, label, key, true, None)
}

/// A plain-text button: no outline, a faint fill on hover.
pub(crate) fn plain(ui: &mut Ui, p: &Palette, label: &str, key: Option<&str>) -> Response {
    button(ui, p, label, key, false, None)
}

/// A plain-text button in a colour.
pub(crate) fn plain_colored(ui: &mut Ui, p: &Palette, label: &str, color: Color32) -> Response {
    button(ui, p, label, None, false, Some(color))
}

fn button(ui: &mut Ui, p: &Palette, label: &str, key: Option<&str>, filled: bool, color: Option<Color32>) -> Response {
    let font = if filled { theme::bold(size::UI) } else { theme::ui(size::UI) };
    let key_font = theme::ui(size::SMALL);
    let tw = text_size(ui, label, &font);
    let kw = key.map_or(0.0, |k| if k == "Enter" { 14.0 + 10.0 } else { text_size(ui, k, &key_font).x + 10.0 });
    let pad = vec2(12.0, 5.0);
    let size = vec2(tw.x + kw + pad.x * 2.0, (tw.y + pad.y * 2.0).max(28.0));
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let fg = if filled { p.on_accent } else { color.unwrap_or(p.text) };
    if filled {
        let fill = if resp.hovered() { p.accent.gamma_multiply(0.9) } else { p.accent };
        ui.painter().rect_filled(rect, 5.0, fill);
    } else if resp.hovered() || resp.has_focus() {
        ui.painter().rect_filled(rect, 5.0, p.faint);
    }
    let galley = ui.fonts_mut(|f| f.layout_no_wrap(label.to_owned(), font, fg));
    let y = rect.center().y - galley.size().y / 2.0;
    ui.painter().galley(pos2(rect.left() + pad.x, y), galley, fg);
    if let Some(k) = key {
        let kc = if filled { p.on_accent } else { p.text2 };
        if k == "Enter" {
            icons::return_key(ui.painter(), pos2(rect.right() - pad.x - 7.0, rect.center().y), 14.0, kc);
        } else {
            let g = ui.fonts_mut(|f| f.layout_no_wrap(k.to_owned(), key_font, kc));
            ui.painter().galley(pos2(rect.right() - pad.x - g.size().x, rect.center().y - g.size().y / 2.0), g, kc);
        }
    }
    if resp.has_focus() {
        focus_ring(ui, rect, p);
    }
    resp
}

/// A link: the link colour, no underline at rest; underlined on hover or keyboard focus.
pub(crate) fn link(ui: &mut Ui, p: &Palette, text: &str, font: egui::FontId) -> Response {
    let resp = ui.add(egui::Label::new(egui::RichText::new(text).font(font).color(p.link)).sense(Sense::click()));
    if resp.hovered() || resp.has_focus() {
        let r = resp.rect;
        ui.painter().hline(r.x_range(), r.bottom() - 1.0, Stroke::new(1.0, p.link));
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A selector: a word and a chevron, no outline, a faint fill on hover.
pub(crate) fn selector(ui: &mut Ui, p: &Palette, label: &str, enabled: bool) -> Response {
    let font = theme::ui(size::UI);
    let tw = text_size(ui, label, &font);
    let chevron = if enabled { 14.0 } else { 0.0 };
    let size = vec2(tw.x + chevron + 10.0, 26.0);
    let (rect, resp) = ui.allocate_exact_size(size, if enabled { Sense::click() } else { Sense::hover() });
    if enabled && (resp.hovered() || resp.has_focus()) {
        ui.painter().rect_filled(rect, 5.0, p.faint);
    }
    let color = p.text;
    let g = ui.fonts_mut(|f| f.layout_no_wrap(label.to_owned(), font, color));
    ui.painter().galley(pos2(rect.left() + 5.0, rect.center().y - g.size().y / 2.0), g, color);
    if enabled {
        icons::chevron(ui.painter(), pos2(rect.right() - 11.0, rect.center().y + 1.0), 12.0, true, p.text2);
    }
    if resp.has_focus() {
        focus_ring(ui, rect, p);
    }
    resp
}

/// A state mark in a square of `d` points, allocated in the layout.
pub(crate) fn mark(ui: &mut Ui, p: &Palette, m: Mark, d: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(d), Sense::hover());
    icons::mark(ui.painter(), rect.center(), d * 0.95, m, p);
    resp
}

/// An icon drawn by `f` in a square of `d` points.
pub(crate) fn icon(ui: &mut Ui, d: f32, f: impl FnOnce(&egui::Painter, egui::Pos2, f32)) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(d), Sense::hover());
    f(ui.painter(), rect.center(), d);
    resp
}

/// Secondary text.
pub(crate) fn secondary(text: impl Into<String>, p: &Palette) -> egui::RichText {
    egui::RichText::new(text.into()).font(theme::ui(size::SMALL)).color(p.text2)
}

/// A faint-filled frame (inputs, the user's message).
pub(crate) fn faint_frame(p: &Palette) -> egui::Frame {
    egui::Frame::NONE.fill(p.faint).corner_radius(6).inner_margin(egui::Margin::symmetric(10, 6))
}

/// A card that needs the user: a faint fill and a 3 px coloured left edge. No outline.
pub(crate) fn card<R>(ui: &mut Ui, p: &Palette, edge: Color32, add: impl FnOnce(&mut Ui) -> R) -> R {
    let inner = egui::Frame::NONE
        .fill(p.raised)
        .corner_radius(6)
        .inner_margin(egui::Margin { left: 16, right: 14, top: 12, bottom: 12 })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        });
    let r = inner.response.rect;
    ui.painter().rect_filled(
        Rect::from_min_max(r.left_top(), pos2(r.left() + 3.0, r.bottom())),
        CornerRadius { nw: 6, sw: 6, ne: 0, se: 0 },
        edge,
    );
    inner.inner
}

/// Unavailable control: no fill, a dashed border, the unavailable-control colour, and its
/// reason beside it.
pub(crate) fn unavailable(ui: &mut Ui, p: &Palette, label: &str, reason: &str) {
    ui.horizontal(|ui| {
        let font = theme::ui(size::UI);
        let tw = text_size(ui, label, &font);
        let (rect, _) = ui.allocate_exact_size(vec2(tw.x + 20.0, 26.0), Sense::hover());
        let s = Stroke::new(1.0, p.unavailable);
        let r = rect.shrink(1.0);
        let mut x = r.left();
        while x < r.right() {
            let e = (x + 4.0).min(r.right());
            ui.painter().line_segment([pos2(x, r.top()), pos2(e, r.top())], s);
            ui.painter().line_segment([pos2(x, r.bottom()), pos2(e, r.bottom())], s);
            x += 7.0;
        }
        ui.painter().line_segment([r.left_top(), r.left_bottom()], s);
        ui.painter().line_segment([r.right_top(), r.right_bottom()], s);
        let g = ui.fonts_mut(|f| f.layout_no_wrap(label.to_owned(), font, p.unavailable));
        ui.painter().galley(pos2(rect.left() + 10.0, rect.center().y - g.size().y / 2.0), g, p.unavailable);
        ui.label(secondary(reason, p));
    });
}

/// Truncate one line to fit `width`, with an ellipsis.
pub(crate) fn elided(ui: &Ui, text: &str, font: &egui::FontId, color: Color32, width: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font.clone(), color);
    job.wrap = egui::text::TextWrapping { max_width: width.max(10.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    ui.fonts_mut(|f| f.layout_job(job))
}

/// A selector for "instance  >  model": two words with a small chevron between them.
pub(crate) fn selector_pair(ui: &mut Ui, p: &Palette, left: &str, right: &str, enabled: bool) -> Response {
    let font = theme::ui(size::UI);
    let lw = text_size(ui, left, &font).x;
    let rw = text_size(ui, right, &font).x;
    let chevron = if enabled { 14.0 } else { 0.0 };
    let (rect, resp) = ui.allocate_exact_size(vec2(lw + rw + 16.0 + chevron + 10.0, 26.0), if enabled { Sense::click() } else { Sense::hover() });
    if enabled && (resp.hovered() || resp.has_focus()) {
        ui.painter().rect_filled(rect, 5.0, p.faint);
    }
    let g = ui.fonts_mut(|f| f.layout_no_wrap(left.to_owned(), font.clone(), p.text));
    ui.painter().galley(pos2(rect.left() + 5.0, rect.center().y - g.size().y / 2.0), g, p.text);
    icons::chevron(ui.painter(), pos2(rect.left() + 5.0 + lw + 8.0, rect.center().y), 10.0, false, p.text2);
    let g = ui.fonts_mut(|f| f.layout_no_wrap(right.to_owned(), font, p.text));
    ui.painter().galley(pos2(rect.left() + 5.0 + lw + 16.0, rect.center().y - g.size().y / 2.0), g, p.text);
    if enabled {
        icons::chevron(ui.painter(), pos2(rect.right() - 11.0, rect.center().y + 1.0), 12.0, true, p.text2);
    }
    if resp.has_focus() {
        focus_ring(ui, rect, p);
    }
    resp
}
