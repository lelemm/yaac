//! Presentation encoding for the WPF retained row visuals.
use serde::Serialize;

#[derive(Serialize)]
pub(super) struct Run(u16, u8, u32, i32, u8, String);

#[derive(Clone, Copy, PartialEq)]
pub(super) struct Style {
    pub width: u8,
    pub ink: u32,
    /// -1 means the terminal canvas, otherwise packed sRGB.
    pub paper: i32,
    pub flags: u8,
}

/// Combining clusters are indivisible. Ordinary cells of equal width/style share
/// one string, including wide cells (their continuation never gets a glyph).
#[derive(Default)]
pub(super) struct Runs {
    end: usize,
    style: Option<Style>,
    runs: Vec<Run>,
}

impl Runs {
    pub(super) fn push(&mut self, col: usize, ch: char, combining: &str, style: Style) {
        if combining.is_empty() && self.end == col && self.style == Some(style) {
            self.runs.last_mut().unwrap().5.push(ch);
        } else {
            let mut text = String::from(ch);
            text.push_str(combining);
            self.runs.push(Run(
                col as u16,
                style.width,
                style.ink,
                style.paper,
                style.flags | if combining.is_empty() { 0 } else { 16 },
                text,
            ));
        }
        self.end = col + usize::from(style.width);
        self.style = combining.is_empty().then_some(style);
    }

    pub(super) fn finish(self) -> Vec<Run> {
        self.runs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_clusters_do_not_merge_with_adjacent_cells() {
        let mut runs = Runs::default();
        let style = Style {
            width: 2,
            ink: 42,
            paper: -1,
            flags: 0,
        };
        runs.push(0, '界', "", style);
        runs.push(2, '表', "", style);
        runs.push(4, '日', "\u{301}", style);
        runs.push(6, '本', "", style);
        let json = serde_json::to_value(runs.finish()).unwrap();
        assert_eq!(
            json,
            serde_json::json!([
                [0, 2, 42, -1, 0, "界表"],
                [4, 2, 42, -1, 16, "日́"],
                [6, 2, 42, -1, 0, "本"]
            ])
        );
    }
}
