//! The harness logos (decisions D97, D98): the owner's SVG marks in `assets/logos`, each a
//! single path, rasterised into an alpha mask at the size drawn (cached per size) and drawn
//! tinted. The Claude mark keeps its brand colour; the Codex mark has no colour of its own and
//! is drawn in the theme's text colour. That is the one exception to the monochrome rule, and
//! it lives here only.

use eframe::egui::{self, Color32, ColorImage, Painter, Pos2, Rect, TextureHandle, TextureOptions, pos2, vec2};

/// A logo.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Logo {
    Codex,
    Claude,
}

/// Provider kind → logo and its own colour (`None`: the theme's text colour). Data, looked up
/// — never a branch on the kind in UI code. A kind not listed falls back to its initial in a
/// circle.
pub(crate) const LOGOS: &[(&str, Logo, Option<Color32>)] =
    &[("codex", Logo::Codex, None), ("claude", Logo::Claude, Some(Color32::from_rgb(0xD9, 0x77, 0x57)))];

/// The logo for a provider kind, with its own colour if it has one.
pub(crate) fn for_kind(kind: &str) -> Option<(Logo, Option<Color32>)> {
    LOGOS.iter().find(|(k, _, _)| *k == kind).map(|(_, l, c)| (*l, *c))
}

const CODEX_SVG: &str = include_str!("../assets/logos/codex.svg");
const CLAUDE_SVG: &str = include_str!("../assets/logos/claude.svg");

/// The path data and the view box (width, height) of a logo.
fn source(logo: Logo) -> (String, (f32, f32)) {
    let svg = match logo {
        Logo::Codex => CODEX_SVG,
        Logo::Claude => CLAUDE_SVG,
    };
    let attr = |name: &str| -> &'static str {
        let key = format!(" {name}=\"");
        svg.find(&key).map_or("", |i| {
            let rest = &svg[i + key.len()..];
            &rest[..rest.find('"').unwrap_or(rest.len())]
        })
    };
    let vb: Vec<f32> = attr("viewBox").split_whitespace().filter_map(|v| v.parse().ok()).collect();
    let size = if vb.len() == 4 { (vb[2], vb[3]) } else { (256.0, 256.0) };
    (plain_numbers(attr("d")), size)
}

/// The rasteriser reads no exponents ("3.3e-05"): write such numbers out in full.
fn plain_numbers(d: &str) -> String {
    d.split_whitespace()
        .map(|tok| match tok.parse::<f64>() {
            Ok(v) if tok.contains(['e', 'E']) => format!("{v:.6}"),
            _ => tok.to_owned(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// An alpha mask of `logo`, `px` pixels square, the mark centred and fitted.
pub(crate) fn mask(logo: Logo, px: u32) -> Vec<u8> {
    let (d, (w, h)) = source(logo);
    let s = px as f32 / w.max(h);
    let (dx, dy) = ((px as f32 - w * s) / 2.0, (px as f32 - h * s) / 2.0);
    let t = zeno::Transform::scale(s, s).then_translate(dx, dy);
    let (data, place) = zeno::Mask::new(d.as_str()).transform(Some(t)).size(px, px).render();
    // Copy into a px × px buffer at the mask's placement.
    let mut out = vec![0u8; (px * px) as usize];
    for y in 0..place.height {
        for x in 0..place.width {
            let (tx, ty) = (place.left + x as i32, place.top + y as i32);
            if tx >= 0 && ty >= 0 && (tx as u32) < px && (ty as u32) < px {
                out[(ty as u32 * px + tx as u32) as usize] = data[(y * place.width + x) as usize];
            }
        }
    }
    out
}

fn texture(ctx: &egui::Context, logo: Logo, px: u32) -> TextureHandle {
    let id = egui::Id::unique(("logo", logo, px));
    if let Some(t) = ctx.data(|d| d.get_temp::<TextureHandle>(id)) {
        return t;
    }
    let alpha = mask(logo, px);
    let img = ColorImage::from_rgba_unmultiplied([px as usize, px as usize], &alpha.iter().flat_map(|a| [255, 255, 255, *a]).collect::<Vec<u8>>());
    let t = ctx.load_texture(format!("logo-{logo:?}-{px}"), img, TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, t.clone()));
    t
}

/// Draw `logo` centred at `c`, `d` points across, in `color`.
pub(crate) fn draw(painter: &Painter, c: Pos2, d: f32, logo: Logo, color: Color32) {
    let painter = &crate::icons::soft(painter);
    let ppp = painter.ctx().pixels_per_point();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "an icon's size in pixels")]
    let px = ((d * ppp).round() as u32).clamp(8, 256);
    let tex = texture(painter.ctx(), logo, px);
    let rect = Rect::from_center_size(c, vec2(d, d));
    painter.image(tex.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), color);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Palette;

    #[test]
    fn both_marks_rasterise_centred_and_inside() {
        for logo in [Logo::Codex, Logo::Claude] {
            let px = 64u32;
            let m = mask(logo, px);
            let covered = m.iter().filter(|a| **a > 128).count();
            if std::env::var_os("LOGO_ASCII").is_some() {
                for row in m.chunks(px as usize) {
                    eprintln!("{}", row.iter().map(|a| if *a > 128 { '#' } else if *a > 0 { '.' } else { ' ' }).collect::<String>());
                }
            }
            assert!(covered > 300, "{logo:?}: {covered} pixels");
            // Roughly centred: the coverage's centre of mass is near the middle.
            let (mut sx, mut sy, mut n) = (0.0, 0.0, 0.0);
            for (i, a) in m.iter().enumerate() {
                let w = f64::from(*a);
                let (x, y) = (i % px as usize, i / px as usize);
                sx += x as f64 * w;
                sy += y as f64 * w;
                n += w;
            }
            let (cx, cy) = (sx / n, sy / n);
            assert!((cx - 32.0).abs() < 6.0 && (cy - 32.0).abs() < 6.0, "{logo:?}: centre {cx:.1},{cy:.1}");
        }
    }

    #[test]
    fn the_brand_colour_is_only_claudes_and_its_contrast_is_measured() {
        assert_eq!(for_kind("codex").map(|x| x.1), Some(None), "Codex is drawn in the text colour");
        assert!(for_kind("gemini").is_none(), "unknown kinds fall back to an initial");
        let orange = for_kind("claude").and_then(|x| x.1).unwrap();
        // Recorded, not enforced (D98: report rather than change the colour). An icon is a
        // graphical object: WCAG asks 3 : 1 of those.
        for p in [Palette::DARK, Palette::LIGHT] {
            for (name, s) in [("panel", p.panel), ("raised", p.raised), ("selected", p.selected), ("window", p.bg)] {
                let r = crate::theme::contrast(orange, s);
                eprintln!("claude orange on {} {name}: {r:.2}", if p.dark { "dark" } else { "light" });
                assert!(r >= 2.5, "{name}: {r:.2}");
            }
        }
    }
}
