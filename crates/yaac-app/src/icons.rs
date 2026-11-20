//! Marks and small icons, drawn with strokes so they stay crisp at any scale and are
//! recognisable in greyscale: state is a shape (and a word beside it), never a colour alone.

use std::f32::consts::TAU;

use eframe::egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, Vec2, pos2, vec2};
use yaac_core::attention::Mark;

use crate::theme::Palette;

fn stroke(c: Color32, w: f32) -> Stroke {
    Stroke::new(w, c)
}

/// A state mark centred at `c`, `d` across.
pub(crate) fn mark(p: &Painter, c: Pos2, d: f32, m: Mark, pal: &Palette) {
    let col = pal.mark(m);
    let r = d * 0.45;
    let w = (d * 0.11).max(1.3);
    match m {
        Mark::Running => {
            p.circle_stroke(c, r, stroke(col, w));
            let s = r * 0.5;
            p.add(Shape::convex_polygon(
                vec![pos2(c.x - s * 0.6, c.y - s), pos2(c.x + s, c.y), pos2(c.x - s * 0.6, c.y + s)],
                col,
                Stroke::NONE,
            ));
        }
        Mark::NeedsYou => warning(p, c, d, col, pal.bg),
        Mark::Paused => {
            p.circle_stroke(c, r, stroke(col, w));
            let h = r * 0.5;
            for dx in [-r * 0.22, r * 0.22] {
                p.line_segment([pos2(c.x + dx, c.y - h), pos2(c.x + dx, c.y + h)], stroke(col, w * 1.1));
            }
        }
        Mark::Waiting => clock(p, c, d, col),
        Mark::Done => {
            p.circle_filled(c, r, col);
            let s = r * 0.5;
            p.line(vec![pos2(c.x - s, c.y), pos2(c.x - s * 0.2, c.y + s * 0.75), pos2(c.x + s, c.y - s * 0.6)], stroke(pal.bg, w * 1.2));
        }
        Mark::Failed => {
            p.circle_stroke(c, r, stroke(col, w));
            let s = r * 0.42;
            p.line_segment([pos2(c.x - s, c.y - s), pos2(c.x + s, c.y + s)], stroke(col, w));
            p.line_segment([pos2(c.x - s, c.y + s), pos2(c.x + s, c.y - s)], stroke(col, w));
        }
        Mark::Idle => {
            p.circle_stroke(c, r, stroke(col, w));
        }
        Mark::Offline | Mark::Off => dashed_circle(p, c, r, stroke(col, w)),
    }
}

/// The warning triangle with an exclamation mark.
pub(crate) fn warning(p: &Painter, c: Pos2, d: f32, col: Color32, ink: Color32) {
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

pub(crate) fn clock(p: &Painter, c: Pos2, d: f32, col: Color32) {
    let r = d * 0.45;
    let w = (d * 0.1).max(1.2);
    p.circle_stroke(c, r, stroke(col, w));
    p.line(vec![pos2(c.x, c.y - r * 0.55), c, pos2(c.x + r * 0.45, c.y + r * 0.2)], stroke(col, w));
}


pub(crate) fn info(p: &Painter, c: Pos2, d: f32, col: Color32) {
    let r = d * 0.45;
    let w = (d * 0.1).max(1.2);
    p.circle_stroke(c, r, stroke(col, w));
    p.line_segment([pos2(c.x, c.y - r * 0.05), pos2(c.x, c.y + r * 0.5)], stroke(col, w));
    p.circle_filled(pos2(c.x, c.y - r * 0.42), w * 0.7, col);
}

pub(crate) fn question(p: &Painter, c: Pos2, d: f32, col: Color32) {
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

pub(crate) fn chevron(p: &Painter, c: Pos2, d: f32, open: bool, col: Color32) {
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
    let s = d * 0.32;
    let w = (d * 0.09).max(1.3);
    p.line_segment([pos2(c.x - s, c.y + s), pos2(c.x + s, c.y - s)], stroke(col, w));
    p.line(vec![pos2(c.x - s * 0.1, c.y - s), pos2(c.x + s, c.y - s), pos2(c.x + s, c.y + s * 0.1)], stroke(col, w));
}

/// ↙: "From".
pub(crate) fn arrow_in(p: &Painter, c: Pos2, d: f32, col: Color32) {
    let s = d * 0.32;
    let w = (d * 0.09).max(1.3);
    p.line_segment([pos2(c.x + s, c.y - s), pos2(c.x - s, c.y + s)], stroke(col, w));
    p.line(vec![pos2(c.x - s, c.y - s * 0.1), pos2(c.x - s, c.y + s), pos2(c.x + s * 0.1, c.y + s)], stroke(col, w));
}

pub(crate) fn lock(p: &Painter, c: Pos2, d: f32, col: Color32) {
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
    let w = (d * 0.08).max(1.2);
    p.circle_stroke(pos2(c.x, c.y - d * 0.15), d * 0.17, stroke(col, w));
    let pts: Vec<Pos2> = (0..=12).map(|k| {
        let a = std::f32::consts::PI + k as f32 / 12.0 * std::f32::consts::PI;
        pos2(c.x + a.cos() * d * 0.3, c.y + d * 0.35 + a.sin() * d * 0.28)
    }).collect();
    p.line(pts, stroke(col, w));
}

pub(crate) fn people(p: &Painter, c: Pos2, d: f32, col: Color32) {
    person(p, c - vec2(d * 0.12, 0.0), d * 0.85, col);
    let w = (d * 0.07).max(1.1);
    p.circle_stroke(pos2(c.x + d * 0.22, c.y - d * 0.18), d * 0.12, stroke(col, w));
}

/// The agent's mark: a four-pointed spark.
pub(crate) fn spark(p: &Painter, c: Pos2, d: f32, col: Color32) {
    let r = d * 0.42;
    let k = r * 0.22;
    let pts = vec![
        pos2(c.x, c.y - r), pos2(c.x + k, c.y - k), pos2(c.x + r, c.y), pos2(c.x + k, c.y + k),
        pos2(c.x, c.y + r), pos2(c.x - k, c.y + k), pos2(c.x - r, c.y), pos2(c.x - k, c.y - k),
    ];
    p.add(Shape::Path(eframe::epaint::PathShape::closed_line(pts.clone(), Stroke::new(1.0, col))));
    // Fill as two convex quads.
    p.add(Shape::convex_polygon(vec![pts[0], pts[1], pts[3], pts[4], pts[5], pts[7]], col, Stroke::NONE));
    p.add(Shape::convex_polygon(vec![pts[1], pts[2], pts[3], pts[5], pts[6], pts[7]], col, Stroke::NONE));
}

pub(crate) fn terminal(p: &Painter, c: Pos2, d: f32, col: Color32) {
    let w = (d * 0.08).max(1.1);
    let r = Rect::from_center_size(c, vec2(d * 0.8, d * 0.62));
    p.rect_stroke(r, 2.0, stroke(col, w), eframe::egui::StrokeKind::Middle);
    let s = d * 0.12;
    let a = pos2(r.left() + d * 0.14, r.center().y - s);
    p.line(vec![a, pos2(a.x + s, a.y + s), pos2(a.x, a.y + 2.0 * s)], stroke(col, w));
    p.line_segment([pos2(c.x, r.center().y + s), pos2(c.x + d * 0.2, r.center().y + s)], stroke(col, w));
}

pub(crate) fn file(p: &Painter, c: Pos2, d: f32, col: Color32, broken: bool) {
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
    let w = (d * 0.08).max(1.1);
    let r = Rect::from_center_size(c, vec2(d * 0.8, d * 0.62));
    p.rect_stroke(r, 2.0, stroke(col, w), eframe::egui::StrokeKind::Middle);
    p.line(vec![pos2(r.left() + 2.0, r.bottom() - 2.0), pos2(c.x - d * 0.05, c.y), pos2(r.right() - 2.0, r.bottom() - 2.0)], stroke(col, w));
    p.circle_filled(pos2(r.right() - d * 0.18, r.top() + d * 0.16), d * 0.06, col);
}

pub(crate) fn gear(p: &Painter, c: Pos2, d: f32, col: Color32) {
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
    let w = (d * 0.09).max(1.2);
    let cc = c - vec2(d * 0.08, d * 0.08);
    p.circle_stroke(cc, d * 0.26, stroke(col, w));
    p.line_segment([cc + vec2(d * 0.19, d * 0.19), c + vec2(d * 0.36, d * 0.36)], stroke(col, w));
}


pub(crate) fn stop_square(p: &Painter, c: Pos2, d: f32, col: Color32) {
    p.rect_filled(Rect::from_center_size(c, Vec2::splat(d * 0.5)), 2.0, col);
}





/// The Return key.
pub(crate) fn return_key(p: &Painter, c: Pos2, d: f32, col: Color32) {
    let w = (d * 0.09).max(1.2);
    let s = d * 0.32;
    p.line(vec![pos2(c.x + s, c.y - s), pos2(c.x + s, c.y + s * 0.3), pos2(c.x - s, c.y + s * 0.3)], stroke(col, w));
    p.line(vec![pos2(c.x - s * 0.45, c.y - s * 0.2), pos2(c.x - s, c.y + s * 0.3), pos2(c.x - s * 0.45, c.y + s * 0.8)], stroke(col, w));
}

/// A git branch.
pub(crate) fn branch(p: &Painter, c: Pos2, d: f32, col: Color32) {
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
    let w = (d * 0.09).max(1.2);
    let s = d * 0.32;
    let dir = if up { -1.0 } else { 1.0 };
    p.line_segment([pos2(c.x, c.y - s * dir), pos2(c.x, c.y + s * dir)], stroke(col, w));
    p.line(vec![pos2(c.x - s * 0.7, c.y + s * dir * 0.2), pos2(c.x, c.y + s * dir), pos2(c.x + s * 0.7, c.y + s * dir * 0.2)], stroke(col, w));
}

/// An arrow pointing left.
pub(crate) fn arrow_left(p: &Painter, c: Pos2, d: f32, col: Color32) {
    let w = (d * 0.09).max(1.2);
    let s = d * 0.32;
    p.line_segment([pos2(c.x - s, c.y), pos2(c.x + s, c.y)], stroke(col, w));
    p.line(vec![pos2(c.x - s * 0.2, c.y - s * 0.7), pos2(c.x - s, c.y), pos2(c.x - s * 0.2, c.y + s * 0.7)], stroke(col, w));
}
