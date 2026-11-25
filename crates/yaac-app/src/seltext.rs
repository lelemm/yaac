//! Selectable text in the transcript, across items (docs/design/screens.md, "Text can be
//! selected and copied").
//!
//! egui's own label selection works within one frame's labels and drops a selection whose end
//! scrolls out of view, which the virtualised transcript does all the time. So the transcript
//! draws its text through [`SelCx`]: every galley is registered under a [`Unit`] (an item, a
//! file of a diff, a turn's summary) and a part number, painted with its share of the
//! selection, and its text remembered. The selection itself is [`yaac_core::selection`]:
//! positions by item order, so units out of view still copy. Everywhere else (settings,
//! dialogs, the agents panel, cards) egui's selectable labels do the job.

use std::collections::BTreeMap;
use std::sync::Arc;

use eframe::egui::{self, Color32, Galley, Pos2, Rect, Response, Sense, Ui, text::CCursor, text_selection::CCursorRange};
use yaac_core::ThreadKey;
use yaac_core::selection::{Drag, Selection, TextPos, Texts, Unit, copy_text};

/// One galley drawn this frame.
#[derive(Clone, Debug)]
struct Placed {
    unit: Unit,
    part: usize,
    pos: Pos2,
    galley: Arc<Galley>,
}

impl Placed {
    fn rect(&self) -> Rect {
        Rect::from_min_size(self.pos, self.galley.size())
    }

    fn at(&self, p: Pos2) -> TextPos {
        let c = self.galley.cursor_from_pos(p - self.pos);
        TextPos::new(self.unit, self.part, c.index.0)
    }

    fn end(&self) -> TextPos {
        TextPos::new(self.unit, self.part, self.galley.text().chars().count())
    }
}

/// The transcript's selection state (view state, per window).
#[derive(Debug, Default)]
pub(crate) struct TextSel {
    pub drag: Drag,
    thread: Option<ThreadKey>,
    frame: Vec<Placed>,
    frame_texts: Texts,
    /// The parts of every unit as last drawn.
    texts: Texts,
    /// Units drawn as one tinted block when they lie inside a selection (cards).
    pub whole: BTreeMap<Unit, Rect>,
}

impl TextSel {
    /// Start a frame of `key`'s transcript.
    pub(crate) fn begin(&mut self, key: &ThreadKey) {
        if self.thread.as_ref() != Some(key) {
            *self = Self { thread: Some(key.clone()), ..Self::default() };
        }
        self.frame.clear();
        self.frame_texts.clear();
        self.whole.clear();
    }

    pub(crate) fn selection(&self) -> Option<Selection> {
        self.drag.selection.filter(|s| !s.is_empty())
    }

    fn nearest(&self, p: Pos2) -> Option<TextPos> {
        if self.frame.is_empty() {
            return None;
        }
        // On a row of text: the closest galley on that row.
        let on_row = self
            .frame
            .iter()
            .filter(|g| g.rect().y_range().contains(p.y))
            .min_by(|a, b| a.rect().distance_to_pos(p).total_cmp(&b.rect().distance_to_pos(p)));
        if let Some(g) = on_row {
            return Some(g.at(p));
        }
        // Between rows or past the ends: the end of the last text above, or the start of the
        // first.
        let mut sorted: Vec<&Placed> = self.frame.iter().collect();
        sorted.sort_by_key(|g| (g.unit, g.part));
        match sorted.iter().rev().find(|g| g.rect().top() <= p.y) {
            Some(g) => Some(g.end()),
            None => sorted.first().map(|g| TextPos::new(g.unit, g.part, 0)),
        }
    }

    /// Where `unit`'s first part was drawn this frame (tests drive the pointer there).
    #[cfg(test)]
    pub(crate) fn drawn(&self, unit: Unit) -> Option<Rect> {
        self.frame.iter().find(|g| g.unit == unit && g.part == 0).map(Placed::rect)
    }

    fn exact(&self, p: Pos2) -> Option<TextPos> {
        self.frame.iter().find(|g| g.rect().expand2(egui::vec2(0.0, 1.0)).contains(p)).map(|g| g.at(p))
    }

    /// After the frame's text is drawn: pointer input, the copy shortcut, and keeping the
    /// text of what was drawn. `area` is the transcript's visible rectangle. Returns true when
    /// the selection changed (the caller repaints).
    pub(crate) fn end(&mut self, ui: &Ui, area: Rect, data: impl FnMut(usize) -> Vec<(Unit, Vec<String>)>) -> bool {
        for (u, parts) in std::mem::take(&mut self.frame_texts) {
            self.texts.insert(u, parts);
        }
        let before = self.drag.selection;
        let (pressed, down, released, pos, copy, escape) = ui.input(|i| {
            (
                i.pointer.primary_pressed(),
                i.pointer.primary_down(),
                i.pointer.primary_released(),
                i.pointer.interact_pos(),
                i.events.iter().any(|e| matches!(e, egui::Event::Copy | egui::Event::Cut)),
                i.key_pressed(egui::Key::Escape),
            )
        });
        // A press on something drawn over the transcript (its right-click menu, a popup, a
        // dialog) is not a press on the text: it neither starts nor clears a selection, so the
        // menu's Copy still has one to copy.
        let over = pos.and_then(|p| ui.ctx().layer_id_at(p)).is_some_and(|l| l != ui.layer_id());
        if pressed && !over {
            let on_text = pos.filter(|p| area.contains(*p)).and_then(|p| self.exact(p));
            self.drag.press(on_text);
            if on_text.is_some() {
                // One selection at a time: drop egui's own label selection.
                ui.ctx().plugin::<egui::text_selection::LabelSelectionState>().lock().clear_selection();
            }
        } else if down
            && self.drag.dragging()
            && let Some(p) = pos
            && let Some(t) = self.nearest(p)
        {
            self.drag.moved(t);
        }
        if released {
            self.drag.release();
        }
        if escape && self.selection().is_some() {
            self.drag.clear();
        }
        if copy && self.selection().is_some() {
            let text = self.copy(data);
            ui.ctx().copy_text(text);
        }
        if self.drag.dragging() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
        }
        self.drag.selection != before
    }

    /// The selection as plain text, filling units that were never drawn from `data`.
    pub(crate) fn copy(&mut self, mut data: impl FnMut(usize) -> Vec<(Unit, Vec<String>)>) -> String {
        let Some(s) = self.selection() else { return String::new() };
        let (a, b) = s.ordered();
        // Every unit between the ends: drawn ones are in `texts` (exactly as drawn); the rest
        // come from the data.
        for item in a.unit.item..=b.unit.item {
            for (u, parts) in data(item) {
                self.texts.entry(u).or_insert(parts);
            }
        }
        copy_text(&s, &self.texts)
    }

    /// Whether `unit` lies wholly inside the selection (between its ends).
    pub(crate) fn covers(&self, unit: Unit) -> bool {
        self.selection().is_some_and(|s| {
            let (a, b) = s.ordered();
            a.unit < unit && unit < b.unit
        })
    }

    /// Autoscroll while a drag is held above or below the visible area.
    pub(crate) fn autoscroll(&self, ui: &Ui, area: Rect) {
        if !self.drag.dragging() {
            return;
        }
        let Some(p) = ui.input(|i| i.pointer.interact_pos()) else { return };
        let dy = if p.y < area.top() {
            (area.top() - p.y).min(40.0)
        } else if p.y > area.bottom() {
            -(p.y - area.bottom()).min(40.0)
        } else {
            0.0
        };
        if dy != 0.0 {
            ui.scroll_with_delta(egui::vec2(0.0, dy));
            ui.ctx().request_repaint();
        }
    }
}

/// Drawing context for one unit.
#[derive(Debug)]
pub(crate) struct SelCx<'a> {
    pub ts: &'a mut TextSel,
    pub unit: Unit,
}

impl SelCx<'_> {
    /// Keep a diagram's source in a selection that spans its preview.
    pub(crate) fn diagram(&mut self, source: &str) {
        self.ts.frame_texts.entry(self.unit).or_default().push(source.to_owned());
    }

    /// Paint `galley` at `pos` with its share of the selection, and register it.
    pub(crate) fn paint(&mut self, ui: &Ui, pos: Pos2, galley: Arc<Galley>, fallback: Color32) {
        let parts = self.ts.frame_texts.entry(self.unit).or_default();
        let part = parts.len();
        parts.push(galley.text().to_owned());
        let len = galley.text().chars().count();
        let mut shown = galley.clone();
        if let Some(sel) = self.ts.selection()
            && let Some((a, b)) = sel.range_in(self.unit, part, len)
        {
            let range = CCursorRange::two(CCursor::new(egui::text::CharIndex(a)), CCursor::new(egui::text::CharIndex(b)));
            egui::text_selection::visuals::paint_text_selection(&mut shown, ui.visuals(), &range, None);
        }
        ui.painter().galley(pos, shown, fallback);
        self.ts.frame.push(Placed { unit: self.unit, part, pos, galley });
    }

    /// A text block in the layout (like a label), selectable.
    pub(crate) fn label(&mut self, ui: &mut Ui, galley: Arc<Galley>, fallback: Color32) -> Response {
        let (rect, resp) = ui.allocate_exact_size(galley.size(), Sense::hover());
        self.paint(ui, rect.min, galley, fallback);
        resp.on_hover_cursor(egui::CursorIcon::Text)
    }

    /// A layout job as a selectable label, wrapped to the available width.
    pub(crate) fn job(&mut self, ui: &mut Ui, mut job: egui::text::LayoutJob, fallback: Color32) -> Response {
        if job.wrap.max_width.is_infinite() {
            job.wrap.max_width = ui.available_width();
        }
        let g = ui.fonts_mut(|f| f.layout_job(job));
        self.label(ui, g, fallback)
    }

    /// Plain text in one font and colour, wrapped, selectable.
    pub(crate) fn text(&mut self, ui: &mut Ui, text: &str, font: egui::FontId, color: Color32) -> Response {
        let w = ui.available_width();
        let g = ui.fonts_mut(|f| f.layout(text.to_owned(), font, color, w));
        self.label(ui, g, color)
    }

    /// A unit drawn by other widgets (a card): tinted as a whole while it lies inside a
    /// selection, so the user sees it will be copied.
    pub(crate) fn whole(&mut self, ui: &Ui, rect: Rect) {
        if self.ts.covers(self.unit) {
            ui.painter().rect_filled(rect, 6.0, ui.visuals().selection.bg_fill.gamma_multiply(0.6));
        }
        self.ts.whole.insert(self.unit, rect);
    }
}

#[cfg(test)]
mod tests {
    //! Driven through a headless egui context with simulated pointer and key events.
    use eframe::egui::{Event, FontId, OutputCommand, PointerButton, RawInput, pos2, vec2};

    use super::*;

    const TEXTS: [&str; 6] = [
        "First message, about the frame decoder.",
        "Ran cargo test · passed",
        "Third item in the middle.",
        "Fourth item: the length prefix counts the kind byte.",
        "Fifth item.",
        "Sixth and last item of the transcript.",
    ];

    fn key() -> ThreadKey {
        ThreadKey::new(yaac_protocol::TargetId::new("local"), yaac_protocol::ThreadId::new("t"))
    }

    /// One frame drawing `units` (a virtualised window onto TEXTS) with `events`.
    fn frame(ctx: &egui::Context, ts: &mut TextSel, units: &[usize], events: Vec<Event>, t: f64) -> egui::FullOutput {
        let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))), events, time: Some(t), ..Default::default() };
        let mut out = ctx.run_ui(input, |ui| {
            ts.begin(&key());
            for &i in units {
                SelCx { ts, unit: Unit::new(i, 0) }.text(ui, TEXTS[i], FontId::proportional(14.0), Color32::WHITE);
                ui.add_space(8.0);
            }
            let area = ui.max_rect();
            ts.end(ui, area, |n| vec![(Unit::new(n, 0), vec![TEXTS[n].to_owned()])]);
        });
        out.textures_delta.clear();
        out
    }

    fn placed(ts: &TextSel, unit: usize) -> Rect {
        ts.frame.iter().find(|g| g.unit.item == unit).map(Placed::rect).unwrap()
    }

    fn press(p: Pos2, pressed: bool) -> Event {
        Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: Default::default() }
    }

    fn copied(out: &egui::FullOutput) -> Option<String> {
        out.platform_output.commands.iter().find_map(|c| match c {
            OutputCommand::CopyText(s) => Some(s.clone()),
            _ => None,
        })
    }

    #[test]
    fn a_drag_across_items_copies_them_in_reading_order_even_after_they_scroll_away() {
        let ctx = egui::Context::default();
        let mut ts = TextSel::default();
        // Units 0..=3 are on screen.
        frame(&ctx, &mut ts, &[0, 1, 2, 3], vec![], 0.0);
        let r0 = placed(&ts, 0);
        // Press in the middle of the first message.
        let start = pos2(r0.left() + r0.width() * 0.45, r0.center().y);
        frame(&ctx, &mut ts, &[0, 1, 2, 3], vec![Event::PointerMoved(start), press(start, true)], 0.1);
        // The list scrolls: units 0 and 1 leave the view, 4 and 5 arrive. Drag into unit 5.
        frame(&ctx, &mut ts, &[2, 3, 4, 5], vec![], 0.2);
        let r5 = placed(&ts, 5);
        let end = pos2(r5.left() + r5.width() * 0.3, r5.center().y);
        frame(&ctx, &mut ts, &[2, 3, 4, 5], vec![Event::PointerMoved(end)], 0.3);
        frame(&ctx, &mut ts, &[2, 3, 4, 5], vec![press(end, false)], 0.4);
        let sel = ts.selection().expect("a selection");
        assert_eq!((sel.anchor.unit.item, sel.focus.unit.item), (0, 5));
        let out = frame(&ctx, &mut ts, &[2, 3, 4, 5], vec![Event::Copy], 0.5);
        let text = copied(&out).expect("Ctrl+C copies");
        let (a, b) = (sel.anchor.ch, sel.focus.ch);
        let first: String = TEXTS[0].chars().skip(a).collect();
        let last: String = TEXTS[5].chars().take(b).collect();
        let want = [first, TEXTS[1].into(), TEXTS[2].into(), TEXTS[3].into(), TEXTS[4].into(), last].join("\n\n");
        assert_eq!(text, want);
        assert!(a > 0 && b > 0 && b < TEXTS[5].chars().count(), "both ends are mid-item");
    }

    #[test]
    fn a_click_selects_nothing_and_escape_clears() {
        let ctx = egui::Context::default();
        let mut ts = TextSel::default();
        frame(&ctx, &mut ts, &[0, 1, 2], vec![], 0.0);
        let r1 = placed(&ts, 1);
        let p = r1.center();
        frame(&ctx, &mut ts, &[0, 1, 2], vec![Event::PointerMoved(p), press(p, true)], 0.1);
        frame(&ctx, &mut ts, &[0, 1, 2], vec![press(p, false)], 0.2);
        assert!(ts.selection().is_none(), "a click is a click");
        let out = frame(&ctx, &mut ts, &[0, 1, 2], vec![Event::Copy], 0.3);
        assert_eq!(copied(&out), None);
        // Select within one item, then press Escape.
        let a = pos2(r1.left() + 2.0, r1.center().y);
        frame(&ctx, &mut ts, &[0, 1, 2], vec![Event::PointerMoved(a), press(a, true)], 0.4);
        frame(&ctx, &mut ts, &[0, 1, 2], vec![Event::PointerMoved(p)], 0.5);
        frame(&ctx, &mut ts, &[0, 1, 2], vec![press(p, false)], 0.6);
        assert!(ts.selection().is_some());
        let esc = Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() };
        frame(&ctx, &mut ts, &[0, 1, 2], vec![esc], 0.7);
        assert!(ts.selection().is_none());
    }
}
