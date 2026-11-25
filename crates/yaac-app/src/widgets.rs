//! Small widgets in the house style: no outlines; one filled button per screen; plain-text
//! buttons with a faint fill on hover; selectors as a word and a chevron; a 2 px focus ring.

use eframe::egui::{self, Color32, CornerRadius, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};
use yaac_core::attention::Mark;

use crate::icons;
use crate::theme::{self, Palette, size};

/// The 2 px focus ring (a neutral).
pub(crate) fn focus_ring(ui: &Ui, rect: Rect, p: &Palette) {
    ui.painter().rect_stroke(rect.expand(2.0), CornerRadius::same(6), Stroke::new(2.0, p.focus), StrokeKind::Outside);
}

/// A row with a chevron and a quiet label that opens and closes what is under it. Returns
/// true when clicked.
pub(crate) fn disclosure_row(ui: &mut Ui, p: &Palette, label: &str, open: bool, id: egui::Id) -> bool {
    let font = theme::ui(size::SMALL);
    let g = ui.fonts_mut(|f| f.layout_no_wrap(label.to_owned(), font, p.text2));
    let w = g.size().x + 22.0;
    let (rect, _) = ui.allocate_exact_size(vec2(w, 20.0), Sense::hover());
    let resp = ui.interact(rect, id, Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    if resp.hovered() || resp.has_focus() {
        ui.painter().rect_filled(rect.expand2(vec2(3.0, 0.0)), 4.0, p.faint);
    }
    icons::chevron(ui.painter(), pos2(rect.left() + 6.0, rect.center().y), 9.0, open, p.text2);
    ui.painter().galley(pos2(rect.left() + 16.0, rect.center().y - g.size().y / 2.0), g, p.text2);
    if resp.has_focus() {
        focus_ring(ui, rect, p);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

fn text_size(ui: &Ui, text: &str, font: &egui::FontId) -> Vec2 {
    ui.fonts_mut(|f| f.layout_no_wrap(text.to_owned(), font.clone(), Color32::WHITE)).size()
}

/// The primary action: the one filled button on a screen. `key` is its shortcut hint.
pub(crate) fn primary(ui: &mut Ui, p: &Palette, label: &str, key: Option<&str>) -> Response {
    button(ui, p, label, key, true, None)
}

/// A filled button with a stable id (one a test or a screenshot can find).
pub(crate) fn primary_with_id(ui: &mut Ui, p: &Palette, label: &str, id: egui::Id) -> Response {
    button_with(ui, p, label, None, true, None, Some(id))
}

/// A plain-text button: no outline, a faint fill on hover.
pub(crate) fn plain(ui: &mut Ui, p: &Palette, label: &str, key: Option<&str>) -> Response {
    button(ui, p, label, key, false, None)
}

fn button(ui: &mut Ui, p: &Palette, label: &str, key: Option<&str>, filled: bool, color: Option<Color32>) -> Response {
    button_with(ui, p, label, key, filled, color, None)
}

#[allow(clippy::too_many_arguments, reason = "the one painter behind every button kind")]
fn button_with(ui: &mut Ui, p: &Palette, label: &str, key: Option<&str>, filled: bool, color: Option<Color32>, id: Option<egui::Id>) -> Response {
    let font = if filled { theme::bold(size::UI) } else { theme::ui(size::UI) };
    let key_font = theme::ui(size::SMALL);
    let tw = text_size(ui, label, &font);
    let kw = key.map_or(0.0, |k| if k == "Enter" { 14.0 + 10.0 } else { text_size(ui, k, &key_font).x + 10.0 });
    let pad = vec2(12.0, 5.0);
    let size = vec2(tw.x + kw + pad.x * 2.0, (tw.y + pad.y * 2.0).max(28.0));
    let (rect, resp) = match id {
        Some(id) => {
            let (_, rect) = ui.allocate_space(size);
            (rect, ui.interact(rect, id, Sense::click()))
        }
        None => ui.allocate_exact_size(size, Sense::click()),
    };
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let fg = if filled { p.on_primary } else { color.unwrap_or(p.text) };
    if filled {
        let fill = if resp.hovered() { p.primary.gamma_multiply(0.88) } else { p.primary };
        ui.painter().rect_filled(rect, 5.0, fill);
    } else if resp.hovered() || resp.has_focus() {
        ui.painter().rect_filled(rect, 5.0, p.faint);
    }
    let galley = ui.fonts_mut(|f| f.layout_no_wrap(label.to_owned(), font, fg));
    let y = rect.center().y - galley.size().y / 2.0;
    ui.painter().galley(pos2(rect.left() + pad.x, y), galley, fg);
    if let Some(k) = key {
        let kc = if filled { p.on_primary } else { p.text2 };
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

/// A link: not coloured — the surrounding text's colour, set bolder; no underline at rest,
/// underlined on hover or keyboard focus.
#[allow(clippy::needless_pass_by_value, reason = "callers pass a fresh FontId; only its size is used")]
pub(crate) fn link(ui: &mut Ui, p: &Palette, text: &str, font: egui::FontId) -> Response {
    let font = theme::bold(font.size);
    let resp = ui.add(egui::Label::new(egui::RichText::new(text).font(font).color(p.text)).selectable(false).sense(Sense::click()));
    if resp.hovered() || resp.has_focus() {
        let r = resp.rect;
        ui.painter().hline(r.x_range(), r.bottom() - 1.0, Stroke::new(1.0, p.text));
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

/// The lightning toggle or speed menu under the composer.
pub(crate) fn speed_control(ui: &mut Ui, p: &Palette, speed: yaac_core::controls::FastToggle) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(if speed.offers_ultrafast { 40.0 } else { 28.0 }, 26.0), Sense::click());
    let label = if speed.offers_ultrafast { format!("Speed · {}", speed.current().label()) } else { yaac_core::controls::FAST_TOOLTIP.to_owned() };
    let resp = resp.on_hover_text(&label);
    let on = speed.on || speed.ultrafast_on;
    resp.widget_info(|| egui::WidgetInfo::selected(if speed.offers_ultrafast { egui::Role::Button } else { egui::Role::CheckBox }, true, on, &label));
    if on {
        ui.painter().rect_filled(rect, 5.0, p.selected);
    } else if resp.hovered() || resp.has_focus() {
        ui.painter().rect_filled(rect, 5.0, p.faint);
    }
    let col = if speed.ultrafast_on {
        p.ultrafast
    } else if on {
        p.fast
    } else if resp.hovered() {
        p.text
    } else {
        p.text2
    };
    let center = rect.center() - vec2(if speed.offers_ultrafast { 5.0 } else { 0.0 }, 0.0);
    icons::bolt(ui.painter(), center, 16.0, on, col);
    if speed.offers_ultrafast {
        let arrow = center + vec2(14.0, 0.0);
        ui.painter().line_segment([arrow + vec2(-3.0, -2.0), arrow + vec2(0.0, 1.0)], Stroke::new(1.0, p.text2));
        ui.painter().line_segment([arrow + vec2(0.0, 1.0), arrow + vec2(3.0, -2.0)], Stroke::new(1.0, p.text2));
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
    // One line: a line break or other control character in the text would draw as a missing
    // glyph, so they become spaces here, for every single-line row.
    let text = if text.chars().any(char::is_control) { yaac_core::message::one_line(text) } else { text.to_owned() };
    let mut job = egui::text::LayoutJob::simple_singleline(text, font.clone(), color);
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

/// A switch: a filled track (the primary-button neutral when on) and a knob. No outline.
pub(crate) fn switch(ui: &mut Ui, p: &Palette, on: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(32.0, 18.0), Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let off_fill = if p.dark { Color32::from_rgb(0x3A, 0x40, 0x4C) } else { Color32::from_rgb(0xC9, 0xCE, 0xD6) };
    let track = if on { p.primary } else { off_fill };
    ui.painter().rect_filled(rect, 9.0, track);
    let x = if on { rect.right() - 9.0 } else { rect.left() + 9.0 };
    let knob = if on { p.on_primary } else { p.bg };
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
        ui.painter().line_segment([pos2(x + 2.0, cy - 8.0), pos2(x + 2.0, cy + 8.0)], Stroke::new(1.5, p.text));
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

/// How wide a form field is.
#[derive(Clone, Copy, Debug)]
pub(crate) enum FieldWidth {
    /// The rest of its column.
    Fill,
    /// About this many characters (a port, a short code).
    Chars(usize),
}

/// One labelled field of a form.
#[derive(Debug)]
pub(crate) struct Field<'a> {
    pub label: &'a str,
    pub text: &'a mut String,
    pub hint: &'a str,
    pub width: FieldWidth,
    pub mono: bool,
}

/// Below this width a two-column form puts one field per row instead of shrinking fields.
pub(crate) const FORM_TWO_COLUMNS: f32 = 560.0;
/// The second column's width (label and field) in a two-column form.
const FORM_RIGHT: f32 = 230.0;

/// A form of labelled fields, each row one or two fields. Field widths come from the space
/// actually available (never from the previous frame's column widths, which is what squeezed
/// fields in `egui::Grid` to a few pixels): the first column fills what the second leaves; a
/// `Chars` field takes about that many characters. Too narrow for two columns, every field
/// gets its own row. Not `editable`: the values as text.
pub(crate) fn form(ui: &mut Ui, p: &Palette, rows: Vec<Vec<Field<'_>>>, editable: bool) {
    let font = theme::ui(size::UI);
    let small = theme::ui(size::SMALL);
    let char_w = text_size(ui, "0", &font).x;
    let avail = ui.available_width();
    let two = avail >= FORM_TWO_COLUMNS && rows.iter().any(|r| r.len() > 1);
    let gap = 10.0;
    let col_label = |c: usize| rows.iter().filter_map(|r| r.get(c)).map(|f| text_size(ui, f.label, &small).x).fold(0.0_f32, f32::max);
    let (l0, l1) = if two { (col_label(0), col_label(1)) } else { (col_label(0).max(col_label(1)), 0.0) };
    let lay = |ui: &mut Ui, fields: Vec<Field<'_>>| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for (c, f) in fields.into_iter().enumerate() {
                let lw = if c == 0 { l0 } else { l1 };
                let (r, _) = ui.allocate_exact_size(vec2(lw, 24.0), Sense::hover());
                let g = ui.fonts_mut(|fo| fo.layout_no_wrap(f.label.to_owned(), small.clone(), p.text2));
                ui.painter().galley(pos2(r.left(), r.center().y - g.size().y / 2.0), g, p.text2);
                let column = match (two, c) {
                    (true, 0) => avail - l0 - FORM_RIGHT - gap * 3.0,
                    (true, _) => FORM_RIGHT - l1 - gap,
                    (false, _) => avail - lw - gap,
                };
                let w = match f.width {
                    FieldWidth::Fill => column,
                    FieldWidth::Chars(n) => (n as f32 * char_w + 20.0).min(column),
                }
                .max(60.0);
                if editable {
                    // The field's own margins are inside its width.
                    field(ui, p, f.text, f.hint, (w - 16.0).max(40.0), f.mono);
                } else {
                    ui.add_sized(vec2(w, 24.0), egui::Label::new(f.text.as_str()).truncate());
                }
            }
        });
    };
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 8.0;
        for row in rows {
            if two {
                lay(ui, row);
            } else {
                for f in row {
                    lay(ui, vec![f]);
                }
            }
        }
    });
}

/// A small icon button: no outline, a faint fill on hover; the hover text says what it does.
pub(crate) fn icon_button(ui: &mut Ui, p: &Palette, d: f32, hover: &str, f: impl FnOnce(&egui::Painter, egui::Pos2, f32, Color32)) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(d + 8.0), Sense::click());
    let resp = resp.on_hover_text(hover);
    if resp.hovered() || resp.has_focus() {
        ui.painter().rect_filled(rect, 4.0, p.faint);
    }
    let col = if resp.hovered() { p.text } else { p.text2 };
    f(ui.painter(), rect.center(), d, col);
    resp
}

/// The one menu style (every context menu, dropdown and picker): items are full-width rows
/// with no fill of their own, sitting on the menu's raised surface; hover and keyboard focus
/// show the row tint of a hovered sidebar row, a press a little stronger; no gaps between
/// rows; separators are hairlines; a small inner padding and corner radius.
pub(crate) fn menu_popup<'a>(popup: egui::Popup<'a>, p: &Palette) -> egui::Popup<'a> {
    let p = *p;
    popup
        .style(move |s: &mut egui::Style| {
            s.spacing.button_padding = egui::vec2(10.0, 5.0);
            s.spacing.item_spacing = egui::vec2(8.0, 0.0);
            s.spacing.menu_margin = egui::Margin::same(4);
            s.spacing.interact_size.y = 26.0;
            let w = &mut s.visuals.widgets;
            for v in [&mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
                v.bg_stroke = Stroke::NONE;
                v.corner_radius = CornerRadius::same(4);
                v.expansion = 0.0;
            }
            w.inactive.weak_bg_fill = Color32::TRANSPARENT;
            w.hovered.weak_bg_fill = p.faint;
            w.open.weak_bg_fill = p.faint;
            w.active.weak_bg_fill = p.selected;
            w.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
            s.visuals.window_fill = p.raised;
        })
        .layout(egui::Layout::top_down_justified(egui::Align::Min))
}

/// What kind of menu row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ItemKind {
    /// An ordinary action.
    Normal,
    /// One that destroys or stops something (Delete, Stop…): the failed colour.
    Danger,
    /// Not available now: the unavailable colour, no hover.
    Disabled,
}

/// One menu row, with an optional right-aligned shortcut in the secondary colour.
pub(crate) fn menu_item(ui: &mut Ui, p: &Palette, label: &str, shortcut: Option<&str>, kind: ItemKind) -> Response {
    let color = match kind {
        ItemKind::Normal => p.text,
        ItemKind::Danger => p.failed,
        ItemKind::Disabled => p.unavailable,
    };
    let mut b = egui::Button::new(egui::RichText::new(label).color(color));
    if let Some(s) = shortcut {
        b = b.shortcut_text(egui::RichText::new(s).color(p.text2).font(theme::ui(size::SMALL)));
    }
    ui.add_enabled(kind != ItemKind::Disabled, b)
}

/// A full-width menu row the caller draws into (for rows with more than a label): the row
/// tint on hover or keyboard focus, stronger when pressed, the selection tint when chosen.
/// Disabled rows do not react.
pub(crate) fn menu_row(ui: &mut Ui, p: &Palette, selected: bool, enabled: bool) -> (Response, Rect) {
    let h = ui.spacing().interact_size.y.max(26.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), h), if enabled { Sense::click() } else { Sense::hover() });
    let fill = if !enabled {
        None
    } else if resp.is_pointer_button_down_on() || selected {
        Some(p.selected)
    } else if resp.hovered() || resp.has_focus() {
        Some(p.faint)
    } else {
        None
    };
    if let Some(f) = fill {
        ui.painter().rect_filled(rect, 4.0, f);
    }
    (resp, rect)
}

/// `under` moved towards `over` by `t` (0..=1): the hover tint of a surface.
pub(crate) fn blend(under: Color32, over: Color32, t: f32) -> Color32 {
    let mix = |a: u8, b: u8| -> u8 {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "a channel in 0..=255")]
        let v = (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8;
        v
    };
    Color32::from_rgb(mix(under.r(), over.r()), mix(under.g(), over.g()), mix(under.b(), over.b()))
}

/// The hover fill of a quiet control on `under`: between the surface and the selected fill.
pub(crate) fn hover_fill(p: &Palette, under: Color32) -> Color32 {
    blend(under, p.selected, 0.6)
}

/// A toolbar icon button (the side pane's, docs/12-files.md#visual-design-of-the-side-pane):
/// 28 px, no outline, a faint fill on hover, the selected fill while pressed or toggled on;
/// disabled ones draw in the unavailable colour and do not react. The hover text names the
/// action and its shortcut.
pub(crate) fn tool_button(ui: &mut Ui, p: &Palette, under: Color32, hover: &str, enabled: bool, on: bool, f: impl FnOnce(&egui::Painter, egui::Pos2, f32, Color32)) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(28.0), if enabled { Sense::click() } else { Sense::hover() });
    let resp = if enabled { resp.on_hover_text(hover) } else { resp.on_hover_text(format!("{hover} (unavailable)")) };
    let fill = if enabled && (resp.is_pointer_button_down_on() || on) {
        Some(p.selected)
    } else if enabled && resp.hovered() {
        Some(hover_fill(p, under))
    } else {
        None
    };
    if let Some(fill) = fill {
        ui.painter().rect_filled(rect, 5.0, fill);
    }
    let col = if !enabled {
        p.unavailable.gamma_multiply(0.55)
    } else if resp.hovered() || on {
        p.text
    } else {
        p.text2
    };
    f(ui.painter(), rect.center(), 16.0, col);
    if resp.has_focus() {
        ui.painter().rect_stroke(rect, CornerRadius::same(5), Stroke::new(2.0, p.focus), StrokeKind::Inside);
    }
    resp
}

/// One line of text cut at its start with "…" to fit `width` (a path, whose end matters).
pub(crate) fn elided_start(ui: &Ui, text: &str, font: &egui::FontId, color: Color32, width: f32) -> std::sync::Arc<egui::Galley> {
    let lay = |s: String| ui.fonts_mut(|f| f.layout_no_wrap(s, font.clone(), color));
    let full = lay(text.to_owned());
    if full.size().x <= width {
        return full;
    }
    let chars: Vec<char> = text.chars().collect();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "a character count")]
    let mut keep = ((chars.len() as f32) * (width / full.size().x)) as usize + 1;
    loop {
        keep = keep.min(chars.len());
        let g = lay(format!("…{}", chars[chars.len() - keep..].iter().collect::<String>()));
        if g.size().x <= width || keep == 0 {
            return g;
        }
        keep -= 1;
    }
}
