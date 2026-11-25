//! The per-frame snapshot both renderers draw from: rows of resolved cells, what changed, the
//! cursor, the selection and search matches (docs/13-terminal.md#emulator--yaac-term).
use std::sync::Arc;

/// A cell colour, theme-neutral: the theme's default foreground or background, one of the 256
/// indexed colours, or a program's own RGB. [`crate::Theme::rgb`] turns it into pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Color {
    /// The theme's default foreground.
    #[default]
    Fg,
    /// The theme's default background.
    Bg,
    /// 0–15 the theme's ANSI colours, 16–231 the 6×6×6 cube, 232–255 the grey ramp.
    Index(u8),
    /// Truecolor.
    Rgb(u8, u8, u8),
}

/// Cell attributes. Inverse and hidden are already resolved by the emulator (colours swapped,
/// glyph blanked), so renderers never see them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Flags(pub u16);

impl Flags {
    pub const BOLD: u16 = 1;
    pub const ITALIC: u16 = 1 << 1;
    pub const UNDERLINE: u16 = 1 << 2;
    pub const DOUBLE_UNDERLINE: u16 = 1 << 3;
    /// Curly, dotted and dashed underlines (drawn as one style).
    pub const FANCY_UNDERLINE: u16 = 1 << 4;
    pub const STRIKE: u16 = 1 << 5;
    pub const DIM: u16 = 1 << 6;
    /// A wide character (CJK, emoji): this cell and the next, a [`Flags::SPACER`].
    pub const WIDE: u16 = 1 << 7;
    /// The second half of a wide character: draw nothing.
    pub const SPACER: u16 = 1 << 8;
    /// Part of an OSC 8 hyperlink ([`Row::links`]).
    pub const LINK: u16 = 1 << 9;

    #[inline]
    pub fn has(self, bit: u16) -> bool {
        self.0 & bit != 0
    }
    /// Any underline style.
    #[inline]
    pub fn underlined(self) -> bool {
        self.has(Self::UNDERLINE | Self::DOUBLE_UNDERLINE | Self::FANCY_UNDERLINE)
    }
}

/// One grid cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Cell {
    pub ch: char,
    pub fg: Color,
    pub bg: Color,
    pub flags: Flags,
}

impl Default for Cell {
    fn default() -> Self {
        Self { ch: ' ', fg: Color::Fg, bg: Color::Bg, flags: Flags::default() }
    }
}

impl Cell {
    /// Nothing to draw in this cell besides its background.
    #[inline]
    pub fn blank(&self) -> bool {
        (self.ch == ' ' || self.flags.has(Flags::SPACER)) && !self.flags.underlined() && !self.flags.has(Flags::STRIKE)
    }
}

/// An OSC 8 hyperlink's cells on one row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkSpan {
    /// First column.
    pub start: usize,
    /// Last column, inclusive.
    pub end: usize,
    pub uri: Arc<str>,
}

/// One row of the viewport.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Row {
    pub cells: Vec<Cell>,
    /// The row continues on the next one (a soft wrap): selection, links and copy join them.
    pub wrapped: bool,
    /// Combining characters (zero-width) drawn over a cell: `(column, chars)`.
    pub combining: Vec<(usize, Box<str>)>,
    pub links: Vec<LinkSpan>,
}

/// A run of cells with the same colours and attributes (for `GlyphRun`s and background rects).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Run {
    pub start: usize,
    pub len: usize,
    pub fg: Color,
    pub bg: Color,
    pub flags: Flags,
}

impl Row {
    /// The row as text (spacers skipped, trailing blanks kept).
    pub fn text(&self) -> String {
        let mut s = String::with_capacity(self.cells.len());
        for (i, c) in self.cells.iter().enumerate() {
            if c.flags.has(Flags::SPACER) {
                continue;
            }
            s.push(c.ch);
            if let Some((_, extra)) = self.combining.iter().find(|(col, _)| *col == i) {
                s.push_str(extra);
            }
        }
        s
    }

    /// Style runs, left to right. Spacer cells join the run of their wide character.
    pub fn runs(&self) -> impl Iterator<Item = Run> + '_ {
        let mut i = 0;
        std::iter::from_fn(move || {
            let first = self.cells.get(i)?;
            let key = |c: &Cell| (c.fg, c.bg, Flags(c.flags.0 & !(Flags::WIDE | Flags::SPACER)));
            let k = key(first);
            let start = i;
            i += 1;
            while let Some(c) = self.cells.get(i) {
                if !(c.flags.has(Flags::SPACER) || key(c) == k) {
                    break;
                }
                i += 1;
            }
            Some(Run { start, len: i - start, fg: k.0, bg: k.1, flags: k.2 })
        })
    }

    /// The OSC 8 link at a column.
    pub fn link_at(&self, col: usize) -> Option<&LinkSpan> {
        self.links.iter().find(|l| (l.start..=l.end).contains(&col))
    }
}

/// Cursor shape, as the program asked for it (DECSCUSR).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CursorShape {
    #[default]
    Block,
    Bar,
    Underline,
    /// Drawn when the terminal does not have focus, whatever the program asked for.
    HollowBlock,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Cursor {
    pub row: usize,
    pub col: usize,
    pub shape: CursorShape,
    /// Shown at all (DECTCEM, and inside the viewport).
    pub visible: bool,
    /// Over a wide character: two cells wide.
    pub wide: bool,
    /// The program asked for a blinking cursor. Renderers still blink only while focused and
    /// for 30 s after the last key.
    pub blinking: bool,
}

/// A range of cells in viewport coordinates, inclusive, in reading order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: (usize, usize),
    pub end: (usize, usize),
    /// A rectangular (Alt+drag) selection: columns `start.1..=end.1` on every row.
    pub block: bool,
}

impl Span {
    /// The columns covered on `row`, if any.
    pub fn columns(&self, row: usize, cols: usize) -> Option<(usize, usize)> {
        if row < self.start.0 || row > self.end.0 || cols == 0 {
            return None;
        }
        if self.block {
            let (a, b) = (self.start.1.min(self.end.1), self.start.1.max(self.end.1));
            return Some((a, b.min(cols - 1)));
        }
        let a = if row == self.start.0 { self.start.1 } else { 0 };
        let b = if row == self.end.0 { self.end.1 } else { cols - 1 };
        (a <= b).then_some((a, b.min(cols - 1)))
    }
}

/// Terminal modes that change how input is encoded or how the view behaves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Modes {
    pub app_cursor: bool,
    pub app_keypad: bool,
    pub bracketed_paste: bool,
    pub alt_screen: bool,
    pub alternate_scroll: bool,
    /// Mouse reporting: clicks (1000), drags (1002), all motion (1003).
    pub mouse_click: bool,
    pub mouse_drag: bool,
    pub mouse_motion: bool,
    pub sgr_mouse: bool,
    pub utf8_mouse: bool,
    pub focus_events: bool,
    /// ConPTY's win32-input-mode (DECSET 9001): keys go as full Windows key events, so
    /// PSReadLine sees exactly what it would in Windows Terminal.
    pub win32_input: bool,
    /// Newline mode (LNM): Enter sends CR LF.
    pub line_feed_new_line: bool,
}

impl Modes {
    /// Any mouse reporting.
    pub fn mouse(&self) -> bool {
        self.mouse_click || self.mouse_drag || self.mouse_motion
    }
}

/// The search as of this frame.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct SearchView {
    pub query: String,
    /// The query is not a valid search (never for plain text).
    pub invalid: bool,
    /// Visible matches.
    pub matches: Vec<Span>,
    /// The current match, if visible.
    pub focused: Option<Span>,
    /// Whether any match exists in the buffer (scrollback included).
    pub found: bool,
}

/// Everything a renderer needs for one frame. Keep one per tab and pass it to
/// [`crate::Terminal::snapshot`] every frame the tab is drawn; only changed rows are rewritten.
#[derive(Clone, Debug, Default)]
pub struct Frame {
    pub cols: usize,
    pub rows: usize,
    pub lines: Vec<Row>,
    /// Rows whose cells changed since the previous snapshot (rebuild only these).
    pub damaged: Vec<usize>,
    /// Every row changed (resize, scroll, theme): drop row caches.
    pub full: bool,
    pub cursor: Cursor,
    pub selection: Option<Span>,
    pub search: Option<SearchView>,
    /// Lines scrolled back from the bottom (0 = following output).
    pub display_offset: usize,
    /// Lines of scrollback.
    pub history: usize,
    pub modes: Modes,
    /// The latest OSC 0/2 title, if the program set one.
    pub title: Option<String>,
    /// Bumped whenever anything above changed: a renderer with an unchanged generation has
    /// nothing to redraw.
    pub generation: u64,
}

impl Frame {
    /// Text of a span, rows joined with newlines except across soft wraps; trailing blanks
    /// trimmed per line.
    pub fn span_text(&self, span: &Span) -> String {
        let mut out = String::new();
        for row in span.start.0..=span.end.0.min(self.rows.saturating_sub(1)) {
            let Some(line) = self.lines.get(row) else { break };
            let Some((a, b)) = span.columns(row, self.cols) else { continue };
            let text: String = line.cells[a..=b.min(line.cells.len().saturating_sub(1))]
                .iter()
                .filter(|c| !c.flags.has(Flags::SPACER))
                .map(|c| c.ch)
                .collect();
            let joined = line.wrapped && !span.block && row != span.end.0;
            out.push_str(if joined { &text } else { text.trim_end() });
            if row != span.end.0 && !joined {
                out.push('\n');
            }
        }
        out
    }
}
