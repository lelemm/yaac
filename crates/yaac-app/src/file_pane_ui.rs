//! egui presentation of the shared thread worktree pane.
use crate::{
    app::YaacApp,
    file_pane::{Document, FileTree, TabKind},
    theme::Palette,
};
use eframe::egui::{self, Color32, RichText, Ui};
use egui_ltreeview::{NodeBuilder, TreeView, TreeViewBuilder};
use std::{collections::BTreeMap, ops::Range};
use yaac_code::{DiffLineKind as Kind, DiffRow, DiffView, Highlighted, ScopeClass};
use yaac_core::{ThreadKey, attachments::DraftRef};
use yaac_protocol::{FsTreeEntryKind, GitFileStatus};

pub(crate) fn draw(ui: &mut Ui, app: &mut YaacApp, key: &ThreadKey, id: u64) {
    let area = app.browsers.areas.get_mut(key).unwrap();
    let tab = area.tabs.iter().find(|t| t.id == id).unwrap();
    let kind = tab.kind.clone();
    let doc = tab.document.clone();
    let error = tab.error.clone();
    let initial_side = app.browsers.side_by_side;
    let state_id = ui
        .scope_id()
        .with((id, tab.generation, app.browsers.side_by_side));
    let mut state = ui
        .data_mut(|d| d.get_temp::<Viewer>(state_id))
        .unwrap_or_default();
    let mut jump = None;
    let mut reload = false;
    ui.horizontal_wrapped(|ui| {
        if kind == TabKind::Files {
            ui.checkbox(&mut area.tree.show_ignored, "Show ignored");
            if ui.button("Refresh").clicked() {
                area.tree.refresh();
            }
            if ui.button("Collapse all").clicked() {
                area.tree.collapse_all();
                state.tree_epoch += 1;
            }
        } else {
            reload = ui.button("Reload").clicked();
            if let Some(Document::Diff(diff)) = doc.as_deref() {
                ui.selectable_value(&mut app.browsers.side_by_side, false, "Unified");
                ui.selectable_value(&mut app.browsers.side_by_side, true, "Side by side");
                let view = if app.browsers.side_by_side {
                    &diff.side_by_side
                } else {
                    &diff.unified
                };
                let n = view.change_indices.len();
                for (label, forward) in [("Previous change", false), ("Next change", true)] {
                    if ui.add_enabled(n > 0, egui::Button::new(label)).clicked() {
                        let i = match (state.change, forward) {
                            (None, true) => 0,
                            (None, false) => n - 1,
                            (Some(i), true) => (i + 1) % n,
                            (Some(i), false) => (i + n - 1) % n,
                        };
                        state.change = Some(i);
                        jump = Some(state.display_index(view.change_indices[i]));
                    }
                }
            }
        }
        if ui
            .button(if area.full_screen {
                "Restore"
            } else {
                "Full screen"
            })
            .clicked()
        {
            area.full_screen = !area.full_screen;
        }
    });
    if app.browsers.side_by_side != initial_side {
        ui.ctx().request_repaint();
        return;
    }
    if reload {
        app.browsers.reload_file(&mut app.client, key, id);
        return;
    }
    if kind == TabKind::Files {
        let mut open = None;
        let mut changes = vec![];
        egui::ScrollArea::both().show(ui, |ui| {
            let area = app.browsers.areas.get(key).unwrap();
            TreeView::new(ui.scope_id().with(("worktree", state.tree_epoch)))
                .allow_drag_and_drop(false)
                .show(ui, |b| {
                    folder(b, &area.tree, "", &app.p, &mut changes, &mut open)
                });
            for (path, e) in &area.tree.errors {
                ui.colored_label(app.p.failed, format!("{path} {e}"));
            }
            if area.tree.folders.values().any(|f| f.truncated) {
                ui.label("Listing limited to 5,000 entries per folder.");
            }
        });
        for (p, on) in changes {
            app.browsers.areas.get_mut(key).unwrap().tree.expand(&p, on);
            ui.ctx().request_repaint();
        }
        if let Some((kind, duplicate)) = open {
            let _ = app
                .browsers
                .open_file_tab(&mut app.client, key, kind, duplicate);
        }
    } else if let Some(e) = error {
        ui.colored_label(app.p.failed, e);
    } else if let Some(doc) = doc {
        if let Document::Notice(s) = doc.as_ref() {
            ui.label(s);
        } else {
            let path = kind.path().unwrap();
            app.preview_ui.file_focus = Some(ui.scope_id().with(("selection", path)));
            let side = app.browsers.side_by_side && matches!(doc.as_ref(), Document::Diff(_));
            let view = Layout { doc: &doc, side };
            if let Some(excerpt) = viewer(ui, &view, &mut state, jump, &app.p, path) {
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
                    app.browsers.areas.get_mut(key).unwrap().full_screen = false;
                    app.ui.focus_composer = true;
                }
            }
        }
    } else {
        ui.label("Loading…");
    }
    ui.data_mut(|d| d.insert_temp(state_id, state));
}

fn folder(
    b: &mut TreeViewBuilder<'_, String>,
    tree: &FileTree,
    dir: &str,
    p: &Palette,
    changes: &mut Vec<(String, bool)>,
    open: &mut Option<(TabKind, bool)>,
) {
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
        let color = if e.ignored { p.unavailable } else { p.text };
        if e.kind == FsTreeEntryKind::Dir {
            let expanded = tree.expanded.contains(&path);
            let is_open = b.node(
                NodeBuilder::dir(path.clone()).default_open(expanded).label(
                    RichText::new(format!("{}{}", e.name, if e.changed { " •" } else { "" }))
                        .color(color),
                ),
            );
            if is_open != expanded {
                changes.push((path.clone(), is_open));
            }
            if is_open {
                folder(b, tree, &path, p, changes, open);
            }
            b.close_dir();
        } else {
            b.node(NodeBuilder::leaf(path.clone()).label_ui(|ui| {
                ui.horizontal(|ui| {
                    let name = ui.label(RichText::new(&e.name).color(color));
                    let hit = |r: &egui::Response| {
                        r.ctx.input(|i| {
                            if !i
                                .pointer
                                .interact_pos()
                                .is_some_and(|pos| r.interact_rect.contains(pos))
                            {
                                return None;
                            }
                            if i.pointer.button_clicked(egui::PointerButton::Middle) {
                                Some(true)
                            } else if i.pointer.primary_clicked() {
                                Some(false)
                            } else {
                                None
                            }
                        })
                    };
                    if let Some(new) = hit(&name) {
                        *open = Some((TabKind::File(path.clone()), new));
                    }
                    if let Some(g) = &e.git {
                        let col = match g {
                            GitFileStatus::Added | GitFileStatus::Untracked => p.git_added,
                            GitFileStatus::Deleted | GitFileStatus::Conflict => p.git_deleted,
                            _ => p.git_modified,
                        };
                        let r = ui.label(RichText::new(g.as_str()).strong().color(col));
                        if let Some(new) = hit(&r) {
                            *open = Some((TabKind::Diff(path.clone()), new));
                        }
                    }
                });
            }));
        }
    }
}

#[derive(Default, Clone)]
struct Viewer {
    tree_epoch: u64,
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
    let height = ui
        .text_style_height(&egui::TextStyle::Monospace)
        .max(ui.spacing().interact_size.y);
    let stride = height + ui.spacing().item_spacing.y;
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
                        ui.horizontal(|ui| {
                            ui.set_min_height(height);
                            let cell = layout.cell(state, row, right);
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
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(format!("{:>5} {sign}", cell.index + 1))
                                            .monospace()
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
