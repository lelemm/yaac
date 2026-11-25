use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use similar::{Algorithm, ChangeTag, DiffTag, TextDiff, capture_diff_slices_deadline};

use crate::highlight::highlight;
use crate::{Highlighted, MAX_LINE_BYTES, is_cancelled};

pub const MAX_FILE_BYTES: usize = 4 * 1024 * 1024;
const FOLD_THRESHOLD: usize = 8;
const LINE_DIFF_BUDGET: Duration = Duration::from_millis(250);
const WORD_DIFF_BUDGET: Duration = Duration::from_millis(25);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Notice {
    Binary { size: usize },
    TooLarge { size: usize },
}

#[derive(Debug)]
pub struct DiffInput {
    pub path: String,
    /// The previous name for renamed files, if known.
    pub old_path: Option<String>,
    /// Absent for untracked files.
    pub old: Option<String>,
    /// Absent for deleted files.
    pub new: Option<String>,
    /// Metadata supplied by the target when either version could not be read as text.
    pub notice: Option<Notice>,
}

#[derive(Debug)]
pub enum DiffResult {
    Text(Box<Diff>),
    Notice(Notice),
}

#[derive(Debug)]
pub struct Diff {
    pub old: Highlighted,
    pub new: Highlighted,
    /// Line-local byte ranges, indexed by zero-based old line. An entry denotes a paired
    /// replacement even when there are no changed bytes to draw (such as a newline-only edit).
    pub old_inline: BTreeMap<usize, Vec<Range<usize>>>,
    /// Line-local byte ranges, indexed by zero-based new line.
    pub new_inline: BTreeMap<usize, Vec<Range<usize>>>,
    pub unified: DiffView,
    pub side_by_side: DiffView,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffRow {
    /// Zero-based line indices. Unified pairs are unchanged, while side-by-side pairs
    /// can also be replacements, identified by entries in the inline maps.
    Line {
        old: Option<usize>,
        new: Option<usize>,
    },
    /// Both sides have the same number of omitted lines, starting at these indices.
    Fold {
        old_start: usize,
        new_start: usize,
        count: usize,
    },
}

#[derive(Debug, Default)]
pub struct DiffView {
    pub rows: Vec<DiffRow>,
    /// First row of each consecutive change, measured in the folded layout.
    pub change_indices: Vec<usize>,
}

pub(crate) fn build(input: DiffInput, cancelled: &AtomicBool) -> Option<DiffResult> {
    let DiffInput {
        path,
        old_path,
        old,
        new,
        notice,
    } = input;
    let old = old.unwrap_or_default();
    let new = new.unwrap_or_default();
    let size = old.len().max(new.len());
    if let Some(notice) = notice {
        return Some(DiffResult::Notice(notice));
    }
    if size > MAX_FILE_BYTES {
        return Some(DiffResult::Notice(Notice::TooLarge { size }));
    }
    if old.as_bytes().contains(&0) || new.as_bytes().contains(&0) {
        return Some(DiffResult::Notice(Notice::Binary { size }));
    }

    let old = highlight(old_path.as_deref().unwrap_or(&path), old, cancelled)?;
    let new = highlight(&path, new, cancelled)?;
    let ops = {
        let lhs: Vec<_> = (0..old.lines.len())
            .map(|i| old.line_with_ending(i))
            .collect();
        let rhs: Vec<_> = (0..new.lines.len())
            .map(|i| new.line_with_ending(i))
            .collect();
        if is_cancelled(cancelled) {
            return None;
        }
        capture_diff_slices_deadline(
            Algorithm::Myers,
            &lhs,
            &rhs,
            Some(Instant::now() + LINE_DIFF_BUDGET),
        )
    };

    let mut diff = Diff {
        old,
        new,
        old_inline: BTreeMap::new(),
        new_inline: BTreeMap::new(),
        unified: DiffView::default(),
        side_by_side: DiffView::default(),
    };
    let mut in_change = false;
    for op in ops {
        if is_cancelled(cancelled) {
            return None;
        }
        let old = op.old_range();
        let new = op.new_range();
        if op.tag() == DiffTag::Equal {
            diff.unchanged(old, new);
            in_change = false;
        } else {
            if !in_change {
                diff.unified.change_indices.push(diff.unified.rows.len());
                diff.side_by_side
                    .change_indices
                    .push(diff.side_by_side.rows.len());
                in_change = true;
            }
            diff.changed(old, new, cancelled)?;
        }
    }
    Some(DiffResult::Text(Box::new(diff)))
}

impl Diff {
    fn unchanged(&mut self, old: Range<usize>, new: Range<usize>) {
        if old.len() > FOLD_THRESHOLD {
            let fold = DiffRow::Fold {
                old_start: old.start,
                new_start: new.start,
                count: old.len(),
            };
            self.unified.rows.push(fold);
            self.side_by_side.rows.push(fold);
        } else {
            for (old, new) in old.zip(new) {
                let row = DiffRow::Line {
                    old: Some(old),
                    new: Some(new),
                };
                self.unified.rows.push(row);
                self.side_by_side.rows.push(row);
            }
        }
    }

    fn changed(
        &mut self,
        old: Range<usize>,
        new: Range<usize>,
        cancelled: &AtomicBool,
    ) -> Option<()> {
        self.unified
            .rows
            .extend(old.clone().map(|old| DiffRow::Line {
                old: Some(old),
                new: None,
            }));
        self.unified
            .rows
            .extend(new.clone().map(|new| DiffRow::Line {
                old: None,
                new: Some(new),
            }));

        // Align replacement lines by position. Any excess lines occupy only one side.
        for offset in 0..old.len().max(new.len()) {
            if is_cancelled(cancelled) {
                return None;
            }
            let old = (offset < old.len()).then_some(old.start + offset);
            let new = (offset < new.len()).then_some(new.start + offset);
            if let (Some(old), Some(new)) = (old, new) {
                let (lhs, rhs) = word_changes(self.old.line_text(old), self.new.line_text(new));
                self.old_inline.insert(old, lhs);
                self.new_inline.insert(new, rhs);
            }
            self.side_by_side.rows.push(DiffRow::Line { old, new });
        }
        Some(())
    }
}

fn word_changes(old: &str, new: &str) -> (Vec<Range<usize>>, Vec<Range<usize>>) {
    let mut removed = Vec::new();
    let mut added = Vec::new();
    if old == new || old.len().max(new.len()) > MAX_LINE_BYTES {
        return (removed, added);
    }
    let words = TextDiff::configure()
        .timeout(WORD_DIFF_BUDGET)
        .diff_unicode_words(old, new);
    let mut lhs = 0;
    let mut rhs = 0;
    for change in words.iter_all_changes() {
        let len = change.value().len();
        match change.tag() {
            ChangeTag::Equal => {
                lhs += len;
                rhs += len;
            }
            ChangeTag::Delete => {
                append_change(&mut removed, lhs..lhs + len);
                lhs += len;
            }
            ChangeTag::Insert => {
                append_change(&mut added, rhs..rhs + len);
                rhs += len;
            }
        }
    }
    (removed, added)
}

fn append_change(ranges: &mut Vec<Range<usize>>, range: Range<usize>) {
    if let Some(previous) = ranges.last_mut()
        && previous.end == range.start
    {
        previous.end = range.end;
    } else {
        ranges.push(range);
    }
}
