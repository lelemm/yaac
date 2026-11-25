//! egui presentation of the shared thread worktree pane.
use crate::{
    app::YaacApp,
    file_pane::{Document, FileTree, TabKind},
    icons,
    theme::{self, Palette, size},
    widgets,
};
use eframe::egui::{self, Color32, RichText, Ui};
use std::{collections::BTreeMap, ops::Range};
use yaac_code::{DiffLineKind as Kind, DiffRow, DiffView, Highlighted, ScopeClass};
use yaac_core::{ThreadKey, attachments::DraftRef};
use yaac_protocol::{FsTreeEntry, FsTreeEntryKind};

pub(crate) fn draw(ui: &mut Ui, app: &mut YaacApp, key: &ThreadKey, id: u64) {
    let p = app.p;
    let area = app.browsers.areas.get_mut(key).unwrap();
    let tab = area.tabs.iter().find(|t| t.id == id).unwrap();
    let kind = tab.kind.clone();
    let doc = tab.document.clone();
    let error = tab.error.clone();
    let loading = tab.loading;
    let initial_side = app.browsers.side_by_side;
    let state_id = ui
        .scope_id()
        .with((id, tab.generation, app.browsers.side_by_side));
    let mut state = ui
        .data_mut(|d| d.get_temp::<Viewer>(state_id))
        .unwrap_or_default();
    let mut jump = None;
    // Shortcuts while nothing else holds the keyboard, or the viewer does.
    let free = ui.memory(|m| m.focused().is_none_or(|f| Some(f) == app.preview_ui.file_focus));
    let (mut reload, mut next, mut prev) = (false, false, false);
    if free {
        ui.input_mut(|i| {
            reload = i.consume_key(egui::Modifiers::NONE, egui::Key::F5);
            prev = i.consume_key(egui::Modifiers::SHIFT, egui::Key::F7);
            next = i.consume_key(egui::Modifiers::NONE, egui::Key::F7);
        });
    }
    let (_, rest) = crate::preview_ui::toolbar(ui, &p, |ui| {
        let reload_button = |ui: &mut Ui, enabled: bool| {
            widgets::tool_button(ui, &p, p.bg, "Reload (F5)", enabled, false, icons::reload).clicked()
        };
        match &kind {
            TabKind::Files => {
                let on = area.tree.show_ignored;
                if widgets::tool_button(ui, &p, p.bg, if on { "Hide ignored files" } else { "Show ignored files" }, true, on, icons::eye).clicked() {
                    area.tree.show_ignored = !on;
                }
                if widgets::tool_button(ui, &p, p.bg, "Refresh (F5)", true, false, icons::reload).clicked() || reload {
                    area.tree.refresh();
                    ui.ctx().request_repaint();
                }
                if widgets::tool_button(ui, &p, p.bg, "Collapse all", !area.tree.expanded.is_empty(), false, icons::collapse_all).clicked() {
                    area.tree.collapse_all();
                }
                reload = false;
            }
            TabKind::File(path) => {
                ui.add_space(6.);
                let w = (ui.available_width() - 28. - 10.).max(0.);
                path_label(ui, &p, path, w);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    reload |= reload_button(ui, !loading);
                });
            }
            TabKind::Diff(path) => {
                let counts = if let Some(Document::Diff(diff)) = doc.as_deref() {
                    let view = if app.browsers.side_by_side { &diff.side_by_side } else { &diff.unified };
                    Some(view.change_indices.clone())
                } else {
                    None
                };
                let n = counts.as_ref().map_or(0, Vec::len);
                // The counter starts at the first change.
                if state.change.is_none() && n > 0 {
                    state.change = Some(0);
                }
                let counter = match (state.change, counts.is_some()) {
                    (_, false) => String::new(),
                    (Some(i), true) => format!("{} of {n}", i + 1),
                    (None, true) => format!("0 of {n}"),
                };
                let font = theme::ui(size::SMALL);
                let counter_w = ui.fonts_mut(|f| f.layout_no_wrap(counter.clone(), font.clone(), p.text2)).size().x + 10.;
                let switch_w = segmented_width(ui, "Unified", "Side by side");
                let right = switch_w + 6. + 28. * 3. + counter_w + 4. + 2. * 5.;
                ui.add_space(4.);
                let path_w = ui.available_width() - right - 6.;
                if path_w > 12. {
                    path_label(ui, &p, path, path_w);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 2.;
                    reload |= reload_button(ui, !loading);
                    ui.add_space(4.);
                    next |= widgets::tool_button(ui, &p, p.bg, "Next change (F7)", n > 0, false, |pa, c, d, col| icons::arrow_vertical(pa, c, d, col, false)).clicked();
                    ui.add_sized([counter_w, 28.], egui::Label::new(egui::RichText::new(&counter).font(font.clone()).color(p.text2)).selectable(false));
                    prev |= widgets::tool_button(ui, &p, p.bg, "Previous change (Shift+F7)", n > 0, false, |pa, c, d, col| icons::arrow_vertical(pa, c, d, col, true)).clicked();
                    ui.add_space(6.);
                    if let Some(side) = segmented(ui, &p, "Unified", "Side by side", app.browsers.side_by_side) {
                        app.browsers.side_by_side = side;
                    }
                });
                if let Some(indices) = counts.filter(|v| !v.is_empty()) {
                    let n = indices.len();
                    for forward in [prev.then_some(false), next.then_some(true)].into_iter().flatten() {
                        let i = match (state.change, forward) {
                            (None, true) => 0,
                            (None, false) => n - 1,
                            (Some(i), true) => (i + 1) % n,
                            (Some(i), false) => (i + n - 1) % n,
                        };
                        state.change = Some(i);
                        jump = Some(state.display_index(indices[i]));
                        ui.ctx().request_repaint();
                    }
                }
            }
            TabKind::Browser | TabKind::Terminal(_) => {}
        }
    });
    if app.browsers.side_by_side != initial_side {
        ui.ctx().request_repaint();
        return;
    }
    if reload {
        app.browsers.reload_file(&mut app.client, key, id);
        ui.ctx().request_repaint();
        return;
    }
    ui.scope_builder(egui::UiBuilder::new().max_rect(rest.shrink2(egui::vec2(0., 2.))), |ui| {
        body(ui, app, key, &kind, doc, error.as_deref(), &mut state, jump);
    });
    ui.data_mut(|d| d.insert_temp(state_id, state));
}

/// A worktree path in the secondary colour, cut at its start to fit.
fn path_label(ui: &mut Ui, p: &Palette, path: &str, width: f32) {
    let g = widgets::elided_start(ui, path, &theme::ui(size::SMALL), p.text2, width);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(g.size().x, 28.), egui::Sense::hover());
    ui.painter().galley(egui::pos2(rect.left(), rect.center().y - g.size().y / 2.), g, p.text2);
    resp.on_hover_text(path);
}

fn segmented_width(ui: &Ui, left: &str, right: &str) -> f32 {
    let font = theme::ui(size::SMALL);
    let w = |s: &str| ui.fonts_mut(|f| f.layout_no_wrap(s.to_owned(), font.clone(), Color32::WHITE)).size().x + 16.;
    w(left) + w(right) + 4.
}

/// A two-segment switch on a quiet track; returns the new choice when one is clicked.
fn segmented(ui: &mut Ui, p: &Palette, left: &str, right: &str, on_right: bool) -> Option<bool> {
    let font = theme::ui(size::SMALL);
    let total = segmented_width(ui, left, right);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(total, 26.), egui::Sense::hover());
    ui.painter().rect_filled(rect, 6., widgets::blend(p.bg, p.selected, 0.7));
    let mut x = rect.left() + 2.;
    let mut pick = None;
    for (label, right_side) in [(left, false), (right, true)] {
        let g = ui.fonts_mut(|f| f.layout_no_wrap(label.to_owned(), font.clone(), p.text));
        let seg = egui::Rect::from_min_size(egui::pos2(x, rect.top() + 2.), egui::vec2(g.size().x + 16., rect.height() - 4.));
        x = seg.right();
        let resp = ui.interact(seg, ui.scope_id().with(("segment", label)), egui::Sense::click());
        let selected = on_right == right_side;
        if selected {
            ui.painter().rect_filled(seg, 5., if p.dark { p.multi } else { p.bg });
        } else if resp.hovered() {
            ui.painter().rect_filled(seg, 5., widgets::blend(p.bg, p.selected, 0.9));
        }
        let ink = if selected || resp.hovered() { p.text } else { p.text2 };
        ui.painter().galley(seg.center() - g.size() / 2., g, ink);
        if resp.has_focus() {
            ui.painter().rect_stroke(seg, 5., egui::Stroke::new(2., p.focus), egui::StrokeKind::Inside);
        }
        if resp.clicked() && !selected {
            pick = Some(right_side);
        }
    }
    pick
}

#[allow(clippy::too_many_arguments)]
fn body(ui: &mut Ui, app: &mut YaacApp, key: &ThreadKey, kind: &TabKind, doc: Option<std::sync::Arc<Document>>, error: Option<&str>, state: &mut Viewer, jump: Option<usize>) {
    let p = app.p;
    if *kind == TabKind::Files {
        let mut open = None;
        let mut changes = vec![];
        let area = app.browsers.areas.get(key).unwrap();
        let mut rows = Vec::new();
        flatten(&area.tree, "", 0, &mut rows);
        let notes: Vec<(String, Color32)> = area
            .tree
            .errors
            .iter()
            .map(|(path, e)| (format!("{path} {e}"), p.failed))
            .chain(
                area.tree
                    .folders
                    .values()
                    .any(|f| f.truncated)
                    .then(|| ("Listing limited to 5,000 entries per folder.".to_owned(), p.text2)),
            )
            .collect();
        if rows.is_empty() && area.tree.folders.is_empty() && notes.is_empty() {
            ui.add_space(12.);
            ui.horizontal(|ui| {
                ui.add_space(14.);
                widgets::icon(ui, 14., |pa, c, d| icons::spinner(pa, c, d, p.text2));
                ui.label(widgets::secondary("Loading files…", &p));
            });
            return;
        }
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show_rows(ui, ROW_H, rows.len() + notes.len(), |ui, range| {
                ui.spacing_mut().item_spacing.y = 0.;
                for i in range {
                    if let Some(row) = rows.get(i) {
                        tree_row(ui, &p, &area.tree, row, state, &mut changes, &mut open);
                    } else if let Some((text, color)) = notes.get(i - rows.len()) {
                        ui.add_sized([ui.available_width(), ROW_H], egui::Label::new(RichText::new(text).font(theme::ui(size::SMALL)).color(*color)).truncate());
                    }
                }
            });
        for (path, on) in changes {
            app.browsers.areas.get_mut(key).unwrap().tree.expand(&path, on);
            ui.ctx().request_repaint();
        }
        if let Some((kind, duplicate)) = open {
            let _ = app
                .browsers
                .open_file_tab(&mut app.client, key, kind, duplicate);
            ui.ctx().request_repaint();
        }
    } else if let Some(e) = error {
        ui.add_space(12.);
        ui.horizontal(|ui| {
            ui.add_space(14.);
            ui.colored_label(p.failed, e);
        });
    } else if let Some(doc) = doc {
        if let Document::Notice(s) = doc.as_ref() {
            ui.add_space(12.);
            ui.horizontal(|ui| {
                ui.add_space(14.);
                ui.label(widgets::secondary(s.clone(), &p));
            });
        } else {
            let path = kind.path().unwrap();
            app.preview_ui.file_focus = Some(ui.scope_id().with(("selection", path)));
            let side = app.browsers.side_by_side && matches!(doc.as_ref(), Document::Diff(_));
            let view = Layout { doc: &doc, side };
            if let Some(excerpt) = viewer(ui, &view, state, jump, &p, path) {
                let draft = DraftRef::Thread(key.clone());
                let edit = crate::composer::composer_id(&draft);
                let mut cursor = egui::TextEdit::load_state(ui.ctx(), edit).unwrap_or_default();
                let at = cursor
                    .cursor
                    .char_range()
                    .map(|r| usize::from(r.primary.index))
                    .unwrap_or_else(|| {
                        app.client
                            .draft(&draft)
                            .map_or(0, |d| d.text.chars().count())
                    });
                if crate::file_pane::insert_excerpt(
                    &mut app.client,
                    key,
                    at,
                    excerpt.label,
                    excerpt.text,
                ) {
                    let len = app
                        .client
                        .draft(&draft)
                        .and_then(|d| d.excerpts.last())
                        .map_or(0, |e| e.token.chars().count());
                    cursor
                        .cursor
                        .set_char_range(Some(egui::text::CCursorRange::one(
                            egui::text::CCursor::new(at + len),
                        )));
                    cursor.store(ui.ctx(), edit);
                    app.browsers.set_full_screen(key, false);
                    app.ui.focus_composer = true;
                }
            }
        }
    } else {
        ui.add_space(12.);
        ui.horizontal(|ui| {
            ui.add_space(14.);
            widgets::icon(ui, 14., |pa, c, d| icons::spinner(pa, c, d, p.text2));
            ui.label(widgets::secondary("Loading…", &p));
        });
    }
}

const ROW_H: f32 = 24.;
const INDENT: f32 = 16.;

struct TreeRow<'a> {
    depth: usize,
    path: String,
    entry: &'a FsTreeEntry,
}

/// The visible rows: folders first then files (as listed), expanded folders' children under
/// them, ignored entries only when shown.
fn flatten<'a>(tree: &'a FileTree, dir: &str, depth: usize, out: &mut Vec<TreeRow<'a>>) {
    let Some(f) = tree.folders.get(dir) else {
        return;
    };
    for e in &f.entries {
        if e.ignored && !tree.show_ignored {
            continue;
        }
        let path = if dir.is_empty() {
            e.name.clone()
        } else {
            format!("{dir}/{}", e.name)
        };
        let open = e.kind == FsTreeEntryKind::Dir && tree.expanded.contains(&path);
        out.push(TreeRow { depth, path: path.clone(), entry: e });
        if open {
            flatten(tree, &path, depth + 1, out);
        }
    }
}

/// One tree row: a closed or open folder, or a file; the name; the git mark after it (a
/// click on the mark opens the Diff tab) or a folder's change dot.
fn tree_row(
    ui: &mut Ui,
    p: &Palette,
    tree: &FileTree,
    row: &TreeRow<'_>,
    state: &mut Viewer,
    changes: &mut Vec<(String, bool)>,
    open: &mut Option<(TabKind, bool)>,
) {
    let e = row.entry;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW_H), egui::Sense::click());
    let row_rect = rect.shrink2(egui::vec2(4., 1.));
    let selected = state.selected.as_deref() == Some(row.path.as_str());
    if selected {
        ui.painter().rect_filled(row_rect, 4., p.selected);
    } else if resp.hovered() {
        ui.painter().rect_filled(row_rect, 4., widgets::hover_fill(p, p.bg));
    }
    if resp.has_focus() {
        ui.painter().rect_stroke(row_rect, 4., egui::Stroke::new(2., p.focus), egui::StrokeKind::Inside);
    }
    let ink = if e.ignored { p.unavailable } else { p.text };
    let icon_ink = if e.ignored { p.unavailable } else { p.text2 };
    let x = rect.left() + 14. + row.depth as f32 * INDENT;
    let c = egui::pos2(x + 8., rect.center().y);
    let dir = e.kind == FsTreeEntryKind::Dir;
    let expanded = dir && tree.expanded.contains(&row.path);
    if expanded {
        icons::folder_open(ui.painter(), c, 17., icon_ink);
    } else if dir {
        icons::folder(ui.painter(), c, 17., icon_ink);
    } else {
        icons::file(ui.painter(), c, 16., icon_ink, false);
    }
    let mark = e.git.as_ref().map(|g| (g, crate::preview_ui::git_color(p, g)));
    let mark_w = if mark.is_some() || e.changed { 22. } else { 0. };
    let name_x = x + 24.;
    let font = theme::ui(size::UI);
    let g = widgets::elided(ui, &e.name, &font, ink, (rect.right() - 8. - mark_w - name_x).max(10.));
    let name_w = g.size().x;
    ui.painter().galley(egui::pos2(name_x, rect.center().y - g.size().y / 2.), g, ink);
    let mut on_mark = false;
    if let Some((g, col)) = mark {
        let r = egui::Rect::from_center_size(egui::pos2(name_x + name_w + 14., rect.center().y), egui::vec2(18., 18.));
        let m = ui.interact(r, resp.id.with("mark"), egui::Sense::click()).on_hover_text("Show the diff");
        if m.hovered() {
            ui.painter().rect_filled(r, 4., widgets::hover_fill(p, p.bg));
        }
        ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, g.as_str(), theme::bold(size::SMALL), col);
        if m.clicked() || m.middle_clicked() {
            *open = Some((TabKind::Diff(row.path.clone()), m.middle_clicked()));
            state.selected = Some(row.path.clone());
            on_mark = true;
        }
    } else if e.changed {
        ui.painter().circle_filled(egui::pos2(name_x + name_w + 10., rect.center().y), 3., p.git_modified);
    }
    if on_mark {
        return;
    }
    if resp.clicked() || resp.middle_clicked() {
        state.selected = Some(row.path.clone());
        if dir {
            if resp.clicked() {
                changes.push((row.path.clone(), !expanded));
            }
        } else {
            *open = Some((TabKind::File(row.path.clone()), resp.middle_clicked()));
        }
    }
    resp.on_hover_text(&row.path);
}

#[derive(Default, Clone)]
struct Viewer {
    /// The tree's selected row (the last one clicked).
    selected: Option<String>,
    change: Option<usize>,
    // Original folded row -> number of extra displayed rows.
    expanded: BTreeMap<usize, usize>,
    selection: Option<Selection>,
}
#[derive(Clone, Copy)]
struct Selection {
    anchor: (usize, usize),
    end: (usize, usize),
    right: bool,
}
impl Viewer {
    fn display_index(&self, base: usize) -> usize {
        base + self.expanded.range(..base).map(|(_, n)| n).sum::<usize>()
    }
    fn base(&self, display: usize) -> (usize, usize) {
        let mut index = display;
        for (&base, &extra) in &self.expanded {
            if index <= base {
                break;
            }
            if index <= base + extra {
                return (base, index - base);
            }
            index -= extra;
        }
        (index, 0)
    }
}
struct Layout<'a> {
    doc: &'a Document,
    side: bool,
}
struct Cell<'a> {
    src: &'a Highlighted,
    index: usize,
    kind: Kind,
    words: &'a [Range<usize>],
}
impl<'a> Layout<'a> {
    fn diff(&self) -> Option<&DiffView> {
        match self.doc {
            Document::Diff(d) => Some(if self.side {
                &d.side_by_side
            } else {
                &d.unified
            }),
            _ => None,
        }
    }
    fn count(&self, state: &Viewer) -> usize {
        match self.doc {
            Document::File(f) => f.lines.len(),
            Document::Diff(_) => {
                self.diff().unwrap().rows.len() + state.expanded.values().sum::<usize>()
            }
            _ => 0,
        }
    }
    fn row(&self, state: &Viewer, row: usize) -> Option<DiffRow> {
        let (base, offset) = state.base(row);
        match *self.diff()?.rows.get(base)? {
            DiffRow::Fold {
                old_start,
                new_start,
                ..
            } if state.expanded.contains_key(&base) => Some(DiffRow::Line {
                old: Some(old_start + offset),
                new: Some(new_start + offset),
            }),
            other => Some(other),
        }
    }
    fn cell(&self, state: &Viewer, row: usize, right: bool) -> Option<Cell<'a>> {
        match self.doc {
            Document::File(src) => Some(Cell {
                src,
                index: row,
                kind: Kind::Context,
                words: &[],
            }),
            Document::Diff(d) => {
                let DiffRow::Line { old, new } = self.row(state, row)? else {
                    return None;
                };
                let use_new = if self.side { right } else { new.is_some() };
                let (src, index, inline, other, change) = if use_new {
                    (&d.new, new?, &d.new_inline, old, Kind::Added)
                } else {
                    (&d.old, old?, &d.old_inline, new, Kind::Removed)
                };
                let words = inline.get(&index);
                let changed = other.is_none() || (self.side && words.is_some());
                Some(Cell {
                    src,
                    index,
                    kind: if changed { change } else { Kind::Context },
                    words: words.map(Vec::as_slice).unwrap_or_default(),
                })
            }
            _ => None,
        }
    }
}
fn tint(p: &Palette, c: ScopeClass) -> Color32 {
    match c {
        ScopeClass::Comment | ScopeClass::Punctuation => p.text2,
        ScopeClass::String | ScopeClass::Attribute => p.git_added,
        ScopeClass::Function | ScopeClass::Link | ScopeClass::Tag => p.unseen,
        ScopeClass::Keyword | ScopeClass::Heading | ScopeClass::Type => p.git_modified,
        ScopeClass::Constant | ScopeClass::Number => p.git_deleted,
        _ => p.text,
    }
}

fn styled(cell: &Cell<'_>, selected: Option<Range<usize>>, p: &Palette) -> egui::text::LayoutJob {
    let s = cell.src.line_text(cell.index);
    let spans = &cell.src.lines[cell.index].spans;
    let mut edges = vec![0, s.len()];
    for r in spans
        .iter()
        .map(|s| &s.range)
        .chain(cell.words)
        .chain(selected.as_ref())
    {
        edges.extend([r.start.min(s.len()), r.end.min(s.len())]);
    }
    edges.sort_unstable();
    edges.dedup();
    let mut job = egui::text::LayoutJob::default();
    for pair in edges.windows(2) {
        let at = pair[0];
        let span = spans
            .get(spans.partition_point(|s| s.range.end <= at))
            .filter(|s| s.range.contains(&at));
        let word = cell
            .words
            .get(cell.words.partition_point(|s| s.end <= at))
            .is_some_and(|r| r.contains(&at));
        let color = span.map_or(p.text, |s| tint(p, s.class));
        let background = if selected.as_ref().is_some_and(|r| r.contains(&at)) {
            p.multi
        } else if word {
            (if cell.kind == Kind::Added {
                p.git_added
            } else {
                p.git_deleted
            })
            .gamma_multiply(0.35)
        } else {
            Color32::TRANSPARENT
        };
        job.append(
            &s[at..pair[1]],
            0.,
            egui::TextFormat {
                font_id: egui::FontId::monospace(13.),
                color,
                background,
                ..Default::default()
            },
        );
    }
    job
}
impl Selection {
    fn range(self, row: usize, len: usize) -> Option<Range<usize>> {
        let (a, b) = if self.anchor <= self.end {
            (self.anchor, self.end)
        } else {
            (self.end, self.anchor)
        };
        if row < a.0 || row > b.0 {
            return None;
        }
        Some(
            (if row == a.0 { a.1.min(len) } else { 0 })..(if row == b.0 {
                b.1.min(len)
            } else {
                len
            }),
        )
    }
}
fn viewer(
    ui: &mut Ui,
    layout: &Layout<'_>,
    state: &mut Viewer,
    jump: Option<usize>,
    p: &Palette,
    path: &str,
) -> Option<yaac_code::Excerpt> {
    // Compact rows at the mono line height, no gaps between them.
    ui.spacing_mut().item_spacing.y = 0.;
    let height = (ui.fonts_mut(|f| f.row_height(&theme::mono(size::MONO))) + 3.).round();
    let stride = height;
    // Horizontal rows are at least `interact_size.y` tall: make that the line height.
    ui.spacing_mut().interact_size.y = height;
    let mut scroll = egui::ScrollArea::vertical().id_salt(("code", path, layout.side));
    if let Some(row) = jump {
        scroll = scroll.vertical_scroll_offset(row as f32 * stride);
    }
    let mut add = false;
    let mut copy = false;
    let focus = ui.scope_id().with(("selection", path));
    ui.interact(
        ui.available_rect_before_wrap(),
        focus,
        egui::Sense::focusable_noninteractive(),
    );
    let mut expand = None;
    scroll.show_rows(ui, height, layout.count(state), |ui, rows| {
        // Share the vertical viewport and give each side its own horizontal scrollbar.
        let mut column = |ui: &mut Ui, right: bool| {
            egui::ScrollArea::horizontal()
                .id_salt(("source-column", right))
                .show(ui, |ui| {
                    for row in rows.clone() {
                        if let Some(DiffRow::Fold { count, .. }) = layout.row(state, row) {
                            if ui
                                .add_sized(
                                    [0., height],
                                    egui::Button::new(format!("⋯ {count} unchanged lines")),
                                )
                                .clicked()
                            {
                                expand = Some((state.base(row).0, count - 1));
                            }
                            continue;
                        }
                        let cell = layout.cell(state, row, right);
                        if let Some(cell) = cell.as_ref().filter(|c| c.kind != Kind::Context) {
                            // Added and removed lines on their tint, the full row wide.
                            let top = ui.cursor().top();
                            let clip = ui.clip_rect();
                            let r = egui::Rect::from_min_max(egui::pos2(clip.left(), top), egui::pos2(clip.right(), top + height));
                            ui.painter().rect_filled(r, 0., p.diff_ground(cell.kind == Kind::Added));
                        }
                        ui.horizontal(|ui| {
                            ui.set_min_height(height);
                            if let Some(cell) = cell {
                                let s = cell.src.line_text(cell.index);
                                let sign = match cell.kind {
                                    Kind::Added => "+",
                                    Kind::Removed => "−",
                                    Kind::Context => " ",
                                };
                                let color = match cell.kind {
                                    Kind::Added => p.git_added,
                                    Kind::Removed => p.git_deleted,
                                    Kind::Context => p.text2,
                                };
                                // Unified diffs number both sides: old, new, then the sign.
                                let number = |n: Option<usize>| n.map_or_else(|| " ".repeat(5), |n| format!("{:>5}", n + 1));
                                let gutter = match (layout.doc, layout.side, layout.row(state, row)) {
                                    (Document::Diff(_), false, Some(DiffRow::Line { old, new })) => {
                                        format!("{} {} {sign} ", number(old), number(new))
                                    }
                                    (Document::Diff(_), ..) => format!("{} {sign} ", number(Some(cell.index))),
                                    _ => format!("{} ", number(Some(cell.index))),
                                };
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(gutter)
                                            .font(theme::mono(size::MONO))
                                            .color(color),
                                    )
                                    .selectable(false)
                                    .extend(),
                                );
                                let range = state
                                    .selection
                                    .filter(|sel| sel.right == right)
                                    .and_then(|sel| sel.range(row, s.len()));
                                let job = styled(&cell, range, p);
                                let galley = ui.fonts_mut(|f| f.layout_job(job));
                                let (rect, r) = ui.allocate_exact_size(
                                    egui::vec2(galley.size().x.max(8.), height),
                                    egui::Sense::click_and_drag(),
                                );
                                ui.painter().galley(rect.min, galley.clone(), p.text);
                                if let Some(pos) =
                                    ui.input(|i| i.pointer.interact_pos()).filter(|pos| {
                                        ui.rect_contains_pointer(rect)
                                            && rect
                                                .expand2(egui::vec2(
                                                    0.,
                                                    ui.spacing().item_spacing.y / 2.,
                                                ))
                                                .contains(*pos)
                                    })
                                {
                                    let i: usize =
                                        galley.cursor_from_pos(pos - rect.min).index.into();
                                    let byte = s.char_indices().nth(i).map_or(s.len(), |(i, _)| i);
                                    if ui.input(|i| i.pointer.primary_pressed()) {
                                        state.selection = Some(Selection {
                                            anchor: (row, byte),
                                            end: (row, byte),
                                            right,
                                        });
                                        ui.memory_mut(|m| m.request_focus(focus));
                                    } else if ui.input(|i| i.pointer.primary_down())
                                        && let Some(sel) = &mut state.selection
                                        && sel.right == right
                                    {
                                        sel.end = (row, byte);
                                    }
                                }
                                r.context_menu(|ui| {
                                    if ui.button("Copy").clicked() {
                                        copy = true;
                                        ui.close();
                                    }
                                    if ui.button("Add to composer").clicked() {
                                        add = true;
                                        ui.close();
                                    }
                                });
                            }
                        });
                    }
                });
        };
        if layout.side {
            ui.columns(2, |columns| {
                column(&mut columns[0], false);
                column(&mut columns[1], true);
            });
        } else {
            column(ui, false);
        }
    });
    if let Some((base, extra)) = expand {
        state.expanded.insert(base, extra);
        state.selection = None;
    }
    if ui.memory(|m| m.has_focus(focus)) {
        add |= ui.input_mut(|i| {
            i.consume_key(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, egui::Key::A)
        });
        copy |= ui.input(|i| i.events.contains(&egui::Event::Copy));
    }
    if !add && !copy {
        return None;
    }
    let sel = state.selection.filter(|s| s.anchor != s.end)?;
    let first = sel.anchor.0.min(sel.end.0);
    let last = sel.anchor.0.max(sel.end.0);
    let mut parts = Vec::new();
    let mut min = usize::MAX;
    let mut max = 0;
    for row in first..=last {
        let Some(cell) = layout.cell(state, row, sel.right) else {
            continue;
        };
        let s = cell.src.line_text(cell.index);
        min = min.min(cell.index + 1);
        max = max.max(cell.index + 1);
        parts.push((cell.kind, &s[sel.range(row, s.len())?]));
    }
    if parts.is_empty() {
        return None;
    }
    let plain = parts.iter().map(|(_, s)| *s).collect::<Vec<_>>().join("\n");
    if copy {
        ui.ctx().copy_text(plain.clone());
    }
    add.then(|| match layout.doc {
        Document::File(f) => yaac_code::file_excerpt(
            path,
            f.language.as_ref().map(|l| l.fence.as_str()),
            min..=max,
            &plain,
        ),
        _ => yaac_code::diff_excerpt(path, min..=max, parts),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{Event, Key, Modifiers, PointerButton, Pos2};

    #[test]
    fn drag_then_shortcut_in_virtual_file_view_adds_exact_selection() {
        let ctx = egui::Context::default();
        let file = Highlighted {
            text: "alpha\nbravo".into(),
            language: None,
            lines: vec![
                yaac_code::Line {
                    range: 0..5,
                    spans: vec![],
                },
                yaac_code::Line {
                    range: 6..11,
                    spans: vec![],
                },
            ],
        };
        let doc = Document::File(file);
        let layout = Layout {
            doc: &doc,
            side: false,
        };
        let mut state = Viewer::default();
        let mut selected = None;
        let mut render = |events: Vec<Event>| {
            let mut out = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        Pos2::ZERO,
                        egui::vec2(800., 500.),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    if let Some(s) =
                        viewer(ui, &layout, &mut state, None, &Palette::DARK, "src/main.rs")
                    {
                        selected = Some(s);
                    }
                },
            );
            out.textures_delta.clear();
            out.shapes
                .into_iter()
                .filter_map(|s| {
                    let egui::Shape::Text(t) = s.shape else {
                        return None;
                    };
                    let text = t.galley.text();
                    (text == "alpha" || text == "bravo")
                        .then(|| (text.to_owned(), t.pos, t.galley.size()))
                })
                .collect::<Vec<_>>()
        };
        let lines = render(vec![]);
        let pos = |line: &str, n: usize| {
            let (_, p, size) = lines
                .iter()
                .find(|(s, _, _)| s == line)
                .expect("visible source line");
            *p + egui::vec2(size.x / 5. * n as f32, size.y / 2.)
        };
        let a = pos("alpha", 2);
        let b = pos("bravo", 3);
        render(vec![
            Event::PointerMoved(a),
            Event::PointerButton {
                pos: a,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
        ]);
        render(vec![Event::PointerMoved(b)]);
        render(vec![Event::PointerButton {
            pos: b,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }]);
        render(vec![Event::Key {
            key: Key::A,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::CTRL | Modifiers::SHIFT,
        }]);
        let selected = selected.expect("Ctrl+Shift+A inserts the dragged range");
        assert_eq!(selected.label, "main.rs:1–2");
        assert!(selected.text.contains("pha\nbra"), "{selected:?}");
    }
}
