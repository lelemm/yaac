//! The terminal colour schemes for YAAC's dark and light themes. Both renderers resolve cells
//! through these exact values, so their pixels match.
use crate::frame::{Color, Flags};

/// An sRGB colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rgb(pub u8, pub u8, pub u8);

const fn hex(v: u32) -> Rgb {
    let [_, r, g, b] = v.to_be_bytes();
    Rgb(r, g, b)
}

/// A terminal colour scheme.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Theme {
    pub dark: bool,
    pub fg: Rgb,
    pub bg: Rgb,
    pub cursor: Rgb,
    /// Text drawn over a block cursor.
    pub cursor_text: Rgb,
    /// Selection fill (opaque, drawn under the glyphs).
    pub selection: Rgb,
    /// Search matches and the current match.
    pub search: Rgb,
    pub search_focused: Rgb,
    /// Text over a search match.
    pub search_text: Rgb,
    /// ANSI 0–15.
    pub ansi: [Rgb; 16],
}

impl Theme {
    /// On YAAC's dark content background (#16181D, P315's colour tokens).
    pub const DARK: Theme = Theme {
        dark: true,
        fg: hex(0xE6E8EC),
        bg: hex(0x16181D),
        cursor: hex(0xE6E8EC),
        cursor_text: hex(0x16181D),
        selection: hex(0x3D4451),
        search: hex(0x5C4A1E),
        search_focused: hex(0xF2B24C),
        search_text: hex(0x16181D),
        ansi: [
            hex(0x3A3F4B), // black
            hex(0xF07178), // red
            hex(0x73C991), // green
            hex(0xE2C08D), // yellow
            hex(0x6CA8FF), // blue
            hex(0xC792EA), // magenta
            hex(0x5FC9D4), // cyan
            hex(0xC9CED6), // white
            hex(0x7A8291), // bright black
            hex(0xFF9AA2), // bright red
            hex(0x9BE3B0), // bright green
            hex(0xF2D49B), // bright yellow
            hex(0x93C0FF), // bright blue
            hex(0xDDB6F7), // bright magenta
            hex(0x8BE3EA), // bright cyan
            hex(0xF4F5F7), // bright white
        ],
    };

    /// On YAAC's light content background (#FFFFFF). White and bright white are mid greys, so
    /// text programs print in "white" (PSReadLine's arguments, `ls` of some files) stays legible.
    pub const LIGHT: Theme = Theme {
        dark: false,
        fg: hex(0x1A1D23),
        bg: hex(0xFFFFFF),
        cursor: hex(0x1A1D23),
        cursor_text: hex(0xFFFFFF),
        selection: hex(0xCBD0D9),
        search: hex(0xFBE3B0),
        search_focused: hex(0xE59A1A),
        search_text: hex(0x1A1D23),
        ansi: [
            hex(0x1A1D23), // black
            hex(0xC22E29), // red
            hex(0x166F31), // green
            hex(0x855D00), // yellow
            hex(0x1F5FBF), // blue
            hex(0x8E3FB8), // magenta
            hex(0x0E7383), // cyan
            hex(0x6E7781), // white
            hex(0x57606A), // bright black
            hex(0xD6413B), // bright red
            hex(0x1E7F3E), // bright green
            hex(0x9A6C00), // bright yellow
            hex(0x2F6FD6), // bright blue
            hex(0xA04ACB), // bright magenta
            hex(0x0F8194), // bright cyan
            hex(0x8C959F), // bright white
        ],
    };

    /// The colour of index 0–255.
    pub fn indexed(&self, i: u8) -> Rgb {
        match i {
            0..=15 => self.ansi[usize::from(i)],
            16..=231 => {
                let i = i - 16;
                let level = |v: u8| if v == 0 { 0 } else { 55 + 40 * v };
                Rgb(level(i / 36), level((i / 6) % 6), level(i % 6))
            }
            _ => {
                let v = 8 + 10 * (i - 232);
                Rgb(v, v, v)
            }
        }
    }

    pub fn rgb(&self, c: Color) -> Rgb {
        match c {
            Color::Fg => self.fg,
            Color::Bg => self.bg,
            Color::Index(i) => self.indexed(i),
            Color::Rgb(r, g, b) => Rgb(r, g, b),
        }
    }

    /// A cell's glyph colour: dim text is mixed 40 % of the way to the background.
    pub fn fg(&self, fg: Color, bg: Color, flags: Flags) -> Rgb {
        let c = self.rgb(fg);
        if flags.has(Flags::DIM) { mix(c, self.rgb(bg), 0.4) } else { c }
    }
}

/// `a` moved `t` of the way to `b`.
pub fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let f = |x: u8, y: u8| {
        let v = f32::from(x) + (f32::from(y) - f32::from(x)) * t;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let v = v.round().clamp(0.0, 255.0) as u8;
        v
    };
    Rgb(f(a.0, b.0), f(a.1, b.1), f(a.2, b.2))
}
