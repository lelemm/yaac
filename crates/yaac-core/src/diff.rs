//! File changes as diffs (docs/design/screens.md, "Code blocks and diffs").
//!
//! A `toolCall` item of kind `fileChange` carries its change as unified-diff text in its
//! `output`, several files concatenated. This module parses that text into files and hunks,
//! counts the lines added and removed per file and per turn, and decides how much of a long
//! diff is shown before the "show the rest" line. Text that is not a diff is reported as such,
//! and the window shows it as plain output.

/// How a file changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileStatus {
    /// Changed in place.
    Modified,
    /// Created.
    Added,
    /// Removed.
    Deleted,
    /// Moved (possibly changed as well).
    Renamed,
}

/// What a diff line is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    /// Unchanged context.
    Context,
    /// Added (`+`).
    Added,
    /// Removed (`-`).
    Removed,
    /// `\ No newline at end of file`: shown as a note, not counted.
    NoNewline,
}

/// One line of a hunk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLine {
    /// Its kind.
    pub kind: LineKind,
    /// Line number in the old file (context and removed lines).
    pub old: Option<u32>,
    /// Line number in the new file (context and added lines).
    pub new: Option<u32>,
    /// The text, without the leading `+`, `-`, or space.
    pub text: String,
}

/// One hunk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk {
    /// The `@@ -a,b +c,d @@ …` line, verbatim.
    pub header: String,
    /// Its lines.
    pub lines: Vec<DiffLine>,
}

/// One file's change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileDiff {
    /// The path before (none for a created file).
    pub old_path: Option<String>,
    /// The path after (none for a deleted file).
    pub new_path: Option<String>,
    /// How it changed.
    pub status: FileStatus,
    /// The hunks.
    pub hunks: Vec<Hunk>,
    /// Lines added.
    pub added: u32,
    /// Lines removed.
    pub removed: u32,
    /// The file's section of the diff text, verbatim (what its copy button copies).
    pub text: String,
}

impl FileDiff {
    /// The path to show: the new one, or the old one for a deleted file.
    pub fn path(&self) -> &str {
        self.new_path.as_deref().or(self.old_path.as_deref()).unwrap_or("")
    }

    /// The collapsed line's words: "Edited src/frame.rs", "Created …", "Deleted …",
    /// "Renamed a → b".
    pub fn title(&self) -> String {
        match self.status {
            FileStatus::Modified => format!("Edited {}", self.path()),
            FileStatus::Added => format!("Created {}", self.path()),
            FileStatus::Deleted => format!("Deleted {}", self.path()),
            FileStatus::Renamed => format!("Renamed {} → {}", self.old_path.as_deref().unwrap_or(""), self.new_path.as_deref().unwrap_or("")),
        }
    }

    /// Lines in all hunks.
    pub fn line_count(&self) -> usize {
        self.hunks.iter().map(|h| h.lines.len() + 1).sum()
    }
}

/// `+12 −3` (U+2212 minus), as two separate words so each can take its colour.
pub fn counts(added: u32, removed: u32) -> (String, String) {
    (format!("+{added}"), format!("\u{2212}{removed}"))
}

fn strip_prefix_path(p: &str) -> Option<String> {
    let p = p.trim_end();
    // A tab separates the path from a timestamp in some diffs.
    let p = p.split('\t').next().unwrap_or(p);
    if p == "/dev/null" {
        return None;
    }
    let p = p.strip_prefix("a/").or_else(|| p.strip_prefix("b/")).unwrap_or(p);
    Some(p.to_owned())
}

/// `@@ -12,5 +12,7 @@` → (12, 12).
fn hunk_starts(line: &str) -> Option<(u32, u32)> {
    let rest = line.strip_prefix("@@ ")?;
    let mut parts = rest.split_whitespace();
    let old = parts.next()?.strip_prefix('-')?;
    let new = parts.next()?.strip_prefix('+')?;
    let start = |s: &str| s.split(',').next().and_then(|n| n.parse::<u32>().ok());
    Some((start(old)?, start(new)?))
}

#[derive(Default)]
struct Builder {
    old_path: Option<String>,
    new_path: Option<String>,
    status: Option<FileStatus>,
    hunks: Vec<Hunk>,
    text: String,
    started: bool,
}

impl Builder {
    fn finish(self, out: &mut Vec<FileDiff>) {
        if !self.started {
            return;
        }
        let status = self.status.unwrap_or(match (&self.old_path, &self.new_path) {
            (None, Some(_)) => FileStatus::Added,
            (Some(_), None) => FileStatus::Deleted,
            (Some(a), Some(b)) if a != b => FileStatus::Renamed,
            _ => FileStatus::Modified,
        });
        let (mut added, mut removed) = (0, 0);
        for l in self.hunks.iter().flat_map(|h| &h.lines) {
            match l.kind {
                LineKind::Added => added += 1,
                LineKind::Removed => removed += 1,
                _ => {}
            }
        }
        let mut text = self.text;
        if text.ends_with('\n') {
            text.pop();
        }
        out.push(FileDiff { old_path: self.old_path, new_path: self.new_path, status, hunks: self.hunks, added, removed, text });
    }
}

/// Parse unified-diff text. `None` when the text is not a diff (no file header with at least
/// one hunk, or anything before the first file header that is not part of a diff).
pub fn parse(text: &str) -> Option<Vec<FileDiff>> {
    let mut out = Vec::new();
    let mut cur = Builder::default();
    let (mut old_no, mut new_no) = (0u32, 0u32);
    let mut in_hunk = false;
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut i = 0;
    while i < lines.len() {
        let raw = lines[i];
        let line = raw.trim_end_matches('\n').trim_end_matches('\r');
        if let Some(rest) = line.strip_prefix("diff --git ") {
            std::mem::take(&mut cur).finish(&mut out);
            cur.started = true;
            in_hunk = false;
            // "a/x b/y": the paths, in case no ---/+++ lines follow (a pure rename).
            if let Some((a, b)) = rest.split_once(" b/") {
                cur.old_path = strip_prefix_path(a);
                cur.new_path = Some(b.to_owned());
            }
            cur.text.push_str(raw);
            i += 1;
            continue;
        }
        let next_is_plus = lines.get(i + 1).is_some_and(|n| n.starts_with("+++ "));
        if line.starts_with("--- ") && next_is_plus && (!in_hunk || !line[4..].trim().is_empty()) {
            // A file header. Without a `diff --git` line, it starts a new file.
            if !cur.started || !cur.hunks.is_empty() {
                std::mem::take(&mut cur).finish(&mut out);
                cur.started = true;
            }
            in_hunk = false;
            let next = lines[i + 1].trim_end_matches('\n').trim_end_matches('\r');
            cur.old_path = strip_prefix_path(&line[4..]);
            cur.new_path = strip_prefix_path(&next[4..]);
            cur.text.push_str(raw);
            cur.text.push_str(lines[i + 1]);
            i += 2;
            continue;
        }
        if !cur.started {
            // Something before any file header: not a diff.
            if line.trim().is_empty() {
                i += 1;
                continue;
            }
            return None;
        }
        cur.text.push_str(raw);
        if let Some((o, n)) = hunk_starts(line) {
            cur.hunks.push(Hunk { header: line.to_owned(), lines: Vec::new() });
            old_no = o;
            new_no = n;
            in_hunk = true;
        } else if in_hunk && let Some(h) = cur.hunks.last_mut() {
            let (kind, body) = match line.chars().next() {
                Some('+') => (LineKind::Added, &line[1..]),
                Some('-') => (LineKind::Removed, &line[1..]),
                Some(' ') => (LineKind::Context, &line[1..]),
                Some('\\') => (LineKind::NoNewline, line),
                // An empty line inside a hunk is an empty context line (some tools trim it).
                None => (LineKind::Context, ""),
                Some(_) => (LineKind::Context, line),
            };
            let (old, new) = match kind {
                LineKind::Context => {
                    old_no += 1;
                    new_no += 1;
                    (Some(old_no - 1), Some(new_no - 1))
                }
                LineKind::Added => {
                    new_no += 1;
                    (None, Some(new_no - 1))
                }
                LineKind::Removed => {
                    old_no += 1;
                    (Some(old_no - 1), None)
                }
                LineKind::NoNewline => (None, None),
            };
            h.lines.push(DiffLine { kind, old, new, text: body.to_owned() });
        } else if let Some(p) = line.strip_prefix("rename from ") {
            cur.old_path = Some(p.to_owned());
            cur.status = Some(FileStatus::Renamed);
        } else if let Some(p) = line.strip_prefix("rename to ") {
            cur.new_path = Some(p.to_owned());
            cur.status = Some(FileStatus::Renamed);
        } else if line.starts_with("new file mode") {
            cur.status = Some(FileStatus::Added);
            cur.old_path = None;
        } else if line.starts_with("deleted file mode") {
            cur.status = Some(FileStatus::Deleted);
            cur.new_path = None;
        }
        i += 1;
    }
    cur.finish(&mut out);
    let any = out.iter().any(|f| !f.hunks.is_empty() || f.status == FileStatus::Renamed);
    if out.is_empty() || !any { None } else { Some(out) }
}

/// A file's lines added and removed, without keeping the hunks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileStat {
    /// "Edited src/frame.rs".
    pub title: String,
    /// Lines added.
    pub added: u32,
    /// Lines removed.
    pub removed: u32,
}

/// The per-file counts of a diff text, or `None` when it is not a diff.
pub fn stats(text: &str) -> Option<Vec<FileStat>> {
    parse(text).map(|files| files.iter().map(|f| FileStat { title: f.title(), added: f.added, removed: f.removed }).collect())
}

/// A turn's files changed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TurnTotals {
    /// Files.
    pub files: usize,
    /// Lines added.
    pub added: u32,
    /// Lines removed.
    pub removed: u32,
}

impl TurnTotals {
    /// Add a file.
    pub fn add(&mut self, f: &FileStat) {
        self.files += 1;
        self.added += f.added;
        self.removed += f.removed;
    }

    /// "Changed 3 files".
    pub fn text(&self) -> String {
        format!("Changed {} {}", self.files, if self.files == 1 { "file" } else { "files" })
    }
}

/// A long diff shows its first hunks up to this many lines, then "show the rest".
pub const FOLD_LINES: usize = 40;

/// How many hunks to show, and how many lines are behind "show the rest". At least one hunk
/// is always shown, whole.
pub fn fold(file: &FileDiff, rest_open: bool) -> (usize, usize) {
    if rest_open {
        return (file.hunks.len(), 0);
    }
    let mut shown = 0;
    let mut lines = 0;
    for h in &file.hunks {
        let n = h.lines.len() + 1;
        if shown > 0 && lines + n > FOLD_LINES {
            break;
        }
        lines += n;
        shown += 1;
    }
    let hidden = file.hunks[shown..].iter().map(|h| h.lines.len() + 1).sum();
    (shown, hidden)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODIFIED: &str = "diff --git a/src/frame.rs b/src/frame.rs
index 3f1c2aa..8d0e4b1 100644
--- a/src/frame.rs
+++ b/src/frame.rs
@@ -10,4 +10,5 @@ fn decode(buf: &[u8]) -> Result<Frame> {
     let kind = buf[0];
-    let len = u32::from_be_bytes(buf[1..5].try_into()?) as usize;
+    let len = u32::from_be_bytes(buf[1..5].try_into()?);
+    let total = 1 + len as usize;
     if buf.len() < total {
";

    const ADDED: &str = "diff --git a/tests/last_frame.rs b/tests/last_frame.rs
new file mode 100644
index 0000000..5b2a3c4
--- /dev/null
+++ b/tests/last_frame.rs
@@ -0,0 +1,3 @@
+#[test]
+fn keeps_the_last_frame() {}
+
";

    const DELETED: &str = "diff --git a/src/old_auth.rs b/src/old_auth.rs
deleted file mode 100644
index 9a1b2c3..0000000
--- a/src/old_auth.rs
+++ /dev/null
@@ -1,2 +0,0 @@
-pub fn login() {}
-pub fn logout() {}
";

    const RENAMED: &str = "diff --git a/src/auth.rs b/src/auth/mod.rs
similarity index 90%
rename from src/auth.rs
rename to src/auth/mod.rs
index 1111111..2222222 100644
--- a/src/auth.rs
+++ b/src/auth/mod.rs
@@ -1,3 +1,3 @@
 pub mod client;
-pub use old::Token;
+pub use client::Token;
 pub mod session;
";

    const PURE_RENAME: &str = "diff --git a/README b/README.md
similarity index 100%
rename from README
rename to README.md
";

    const HUNKS: &str = "--- a/lib.rs
+++ b/lib.rs
@@ -1,3 +1,3 @@
 a
-b
+B
 c
@@ -20,2 +20,3 @@ fn tail() {
 x
+y
 z
";

    const NO_NEWLINE: &str = "--- a/VERSION
+++ b/VERSION
@@ -1 +1 @@
-0.1.0
\\ No newline at end of file
+0.2.0
\\ No newline at end of file
";

    #[test]
    fn table() {
        // (name, text, expected per file: (title, added, removed, hunks))
        type Case<'a> = (&'a str, &'a str, &'a [(&'a str, u32, u32, usize)]);
        let cases: &[Case<'_>] = &[
            ("modified", MODIFIED, &[("Edited src/frame.rs", 2, 1, 1)]),
            ("added file", ADDED, &[("Created tests/last_frame.rs", 3, 0, 1)]),
            ("deleted file", DELETED, &[("Deleted src/old_auth.rs", 0, 2, 1)]),
            ("rename with a change", RENAMED, &[("Renamed src/auth.rs → src/auth/mod.rs", 1, 1, 1)]),
            ("pure rename", PURE_RENAME, &[("Renamed README → README.md", 0, 0, 0)]),
            ("several hunks, no git header", HUNKS, &[("Edited lib.rs", 2, 1, 2)]),
            ("no trailing newline", NO_NEWLINE, &[("Edited VERSION", 1, 1, 1)]),
        ];
        for (name, text, want) in cases {
            let files = parse(text).unwrap_or_else(|| panic!("{name}: not parsed"));
            let got: Vec<(String, u32, u32, usize)> = files.iter().map(|f| (f.title(), f.added, f.removed, f.hunks.len())).collect();
            let want: Vec<(String, u32, u32, usize)> = want.iter().map(|(t, a, r, h)| ((*t).to_owned(), *a, *r, *h)).collect();
            assert_eq!(got, want, "{name}");
        }
    }

    #[test]
    fn several_files_concatenated() {
        let text = format!("{MODIFIED}{ADDED}{DELETED}{RENAMED}");
        let files = parse(&text).unwrap();
        let titles: Vec<String> = files.iter().map(FileDiff::title).collect();
        assert_eq!(titles, ["Edited src/frame.rs", "Created tests/last_frame.rs", "Deleted src/old_auth.rs", "Renamed src/auth.rs → src/auth/mod.rs"]);
        // Each file keeps its own section of the text verbatim, for its copy button.
        assert_eq!(files[1].text, ADDED.trim_end_matches('\n'));
        let mut t = TurnTotals::default();
        for s in stats(&text).unwrap() {
            t.add(&s);
        }
        assert_eq!(t, TurnTotals { files: 4, added: 6, removed: 4 });
        assert_eq!(t.text(), "Changed 4 files");
        assert_eq!(counts(t.added, t.removed), ("+6".to_owned(), "\u{2212}4".to_owned()));
    }

    #[test]
    fn line_numbers_follow_the_hunk_headers() {
        let f = &parse(MODIFIED).unwrap()[0];
        let nums: Vec<(LineKind, Option<u32>, Option<u32>)> = f.hunks[0].lines.iter().map(|l| (l.kind, l.old, l.new)).collect();
        assert_eq!(
            nums,
            [
                (LineKind::Context, Some(10), Some(10)),
                (LineKind::Removed, Some(11), None),
                (LineKind::Added, None, Some(11)),
                (LineKind::Added, None, Some(12)),
                (LineKind::Context, Some(12), Some(13)),
            ]
        );
        assert_eq!(f.hunks[0].lines[1].text, "    let len = u32::from_be_bytes(buf[1..5].try_into()?) as usize;");
        let nn = &parse(NO_NEWLINE).unwrap()[0];
        assert_eq!(nn.hunks[0].lines[1].kind, LineKind::NoNewline);
        assert_eq!(nn.hunks[0].lines[2], DiffLine { kind: LineKind::Added, old: None, new: Some(1), text: "0.2.0".into() });
    }

    #[test]
    fn not_a_diff_falls_back() {
        for text in [
            "",
            "Wrote docs/release/auth-migration.md",
            "test result: ok. 14 passed",
            "--- just a dashed line\nand more text",
            "Updated 3 files:\n--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n+b\n",
        ] {
            assert_eq!(parse(text), None, "{text:?}");
            assert_eq!(stats(text), None);
        }
    }

    #[test]
    fn a_long_diff_folds_after_whole_hunks() {
        let mut text = String::from("--- a/big.rs\n+++ b/big.rs\n");
        for h in 0..6 {
            text.push_str(&format!("@@ -{0},10 +{0},10 @@\n", h * 100 + 1));
            for i in 0..10 {
                text.push_str(&format!("-old {h}.{i}\n+new {h}.{i}\n"));
            }
        }
        let f = &parse(&text).unwrap()[0];
        assert_eq!((f.added, f.removed), (60, 60));
        // 21 lines per hunk (header + 20): one hunk fits under 40, the second would not.
        assert_eq!(fold(f, false), (1, 5 * 21));
        assert_eq!(fold(f, true), (6, 0));
        let small = &parse(HUNKS).unwrap()[0];
        assert_eq!(fold(small, false), (2, 0));
    }
}
