//! The pipe retains at most one unacknowledged frame. All output received meanwhile
//! stays in the shared emulator. Its next snapshot includes the union of the damage.
use std::io::{self, Write};
use std::time::{Duration, Instant};

use serde::Serialize;
use yaac_term::{Flags, Frame, Rgb, Span, Theme};

mod input;
pub(super) mod tabs;
mod wire;
use wire::{Run, Runs, Style};

#[derive(Default)]
pub(super) struct TerminalPipe {
    tab: Option<u64>,
    panel: bool,
    dark: bool,
    cell_size: (u16, u16),
    copied: Option<String>,
    paste_token: u64,
    paste_prompt: Option<serde_json::Value>,
    read_clipboard: bool,
    serial: u64,
    outstanding: Option<u64>,
    next_frame: Option<Instant>,
    force_full: bool,
    generation: Option<u64>,
    theme: Option<Theme>,
    pointer: Option<(usize, usize)>,
    pointer_changed: bool,
    highlights: Vec<(Span, Highlight)>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Highlight {
    Link,
    Selection,
    Match,
    Focused,
}

fn packed(Rgb(r, g, b): Rgb) -> u32 {
    u32::from_be_bytes([0, r, g, b])
}

impl TerminalPipe {
    pub(super) fn surface(&mut self, panel: bool) {
        self.panel = panel;
    }

    pub(super) fn select(&mut self, tab: Option<u64>) {
        if self.tab != tab {
            self.tab = tab;
            self.next_frame = None;
            self.outstanding = None;
            self.read_clipboard = false;
            self.force_full = true;
            self.generation = None;
            self.pointer = None;
            self.highlights.clear();
        }
    }

    pub(super) fn update(
        &mut self,
        tab: Option<(u64, &mut crate::terminal::Session)>,
        theme: Option<Theme>,
        out: &mut impl Write,
    ) -> io::Result<Option<Duration>> {
        self.select(tab.as_ref().map(|(id, _)| *id));
        if let Some(text) = self.copied.take() {
            super::write_frame(out, &serde_json::json!({ "terminalCopy": text }))?;
        }
        if std::mem::take(&mut self.read_clipboard)
            && let Some(tab) = self.tab
        {
            super::write_frame(
                out,
                &serde_json::json!({"terminalReadPaste":{"tab":tab,"panel":self.panel}}),
            )?;
        }
        if let Some(prompt) = self.paste_prompt.take() {
            super::write_frame(out, &serde_json::json!({ "terminalPasteConfirm":prompt }))?;
        }
        let Some((_, session)) = tab.filter(|_| self.ready()) else {
            return Ok(None);
        };
        // Software composition and remote desktops can raise Rendering much faster
        // than scan-out. Leave subsequent damage in the grid until the next slot.
        if let Some(wait) = self
            .next_frame
            .and_then(|at| at.checked_duration_since(Instant::now()))
        {
            return Ok(Some(wait));
        }
        let theme = theme.unwrap_or_else(|| {
            if self.cell_size.0 == 0 || self.dark { Theme::DARK } else { Theme::LIGHT }
        });
        let pixels = if self.cell_size.0 > 0 {
            self.cell_size
        } else {
            (8, 16)
        };
        session.term.set_theme(theme, pixels);
        session.term.snapshot(&mut session.frame);
        self.write(&session.frame, theme, out)?;
        if self.outstanding.is_some() {
            self.next_frame = Some(Instant::now() + Duration::from_millis(16));
        }
        Ok(None)
    }

    pub(super) fn viewport(&mut self, data: &serde_json::Value) -> Option<(u16, u16)> {
        let number = |key: &str| u16::try_from(data[key].as_u64()?).ok();
        self.dark = data["dark"] == true;
        self.cell_size = (number("cellWidth")?.max(1), number("cellHeight")?.max(1));
        Some((
            number("cols")?.clamp(2, 1000),
            number("rows")?.clamp(1, 500),
        ))
    }

    pub(super) fn input(
        &mut self,
        session: &crate::terminal::Session,
        name: &str,
        data: &serde_json::Value,
    ) -> Option<Vec<u8>> {
        match name {
            "terminal.hover" => {
                let at = data["row"]
                    .as_u64()
                    .zip(data["col"].as_u64())
                    .map(|(r, c)| (r as usize, c as usize));
                self.hover(at);
                None
            }
            "terminal.searchSelection" => {
                if let Some(text) = session.term.selection_text()
                    && let Some(query) = text.lines().next()
                {
                    session.term.search(query);
                }
                None
            }
            "terminal.copy" => {
                self.copied = session.term.selection_text();
                None
            }
            "terminal.refresh" => {
                self.force_full = true;
                None
            }
            _ => input::handle(&session.term, &session.frame, name, data),
        }
    }

    fn hover(&mut self, cell: Option<(usize, usize)>) {
        self.pointer_changed |= self.pointer != cell;
        self.pointer = cell;
    }

    pub(super) fn ready(&self) -> bool {
        self.tab.is_some() && self.outstanding.is_none()
    }

    pub(super) fn ack(&mut self, serial: Option<u64>) -> bool {
        if serial.is_some() && serial == self.outstanding {
            self.outstanding = None;
            true
        } else {
            false
        }
    }

    pub(super) fn write(
        &mut self,
        frame: &Frame,
        theme: Theme,
        out: &mut impl Write,
    ) -> io::Result<()> {
        let Some(tab) = self.tab.filter(|_| self.ready()) else {
            return Ok(());
        };
        let full = self.force_full || self.theme != Some(theme);
        if !full
            && !frame.full
            && !self.pointer_changed
            && self.generation == Some(frame.generation)
        {
            return Ok(());
        }
        self.generation = Some(frame.generation);
        self.theme = Some(theme);
        self.pointer_changed = false;
        self.force_full = false;
        let mut dirty = vec![full || frame.full; frame.rows];
        for &row in &frame.damaged {
            if let Some(flag) = dirty.get_mut(row) {
                *flag = true;
            }
        }

        let mut spans = Vec::new();
        if let Some(link) = self.pointer.and_then(|(r, c)| frame.link_at(r, c)) {
            spans.extend(link.cells.into_iter().map(|(r, a, b)| {
                (
                    Span {
                        start: (r, a),
                        end: (r, b),
                        block: false,
                    },
                    Highlight::Link,
                )
            }));
        }
        if let Some(search) = &frame.search {
            spans.extend(search.matches.iter().map(|span| (*span, Highlight::Match)));
            spans.extend(search.focused.map(|span| (span, Highlight::Focused)));
        }
        spans.extend(frame.selection.map(|span| (span, Highlight::Selection)));
        if spans != self.highlights {
            for (span, _) in spans.iter().chain(&self.highlights) {
                let end = span.end.0.saturating_add(1).min(dirty.len());
                if let Some(rows) = dirty.get_mut(span.start.0..end) {
                    rows.fill(true);
                }
            }
            self.highlights = spans;
        }
        let rows: Vec<_> = frame
            .lines
            .iter()
            .zip(dirty)
            .enumerate()
            .filter(|(_, (_, changed))| *changed)
            .map(|(row, (line, _))| (row, encode_row(row, line, theme, &self.highlights)))
            .collect();
        let c = frame.cursor;
        use yaac_term::CursorShape;
        let shape = match (c.visible, c.shape) {
            (false, _) => "hidden",
            (_, CursorShape::Block) => "block",
            (_, CursorShape::HollowBlock) => "hollow",
            (_, CursorShape::Underline) => "underline",
            (_, CursorShape::Bar) => "bar",
        };
        self.serial += 1;
        #[derive(Serialize)]
        struct Message<'a> {
            search: Option<&'a str>,
            matches: usize,
            found: bool,
            panel: bool,
            tab: u64,
            frame: u64,
            height: usize,
            rows: Vec<(usize, Vec<Run>)>,
            cursor: (usize, usize, &'static str, u32, bool, bool),
            glyph: Option<(u32, u8, String)>,
            mouse: bool,
            selection: bool,
        }
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Envelope<'a> {
            terminal_frame: Message<'a>,
        }
        let message = Message {
            glyph: frame.lines.get(c.row).and_then(|line| {
                let cell = line.cells.get(c.col)?;
                if !c.visible || cell.blank() {
                    return None;
                }
                let mut text = cell.ch.to_string();
                let mut flags = (cell.flags.0 & (Flags::BOLD | Flags::ITALIC)) as u8;
                if let Some((_, extra)) = line.combining.iter().find(|(col, _)| *col == c.col) {
                    text.push_str(extra);
                    flags |= 16;
                }
                Some((packed(theme.cursor_text), flags, text))
            }),
            panel: self.panel,
            matches: frame.search.as_ref().map_or(0, |s| s.matches.len()),
            found: frame.search.as_ref().is_some_and(|s| s.found),
            search: frame.search.as_ref().map(|s| s.query.as_str()),
            tab,
            frame: self.serial,
            height: frame.rows,
            rows,
            cursor: (
                c.col,
                c.row,
                shape,
                packed(theme.cursor),
                c.wide,
                c.blinking,
            ),
            mouse: frame.modes.mouse(),
            selection: frame.selection.is_some(),
        };
        serde_json::to_writer(
            &mut *out,
            &Envelope {
                terminal_frame: message,
            },
        )?;
        writeln!(out)?;
        out.flush()?;
        self.outstanding = Some(self.serial);
        Ok(())
    }
}

fn encode_row(
    row: usize,
    line: &yaac_term::Row,
    theme: Theme,
    spans: &[(Span, Highlight)],
) -> Vec<Run> {
    let mut overlay = Vec::new();
    for (span, kind) in spans {
        if let Some((a, b)) = span.columns(row, line.cells.len()) {
            overlay.resize(line.cells.len(), None);
            overlay[a..=b].fill(Some(*kind));
        }
    }
    let mut runs = Runs::default();
    let mut combining = line.combining.iter().peekable();
    // Omit the untouched tail. Interior spaces stay in their style run (one GlyphRun
    // for a line of prose, irrespective of how many words it contains).
    let end = line
        .cells
        .iter()
        .rposition(|cell| {
            !cell.blank() || theme.rgb(cell.bg) != theme.bg || cell.flags.has(Flags::LINK)
        })
        .map_or(0, |i| i + 1)
        .max(
            overlay
                .iter()
                .rposition(Option::is_some)
                .map_or(0, |i| i + 1),
        )
        .max(line.combining.last().map_or(0, |(i, _)| i + 1));
    for (col, cell) in line.cells.iter().take(end).enumerate() {
        if cell.flags.has(Flags::SPACER) {
            continue;
        }
        let mut ink = theme.fg(cell.fg, cell.bg, cell.flags);
        let mut paper = theme.rgb(cell.bg);
        match overlay.get(col).copied().flatten() {
            Some(Highlight::Selection) => paper = theme.selection,
            Some(Highlight::Match) => paper = theme.search,
            Some(Highlight::Focused) => {
                ink = theme.search_text;
                paper = theme.search_focused;
            }
            Some(Highlight::Link) | None => {}
        }
        let extra = combining
            .next_if(|(i, _)| *i == col)
            .map_or("", |(_, s)| s.as_ref());
        let flags = u8::from(cell.flags.has(Flags::BOLD))
            | (u8::from(cell.flags.has(Flags::ITALIC)) << 1)
            | (u8::from(
                cell.flags.underlined()
                    || cell.flags.has(Flags::LINK)
                    || overlay.get(col) == Some(&Some(Highlight::Link)),
            ) << 2)
            | (u8::from(cell.flags.has(Flags::STRIKE)) << 3)
            | (u8::from(cell.flags.has(Flags::DOUBLE_UNDERLINE)) << 5);
        runs.push(
            col,
            cell.ch,
            extra,
            Style {
                width: if cell.flags.has(Flags::WIDE) { 2 } else { 1 },
                ink: packed(ink),
                paper: if paper == theme.bg {
                    -1
                } else {
                    packed(paper) as i32
                },
                flags,
            },
        );
    }
    runs.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    #[test]
    fn damage_accumulates_until_the_display_acknowledges_and_switching_restores_the_grid() {
        let term = yaac_term::Terminal::new(10, 3, || {});
        let mut pipe = TerminalPipe::default();
        let mut frame = Frame::default();
        let mut bytes = vec![];
        pipe.select(Some(7));
        term.feed(b"first");
        term.snapshot(&mut frame);
        pipe.write(&frame, Theme::DARK, &mut bytes).unwrap();
        let view =
            |bytes: &[u8]| serde_json::from_slice::<Value>(bytes).unwrap()["terminalFrame"].clone();
        assert_eq!(view(&bytes)["rows"].as_array().unwrap().len(), 3);
        bytes.clear();
        term.feed(b"\r\nsecond");
        assert!(!pipe.ready());
        assert!(!pipe.ack(Some(99)));
        term.feed(b"\r\nthird");
        assert!(pipe.ack(Some(1)));
        term.snapshot(&mut frame);
        pipe.write(&frame, Theme::DARK, &mut bytes).unwrap();
        let indices = |wire: &[u8]| {
            view(wire)["rows"]
                .as_array()
                .unwrap()
                .iter()
                .map(|row| row[0].clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(indices(&bytes), vec![json!(1), json!(2)]);
        bytes.clear();
        pipe.select(None);
        pipe.select(Some(7));
        pipe.write(&frame, Theme::LIGHT, &mut bytes).unwrap();
        assert_eq!(indices(&bytes), vec![json!(0), json!(1), json!(2)]);
        assert!(!pipe.ack(Some(2)));
    }
}
