#![doc = include_str!("../README.md")]

use egui::{Color32, ColorImage, Key};

/// Convert a CEF dirty rectangle into the premultiplied texture patch consumed by
/// egui's TextureHandle::set_partial. CEF supplies a full view-sized BGRA buffer.
pub fn paint_patch(
    buffer: &[u8],
    [width, height]: [usize; 2],
    [x, y, w, h]: [usize; 4],
) -> Option<ColorImage> {
    if w == 0 || h == 0 || x.checked_add(w)? > width || y.checked_add(h)? > height {
        return None;
    }
    let stride = width.checked_mul(4)?;
    if buffer.len() != stride.checked_mul(height)? {
        return None;
    }
    let mut pixels = Vec::with_capacity(w * h);
    for row in y..y + h {
        let start = row * stride + x * 4;
        for bgra in buffer[start..start + w * 4].as_chunks::<4>().0 {
            pixels.push(Color32::from_rgba_premultiplied(
                bgra[2], bgra[1], bgra[0], bgra[3],
            ));
        }
    }
    Some(ColorImage::new([w, h], pixels))
}

/// Windows virtual-key code for CEF's raw key events on both supported platforms.
/// Text/IME events carry the actual composed characters separately.
pub fn virtual_key(key: Key) -> Option<i32> {
    use Key::*;
    Some(match key {
        Backspace => 8,
        Tab => 9,
        Enter => 13,
        Escape => 27,
        Space => 32,
        PageUp => 33,
        PageDown => 34,
        End => 35,
        Home => 36,
        ArrowLeft => 37,
        ArrowUp => 38,
        ArrowRight => 39,
        ArrowDown => 40,
        Insert => 45,
        Delete => 46,
        Semicolon | Colon => 0xba,
        Equals | Plus => 0xbb,
        Comma => 0xbc,
        Minus => 0xbd,
        Period => 0xbe,
        Slash | Questionmark => 0xbf,
        Backtick => 0xc0,
        OpenBracket | OpenCurlyBracket => 0xdb,
        Backslash | Pipe => 0xdc,
        CloseBracket | CloseCurlyBracket => 0xdd,
        Quote => 0xde,
        Exclamationmark => 49,
        BrowserBack => 0xa6,
        BrowserForward => 0xa7,
        BrowserRefresh => 0xa8,
        BrowserStop => 0xa9,
        BrowserSearch => 0xaa,
        BrowserFavorites => 0xab,
        BrowserHome => 0xac,
        k if (A..=Z).contains(&k) => 65 + k as i32 - A as i32,
        k if (Num0..=Num9).contains(&k) => 48 + k as i32 - Num0 as i32,
        k if (F1..=F24).contains(&k) => 112 + k as i32 - F1 as i32,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dirty_patch_uses_full_row_stride_and_preserves_alpha() {
        let pixels: Vec<u8> = (0..24).collect();
        let patch = paint_patch(&pixels, [3, 2], [1, 0, 1, 2]).unwrap();
        assert_eq!(patch.pixels[0].to_array(), [6, 5, 4, 7]);
        assert_eq!(patch.pixels[1].to_array(), [18, 17, 16, 19]);
        assert!(paint_patch(&pixels, [3, 2], [usize::MAX, 0, 1, 1]).is_none());
        assert!(paint_patch(&pixels[..23], [3, 2], [0, 0, 1, 1]).is_none());
    }
    #[test]
    fn translates_navigation_letters_and_punctuation() {
        for (key, value) in [
            (Key::ArrowLeft, 37),
            (Key::Z, 90),
            (Key::Num9, 57),
            (Key::F12, 123),
            (Key::Questionmark, 191),
        ] {
            assert_eq!(virtual_key(key), Some(value));
        }
    }
}

mod client;
pub mod wire;
pub use client::*;
mod widget;
pub use widget::{PageState, Preview};
mod diagnostics;
