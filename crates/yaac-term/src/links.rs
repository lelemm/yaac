//! Links under the pointer: OSC 8 hyperlinks, and URLs found in the text (joined across soft
//! wraps), from the frame alone so hover never touches the grid.
use crate::frame::{Flags, Frame};

/// A link and the cells it covers (one span per row).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub uri: String,
    /// `(row, first column, last column)`, inclusive.
    pub cells: Vec<(usize, usize, usize)>,
}

const SCHEMES: [&str; 7] = ["https://", "http://", "file://", "ftp://", "ssh://", "git://", "mailto:"];

impl Frame {
    /// The link at a viewport cell.
    pub fn link_at(&self, row: usize, col: usize) -> Option<Link> {
        let line = self.lines.get(row)?;
        if let Some(l) = line.link_at(col) {
            return Some(Link { uri: l.uri.to_string(), cells: vec![(row, l.start, l.end)] });
        }
        // The logical line: rows joined by soft wraps.
        let mut top = row;
        while top > 0 && self.lines[top - 1].wrapped {
            top -= 1;
        }
        let mut bottom = row;
        while bottom + 1 < self.lines.len() && self.lines[bottom].wrapped {
            bottom += 1;
        }
        let mut text = String::new();
        let mut at: Vec<(usize, usize)> = Vec::new();
        for (r, line) in self.lines.iter().enumerate().take(bottom + 1).skip(top) {
            for (c, cell) in line.cells.iter().enumerate() {
                if cell.flags.has(Flags::SPACER) {
                    continue;
                }
                text.push(cell.ch);
                at.push((r, c));
            }
        }
        let chars: Vec<char> = text.chars().collect();
        let here = at.iter().position(|p| *p == (row, col)).or_else(|| {
            // A spacer: its wide character.
            at.iter().position(|p| *p == (row, col.saturating_sub(1)))
        })?;
        let (start, end) = url_around(&chars, here)?;
        let uri: String = chars[start..=end].iter().collect();
        let mut cells: Vec<(usize, usize, usize)> = Vec::new();
        for &(r, c) in &at[start..=end] {
            match cells.last_mut() {
                Some(last) if last.0 == r => last.2 = c,
                _ => cells.push((r, c, c)),
            }
        }
        Some(Link { uri, cells })
    }
}

fn stop(c: char) -> bool {
    c.is_whitespace() || c.is_control() || matches!(c, '<' | '>' | '"' | '`' | '{' | '}' | '|' | '\\' | '^' | '│')
}

/// The URL containing `here`, as an inclusive char range.
pub(crate) fn url_around(chars: &[char], here: usize) -> Option<(usize, usize)> {
    if stop(*chars.get(here)?) {
        return None;
    }
    let mut a = here;
    while a > 0 && !stop(chars[a - 1]) {
        a -= 1;
    }
    let mut b = here;
    while b + 1 < chars.len() && !stop(chars[b + 1]) {
        b += 1;
    }
    let word: String = chars[a..=b].iter().collect();
    // The scheme may start inside the word (`(see:https://…`).
    let (offset, _) = SCHEMES.iter().filter_map(|s| word.find(s).map(|i| (i, s))).min_by_key(|(i, _)| *i)?;
    let start = a + word[..offset].chars().count();
    let mut end = b;
    // Trailing punctuation is not part of a URL; a closing bracket only when unbalanced.
    loop {
        let c = chars[end];
        let body = &chars[start..=end];
        let unbalanced = |open: char, close: char| {
            c == close && body.iter().filter(|x| **x == open).count() < body.iter().filter(|x| **x == close).count()
        };
        if matches!(c, '.' | ',' | ':' | ';' | '!' | '?' | '\'') || unbalanced('(', ')') || unbalanced('[', ']') {
            if end == start {
                return None;
            }
            end -= 1;
        } else {
            break;
        }
    }
    let scheme_len = SCHEMES.iter().find(|s| chars[start..].iter().collect::<String>().starts_with(**s))?.chars().count();
    (end + 1 - start > scheme_len && (start..=end).contains(&here)).then_some((start, end))
}
