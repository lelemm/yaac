//! A text selection that runs across the transcript (docs/design/screens.md, "Text can be
//! selected and copied").
//!
//! The transcript is a sequence of **units** in reading order — one per message, activity
//! line, file diff, or card — and each unit is drawn as one or more **parts** (a paragraph, a
//! list item, a code block, a diff line). A position is a unit, a part, and a character offset
//! in that part's text. Because units are identified by the transcript's own item order, not
//! by what happens to be on screen, a selection survives scrolling the virtualised list: a unit
//! that scrolled out of view still contributes its text when the selection is copied.
//!
//! Copying joins parts of one unit with a newline and units with a blank line; a code block is
//! one part, kept verbatim.

use std::collections::BTreeMap;

/// A unit's place in reading order: the transcript item's index, then a sub-index (a file of
/// a file change; [`Unit::END_OF_TURN`] for the summary line after a turn).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Unit {
    /// Index of the item in the transcript.
    pub item: usize,
    /// Sub-index within the item.
    pub sub: usize,
}

impl Unit {
    /// The sub-index of the summary line that ends a turn.
    pub const END_OF_TURN: usize = usize::MAX;

    /// A unit.
    pub fn new(item: usize, sub: usize) -> Self {
        Self { item, sub }
    }
}

/// A position: unit, part, character.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TextPos {
    /// The unit.
    pub unit: Unit,
    /// The part within the unit.
    pub part: usize,
    /// Character offset in the part's text (not bytes).
    pub ch: usize,
}

impl TextPos {
    /// A position.
    pub fn new(unit: Unit, part: usize, ch: usize) -> Self {
        Self { unit, part, ch }
    }
}

/// A selection: where the drag started, and where it is now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    /// Where it started.
    pub anchor: TextPos,
    /// Where it ends (follows the pointer).
    pub focus: TextPos,
}

impl Selection {
    /// A caret at `p` (nothing selected yet).
    pub fn at(p: TextPos) -> Self {
        Self { anchor: p, focus: p }
    }

    /// Start and end in reading order.
    pub fn ordered(&self) -> (TextPos, TextPos) {
        if self.anchor <= self.focus { (self.anchor, self.focus) } else { (self.focus, self.anchor) }
    }

    /// Nothing selected.
    pub fn is_empty(&self) -> bool {
        self.anchor == self.focus
    }

    /// Whether the selection covers part of `unit`.
    pub fn touches(&self, unit: Unit) -> bool {
        let (a, b) = self.ordered();
        !self.is_empty() && a.unit <= unit && unit <= b.unit
    }

    /// The selected characters of one part whose text is `len` characters long, as a
    /// half-open range, or `None` when none of it is selected.
    pub fn range_in(&self, unit: Unit, part: usize, len: usize) -> Option<(usize, usize)> {
        if self.is_empty() {
            return None;
        }
        let (a, b) = self.ordered();
        let here = (unit, part);
        if here < (a.unit, a.part) || here > (b.unit, b.part) {
            return None;
        }
        let start = if here == (a.unit, a.part) { a.ch.min(len) } else { 0 };
        let end = if here == (b.unit, b.part) { b.ch.min(len) } else { len };
        // A part between the ends with no characters still counts (an empty line).
        (start < end || (start == end && here != (a.unit, a.part) && here != (b.unit, b.part))).then_some((start, end))
    }
}

/// The text of the units a selection may cover, in reading order: each unit's parts.
pub type Texts = BTreeMap<Unit, Vec<String>>;

fn slice_chars(s: &str, start: usize, end: usize) -> String {
    s.chars().skip(start).take(end.saturating_sub(start)).collect()
}

/// The selection as plain text: parts of a unit joined by a newline, units by a blank line.
/// `texts` must hold every unit between the ends (more is fine).
pub fn copy_text(sel: &Selection, texts: &Texts) -> String {
    if sel.is_empty() {
        return String::new();
    }
    let (a, b) = sel.ordered();
    let mut units = Vec::new();
    for (unit, parts) in texts.range(a.unit..=b.unit) {
        let mut out = Vec::new();
        for (i, text) in parts.iter().enumerate() {
            let len = text.chars().count();
            if let Some((s, e)) = sel.range_in(*unit, i, len) {
                out.push(slice_chars(text, s, e));
            }
        }
        if !out.is_empty() {
            units.push(out.join("\n"));
        }
    }
    units.join("\n\n")
}

/// The drag that makes a selection, independent of any toolkit: a press on text starts it,
/// moving extends it, and a press that never moved is a click (it selects nothing and leaves
/// the click to whatever is under it — a link, an activity line, a button).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Drag {
    /// The selection, if any.
    pub selection: Option<Selection>,
    dragging: bool,
}

impl Drag {
    /// A press. `on_text` is the position under the pointer, if it is over selectable text.
    /// A press elsewhere clears the selection.
    pub fn press(&mut self, on_text: Option<TextPos>) {
        self.selection = on_text.map(Selection::at);
        self.dragging = on_text.is_some();
    }

    /// The pointer moved to `p` (the nearest text position) while pressed.
    pub fn moved(&mut self, p: TextPos) {
        if self.dragging
            && let Some(s) = &mut self.selection
        {
            s.focus = p;
        }
    }

    /// The button was released.
    pub fn release(&mut self) {
        self.dragging = false;
        if self.selection.is_some_and(|s| s.is_empty()) {
            self.selection = None;
        }
    }

    /// A drag is under way.
    pub fn dragging(&self) -> bool {
        self.dragging
    }

    /// Select everything in `first..=last`.
    pub fn select(&mut self, from: TextPos, to: TextPos) {
        self.selection = Some(Selection { anchor: from, focus: to });
        self.dragging = false;
    }

    /// Forget the selection (the thread changed, Escape).
    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts() -> Texts {
        let mut t = Texts::new();
        // A user message (two paragraphs), an activity line, a code-bearing prose item, a diff.
        t.insert(Unit::new(0, 0), vec!["Why does decode() drop the last frame?".into(), "It happens on every run.".into()]);
        t.insert(Unit::new(1, 0), vec!["Ran cargo test -p yaac-protocol — passed".into()]);
        t.insert(Unit::new(2, 0), vec!["The length prefix counts the kind byte.".into(), "let kind = buf[0];\n    let len = 4;".into(), "That is the fix.".into()]);
        t.insert(Unit::new(3, 1), vec!["Edited src/frame.rs".into()]);
        t
    }

    fn pos(item: usize, part: usize, ch: usize) -> TextPos {
        TextPos::new(Unit::new(item, 0), part, ch)
    }

    #[test]
    fn copy_table() {
        let cases: &[(&str, TextPos, TextPos, &str)] = &[
            ("inside one part", pos(0, 0, 4), pos(0, 0, 8), "does"),
            ("backwards drag is the same", pos(0, 0, 8), pos(0, 0, 4), "does"),
            ("across two parts of one unit", pos(0, 0, 30), pos(0, 1, 2), "t frame?\nIt"),
            (
                "middle of one message to the middle of another two items down",
                pos(0, 1, 11),
                pos(2, 0, 10),
                "on every run.\n\nRan cargo test -p yaac-protocol — passed\n\nThe length",
            ),
            ("a code block in the middle is kept verbatim", pos(2, 0, 29), pos(2, 2, 4), "kind byte.\nlet kind = buf[0];\n    let len = 4;\nThat"),
            ("to the end of the transcript", TextPos::new(Unit::new(2, 0), 2, 12), TextPos::new(Unit::new(3, 1), 0, 6), "fix.\n\nEdited"),
            ("nothing", pos(1, 0, 3), pos(1, 0, 3), ""),
        ];
        for (name, a, b, want) in cases {
            let s = Selection { anchor: *a, focus: *b };
            assert_eq!(copy_text(&s, &texts()), *want, "{name}");
        }
    }

    #[test]
    fn ranges_for_painting() {
        let s = Selection { anchor: pos(0, 1, 3), focus: pos(2, 1, 5) };
        assert_eq!(s.range_in(Unit::new(0, 0), 0, 10), None, "before the start");
        assert_eq!(s.range_in(Unit::new(0, 0), 1, 10), Some((3, 10)));
        assert_eq!(s.range_in(Unit::new(1, 0), 0, 7), Some((0, 7)), "whole unit between the ends");
        assert_eq!(s.range_in(Unit::new(2, 0), 1, 9), Some((0, 5)));
        assert_eq!(s.range_in(Unit::new(2, 0), 2, 9), None, "after the end");
        assert!(s.touches(Unit::new(1, 0)) && !s.touches(Unit::new(3, 1)));
        assert_eq!(s.range_in(Unit::new(1, 0), 3, 0), Some((0, 0)), "an empty line between the ends is kept");
    }

    #[test]
    fn units_scrolled_out_of_view_still_copy() {
        // The texts come from the transcript's items, not from what is drawn: a unit that is
        // not on screen is just another entry.
        let mut t = texts();
        for i in 10..200 {
            t.insert(Unit::new(i, 0), vec![format!("line {i}")]);
        }
        let s = Selection { anchor: pos(0, 0, 0), focus: TextPos::new(Unit::new(199, 0), 0, 8) };
        let c = copy_text(&s, &t);
        assert!(c.starts_with("Why does"));
        assert!(c.contains("\n\nline 100\n\n"));
        assert!(c.ends_with("line 199"));
    }

    #[test]
    fn a_press_without_a_move_is_a_click_not_a_selection() {
        let mut d = Drag::default();
        d.press(Some(pos(1, 0, 4)));
        d.release();
        assert_eq!(d.selection, None);
        d.press(Some(pos(0, 0, 4)));
        d.moved(pos(0, 0, 6));
        d.moved(pos(2, 0, 3));
        d.release();
        assert_eq!(d.selection, Some(Selection { anchor: pos(0, 0, 4), focus: pos(2, 0, 3) }));
        // Moving after release does nothing; a press elsewhere clears.
        d.moved(pos(3, 0, 0));
        assert_eq!(d.selection.unwrap().focus, pos(2, 0, 3));
        d.press(None);
        assert_eq!(d.selection, None);
    }
}
