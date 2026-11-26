//! The egui terminal renderer (docs/13-terminal.md#drawing) and the Terminal tab, the panel
//! under the composer, and their input.
//!
//! The grid is **one mesh per frame**: the default background, per-row background runs, the
//! selection and search highlights, per-row glyph quads, then the cursor — all on egui's font
//! texture, so one draw call. Glyphs come from egui's own atlas: each distinct character is laid
//! out once (a one-character galley, so no ligatures), its quad cached, and cells copy it. Row
//! meshes are cached and rebuilt only for the rows [`yaac_term::Frame::damaged`] lists; box
//! drawing and block characters are drawn as rectangles so they join exactly.
#![allow(clippy::cast_possible_truncation, clippy::type_complexity, clippy::needless_range_loop, clippy::chunks_exact_to_as_chunks, reason = "cell and vertex counts are small; cache keys are tuples; quads are read in fours")]
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, Pos2, Rect, Vec2, epaint::Vertex, pos2, vec2};
use yaac_core::ThreadKey;
use yaac_term::{CellSide, Color, CursorShape, Flags, Frame, Key, Mods, MouseAction, MouseButton, SelectKind, Shortcut, Theme};

use crate::app::YaacApp;
use crate::terminal::SessionState;
use crate::theme::{self, Palette, size};
use crate::{icons, widgets};

/// Padding around the grid inside its area.
const PAD: Vec2 = vec2(8., 4.);
/// Cursor blink half-period.
const BLINK: f64 = 0.53;
/// Italic shear (x per unit of height above the baseline).
const SHEAR: f32 = 0.18;

fn focus_slot() -> egui::Id {
    egui::Id::unique("yaac-terminal-focus")
}

/// Whether a terminal has keyboard focus (the dispatcher then leaves its keys alone).
pub(crate) fn focused(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<egui::Id>(focus_slot())).is_some_and(|id| ctx.memory(|m| m.has_focus(id)))
}

/// The widget id of a session's grid.
pub(crate) fn widget_id(session: u64) -> egui::Id {
    egui::Id::unique(("yaac-terminal", session))
}

pub(crate) fn theme_of(p: &Palette) -> Theme {
    let base = if p.dark { Theme::DARK } else { Theme::LIGHT };
    let rgb = |c: Color32| yaac_term::Rgb(c.r(), c.g(), c.b());
    Theme { bg: rgb(p.bg), fg: rgb(p.text), cursor: rgb(p.text), cursor_text: rgb(p.bg), ..base }
}

fn c32(c: yaac_term::Rgb) -> Color32 {
    Color32::from_rgb(c.0, c.1, c.2)
}

/// Cell metrics at the current scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Metrics {
    /// Cell width and height (points); the height is a whole number of pixels.
    pub(crate) cw: f32,
    pub(crate) rh: f32,
    ppp: f32,
    /// The baseline, from the cell's top.
    base: f32,
    /// One physical pixel, in points.
    px: f32,
}

impl Metrics {
    fn snap(&self, v: f32) -> f32 {
        (v * self.ppp).round() / self.ppp
    }
    fn x(&self, col: usize) -> f32 {
        self.snap(col as f32 * self.cw)
    }
    fn line(&self) -> f32 {
        // Light lines: one pixel per point of scale, at least one pixel.
        self.snap((self.ppp.round().max(1.)) / self.ppp).max(self.px)
    }
}

pub(crate) fn font() -> egui::FontId {
    theme::mono(size::MONO)
}

/// One cached glyph: quads relative to the pen on the baseline.
#[derive(Clone, Debug)]
struct Glyph {
    quads: Vec<[(Pos2, Pos2); 4]>,
    advance: f32,
    color: bool,
}

#[derive(Default)]
struct Atlas {
    key: Option<(u32, [usize; 2], u64)>,
    metrics: Option<Metrics>,
    glyphs: HashMap<char, Option<Arc<Glyph>>>,
}

/// Cached meshes of one row: under the selection (backgrounds) and over it (glyphs, lines).
#[derive(Default, Clone)]
struct RowMesh {
    bg: egui::Mesh,
    fg: egui::Mesh,
}

#[derive(Default)]
struct Grid {
    key: Option<((u32, [usize; 2], u64), usize, usize, Theme)>,
    rows: Vec<RowMesh>,
    assembled: Option<(u64, Pos2, u64, Arc<egui::Mesh>)>,
}

/// The renderer's caches, kept in egui's memory.
#[derive(Default)]
pub(crate) struct Renderer {
    atlas: Atlas,
    grids: HashMap<u64, Grid>,
    /// Rows rebuilt and frames assembled (benchmarks and tests).
    pub(crate) rows_built: u64,
    pub(crate) assembled: u64,
}

type Shared = Arc<Mutex<Renderer>>;

pub(crate) fn renderer(ctx: &egui::Context) -> Shared {
    ctx.data_mut(|d| d.get_temp_mut_or_default::<Shared>(egui::Id::unique("yaac-term-renderer")).clone())
}

impl Atlas {
    fn refresh(&mut self, ui: &egui::Ui) -> Metrics {
        let ppp = ui.ctx().pixels_per_point();
        let font = font();
        // egui clears its glyph atlas when it is 80 % full (and on option changes), at the same
        // size, together with its galley cache: a probe galley that is no longer the same one
        // means every cached quad points at stale texels.
        let (size, probe) = ui.fonts_mut(|f| {
            let g = f.layout_no_wrap("M".into(), font.clone(), Color32::WHITE);
            (f.font_image_size(), Arc::as_ptr(&g) as usize as u64)
        });
        let key = (ppp.to_bits(), size, probe);
        if self.key != Some(key) || self.metrics.is_none() {
            self.key = Some(key);
            self.glyphs.clear();
            let (cw, row_h, galley) = ui.fonts_mut(|f| {
                let width = (' '..='~').map(|ch| f.glyph_width(&font, ch)).fold(0.0, f32::max);
                (width, f.row_height(&font), f.layout_no_wrap("M".into(), font.clone(), Color32::WHITE))
            });
            let rh = (row_h * ppp).ceil() / ppp;
            let baseline = galley.rows.first().and_then(|r| r.row.glyphs.first().map(|g| r.pos.y + g.pos.y)).unwrap_or(row_h * 0.8);
            let top = ((rh - galley.size().y) / 2. * ppp).round() / ppp;
            self.metrics = Some(Metrics { cw, rh, ppp, base: top + baseline, px: 1. / ppp });
        }
        self.metrics.expect("metrics set above")
    }

    fn glyph(&mut self, ui: &egui::Ui, c: char) -> Option<Arc<Glyph>> {
        if let Some(g) = self.glyphs.get(&c) {
            return g.clone();
        }
        let font = font();
        let (galley, tex) = ui.fonts_mut(|f| (f.layout_no_wrap(c.to_string(), font, Color32::WHITE), f.font_image_size()));
        let g = galley.rows.first().and_then(|placed| {
            let row = &placed.row;
            let first = row.glyphs.first()?;
            let baseline = placed.pos.y + first.pos.y;
            let v = &row.visuals;
            let verts = &v.mesh.vertices[v.glyph_vertex_range.clone()];
            let norm = vec2(1. / tex[0] as f32, 1. / tex[1] as f32);
            let quads = verts
                .chunks_exact(4)
                .map(|q| {
                    let f = |v: &Vertex| (pos2(v.pos.x + placed.pos.x, v.pos.y + placed.pos.y - baseline), pos2(v.uv.x * norm.x, v.uv.y * norm.y));
                    [f(&q[0]), f(&q[1]), f(&q[2]), f(&q[3])]
                })
                .collect::<Vec<_>>();
            Some(Arc::new(Glyph { quads, advance: row.glyphs.iter().map(|g| g.advance_width).sum(), color: first.is_color }))
        });
        self.glyphs.insert(c, g.clone());
        g
    }
}

// ---- mesh building ------------------------------------------------------------------------

fn rect(mesh: &mut egui::Mesh, r: Rect, color: Color32) {
    if r.width() > 0. && r.height() > 0. {
        mesh.add_colored_rect(r, color);
    }
}

/// A thick segment (diagonals, arcs).
fn segment(mesh: &mut egui::Mesh, a: Pos2, b: Pos2, w: f32, color: Color32) {
    let d = b - a;
    let len = d.length();
    if len <= 0. {
        return;
    }
    let n = vec2(-d.y, d.x) / len * (w / 2.);
    let i = mesh.vertices.len() as u32;
    for p in [a + n, b + n, b - n, a - n] {
        mesh.vertices.push(Vertex { pos: p, uv: egui::epaint::WHITE_UV, color });
    }
    mesh.indices.extend_from_slice(&[i, i + 1, i + 2, i, i + 2, i + 3]);
}

/// Pen at `pen` (baseline), glyph scaled to `fit` points wide if it is wider.
#[allow(clippy::too_many_arguments)]
fn glyph_quads(mesh: &mut egui::Mesh, g: &Glyph, pen: Pos2, fit: f32, color: Color32, italic: bool, bold: Option<f32>, m: &Metrics) {
    let sx = if g.advance > fit * 1.02 { fit / g.advance } else { 1. };
    let color = if g.color { Color32::WHITE } else { color };
    let passes: &[f32] = if bold.is_some() { &[0., 1.] } else { &[0.] };
    for &pass in passes {
        let dx = pass * bold.unwrap_or(0.);
        for q in &g.quads {
            let i = mesh.vertices.len() as u32;
            for (p, uv) in q {
                let mut x = pen.x + p.x * sx + dx;
                if italic {
                    x -= p.y * SHEAR;
                }
                let x = if italic { x } else { m.snap(x - dx) + dx };
                mesh.vertices.push(Vertex { pos: pos2(x, pen.y + p.y), uv: *uv, color });
            }
            // egui's glyph quads are left-top, right-top, left-bottom, right-bottom.
            mesh.indices.extend_from_slice(&[i, i + 1, i + 2, i + 2, i + 1, i + 3]);
        }
    }
}

/// Box drawing (U+2500–U+257F): weights left, right, up, down (0 none, 1 light, 2 heavy,
/// 3 double), or a special shape.
fn box_weights(c: char) -> Option<[u8; 4]> {
    const T: &str = "1100220000110022000000000000000000000000\
                     0101020101020202100110021002200201100120\
                     0210022010012001200120022002001110011120\
                     1022100221012201200212021202201110112011\
                     1022110222012202101121011102221201112021\
                     1112111211121112112121122121222111222112\
                     2122211222221222";
    let i = (c as u32).checked_sub(0x2500)? as usize;
    if i < 0x4C {
        let d = T.as_bytes().get(i * 4..i * 4 + 4)?;
        return Some([d[0] - b'0', d[1] - b'0', d[2] - b'0', d[3] - b'0']);
    }
    Some(match c {
        '═' => [3, 3, 0, 0],
        '║' => [0, 0, 3, 3],
        '╒' => [0, 3, 0, 1],
        '╓' => [0, 1, 0, 3],
        '╔' => [0, 3, 0, 3],
        '╕' => [3, 0, 0, 1],
        '╖' => [1, 0, 0, 3],
        '╗' => [3, 0, 0, 3],
        '╘' => [0, 3, 1, 0],
        '╙' => [0, 1, 3, 0],
        '╚' => [0, 3, 3, 0],
        '╛' => [3, 0, 1, 0],
        '╜' => [1, 0, 3, 0],
        '╝' => [3, 0, 3, 0],
        '╞' => [0, 3, 1, 1],
        '╟' => [0, 1, 3, 3],
        '╠' => [0, 3, 3, 3],
        '╡' => [3, 0, 1, 1],
        '╢' => [1, 0, 3, 3],
        '╣' => [3, 0, 3, 3],
        '╤' => [3, 3, 0, 1],
        '╥' => [1, 1, 0, 3],
        '╦' => [3, 3, 0, 3],
        '╧' => [3, 3, 1, 0],
        '╨' => [1, 1, 3, 0],
        '╩' => [3, 3, 3, 0],
        '╪' => [3, 3, 1, 1],
        '╫' => [1, 1, 3, 3],
        '╬' => [3, 3, 3, 3],
        '╴' => [1, 0, 0, 0],
        '╵' => [0, 0, 1, 0],
        '╶' => [0, 1, 0, 0],
        '╷' => [0, 0, 0, 1],
        '╸' => [2, 0, 0, 0],
        '╹' => [0, 0, 2, 0],
        '╺' => [0, 2, 0, 0],
        '╻' => [0, 0, 0, 2],
        '╼' => [1, 2, 0, 0],
        '╽' => [0, 0, 1, 2],
        '╾' => [2, 1, 0, 0],
        '╿' => [0, 0, 2, 1],
        _ => return None,
    })
}

/// Draw a box-drawing or block character as shapes. False when `c` is not one.
#[allow(clippy::too_many_lines)]
fn special(mesh: &mut egui::Mesh, c: char, cell: Rect, fg: Color32, bg: Color32, m: &Metrics) -> bool {
    let (l, r, t, b) = (cell.left(), cell.right(), cell.top(), cell.bottom());
    let cx = m.snap(cell.center().x - m.line() / 2.) + m.line() / 2.;
    let cy = m.snap(cell.center().y - m.line() / 2.) + m.line() / 2.;
    let light = m.line();
    let heavy = m.snap(light * 2.).max(light + m.px);
    let gap = light;
    let code = c as u32;
    // Dashed lines.
    let dashes = match c {
        '┄' | '┅' | '┆' | '┇' => Some(3),
        '┈' | '┉' | '┊' | '┋' => Some(4),
        '╌' | '╍' | '╎' | '╏' => Some(2),
        _ => None,
    };
    if let Some(n) = dashes {
        let heavy_dash = matches!(c, '┅' | '┇' | '┉' | '┋' | '╍' | '╏');
        let w = if heavy_dash { heavy } else { light };
        let vertical = matches!(c, '┆' | '┇' | '┊' | '┋' | '╎' | '╏');
        for i in 0..n {
            let f0 = i as f32 / n as f32;
            let f1 = (i as f32 + 0.6) / n as f32;
            if vertical {
                rect(mesh, Rect::from_min_max(pos2(cx - w / 2., m.snap(t + (b - t) * f0)), pos2(cx + w / 2., m.snap(t + (b - t) * f1))), fg);
            } else {
                rect(mesh, Rect::from_min_max(pos2(m.snap(l + (r - l) * f0), cy - w / 2.), pos2(m.snap(l + (r - l) * f1), cy + w / 2.)), fg);
            }
        }
        return true;
    }
    if let Some([wl, wr, wu, wd]) = box_weights(c) {
        let thick = |w: u8| if w == 2 { heavy } else { light };
        let any_double_v = wu == 3 || wd == 3;
        let any_double_h = wl == 3 || wr == 3;
        // Horizontal arms.
        for (w, left) in [(wl, true), (wr, false)] {
            if w == 0 {
                continue;
            }
            if w == 3 {
                for (y, side_up) in [(cy - gap - light / 2., true), (cy + gap + light / 2., false)] {
                    let vert = if side_up { wu } else { wd };
                    let inner = if vert == 3 { gap + light } else { -(gap + light) };
                    let stop = if left { cx - inner } else { cx + inner };
                    let stop = if vert == 0 && !any_double_v { if left { cx + light / 2. } else { cx - light / 2. } } else { stop };
                    let (x0, x1) = if left { (l, stop) } else { (stop, r) };
                    rect(mesh, Rect::from_min_max(pos2(x0, y - light / 2.), pos2(x1, y + light / 2.)), fg);
                }
            } else {
                let tw = thick(w);
                let reach = if any_double_v { gap + light * 1.5 } else { tw.max(thick(wu.max(wd))) / 2. };
                let (x0, x1) = if left { (l, cx + reach) } else { (cx - reach, r) };
                rect(mesh, Rect::from_min_max(pos2(x0, cy - tw / 2.), pos2(x1, cy + tw / 2.)), fg);
            }
        }
        // Vertical arms.
        for (w, up) in [(wu, true), (wd, false)] {
            if w == 0 {
                continue;
            }
            if w == 3 {
                for (x, side_left) in [(cx - gap - light / 2., true), (cx + gap + light / 2., false)] {
                    let horiz = if side_left { wl } else { wr };
                    let stop = if horiz == 3 {
                        if up { cy - gap - light } else { cy + gap + light }
                    } else if horiz == 0 && !any_double_h {
                        if up { cy + light / 2. } else { cy - light / 2. }
                    } else if up {
                        cy + gap + light
                    } else {
                        cy - gap - light
                    };
                    let (y0, y1) = if up { (t, stop) } else { (stop, b) };
                    rect(mesh, Rect::from_min_max(pos2(x - light / 2., y0), pos2(x + light / 2., y1)), fg);
                }
            } else {
                let tw = thick(w);
                let reach = if any_double_h { gap + light * 1.5 } else { tw.max(thick(wl.max(wr))) / 2. };
                let (y0, y1) = if up { (t, cy + reach) } else { (cy - reach, b) };
                rect(mesh, Rect::from_min_max(pos2(cx - tw / 2., y0), pos2(cx + tw / 2., y1)), fg);
            }
        }
        return true;
    }
    match c {
        // Rounded corners: a quarter circle joining two arms.
        '╭' | '╮' | '╯' | '╰' => {
            let rad = ((r - l).min(b - t) / 2.).max(1.);
            let (sx, sy) = match c {
                '╭' => (1., 1.),
                '╮' => (-1., 1.),
                '╯' => (-1., -1.),
                _ => (1., -1.),
            };
            let center = pos2(cx + sx * rad, cy + sy * rad);
            let steps = 8;
            let mut prev = pos2(cx, cy + sy * rad);
            // Straight part of the vertical arm to the cell edge.
            segment(mesh, pos2(cx, if sy > 0. { b } else { t }), prev, light, fg);
            for i in 1..=steps {
                let a = i as f32 / steps as f32 * std::f32::consts::FRAC_PI_2;
                let p = pos2(center.x - sx * rad * a.cos(), center.y - sy * rad * a.sin());
                segment(mesh, prev, p, light, fg);
                prev = p;
            }
            segment(mesh, prev, pos2(if sx > 0. { r } else { l }, cy), light, fg);
            true
        }
        '╱' => {
            segment(mesh, pos2(l, b), pos2(r, t), light, fg);
            true
        }
        '╲' => {
            segment(mesh, pos2(l, t), pos2(r, b), light, fg);
            true
        }
        '╳' => {
            segment(mesh, pos2(l, b), pos2(r, t), light, fg);
            segment(mesh, pos2(l, t), pos2(r, b), light, fg);
            true
        }
        // Block elements.
        '\u{2580}'..='\u{259F}' => {
            let h = b - t;
            let w = r - l;
            let y = |f: f32| m.snap(t + h * f);
            let x = |f: f32| m.snap(l + w * f);
            let mut fill = |x0: f32, y0: f32, x1: f32, y1: f32, col: Color32| rect(mesh, Rect::from_min_max(pos2(x0, y0), pos2(x1, y1)), col);
            match code {
                0x2580 => fill(l, t, r, y(0.5), fg),
                0x2581..=0x2588 => fill(l, y(1. - (code - 0x2580) as f32 / 8.), r, b, fg),
                0x2589..=0x258F => fill(l, t, x((0x2590 - code) as f32 / 8.), b, fg),
                0x2590 => fill(x(0.5), t, r, b, fg),
                0x2591..=0x2593 => {
                    let share = (code - 0x2590) as f32 / 4.;
                    let mixed = c32(yaac_term::mix(yaac_term::Rgb(bg.r(), bg.g(), bg.b()), yaac_term::Rgb(fg.r(), fg.g(), fg.b()), share));
                    fill(l, t, r, b, mixed);
                }
                0x2594 => fill(l, t, r, y(1. / 8.), fg),
                0x2595 => fill(x(7. / 8.), t, r, b, fg),
                _ => {
                    // Quadrants: bits UL, UR, LL, LR.
                    let q: u8 = match code {
                        0x2596 => 0b0010,
                        0x2597 => 0b0001,
                        0x2598 => 0b1000,
                        0x2599 => 0b1011,
                        0x259A => 0b1001,
                        0x259B => 0b1110,
                        0x259C => 0b1101,
                        0x259D => 0b0100,
                        0x259E => 0b0110,
                        _ => 0b0111,
                    };
                    let (mx, my) = (x(0.5), y(0.5));
                    if q & 0b1000 != 0 {
                        fill(l, t, mx, my, fg);
                    }
                    if q & 0b0100 != 0 {
                        fill(mx, t, r, my, fg);
                    }
                    if q & 0b0010 != 0 {
                        fill(l, my, mx, b, fg);
                    }
                    if q & 0b0001 != 0 {
                        fill(mx, my, r, b, fg);
                    }
                }
            }
            true
        }
        // Powerline arrows, so they meet the next cell's background exactly.
        '\u{E0B0}' | '\u{E0B2}' => {
            let i = mesh.vertices.len() as u32;
            let pts = if c == '\u{E0B0}' { [pos2(l, t), pos2(r, cell.center().y), pos2(l, b)] } else { [pos2(r, t), pos2(l, cell.center().y), pos2(r, b)] };
            for p in pts {
                mesh.vertices.push(Vertex { pos: p, uv: egui::epaint::WHITE_UV, color: fg });
            }
            mesh.indices.extend_from_slice(&[i, i + 1, i + 2]);
            true
        }
        _ => false,
    }
}

/// Underline, double underline, strikethrough under a run.
fn decorations(mesh: &mut egui::Mesh, flags: Flags, x0: f32, x1: f32, top: f32, color: Color32, m: &Metrics) {
    let w = m.line();
    let under = m.snap(top + m.base + w * 1.5);
    if flags.has(Flags::UNDERLINE) || flags.has(Flags::FANCY_UNDERLINE) {
        rect(mesh, Rect::from_min_max(pos2(x0, under), pos2(x1, under + w)), color);
    }
    if flags.has(Flags::DOUBLE_UNDERLINE) {
        rect(mesh, Rect::from_min_max(pos2(x0, under), pos2(x1, under + w)), color);
        rect(mesh, Rect::from_min_max(pos2(x0, under + 2. * w), pos2(x1, under + 3. * w)), color);
    }
    if flags.has(Flags::STRIKE) {
        let y = m.snap(top + m.base - m.rh * 0.28);
        rect(mesh, Rect::from_min_max(pos2(x0, y), pos2(x1, y + w)), color);
    }
}

/// One glyph (or shape) in a cell span.
#[allow(clippy::too_many_arguments)]
fn draw_char(mesh: &mut egui::Mesh, atlas: &mut Atlas, ui: &egui::Ui, ch: char, cell: Rect, fg: Color32, bg: Color32, flags: Flags, m: &Metrics) {
    if ch == ' ' {
        return;
    }
    if special(mesh, ch, cell, fg, bg, m) {
        return;
    }
    let Some(g) = atlas.glyph(ui, ch) else { return };
    let fit = cell.width();
    let shown = g.advance.min(fit);
    let pen = pos2(cell.left() + (fit - shown) / 2., cell.top() + m.base);
    let bold = flags.has(Flags::BOLD).then_some(m.px.max(0.6 / m.ppp.max(1.)));
    glyph_quads(mesh, &g, pen, fit, fg, flags.has(Flags::ITALIC), bold, m);
}

fn build_row(atlas: &mut Atlas, ui: &egui::Ui, row: &yaac_term::Row, r: usize, m: &Metrics, theme: &Theme) -> RowMesh {
    let mut out = RowMesh::default();
    let top = r as f32 * m.rh;
    let bottom = top + m.rh;
    for run in row.runs() {
        if run.bg != Color::Bg {
            rect(&mut out.bg, Rect::from_min_max(pos2(m.x(run.start), top), pos2(m.x(run.start + run.len), bottom)), c32(theme.rgb(run.bg)));
        }
        if run.flags.underlined() || run.flags.has(Flags::STRIKE) {
            let fg = c32(theme.fg(run.fg, run.bg, run.flags));
            decorations(&mut out.fg, run.flags, m.x(run.start), m.x(run.start + run.len), top, fg, m);
        }
    }
    for (col, cell) in row.cells.iter().enumerate() {
        if cell.flags.has(Flags::SPACER) || cell.ch == ' ' {
            continue;
        }
        let span = if cell.flags.has(Flags::WIDE) { 2 } else { 1 };
        let rect_ = Rect::from_min_max(pos2(m.x(col), top), pos2(m.x(col + span), bottom));
        let fg = c32(theme.fg(cell.fg, cell.bg, cell.flags));
        let bg = c32(theme.rgb(cell.bg));
        draw_char(&mut out.fg, atlas, ui, cell.ch, rect_, fg, bg, cell.flags, m);
    }
    for (col, extra) in &row.combining {
        let Some(cell) = row.cells.get(*col) else { continue };
        let rect_ = Rect::from_min_max(pos2(m.x(*col), top), pos2(m.x(col + 1), bottom));
        let fg = c32(theme.fg(cell.fg, cell.bg, cell.flags));
        for ch in extra.chars() {
            draw_char(&mut out.fg, atlas, ui, ch, rect_, fg, fg, cell.flags, m);
        }
    }
    out
}

fn append(dst: &mut egui::Mesh, src: &egui::Mesh, offset: Vec2) {
    let base = dst.vertices.len() as u32;
    dst.indices.extend(src.indices.iter().map(|i| i + base));
    dst.vertices.extend(src.vertices.iter().map(|v| Vertex { pos: v.pos + offset, ..*v }));
}

/// What the grid needs from the tab beyond the frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Overlay {
    focused: bool,
    cursor_on: bool,
    link: Option<(usize, usize, usize)>,
    hide_cursor: bool,
}

impl Overlay {
    fn key(&self) -> u64 {
        let l = self.link.map_or(0, |(r, a, b)| 1 + r as u64 * 1_000_003 + a as u64 * 1009 + b as u64);
        u64::from(self.focused) | u64::from(self.cursor_on) << 1 | u64::from(self.hide_cursor) << 2 | l << 3
    }
}

/// Assemble (or reuse) the frame's one mesh.
fn grid_mesh(ui: &egui::Ui, r: &mut Renderer, session: u64, frame: &Frame, origin: Pos2, theme: &Theme, ov: &Overlay) -> Arc<egui::Mesh> {
    let m = r.atlas.refresh(ui);
    let atlas_key = r.atlas.key.expect("refreshed");
    let grid = r.grids.entry(session).or_default();
    let key = (atlas_key, frame.cols, frame.rows, *theme);
    let full = grid.key != Some(key) || frame.full || grid.rows.len() != frame.rows;
    let mut built = 0;
    if full {
        grid.key = Some(key);
        grid.rows = vec![RowMesh::default(); frame.rows];
        for (i, row) in frame.lines.iter().enumerate() {
            grid.rows[i] = build_row(&mut r.atlas, ui, row, i, &m, theme);
            built += 1;
        }
        grid.assembled = None;
    } else {
        for &i in &frame.damaged {
            if let Some(row) = frame.lines.get(i) {
                grid.rows[i] = build_row(&mut r.atlas, ui, row, i, &m, theme);
                built += 1;
            }
        }
    }
    r.rows_built += built;
    let akey = (frame.generation, origin, ov.key());
    if built == 0
        && let Some((g, o, k, mesh)) = &grid.assembled
        && (*g, *o, *k) == akey
    {
        return mesh.clone();
    }
    r.assembled += 1;
    let mut mesh = egui::Mesh::default();
    let off = origin.to_vec2();
    let size = vec2(m.x(frame.cols), frame.rows as f32 * m.rh);
    for row in &grid.rows {
        append(&mut mesh, &row.bg, off);
    }
    let cell_rect = |row: usize, a: usize, b: usize| Rect::from_min_max(pos2(m.x(a), row as f32 * m.rh) + off, pos2(m.x(b + 1), (row + 1) as f32 * m.rh) + off);
    if let Some(sel) = &frame.selection {
        for row in sel.start.0..=sel.end.0.min(frame.rows.saturating_sub(1)) {
            if let Some((a, b)) = sel.columns(row, frame.cols) {
                rect(&mut mesh, cell_rect(row, a, b), c32(theme.selection));
            }
        }
    }
    let mut recolor: Vec<(usize, usize, usize, Color32)> = Vec::new();
    if let Some(search) = &frame.search {
        for (spans, fill) in [(&search.matches, theme.search), (&search.focused.iter().copied().collect::<Vec<_>>(), theme.search_focused)] {
            for s in spans {
                for row in s.start.0..=s.end.0 {
                    if let Some((a, b)) = s.columns(row, frame.cols) {
                        rect(&mut mesh, cell_rect(row, a, b), c32(fill));
                        if fill == theme.search_focused {
                            recolor.push((row, a, b, c32(theme.search_text)));
                        }
                    }
                }
            }
        }
    }
    for row in &grid.rows {
        append(&mut mesh, &row.fg, off);
    }
    // Glyphs over the current match, in its text colour.
    for (row, a, b, color) in recolor {
        let Some(line) = frame.lines.get(row) else { continue };
        for col in a..=b.min(line.cells.len().saturating_sub(1)) {
            let cell = &line.cells[col];
            if cell.flags.has(Flags::SPACER) {
                continue;
            }
            let span = if cell.flags.has(Flags::WIDE) { 2 } else { 1 };
            draw_char(&mut mesh, &mut r.atlas, ui, cell.ch, cell_rect(row, col, col + span - 1), color, color, cell.flags, &m);
        }
    }
    if let Some((row, a, b)) = ov.link {
        let y = m.snap(row as f32 * m.rh + m.base + m.line() * 1.5) + off.y;
        let color = frame.lines.get(row).and_then(|l| l.cells.get(a)).map_or(c32(theme.fg), |c| c32(theme.fg(c.fg, c.bg, c.flags)));
        rect(&mut mesh, Rect::from_min_max(pos2(m.x(a) + off.x, y), pos2(m.x(b + 1) + off.x, y + m.line())), color);
    }
    // The cursor.
    let cur = frame.cursor;
    if cur.visible && cur.row < frame.rows && !ov.hide_cursor {
        let span = if cur.wide { 2 } else { 1 };
        let cell = cell_rect(cur.row, cur.col, cur.col + span - 1);
        let color = c32(theme.cursor);
        let w = m.snap(m.line() * 2.).max(m.px * 2.);
        let shape = if ov.focused { cur.shape } else { CursorShape::HollowBlock };
        match shape {
            CursorShape::Block if ov.cursor_on => {
                rect(&mut mesh, cell, color);
                if let Some(c) = frame.lines.get(cur.row).and_then(|l| l.cells.get(cur.col)) {
                    draw_char(&mut mesh, &mut r.atlas, ui, c.ch, cell, c32(theme.cursor_text), color, c.flags, &m);
                }
            }
            CursorShape::Bar if ov.cursor_on => rect(&mut mesh, Rect::from_min_max(cell.min, pos2(cell.left() + w, cell.bottom())), color),
            CursorShape::Underline if ov.cursor_on => rect(&mut mesh, Rect::from_min_max(pos2(cell.left(), cell.bottom() - w), cell.max), color),
            CursorShape::HollowBlock => {
                let t = m.line();
                rect(&mut mesh, Rect::from_min_max(cell.min, pos2(cell.right(), cell.top() + t)), color);
                rect(&mut mesh, Rect::from_min_max(pos2(cell.left(), cell.bottom() - t), cell.max), color);
                rect(&mut mesh, Rect::from_min_max(cell.min, pos2(cell.left() + t, cell.bottom())), color);
                rect(&mut mesh, Rect::from_min_max(pos2(cell.right() - t, cell.top()), cell.max), color);
            }
            _ => {}
        }
    }
    let _ = size;
    let mesh = Arc::new(mesh);
    grid.assembled = Some((akey.0, akey.1, akey.2, mesh.clone()));
    mesh
}

/// Forget a closed session's caches.
pub(crate) fn forget(ctx: &egui::Context, session: u64) {
    if let Ok(mut r) = renderer(ctx).lock() {
        r.grids.remove(&session);
    }
}

// ---- the grid widget ------------------------------------------------------------------------

/// Clipboard text read off the UI thread, for a session.
type Clip = Arc<Mutex<Option<(u64, String)>>>;

fn clip_slot(ctx: &egui::Context) -> Clip {
    ctx.data_mut(|d| d.get_temp_mut_or_default::<Clip>(egui::Id::unique("yaac-term-clip")).clone())
}

/// Read the clipboard on a worker thread (a clipboard read can block), then paste.
fn request_paste(ctx: &egui::Context, session: u64) {
    let slot = clip_slot(ctx);
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let text = arboard::Clipboard::new().and_then(|mut c| c.get_text()).unwrap_or_default();
        if !text.is_empty()
            && let Ok(mut g) = slot.lock()
        {
            *g = Some((session, text));
        }
        ctx.request_repaint();
    });
}

fn map_key(key: egui::Key, shift: bool) -> Option<Key> {
    use egui::Key as E;
    Some(match key {
        E::Enter => Key::Enter,
        E::Tab => Key::Tab,
        E::Backspace => Key::Backspace,
        E::Escape => Key::Escape,
        E::ArrowUp => Key::Up,
        E::ArrowDown => Key::Down,
        E::ArrowLeft => Key::Left,
        E::ArrowRight => Key::Right,
        E::Home => Key::Home,
        E::End => Key::End,
        E::PageUp => Key::PageUp,
        E::PageDown => Key::PageDown,
        E::Insert => Key::Insert,
        E::Delete => Key::Delete,
        E::Space => Key::Char(' '),
        E::F1 => Key::F(1),
        E::F2 => Key::F(2),
        E::F3 => Key::F(3),
        E::F4 => Key::F(4),
        E::F5 => Key::F(5),
        E::F6 => Key::F(6),
        E::F7 => Key::F(7),
        E::F8 => Key::F(8),
        E::F9 => Key::F(9),
        E::F10 => Key::F(10),
        E::F11 => Key::F(11),
        E::F12 => Key::F(12),
        E::F13 => Key::F(13),
        E::F14 => Key::F(14),
        E::F15 => Key::F(15),
        E::F16 => Key::F(16),
        E::F17 => Key::F(17),
        E::F18 => Key::F(18),
        E::F19 => Key::F(19),
        E::F20 => Key::F(20),
        other => {
            let s = other.symbol_or_name();
            let mut it = s.chars();
            let (Some(c), None) = (it.next(), it.next()) else { return None };
            Key::Char(if c.is_ascii_alphabetic() && !shift { c.to_ascii_lowercase() } else { c })
        }
    })
}

/// Character keys arrive as text unless Ctrl or Alt is held.
fn is_text_key(key: Key, mods: Mods) -> bool {
    matches!(key, Key::Char(_)) && !mods.ctrl && !mods.alt
}

/// Where a pointer position falls on the grid: row, column, which half.
fn cell_at(pos: Pos2, origin: Pos2, m: &Metrics, cols: usize, rows: usize) -> (usize, usize, CellSide) {
    let x = (pos.x - origin.x) / m.cw;
    let y = (pos.y - origin.y) / m.rh;
    let col = (x.max(0.) as usize).min(cols.saturating_sub(1));
    let row = (y.max(0.) as usize).min(rows.saturating_sub(1));
    let side = if x - x.floor() > 0.5 { CellSide::Right } else { CellSide::Left };
    (row, col, side)
}

/// Per-widget input state kept between frames.
#[derive(Clone, Copy, Default)]
struct Input {
    had_focus: bool,
    wheel: f32,
    /// The last mouse report's cell and buttons, so motion is reported once per cell.
    reported: Option<(usize, usize)>,
    selecting: bool,
}

/// Which menu actions the right-click menu chose.
enum MenuPick {
    Copy,
    Paste,
    AddToComposer,
    SearchFor,
    SelectAll,
    Clear,
    Search,
    Restart,
}

/// The grid of one session in `area`: draws it, sizes the shell to it, and takes its input.
/// `panel`: the thread whose panel shows it (Ctrl+PageUp/Down switch its tabs).
#[allow(clippy::too_many_lines)]
pub(crate) fn grid(ui: &mut egui::Ui, app: &mut YaacApp, session: u64, area: Rect, panel: Option<&ThreadKey>) {
    let ctx = ui.ctx().clone();
    let p = app.p;
    let theme = theme_of(&p);
    let wid = widget_id(session);
    let resp = ui.interact(area, wid, egui::Sense::click_and_drag());
    let shared = renderer(&ctx);
    let m = match shared.lock() {
        Ok(mut r) => r.atlas.refresh(ui),
        Err(_) => return,
    };
    let inner = area.shrink2(PAD);
    let origin = pos2(m.snap(inner.left()), m.snap(inner.top()));
    let fit = (((inner.width() / m.cw).floor() as u16).max(2), ((inner.height() / m.rh).floor() as u16).max(1));
    let size = app.browsers.terms.get(session).and_then(|s| s.pinned).unwrap_or(fit);
    app.browsers.terms.resize(&mut app.client, session, size.0, size.1);
    let Some(s) = app.browsers.terms.get_mut(session) else { return };
    s.term.set_theme(theme, ((m.cw * m.ppp).round() as u16, (m.rh * m.ppp).round() as u16));
    s.term.snapshot(&mut s.frame);
    let (cols, rows) = (s.frame.cols, s.frame.rows);

    // Focus.
    if app.browsers.terms.focus == Some(session) {
        app.browsers.terms.focus = None;
        resp.request_focus();
    }
    if resp.clicked() || resp.drag_started() || resp.secondary_clicked() {
        resp.request_focus();
    }
    let focused = resp.has_focus();
    let shown_focused = focused || app.browsers.terms.get(session).is_some_and(|s| s.shown_focused);
    let mut st: Input = ctx.data(|d| d.get_temp(wid)).unwrap_or_default();
    let mut out: Vec<u8> = Vec::new();
    let mut typed = false;
    if focused {
        ctx.memory_mut(|mem| mem.set_focus_lock_filter(wid, egui::EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true }));
        ctx.data_mut(|d| d.insert_temp(focus_slot(), wid));
    }
    let s = app.browsers.terms.get_mut(session).expect("present above");
    if focused != st.had_focus {
        if let Some(b) = s.term.focus(focused) {
            out.extend(b);
        }
        st.had_focus = focused;
    }
    let modes = s.frame.modes;
    let prefs = app.ui.terminal;
    let mut picked: Option<MenuPick> = None;
    let mut cycle: Option<bool> = None;
    let mut paste_now = false;

    // Keys.
    if focused {
        let events = ui.input(|i| i.events.clone());
        let mut skip_text = false;
        for e in &events {
            match e {
                egui::Event::Key { key, pressed: true, modifiers, .. } => {
                    let mods = Mods { shift: modifiers.shift, ctrl: modifiers.ctrl || (modifiers.command && !modifiers.mac_cmd), alt: modifiers.alt, meta: modifiers.mac_cmd };
                    let Some(k) = map_key(*key, mods.shift) else { continue };
                    if mods.ctrl && mods.shift && k == Key::Char('F') {
                        picked = Some(MenuPick::Search);
                        continue;
                    }
                    if mods.ctrl && !mods.shift && !mods.alt && matches!(k, Key::PageDown | Key::PageUp) && panel.is_some() {
                        cycle = Some(k == Key::PageDown);
                        continue;
                    }
                    if mods.shift && !mods.ctrl && !mods.alt && !modes.alt_screen && matches!(k, Key::PageUp | Key::PageDown) {
                        s.term.scroll_page(k == Key::PageUp);
                        continue;
                    }
                    match yaac_term::shortcut(k, mods, s.frame.selection.is_some(), prefs.ctrl_v_to_shell) {
                        Shortcut::Copy => {
                            if let Some(text) = s.term.selection_text() {
                                ctx.copy_text(text);
                            }
                            if !mods.shift {
                                s.term.clear_selection();
                            }
                        }
                        Shortcut::Paste => paste_now = true,
                        Shortcut::Send => {
                            if is_text_key(k, mods) || (mods.ctrl && mods.alt && matches!(k, Key::Char(_))) {
                                continue;
                            }
                            if let Some(bytes) = s.term.key(k, mods) {
                                out.extend(bytes);
                                typed = true;
                                skip_text = matches!(k, Key::Char(_));
                            }
                        }
                    }
                }
                egui::Event::Text(t) | egui::Event::Ime(egui::ImeEvent::Commit(t)) => {
                    if std::mem::take(&mut skip_text) {
                        continue;
                    }
                    out.extend(s.term.text(t));
                    typed = true;
                }
                egui::Event::Paste(t) => {
                    app_paste(app, session, t);
                    return;
                }
                _ => {}
            }
        }
        ui.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Key { .. } | egui::Event::Text(_) | egui::Event::Ime(_) | egui::Event::Paste(_))));
    }
    let s = app.browsers.terms.get_mut(session).expect("present above");

    // Mouse.
    let mods_now = ui.input(|i| i.modifiers);
    let mods = Mods { shift: mods_now.shift, ctrl: mods_now.ctrl || mods_now.command, alt: mods_now.alt, meta: false };
    let hover = resp.hover_pos();
    let reporting = modes.mouse() && !mods.shift;
    let mut link = None;
    if let Some(pos) = hover {
        let (row, col, _) = cell_at(pos, origin, &m, cols, rows);
        if let Some(l) = s.frame.link_at(row, col) {
            if mods.ctrl {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            link = l.cells.iter().find(|c| c.0 == row).copied().map(|c| (c.0, c.1, c.2));
            if resp.clicked() && mods.ctrl && !reporting {
                crate::browser::open(&l.uri);
            }
        } else {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
        }
        // The wheel.
        let dy: f32 = ui.input(|i| {
            i.events
                .iter()
                .map(|e| match e {
                    egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Line, delta, .. } => delta.y * m.rh,
                    egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Page, delta, .. } => delta.y * area.height(),
                    egui::Event::MouseWheel { delta, .. } => delta.y,
                    _ => 0.,
                })
                .sum()
        });
        if dy != 0. {
            st.wheel += dy / m.rh;
            let lines = st.wheel.trunc() as i32;
            st.wheel -= lines as f32;
            if lines != 0
                && let Some(b) = s.term.wheel(lines, row, col, mods)
            {
                out.extend(b);
            }
            ui.input_mut(|i| i.smooth_scroll_delta = Vec2::ZERO);
        }
    }
    if reporting {
        let events = ui.input(|i| i.events.clone());
        for e in &events {
            let (pos, button, action) = match e {
                egui::Event::PointerButton { pos, button, pressed, .. } if area.contains(*pos) || st.reported.is_some() => {
                    let b = match button {
                        egui::PointerButton::Primary => MouseButton::Left,
                        egui::PointerButton::Middle => MouseButton::Middle,
                        egui::PointerButton::Secondary => MouseButton::Right,
                        _ => continue,
                    };
                    (*pos, b, if *pressed { MouseAction::Press } else { MouseAction::Release })
                }
                egui::Event::PointerMoved(pos) if area.contains(*pos) => {
                    let down = ui.input(|i| i.pointer.primary_down());
                    (*pos, if down { MouseButton::Left } else { MouseButton::None }, MouseAction::Motion)
                }
                _ => continue,
            };
            let (row, col, _) = cell_at(pos, origin, &m, cols, rows);
            if action == MouseAction::Motion && st.reported == Some((row, col)) {
                continue;
            }
            if let Some(b) = s.term.mouse(button, action, row, col, mods) {
                out.extend(b);
                st.reported = Some((row, col));
            }
            if action == MouseAction::Release {
                st.reported = None;
            }
        }
    } else if let Some(pos) = resp.interact_pointer_pos() {
        let (row, col, side) = cell_at(pos, origin, &m, cols, rows);
        if resp.triple_clicked() {
            s.term.select_start(row, col, side, SelectKind::Line);
        } else if resp.double_clicked() {
            s.term.select_start(row, col, side, SelectKind::Word);
        } else if resp.drag_started_by(egui::PointerButton::Primary) {
            if mods.shift && s.frame.selection.is_some() {
                s.term.select_to(row, col, side);
            } else {
                s.term.select_start(row, col, side, if mods.alt { SelectKind::Block } else { SelectKind::Simple });
            }
            st.selecting = true;
        } else if resp.dragged_by(egui::PointerButton::Primary) && st.selecting {
            // Past the top or bottom: scroll the scrollback while selecting.
            if pos.y < area.top() {
                s.term.scroll(1);
            } else if pos.y > area.bottom() {
                s.term.scroll(-1);
            }
            s.term.select_to(row, col, side);
        } else if resp.clicked_by(egui::PointerButton::Primary) && !mods.ctrl {
            if mods.shift && s.frame.selection.is_some() {
                s.term.select_to(row, col, side);
            } else {
                s.term.clear_selection();
            }
        }
        if resp.drag_stopped() && st.selecting {
            st.selecting = false;
            if prefs.copy_on_select
                && let Some(text) = s.term.selection_text()
            {
                ctx.copy_text(text);
            }
        }
    }

    // Right-click menu.
    let has_sel = s.frame.selection.is_some();
    let exited = !s.live();
    if !reporting {
        widgets::menu_popup(egui::Popup::context_menu(&resp), &p).show(|ui| {
            ui.set_min_width(200.);
            let mut row = |ui: &mut egui::Ui, label: &str, key: &str, pick: MenuPick| {
                let (r, rect) = widgets::menu_row(ui, &p, false, true);
                let g = ui.fonts_mut(|f| f.layout_no_wrap(label.to_owned(), theme::ui(size::UI), p.text));
                ui.painter().galley(pos2(rect.left() + 12., rect.center().y - g.size().y / 2.), g, p.text);
                if !key.is_empty() {
                    let g = ui.fonts_mut(|f| f.layout_no_wrap(key.to_owned(), theme::ui(size::SMALL), p.text2));
                    ui.painter().galley(pos2(rect.right() - 10. - g.size().x, rect.center().y - g.size().y / 2.), g, p.text2);
                }
                if r.clicked() {
                    picked = Some(pick);
                    ui.close();
                }
            };
            if has_sel {
                row(ui, "Copy", "Ctrl+Shift+C", MenuPick::Copy);
                row(ui, "Add to composer", "", MenuPick::AddToComposer);
                row(ui, "Search for this", "", MenuPick::SearchFor);
            } else {
                row(ui, "Paste", "Ctrl+Shift+V", MenuPick::Paste);
            }
            ui.separator();
            row(ui, "Select all", "", MenuPick::SelectAll);
            row(ui, "Search", "Ctrl+Shift+F", MenuPick::Search);
            row(ui, "Clear", "", MenuPick::Clear);
            if exited {
                row(ui, "Restart", "", MenuPick::Restart);
            }
        });
    }

    // Draw.
    app.browsers.terms.mark_shown(session);
    let now = Instant::now();
    let s = app.browsers.terms.get_mut(session).expect("present above");
    let t = ctx.input(|i| i.time);
    let live = s.live();
    let blinking = focused && live && s.may_blink(now) && s.frame.cursor.visible;
    let cursor_on = !blinking || ((t / BLINK) as i64) % 2 == 0;
    if blinking {
        let next = BLINK - t % BLINK;
        ctx.request_repaint_after(Duration::from_secs_f64(next.max(0.01)));
    }
    // A shell that ended has no cursor.
    let ov = Overlay { focused: shown_focused, cursor_on, link, hide_cursor: !live };
    ui.painter().rect_filled(area, 0., c32(theme.bg));
    if let Ok(mut r) = shared.lock() {
        let mesh = grid_mesh(ui, &mut r, session, &s.frame, origin, &theme, &ov);
        ui.painter().with_clip_rect(area).add(egui::Shape::Mesh(mesh));
    }
    // A scroll position marker while scrolled back.
    if s.frame.display_offset > 0 && s.frame.history > 0 {
        let total = (s.frame.history + rows) as f32;
        let h = (area.height() * rows as f32 / total).max(16.);
        let top = area.top() + (area.height() - h) * (1. - s.frame.display_offset as f32 / s.frame.history as f32);
        ui.painter().rect_filled(Rect::from_min_size(pos2(area.right() - 5., top), vec2(3., h)), 2., widgets::blend(p.bg, p.text2, 0.6));
    }
    ctx.data_mut(|d| d.insert_temp(wid, st));

    // Act.
    if let Some(text) = app.browsers.terms.clipboard.take() {
        ctx.copy_text(text);
        app.browsers.notice = Some("Copied by the terminal".into());
    }
    if !out.is_empty() {
        if typed {
            app.browsers.terms.typed(&mut app.client, session, &out);
        } else {
            app.browsers.terms.input(&mut app.client, session, &out);
        }
    }
    if paste_now {
        request_paste(&ctx, session);
    }
    let pending = clip_slot(&ctx).lock().ok().and_then(|mut g| g.take_if(|(id, _)| *id == session));
    if let Some((_, text)) = pending {
        app_paste(app, session, &text);
    }
    if let (Some(fwd), Some(key)) = (cycle, panel) {
        app.browsers.terms.panel_cycle(key, fwd);
    }
    if let Some(pick) = picked {
        menu_action(app, &ctx, session, pick);
    }
}

fn app_paste(app: &mut YaacApp, session: u64, text: &str) {
    let confirmed = !app.ui.terminal.confirm_multiline_paste;
    app.browsers.terms.paste(&mut app.client, session, text, confirmed);
}

#[allow(clippy::needless_pass_by_value, reason = "a menu choice is used once")]
fn menu_action(app: &mut YaacApp, ctx: &egui::Context, session: u64, pick: MenuPick) {
    let Some(s) = app.browsers.terms.get_mut(session) else { return };
    match pick {
        MenuPick::Copy => {
            if let Some(t) = s.term.selection_text() {
                ctx.copy_text(t);
            }
        }
        MenuPick::Paste => request_paste(ctx, session),
        MenuPick::AddToComposer => {
            let key = s.key.clone();
            if let Some(ex) = s.excerpt() {
                add_to_composer(ctx, app, &key, ex);
            }
        }
        MenuPick::SearchFor => {
            let q = s.term.selection_text().unwrap_or_default().lines().next().unwrap_or_default().to_owned();
            s.term.search(&q);
            s.search = Some(q);
            ctx.memory_mut(|m| m.request_focus(search_id(session)));
        }
        MenuPick::SelectAll => s.term.select_all(),
        MenuPick::Clear => s.term.clear(),
        MenuPick::Search => {
            if s.search.is_none() {
                s.search = Some(String::new());
            }
            ctx.memory_mut(|m| m.request_focus(search_id(session)));
        }
        MenuPick::Restart => app.browsers.terms.restart(&mut app.client, session),
    }
}

/// Put an excerpt at the composer's caret (as File tabs do).
fn add_to_composer(ctx: &egui::Context, app: &mut YaacApp, key: &ThreadKey, ex: yaac_code::Excerpt) {
    let draft = yaac_core::attachments::DraftRef::Thread(key.clone());
    let edit = crate::composer::composer_id(&draft);
    let mut cursor = egui::TextEdit::load_state(ctx, edit).unwrap_or_default();
    let at = cursor
        .cursor
        .char_range()
        .map(|r| usize::from(r.primary.index))
        .unwrap_or_else(|| app.client.draft(&draft).map_or(0, |d| d.text.chars().count()));
    if crate::file_pane::insert_excerpt(&mut app.client, key, at, ex.label, ex.text) {
        let len = app.client.draft(&draft).and_then(|d| d.excerpts.last()).map_or(0, |e| e.token.chars().count());
        cursor.cursor.set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(at + len))));
        cursor.store(ctx, edit);
        app.browsers.set_full_screen(key, false);
        // A maximised panel hides the composer: give the column back.
        if let Some(panel) = app.browsers.terms.panels.get_mut(key) {
            panel.maximised = false;
        }
        app.ui.focus_composer = true;
    }
}

fn search_id(session: u64) -> egui::Id {
    egui::Id::unique(("yaac-terminal-search", session))
}

/// The search field (toolbar or floating): typing searches, Enter finds older, Shift+Enter
/// newer, Esc closes.
fn search_field(ui: &mut egui::Ui, app: &mut YaacApp, session: u64, width: f32) {
    let p = app.p;
    let Some(s) = app.browsers.terms.get_mut(session) else { return };
    let Some(query) = s.search.as_mut() else { return };
    let id = search_id(session);
    let before = query.clone();
    let (enter, shift_enter, esc) = if ui.memory(|m| m.has_focus(id)) {
        ui.input_mut(|i| (i.consume_key(egui::Modifiers::NONE, egui::Key::Enter), i.consume_key(egui::Modifiers::SHIFT, egui::Key::Enter), i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)))
    } else {
        (false, false, false)
    };
    let edit = egui::TextEdit::singleline(query).id(id).hint_text("Search").desired_width(width).font(theme::ui(size::UI)).margin(egui::Margin::symmetric(8, 4));
    let r = ui.add(edit);
    let changed = *query != before;
    let found = s.frame.search.as_ref();
    let label = match found {
        Some(v) if v.query.is_empty() => String::new(),
        Some(v) if !v.found => "No results".to_owned(),
        Some(v) if !v.matches.is_empty() => format!("{} here", v.matches.len()),
        _ => String::new(),
    };
    if !label.is_empty() {
        ui.label(egui::RichText::new(label).font(theme::ui(size::SMALL)).color(p.text2));
    }
    let up = widgets::tool_button(ui, &p, p.bg, "Older match (Enter)", true, false, |pa, c, d, col| icons::arrow_vertical(pa, c, d, col, true)).clicked();
    let down = widgets::tool_button(ui, &p, p.bg, "Newer match (Shift+Enter)", true, false, |pa, c, d, col| icons::arrow_vertical(pa, c, d, col, false)).clicked();
    let close = widgets::tool_button(ui, &p, p.bg, "Close search (Esc)", true, false, icons::close).clicked();
    let q = query.clone();
    if changed {
        s.term.search(&q);
    }
    if enter || up {
        s.term.search_next(true);
        r.request_focus();
    }
    if shift_enter || down {
        s.term.search_next(false);
        r.request_focus();
    }
    if esc || close {
        s.search = None;
        s.term.search_close();
        app.browsers.terms.focus = Some(session);
    }
}

/// The exited banner over the grid's bottom, with **Restart**.
fn exited_banner(ui: &mut egui::Ui, app: &mut YaacApp, session: u64, area: Rect) {
    let p = app.p;
    let Some(s) = app.browsers.terms.get(session) else { return };
    let text = match &s.state {
        SessionState::Exited(code) => format!("Process exited with code {code}"),
        SessionState::Failed(e) => format!("The terminal could not start: {e}"),
        SessionState::Reconnecting => "Reconnecting… the shell keeps running on the machine".to_owned(),
        _ => return,
    };
    let reconnecting = s.state == SessionState::Reconnecting;
    let bar = Rect::from_min_max(pos2(area.left(), area.bottom() - 40.), area.max);
    ui.painter().rect_filled(bar, 0., p.panel);
    ui.painter().hline(bar.x_range(), ui.painter().round_to_pixel_center(bar.top() + 0.5), egui::Stroke::new(1., p.border));
    let mut restart = false;
    ui.scope_builder(egui::UiBuilder::new().max_rect(bar.shrink2(vec2(12., 6.))).layout(egui::Layout::left_to_right(egui::Align::Center)), |ui| {
        ui.label(egui::RichText::new(text).font(theme::ui(size::UI)).color(p.text));
        if !reconnecting {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                restart = widgets::primary(ui, &p, "Restart", None).clicked();
            });
        }
    });
    if restart {
        app.browsers.terms.restart(&mut app.client, session);
    }
}

/// The multi-line paste question.
fn paste_question(ui: &mut egui::Ui, app: &mut YaacApp, session: u64, area: Rect) {
    let p = app.p;
    let Some(text) = app.browsers.terms.get(session).and_then(|s| s.paste_confirm.clone()) else { return };
    let lines = text.trim_end_matches(['\r', '\n']).lines().count();
    let card = Rect::from_center_size(area.center(), vec2(340f32.min(area.width() - 16.), 104.));
    ui.painter().rect_filled(card, 8., p.raised);
    ui.painter().rect_stroke(card, 8., egui::Stroke::new(1., p.border), egui::StrokeKind::Inside);
    let (mut go, mut never, mut cancel) = (false, false, false);
    ui.scope_builder(egui::UiBuilder::new().max_rect(card.shrink(14.)).layout(egui::Layout::top_down(egui::Align::Min)), |ui| {
        ui.label(egui::RichText::new(format!("Paste {lines} lines? They will run.")).font(theme::bold(size::UI)).color(p.text));
        ui.add_space(14.);
        ui.horizontal(|ui| {
            go = widgets::primary(ui, &p, "Paste", Some("Enter")).clicked();
            never = ui.button("Don't ask again").clicked();
            cancel = ui.button("Cancel").clicked();
        });
    });
    if go || never {
        if never {
            app.ui.terminal.confirm_multiline_paste = false;
        }
        app.browsers.terms.paste(&mut app.client, session, &text, true);
        app.browsers.terms.focus = Some(session);
    } else if cancel || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        if let Some(s) = app.browsers.terms.get_mut(session) {
            s.paste_confirm = None;
        }
        app.browsers.terms.focus = Some(session);
    }
}

/// The grid plus its overlays (exited banner, paste question, floating search).
pub(crate) fn session_view(ui: &mut egui::Ui, app: &mut YaacApp, session: u64, area: Rect, panel: Option<&ThreadKey>, floating_search: bool) {
    grid(ui, app, session, area, panel);
    if floating_search && app.browsers.terms.get(session).is_some_and(|s| s.search.is_some()) {
        let p = app.p;
        let r = Rect::from_min_size(pos2(area.right() - 380., area.top() + 6.), vec2(370., 34.));
        ui.painter().rect_filled(r, 6., p.raised);
        ui.painter().rect_stroke(r, 6., egui::Stroke::new(1., p.border), egui::StrokeKind::Inside);
        ui.scope_builder(egui::UiBuilder::new().max_rect(r.shrink2(vec2(6., 3.))).layout(egui::Layout::left_to_right(egui::Align::Center)), |ui| {
            ui.spacing_mut().item_spacing.x = 2.;
            search_field(ui, app, session, 180.);
        });
    }
    exited_banner(ui, app, session, area);
    paste_question(ui, app, session, area);
}

/// A side-pane Terminal tab: the toolbar (shell and folder; Search, Clear, Restart) and the
/// grid (docs/13-terminal.md#the-tab).
pub(crate) fn tab(ui: &mut egui::Ui, app: &mut YaacApp, session: u64) {
    let p = app.p;
    let Some(s) = app.browsers.terms.get(session) else {
        ui.label("This terminal has ended.");
        return;
    };
    let label = format!("{} · {}", s.shell_name(), s.folder());
    let exited = !s.live();
    let searching = s.search.is_some();
    let mut action = None;
    let (_, rest) = crate::preview_ui::toolbar(ui, &p, |ui| {
        ui.add_space(6.);
        if searching {
            search_field(ui, app, session, (ui.available_width() - 200.).clamp(120., 320.));
        } else {
            let w = (ui.available_width() - 3. * 30. - 8.).max(0.);
            let g = widgets::elided(ui, &label, &theme::ui(size::UI), p.text2, w);
            let (r, _) = ui.allocate_exact_size(vec2(w, 28.), egui::Sense::hover());
            ui.painter().galley(pos2(r.left(), r.center().y - g.size().y / 2.), g, p.text2);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if exited && widgets::tool_button(ui, &p, p.bg, "Restart", true, false, icons::reload).clicked() {
                action = Some(MenuPick::Restart);
            }
            if widgets::tool_button(ui, &p, p.bg, "Clear", true, false, icons::clear).clicked() {
                action = Some(MenuPick::Clear);
            }
            if !searching && widgets::tool_button(ui, &p, p.bg, "Search (Ctrl+Shift+F)", true, false, icons::search).clicked() {
                action = Some(MenuPick::Search);
            }
        });
    });
    session_view(ui, app, session, rest, None, false);
    if let Some(a) = action {
        let ctx = ui.ctx().clone();
        menu_action(app, &ctx, session, a);
    }
}

// ---- the panel under the composer (P338) ----------------------------------------------------

/// The panel's minimum height, and the transcript's above the composer.
pub(crate) const PANEL_MIN: f32 = 120.;
pub(crate) const TRANSCRIPT_MIN: f32 = 160.;

/// The terminal panel: the side pane's tab strip (every tab a terminal; + adds one with the
/// default shell; Maximise / Restore and Close panel at the right end), then the active
/// terminal.
pub(crate) fn panel(ui: &mut egui::Ui, app: &mut YaacApp, key: &ThreadKey, rect: Rect) {
    let p = app.p;
    let Some(panel) = app.browsers.terms.panel(key).cloned() else { return };
    let strip = Rect::from_min_size(rect.min, vec2(rect.width(), crate::preview_ui::STRIP_H));
    let infos: Vec<_> = panel.tabs.iter().map(|id| crate::preview_ui::terminal_info(app, crate::preview_ui::Entry::Tab(*id), *id)).collect();
    let max = panel.maximised;
    let close_icon = |pa: &egui::Painter, c: Pos2, d: f32, col: Color32| icons::close(pa, c, d, col);
    let max_icon = move |pa: &egui::Painter, c: Pos2, d: f32, col: Color32| icons::maximize(pa, c, d, col, max);
    let close_hover = {
        let keys = app.ui.bindings.label(yaac_core::commands::Cmd::ToggleTerminalPanel);
        if keys.is_empty() { "Close panel".to_owned() } else { format!("Close panel ({keys})") }
    };
    let right: [crate::preview_ui::StripButton<'_>; 2] = [(&close_hover, &close_icon, false), (if max { "Restore panel" } else { "Maximise panel" }, &max_icon, false)];
    let mut renaming = app.browsers.terms.renaming.take();
    let rename_entry = renaming.as_ref().filter(|r| panel.tabs.contains(&r.session)).map(|r| crate::preview_ui::Entry::Tab(r.session));
    let out = ui
        .scope_builder(egui::UiBuilder::new().id_salt(("terminal-panel-strip", key)).max_rect(strip), |ui| {
            ui.set_clip_rect(strip.expand2(vec2(0., 1.)));
            crate::preview_ui::tab_strip(ui, &p, strip, &infos, crate::preview_ui::Entry::Tab(panel.active), None, &right, "New terminal", rename_entry.zip(renaming.as_mut()))
        })
        .inner;
    app.browsers.terms.renaming = renaming;
    if let Some(crate::preview_ui::Entry::Tab(id)) = out.rename {
        app.browsers.terms.begin_rename(id);
    }
    match out.rename_done {
        Some(true) => app.browsers.terms.commit_rename(&mut app.client),
        Some(false) => app.browsers.terms.cancel_rename(),
        None => {}
    }
    let mut new_shell: Option<Option<yaac_protocol::TerminalShell>> = None;
    if out.plus.clicked() {
        new_shell = Some(None);
    }
    // A tab's right-click: New terminal with shell…, Close.
    if let Some((crate::preview_ui::Entry::Tab(tab), resp)) = &out.context {
        let tab = *tab;
        let shells = match app.browsers.terms.shells(&mut app.client, &key.target) {
            crate::terminal::Shells::Ready(list) => list.clone(),
            _ => Vec::new(),
        };
        let mut close = false;
        let mut rename = false;
        widgets::menu_popup(egui::Popup::context_menu(resp), &p).show(|ui| {
            ui.set_min_width(220.);
            let (row, rr) = widgets::menu_row(ui, &p, false, true);
            let g = ui.fonts_mut(|f| f.layout_no_wrap("Rename".into(), theme::ui(size::UI), p.text));
            ui.painter().galley(pos2(rr.left() + 34., rr.center().y - g.size().y / 2.), g, p.text);
            if row.clicked() {
                rename = true;
                ui.close();
            }
            ui.separator();
            let g = ui.fonts_mut(|f| f.layout_no_wrap("New terminal with shell".into(), theme::ui(size::SMALL), p.text2));
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 22.), egui::Sense::hover());
            ui.painter().galley(pos2(r.left() + 12., r.center().y - g.size().y / 2.), g, p.text2);
            if shells.is_empty() {
                ui.label(widgets::secondary("Loading shells…", &p));
            }
            for (i, sh) in shells.iter().enumerate() {
                let (row, rr) = widgets::menu_row(ui, &p, false, true);
                icons::prompt(ui.painter(), pos2(rr.left() + 18., rr.center().y), 16., p.text);
                let label = if i == 0 { format!("{} (default)", sh.name) } else { sh.name.clone() };
                let g = ui.fonts_mut(|f| f.layout_no_wrap(label, theme::ui(size::UI), p.text));
                ui.painter().galley(pos2(rr.left() + 34., rr.center().y - g.size().y / 2.), g, p.text);
                if row.on_hover_text(&sh.path).clicked() {
                    new_shell = Some(Some(sh.clone()));
                    ui.close();
                }
            }
            ui.separator();
            let (row, rr) = widgets::menu_row(ui, &p, false, true);
            let g = ui.fonts_mut(|f| f.layout_no_wrap("Close terminal".into(), theme::ui(size::UI), p.text));
            ui.painter().galley(pos2(rr.left() + 34., rr.center().y - g.size().y / 2.), g, p.text);
            if row.clicked() {
                close = true;
                ui.close();
            }
        });
        if close {
            app.browsers.terms.close(&mut app.client, tab);
            forget(ui.ctx(), tab);
        }
        if rename {
            app.browsers.terms.begin_rename(tab);
        }
    }
    if let Some(id) = out.close {
        app.browsers.terms.close(&mut app.client, id);
        forget(ui.ctx(), id);
        if !app.browsers.terms.panel_open(key) {
            app.browsers.terms.focus_composer = true;
        }
    } else if let Some(crate::preview_ui::Entry::Tab(id)) = out.select {
        app.browsers.terms.panel_select(key, id);
    }
    match out.right {
        Some(0) => {
            let _ = app.browsers.terms.toggle_panel(&app.client, key);
        }
        Some(1) => app.browsers.terms.panel_maximise(key),
        _ => {}
    }
    if let Some(shell) = new_shell
        && let Err(e) = app.browsers.terms.panel_new(&app.client, key, shell)
    {
        app.browsers.notice = Some(e);
    }
    let Some(active) = app.browsers.terms.panel(key).filter(|p| p.open).map(|p| p.active) else { return };
    let below = Rect::from_min_max(pos2(rect.left(), strip.bottom()), rect.max);
    ui.scope_builder(egui::UiBuilder::new().id_salt(("terminal-panel", key, active)).max_rect(below), |ui| {
        ui.set_clip_rect(below);
        session_view(ui, app, active, below, Some(key), true);
    });
}

/// The panel's height in a chat column of `column` points (the divider's share, clamped).
pub(crate) fn panel_height(share: f32, column: f32, composer: f32) -> f32 {
    let max = (column - composer - TRANSCRIPT_MIN).max(PANEL_MIN);
    (share * column).clamp(PANEL_MIN, max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    const AREA: Rect = Rect { min: Pos2 { x: 0., y: 0. }, max: Pos2 { x: 1100., y: 700. } };

    struct Rig {
        ctx: egui::Context,
        app: YaacApp,
        id: u64,
    }

    impl Rig {
        fn new() -> Self {
            let ctx = egui::Context::default();
            let mut app = crate::app::tests::demo_app(&ctx);
            let key = yaac_core::demo::threads::orchestrator();
            let id = app.browsers.terms.insert(&key, None, "/work".into());
            app.browsers.terms.detach_demo(id);
            let mut rig = Self { ctx, app, id };
            // Fonts and layout settle; the first frame sizes the grid, which the terminal's
            // own thread applies, so a second frame draws the settled grid.
            rig.frame();
            rig.frame();
            rig
        }

        /// Waits until the terminal's thread has parsed everything fed to it and applied any
        /// resize, so a test frame never races it (CI machines are slower than a laptop).
        fn settle(&mut self) {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !self.session().term.idle() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(1));
            }
        }

        /// One frame: run the grid, then tessellate (what a real frame costs on the CPU).
        /// Returns the meshes painted and the time it took.
        fn frame(&mut self) -> (usize, Duration) {
            self.settle();
            let start = Instant::now();
            let id = self.id;
            let app = &mut self.app;
            let mut out = self.ctx.run_ui(
                egui::RawInput { screen_rect: Some(AREA), ..Default::default() },
                |ui| grid(ui, app, id, AREA, None),
            );
            let meshes = out.shapes.iter().filter(|s| matches!(s.shape, egui::Shape::Mesh(_))).count();
            let ppp = out.pixels_per_point;
            let _ = self.ctx.tessellate(std::mem::take(&mut out.shapes), ppp);
            out.textures_delta.clear();
            (meshes, start.elapsed())
        }

        fn session(&mut self) -> &mut crate::terminal::Session {
            self.app.browsers.terms.get_mut(self.id).unwrap()
        }

        fn stats(&self) -> (u64, u64) {
            let r = renderer(&self.ctx);
            let r = r.lock().unwrap();
            (r.rows_built, r.assembled)
        }
    }

    #[test]
    fn the_grid_is_one_mesh_and_only_damaged_rows_are_rebuilt() {
        let mut rig = Rig::new();
        rig.session().term.feed(b"$ echo hi\r\nhi\r\n$ ");
        let (meshes, _) = rig.frame();
        assert_eq!(meshes, 1, "one mesh for the whole grid");
        let (built, assembled) = rig.stats();
        // Nothing changed: nothing rebuilt, the assembled mesh reused.
        rig.frame();
        assert_eq!(rig.stats(), (built, assembled));
        // A keystroke's echo rebuilds its one row.
        rig.session().term.feed(b"l");
        rig.frame();
        assert_eq!(rig.stats().0, built + 1);
        // Switching between two dark palettes invalidates already painted glyphs.
        rig.app.p.text = Color32::from_rgb(121, 222, 113);
        rig.frame();
        assert!(rig.stats().0 > built + 1);
    }

    #[test]
    fn an_echo_is_on_screen_in_the_frame_after_its_bytes() {
        let mut rig = Rig::new();
        rig.session().term.feed(b"$ ");
        rig.frame();
        let start = Instant::now();
        rig.session().term.feed(b"x");
        rig.frame();
        let elapsed = start.elapsed();
        let row = rig.session().frame.lines[0].text();
        assert_eq!(row.trim_end(), "$ x", "drawn in the very next frame");
        eprintln!("bytes → painted mesh: {elapsed:?}");
    }

    #[test]
    fn the_grid_fits_the_area_and_tells_the_session() {
        let mut rig = Rig::new();
        rig.frame();
        let (cols, rows) = rig.session().size().unwrap();
        let m = renderer(&rig.ctx).lock().unwrap().atlas.metrics.unwrap();
        assert_eq!(u32::from(cols), ((AREA.width() - 2. * PAD.x) / m.cw).floor() as u32);
        assert_eq!(u32::from(rows), ((AREA.height() - 2. * PAD.y) / m.rh).floor() as u32);
        assert_eq!((rig.session().frame.cols, rig.session().frame.rows), (usize::from(cols), usize::from(rows)));
    }

    #[test]
    fn box_drawing_is_shapes_not_glyphs() {
        let mut m = egui::Mesh::default();
        let metrics = Metrics { cw: 8., rh: 16., ppp: 1., base: 12., px: 1. };
        let cell = Rect::from_min_size(Pos2::ZERO, vec2(8., 16.));
        for c in ['─', '│', '┼', '╔', '╬', '╭', '█', '▀', '░', '\u{E0B0}', '┄'] {
            let before = m.vertices.len();
            assert!(special(&mut m, c, cell, Color32::WHITE, Color32::BLACK, &metrics), "{c}");
            assert!(m.vertices.len() > before, "{c} drew something");
            assert!(m.vertices[before..].iter().all(|v| v.uv == egui::epaint::WHITE_UV), "{c} is untextured");
        }
        assert!(!special(&mut m, 'a', cell, Color32::WHITE, Color32::BLACK, &metrics));
        // A horizontal line spans the cell exactly, so neighbours join.
        let mut h = egui::Mesh::default();
        special(&mut h, '─', cell, Color32::WHITE, Color32::BLACK, &metrics);
        let xs: Vec<f32> = h.vertices.iter().map(|v| v.pos.x).collect();
        assert_eq!(xs.iter().copied().fold(f32::MAX, f32::min), 0.);
        assert_eq!(xs.iter().copied().fold(f32::MIN, f32::max), 8.);
    }

    fn cpu_seconds() -> f64 {
        // utime + stime of this process, in clock ticks (Linux).
        let stat = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
        let after = stat.rsplit_once(')').map_or("", |(_, r)| r);
        let f: Vec<&str> = after.split_whitespace().collect();
        let ticks = |i: usize| f.get(i).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.);
        (ticks(11) + ticks(12)) / 100.
    }

    /// 100 MB streamed through the emulator and the renderer at 1 Gbit/s (`cat` of a big file
    /// over a fast link), one frame per 16 ms of link time: frame times and CPU.
    /// `cargo test -p yaac-app --release --lib terminal_ui::tests::flood -- --ignored --nocapture`
    #[test]
    #[ignore = "benchmark"]
    fn flood() {
        let mut rig = Rig::new();
        let mut line = Vec::new();
        for i in 0..4000 {
            line.extend_from_slice(format!("\x1b[3{}m{i:>6}\x1b[0m src/session.rs:{i}: \x1b[1mwarning\x1b[0m: unused variable `idle` ── consider `_idle` ──────────\r\n", i % 8).as_bytes());
        }
        let total = 100usize << 20;
        let per_frame = 2 * 1024 * 1024; // 125 MB/s × 16 ms
        let mut sent = 0;
        let mut times = Vec::new();
        let cpu0 = cpu_seconds();
        let wall = Instant::now();
        while sent < total {
            let mut budget = per_frame.min(total - sent);
            // Arrives in 16 KiB pieces, as the target flushes them.
            while budget > 0 {
                let off = sent % line.len();
                let n = (line.len() - off).min(16 * 1024).min(budget);
                rig.session().term.feed(&line[off..off + n]);
                sent += n;
                budget -= n;
            }
            let (_, t) = rig.frame();
            times.push(t);
        }
        while !rig.session().term.idle() {
            let (_, t) = rig.frame();
            times.push(t);
        }
        let wall = wall.elapsed().as_secs_f64();
        let cpu = cpu_seconds() - cpu0;
        times.sort();
        let pct = |p: f64| times[((times.len() - 1) as f64 * p) as usize];
        eprintln!(
            "flood: {} MB in {wall:.2} s ({:.0} MB/s), {} frames; frame time p50 {:?}, p99 {:?}, max {:?}; CPU {cpu:.2} s ({:.0} % of one core)",
            total >> 20,
            (total >> 20) as f64 / wall,
            times.len(),
            pct(0.5),
            pct(0.99),
            times.last().unwrap(),
            cpu / wall * 100.
        );
        assert!(*times.last().unwrap() < Duration::from_millis(16), "every frame under 16 ms");
        // Typing while idle: bytes to painted mesh.
        let mut worst = Duration::ZERO;
        for _ in 0..200 {
            let start = Instant::now();
            rig.session().term.feed(b"k");
            rig.frame();
            worst = worst.max(start.elapsed());
        }
        eprintln!("typing: echo bytes → painted and tessellated, worst of 200: {worst:?}");
    }
}

#[cfg(test)]
mod focus_tests {
    use super::*;

    #[test]
    fn a_requested_focus_sticks() {
        let ctx = egui::Context::default();
        let mut app = crate::app::tests::demo_app(&ctx);
        let key = yaac_core::demo::threads::orchestrator();
        let id = app.browsers.terms.insert(&key, None, "/work".into());
        app.browsers.terms.detach_demo(id);
        let area = Rect::from_min_size(Pos2::ZERO, vec2(600., 400.));
        for i in 0..5 {
            let mut out = ctx.run_ui(egui::RawInput { screen_rect: Some(area), ..Default::default() }, |ui| grid(ui, &mut app, id, area, None));
            out.textures_delta.clear();
            eprintln!("frame {i}: focused {}", ctx.memory(|m| m.has_focus(widget_id(id))));
        }
        assert!(ctx.memory(|m| m.has_focus(widget_id(id))));
    }
}
