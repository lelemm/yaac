use std::ops::RangeInclusive;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Excerpt {
    pub label: String,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffLineKind {
    Context,
    Removed,
    Added,
}

/// Format a file selection. The range is one-based and inclusive.
pub fn file_excerpt(
    path: &str,
    language: Option<&str>,
    lines: RangeInclusive<usize>,
    text: &str,
) -> Excerpt {
    format_excerpt(path, language.unwrap_or_default(), lines, text, false)
}

/// Format selected diff rows in display order, each containing the selected part of one line
/// with its terminator excluded. The range is one-based and inclusive.
pub fn diff_excerpt<'a>(
    path: &str,
    lines: RangeInclusive<usize>,
    rows: impl IntoIterator<Item = (DiffLineKind, &'a str)>,
) -> Excerpt {
    let mut diff = String::new();
    for (kind, text) in rows {
        diff.push(match kind {
            DiffLineKind::Context => ' ',
            DiffLineKind::Removed => '-',
            DiffLineKind::Added => '+',
        });
        diff.push_str(text);
        diff.push('\n');
    }
    format_excerpt(path, "diff", lines, &diff, true)
}

fn format_excerpt(
    path: &str,
    language: &str,
    lines: RangeInclusive<usize>,
    text: &str,
    diff: bool,
) -> Excerpt {
    let (start, end) = lines.into_inner();
    let range = if start == end {
        start.to_string()
    } else {
        format!("{start}–{end}")
    };
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let label = if diff {
        format!("{name} diff {range}")
    } else {
        format!("{name}:{range}")
    };

    // A selection can contain fenced code of its own. Choose a fence longer than any of
    // its backtick runs so the excerpt is still a single block.
    let mut longest = 0;
    let mut run = 0;
    for byte in text.bytes() {
        if byte == b'`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    let fence = "`".repeat(3.max(longest + 1));
    let language = language.split_whitespace().next().unwrap_or_default();
    let language = if language.contains('`') { "" } else { language };
    let mut output = format!("{path}:{range}\n{fence}{language}\n{text}");
    if !text.is_empty() && !text.ends_with('\n') {
        output.push('\n');
    }
    output.push_str(&fence);
    output.push('\n');
    Excerpt {
        label,
        text: output,
    }
}
