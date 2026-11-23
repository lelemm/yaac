//! The skill picker over the egui composer: `/` at the start of a message, `$` inside one
//! (docs/04-providers.md#skills). The decisions are `yaac_core::skills`; this draws the popup
//! and takes the keys. The picker never changes text the user did not pick.

use eframe::egui::{self, Align2, Key, Modifiers};
use yaac_core::skills::{self, Composer, Entry, PickerState, Status, View};

use crate::app::Action;
use crate::theme::{self, Palette, size};
use crate::widgets;

/// The window's picker state: which text box it belongs to, and what the user did to it.
#[derive(Debug, Default)]
pub(crate) struct Picker {
    owner: Option<egui::Id>,
    state: PickerState,
    /// What was drawn last frame: the rows the keys act on.
    last: Option<(skills::Active, Vec<Entry>, usize)>,
    /// Where the popup was drawn, so a click on it is not a click away from the text box.
    rect: Option<egui::Rect>,
}

fn byte_of(text: &str, chars: usize) -> usize {
    text.char_indices().nth(chars).map_or(text.len(), |(i, _)| i)
}

fn chars_of(text: &str, bytes: usize) -> usize {
    text[..bytes.min(text.len())].chars().count()
}

/// Before the text box: takes the picker's keys out of the input (Up, Down, Tab, Enter, Esc)
/// so the text box neither sends nor moves, and applies a pick to `text`. Returns whether the
/// text changed.
pub(crate) fn keys(ui: &egui::Ui, picker: &mut Picker, id: egui::Id, comp: &Composer, text: &mut String, focus: &mut bool) -> bool {
    if picker.owner != Some(id) || !ui.memory(|m| m.has_focus(id)) {
        return false;
    }
    let Some((active, rows, selected)) = picker.last.clone() else { return false };
    let take = |k: Key| ui.input_mut(|i| i.consume_key(Modifiers::NONE, k));
    if take(Key::Escape) {
        picker.state.dismiss(&active);
        return false;
    }
    if rows.is_empty() {
        return false;
    }
    if take(Key::ArrowDown) {
        picker.state.selected = selected;
        picker.state.next(rows.len());
        return false;
    }
    if take(Key::ArrowUp) {
        picker.state.selected = selected;
        picker.state.prev(rows.len());
        return false;
    }
    if take(Key::Tab) || take(Key::Enter) {
        return choose(ui.ctx(), picker, id, comp, text, focus, &active, &rows[selected]);
    }
    false
}

#[allow(clippy::too_many_arguments, reason = "one pick, from the keys or a click")]
fn choose(ctx: &egui::Context, picker: &mut Picker, id: egui::Id, comp: &Composer, text: &mut String, focus: &mut bool, active: &skills::Active, entry: &Entry) -> bool {
    let edit = skills::apply(text, active, entry, comp.caps);
    let cursor = chars_of(&edit.text, edit.cursor);
    *text = edit.text;
    let mut st = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
    st.cursor.set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(cursor))));
    st.store(ctx, id);
    picker.state = PickerState::default();
    picker.last = None;
    *focus = true;
    true
}

/// After the text box: finds the trigger at the cursor, asks for the list when it is not there,
/// and draws the picker (or the tooltip of an unsupported trigger) above the box.
#[allow(clippy::too_many_arguments, reason = "the text box's own context")]
pub(crate) fn show(ui: &mut egui::Ui, picker: &mut Picker, id: egui::Id, comp: &Composer, state: &yaac_core::state::AppState, text: &mut String, focus: &mut bool, p: &Palette, actions: &mut Vec<Action>) {
    let focused = ui.memory(|m| m.has_focus(id));
    let over = picker.rect.zip(ui.input(|i| i.pointer.hover_pos())).is_some_and(|(r, pos)| r.contains(pos));
    if picker.owner == Some(id) && !focused && !over {
        // Focus moved away: the picker goes with it.
        *picker = Picker::default();
        return;
    }
    if !focused && picker.owner != Some(id) {
        return;
    }
    let Some(cursor) = egui::TextEdit::load_state(ui.ctx(), id).and_then(|s| s.cursor.char_range()).map(|r| byte_of(text, r.primary.index.into())) else {
        return;
    };
    let origin = ui.min_rect().left_top();
    match skills::view(state, comp, text, cursor, &picker.state) {
        View::Closed => {
            if picker.state.dismissed.is_some() && !matches!(skills::detect(text, cursor, comp.caps, &comp.provider), skills::Detection::Open(_)) {
                picker.state = PickerState::default();
            }
            picker.last = None;
            picker.rect = None;
            picker.owner = focused.then_some(id).filter(|_| picker.state.dismissed.is_some());
        }
        View::Tooltip(tip) => {
            picker.owner = Some(id);
            picker.last = None;
            popup(ui.ctx(), id, origin, p, |ui| {
                ui.set_max_width(360.0);
                ui.label(widgets::secondary(tip, p));
            });
        }
        View::Open { active, rows, status, selected, want } => {
            picker.owner = Some(id);
            if let Some(scope) = want {
                actions.push(Action::Skills(scope, false));
            }
            picker.last = Some((active.clone(), rows.clone(), selected));
            let mut picked = None;
            let rect = popup(ui.ctx(), id, origin, p, |ui| {
                ui.set_min_width(300.0);
                ui.set_max_width(460.0);
                match &status {
                    Status::Loading => {
                        ui.label(widgets::secondary("Loading skills…", p));
                    }
                    Status::Empty => {
                        ui.label(widgets::secondary(if active.query.is_empty() { "No skills here." } else { "No skill matches." }, p));
                    }
                    Status::Failed(why) => {
                        ui.label(egui::RichText::new(format!("Skills could not be listed: {why}")).font(theme::ui(size::SMALL)).color(p.failed));
                        if widgets::plain(ui, p, "Retry", None).clicked() {
                            actions.push(Action::Skills(comp.scope.clone(), true));
                        }
                    }
                    Status::Rows => {
                        egui::ScrollArea::vertical().max_height(220.0).auto_shrink([true, true]).show(ui, |ui| {
                            for (n, e) in rows.iter().enumerate() {
                                if row(ui, p, e, comp, n == selected) {
                                    picked = Some(n);
                                }
                            }
                        });
                    }
                }
            });
            picker.rect = Some(rect);
            if let Some(n) = picked {
                choose(ui.ctx(), picker, id, comp, text, focus, &active, &rows[n]);
            }
        }
    }
}

fn row(ui: &mut egui::Ui, p: &Palette, e: &Entry, comp: &Composer, selected: bool) -> bool {
    let sigil = if comp.caps.inline && e.kind != skills::EntryKind::Builtin { '$' } else { '/' };
    let w = ui.available_width().max(300.0);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, 36.0), egui::Sense::click());
    if selected || resp.hovered() {
        ui.painter().rect_filled(rect, 4.0, p.selected);
    }
    let name = match &e.hint {
        Some(h) => format!("{sigil}{} {h}", e.name),
        None => format!("{sigil}{}", e.name),
    };
    let painter = ui.painter();
    painter.text(rect.left_top() + egui::vec2(8.0, 4.0), Align2::LEFT_TOP, name, theme::ui(size::SMALL + 1.0), p.text);
    let desc: String = e.description.chars().take(90).collect();
    painter.text(rect.left_top() + egui::vec2(8.0, 20.0), Align2::LEFT_TOP, desc, theme::ui(size::SMALL), p.text2);
    if let Some(word) = e.scope_word() {
        painter.text(rect.right_top() + egui::vec2(-8.0, 4.0), Align2::RIGHT_TOP, word, theme::ui(size::SMALL - 1.0), p.text2);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

/// A popup whose bottom-left corner sits just above `origin`. Returns where it was drawn.
fn popup(ctx: &egui::Context, id: egui::Id, origin: egui::Pos2, p: &Palette, add: impl FnOnce(&mut egui::Ui)) -> egui::Rect {
    let out = egui::Area::new(id.with("skills-picker"))
        .order(egui::Order::Foreground)
        .pivot(Align2::LEFT_BOTTOM)
        .fixed_pos(origin - egui::vec2(0.0, 6.0))
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(p.raised)
                .stroke(egui::Stroke::new(1.0, p.border))
                .corner_radius(6)
                .inner_margin(egui::Margin::symmetric(8, 6))
                .show(ui, add);
        });
    out.response.rect
}

/// The WPF composer's side of the picker: where its caret is (it reports it with each edit) and
/// what the user did to the picker. The decisions are the same `yaac_core::skills` ones.
#[derive(Debug, Default)]
pub(crate) struct WpfSkills {
    /// The composer the caret is in (its draft key).
    key: String,
    /// The caret, in UTF-16 units of the text box.
    caret: usize,
    state: PickerState,
}

impl WpfSkills {
    /// The caret moved or the text changed in composer `key`.
    pub(crate) fn caret(&mut self, key: &str, caret: usize) {
        if self.key != key {
            *self = WpfSkills { key: key.to_owned(), ..WpfSkills::default() };
        }
        self.caret = caret;
        // A new word, or the trigger gone, ends a dismissal; `view` clears it when it no longer applies.
    }

    fn cursor(&self, key: &str, text: &str) -> Option<usize> {
        (self.key == key).then(|| skills::byte_of_utf16(text, self.caret))
    }

    /// Esc: closes the picker for the trigger the caret is in.
    pub(crate) fn dismiss(&mut self, key: &str, text: &str, comp: &Composer) {
        let Some(cursor) = self.cursor(key, text) else { return };
        if let skills::Detection::Open(a) = skills::detect(text, cursor, comp.caps, &comp.provider) {
            self.state.dismiss(&a);
        }
    }

    /// After an edit or a caret move: a dismissal ends once its trigger is gone, and the list is
    /// asked for when the picker needs it. Returns what to ask for (`skills/list`).
    pub(crate) fn settle(&mut self, key: &str, text: &str, comp: &Composer, state: &yaac_core::state::AppState) -> Option<skills::SkillScopeKey> {
        let cursor = self.cursor(key, text)?;
        if self.state.dismissed.is_some() && !matches!(skills::detect(text, cursor, comp.caps, &comp.provider), skills::Detection::Open(_)) {
            self.state = PickerState::default();
        }
        match skills::view(state, comp, text, cursor, &self.state) {
            View::Open { want, .. } => want,
            _ => None,
        }
    }

    /// The `skills` field of the composer view (null when there is nothing to show).
    pub(crate) fn view(&self, key: &str, text: &str, comp: &Composer, state: &yaac_core::state::AppState) -> serde_json::Value {
        use serde_json::json;
        let Some(cursor) = self.cursor(key, text) else { return serde_json::Value::Null };
        match skills::view(state, comp, text, cursor, &self.state) {
            View::Closed => serde_json::Value::Null,
            View::Tooltip(tip) => json!({ "tooltip": tip }),
            View::Open { active, rows, status, selected, .. } => {
                let (status, message) = match &status {
                    Status::Rows => ("rows", String::new()),
                    Status::Loading => ("loading", "Loading skills…".to_owned()),
                    Status::Empty => ("empty", if active.query.is_empty() { "No skills here." } else { "No skill matches." }.to_owned()),
                    Status::Failed(why) => ("failed", format!("Skills could not be listed: {why}")),
                };
                let sigil_of = |e: &Entry| if comp.caps.inline && e.kind != skills::EntryKind::Builtin { '$' } else { '/' };
                let rows: Vec<_> = rows
                    .iter()
                    .map(|e| {
                        json!({
                            "label": format!("{}{}", sigil_of(e), e.name),
                            "hint": e.hint,
                            "detail": e.description,
                            "scope": e.scope_word(),
                            "insert": skills::insertion(text, &active, e, comp.caps),
                        })
                    })
                    .collect();
                json!({
                    "status": status,
                    "message": message,
                    "start": skills::utf16_of_byte(text, active.start),
                    "end": skills::utf16_of_byte(text, active.end),
                    "query": active.query,
                    "selected": selected,
                    "rows": rows,
                })
            }
        }
    }
}
