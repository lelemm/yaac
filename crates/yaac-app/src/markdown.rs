//! Agent prose as markdown: `pulldown-cmark` into a small block model, drawn with egui.
//!
//! Parse once: a completed item keeps its parsed blocks. While an item streams, only a
//! bounded tail is re-parsed — the text before the last blank line outside a code fence is
//! parsed once and kept — and the item gets one exact parse when it completes.

use std::collections::HashMap;

use eframe::egui::{self, Color32, FontId, text::LayoutJob, vec2};
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use yaac_protocol::ItemId;

use crate::seltext::SelCx;
use crate::theme::{self, Palette, size};

/// A run of inline text with its style.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub link: Option<String>,
}

/// A block.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Block {
    Para(Vec<Span>),
    Heading(u8, Vec<Span>),
    Code { lang: String, text: String },
    Item { depth: u8, marker: String, spans: Vec<Span> },
    Quote(Vec<Span>),
    Rule,
    /// A table: the header cells, then each row's cells. Each cell is drawn (and selected)
    /// as its own part, header first, then row by row.
    Table { head: Vec<Vec<Span>>, rows: Vec<Vec<Vec<Span>>> },
}

/// A table row: its cells' spans.
type Row = Vec<Vec<Span>>;

/// Parse markdown into blocks.
pub(crate) fn parse(text: &str) -> Vec<Block> {
    parse_with(text, false)
}

/// Parse markdown the way a person typed it (D157): every single line break is kept as a line
/// break, not joined into the paragraph as markdown does. For the user's own messages.
pub(crate) fn parse_typed(text: &str) -> Vec<Block> {
    parse_with(text, true)
}

fn parse_with(text: &str, typed: bool) -> Vec<Block> {
    let mut out = Vec::new();
    let mut spans: Vec<Span> = Vec::new();
    let (mut bold, mut italic) = (0u32, 0u32);
    let mut link = None;
    let mut code: Option<(String, String)> = None;
    let mut lists: Vec<Option<u64>> = Vec::new();
    let mut item_marker: Option<(u8, String)> = None;
    let mut heading: Option<u8> = None;
    let mut quote = 0u32;
    // A table being read: (header cells, rows, the row being read).
    let mut table: Option<(Row, Vec<Row>, Row)> = None;
    let flush = |spans: &mut Vec<Span>, out: &mut Vec<Block>, heading: Option<u8>, item: &mut Option<(u8, String)>, quote: u32| {
        if spans.is_empty() {
            return;
        }
        let s = std::mem::take(spans);
        if let Some(level) = heading {
            out.push(Block::Heading(level, s));
        } else if let Some((depth, marker)) = item.take() {
            out.push(Block::Item { depth, marker, spans: s });
        } else if quote > 0 {
            out.push(Block::Quote(s));
        } else {
            out.push(Block::Para(s));
        }
    };
    for ev in Parser::new_ext(text, Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES) {
        if let Some((_, buf)) = &mut code {
            match ev {
                Event::Text(t) => buf.push_str(&t),
                Event::End(TagEnd::CodeBlock) => {
                    if let Some((lang, text)) = code.take() {
                        let text = if crate::mermaid::language(&lang) { text } else { text.trim_end_matches('\n').to_owned() };
                        out.push(Block::Code { lang, text });
                    }
                }
                _ => {}
            }
            continue;
        }
        let push = |spans: &mut Vec<Span>, t: &str, c: bool, b: u32, i: u32, l: Option<&str>| {
            spans.push(Span { text: t.to_owned(), bold: b > 0, italic: i > 0, code: c, link: l.map(str::to_owned) });
        };
        match ev {
            Event::Start(Tag::Paragraph) => {}
            Event::End(TagEnd::Paragraph) => flush(&mut spans, &mut out, heading, &mut item_marker, quote),
            Event::Start(Tag::Heading { level, .. }) => heading = Some(level as u8),
            Event::End(TagEnd::Heading(_)) => {
                flush(&mut spans, &mut out, heading, &mut item_marker, quote);
                heading = None;
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                flush(&mut spans, &mut out, heading, &mut item_marker, quote);
                let lang = match kind {
                    CodeBlockKind::Fenced(l) => l.to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                code = Some((lang, String::new()));
            }
            Event::Start(Tag::List(start)) => {
                flush(&mut spans, &mut out, heading, &mut item_marker, quote);
                lists.push(start);
            }
            Event::End(TagEnd::List(_)) => {
                lists.pop();
            }
            Event::Start(Tag::Item) => {
                flush(&mut spans, &mut out, heading, &mut item_marker, quote);
                let depth = u8::try_from(lists.len().saturating_sub(1)).unwrap_or(0);
                let marker = match lists.last_mut() {
                    Some(Some(n)) => {
                        let m = format!("{n}.");
                        *n += 1;
                        m
                    }
                    _ => "•".to_owned(),
                };
                item_marker = Some((depth, marker));
            }
            Event::End(TagEnd::Item) => flush(&mut spans, &mut out, heading, &mut item_marker, quote),
            Event::Start(Tag::BlockQuote(_)) => quote += 1,
            Event::End(TagEnd::BlockQuote(_)) => {
                flush(&mut spans, &mut out, heading, &mut item_marker, quote);
                quote = quote.saturating_sub(1);
            }
            Event::Start(Tag::Strong) => bold += 1,
            Event::End(TagEnd::Strong) => bold = bold.saturating_sub(1),
            Event::Start(Tag::Emphasis) => italic += 1,
            Event::End(TagEnd::Emphasis) => italic = italic.saturating_sub(1),
            Event::Start(Tag::Link { dest_url, .. }) => link = Some(dest_url.into_string()),
            Event::End(TagEnd::Link) => link = None,
            Event::Text(t) => push(&mut spans, &t, false, bold, italic, link.as_deref()),
            Event::Code(t) => push(&mut spans, &t, true, bold, italic, link.as_deref()),
            // HTML is not rendered: it is text the author wrote (`<details>`, `Vec<T>`, a tag a
            // harness left), shown as written rather than dropped.
            Event::InlineHtml(t) => push(&mut spans, &t, false, bold, italic, link.as_deref()),
            Event::Start(Tag::HtmlBlock) => flush(&mut spans, &mut out, heading, &mut item_marker, quote),
            Event::Html(t) => push(&mut spans, &t, false, 0, 0, None),
            Event::End(TagEnd::HtmlBlock) => {
                if let Some(last) = spans.last_mut() {
                    let trimmed = last.text.trim_end_matches('\n').len();
                    last.text.truncate(trimmed);
                }
                flush(&mut spans, &mut out, heading, &mut item_marker, quote);
            }
            // Tables: before, the cells' text ran together into one paragraph.
            Event::Start(Tag::Table(_)) => {
                flush(&mut spans, &mut out, heading, &mut item_marker, quote);
                table = Some((Vec::new(), Vec::new(), Vec::new()));
            }
            Event::Start(Tag::TableCell) => spans.clear(),
            Event::End(TagEnd::TableCell) => {
                if let Some((_, _, row)) = &mut table {
                    row.push(std::mem::take(&mut spans));
                }
            }
            Event::End(TagEnd::TableHead) => {
                if let Some((head, _, row)) = &mut table {
                    *head = std::mem::take(row);
                }
            }
            Event::End(TagEnd::TableRow) => {
                if let Some((_, rows, row)) = &mut table {
                    rows.push(std::mem::take(row));
                }
            }
            Event::End(TagEnd::Table) => {
                if let Some((head, rows, _)) = table.take() {
                    out.push(Block::Table { head, rows });
                }
            }
            Event::SoftBreak => push(&mut spans, if typed { "\n" } else { " " }, false, 0, 0, None),
            Event::HardBreak => push(&mut spans, "\n", false, 0, 0, None),
            Event::Rule => out.push(Block::Rule),
            _ => {}
        }
    }
    if let Some((lang, text)) = code.take() {
        // An unclosed fence while streaming.
        out.push(Block::Code { lang, text });
    }
    flush(&mut spans, &mut out, heading, &mut item_marker, quote);
    out
}

/// Bytes of tail re-parsed while streaming before falling back to plain text.
const MAX_TAIL: usize = 16 * 1024;

/// Parsed form of one streaming or completed item.
#[derive(Clone, Debug, Default)]
pub(crate) struct Parsed {
    len: usize,
    stable_end: usize,
    stable: Vec<Block>,
    tail: Vec<Block>,
    exact: bool,
}

impl Parsed {
    pub(crate) fn blocks(&self) -> impl Iterator<Item = &Block> {
        self.stable.iter().chain(self.tail.iter())
    }

    /// Bring up to date with `text`. Streaming: parse newly stable paragraphs once, re-parse
    /// only the tail. Complete: one exact parse.
    pub(crate) fn update(&mut self, text: &str, complete: bool) {
        if complete {
            if !self.exact || self.len != text.len() {
                *self = Parsed { len: text.len(), stable_end: text.len(), stable: parse(text), tail: Vec::new(), exact: true };
            }
            return;
        }
        if self.len == text.len() && !self.exact {
            return;
        }
        if text.len() < self.stable_end {
            *self = Parsed::default();
        }
        // Advance the stable prefix to the last blank line outside a code fence.
        let mut fence = false;
        let mut split = self.stable_end;
        let mut pos = self.stable_end;
        for line in text[self.stable_end..].split_inclusive('\n') {
            let start = pos;
            pos += line.len();
            if line.trim_start().starts_with("```") {
                fence = !fence;
            }
            if !fence && line.trim().is_empty() && line.ends_with('\n') && pos < text.len() {
                split = pos;
            }
            let _ = start;
        }
        if split > self.stable_end {
            self.stable.extend(parse(&text[self.stable_end..split]));
            self.stable_end = split;
        }
        let tail = &text[self.stable_end..];
        self.tail = if tail.len() > MAX_TAIL {
            vec![Block::Para(vec![Span { text: tail.to_owned(), bold: false, italic: false, code: false, link: None }])]
        } else {
            parse(tail)
        };
        self.len = text.len();
        self.exact = false;
    }
}

/// Parsed markdown per item.
#[derive(Debug, Default)]
pub(crate) struct Cache {
    items: HashMap<ItemId, Parsed>,
}

impl Cache {
    pub(crate) fn get(&mut self, id: &ItemId, text: &str, complete: bool) -> &Parsed {
        let p = self.items.entry(id.clone()).or_default();
        p.update(text, complete);
        p
    }

    /// Text the user typed (D157): parsed once with every line break kept.
    pub(crate) fn get_typed(&mut self, id: &ItemId, text: &str) -> &Parsed {
        let p = self.items.entry(id.clone()).or_default();
        if !p.exact || p.len != text.len() {
            *p = Parsed { len: text.len(), stable_end: text.len(), stable: parse_typed(text), tail: Vec::new(), exact: true };
        }
        p
    }

    pub(crate) fn clear(&mut self) {
        self.items.clear();
    }
}

fn job_for(spans: &[Span], font: &FontId, color: Color32, p: &Palette, wrap: f32) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = wrap;
    for s in spans {
        let font_id = if s.code {
            theme::mono(font.size * 0.9)
        } else if s.bold || s.link.is_some() {
            theme::bold(font.size)
        } else {
            font.clone()
        };
        let mut fmt = egui::TextFormat {
            font_id,
            color,
            italics: s.italic,
            line_height: Some(font.size * 1.45),
            ..Default::default()
        };
        if s.code {
            fmt.background = p.code_bg;
            fmt.expand_bg = 1.0;
        }
        job.append(&s.text, 0.0, fmt);
    }
    job
}

/// Inline markdown in a single string (tool titles, message bodies).
pub(crate) fn inline_job(text: &str, font: &FontId, color: Color32, p: &Palette, wrap: f32) -> LayoutJob {
    let spans: Vec<Span> = parse(text)
        .into_iter()
        .flat_map(|b| match b {
            Block::Para(s) | Block::Heading(_, s) | Block::Quote(s) | Block::Item { spans: s, .. } => s,
            Block::Code { text, .. } => vec![Span { text, bold: false, italic: false, code: true, link: None }],
            Block::Rule => vec![],
            Block::Table { head, rows } => {
                let mut out = Vec::new();
                for cell in head.into_iter().chain(rows.into_iter().flatten()) {
                    if !out.is_empty() {
                        out.push(Span { text: " · ".into(), bold: false, italic: false, code: false, link: None });
                    }
                    out.extend(cell);
                }
                out
            }
        })
        .collect();
    job_for(&spans, font, color, p, wrap)
}

const KEYWORDS: &[&str] = &[
    "let", "mut", "fn", "if", "else", "return", "while", "for", "in", "match", "struct", "enum", "impl", "pub", "use",
    "as", "const", "async", "await", "def", "class", "import", "from", "function", "var", "true", "false", "None",
    "Some", "Ok", "Err", "self", "Self", "loop", "break", "continue", "type", "interface", "export",
];

/// A small, language-agnostic highlighter: comments, strings, numbers, keywords, types.
fn highlight(line: &str, p: &Palette, font: &FontId) -> LayoutJob {
    let (kw, string, comment, num, ty) = if p.dark {
        (Color32::from_rgb(0xC6, 0x8C, 0xFF), Color32::from_rgb(0x9E, 0xCE, 0x6A), Color32::from_rgb(0x7E, 0x9C, 0x6E), Color32::from_rgb(0xF0, 0x9A, 0x5C), Color32::from_rgb(0x6C, 0xC4, 0xFF))
    } else {
        (Color32::from_rgb(0x8A, 0x2B, 0xC2), Color32::from_rgb(0x2E, 0x7D, 0x32), Color32::from_rgb(0x4E, 0x7A, 0x3A), Color32::from_rgb(0xB3, 0x50, 0x00), Color32::from_rgb(0x1A, 0x5F, 0xB4))
    };
    let mut job = LayoutJob::default();
    let fmt = |c: Color32| egui::TextFormat { font_id: font.clone(), color: c, ..Default::default() };
    let b = line.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let rest = &line[i..];
        if rest.starts_with("//") || rest.starts_with('#') {
            job.append(rest, 0.0, fmt(comment));
            break;
        }
        let c = b[i];
        if c == b'"' || c == b'\'' {
            let end = rest[1..].find(c as char).map_or(rest.len(), |e| e + 2);
            job.append(&rest[..end], 0.0, fmt(string));
            i += end;
            continue;
        }
        if c.is_ascii_digit() {
            let end = rest.find(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_' && ch != '.').unwrap_or(rest.len());
            job.append(&rest[..end], 0.0, fmt(num));
            i += end;
            continue;
        }
        if c.is_ascii_alphabetic() || c == b'_' {
            let end = rest.find(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_').unwrap_or(rest.len());
            let word = &rest[..end];
            let color = if KEYWORDS.contains(&word) {
                kw
            } else if word.chars().next().is_some_and(char::is_uppercase) || ["u32", "usize", "i32", "u8", "str", "String"].contains(&word) {
                ty
            } else {
                p.text
            };
            job.append(word, 0.0, fmt(color));
            i += end;
            continue;
        }
        let ch_len = rest.chars().next().map_or(1, char::len_utf8);
        job.append(&rest[..ch_len], 0.0, fmt(p.text));
        i += ch_len;
    }
    job
}

fn spans_text(spans: &[Span]) -> String {
    spans.iter().map(|s| s.text.as_str()).collect()
}

/// The text of each block as it is drawn (one entry per selectable part): what a selection
/// copies when the block was never on screen.
pub(crate) fn plain_parts(blocks: &[&Block]) -> Vec<String> {
    blocks
        .iter()
        .flat_map(|b| match b {
            Block::Para(s) | Block::Heading(_, s) | Block::Quote(s) => vec![spans_text(s)],
            Block::Item { marker, spans, .. } => vec![format!("{marker} {}", spans_text(spans))],
            Block::Code { text, .. } => vec![text.clone()],
            Block::Rule => vec![],
            // One part per cell, header first, as `show` draws them.
            Block::Table { head, rows } => head.iter().chain(rows.iter().flatten()).map(|c| spans_text(c)).collect(),
        })
        .collect()
}

/// A guess at the height [`show`] gives `text` at `width`, from the text alone (D163, P251):
/// its lines wrapped at `char_w` points a character, code lines unwrapped, the space between
/// blocks. For a transcript block not drawn yet, so the scroll bar and the content's height
/// are near the truth before it is measured. Cheap: one pass over the text, no parse.
pub(crate) fn guess_height(text: &str, width: f32, char_w: f32, font_size: f32, gap: f32) -> f32 {
    let line = font_size * 1.45;
    let code_line = size::MONO * 1.55;
    let per = (width / char_w.max(1.0)).max(8.0);
    let (mut h, mut blocks, mut code, mut para) = (0.0_f32, 0_u16, false, false);
    for l in text.split('\n') {
        let s = l.trim();
        if s.starts_with("```") || s.starts_with("~~~") {
            // The code frame's margins, once per block.
            if !code {
                blocks = blocks.saturating_add(1);
                h += 16.0;
            }
            code = !code;
            para = false;
            continue;
        }
        if code {
            h += code_line;
            continue;
        }
        if s.is_empty() {
            para = false;
            continue;
        }
        let item = s.starts_with("- ") || s.starts_with("* ") || s.starts_with('#') || s.split_once(". ").is_some_and(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
        if !para || item {
            blocks = blocks.saturating_add(1);
        }
        para = true;
        #[allow(clippy::cast_precision_loss, reason = "a line's length")]
        let rows = (l.chars().count() as f32 / per).ceil().max(1.0);
        h += rows * line;
    }
    h + f32::from(blocks.saturating_sub(1)) * (6.0 + gap)
}

/// Draw blocks. `color` is the prose colour (primary, or secondary for inherited history).
/// Text is drawn through `sc`, so it can be selected across items.
pub(crate) fn show(ui: &mut egui::Ui, sc: &mut SelCx<'_>, blocks: &[&Block], p: &Palette, color: Color32, font_size: f32) {
    let width = ui.available_width();
    let font = theme::ui(font_size);
    for (n, b) in blocks.iter().enumerate() {
        if n > 0 {
            ui.add_space(6.0);
        }
        match b {
            Block::Para(s) => {
                sc.job(ui, job_for(s, &font, color, p, width), color);
            }
            Block::Heading(level, s) => {
                let size = match level {
                    1 => font_size + 5.0,
                    2 => font_size + 3.0,
                    _ => font_size + 1.0,
                };
                let mut job = job_for(s, &theme::bold(size), color, p, width);
                for sec in &mut job.sections {
                    if !sec.format.font_id.family.eq(&egui::FontFamily::Monospace) {
                        sec.format.font_id = theme::bold(size);
                    }
                }
                sc.job(ui, job, color);
            }
            Block::Item { depth, marker, spans } => {
                ui.horizontal_top(|ui| {
                    ui.add_space(6.0 + f32::from(*depth) * 18.0);
                    // The marker is part of the item's text, so a copied list keeps its bullets
                    // and numbers.
                    let mut job = LayoutJob::default();
                    job.append(
                        &format!("{marker} "),
                        0.0,
                        egui::TextFormat { font_id: font.clone(), color: p.text2, line_height: Some(font.size * 1.45), ..Default::default() },
                    );
                    let rest = job_for(spans, &font, color, p, f32::INFINITY);
                    for sec in rest.sections {
                        job.append(&rest.text[sec.byte_range.start.0..sec.byte_range.end.0], 0.0, sec.format);
                    }
                    job.wrap.max_width = ui.available_width();
                    sc.job(ui, job, color);
                });
            }
            Block::Quote(s) => {
                egui::Frame::NONE.inner_margin(egui::Margin { left: 12, right: 0, top: 0, bottom: 0 }).show(ui, |ui| {
                    sc.job(ui, job_for(s, &font, p.text2, p, width - 12.0), p.text2);
                });
            }
            Block::Rule => {
                ui.add_space(4.0);
            }
            Block::Code { lang, text } if crate::mermaid::language(lang) => {
                sc.diagram(text);
                crate::mermaid::show(ui, text, p.dark);
            }
            Block::Code { text, .. } => code_block(ui, sc, text, p),
            Block::Table { head, rows } => table(ui, sc, head, rows, p, color, &font, width, n),
        }
    }
}

/// A markdown table: a grid with a bold header row and a hairline under it, no other rules
/// (the visual direction: no boxes). Columns share the width; a long cell wraps within its
/// column. Every cell is its own selectable part (see [`plain_parts`]).
#[allow(clippy::too_many_arguments, reason = "the drawing context of one block")]
fn table(ui: &mut egui::Ui, sc: &mut SelCx<'_>, head: &[Vec<Span>], rows: &[Vec<Vec<Span>>], p: &Palette, color: Color32, font: &FontId, width: f32, n: usize) {
    let cols = head.len().max(rows.iter().map(Vec::len).max().unwrap_or(0)).max(1);
    const GAP: f32 = 18.0;
    #[allow(clippy::cast_precision_loss, reason = "a handful of columns")]
    let col_w = ((width - GAP * (cols - 1) as f32) / cols as f32).max(60.0);
    let bold = theme::bold(font.size);
    let header_rect = std::cell::Cell::new(None::<egui::Rect>);
    egui::Grid::new(ui.scope_id().with(("md-table", n))).num_columns(cols).spacing(vec2(GAP, 6.0)).max_col_width(col_w).show(ui, |ui| {
        if !head.is_empty() {
            for c in 0..cols {
                let cell = head.get(c).map_or(&[][..], Vec::as_slice);
                let mut job = job_for(cell, &bold, color, p, col_w);
                for sec in &mut job.sections {
                    if !sec.format.font_id.family.eq(&egui::FontFamily::Monospace) {
                        sec.format.font_id = bold.clone();
                    }
                }
                let r = sc.job(ui, job, color);
                header_rect.set(Some(header_rect.get().map_or(r.rect, |h| h.union(r.rect))));
            }
            ui.end_row();
        }
        for row in rows {
            for c in 0..cols {
                let cell = row.get(c).map_or(&[][..], Vec::as_slice);
                sc.job(ui, job_for(cell, font, color, p, col_w), color);
            }
            ui.end_row();
        }
    });
    if let Some(h) = header_rect.get() {
        let y = h.bottom() + 3.0;
        ui.painter().hline(h.left()..=(h.left() + width).min(ui.max_rect().right()), y, egui::Stroke::new(1.0, p.border));
    }
}

/// A code block: faint fill, no outline, line numbers beside (not part of the selectable or
/// copied text), and a copy button at the top right.
pub(crate) fn code_block(ui: &mut egui::Ui, sc: &mut SelCx<'_>, text: &str, p: &Palette) {
    let font = theme::mono(size::MONO);
    let line_h = size::MONO * 1.55;
    let fmt = |c: Color32| egui::TextFormat { font_id: font.clone(), color: c, line_height: Some(line_h), ..Default::default() };
    let frame = egui::Frame::NONE.fill(p.code_bg).corner_radius(5).inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let lines: Vec<&str> = text.split('\n').collect();
        let digits = lines.len().to_string().len();
        let mut numbers = LayoutJob::default();
        let mut code = LayoutJob::default();
        for (i, line) in lines.iter().enumerate() {
            let last = i + 1 == lines.len();
            numbers.append(&format!("{:>digits$}{}", i + 1, if last { "" } else { "\n" }), 0.0, fmt(p.text2));
            let hl = highlight(line, p, &font);
            for mut sec in hl.sections {
                sec.format.line_height = Some(line_h);
                code.append(&hl.text[sec.byte_range.start.0..sec.byte_range.end.0], 0.0, sec.format);
            }
            if !last {
                code.append("\n", 0.0, fmt(p.text));
            }
        }
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 14.0;
            let ng = ui.fonts_mut(|f| f.layout_job(numbers));
            let (r, _) = ui.allocate_exact_size(ng.size(), egui::Sense::hover());
            ui.painter().galley(r.min, ng, p.text2);
            sc.job(ui, code, p.text);
        });
    });
    let id = ui.scope_id().with(("code-copy", sc.unit, text.len()));
    copy_button(ui, p, frame.response.rect, text, id);
}

/// Screenshots: draw every copy button as if the pointer were over its block.
pub(crate) const FORCE_COPY_BUTTONS: &str = "force-copy-buttons";

/// The copy button at the top right of `block`: shown while the pointer is over the block, or
/// while it has keyboard focus (it is always in the focus order); reads "Copied" for a moment
/// after use. Drawn without an outline: a faint fill under it.
pub(crate) fn copy_button(ui: &mut egui::Ui, p: &Palette, block: egui::Rect, text: &str, id: egui::Id) {
    const SHOWN_FOR: f64 = 1.5;
    let now = ui.input(|i| i.time);
    let copied_at: Option<f64> = ui.data(|d| d.get_temp(id));
    let copied = copied_at.is_some_and(|t| now - t < SHOWN_FOR);
    let label = if copied { "Copied" } else { "Copy" };
    let font = theme::ui(size::SMALL);
    let g = ui.fonts_mut(|f| f.layout_no_wrap(label.to_owned(), font, p.text2));
    let size = egui::vec2(g.size().x + 30.0, 22.0);
    let rect = egui::Rect::from_min_size(egui::pos2(block.right() - size.x - 6.0, block.top() + 6.0), size);
    let resp = ui.interact(rect, id, egui::Sense::click());
    let force = ui.data(|d| d.get_temp::<bool>(egui::Id::unique(FORCE_COPY_BUTTONS))).unwrap_or(false);
    let show = force || copied || resp.has_focus() || ui.rect_contains_pointer(block);
    let enter = resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) || i.key_pressed(egui::Key::Space));
    if resp.clicked() || enter {
        ui.ctx().copy_text(text.to_owned());
        ui.data_mut(|d| d.insert_temp(id, now));
        // One repaint when "Copied" should read "Copy" again.
        ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(SHOWN_FOR));
    }
    if !show {
        return;
    }
    let pa = ui.painter();
    pa.rect_filled(rect, 5.0, if resp.hovered() || resp.has_focus() { p.faint } else { p.code_bg });
    let c = p.text2;
    let ic = egui::pos2(rect.left() + 13.0, rect.center().y);
    if copied {
        crate::icons::check(pa, ic, 12.0, c);
    } else {
        crate::icons::copy(pa, ic, 12.0, c);
    }
    pa.galley(egui::pos2(rect.left() + 23.0, rect.center().y - g.size().y / 2.0), g, c);
    if resp.has_focus() {
        crate::widgets::focus_ring(ui, rect, p);
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
}

/// The width paragraphs and headings take laid out on one line each, as [`show`] would draw
/// them; `None` when the blocks hold something drawn to the full width (code, a table, a list,
/// a quote). A user's bubble hugs a short message with it (D155).
pub(crate) fn natural_width(ui: &egui::Ui, blocks: &[&Block], p: &Palette, font_size: f32) -> Option<f32> {
    let font = theme::ui(font_size);
    let mut w: f32 = 0.0;
    for b in blocks {
        let job = match b {
            Block::Para(s) => job_for(s, &font, p.text, p, f32::INFINITY),
            Block::Heading(level, s) => {
                let size = match level {
                    1 => font_size + 5.0,
                    2 => font_size + 3.0,
                    _ => font_size + 1.0,
                };
                job_for(s, &theme::bold(size), p.text, p, f32::INFINITY)
            }
            Block::Rule => continue,
            _ => return None,
        };
        let g = ui.fonts_mut(|f| f.layout_job(job));
        w = w.max(g.size().x);
    }
    Some(w)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_the_user_typed_keeps_its_line_breaks() {
        // One Enter is a line break in a user's message (D157); markdown would join the lines.
        let text = "first line\nsecond line\n\nnew paragraph\n- a\n- b";
        let typed = parse_typed(text);
        assert!(matches!(&typed[0], Block::Para(s) if spans_text(s) == "first line\nsecond line"));
        assert!(matches!(&typed[1], Block::Para(s) if spans_text(s) == "new paragraph"));
        assert!(matches!(&typed[2], Block::Item { .. }), "lists still work");
        // An agent's markdown still joins soft breaks into the paragraph.
        assert!(matches!(&parse(text)[0], Block::Para(s) if spans_text(s) == "first line second line"));
    }

    #[test]
    fn a_table_is_a_table_not_its_cells_run_together() {
        // The shape of the message that showed as one run-on paragraph.
        let md = "**Checked:**\n\n| Item | Status |\n|---|---|\n| Host C: | **40 GB free**. The compaction worked |\n| Git | healthy, origin at `df88f4aa31` |\n\nAfter the table.";
        let b = parse(md);
        assert!(matches!(&b[0], Block::Para(_)));
        let Block::Table { head, rows } = &b[1] else { panic!("a table, got {:?}", b[1]) };
        assert_eq!(head.iter().map(|c| spans_text(c)).collect::<Vec<_>>(), ["Item", "Status"]);
        assert_eq!(rows.len(), 2);
        assert_eq!(spans_text(&rows[0][0]), "Host C:");
        assert_eq!(spans_text(&rows[0][1]), "40 GB free. The compaction worked");
        assert!(rows[0][1][0].bold, "inline formatting kept inside a cell");
        assert!(rows[1][1].iter().any(|s| s.code && s.text == "df88f4aa31"));
        assert!(matches!(&b[2], Block::Para(s) if spans_text(s) == "After the table."), "text after the table is its own paragraph");
        // One part per cell, header first, in reading order.
        let parts = plain_parts(&b.iter().collect::<Vec<_>>());
        assert_eq!(parts[1..7], ["Item", "Status", "Host C:", "40 GB free. The compaction worked", "Git", "healthy, origin at df88f4aa31"]);
    }

    #[test]
    fn parses_blocks() {
        let b = parse("# Title\n\nSome **bold** and `code`.\n\n1. one\n2. two\n\n```rust\nlet a = 1;\n```\n");
        assert!(matches!(&b[0], Block::Heading(1, _)));
        assert!(matches!(&b[1], Block::Para(s) if s.iter().any(|x| x.bold) && s.iter().any(|x| x.code)));
        assert!(matches!(&b[2], Block::Item { marker, .. } if marker == "1."));
        assert!(matches!(&b[3], Block::Item { marker, .. } if marker == "2."));
        assert!(matches!(&b[4], Block::Code { lang, text } if lang == "rust" && text == "let a = 1;"));
    }

    fn text_of(b: &[Block]) -> Vec<String> {
        b.iter()
            .map(|x| match x {
                Block::Para(s) => s.iter().map(|s| s.text.as_str()).collect::<String>(),
                Block::Code { text, .. } => format!("code:{text}"),
                other => format!("{other:?}"),
            })
            .collect()
    }

    #[test]
    fn text_with_angle_brackets_is_text_never_code_and_never_dropped() {
        // An HTML-like block (a harness tag left in a message) was dropped before; a paste
        // wrapper is not a code block.
        let b = parse("<task-notification>\n<task-id>x</task-id>\n</task-notification>\n\nafter");
        assert_eq!(text_of(&b), vec!["<task-notification>\n<task-id>x</task-id>\n</task-notification>".to_owned(), "after".to_owned()]);
        let b = parse("is a < b? use Vec<String> and <T> here");
        assert_eq!(text_of(&b), vec!["is a < b? use Vec<String> and <T> here".to_owned()]);
        let b = parse("<pasted_content id=\"6de6\">\nalpha\n</pasted_content id=\"6de6\">\n\nMy sentence");
        assert!(b.iter().all(|x| matches!(x, Block::Para(_))));
    }

    #[test]
    fn streaming_reparses_only_the_tail_and_matches_the_exact_parse() {
        let full = "First paragraph.\n\n```\nfn x() {}\n\nstill code\n```\n\nThird paragraph with **bold** text.";
        let mut p = Parsed::default();
        let mut stable_ends = Vec::new();
        for end in (1..=full.len()).filter(|&i| full.is_char_boundary(i)) {
            p.update(&full[..end], false);
            stable_ends.push(p.stable_end);
        }
        // The stable prefix only grows, and never splits inside the fence.
        assert!(stable_ends.windows(2).all(|w| w[0] <= w[1]));
        let fence_start = full.find("```").unwrap();
        let fence_end = full.rfind("```").unwrap();
        assert!(stable_ends.iter().all(|&s| s <= fence_start || s > fence_end));
        let streamed: Vec<Block> = p.blocks().cloned().collect();
        p.update(full, true);
        let exact: Vec<Block> = p.blocks().cloned().collect();
        assert_eq!(streamed, exact);
        assert_eq!(exact, parse(full));
    }
}
