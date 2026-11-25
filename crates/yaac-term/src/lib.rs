//! YAAC's terminal emulator core (docs/13-terminal.md#emulator--yaac-term), renderer-neutral.
//!
//! A [`Terminal`] wraps `alacritty_terminal`'s parser and grid. Output from the target is
//! [`Terminal::feed`]: small chunks (an echo) are parsed at once on the caller's thread so the
//! very next frame shows them; anything larger, and resizes and searches, go to the terminal's
//! own thread, which parses in bounded slices so a snapshot never waits more than a fraction of
//! a millisecond. Each frame the renderer calls [`Terminal::snapshot`] with its [`Frame`]; only
//! rows that changed are rewritten and listed in [`Frame::damaged`].
//!
//! Input goes the other way: [`Terminal::key`], [`Terminal::text`], [`Terminal::paste`],
//! [`Terminal::mouse`] and [`Terminal::wheel`] return the bytes to send to the target, and
//! [`Terminal::take_events`] the emulator's own replies (cursor reports, colour queries).
#![allow(clippy::cast_possible_truncation)]

mod frame;
mod input;
mod links;
mod theme;

pub use frame::{Cell, Color, Cursor, CursorShape, Flags, Frame, LinkSpan, Modes, Row, Run, SearchView, Span};
pub use input::{Key, KeypadKey, MouseAction, MouseButton, Mods, Paste, Shortcut, encode_focus, encode_key, encode_mouse, encode_paste, shortcut};
pub use links::Link;
pub use theme::{Rgb, Theme, mix};

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Direction, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags as AFlags;
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::term::search::{Match, RegexIter, RegexSearch};
use alacritty_terminal::term::{Config, Term, TermDamage, TermMode};
use alacritty_terminal::vte::ansi::{self, Color as AColor, CursorShape as AShape, Handler, NamedColor, Processor, StdSyncHandler};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

/// Lines kept in the window beyond what the target replays.
pub const SCROLLBACK: usize = 10_000;
/// Output up to this size is parsed on the caller's thread when nothing is queued.
const INLINE_BYTES: usize = 4096;
/// The terminal thread holds the grid for at most this many bytes at a time.
const SLICE_BYTES: usize = 16 * 1024;

/// How a selection grows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectKind {
    /// Character by character (drag).
    Simple,
    /// Whole words (double click).
    Word,
    /// Whole lines (triple click).
    Line,
    /// A rectangle (Alt+drag).
    Block,
}

/// Which half of a cell the pointer is over; a selection starting on the right half of a cell
/// does not include it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellSide {
    Left,
    Right,
}

/// What the emulator produced besides the grid, since the last [`Terminal::take_events`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Events {
    /// Replies to send to the target (cursor position, device attributes, colour queries).
    pub input: Vec<u8>,
    /// Bells rung.
    pub bells: u32,
    /// A program asked to put text on the clipboard (OSC 52).
    pub clipboard: Option<String>,
    /// The title changed (the new one is in the next [`Frame::title`]).
    pub title_changed: bool,
}

#[derive(Default)]
struct Outbox {
    events: Events,
    title: Option<String>,
    theme: Option<Theme>,
    cell: (u16, u16),
    size: (u16, u16),
}

#[derive(Clone)]
struct Listener(Arc<Mutex<Outbox>>);

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        let mut out = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        match event {
            Event::PtyWrite(s) => out.events.input.extend_from_slice(s.as_bytes()),
            Event::Bell => out.events.bells += 1,
            Event::Title(t) => {
                out.title = Some(t);
                out.events.title_changed = true;
            }
            Event::ResetTitle => {
                out.title = None;
                out.events.title_changed = true;
            }
            Event::ClipboardStore(_, s) => out.events.clipboard = Some(s),
            Event::ColorRequest(index, reply) => {
                let theme = out.theme.unwrap_or(Theme::DARK);
                let c = match index {
                    0..=255 => theme.indexed(index as u8),
                    256 => theme.fg,
                    258 => theme.cursor,
                    _ => theme.bg,
                };
                let s = reply(ansi::Rgb { r: c.0, g: c.1, b: c.2 });
                out.events.input.extend_from_slice(s.as_bytes());
            }
            Event::TextAreaSizeRequest(reply) => {
                let ((cw, ch), (cols, rows)) = (out.cell, out.size);
                let s = reply(WindowSize { num_lines: rows, num_cols: cols, cell_width: cw, cell_height: ch });
                out.events.input.extend_from_slice(s.as_bytes());
            }
            _ => {}
        }
    }
}

struct Size(usize, usize);
impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.1
    }
    fn screen_lines(&self) -> usize {
        self.1
    }
    fn columns(&self) -> usize {
        self.0
    }
}

struct SearchState {
    query: String,
    regex: Option<RegexSearch>,
    focused: Option<Match>,
    found: bool,
}

struct Engine {
    term: Term<Listener>,
    parser: Processor<StdSyncHandler>,
    scan: ModeScan,
    win32_input: bool,
    search: Option<SearchState>,
    /// Selection, search or viewport changed outside the parser.
    view_dirty: bool,
    generation: u64,
}

impl Engine {
    fn advance(&mut self, bytes: &[u8]) {
        if let Some(on) = self.scan.scan(bytes) {
            self.win32_input = on;
        }
        self.parser.advance(&mut self.term, bytes);
    }
}

enum Msg {
    Data(Vec<u8>),
    Resize(usize, usize),
    Search(String),
    SearchNext(bool),
}

struct Shared {
    engine: FairMutex<Engine>,
    outbox: Arc<Mutex<Outbox>>,
    /// Messages sent to the thread and not yet finished.
    in_flight: AtomicUsize,
    /// Output bytes queued for the thread and not yet parsed.
    backlog: AtomicUsize,
    /// Something changed since the last snapshot (wake already sent).
    dirty: AtomicBool,
    wake: Box<dyn Fn() + Send + Sync>,
}

impl Shared {
    fn changed(&self) {
        if !self.dirty.swap(true, Ordering::AcqRel) {
            (self.wake)();
        }
    }
}

/// One terminal emulator. Cheap to keep around hidden: it parses but nobody draws.
pub struct Terminal {
    shared: Arc<Shared>,
    tx: Option<Sender<Msg>>,
    thread: Option<std::thread::JoinHandle<()>>,
    size: (usize, usize),
}

impl std::fmt::Debug for Terminal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Terminal").field("size", &self.size).finish_non_exhaustive()
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        self.tx = None;
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Terminal {
    /// A terminal of `cols` × `rows` cells; `wake` is called (from any thread) when there is
    /// something new to draw, at most once per snapshot.
    pub fn new(cols: usize, rows: usize, wake: impl Fn() + Send + Sync + 'static) -> Self {
        let (cols, rows) = (cols.max(2), rows.max(1));
        let outbox = Arc::new(Mutex::new(Outbox { size: (cols as u16, rows as u16), cell: (8, 16), ..Outbox::default() }));
        // OSC 52: a program may write the clipboard, never read it (P339).
        let config = Config { scrolling_history: SCROLLBACK, kitty_keyboard: false, osc52: alacritty_terminal::term::Osc52::OnlyCopy, ..Config::default() };
        let term = Term::new(config, &Size(cols, rows), Listener(outbox.clone()));
        let engine = Engine {
            term,
            parser: Processor::new(),
            scan: ModeScan::default(),
            win32_input: false,
            search: None,
            view_dirty: true,
            generation: 0,
        };
        let shared = Arc::new(Shared {
            engine: FairMutex::new(engine),
            outbox,
            in_flight: AtomicUsize::new(0),
            backlog: AtomicUsize::new(0),
            dirty: AtomicBool::new(false),
            wake: Box::new(wake),
        });
        let (tx, rx) = channel();
        let worker = shared.clone();
        let thread = std::thread::Builder::new()
            .name("yaac-term".into())
            .spawn(move || run(&worker, &rx))
            .ok();
        Self { shared, tx: Some(tx), thread, size: (cols, rows) }
    }

    fn send(&self, msg: Msg) {
        self.shared.in_flight.fetch_add(1, Ordering::AcqRel);
        if self.tx.as_ref().is_none_or(|tx| tx.send(msg).is_err()) {
            self.shared.in_flight.fetch_sub(1, Ordering::AcqRel);
        }
    }

    /// Output from the target.
    pub fn feed(&self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        if bytes.len() <= INLINE_BYTES && self.shared.in_flight.load(Ordering::Acquire) == 0 {
            self.shared.engine.lock().advance(bytes);
            self.shared.changed();
        } else {
            self.shared.backlog.fetch_add(bytes.len(), Ordering::AcqRel);
            self.send(Msg::Data(bytes.to_vec()));
        }
    }

    /// Output bytes waiting to be parsed: callers stop granting the target credit while this
    /// is large, so a flood is held by the link, not by memory.
    pub fn backlog(&self) -> usize {
        self.shared.backlog.load(Ordering::Acquire)
    }

    /// The program's title (OSC 0/2), if it set one.
    pub fn title(&self) -> Option<String> {
        self.shared.outbox.lock().ok().and_then(|o| o.title.clone())
    }

    /// Whether everything fed so far has been parsed.
    pub fn idle(&self) -> bool {
        self.shared.in_flight.load(Ordering::Acquire) == 0
    }

    /// Resize the grid (reflowing the scrollback); done on the terminal's thread, in order with
    /// output. The returned size is what the target should be told.
    pub fn resize(&mut self, cols: usize, rows: usize) -> (usize, usize) {
        let size = (cols.max(2), rows.max(1));
        if size != self.size {
            self.size = size;
            if let Ok(mut out) = self.shared.outbox.lock() {
                out.size = (size.0 as u16, size.1 as u16);
            }
            self.send(Msg::Resize(size.0, size.1));
        }
        size
    }

    /// The size last asked for.
    pub fn size(&self) -> (usize, usize) {
        self.size
    }

    /// The theme colour queries (OSC 10/11/4) are answered from, and the cell size in pixels
    /// for size reports.
    pub fn set_theme(&self, theme: Theme, cell_px: (u16, u16)) {
        if let Ok(mut out) = self.shared.outbox.lock() {
            out.theme = Some(theme);
            out.cell = cell_px;
        }
    }

    /// Replies and notifications since the last call.
    pub fn take_events(&self) -> Events {
        self.shared.outbox.lock().map(|mut o| std::mem::take(&mut o.events)).unwrap_or_default()
    }

    /// The live modes (the ones in the last frame may be one frame old).
    pub fn modes(&self) -> Modes {
        let e = self.shared.engine.lock();
        modes(&e)
    }

    /// Bring `frame` up to date. Returns whether anything changed since the last call.
    pub fn snapshot(&self, frame: &mut Frame) -> bool {
        self.shared.dirty.store(false, Ordering::Release);
        let title = self.shared.outbox.lock().ok().and_then(|o| o.title.clone());
        let mut e = self.shared.engine.lock();
        snapshot(&mut e, frame, title)
    }

    // ---- input ------------------------------------------------------------------------------

    fn follow(&self) {
        let mut e = self.shared.engine.lock();
        if e.term.grid().display_offset() != 0 {
            e.term.scroll_display(Scroll::Bottom);
            e.view_dirty = true;
            drop(e);
            self.shared.changed();
        }
    }

    /// The bytes for a key press; typing returns the view to the bottom.
    pub fn key(&self, key: Key, mods: Mods) -> Option<Vec<u8>> {
        let bytes = encode_key(key, mods, &self.modes())?;
        self.follow();
        Some(bytes)
    }

    /// Typed text.
    pub fn text(&self, text: &str) -> Vec<u8> {
        self.follow();
        text.as_bytes().to_vec()
    }

    /// A paste. When [`Paste::confirm`] is set the caller asks the user before sending.
    pub fn paste(&self, text: &str) -> Paste {
        self.follow();
        encode_paste(text, &self.modes())
    }

    /// A mouse report, if the program wants this event.
    pub fn mouse(&self, button: MouseButton, action: MouseAction, row: usize, col: usize, mods: Mods) -> Option<Vec<u8>> {
        encode_mouse(button, action, row, col, mods, &self.modes())
    }

    /// The wheel moved `lines` (positive = up): reported to a program that wants the mouse,
    /// arrow keys to a full-screen program with alternate scroll (less, man), otherwise the
    /// view scrolls through the scrollback.
    pub fn wheel(&self, lines: i32, row: usize, col: usize, mods: Mods) -> Option<Vec<u8>> {
        if lines == 0 {
            return None;
        }
        let m = self.modes();
        if m.mouse() && !mods.shift {
            let button = if lines > 0 { MouseButton::WheelUp } else { MouseButton::WheelDown };
            let one = encode_mouse(button, MouseAction::Press, row, col, mods, &m)?;
            return Some(one.repeat(lines.unsigned_abs() as usize));
        }
        if m.alt_screen && m.alternate_scroll && !mods.shift {
            let key = if lines > 0 { Key::Up } else { Key::Down };
            let one = encode_key(key, Mods::NONE, &m)?;
            return Some(one.repeat(lines.unsigned_abs() as usize));
        }
        self.scroll(lines);
        None
    }

    /// Focus gained or lost: the report, if the program asked for one.
    pub fn focus(&self, focused: bool) -> Option<Vec<u8>> {
        let mut e = self.shared.engine.lock();
        e.term.is_focused = focused;
        e.view_dirty = true;
        let m = modes(&e);
        drop(e);
        self.shared.changed();
        encode_focus(focused, &m).map(<[u8]>::to_vec)
    }

    // ---- view -------------------------------------------------------------------------------

    /// Scroll the view `lines` into the scrollback (positive = up).
    pub fn scroll(&self, lines: i32) {
        let mut e = self.shared.engine.lock();
        e.term.scroll_display(Scroll::Delta(lines));
        e.view_dirty = true;
        drop(e);
        self.shared.changed();
    }

    /// Page up or down through the scrollback.
    pub fn scroll_page(&self, up: bool) {
        let mut e = self.shared.engine.lock();
        e.term.scroll_display(if up { Scroll::PageUp } else { Scroll::PageDown });
        e.view_dirty = true;
        drop(e);
        self.shared.changed();
    }

    /// Top or bottom of the scrollback.
    pub fn scroll_to(&self, top: bool) {
        let mut e = self.shared.engine.lock();
        e.term.scroll_display(if top { Scroll::Top } else { Scroll::Bottom });
        e.view_dirty = true;
        drop(e);
        self.shared.changed();
    }

    /// **Clear**: drop the scrollback and everything above the cursor's line, which moves to
    /// the top (the shell's prompt stays). A full-screen program only loses the scrollback.
    pub fn clear(&self) {
        let mut e = self.shared.engine.lock();
        let t = &mut e.term;
        t.selection = None;
        if !t.mode().contains(TermMode::ALT_SCREEN) {
            let cursor = t.grid().cursor.point;
            let line = usize::try_from(cursor.line.0).unwrap_or(0);
            if line > 0 {
                t.scroll_up(line);
                t.goto(0, cursor.column.0);
            }
        }
        t.scroll_display(Scroll::Bottom);
        t.clear_screen(ansi::ClearMode::Saved);
        e.view_dirty = true;
        drop(e);
        self.shared.changed();
    }

    /// After **Restart**: leave whatever mode the old shell left (alternate screen, mouse,
    /// colours) without losing the output above.
    pub fn reset_modes(&self) {
        self.feed(b"\x1b[?1049l\x1b[!p\x1b[0m\x1b[?25h\r\n");
        let mut e = self.shared.engine.lock();
        e.win32_input = false;
    }

    // ---- selection --------------------------------------------------------------------------

    fn point(e: &Engine, row: usize, col: usize) -> Point {
        let offset = e.term.grid().display_offset() as i32;
        let cols = e.term.columns();
        let rows = e.term.screen_lines();
        Point::new(Line(row.min(rows.saturating_sub(1)) as i32 - offset), Column(col.min(cols.saturating_sub(1))))
    }

    /// Start a selection at a viewport cell.
    pub fn select_start(&self, row: usize, col: usize, side: CellSide, kind: SelectKind) {
        let mut e = self.shared.engine.lock();
        let p = Self::point(&e, row, col);
        let ty = match kind {
            SelectKind::Simple => SelectionType::Simple,
            SelectKind::Word => SelectionType::Semantic,
            SelectKind::Line => SelectionType::Lines,
            SelectKind::Block => SelectionType::Block,
        };
        e.term.selection = Some(Selection::new(ty, p, side_of(side)));
        e.view_dirty = true;
        drop(e);
        self.shared.changed();
    }

    /// Extend the selection to a viewport cell (drag).
    pub fn select_to(&self, row: usize, col: usize, side: CellSide) {
        let mut e = self.shared.engine.lock();
        let p = Self::point(&e, row, col);
        if let Some(s) = e.term.selection.as_mut() {
            s.update(p, side_of(side));
            e.view_dirty = true;
            drop(e);
            self.shared.changed();
        }
    }

    pub fn select_all(&self) {
        let mut e = self.shared.engine.lock();
        let mut s = Selection::new(SelectionType::Lines, Point::new(e.term.topmost_line(), Column(0)), Side::Left);
        s.update(Point::new(e.term.bottommost_line(), Column(0)), Side::Right);
        e.term.selection = Some(s);
        e.view_dirty = true;
        drop(e);
        self.shared.changed();
    }

    pub fn clear_selection(&self) {
        let mut e = self.shared.engine.lock();
        if e.term.selection.take().is_some() {
            e.view_dirty = true;
            drop(e);
            self.shared.changed();
        }
    }

    /// The selected text (scrollback included), clean (P339): soft-wrapped rows joined,
    /// trailing spaces trimmed, wide characters once, and no final newline (a copied line
    /// pasted into a shell does not run).
    pub fn selection_text(&self) -> Option<String> {
        let e = self.shared.engine.lock();
        let s = e.term.selection_to_string()?;
        let s = s.strip_suffix('\n').unwrap_or(&s);
        let s = s.strip_suffix('\r').unwrap_or(s);
        (!s.is_empty()).then(|| s.to_owned())
    }

    // ---- search -----------------------------------------------------------------------------

    /// Search for plain text (case-insensitive unless it has a capital), from the bottom
    /// upwards. Empty clears the search.
    pub fn search(&self, query: &str) {
        self.send(Msg::Search(query.to_owned()));
    }

    /// The next (older, `up`) or previous match.
    pub fn search_next(&self, up: bool) {
        self.send(Msg::SearchNext(up));
    }

    pub fn search_close(&self) {
        self.send(Msg::Search(String::new()));
    }
}

fn side_of(side: CellSide) -> Side {
    match side {
        CellSide::Left => Side::Left,
        CellSide::Right => Side::Right,
    }
}

/// The terminal's thread: output, resizes and searches, in order.
fn run(shared: &Shared, rx: &Receiver<Msg>) {
    loop {
        let deadline = shared.engine.lock_unfair().parser.sync_timeout().sync_timeout();
        let msg = match deadline {
            Some(d) => match rx.recv_timeout(d.saturating_duration_since(Instant::now())) {
                Ok(m) => m,
                Err(RecvTimeoutError::Timeout) => {
                    let mut e = shared.engine.lock_unfair();
                    let Engine { parser, term, .. } = &mut *e;
                    parser.stop_sync(term);
                    drop(e);
                    shared.changed();
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => return,
            },
            None => match rx.recv() {
                Ok(m) => m,
                Err(_) => return,
            },
        };
        match msg {
            Msg::Data(bytes) => {
                for slice in bytes.chunks(SLICE_BYTES) {
                    shared.engine.lock_unfair().advance(slice);
                    shared.backlog.fetch_sub(slice.len(), Ordering::AcqRel);
                    shared.changed();
                }
            }
            Msg::Resize(cols, rows) => {
                let mut e = shared.engine.lock_unfair();
                e.term.resize(Size(cols, rows));
                e.view_dirty = true;
            }
            Msg::Search(query) => {
                let mut e = shared.engine.lock_unfair();
                if query.is_empty() {
                    e.search = None;
                } else {
                    let regex = RegexSearch::new(&escape(&query)).ok();
                    e.search = Some(SearchState { query, regex, focused: None, found: false });
                    find(&mut e, true, true);
                }
                e.view_dirty = true;
            }
            Msg::SearchNext(up) => {
                let mut e = shared.engine.lock_unfair();
                find(&mut e, up, false);
                e.view_dirty = true;
            }
        }
        shared.in_flight.fetch_sub(1, Ordering::AcqRel);
        shared.changed();
    }
}

/// Move the search to the next match up or down (from the cursor for a new search).
fn find(e: &mut Engine, up: bool, fresh: bool) {
    let Engine { term, search, .. } = e;
    let Some(s) = search.as_mut() else { return };
    let Some(regex) = s.regex.as_mut() else { return };
    let dir = if up { Direction::Left } else { Direction::Right };
    let origin = match (&s.focused, fresh) {
        (Some(m), false) => {
            if up {
                *m.start()
            } else {
                *m.end()
            }
        }
        _ => {
            let c = term.grid().cursor.point;
            Point::new(c.line, Column(term.columns() - 1))
        }
    };
    // Step off the current match so "next" never finds it again.
    let origin = if fresh {
        origin
    } else if up {
        origin.sub(term, alacritty_terminal::index::Boundary::None, 1)
    } else {
        origin.add(term, alacritty_terminal::index::Boundary::None, 1)
    };
    let found = term.search_next(regex, origin, dir, Side::Left, None);
    s.found = found.is_some() || (s.found && !fresh);
    if let Some(m) = found {
        term.scroll_to_point(*m.start());
        s.focused = Some(m);
    } else if fresh {
        s.focused = None;
    }
}

/// Plain text as a regex.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for c in s.chars() {
        if "\\.+*?()|[]{}^$#&-~".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn modes(e: &Engine) -> Modes {
    let m = *e.term.mode();
    Modes {
        app_cursor: m.contains(TermMode::APP_CURSOR),
        app_keypad: m.contains(TermMode::APP_KEYPAD),
        bracketed_paste: m.contains(TermMode::BRACKETED_PASTE),
        alt_screen: m.contains(TermMode::ALT_SCREEN),
        alternate_scroll: m.contains(TermMode::ALTERNATE_SCROLL),
        mouse_click: m.contains(TermMode::MOUSE_REPORT_CLICK),
        mouse_drag: m.contains(TermMode::MOUSE_DRAG),
        mouse_motion: m.contains(TermMode::MOUSE_MOTION),
        sgr_mouse: m.contains(TermMode::SGR_MOUSE),
        utf8_mouse: m.contains(TermMode::UTF8_MOUSE),
        focus_events: m.contains(TermMode::FOCUS_IN_OUT),
        win32_input: e.win32_input,
        line_feed_new_line: m.contains(TermMode::LINE_FEED_NEW_LINE),
    }
}

fn color(c: AColor, colors: &Colors, dim: &mut bool) -> Color {
    let over = |i: usize| colors[i].map(|r| Color::Rgb(r.r, r.g, r.b));
    match c {
        AColor::Spec(r) => Color::Rgb(r.r, r.g, r.b),
        AColor::Indexed(i) => over(usize::from(i)).unwrap_or(Color::Index(i)),
        AColor::Named(n) => {
            let i = n as usize;
            match n {
                _ if i < 16 => over(i).unwrap_or(Color::Index(i as u8)),
                NamedColor::Foreground | NamedColor::BrightForeground | NamedColor::Cursor => over(256).unwrap_or(Color::Fg),
                NamedColor::DimForeground => {
                    *dim = true;
                    over(256).unwrap_or(Color::Fg)
                }
                NamedColor::Background => over(257).unwrap_or(Color::Bg),
                _ => {
                    // DimBlack..DimWhite follow Cursor in order.
                    *dim = true;
                    let base = i.saturating_sub(NamedColor::DimBlack as usize).min(7);
                    over(base).unwrap_or(Color::Index(base as u8))
                }
            }
        }
    }
}

fn build_row(term: &Term<Listener>, line: Line, out: &mut Row) {
    let cols = term.columns();
    let colors = term.colors();
    let row = &term.grid()[line];
    out.cells.clear();
    out.combining.clear();
    out.links.clear();
    out.wrapped = false;
    for c in 0..cols {
        let cell = &row[Column(c)];
        let f = cell.flags;
        let mut dim = f.contains(AFlags::DIM);
        let mut fg = color(cell.fg, colors, &mut dim);
        let mut bg = color(cell.bg, colors, &mut false);
        if f.contains(AFlags::INVERSE) {
            std::mem::swap(&mut fg, &mut bg);
        }
        let mut bits = 0u16;
        for (a, b) in [
            (AFlags::BOLD, Flags::BOLD),
            (AFlags::ITALIC, Flags::ITALIC),
            (AFlags::UNDERLINE, Flags::UNDERLINE),
            (AFlags::DOUBLE_UNDERLINE, Flags::DOUBLE_UNDERLINE),
            (AFlags::STRIKEOUT, Flags::STRIKE),
            (AFlags::WIDE_CHAR, Flags::WIDE),
            (AFlags::WIDE_CHAR_SPACER, Flags::SPACER),
        ] {
            if f.contains(a) {
                bits |= b;
            }
        }
        if f.intersects(AFlags::UNDERCURL | AFlags::DOTTED_UNDERLINE | AFlags::DASHED_UNDERLINE) {
            bits |= Flags::FANCY_UNDERLINE;
        }
        if dim {
            bits |= Flags::DIM;
        }
        let mut ch = cell.c;
        if f.intersects(AFlags::HIDDEN | AFlags::LEADING_WIDE_CHAR_SPACER) || ch == '\0' || ch == '\t' {
            ch = ' ';
        }
        if cell.extra.is_some() {
            if let Some(z) = cell.zerowidth().filter(|z| !z.is_empty()) {
                out.combining.push((c, z.iter().collect::<String>().into_boxed_str()));
            }
            if let Some(link) = cell.hyperlink() {
                bits |= Flags::LINK;
                match out.links.last_mut() {
                    Some(l) if l.end + 1 == c && *l.uri == *link.uri() => l.end = c,
                    _ => out.links.push(LinkSpan { start: c, end: c, uri: Arc::from(link.uri()) }),
                }
            }
        }
        out.cells.push(Cell { ch, fg, bg, flags: Flags(bits) });
    }
    out.wrapped = row[Column(cols - 1)].flags.contains(AFlags::WRAPLINE);
}

fn span(m: &Match, offset: i32, rows: usize, cols: usize, block: bool) -> Option<Span> {
    let (s, e) = (m.start(), m.end());
    let (top, bottom) = (s.line.0 + offset, e.line.0 + offset);
    if bottom < 0 || top >= rows as i32 {
        return None;
    }
    let start = if top < 0 { (0, if block { s.column.0 } else { 0 }) } else { (top as usize, s.column.0) };
    let end = if bottom >= rows as i32 { (rows - 1, if block { e.column.0 } else { cols - 1 }) } else { (bottom as usize, e.column.0) };
    Some(Span { start, end, block })
}

fn snapshot(e: &mut Engine, frame: &mut Frame, title: Option<String>) -> bool {
    let (cols, rows) = (e.term.columns(), e.term.screen_lines());
    let resized = frame.cols != cols || frame.rows != rows || frame.lines.len() != rows;
    let mut candidates: Vec<usize> = match e.term.damage() {
        TermDamage::Full => (0..rows).collect(),
        TermDamage::Partial(it) => it.map(|d| d.line).filter(|l| *l < rows).collect(),
    };
    e.term.reset_damage();
    if resized {
        frame.cols = cols;
        frame.rows = rows;
        frame.lines = vec![Row::default(); rows];
        candidates = (0..rows).collect();
    }
    frame.full = resized;
    frame.damaged.clear();
    let offset = e.term.grid().display_offset();
    let mut scratch = Row::default();
    for r in candidates {
        build_row(&e.term, Line(r as i32 - offset as i32), &mut scratch);
        if scratch != frame.lines[r] {
            std::mem::swap(&mut scratch, &mut frame.lines[r]);
            frame.damaged.push(r);
        }
    }
    let mut changed = !frame.damaged.is_empty() || resized;

    let rc = e.term.renderable_content();
    let row = rc.cursor.point.line.0 + offset as i32;
    let cursor = Cursor {
        row: row.clamp(0, rows as i32 - 1) as usize,
        col: rc.cursor.point.column.0.min(cols - 1),
        shape: match rc.cursor.shape {
            AShape::Beam => CursorShape::Bar,
            AShape::Underline => CursorShape::Underline,
            AShape::HollowBlock => CursorShape::HollowBlock,
            AShape::Block | AShape::Hidden => CursorShape::Block,
        },
        visible: rc.cursor.shape != AShape::Hidden && (0..rows as i32).contains(&row),
        wide: e.term.grid()[rc.cursor.point].flags.contains(AFlags::WIDE_CHAR),
        blinking: e.term.cursor_style().blinking,
    };
    let selection = rc.selection.and_then(|s| span(&(s.start..=s.end), offset as i32, rows, cols, s.is_block));
    let m = modes(e);
    let history = e.term.grid().history_size();
    let mut view = SearchView::default();
    let search = if let Some(s) = e.search.as_mut() {
        view.query.clone_from(&s.query);
        view.invalid = s.regex.is_none();
        view.found = s.found;
        if let Some(regex) = s.regex.as_mut() {
            let start = Point::new(Line(-(offset as i32)), Column(0));
            let end = Point::new(Line(rows as i32 - 1 - offset as i32), Column(cols - 1));
            for m in RegexIter::new(start, end, Direction::Right, &e.term, regex).take(1000) {
                if let Some(sp) = span(&m, offset as i32, rows, cols, false) {
                    view.matches.push(sp);
                }
            }
            if !view.matches.is_empty() {
                s.found = true;
                view.found = true;
            }
        }
        view.focused = s.focused.as_ref().and_then(|m| span(m, offset as i32, rows, cols, false));
        Some(view)
    } else {
        None
    };
    changed |= frame.cursor != cursor
        || frame.selection != selection
        || frame.modes != m
        || frame.display_offset != offset
        || frame.history != history
        || frame.search != search
        || frame.title != title
        || e.view_dirty;
    frame.cursor = cursor;
    frame.selection = selection;
    frame.modes = m;
    frame.display_offset = offset;
    frame.history = history;
    frame.search = search;
    frame.title = title;
    e.view_dirty = false;
    if changed {
        e.generation += 1;
    }
    frame.generation = e.generation;
    changed
}

/// Watches the output for ConPTY's win32-input-mode switch (`CSI ? 9001 h` / `l`), which the
/// parser itself does not know.
#[derive(Default)]
struct ModeScan {
    state: u8,
    params: Vec<u8>,
}

impl ModeScan {
    fn scan(&mut self, bytes: &[u8]) -> Option<bool> {
        let mut result = None;
        let mut i = 0;
        while i < bytes.len() {
            if self.state == 0 {
                match bytes[i..].iter().position(|b| *b == 0x1b) {
                    Some(p) => i += p,
                    None => break,
                }
            }
            let b = bytes[i];
            i += 1;
            self.state = match (self.state, b) {
                (_, 0x1b) => 1,
                (1, b'[') => 2,
                (2, b'?') => {
                    self.params.clear();
                    3
                }
                (3, b'0'..=b'9' | b';') if self.params.len() < 32 => {
                    self.params.push(b);
                    3
                }
                (3, b'h' | b'l') => {
                    if self.params.split(|c| *c == b';').any(|p| p == b"9001") {
                        result = Some(b == b'h');
                    }
                    0
                }
                _ => 0,
            };
        }
        result
    }
}

#[cfg(test)]
mod tests;
