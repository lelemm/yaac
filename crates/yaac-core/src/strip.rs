//! The sidebar's width, and its collapsed form: a narrow vertical strip (decision D94).
//!
//! The sidebar is resized by dragging the hairline beside it, between [`MIN_WIDTH`] and
//! [`MAX_WIDTH`]; dragging it well below the minimum, the button at its top, or Ctrl+B
//! collapses it to a [`STRIP_WIDTH`] strip, and the same controls bring back the remembered
//! width. A window too narrow for the full sidebar shows the strip by itself.
//!
//! The strip's content comes from the same sidebar model as the full list, in the same order:
//! the Needs-you threads first under a small amber count, then each section's name (drawn
//! vertically) followed by one state icon per thread.

use crate::attention::Mark;
use crate::key::ThreadKey;
use crate::sidebar::{Section, SectionKind};

/// The narrowest full sidebar.
pub const MIN_WIDTH: f32 = 220.0;
/// The widest.
pub const MAX_WIDTH: f32 = 480.0;
/// The default width.
pub const DEFAULT_WIDTH: f32 = 280.0;
/// The collapsed strip.
pub const STRIP_WIDTH: f32 = 48.0;
/// Dragging the edge this far below the minimum collapses the sidebar.
pub const COLLAPSE_BELOW: f32 = MIN_WIDTH - 60.0;

/// The sidebar's size, as the user left it (remembered in the window's preferences).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SidebarSize {
    /// The full sidebar's width (kept while collapsed, to come back to).
    pub width: f32,
    /// Collapsed to the strip.
    pub collapsed: bool,
}

impl Default for SidebarSize {
    fn default() -> Self {
        Self { width: DEFAULT_WIDTH, collapsed: false }
    }
}

impl SidebarSize {
    /// The width drawn: the strip when collapsed or when the window is too narrow (`narrow`).
    pub fn shown(&self, narrow: bool) -> f32 {
        if self.collapsed || narrow { STRIP_WIDTH } else { self.width.clamp(MIN_WIDTH, MAX_WIDTH) }
    }

    /// The button or Ctrl+B.
    pub fn toggle(&mut self) {
        self.collapsed = !self.collapsed;
    }

    /// The edge dragged so the sidebar would be `to` points wide (from the drag's start and
    /// the pointer). Below [`COLLAPSE_BELOW`] it collapses; from the strip, dragging out past
    /// the minimum expands it.
    pub fn drag_to(&mut self, to: f32) {
        if to < COLLAPSE_BELOW {
            self.collapsed = true;
        } else {
            self.collapsed = false;
            self.width = to.clamp(MIN_WIDTH, MAX_WIDTH);
        }
    }
}

/// One entry of the strip, top to bottom.
#[derive(Clone, Debug, PartialEq)]
pub enum StripEntry {
    /// "Needs you": a small amber count.
    NeedsCount(usize),
    /// A section's name, drawn vertically (reading bottom to top); `count` for a collapsed
    /// section (Muted, Archived) whose threads are not listed.
    Label {
        /// The name ("api", "Pinned", "Muted").
        text: String,
        /// Collapsed: its threads are not listed.
        collapsed: bool,
    },
    /// A thread's state icon.
    Thread {
        /// The thread.
        key: ThreadKey,
        /// Its mark.
        mark: Mark,
        /// It is open (drawn on the selected-row tint).
        selected: bool,
        /// Its section is Needs you.
        needs_you: bool,
    },
}

/// The strip's entries from the sidebar's sections, in the same order as the full list.
pub fn entries(sections: &[Section]) -> Vec<StripEntry> {
    let mut out = Vec::new();
    for s in sections {
        match s.kind {
            SectionKind::NeedsYou => out.push(StripEntry::NeedsCount(s.count)),
            _ => {
                let text = s.header.as_ref().map_or_else(String::new, |h| {
                    // The section's own name, without its count ("Muted · 4" → "Muted").
                    h.left.split(" · ").next().unwrap_or(&h.left).to_owned()
                });
                if !text.is_empty() || s.collapsed {
                    out.push(StripEntry::Label { text, collapsed: s.collapsed });
                }
            }
        }
        for r in &s.rows {
            out.push(StripEntry::Thread { key: r.key.clone(), mark: r.display.mark, selected: r.selected, needs_you: s.kind == SectionKind::NeedsYou });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn width_rules() {
        let cases: &[(&str, &[f32], (f32, bool))] = &[
            ("default", &[], (DEFAULT_WIDTH, false)),
            ("wider", &[360.0], (360.0, false)),
            ("clamped wide", &[900.0], (MAX_WIDTH, false)),
            ("clamped narrow, not collapsed", &[200.0], (MIN_WIDTH, false)),
            ("well below the minimum collapses, keeping the width", &[330.0, 100.0], (330.0, true)),
            ("dragging out of the strip expands", &[330.0, 100.0, 250.0], (250.0, false)),
        ];
        for (name, drags, want) in cases {
            let mut s = SidebarSize::default();
            for d in *drags {
                s.drag_to(*d);
            }
            assert_eq!((s.width, s.collapsed), *want, "{name}");
        }
        let mut s = SidebarSize { width: 400.0, collapsed: false };
        s.toggle();
        assert_eq!(s.shown(false), STRIP_WIDTH);
        s.toggle();
        assert_eq!(s.shown(false), 400.0, "back to the remembered width");
        assert_eq!(s.shown(true), STRIP_WIDTH, "a narrow window shows the strip by itself");
    }
}
