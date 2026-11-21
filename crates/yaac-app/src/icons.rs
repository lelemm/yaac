//! Marks and small icons, drawn with strokes so they stay crisp at any scale and are
//! recognisable in greyscale: state is a shape (and a word beside it), never a colour alone.

use std::f32::consts::TAU;

use eframe::egui::{self, Color32, Painter, Pos2, Rect, Shape, Stroke, Vec2, pos2, vec2};
use yaac_core::attention::Mark;

use crate::theme::Palette;

/// Icons are drawn at Material Design's high-emphasis opacity (87%), never fully opaque —
/// the owner's choice, for every icon, mark and logo.
pub(crate) const ICON_OPACITY: f32 = 0.87;

/// The painter an icon draws with: [`ICON_OPACITY`] applied once (an icon drawn by another
/// icon, or on an already faded painter, is not faded twice).
pub(crate) fn soft(p: &Painter) -> Painter {
    let mut q = p.clone();
    if q.opacity() >= 1.0 {
        q.multiply_opacity(ICON_OPACITY);
    }
    q
}

fn stroke(c: Color32, w: f32) -> Stroke {
    Stroke::new(w, c)
}

/// A state mark centred at `c`, `d` across.
pub(crate) fn mark(p: &Painter, c: Pos2, d: f32, m: Mark, pal: &Palette) {
    mark_fast(p, c, d, m, false, pal);
}

/// A thread's mark: as [`mark`], and a running turn in fast mode (D125) spins yellow and a
/// little faster.
pub(crate) fn thread_mark(p: &Painter, c: Pos2, d: f32, display: &yaac_core::attention::ThreadDisplay, pal: &Palette) {
    mark_fast(p, c, d, display.mark, display.fast, pal);
}

/// A state mark; `fast` only changes the running spinner.
pub(crate) fn mark_fast(p: &Painter, c: Pos2, d: f32, m: Mark, fast: bool, pal: &Palette) {
    let p = &soft(p);
    if fast && m == Mark::Running {
        spinner_turning(p, c, d, pal.fast, SPINNER_TURN_FAST);
        return;
    }
    let col = pal.mark(m);
    let r = d * 0.45;
    let w = (d * 0.11).max(1.3);
    // The symbol alone, in its colour — no circle around it (owner, D122). Idle, offline and
    // off draw nothing: the row's dimming and its words carry those.
    match m {
        Mark::Running => spinner(p, c, d, col),
        Mark::NeedsYou => exclaim(p, c, d, col),
        Mark::Paused => {
            let h = r * 0.8;
            for dx in [-r * 0.38, r * 0.38] {
                p.rect_filled(Rect::from_center_size(pos2(c.x + dx, c.y), vec2(w * 2.0, h * 2.0)), w * 0.6, col);
            }
        }
        Mark::Waiting => hourglass(p, c, d, col),
        Mark::Done => {
            let s = r * 0.9;
            p.line(vec![pos2(c.x - s, c.y + s * 0.05), pos2(c.x - s * 0.25, c.y + s * 0.75), pos2(c.x + s, c.y - s * 0.7)], stroke(col, w * 1.6));
        }
        Mark::Failed => {
            let s = r * 0.75;
            p.line_segment([pos2(c.x - s, c.y - s), pos2(c.x + s, c.y + s)], stroke(col, w * 1.6));
            p.line_segment([pos2(c.x - s, c.y + s), pos2(c.x + s, c.y - s)], stroke(col, w * 1.6));
        }
        Mark::Idle | Mark::Offline | Mark::Off => {}
    }
}

/// How often a visible spinner is redrawn: about 30 frames a second, so the slow turn stays smooth, only while one is on
/// screen and the window has focus. Unfocused (or hidden, or minimised) it stands still and
/// costs nothing.
const SPINNER_FRAME: std::time::Duration = std::time::Duration::from_millis(33);

/// Seconds per turn: slow and calm, but visibly moving.
const SPINNER_TURN: f64 = 2.5;

/// Seconds per turn of a running turn in fast mode (D125): a little quicker.
pub(crate) const SPINNER_TURN_FAST: f64 = 1.8;

/// Running: a three-quarter arc turning once every [`SPINNER_TURN`] seconds. Drawing it asks for the next frame, so
/// the window repaints only while a running mark is actually painted.
pub(crate) fn spinner(p: &Painter, c: Pos2, d: f32, col: Color32) {
    spinner_turning(p, c, d, col, SPINNER_TURN);
}

/// The spinner, turning once every `turn` seconds.
pub(crate) fn spinner_turning(p: &Painter, c: Pos2, d: f32, col: Color32, turn: f64) {
    let p = &soft(p);
    let r = d * 0.38;
    let w = (d * 0.12).max(1.5);
    let ctx = p.ctx();
    let (t, focused) = ctx.input(|i| (i.time, i.focused));
    #[allow(clippy::cast_possible_truncation, reason = "a phase in 0..1")]
    let start = ((t / turn).fract() as f32) * TAU;
    let n = 24;
    let pts: Vec<Pos2> = (0..=n)
        .map(|i| {
            let a = start + (i as f32 / n as f32) * TAU * 0.75;
            pos2(c.x + r * a.cos(), c.y + r * a.sin())
        })
        .collect();
    p.add(Shape::line(pts, Stroke::new(w, col)));
    if focused {
        ctx.request_repaint_after(SPINNER_FRAME);
    }
}

/// Waiting: an hourglass, the symbol alone.
pub(crate) fn hourglass(p: &Painter, c: Pos2, d: f32, col: Color32) {
    let p = &soft(p);
    let r = d * 0.45;
    let w = (d * 0.1).max(1.3);
    let (x0, x1, y0, y1) = (c.x - r * 0.62, c.x + r * 0.62, c.y - r * 0.9, c.y + r * 0.9);
    p.line_segment([pos2(x0 - w * 0.3, y0), pos2(x1 + w * 0.3, y0)], stroke(col, w * 1.2));
    p.line_segment([pos2(x0 - w * 0.3, y1), pos2(x1 + w * 0.3, y1)], stroke(col, w * 1.2));
    p.line(vec![pos2(x0, y0), pos2(c.x, c.y), pos2(x1, y1)], stroke(col, w));
    p.line(vec![pos2(x1, y0), pos2(c.x, c.y), pos2(x0, y1)], stroke(col, w));
    p.add(Shape::convex_polygon(vec![pos2(x0 + r * 0.15, y1 - w * 0.6), pos2(x1 - r * 0.15, y1 - w * 0.6), pos2(c.x, c.y + r * 0.35)], col, Stroke::NONE));
}

/// The needs-you mark (D113): the exclamation symbol alone, in `col` (the failed colour) —
/// a bar tapering towards its foot, and a dot. No disc behind it.
pub(crate) fn exclaim(p: &Painter, c: Pos2, d: f32, col: Color32) {
    let p = &soft(p);
    let h = d * 0.86;
    let top = c.y - h / 2.0;
    let wt = (d * 0.2).max(2.2);
    let wb = wt * 0.6;
    let bar_bottom = top + h * 0.62;
    p.circle_filled(pos2(c.x, top + wt / 2.0), wt / 2.0, col);
    p.add(egui::Shape::convex_polygon(
        vec![pos2(c.x - wt / 2.0, top + wt / 2.0), pos2(c.x + wt / 2.0, top + wt / 2.0), pos2(c.x + wb / 2.0, bar_bottom), pos2(c.x - wb / 2.0, bar_bottom)],
        col,
        Stroke::NONE,
    ));
    p.circle_filled(pos2(c.x, bar_bottom), wb / 2.0, col);
    p.circle_filled(pos2(c.x, top + h - wt * 0.55), wt * 0.55, col);
}

/// The ordering control (D116): three bars of falling length beside a down arrow.
pub(crate) fn order(p: &Painter, c: Pos2, d: f32, col: Color32) {
    let p = &soft(p);
    let s = Stroke::new(1.5, col);
    let x0 = c.x - d * 0.42;
    for (i, w) in [0.5_f32, 0.36, 0.22].iter().enumerate() {
        let y = c.y - d * 0.26 + i as f32 * d * 0.26;
        p.line_segment([pos2(x0, y), pos2(x0 + d * w, y)], s);
    }
    let ax = c.x + d * 0.3;
    p.line_segment([pos2(ax, c.y - d * 0.32), pos2(ax, c.y + d * 0.3)], s);
    p.line(vec![pos2(ax - d * 0.14, c.y + d * 0.15), pos2(ax, c.y + d * 0.3), pos2(ax + d * 0.14, c.y + d * 0.15)], s);
}

/// A calendar page: a rounded box, two rings at the top, a rule under the header.
pub(crate) fn calendar(p: &Painter, c: Pos2, d: f32, col: Color32) {
    let p = &soft(p);
    let s = Stroke::new((d * 0.09).max(1.2), col);
    let r = Rect::from_center_size(c + vec2(0.0, d * 0.04), vec2(d * 0.84, d * 0.74));
    p.rect_stroke(r, d * 0.12, s, egui::StrokeKind::Middle);
    p.hline(r.x_range(), r.top() + d * 0.22, s);
    for dx in [-0.2, 0.2] {
        let x = c.x + d * dx;
        p.line_segment([pos2(x, r.top() - d * 0.1), pos2(x, r.top() + d * 0.08)], s);
    }
}

/// Two opposed arrows (⇄): "in contact with" (D118).
pub(crate) fn swap(p: &Painter, c: Pos2, d: f32, col: Color32) {
    let p = &soft(p);
    let s = Stroke::new((d * 0.1).max(1.2), col);
    let (l, r) = (c.x - d * 0.4, c.x + d * 0.4);
    let (y1, y2) = (c.y - d * 0.18, c.y + d * 0.18);
    let h = d * 0.16;
    p.line_segment([pos2(l, y1), pos2(r, y1)], s);
    p.line(vec![pos2(r - h, y1 - h), pos2(r, y1), pos2(r - h, y1 + h)], s);
    p.line_segment([pos2(l, y2), pos2(r, y2)], s);
    p.line(vec![pos2(l + h, y2 - h), pos2(l, y2), pos2(l + h, y2 + h)], s);
}

/// The warning triangle with an exclamation mark.
pub(crate) fn warning(p: &Painter, c: Pos2, d: f32, col: Color32, ink: Color32) {
    let p = &soft(p);
    let h = d * 0.9;
    let top = pos2(c.x, c.y - h * 0.52);
    let l = pos2(c.x - h * 0.56, c.y + h * 0.44);
    let rr = pos2(c.x + h * 0.56, c.y + h * 0.44);
    p.add(Shape::convex_polygon(vec![top, rr, l], col, Stroke::new(1.0, col)));
    let w = (d * 0.1).max(1.2);
    p.line_segment([pos2(c.x, c.y - h * 0.18), pos2(c.x, c.y + h * 0.14)], stroke(ink, w));
    p.circle_filled(pos2(c.x, c.y + h * 0.29), w * 0.6, ink);
}

pub(crate) fn dashed_circle(p: &Painter, c: Pos2, r: f32, s: Stroke) {
    let p = &soft(p);
    let n = 8;
    for i in 0..n {
        let a0 = i as f32 / n as f32 * TAU;
        let a1 = a0 + TAU / n as f32 * 0.55;
        let pts: Vec<Pos2> = (0..=4).map(|k| {
            let a = a0 + (a1 - a0) * k as f32 / 4.0;
            c + vec2(a.cos(), a.sin()) * r
        }).collect();
        p.line(pts, s);
    }
}


pub(crate) fn info(p: &Painter, c: Pos2, d: f32, col: Color32) {
    let p = &soft(p);
    let r = d * 0.45;
    let w = (d * 0.1).max(1.2);
    p.circle_stroke(c, r, stroke(col, w));
    p.line_segment([pos2(c.x, c.y - r * 0.05), pos2(c.x, c.y + r * 0.5)], stroke(col, w));
    p.circle_filled(pos2(c.x, c.y - r * 0.42), w * 0.7, col);
}

pub(crate) fn question(p: &Painter, c: Pos2, d: f32, col: Color32) {
    let p = &soft(p);
    let r = d * 0.45;
    let w = (d * 0.1).max(1.2);
    p.circle_stroke(c, r, stroke(col, w));
    let pts: Vec<Pos2> = (0..=8)
        .map(|k| {
            let a = -std::f32::consts::PI + k as f32 / 8.0 * (std::f32::consts::PI * 1.4);
            pos2(c.x + a.cos() * r * 0.35, c.y - r * 0.2 + a.sin() * r * 0.3)
        })
        .collect();
    p.line(pts, stroke(col, w));
    p.line_segment([pos2(c.x, c.y + r * 0.05), pos2(c.x, c.y + r * 0.25)], stroke(col, w));
    p.circle_filled(pos2(c.x, c.y + r * 0.55), w * 0.6, col);
}

/// Fast mode's lightning bolt (D125), `d` tall: filled when on, an outline when off — the
/// two read apart in greyscale; the colour is the caller's (the fast yellow when on).
pub(crate) fn bolt(p: &Painter, c: Pos2, d: f32, filled: bool, col: Color32) {
    let p = &soft(p);
    let h = d / 2.0;
    let w = h * 0.62;
    // The six corners, from the top: down to the left corner, in to the lower notch, down to
    // the bottom tip, up to the right corner, in to the upper notch.
    let v = [(0.30, -1.0), (-0.50, 0.12), (-0.02, 0.12), (-0.30, 1.0), (0.50, -0.12), (0.02, -0.12)];
    let pts: Vec<Pos2> = v.iter().map(|&(x, y)| pos2(c.x + x * w * 2.0, c.y + y * h)).collect();
    if filled {
        // A concave outline: filled as four triangles.
        let mut mesh = egui::Mesh::default();
        for q in &pts {
            mesh.colored_vertex(*q, col);
        }
        for [a, b, t] in [[0, 1, 5], [1, 2, 5], [5, 2, 4], [2, 3, 4]] {
            mesh.add_triangle(a, b, t);
        }
        p.add(Shape::mesh(mesh));
    } else {
        p.add(Shape::closed_line(pts, Stroke::new((d * 0.08).max(1.2), col)));
    }
}

pub(crate) fn chevron(p: &Painter, c: Pos2, d: f32, open: bool, col: Color32) {
    let p = &soft(p);
    let s = d * 0.22;
    let w = (d * 0.09).max(1.2);
    let pts = if open {
        vec![pos2(c.x - s * 1.4, c.y - s * 0.6), pos2(c.x, c.y + s * 0.8), pos2(c.x + s * 1.4, c.y - s * 0.6)]
    } else {
        vec![pos2(c.x - s * 0.6, c.y - s * 1.4), pos2(c.x + s * 0.8, c.y), pos2(c.x - s * 0.6, c.y + s * 1.4)]
    };
    p.line(pts, stroke(col, w));
}

/// ↗: "spawned by an agent", "To".
pub(crate) fn arrow_out(p: &Painter, c: Pos2, d: f32, col: Color32) {
    let p = &soft(p);
    let s = d * 0.32;
    let w = (d * 0.09).max(1.3);
    p.line_segment([pos2(c.x - s, c.y + s), pos2(c.x + s, c.y - s)], stroke(col, w));
    p.line(vec![pos2(c.x - s * 0.1, c.y - s), pos2(c.x + s, c.y - s), pos2(c.x + s, c.y + s * 0.1)], stroke(col, w));
}

/// ↙: "From".
pub(crate) fn arrow_in(p: &Painter, c: Pos2, d: f32, col: Color32) {
    let p = &soft(p);
    let s = d * 0.32;
    let w = (d * 0.09).max(1.3);
    p.line_segment([pos2(c.x + s, c.y - s), pos2(c.x - s, c.y + s)], stroke(col, w));
    p.line(vec![pos2(c.x - s, c.y - s * 0.1), pos2(c.x - s, c.y + s), pos2(c.x + s * 0.1, c.y + s)], stroke(col, w));
}

pub(crate) fn lock(p: &Painter, c: Pos2, d: f32, col: Color32) {
    let p = &soft(p);
    let w = (d * 0.09).max(1.2);
    let body = Rect::from_center_size(pos2(c.x, c.y + d * 0.14), vec2(d * 0.62, d * 0.46));
    p.rect_stroke(body, 1.5, stroke(col, w), eframe::egui::StrokeKind::Middle);
    let r = d * 0.2;
    let top = pos2(c.x, body.top() - d * 0.02);
    let pts: Vec<Pos2> = (0..=10).map(|k| {
        let a = std::f32::consts::PI + k as f32 / 10.0 * std::f32::consts::PI;
        pos2(top.x + a.cos() * r, top.y + a.sin() * r * 1.2)
    }).collect();
    p.line(pts, stroke(col, w));
}

pub(crate) fn folder(p: &Painter, c: Pos2, d: f32, col: Color32) {
    let p = &soft(p);
    let w = (d * 0.08).max(1.1);
    let r = Rect::from_center_size(c + vec2(0.0, d * 0.05), vec2(d * 0.8, d * 0.55));
    p.line(
        vec![
            pos2(r.left(), r.top() - d * 0.08),
            pos2(r.left() + d * 0.28, r.top() - d * 0.08),
            pos2(r.left() + d * 0.36, r.top()),
            pos2(r.right(), r.top()),
            r.right_bottom(),
            r.left_bottom(),
            pos2(r.left(), r.top() - d * 0.08),
        ],
        stroke(col, w),
    );
}

pub(crate) fn person(p: &Painter, c: Pos2, d: f32, col: Color32) {
    let p = &soft(p);
    let w = (d * 0.08).max(1.2);
    p.circle_stroke(pos2(c.x, c.y - d * 0.15), d * 0.17, stroke(col, w));
    let pts: Vec<Pos2> = (0..=12).map(|k| {
        let a = std::f32::consts::PI + k as f32 / 12.0 * std::f32::consts::PI;
        pos2(c.x + a.cos() * d * 0.3, c.y + d * 0.35 + a.sin() * d * 0.28)
    }).collect();
    p.line(pts, stroke(col, w));
}

pub(crate) fn people(p: &Painter, c: Pos2, d: f32, col: Color32) {
    let p = &soft(p);
    person(p, c - vec2(d * 0.12, 0.0), d * 0.85, col);
    let w = (d * 0.07).max(1.1);
    p.circle_stroke(pos2(c.x + d * 0.22, c.y - d * 0.18), d * 0.12, stroke(col, w));
}


pub(crate) fn terminal(p: &Painter, c: Pos2, d: f32, col: Color32) {
    let p = &soft(p);
    let w = (d * 0.08).max(1.1);
    let r = Rect::from_center_size(c, vec2(d * 0.8, d * 0.62));
    p.rect_stroke(r, 2.0, stroke(col, w), eframe::egui::StrokeKind::Middle);
    let s = d * 0.12;
    let a = pos2(r.left() + d * 0.14, r.center().y - s);
    p.line(vec![a, pos2(a.x + s, a.y + s), pos2(a.x, a.y + 2.0 * s)], stroke(col, w));
    p.line_segment([pos2(c.x, r.center().y + s), pos2(c.x + d * 0.2, r.center().y + s)], stroke(col, w));
}

pub(crate) fn file(p: &Painter, c: Pos2, d: f32, col: Color32, broken: bool) {
    let p = &soft(p);
    let w = (d * 0.08).max(1.1);
    let r = Rect::from_center_size(c, vec2(d * 0.56, d * 0.74));
    let f = d * 0.18;
    p.line(
        vec![r.left_top(), pos2(r.right() - f, r.top()), pos2(r.right(), r.top() + f), r.right_bottom(), r.left_bottom(), r.left_top()],
        stroke(col, w),
    );
    if broken {
        p.line_segment([pos2(r.left() + d * 0.1, r.center().y - d * 0.1), pos2(r.right() - d * 0.1, r.center().y + d * 0.12)], stroke(col, w));
        p.line_segment([pos2(r.left() + d * 0.1, r.center().y + d * 0.12), pos2(r.right() - d * 0.1, r.center().y - d * 0.1)], stroke(col, w));
    }
}

pub(crate) fn image(p: &Painter, c: Pos2, d: f32, col: Color32) {
    let p = &soft(p);
    let w = (d * 0.08).max(1.1);
    let r = Rect::from_center_size(c, vec2(d * 0.8, d * 0.62));
    p.rect_stroke(r, 2.0, stroke(col, w), eframe::egui::StrokeKind::Middle);
    p.line(vec![pos2(r.left() + 2.0, r.bottom() - 2.0), pos2(c.x - d * 0.05, c.y), pos2(r.right() - 2.0, r.bottom() - 2.0)], stroke(col, w));
    p.circle_filled(pos2(r.right() - d * 0.18, r.top() + d * 0.16), d * 0.06, col);
}

pub(crate) fn gear(p: &Painter, c: Pos2, d: f32, col: Color32) {
    let p = &soft(p);
    let w = (d * 0.09).max(1.2);
    p.circle_stroke(c, d * 0.16, stroke(col, w));
    let n = 8;
    let pts: Vec<Pos2> = (0..=n * 2).map(|i| {
        let a = i as f32 / (n * 2) as f32 * TAU;
        let r = if i % 2 == 0 { d * 0.42 } else { d * 0.32 };
        c + vec2(a.cos(), a.sin()) * r
    }).collect();
    p.line(pts, stroke(col, w));
}

pub(crate) fn search(p: &Painter, c: Pos2, d: f32, col: Color32) {
    let p = &soft(p);
    let w = (d * 0.09).max(1.2);
    let cc = c - vec2(d * 0.08, d * 0.08);
    p.circle_stroke(cc, d * 0.26, stroke(col, w));
    p.line_segment([cc + vec2(d * 0.19, d * 0.19), c + vec2(d * 0.36, d * 0.36)], stroke(col, w));
}


pub(crate) fn stop_square(p: &Painter, c: Pos2, d: f32, col: Color32) {
    let p = &soft(p);
    p.rect_filled(Rect::from_center_size(c, Vec2::splat(d * 0.5)), 2.0, col);
}





/// The Return key.
pub(crate) fn return_key(p: &Painter, c: Pos2, d: f32, col: Color32) {
    let p = &soft(p);
    let w = (d * 0.09).max(1.2);
    let s = d * 0.32;
    p.line(vec![pos2(c.x + s, c.y - s), pos2(c.x + s, c.y + s * 0.3), pos2(c.x - s, c.y + s * 0.3)], stroke(col, w));
    p.line(vec![pos2(c.x - s * 0.45, c.y - s * 0.2), pos2(c.x - s, c.y + s * 0.3), pos2(c.x - s * 0.45, c.y + s * 0.8)], stroke(col, w));
}

/// A git branch.
pub(crate) fn branch(p: &Painter, c: Pos2, d: f32, col: Color32) {
    let p = &soft(p);
    let w = (d * 0.09).max(1.1);
    let a = pos2(c.x - d * 0.18, c.y - d * 0.3);
    let b = pos2(c.x - d * 0.18, c.y + d * 0.3);
    let e = pos2(c.x + d * 0.22, c.y - d * 0.22);
    for q in [a, b, e] {
        p.circle_stroke(q, d * 0.09, stroke(col, w));
    }
    p.line_segment([a + vec2(0.0, d * 0.1), b - vec2(0.0, d * 0.1)], stroke(col, w));
    p.line(vec![e + vec2(0.0, d * 0.1), pos2(e.x, c.y), pos2(a.x, c.y + d * 0.16)], stroke(col, w));
}

/// Git change kinds: a small square with a plus, a dot, or a minus.
pub(crate) fn git_square(p: &Painter, c: Pos2, d: f32, col: Color32, kind: char) {
    let p = &soft(p);
    let w = (d * 0.09).max(1.1);
    let r = Rect::from_center_size(c, Vec2::splat(d * 0.72));
    p.rect_stroke(r, 2.0, stroke(col, w), eframe::egui::StrokeKind::Middle);
    let s = d * 0.2;
    match kind {
        '+' => {
            p.line_segment([pos2(c.x - s, c.y), pos2(c.x + s, c.y)], stroke(col, w));
            p.line_segment([pos2(c.x, c.y - s), pos2(c.x, c.y + s)], stroke(col, w));
        }
        '-' => {
            p.line_segment([pos2(c.x - s, c.y), pos2(c.x + s, c.y)], stroke(col, w));
        }
        _ => {
            p.circle_filled(c, d * 0.1, col);
        }
    }
}

/// An arrow pointing up (or down).
pub(crate) fn arrow_vertical(p: &Painter, c: Pos2, d: f32, col: Color32, up: bool) {
    let p = &soft(p);
    let w = (d * 0.09).max(1.2);
    let s = d * 0.32;
    let dir = if up { -1.0 } else { 1.0 };
    p.line_segment([pos2(c.x, c.y - s * dir), pos2(c.x, c.y + s * dir)], stroke(col, w));
    p.line(vec![pos2(c.x - s * 0.7, c.y + s * dir * 0.2), pos2(c.x, c.y + s * dir), pos2(c.x + s * 0.7, c.y + s * dir * 0.2)], stroke(col, w));
}

/// An arrow pointing left.
pub(crate) fn arrow_left(p: &Painter, c: Pos2, d: f32, col: Color32) {
    let p = &soft(p);
    let w = (d * 0.09).max(1.2);
    let s = d * 0.32;
    p.line_segment([pos2(c.x - s, c.y), pos2(c.x + s, c.y)], stroke(col, w));
    p.line(vec![pos2(c.x - s * 0.2, c.y - s * 0.7), pos2(c.x - s, c.y), pos2(c.x - s * 0.2, c.y + s * 0.7)], stroke(col, w));
}

/// A fork: one stem splitting in two.
pub(crate) fn fork(p: &Painter, c: Pos2, d: f32, col: Color32) {
    let p = &soft(p);
    let s = d * 0.36;
    let w = (d * 0.09).max(1.2);
    let base = pos2(c.x, c.y + s);
    let mid = pos2(c.x, c.y);
    p.line_segment([base, mid], stroke(col, w));
    p.line_segment([mid, pos2(c.x - s * 0.8, c.y - s)], stroke(col, w));
    p.line_segment([mid, pos2(c.x + s * 0.8, c.y - s)], stroke(col, w));
    for q in [base, pos2(c.x - s * 0.8, c.y - s), pos2(c.x + s * 0.8, c.y - s)] {
        p.circle_filled(q, w * 1.3, col);
    }
}

/// Copy: two overlapping sheets.
pub(crate) fn copy(p: &Painter, c: Pos2, d: f32, col: Color32) {
    let p = &soft(p);
    let w = (d * 0.09).max(1.1);
    let s = d * 0.5;
    let back = Rect::from_min_size(pos2(c.x - s * 0.75, c.y - s * 0.75), vec2(s, s));
    let front = Rect::from_min_size(pos2(c.x - s * 0.25, c.y - s * 0.25), vec2(s, s));
    p.line(vec![pos2(back.left(), back.bottom()), back.left_top(), pos2(back.right(), back.top())], stroke(col, w));
    p.rect_stroke(front, 1.5, stroke(col, w), eframe::egui::StrokeKind::Middle);
}

/// A check mark (done; "Copied").
pub(crate) fn check(p: &Painter, c: Pos2, d: f32, col: Color32) {
    let p = &soft(p);
    let w = (d * 0.1).max(1.3);
    let s = d * 0.36;
    p.line(vec![pos2(c.x - s, c.y), pos2(c.x - s * 0.3, c.y + s * 0.7), pos2(c.x + s, c.y - s * 0.6)], stroke(col, w));
}

/// Back (‹) and forward (›): a single chevron pointing left or right.
pub(crate) fn nav(p: &Painter, c: Pos2, d: f32, forward: bool, col: Color32) {
    let p = &soft(p);
    let w = (d * 0.1).max(1.4);
    let s = d * 0.22;
    let dir = if forward { 1.0 } else { -1.0 };
    p.line(vec![pos2(c.x - s * dir * 0.6, c.y - s * 1.4), pos2(c.x + s * dir * 0.8, c.y), pos2(c.x - s * dir * 0.6, c.y + s * 1.4)], stroke(col, w));
}

/// Plus: a new thread.
pub(crate) fn plus(p: &Painter, c: Pos2, d: f32, col: Color32) {
    let p = &soft(p);
    let w = (d * 0.1).max(1.4);
    let s = d * 0.34;
    p.line_segment([pos2(c.x - s, c.y), pos2(c.x + s, c.y)], stroke(col, w));
    p.line_segment([pos2(c.x, c.y - s), pos2(c.x, c.y + s)], stroke(col, w));
}

/// The sidebar toggle: a frame with a narrow left column.
pub(crate) fn sidebar(p: &Painter, c: Pos2, d: f32, col: Color32) {
    let p = &soft(p);
    let w = (d * 0.08).max(1.2);
    let r = Rect::from_center_size(c, vec2(d * 0.8, d * 0.66));
    p.rect_stroke(r, 2.0, stroke(col, w), eframe::egui::StrokeKind::Middle);
    let x = r.left() + r.width() * 0.34;
    p.line_segment([pos2(x, r.top()), pos2(x, r.bottom())], stroke(col, w));
}

/// A paper clip: the attachment button. One wire: up the right side, round the top, down the
/// left side, round a smaller bottom, and up the middle.
pub(crate) fn paperclip(p: &Painter, c: Pos2, d: f32, col: Color32) {
    let p = &soft(p);
    use std::f32::consts::PI;
    let w = (d * 0.08).max(1.3);
    let arc = |cx: f32, cy: f32, r: f32, from: f32, to: f32| -> Vec<Pos2> {
        (0..=10).map(|i| {
            let a = from + (to - from) * i as f32 / 10.0;
            pos2(cx + r * a.cos(), cy + r * a.sin())
        }).collect()
    };
    let (top, bot) = (c.y - d * 0.42, c.y + d * 0.42);
    let r = d * 0.2;
    let rb = d * 0.12;
    let mut pts = vec![pos2(c.x + r, c.y + d * 0.1), pos2(c.x + r, top + r)];
    pts.extend(arc(c.x, top + r, r, 0.0, -PI));
    pts.push(pos2(c.x - r, bot - rb));
    pts.extend(arc(c.x - r + rb, bot - rb, rb, PI, 0.0));
    pts.push(pos2(c.x - r + 2.0 * rb, top + r + d * 0.08));
    p.line(pts, stroke(col, w));
}

/// A small cross: remove.
pub(crate) fn close(p: &Painter, c: Pos2, d: f32, col: Color32) {
    let p = &soft(p);
    let w = (d * 0.1).max(1.3);
    let s = d * 0.26;
    p.line_segment([pos2(c.x - s, c.y - s), pos2(c.x + s, c.y + s)], stroke(col, w));
    p.line_segment([pos2(c.x - s, c.y + s), pos2(c.x + s, c.y - s)], stroke(col, w));
}

/// A connection: two nodes joined by a link; crossed out when there is none (the hub cannot
/// be reached).
pub(crate) fn connection(p: &Painter, c: Pos2, d: f32, col: Color32, crossed: bool) {
    let p = &soft(p);
    let w = (d * 0.08).max(1.2);
    let a = pos2(c.x - d * 0.28, c.y + d * 0.18);
    let b = pos2(c.x + d * 0.28, c.y - d * 0.18);
    p.circle_stroke(a, d * 0.12, stroke(col, w));
    p.circle_stroke(b, d * 0.12, stroke(col, w));
    let v = (b - a).normalized() * d * 0.12;
    p.line_segment([a + v, b - v], stroke(col, w));
    if crossed {
        p.line_segment([pos2(c.x - d * 0.36, c.y - d * 0.36), pos2(c.x + d * 0.36, c.y + d * 0.36)], stroke(col, w));
    }
}

/// Minimize: a short horizontal bar.
pub(crate) fn minimize(p: &Painter, c: Pos2, d: f32, col: Color32) {
    let p = &soft(p);
    let s = d * 0.32;
    p.line_segment([pos2(c.x - s, c.y), pos2(c.x + s, c.y)], stroke(col, (d * 0.08).max(1.1)));
}

/// Maximize: a square; restore: two overlapping squares.
pub(crate) fn maximize(p: &Painter, c: Pos2, d: f32, col: Color32, restore: bool) {
    let p = &soft(p);
    let w = (d * 0.08).max(1.1);
    let s = d * 0.56;
    if restore {
        let o = d * 0.1;
        let front = Rect::from_center_size(pos2(c.x - o, c.y + o), Vec2::splat(s * 0.85));
        p.rect_stroke(front, 1.0, stroke(col, w), eframe::egui::StrokeKind::Middle);
        let back = front.translate(vec2(2.0 * o, -2.0 * o));
        p.line(vec![pos2(back.left(), front.top()), back.left_top(), back.right_top(), back.right_bottom(), pos2(front.right(), back.bottom())], stroke(col, w));
    } else {
        p.rect_stroke(Rect::from_center_size(c, Vec2::splat(s)), 1.0, stroke(col, w), eframe::egui::StrokeKind::Middle);
    }
}

/// A push pin, tilted: the Pinned section in the strip.
pub(crate) fn pin(p: &Painter, c: Pos2, d: f32, col: Color32) {
    let p = &soft(p);
    let w = (d * 0.08).max(1.2);
    // Drawn upright around the origin, then turned 45°.
    let (sin, cos) = (std::f32::consts::FRAC_PI_4.sin(), std::f32::consts::FRAC_PI_4.cos());
    let t = |x: f32, y: f32| pos2(c.x + x * cos - y * sin, c.y + x * sin + y * cos);
    let (hw, bw) = (d * 0.16, d * 0.3);
    let head = vec![t(-hw, -d * 0.42), t(hw, -d * 0.42), t(hw, -d * 0.12), t(bw, d * 0.04), t(-bw, d * 0.04), t(-hw, -d * 0.12), t(-hw, -d * 0.42)];
    p.line(head, stroke(col, w));
    p.line_segment([t(0.0, d * 0.04), t(0.0, d * 0.44)], stroke(col, w));
}
