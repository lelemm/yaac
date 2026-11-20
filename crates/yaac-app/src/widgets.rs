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

/// A switch: a filled track (the accent when on) and a knob. No outline.
pub(crate) fn switch(ui: &mut Ui, p: &Palette, on: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(32.0, 18.0), Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let off_fill = if p.dark { Color32::from_rgb(0x3A, 0x40, 0x4C) } else { Color32::from_rgb(0xC9, 0xCE, 0xD6) };
    let track = if on { p.accent } else { off_fill };
    ui.painter().rect_filled(rect, 9.0, track);
    let x = if on { rect.right() - 9.0 } else { rect.left() + 9.0 };
    let knob = if on { p.on_accent } else { p.bg };
    ui.painter().circle_filled(pos2(x, rect.center().y), 6.5, knob);
    if resp.has_focus() {
        focus_ring(ui, rect, p);
    }
    resp
}

/// A single-line input: a faint fill, no outline; the focused one gets the focus ring.
pub(crate) fn field(ui: &mut Ui, p: &Palette, text: &mut String, hint: &str, width: f32, mono: bool) -> Response {
    let mut te = egui::TextEdit::singleline(text)
        .hint_text(hint)
        .frame(egui::Frame::NONE.fill(p.faint).corner_radius(5).inner_margin(egui::Margin::symmetric(8, 5)))
        .desired_width(width);
    if mono {
        te = te.font(theme::mono(size::MONO));
    }
    let r = ui.add(te);
    if r.has_focus() {
        focus_ring(ui, r.rect, p);
    }
    r
}

/// A password field that never hands the text to egui: typed characters go straight into the
/// dialog's zeroised [`yaac_core::ssh_setup::Secret`]. (egui's own text box keeps copies of
/// its text in its undo history, so it is not used for secrets.) Shown masked unless `show`;
/// shown text is drawn one glyph at a time so no layout cache holds the whole secret.
pub(crate) fn secret_field(ui: &mut Ui, p: &Palette, secret: &mut yaac_core::ssh_setup::Secret, show: bool, width: f32, id: egui::Id) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 28.0), Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::Text);
    if resp.clicked() {
        ui.memory_mut(|m| m.request_focus(id));
    }
    ui.memory_mut(|m| m.interested_in_focus(id, ui.layer_id()));
    let focused = ui.memory(|m| m.has_focus(id));
    if focused {
        ui.input(|i| {
            for e in &i.events {
                match e {
                    egui::Event::Text(t) | egui::Event::Paste(t) => {
                        for c in t.chars().filter(|c| !c.is_control()) {
                            secret.push(c);
                        }
                    }
                    egui::Event::Key { key: egui::Key::Backspace, pressed: true, .. } => secret.pop(),
                    _ => {}
                }
            }
        });
    }
    ui.painter().rect_filled(rect, 5.0, p.faint);
    let font = theme::ui(size::UI);
    let mut x = rect.left() + 8.0;
    let cy = rect.center().y;
    if secret.is_empty() && !focused {
        let g = ui.fonts_mut(|f| f.layout_no_wrap("Password".to_owned(), font.clone(), p.text2));
        ui.painter().galley(pos2(x, cy - g.size().y / 2.0), g, p.text2);
    } else if show {
        for c in secret.expose().chars() {
            let g = ui.fonts_mut(|f| f.layout_no_wrap(c.to_string(), font.clone(), p.text));
            let w = g.size().x;
            ui.painter().galley(pos2(x, cy - g.size().y / 2.0), g, p.text);
            x += w;
        }
    } else {
        for _ in 0..secret.chars() {
            ui.painter().circle_filled(pos2(x + 4.0, cy), 3.0, p.text);
            x += 10.0;
        }
    }
    if focused {
        ui.painter().line_segment([pos2(x + 2.0, cy - 8.0), pos2(x + 2.0, cy + 8.0)], Stroke::new(1.5, p.accent));
        focus_ring(ui, rect, p);
    }
    resp
}

/// A quiet section heading inside a page: space above, small bold text, no rule.
pub(crate) fn heading(ui: &mut Ui, text: &str) {
    ui.add_space(18.0);
    ui.label(egui::RichText::new(text).font(theme::bold(size::SECTION)));
    ui.add_space(6.0);
}

/// Text with `inline code` in backticks drawn in monospace (a status message from a target).
pub(crate) fn rich_inline(ui: &mut Ui, p: &Palette, text: &str, color: Color32, font_size: f32) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for (i, part) in text.split('`').enumerate() {
            if part.is_empty() {
                continue;
            }
            let rt = if i % 2 == 1 {
                egui::RichText::new(part).font(theme::mono(font_size - 0.5)).color(color).background_color(p.faint)
            } else {
                egui::RichText::new(part).font(theme::ui(font_size)).color(color)
            };
            ui.label(rt);
        }
    });
}

/// A selector whose value removes a safety net (a mode that bypasses approvals): drawn in the
/// failed colour with a warning icon.
pub(crate) fn selector_warn(ui: &mut Ui, p: &Palette, label: &str) -> Response {
    let font = theme::ui(size::UI);
    let tw = text_size(ui, label, &font);
    let (rect, resp) = ui.allocate_exact_size(vec2(tw.x + 18.0 + 14.0 + 10.0, 26.0), Sense::click());
    if resp.hovered() || resp.has_focus() {
        ui.painter().rect_filled(rect, 5.0, p.faint);
    }
    icons::warning(ui.painter(), pos2(rect.left() + 12.0, rect.center().y), 14.0, p.failed, p.bg);
    let g = ui.fonts_mut(|f| f.layout_no_wrap(label.to_owned(), font, p.failed));
    ui.painter().galley(pos2(rect.left() + 23.0, rect.center().y - g.size().y / 2.0), g, p.failed);
    icons::chevron(ui.painter(), pos2(rect.right() - 11.0, rect.center().y + 1.0), 12.0, true, p.failed);
    if resp.has_focus() {
        focus_ring(ui, rect, p);
    }
    resp
}
