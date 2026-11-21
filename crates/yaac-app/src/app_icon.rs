//! The application icon, drawn in code so no image file is shipped: a rounded square in the
//! dark theme's accent with a light "Y" of three strokes. The window uses it at run time, and
//! on Windows `build.rs` (which includes this file) writes it into the executable's resources
//! so Explorer and the taskbar show it too.

/// The icon as straight RGBA, `size` × `size` pixels.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "colour channels clamped to 0..=255")]
pub(crate) fn rgba(size: u32) -> Vec<u8> {
    const ACCENT: [f32; 3] = [108.0, 140.0, 255.0];
    const MARK: [f32; 3] = [246.0, 247.0, 250.0];
    let s = size as f32;
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    // In unit coordinates (0..1): the square's corner radius, and the Y's strokes.
    let radius = 0.22;
    let half_width = 0.075;
    let (fork, top_l, top_r, foot) = ((0.5, 0.52), (0.27, 0.22), (0.73, 0.22), (0.5, 0.80));
    let segment = |p: (f32, f32), a: (f32, f32), b: (f32, f32)| {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let t = (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
        let (cx, cy) = (a.0 + t * dx - p.0, a.1 + t * dy - p.1);
        (cx * cx + cy * cy).sqrt()
    };
    for y in 0..size {
        for x in 0..size {
            let p = ((x as f32 + 0.5) / s, (y as f32 + 0.5) / s);
            // Signed distance to the rounded square (negative inside).
            let qx = (p.0 - 0.5).abs() - (0.5 - radius);
            let qy = (p.1 - 0.5).abs() - (0.5 - radius);
            let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - radius;
            let px = 1.0 / s;
            let cover = (0.5 - outside / px).clamp(0.0, 1.0);
            let stroke = segment(p, top_l, fork).min(segment(p, top_r, fork)).min(segment(p, fork, foot)) - half_width;
            let mark = (0.5 - stroke / px).clamp(0.0, 1.0);
            for c in 0..3 {
                out.push((ACCENT[c] + (MARK[c] - ACCENT[c]) * mark).round() as u8);
            }
            out.push((cover * 255.0).round() as u8);
        }
    }
    out
}

/// The icon with a small red badge on its bottom-right corner: the tray's "some thread needs
/// you" variant (D127), in a red that reads on light and dark taskbars alike.
#[allow(dead_code, reason = "only the Windows tray uses it; build.rs includes this file too")]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "colour channels clamped to 0..=255")]
pub(crate) fn badged(size: u32) -> Vec<u8> {
    const RED: [f32; 3] = [229.0, 72.0, 77.0];
    let mut out = rgba(size);
    let s = size as f32;
    // The badge's centre and radius, and a ring of transparency around it so it reads as a
    // separate mark over the icon.
    let (cx, cy, r, ring) = (0.76 * s, 0.76 * s, 0.24 * s, 0.07 * s);
    for y in 0..size {
        for x in 0..size {
            let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
            let i = ((y * size + x) * 4) as usize;
            let dot = (r - d + 0.5).clamp(0.0, 1.0);
            let gap = (r + ring - d + 0.5).clamp(0.0, 1.0);
            // Cut the gap out of the icon, then lay the dot over it.
            let under = f32::from(out[i + 3]) * (1.0 - gap);
            let alpha = dot * 255.0 + under * (1.0 - dot);
            for c in 0..3 {
                let base = f32::from(out[i + c]);
                out[i + c] = (RED[c] * dot + base * (1.0 - dot)).round() as u8;
            }
            out[i + 3] = alpha.round().clamp(0.0, 255.0) as u8;
        }
    }
    out
}
