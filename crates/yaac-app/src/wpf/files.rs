//! Pipe projection of the shared Files controller. WPF holds only the rendered document.
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashSet},
    io::{self, Write},
    ops::Range,
    sync::{Arc, mpsc},
};
use yaac_code::{Diff, DiffLineKind, DiffRow, DiffView, Excerpt, Highlighted};
use yaac_core::{ThreadKey, attachments::DraftRef, client::Client};
use yaac_protocol::FsTreeEntryKind;

use super::{string, write_frame};
use crate::{
    app::YaacApp,
    file_pane::{self, Document, FileTree, TabKind},
    preview_controller::{BrowserArea, BrowserTab},
};

const CHUNK_BYTES: usize = 24 * 1024;

pub(super) fn kind(tab: &BrowserTab) -> &'static str {
    match tab.kind {
        TabKind::Browser => "browser",
        TabKind::Terminal(_) => "terminal",
        TabKind::Files => "files",
        TabKind::File(_) => "file",
        TabKind::Diff(_) => "diff",
    }
}

pub(super) fn reason(client: &Client, thread: Option<&ThreadKey>) -> Option<&'static str> {
    match thread {
        None => Some("Open a thread to use Files."),
        Some(key) if !file_pane::supported(client, key) => {
            Some("Worktree files are unavailable on this target.")
        }
        _ => None,
    }
}

#[derive(Default)]
pub(super) struct FilesPipe {
    serial: u64,
    current: Option<Transfer>,
    worker: Option<mpsc::Receiver<io::Result<String>>>,
    worker_id: u64,
    change: Option<(u64, usize, usize)>,
}

struct Transfer {
    id: u64,
    tab: u64,
    document: Arc<Document>,
    side_by_side: bool,
    wire: Option<String>,
    offset: usize,
    failed: bool,
}

impl FilesPipe {
    pub(super) fn prepare(
        &mut self,
        area: Option<&BrowserArea>,
        side_by_side: bool,
        wake: impl Fn() + Send + 'static,
    ) -> io::Result<()> {
        let desired = area
            .and_then(|area| area.tabs.iter().find(|tab| tab.id == area.active))
            .and_then(|tab| Some((tab.id, tab.document.as_ref()?)))
            .filter(|(_, document)| !matches!(document.as_ref(), Document::Notice(_)));
        if !self
            .current
            .as_ref()
            .zip(desired)
            .is_some_and(|(current, (tab, document))| {
                tab == current.tab
                    && Arc::ptr_eq(&current.document, document)
                    && (!matches!(document.as_ref(), Document::Diff(_))
                        || current.side_by_side == side_by_side)
            })
        {
            self.current = desired.map(|(tab, document)| {
                self.serial += 1;
                Transfer {
                    id: self.serial,
                    tab,
                    document: document.clone(),
                    side_by_side,
                    wire: None,
                    offset: 0,
                    failed: false,
                }
            });
        }
        if let Some(rx) = &self.worker {
            let result = match rx.try_recv() {
                Ok(result) => result,
                Err(mpsc::TryRecvError::Empty) => return Ok(()),
                Err(mpsc::TryRecvError::Disconnected) => {
                    Err(io::Error::other("File document worker disconnected"))
                }
            };
            self.worker = None;
            if let Some(current) = self
                .current
                .as_mut()
                .filter(|value| value.id == self.worker_id)
            {
                match result {
                    Ok(text) => current.wire = Some(text),
                    Err(_) => current.failed = true,
                }
            }
        }
        if let Some(current) = self
            .current
            .as_ref()
            .filter(|value| value.id != self.worker_id)
        {
            let document = current.document.clone();
            let (tx, rx) = mpsc::channel();
            self.worker_id = current.id;
            std::thread::Builder::new()
                .name("yaac-wpf-file".into())
                .spawn(move || {
                    let _ = tx.send(encode(&document, side_by_side));
                    wake();
                })?;
            self.worker = Some(rx);
        }
        Ok(())
    }

    pub(super) fn write(&mut self, stdout: &mut impl Write) -> io::Result<bool> {
        if let Some((id, row, ordinal)) = self.change.take() {
            write_frame(stdout, &json!({ "filesChange": { "id": id, "row": row, "ordinal": ordinal } }))?;
        }
        let Some(current) = &mut self.current else {
            return Ok(false);
        };
        let Some(text) = &current.wire else {
            return Ok(false);
        };
        let start = current.offset;
        let mut end = (start + CHUNK_BYTES).min(text.len());
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        write_frame(
            stdout,
            &json!({ "filesDocument": {
            "id": current.id, "start": start == 0, "end": end == text.len(), "chunk": &text[start..end],
        } }),
        )?;
        current.offset = end;
        if end == text.len() {
            current.wire = None;
        }
        Ok(current.wire.is_some())
    }

    pub(super) fn view(&self, area: Option<&BrowserArea>, side_by_side: bool) -> Value {
        let Some(area) = area else { return Value::Null };
        let Some(tab) = area.tabs.iter().find(|tab| tab.id == area.active) else {
            return Value::Null;
        };
        match &tab.kind {
            TabKind::Browser | TabKind::Terminal(_) => Value::Null,
            TabKind::Files => {
                let tree = &area.tree;
                let root = tree.folders.get("");
                let status = tree
                    .errors
                    .get("")
                    .map(String::as_str)
                    .or_else(|| match root {
                        None => Some("Loading…"),
                        Some(root) if root.truncated => {
                            Some("Only the first 5,000 entries are shown.")
                        }
                        Some(root)
                            if root
                                .entries
                                .iter()
                                .all(|entry| entry.ignored && !tree.show_ignored) =>
                        {
                            Some("No files to show.")
                        }
                        _ => None,
                    });
                let mut folders = BTreeMap::new();
                visible_folders(tree, "", &mut folders);
                json!({ "status": status, "showIgnored": tree.show_ignored, "canCollapse": !tree.expanded.is_empty(), "folders": folders, "errors": tree.errors })
            }
            TabKind::File(_) | TabKind::Diff(_) => {
                let current = self.current.as_ref().filter(|value| value.tab == tab.id);
                let status = tab
                    .error
                    .as_deref()
                    .or_else(|| match tab.document.as_deref() {
                        Some(Document::Notice(message)) => Some(message.as_str()),
                        _ if current.is_some_and(|value| value.failed) => {
                            Some("Could not prepare the document.")
                        }
                        _ => None,
                    });
                let indices = match tab.document.as_deref() {
                    Some(Document::Diff(diff)) => {
                        diff_rows(diff, side_by_side).change_indices.as_slice()
                    }
                    _ => &[],
                };
                json!({
                    "status": status, "sideBySide": side_by_side, "changes": !indices.is_empty(),
                    "changeCount": indices.len(), "firstChange": indices.first(),
                    "document": current.filter(|value| !value.failed).map(|value| value.id),
                })
            }
        }
    }

    pub(super) fn command(
        &mut self,
        app: &mut YaacApp,
        key: &ThreadKey,
        id: Option<u64>,
        command: &str,
        data: &Value,
        payload: &Value,
    ) -> Result<bool, String> {
        let browsers = &mut app.browsers;
        if command == "files.new" {
            browsers.open_file_tab(&mut app.client, key, TabKind::Files, false)?;
            return Ok(true);
        }
        let Some(area) = browsers
            .areas
            .get_mut(key)
            .filter(|area| Some(area.active) == id)
        else {
            return Ok(false);
        };
        let id = area.active;
        let Some(tab) = area.tabs.iter().find(|tab| tab.id == id) else {
            return Ok(false);
        };
        match command {
            "files.expand" | "files.open" | "files.diff" if tab.kind == TabKind::Files => {
                let path = string(data, "path");
                let Some(entry) = visible_entry(&area.tree, path) else {
                    return Ok(false);
                };
                match command {
                    "files.expand" if entry.kind == FsTreeEntryKind::Dir => {
                        area.tree.expand(path, data["expanded"] == true);
                    }
                    "files.open" | "files.diff" if entry.kind != FsTreeEntryKind::Dir => {
                        let kind = if command == "files.diff" {
                            if entry.git.is_none() {
                                return Ok(false);
                            }
                            TabKind::Diff(path.to_owned())
                        } else {
                            TabKind::File(path.to_owned())
                        };
                        browsers.open_file_tab(
                            &mut app.client,
                            key,
                            kind,
                            data["duplicate"] == true,
                        )?;
                    }
                    _ => return Ok(false),
                }
            }
            "files.refresh" if tab.kind == TabKind::Files => area.tree.refresh(),
            "files.collapse" if tab.kind == TabKind::Files => area.tree.collapse_all(),
            "files.ignored" if tab.kind == TabKind::Files => {
                area.tree.show_ignored = data["show"] == true
            }
            "files.reload" if tab.kind.path().is_some() => {
                browsers.reload_file(&mut app.client, key, id)
            }
            "files.mode" if matches!(tab.kind, TabKind::Diff(_)) => {
                browsers.side_by_side = data["sideBySide"] == true
            }
            "files.change" | "files.add" => {
                let Some(current) = self.current.as_ref().filter(|current| {
                    current.tab == id
                        && Some(current.id) == data["document"].as_u64()
                        && tab
                            .document
                            .as_ref()
                            .is_some_and(|doc| Arc::ptr_eq(doc, &current.document))
                        && (!matches!(tab.kind, TabKind::Diff(_))
                            || current.side_by_side == browsers.side_by_side)
                }) else {
                    return Ok(false);
                };
                if command == "files.change" {
                    let Document::Diff(diff) = current.document.as_ref() else {
                        return Ok(false);
                    };
                    let display = diff_rows(diff, current.side_by_side);
                    if let Some(row) =
                        next_change(display, number(data, "row"), data["forward"] == true)
                    {
                        let ordinal = display.change_indices.partition_point(|&start| start <= row);
                        self.change = Some((current.id, row, ordinal));
                    }
                } else {
                    let Some(path) = tab.kind.path() else {
                        return Ok(false);
                    };
                    let Some(excerpt) = excerpt(path, current, data) else {
                        return Ok(false);
                    };
                    let draft = DraftRef::Thread(key.clone());
                    let caret = app.client.draft(&draft).map_or(0, |draft| {
                        scalar_caret(&draft.text, number(payload, "caret").unwrap_or(usize::MAX))
                    });
                    if !file_pane::insert_excerpt(
                        &mut app.client,
                        key,
                        caret,
                        excerpt.label,
                        excerpt.text,
                    ) {
                        return Err("The composer is unavailable for this thread.".into());
                    }
                    browsers.set_full_screen(key, false);
                    app.ui.focus_composer = true;
                }
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
}

fn visible_entry<'a>(tree: &'a FileTree, path: &str) -> Option<&'a yaac_protocol::FsTreeEntry> {
    let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
    if !dir.is_empty() && !tree.expanded.contains(dir) {
        return None;
    }
    tree.folders
        .get(dir)?
        .entries
        .iter()
        .find(|entry| entry.name == name && (tree.show_ignored || !entry.ignored))
}

fn visible_folders(tree: &FileTree, path: &str, folders: &mut BTreeMap<String, Vec<Value>>) {
    let Some(folder) = tree.folders.get(path) else {
        return;
    };
    let rows = folder
        .entries
        .iter()
        .filter(|entry| tree.show_ignored || !entry.ignored)
        .map(|entry| {
            let directory = entry.kind == FsTreeEntryKind::Dir;
            let full = if path.is_empty() {
                entry.name.clone()
            } else {
                format!("{path}/{}", entry.name)
            };
            let expanded = directory && tree.expanded.contains(&full);
            if expanded {
                visible_folders(tree, &full, folders);
            }
            json!({ "name": entry.name, "directory": directory, "mark": entry.git,
            "ignored": entry.ignored, "changed": entry.changed, "expanded": expanded })
        })
        .collect();
    folders.insert(path.to_owned(), rows);
}

fn next_change(display: &DiffView, row: Option<usize>, forward: bool) -> Option<usize> {
    let changes = &display.change_indices;
    match (row, forward) {
        (None, true) => changes.first().copied(),
        (None, false) => changes.last().copied(),
        (Some(row), true) => changes
            .get(changes.partition_point(|&change| change <= row))
            .copied()
            .or_else(|| changes.first().copied()),
        (Some(row), false) => changes
            .partition_point(|&change| change < row)
            .checked_sub(1)
            .and_then(|index| changes.get(index))
            .copied()
            .or_else(|| changes.last().copied()),
    }
}

fn diff_rows(diff: &Diff, side_by_side: bool) -> &DiffView {
    if side_by_side {
        &diff.side_by_side
    } else {
        &diff.unified
    }
}

fn number(value: &Value, field: &str) -> Option<usize> {
    value[field].as_u64().and_then(|n| usize::try_from(n).ok())
}

pub(super) fn scalar_caret(text: &str, utf16: usize) -> usize {
    let mut end = 0;
    text.chars()
        .take_while(|c| {
            end += c.len_utf16();
            end <= utf16
        })
        .count()
}

fn selection_column(text: &str, column: Option<usize>, fallback: usize) -> usize {
    column.map_or(fallback, |column| {
        text.char_indices()
            .nth(scalar_caret(text, column))
            .map_or(text.len(), |(byte, _)| byte)
    })
}

fn excerpt(path: &str, transfer: &Transfer, data: &Value) -> Option<Excerpt> {
    let first = number(data, "first")?;
    let last = number(data, "last")?;
    if first > last {
        return None;
    }
    match transfer.document.as_ref() {
        Document::File(file) => {
            let start = file.lines.get(first)?.range.start;
            let end = file.lines.get(last)?.range.start;
            let a = start + selection_column(file.line_text(first), number(data, "firstColumn"), 0);
            let last_text = file.line_text(last);
            let b = end + selection_column(last_text, number(data, "lastColumn"), last_text.len());
            let text = file.text.get(a..b)?;
            let language = file
                .language
                .as_ref()
                .map(|language| language.fence.as_str());
            Some(yaac_code::file_excerpt(
                path,
                language,
                first + 1..=last + 1,
                text,
            ))
        }
        Document::Diff(diff) => {
            diff_selection(path, diff, transfer.side_by_side, first, last, data)
        }
        Document::Notice(_) => None,
    }
}

fn diff_selection(
    path: &str,
    diff: &Diff,
    paired: bool,
    first: usize,
    last: usize,
    data: &Value,
) -> Option<Excerpt> {
    let side = string(data, "side");
    if !(if paired {
        matches!(side, "old" | "new")
    } else {
        side == "unified"
    }) {
        return None;
    }
    let rows = diff_rows(diff, paired).rows.get(first..=last)?;
    let first_offset = number(data, "firstOffset")?;
    let last_offset = number(data, "lastOffset")?;
    let folded: HashSet<_> = data["folded"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| row.as_u64().and_then(|row| usize::try_from(row).ok()))
        .collect();
    let mut lines = Vec::new();
    let mut low = usize::MAX;
    let mut high = 0;
    for (index, row) in rows.iter().enumerate() {
        let (old, new, count) = match *row {
            DiffRow::Line { old, new } => (old, new, 1),
            DiffRow::Fold {
                old_start,
                new_start,
                count,
            } => {
                if folded.contains(&(first + index)) {
                    continue;
                }
                (Some(old_start), Some(new_start), count)
            }
        };
        let start = if index == 0 { first_offset } else { 0 };
        let end = if index + 1 == rows.len() {
            last_offset
        } else {
            count - 1
        };
        if start > end || end >= count {
            return None;
        }
        for offset in start..=end {
            let old = old.map(|line| line + offset);
            let new = new.map(|line| line + offset);
            let replaced = paired
                && (old.is_some_and(|line| diff.old_inline.contains_key(&line))
                    || new.is_some_and(|line| diff.new_inline.contains_key(&line)));
            let (line, kind, text) = match (side, old, new) {
                ("old", Some(line), other) | ("unified", Some(line), other @ None) => {
                    let kind = if other.is_none() || replaced {
                        DiffLineKind::Removed
                    } else {
                        DiffLineKind::Context
                    };
                    (line, kind, diff.old.line_text(line))
                }
                ("new" | "unified", other, Some(line)) => {
                    let kind = if other.is_none() || replaced {
                        DiffLineKind::Added
                    } else {
                        DiffLineKind::Context
                    };
                    (line, kind, diff.new.line_text(line))
                }
                _ => continue,
            };
            let start = if index == 0 && offset == first_offset {
                selection_column(text, number(data, "firstColumn"), 0)
            } else {
                0
            };
            let end = if index + 1 == rows.len() && offset == last_offset {
                selection_column(text, number(data, "lastColumn"), text.len())
            } else {
                text.len()
            };
            let text = text.get(start..end)?;
            low = low.min(line + 1);
            high = high.max(line + 1);
            lines.push((kind, text));
        }
    }
    (!lines.is_empty()).then(|| yaac_code::diff_excerpt(path, low..=high, lines))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WireDocument {
    file: Option<CodeSource>,
    old: Option<CodeSource>,
    new: Option<CodeSource>,
    rows: Option<Vec<(Option<usize>, Option<usize>, usize)>>,
    side_by_side: bool,
}

#[derive(Serialize)]
struct CodeSource {
    text: String,
    styles: BTreeMap<usize, CodeStyle>,
}

#[derive(Serialize)]
struct CodeStyle {
    spans: Vec<CodeSpan>,
    words: Option<Vec<(usize, usize)>>,
}

#[derive(Serialize)]
struct CodeSpan {
    start: usize,
    end: usize,
    scope: &'static str,
}

fn encode(document: &Document, side_by_side: bool) -> io::Result<String> {
    let mut data = WireDocument {
        file: None,
        old: None,
        new: None,
        rows: None,
        side_by_side,
    };
    match document {
        Document::File(file) => data.file = Some(source(file, None)),
        Document::Diff(diff) => {
            data.old = Some(source(&diff.old, Some(&diff.old_inline)));
            data.new = Some(source(&diff.new, Some(&diff.new_inline)));
            data.rows = Some(
                diff_rows(diff, side_by_side)
                    .rows
                    .iter()
                    .map(|row| match *row {
                        DiffRow::Line { old, new } => (old, new, 0),
                        DiffRow::Fold {
                            old_start,
                            new_start,
                            count,
                        } => (Some(old_start), Some(new_start), count),
                    })
                    .collect(),
            );
        }
        Document::Notice(_) => return Err(io::Error::other("A notice has no document")),
    }
    Ok(serde_json::to_string(&data)?)
}

fn source(file: &Highlighted, inline: Option<&BTreeMap<usize, Vec<Range<usize>>>>) -> CodeSource {
    let mut text = String::with_capacity(file.text.len());
    let mut styles = BTreeMap::new();
    for (index, line) in file.lines.iter().enumerate() {
        if index != 0 {
            text.push('\n')
        }
        let raw = file.line_text(index);
        // AvalonEdit sees a bare CR as another line. Keep it visible in its original column.
        if raw.contains('\r') {
            text.extend(raw.chars().map(|c| if c == '\r' { '␍' } else { c }));
        } else {
            text.push_str(raw);
        }
        let words = inline.and_then(|words| words.get(&index));
        if line.spans.is_empty() && words.is_none() {
            continue;
        }
        let spans = utf16_ranges(raw, line.spans.iter().map(|span| &span.range))
            .zip(&line.spans)
            .map(|((start, end), span)| CodeSpan {
                start,
                end,
                scope: span.class.as_str(),
            })
            .collect();
        let words = words.map(|words| utf16_ranges(raw, words.iter()).collect());
        styles.insert(index, CodeStyle { spans, words });
    }
    if inline.is_none() && file.text.ends_with('\n') {
        text.push('\n')
    }
    CodeSource { text, styles }
}

fn utf16_ranges<'a>(
    text: &'a str,
    ranges: impl Iterator<Item = &'a Range<usize>> + 'a,
) -> impl Iterator<Item = (usize, usize)> + 'a {
    let (mut byte, mut end) = (0, 0);
    ranges.map(move |range| {
        let start = if range.start >= byte {
            end + text[byte..range.start].encode_utf16().count()
        } else {
            text[..range.start].encode_utf16().count()
        };
        end = start + text[range.clone()].encode_utf16().count();
        byte = range.end;
        (start, end)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_chunks_round_trip_multibyte_text() {
        let payload = format!(
            "{}😀{}",
            "x".repeat(CHUNK_BYTES - 1),
            "y".repeat(CHUNK_BYTES)
        );
        let mut files = FilesPipe {
            current: Some(Transfer {
                id: 1,
                tab: 2,
                document: Arc::new(Document::Notice("test".into())),
                side_by_side: false,
                wire: Some(payload.clone()),
                offset: 0,
                failed: false,
            }),
            ..Default::default()
        };
        let mut pipe = Vec::new();
        while files.write(&mut pipe).unwrap() {}
        let frames: Vec<Value> = str::from_utf8(&pipe)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[0]["filesDocument"]["start"], true);
        assert_eq!(frames[2]["filesDocument"]["end"], true);
        let full: String = frames
            .iter()
            .map(|frame| frame["filesDocument"]["chunk"].as_str().unwrap())
            .collect();
        assert_eq!(payload, full);
    }

    #[test]
    fn wpf_offsets_count_utf16_units() {
        let text = "x😀éz";
        let ranges = [1..5, 5..7, 7..8];
        assert_eq!(
            utf16_ranges(text, ranges.iter()).collect::<Vec<_>>(),
            [(1, 3), (3, 4), (4, 5)]
        );
        assert_eq!(scalar_caret(text, 3), 2);
    }

    #[test]
    fn file_selection_becomes_a_removable_composer_chip() {
        let ctx = eframe::egui::Context::default();
        let mut app = crate::app::tests::demo_app(&ctx);
        let key = yaac_core::demo::threads::users();
        app.client.open(&key);
        app.client.pump();
        app.client.state.drafts.entry(key.clone()).or_default().text = "😀go".into();
        let tab = app.browsers.insert(&key);
        let document = Arc::new(Document::File(Highlighted {
            text: "a😀b\ncafé".into(),
            language: None,
            lines: vec![
                yaac_code::Line {
                    range: 0..6,
                    spans: vec![],
                },
                yaac_code::Line {
                    range: 7..12,
                    spans: vec![],
                },
            ],
        }));
        let view = app.browsers.tab_mut(&key, tab).unwrap();
        view.kind = TabKind::File("src/main.rs".into());
        view.document = Some(document.clone());
        app.browsers.areas.get_mut(&key).unwrap().full_screen = true;
        let mut pipe = FilesPipe {
            current: Some(Transfer {
                id: 1,
                tab,
                document,
                side_by_side: false,
                wire: None,
                offset: 0,
                failed: false,
            }),
            ..Default::default()
        };
        let data =
            json!({ "document": 1, "first": 0, "firstColumn": 1, "last": 1, "lastColumn": 3 });
        assert!(
            pipe.command(
                &mut app,
                &key,
                Some(tab),
                "files.add",
                &data,
                &json!({ "caret": 2 })
            )
            .unwrap()
        );
        let draft = DraftRef::Thread(key.clone());
        let selection = &app.client.draft(&draft).unwrap().excerpts[0];
        assert_eq!(selection.label, "main.rs:1–2");
        assert!(selection.formatted.contains("😀b\ncaf\n"));
        assert_eq!(
            app.client.draft(&draft).unwrap().text,
            format!("😀{}go", selection.token)
        );
        assert!(!app.browsers.areas[&key].full_screen);

        let action = format!("removeExcerpt:{}", selection.token);
        super::super::composer::command(
            &mut app,
            "composer.action",
            &json!({ "key": draft.store_key(), "id": action }),
        )
        .unwrap();
        let value = app.client.draft(&draft).unwrap();
        assert_eq!(value.text, "😀go");
        assert!(value.excerpts.is_empty());
    }
}
