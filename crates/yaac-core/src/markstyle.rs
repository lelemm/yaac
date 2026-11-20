//! Markdown styled in place for the composer: the text stays exactly as typed (syntax
//! characters visible and editable); this only says how each byte range is drawn. Bounded:
//! beyond `budget` bytes the rest is plain, so pasting a huge text never stalls typing.

use std::ops::Range;

/// How a range is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    /// Ordinary text.
    Plain,
    /// Syntax characters (`#`, `**`, `` ` ``, fence lines' backticks): visible, quieter.
    Marker,
    /// Heading text, by level.
    Heading(u8),
    /// Strong.
    Bold,
    /// Emphasis.
    Italic,
    /// Inline code.
    Code,
    /// Inside a fenced code block (including its fence lines).
    CodeBlock,
}

/// A styled range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    /// Byte range in the text.
    pub range: Range<usize>,
    /// Its style.
    pub style: Style,
}

fn push(out: &mut Vec<Span>, range: Range<usize>, style: Style) {
    if range.is_empty() {
        return;
    }
    if let Some(last) = out.last_mut()
        && last.style == style
        && last.range.end == range.start
    {
        last.range.end = range.end;
        return;
    }
    out.push(Span { range, style });
}

/// Style `text`. The spans cover the whole text, in order, without gaps.
pub fn style(text: &str, budget: usize) -> Vec<Span> {
    let mut out = Vec::new();
    let mut limit = budget.min(text.len());
    while !text.is_char_boundary(limit) {
        limit -= 1;
    }
    let mut in_fence = false;
    let mut pos = 0;
    for line in text[..limit].split_inclusive('\n') {
        let start = pos;
        pos += line.len();
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            push(&mut out, start..pos, Style::CodeBlock);
            continue;
        }
        if in_fence {
            push(&mut out, start..pos, Style::CodeBlock);
            continue;
        }
        let hashes = line.bytes().take_while(|&b| b == b'#').count();
        if (1..=6).contains(&hashes) && line[hashes..].starts_with(' ') {
            let level = u8::try_from(hashes).unwrap_or(6);
            push(&mut out, start..start + hashes + 1, Style::Marker);
            inline(&line[hashes + 1..], start + hashes + 1, Style::Heading(level), &mut out);
            continue;
        }
        let indent = line.len() - trimmed.len();
        let marker = if trimmed.starts_with("- ") || trimmed.starts_with("* ") || trimmed.starts_with("> ") {
            2
        } else {
            let digits = trimmed.bytes().take_while(u8::is_ascii_digit).count();
            if digits > 0 && trimmed[digits..].starts_with(". ") { digits + 2 } else { 0 }
        };
        if marker > 0 {
            push(&mut out, start..start + indent, Style::Plain);
            push(&mut out, start + indent..start + indent + marker, Style::Marker);
            inline(&line[indent + marker..], start + indent + marker, Style::Plain, &mut out);
        } else {
            inline(line, start, Style::Plain, &mut out);
        }
    }
    push(&mut out, limit..text.len(), Style::Plain);
    out
}

/// Inline code, bold, and italic within one line.
fn inline(line: &str, base: usize, plain: Style, out: &mut Vec<Span>) {
    let b = line.as_bytes();
    let mut i = 0;
    let mut run = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'`'
            && let Some(close) = line[i + 1..].find('`')
        {
            push(out, base + run..base + i, plain);
            push(out, base + i..base + i + 1, Style::Marker);
            push(out, base + i + 1..base + i + 1 + close, Style::Code);
            push(out, base + i + 1 + close..base + i + 2 + close, Style::Marker);
            i += close + 2;
            run = i;
            continue;
        }
        if (c == b'*' || c == b'_') && b.get(i + 1) == Some(&c) {
            let delim = if c == b'*' { "**" } else { "__" };
            if let Some(close) = line[i + 2..].find(delim)
                && close > 0
            {
                push(out, base + run..base + i, plain);
                push(out, base + i..base + i + 2, Style::Marker);
                push(out, base + i + 2..base + i + 2 + close, Style::Bold);
                push(out, base + i + 2 + close..base + i + 4 + close, Style::Marker);
                i += close + 4;
                run = i;
                continue;
            }
        } else if (c == b'*' || c == b'_')
            && b.get(i + 1).is_some_and(|n| !n.is_ascii_whitespace())
            && let Some(close) = line[i + 1..].find(c as char)
            && close > 0
        {
            push(out, base + run..base + i, plain);
            push(out, base + i..base + i + 1, Style::Marker);
            push(out, base + i + 1..base + i + 1 + close, Style::Italic);
            push(out, base + i + 1 + close..base + i + 2 + close, Style::Marker);
            i += close + 2;
            run = i;
            continue;
        }
        i += 1;
        while i < b.len() && !line.is_char_boundary(i) {
            i += 1;
        }
    }
    push(out, base + run..base + line.len(), plain);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn covered(text: &str, spans: &[Span]) {
        let mut at = 0;
        for s in spans {
            assert_eq!(s.range.start, at, "gap at {at} in {spans:?}");
            at = s.range.end;
        }
        assert_eq!(at, text.len());
    }

    #[test]
    fn styles_the_brief_example_with_syntax_visible() {
        let text = "# Investigate frame parsing\nReproduce the **last-frame** drop in `decode()`.\n";
        let spans = style(text, 1 << 20);
        covered(text, &spans);
        let at = |needle: &str| spans.iter().find(|s| &text[s.range.clone()] == needle).map(|s| s.style);
        assert_eq!(at("# "), Some(Style::Marker));
        assert_eq!(at("last-frame"), Some(Style::Bold));
        assert_eq!(at("**"), Some(Style::Marker));
        assert_eq!(at("decode()"), Some(Style::Code));
    }

    #[test]
    fn fences_unclosed_marks_and_budget() {
        let text = "```rust\nlet a = 1;\n```\nan *open marker and `tick\n";
        let spans = style(text, 1 << 20);
        covered(text, &spans);
        assert_eq!(spans[0].style, Style::CodeBlock);
        assert_eq!(&text[spans[0].range.clone()], "```rust\nlet a = 1;\n```\n");
        let big = "**x** ".repeat(10_000);
        let spans = style(&big, 64);
        covered(&big, &spans);
        assert_eq!(spans.last().map(|s| s.style), Some(Style::Plain));
        let uni = "é **ü** `ß`";
        covered(uni, &style(uni, 3));
        covered(uni, &style(uni, 100));
    }
}
