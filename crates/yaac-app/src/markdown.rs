//! Agent prose as markdown: `pulldown-cmark` into a small block model, drawn with egui.
//!
//! Parse once: a completed item keeps its parsed blocks. While an item streams, only a
//! bounded tail is re-parsed — the text before the last blank line outside a code fence is
//! parsed once and kept — and the item gets one exact parse when it completes.

use std::collections::HashMap;

use eframe::egui::{self, Color32, FontId, text::LayoutJob};
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use yaac_protocol::ItemId;

use crate::theme::{self, Palette, size};

/// A run of inline text with its style.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub link: bool,
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
}

/// Parse markdown into blocks.
pub(crate) fn parse(text: &str) -> Vec<Block> {
    let mut out = Vec::new();
    let mut spans: Vec<Span> = Vec::new();
    let (mut bold, mut italic, mut link) = (0u32, 0u32, 0u32);
    let mut code: Option<(String, String)> = None;
    let mut lists: Vec<Option<u64>> = Vec::new();
    let mut item_marker: Option<(u8, String)> = None;
    let mut heading: Option<u8> = None;
    let mut quote = 0u32;
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
                        out.push(Block::Code { lang, text: text.trim_end_matches('\n').to_owned() });
                    }
                }
                _ => {}
            }
            continue;
        }
        let push = |spans: &mut Vec<Span>, t: &str, c: bool, b: u32, i: u32, l: u32| {
            spans.push(Span { text: t.to_owned(), bold: b > 0, italic: i > 0, code: c, link: l > 0 });
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
            Event::Start(Tag::Link { .. }) => link += 1,
            Event::End(TagEnd::Link) => link = link.saturating_sub(1),
            Event::Text(t) => push(&mut spans, &t, false, bold, italic, link),
            Event::Code(t) => push(&mut spans, &t, true, bold, italic, link),
            Event::SoftBreak => push(&mut spans, " ", false, 0, 0, 0),
            Event::HardBreak => push(&mut spans, "\n", false, 0, 0, 0),
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
            vec![Block::Para(vec![Span { text: tail.to_owned(), bold: false, italic: false, code: false, link: false }])]
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
        } else if s.bold || s.link {
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
            Block::Code { text, .. } => vec![Span { text, bold: false, italic: false, code: true, link: false }],
            Block::Rule => vec![],
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

/// Draw blocks. `color` is the prose colour (primary, or secondary for inherited history).
pub(crate) fn show(ui: &mut egui::Ui, blocks: &[&Block], p: &Palette, color: Color32, font_size: f32) {
    let width = ui.available_width();
    let font = theme::ui(font_size);
    for (n, b) in blocks.iter().enumerate() {
        if n > 0 {
            ui.add_space(6.0);
        }
        match b {
            Block::Para(s) => {
                ui.label(job_for(s, &font, color, p, width));
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
                ui.label(job);
            }
            Block::Item { depth, marker, spans } => {
                ui.horizontal_top(|ui| {
                    ui.add_space(6.0 + f32::from(*depth) * 18.0);
                    ui.label(egui::RichText::new(marker).font(font.clone()).color(p.text2));
                    let w = ui.available_width();
                    ui.label(job_for(spans, &font, color, p, w));
                });
            }
            Block::Quote(s) => {
                egui::Frame::NONE.inner_margin(egui::Margin { left: 12, right: 0, top: 0, bottom: 0 }).show(ui, |ui| {
                    ui.label(job_for(s, &font, p.text2, p, width - 12.0));
                });
            }
            Block::Rule => {
                ui.add_space(4.0);
            }
            Block::Code { text, .. } => code_block(ui, text, p),
        }
    }
}

/// A code block: faint fill, no outline, line numbers.
pub(crate) fn code_block(ui: &mut egui::Ui, text: &str, p: &Palette) {
    let font = theme::mono(size::MONO);
    egui::Frame::NONE.fill(p.code_bg).corner_radius(5).inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let lines: Vec<&str> = text.lines().collect();
        let digits = lines.len().to_string().len();
        for (i, line) in lines.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 14.0;
                ui.label(egui::RichText::new(format!("{:>digits$}", i + 1)).font(font.clone()).color(p.text2));
                ui.label(highlight(line, p, &font));
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_blocks() {
        let b = parse("# Title\n\nSome **bold** and `code`.\n\n1. one\n2. two\n\n```rust\nlet a = 1;\n```\n");
        assert!(matches!(&b[0], Block::Heading(1, _)));
        assert!(matches!(&b[1], Block::Para(s) if s.iter().any(|x| x.bold) && s.iter().any(|x| x.code)));
        assert!(matches!(&b[2], Block::Item { marker, .. } if marker == "1."));
        assert!(matches!(&b[3], Block::Item { marker, .. } if marker == "2."));
        assert!(matches!(&b[4], Block::Code { lang, text } if lang == "rust" && text == "let a = 1;"));
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
