//! The window's own title strip (D104): the window has no system title bar. A strip across
//! the top of the window — each column's own background continues into it — holds three
//! buttons at the right (minimize, maximize or restore, close); the rest of the strip drags the
//! window, and a double click maximizes or restores it. While the window is not maximized,
//! its edges and corners resize it.

use eframe::egui::{self, Rect, Sense, ViewportCommand, pos2, vec2};

use crate::icons;
use crate::theme::Palette;

/// The strip's height.
pub(crate) const HEIGHT: f32 = 30.0;
const BUTTON_W: f32 = 44.0;
/// How far in from the window's edge a drag resizes it.
const GRIP: f32 = 5.0;

/// The strip's draggable part. Called before the content, so the content's own widgets in the
/// strip take precedence; [`buttons`] is called after it, so the buttons are on top.
pub(crate) fn drag_area(ui: &mut egui::Ui, full: Rect) {
    let bar = Rect::from_min_size(full.min, vec2(full.width() - 3.0 * BUTTON_W, HEIGHT));
    let resp = ui.interact(bar, egui::Id::unique("title-drag"), Sense::click_and_drag());
    let maximized = ui.input(|i| i.viewport().maximized.unwrap_or(false));
    if resp.double_clicked() {
        ui.ctx().send_viewport_cmd(ViewportCommand::Maximized(!maximized));
    } else if resp.drag_started_by(egui::PointerButton::Primary) {
        ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
    }
}

/// The three buttons at the top right, and the resize edges.
pub(crate) fn buttons(ui: &mut egui::Ui, full: Rect, p: &Palette) {
    let ctx = ui.ctx().clone();
    let maximized = ui.input(|i| i.viewport().maximized.unwrap_or(false));
    let mut x = full.right();
    let mut button = |id: &str, tip: &str, close: bool, draw: &dyn Fn(&egui::Painter, egui::Pos2, egui::Color32)| -> bool {
        x -= BUTTON_W;
        let r = Rect::from_min_size(pos2(x, full.top()), vec2(BUTTON_W, HEIGHT));
        let resp = ui.interact(r, egui::Id::unique(("title-button", id)), Sense::click()).on_hover_text(tip);
        let hot = resp.hovered() || resp.has_focus();
        let ink = if close && hot {
            ui.painter().rect_filled(r, 0.0, p.failed);
            egui::Color32::WHITE
        } else {
            if hot {
                ui.painter().rect_filled(r, 0.0, p.faint);
            }
            if hot { p.text } else { p.text2 }
        };
        draw(ui.painter(), r.center(), ink);
        resp.clicked()
    };
    if button("close", "Close", true, &|pa, c, ink| icons::close(pa, c, 16.0, ink)) {
        ctx.send_viewport_cmd(ViewportCommand::Close);
    }
    let tip = if maximized { "Restore" } else { "Maximize" };
    if button("max", tip, false, &|pa, c, ink| icons::maximize(pa, c, 15.0, ink, maximized)) {
        ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
    }
    if button("min", "Minimize", false, &|pa, c, ink| icons::minimize(pa, c, 15.0, ink)) {
        ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
    }
    if !maximized {
        resize_edges(ui, full);
    }
}

/// Without a system frame the window is resized from thin zones along its edges and corners.
fn resize_edges(ui: &mut egui::Ui, full: Rect) {
    use egui::{CursorIcon as C, ResizeDirection as D};
    let (l, r, t, b) = (full.left(), full.right(), full.top(), full.bottom());
    let g = GRIP;
    let zones = [
        (Rect::from_min_max(pos2(l, t), pos2(l + 2.0 * g, t + 2.0 * g)), D::NorthWest, C::ResizeNorthWest),
        (Rect::from_min_max(pos2(r - 2.0 * g, t), pos2(r, t + 2.0 * g)), D::NorthEast, C::ResizeNorthEast),
        (Rect::from_min_max(pos2(l, b - 2.0 * g), pos2(l + 2.0 * g, b)), D::SouthWest, C::ResizeSouthWest),
        (Rect::from_min_max(pos2(r - 2.0 * g, b - 2.0 * g), pos2(r, b)), D::SouthEast, C::ResizeSouthEast),
        (Rect::from_min_max(pos2(l + 2.0 * g, t), pos2(r - 2.0 * g, t + g)), D::North, C::ResizeNorth),
        (Rect::from_min_max(pos2(l + 2.0 * g, b - g), pos2(r - 2.0 * g, b)), D::South, C::ResizeSouth),
        (Rect::from_min_max(pos2(l, t + 2.0 * g), pos2(l + g, b - 2.0 * g)), D::West, C::ResizeWest),
        (Rect::from_min_max(pos2(r - g, t + 2.0 * g), pos2(r, b - 2.0 * g)), D::East, C::ResizeEast),
    ];
    for (i, (zone, dir, cursor)) in zones.into_iter().enumerate() {
        let resp = ui.interact(zone, egui::Id::unique(("window-edge", i)), Sense::drag()).on_hover_cursor(cursor);
        if resp.drag_started_by(egui::PointerButton::Primary) {
            ui.ctx().send_viewport_cmd(ViewportCommand::BeginResize(dir));
        }
    }
}
