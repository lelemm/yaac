//! The window's own title row (D104): the window has no system title bar. The top row of the
//! window is the title bar — like a browser's tab strip: over the sidebar it holds YAAC, back
//! and forward, New and the collapse button; over the thread, the thread's state, title and
//! tag; at the far right, minimize, maximize or restore, and close. Every empty part of the
//! row (and the non-interactive title) drags the window, and a double click maximizes or
//! restores it; the controls in it work as usual. While the window is not maximized, its edges
//! and corners resize it.

use eframe::egui::{self, Rect, Sense, ViewportCommand, pos2, vec2};

use crate::icons;
use crate::theme::Palette;

/// The title row's height.
pub(crate) const HEIGHT: f32 = 42.0;
const BUTTON_W: f32 = 44.0;
/// How far in from the window's edge a drag resizes it.
const GRIP: f32 = 5.0;

/// The row's draggable part. Called before the content, so the content's own widgets in the
/// strip take precedence; [`buttons`] is called after it, so the buttons are on top.
pub(crate) fn drag_area(ui: &mut egui::Ui, full: Rect) {
    let bar = Rect::from_min_size(full.min, vec2(full.width() - 3.0 * BUTTON_W, HEIGHT));
    let resp = ui.interact(bar, egui::Id::unique("title-drag"), Sense::click_and_drag());
    let maximized = ui.input(|i| i.viewport().maximized.unwrap_or(false));
    if resp.double_clicked() {
        ui.ctx().send_viewport_cmd(ViewportCommand::Maximized(!maximized));
    } else if resp.drag_started_by(egui::PointerButton::Primary) {
        // A press on a control in the row (a button senses clicks, not drags, so egui hands
        // the drag to the row underneath) never moves the window.
        let origin = ui.input(|i| i.pointer.press_origin());
        let on_control = origin.is_some_and(|o| {
            ui.ctx().interactive_rects_last_pass().iter().any(|r| r.contains(o) && r.area() < bar.area() * 0.5)
        });
        if !on_control {
            ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
        }
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

/// What a click on the update item asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UpdateClick {
    Download,
    Install,
}

/// The update item (D136), left of the window buttons: an update icon while a newer release
/// is offered, a mini progress bar while it downloads, "Restart to update" once it is
/// verified. Calm and monochrome, like the window buttons. Nothing while there is no update.
pub(crate) fn update(ui: &mut egui::Ui, full: Rect, p: &Palette, u: &crate::updater::Updater) -> Option<UpdateClick> {
    use crate::updater::Phase;
    let w = u.title_width();
    if w <= 0.0 {
        return None;
    }
    let r = Rect::from_min_size(pos2(full.right() - 3.0 * BUTTON_W - w, full.top()), vec2(w, HEIGHT));
    let label = |f: &crate::updater::Found| f.manifest.label();
    let tip = match &u.phase {
        Phase::Idle => return None,
        Phase::Available(f) => format!("YAAC {} is available · Download", label(f)),
        Phase::Downloading { found, done, total } => {
            let pct = done.saturating_mul(100).checked_div(*total).unwrap_or(0);
            format!("Downloading YAAC {} · {pct}%", label(found))
        }
        Phase::Ready(pending) => format!("YAAC {} is downloaded · Restart to update", pending.version),
        Phase::Failed { found, message } => format!("The download of YAAC {} failed: {message} · Try again", label(found)),
    };
    let clickable = !matches!(u.phase, Phase::Downloading { .. });
    let resp = ui.interact(r, egui::Id::unique("title-update"), if clickable { Sense::click() } else { Sense::hover() }).on_hover_text(tip);
    let hot = clickable && (resp.hovered() || resp.has_focus());
    if hot {
        ui.painter().rect_filled(r, 0.0, p.faint);
    }
    let ink = if hot { p.text } else { p.text2 };
    let c = r.center();
    match &u.phase {
        Phase::Available(_) | Phase::Failed { .. } => icons::download(ui.painter(), c, 16.0, ink),
        Phase::Downloading { done, total, .. } => {
            let track = Rect::from_center_size(c, vec2(w - 16.0, 4.0));
            ui.painter().rect_filled(track, 2.0, p.border);
            let frac = if *total == 0 { 0.0 } else { (*done as f32 / *total as f32).clamp(0.0, 1.0) };
            let fill = Rect::from_min_size(track.min, vec2(track.width() * frac, track.height()));
            ui.painter().rect_filled(fill, 2.0, p.text2);
        }
        Phase::Ready(_) => icons::refresh(ui.painter(), c, 16.0, ink),
        Phase::Idle => {}
    }
    if !resp.clicked() {
        return None;
    }
    match u.phase {
        Phase::Available(_) | Phase::Failed { .. } => Some(UpdateClick::Download),
        Phase::Ready(_) => Some(UpdateClick::Install),
        _ => None,
    }
}
