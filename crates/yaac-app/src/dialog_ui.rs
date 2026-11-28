//! External dialogs in the egui window (docs/16-external-dialogs.md, docs/07-ui.md#external-dialogs):
//! one generic renderer that walks the integrator's layout tree, used by the request card in a
//! thread and by the inbox dialog opened from Needs you. Every decision — defaults, validation,
//! the answer, its summary, the source line — comes from [`yaac_core::dialogs`].

use egui::{self, RichText};
use yaac_core::dialogs::{self, ButtonStyle, DialogDraft, DraftValue, NEWER_YAAC};
use yaac_protocol::attach::RequestLocation;
use yaac_protocol::dialog_tree;
use yaac_protocol::{DialogNode, DialogRequest, DialogTextStyle, DialogTextTone, RequestState};

use crate::app::{Action, YaacApp};
use crate::theme::{self, Palette, size};
use crate::widgets;

/// Draws the dialog's layout; returns the id of the button pressed this frame.
pub(crate) fn layout(ui: &mut egui::Ui, p: &Palette, d: &DialogRequest, draft: &mut DialogDraft, salt: &str) -> Option<String> {
    let mut pressed = None;
    let enabled = dialogs::answerable(d);
    if let Some(problem) = draft.problems.get("").cloned() {
        ui.label(RichText::new(problem).color(p.failed));
    }
    ui.push_id(("dialog", salt), |ui| node(ui, p, &d.layout, draft, enabled, &mut pressed));
    pressed
}

fn number(n: Option<&serde_json::Number>) -> Option<f32> {
    #[allow(clippy::cast_possible_truncation, reason = "a UI gap in points")]
    n.and_then(serde_json::Number::as_f64).map(|v| v.clamp(0.0, 64.0) as f32)
}

fn tone(p: &Palette, t: Option<&DialogTextTone>) -> egui::Color32 {
    match t {
        Some(DialogTextTone::Muted) => p.text2,
        Some(DialogTextTone::Warning) => p.needs,
        Some(DialogTextTone::Error) => p.failed,
        Some(DialogTextTone::Success) => p.done,
        _ => p.text,
    }
}

fn input_header(ui: &mut egui::Ui, _p: &Palette, n: &DialogNode) {
    let Some(i) = dialog_tree::input(n) else { return };
    if let Some(l) = &i.label {
        let mark = if i.required == Some(true) { " *" } else { "" };
        ui.label(RichText::new(format!("{l}{mark}")).font(theme::bold(size::UI)));
    }
}

fn input_footer(ui: &mut egui::Ui, p: &Palette, n: &DialogNode, draft: &DialogDraft) {
    let Some(i) = dialog_tree::input(n) else { return };
    if let Some(h) = &i.help {
        ui.label(widgets::secondary(h, p));
    }
    if let Some(problem) = draft.problems.get(&i.id) {
        ui.label(RichText::new(problem).color(p.failed));
    }
}

fn text_field(ui: &mut egui::Ui, p: &Palette, value: &str, hint: &str, multiline: bool, secret: bool, enabled: bool) -> Option<String> {
    let mut s = value.to_owned();
    let frame = egui::Frame::NONE.fill(p.faint).corner_radius(5).inner_margin(egui::Margin::symmetric(8, 5));
    let te = if multiline { egui::TextEdit::multiline(&mut s).desired_rows(3) } else { egui::TextEdit::singleline(&mut s) };
    // A secret is masked while it is typed and lives only in the draft.
    let te = te.password(secret).hint_text(hint).frame(frame).desired_width(ui.available_width().min(480.0));
    let changed = ui.add_enabled(enabled, te).changed();
    changed.then_some(s)
}

#[allow(clippy::too_many_lines)]
fn node(ui: &mut egui::Ui, p: &Palette, n: &DialogNode, draft: &mut DialogDraft, enabled: bool, pressed: &mut Option<String>) {
    let on = |i: &yaac_protocol::DialogInput| enabled && i.disabled != Some(true);
    match n {
        DialogNode::Column(c) => {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = number(c.gap.as_ref()).unwrap_or(6.0);
                for child in &c.children {
                    node(ui, p, child, draft, enabled, pressed);
                }
            });
        }
        DialogNode::Row(c) => {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = number(c.gap.as_ref()).unwrap_or(8.0);
                for child in &c.children {
                    node(ui, p, child, draft, enabled, pressed);
                }
            });
        }
        DialogNode::Group(g) => {
            egui::Frame::NONE.stroke(egui::Stroke::new(1.0, p.border)).corner_radius(6).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
                if let Some(l) = &g.label {
                    ui.label(RichText::new(l).font(theme::bold(size::UI)));
                }
                for child in &g.children {
                    node(ui, p, child, draft, enabled, pressed);
                }
            });
        }
        DialogNode::Divider(_) => {
            ui.separator();
        }
        DialogNode::Text(t) => {
            let font = match t.style {
                Some(DialogTextStyle::Heading) => theme::bold(size::SECTION),
                Some(DialogTextStyle::Caption) => theme::ui(size::SMALL),
                Some(DialogTextStyle::Code) => theme::mono(size::MONO),
                _ => theme::ui(size::UI),
            };
            ui.label(RichText::new(&t.text).font(font).color(tone(p, t.tone.as_ref())));
        }
        DialogNode::Markdown(m) => {
            for para in m.text.split("\n\n") {
                widgets::rich_inline(ui, p, para, p.text, size::UI);
            }
        }
        DialogNode::Image(img) => {
            // Images are not fetched by the window; the alternative text stands in, and an
            // HTTPS image can be opened in the browser.
            if img.src.starts_with("https://") {
                ui.hyperlink_to(format!("🖼 {}", img.alt), &img.src);
            } else {
                ui.label(widgets::secondary(format!("🖼 {}", img.alt), p));
            }
        }
        DialogNode::Link(l) => {
            ui.hyperlink_to(&l.label, &l.url);
        }
        DialogNode::List(l) => {
            for item in &l.items {
                ui.label(format!("•  {item}"));
            }
        }
        DialogNode::Table(t) => {
            egui::Grid::new(ui.next_auto_id()).striped(true).show(ui, |ui| {
                for c in &t.columns {
                    ui.label(RichText::new(c).font(theme::bold(size::UI)));
                }
                ui.end_row();
                for row in &t.rows {
                    for cell in row {
                        ui.label(cell);
                    }
                    ui.end_row();
                }
            });
        }
        DialogNode::Progress(pr) => {
            #[allow(clippy::cast_possible_truncation, reason = "a fraction for drawing")]
            let bar = match pr.value.as_ref().and_then(serde_json::Number::as_f64) {
                Some(v) => egui::ProgressBar::new(v.clamp(0.0, 1.0) as f32).show_percentage(),
                None => egui::ProgressBar::new(0.0).animate(true),
            };
            ui.add(bar.desired_width(ui.available_width().min(480.0)));
        }
        DialogNode::TextInput(t) => {
            input_header(ui, p, n);
            let secret = dialog_tree::is_secret(n);
            let hint = t.placeholder.as_deref().unwrap_or("");
            if let Some(s) = text_field(ui, p, draft.text(&t.input.id), hint, t.multiline == Some(true), secret, on(&t.input)) {
                draft.set(&t.input.id, DraftValue::Text(s));
            }
            input_footer(ui, p, n, draft);
        }
        DialogNode::Number(num) => {
            input_header(ui, p, n);
            if let Some(s) = text_field(ui, p, draft.text(&num.input.id), "0", false, false, on(&num.input)) {
                draft.set(&num.input.id, DraftValue::Text(s));
            }
            input_footer(ui, p, n, draft);
        }
        DialogNode::Date(dt) => {
            input_header(ui, p, n);
            if let Some(s) = text_field(ui, p, draft.text(&dt.input.id), "YYYY-MM-DD", false, false, on(&dt.input)) {
                draft.set(&dt.input.id, DraftValue::Text(s));
            }
            input_footer(ui, p, n, draft);
        }
        DialogNode::Slider(s) => {
            input_header(ui, p, n);
            let min = s.min.as_f64().unwrap_or(0.0);
            let max = s.max.as_f64().unwrap_or(min).max(min);
            let mut v = draft.text(&s.input.id).parse::<f64>().unwrap_or(min);
            let mut slider = egui::Slider::new(&mut v, min..=max);
            if let Some(step) = s.step.as_ref().and_then(serde_json::Number::as_f64).filter(|st| *st > 0.0) {
                slider = slider.step_by(step);
            }
            if ui.add_enabled(on(&s.input), slider).changed() {
                let text = if v.fract() == 0.0 && v.abs() < 1e15 { format!("{v:.0}") } else { v.to_string() };
                draft.set(&s.input.id, DraftValue::Text(text));
            }
            input_footer(ui, p, n, draft);
        }
        DialogNode::Checkbox(c) | DialogNode::Toggle(c) => {
            let mut v = draft.flag(&c.input.id);
            let label = c.input.label.clone().unwrap_or_default();
            let changed = if matches!(n, DialogNode::Toggle(_)) {
                ui.horizontal(|ui| {
                    let r = ui.add_enabled_ui(on(&c.input), |ui| widgets::switch(ui, p, v)).inner;
                    ui.label(&label);
                    if r.clicked() {
                        v = !v;
                    }
                    r.clicked()
                })
                .inner
            } else {
                ui.add_enabled(on(&c.input), egui::Checkbox::new(&mut v, label)).changed()
            };
            if changed {
                draft.set(&c.input.id, DraftValue::Bool(v));
            }
            input_footer(ui, p, n, draft);
        }
        DialogNode::Select(s) => {
            input_header(ui, p, n);
            let id = s.input.id.clone();
            let current = draft.choice(&id).and_then(|c| s.options.iter().find(|o| o.value == c)).map(|o| o.label.clone());
            let shown = current.or_else(|| s.placeholder.clone()).unwrap_or_else(|| "Choose…".to_owned());
            let mut chosen = None;
            ui.add_enabled_ui(on(&s.input), |ui| {
                egui::ComboBox::from_id_salt(("dialog-select", &id)).selected_text(shown).width(ui.available_width().min(320.0)).show_ui(ui, |ui| {
                    for o in &s.options {
                        let r = ui.selectable_label(draft.choice(&id) == Some(o.value.as_str()), &o.label);
                        let r = match &o.detail { Some(d) => r.on_hover_text(d), None => r };
                        if r.clicked() {
                            chosen = Some(o.value.clone());
                        }
                    }
                });
            });
            if let Some(c) = chosen {
                draft.set(&id, DraftValue::Choice(Some(c)));
            }
            input_footer(ui, p, n, draft);
        }
        DialogNode::Radio(s) => {
            input_header(ui, p, n);
            let id = s.input.id.clone();
            for o in &s.options {
                let selected = draft.choice(&id) == Some(o.value.as_str());
                if ui.add_enabled(on(&s.input), egui::RadioButton::new(selected, &o.label)).clicked() {
                    draft.set(&id, DraftValue::Choice(Some(o.value.clone())));
                }
                if let Some(d) = &o.detail {
                    ui.indent(("radio-detail", &o.value), |ui| ui.label(widgets::secondary(d, p)));
                }
            }
            input_footer(ui, p, n, draft);
        }
        DialogNode::MultiSelect(m) => {
            input_header(ui, p, n);
            let id = m.input.id.clone();
            for o in &m.options {
                let mut v = draft.chosen(&id, &o.value);
                if ui.add_enabled(on(&m.input), egui::Checkbox::new(&mut v, &o.label)).changed() {
                    draft.toggle_choice(&id, &o.value);
                }
            }
            input_footer(ui, p, n, draft);
        }
        DialogNode::Button(b) => {
            let r = ui.add_enabled_ui(enabled, |ui| match dialogs::button_style(b) {
                ButtonStyle::Primary => widgets::primary(ui, p, &b.label, None),
                ButtonStyle::Danger => crate::dialogs::danger(ui, p, &b.label),
                ButtonStyle::Normal => widgets::plain(ui, p, &b.label, None),
            });
            if r.inner.clicked() {
                *pressed = Some(b.id.clone());
            }
        }
        DialogNode::Unknown(_) => {
            ui.label(widgets::secondary(NEWER_YAAC, p).italics());
        }
    }
}

/// The lines above the layout: who asks, and whether it is still there.
pub(crate) fn source(ui: &mut egui::Ui, p: &Palette, d: &DialogRequest, target_name: &str) {
    ui.label(widgets::secondary(dialogs::source_line(d, target_name), p));
    if let Some(gone) = dialogs::gone_line(d) {
        ui.label(RichText::new(gone).color(p.needs));
    }
}

/// Validates the press of `button` and gives the answer, or keeps the problems on the draft.
pub(crate) fn press(d: &DialogRequest, draft: &mut DialogDraft, button: &str) -> Option<yaac_protocol::DialogAnswer> {
    match dialogs::answer(&d.layout, draft, button) {
        Ok(a) => Some(a),
        Err(problems) => {
            draft.problems = problems;
            None
        }
    }
}

/// The dialog of a target's inbox, opened from Needs you or a notification: a modal card over
/// the shell. Closing it (× or Esc) dismisses it — the integrator is told it was cancelled;
/// **Later** only hides it, and it stays in Needs you.
pub(crate) fn inbox_modal(ctx: &egui::Context, app: &mut YaacApp, actions: &mut Vec<Action>) {
    let Some((target, request_id)) = app.ui.inbox_dialog.clone() else { return };
    let Some(request) = app.client.state.inbox.request(&target, &request_id) else {
        // Answered, withdrawn, or gone with its target.
        app.ui.inbox_dialog = None;
        return;
    };
    let Some(d) = dialogs::of(&request).cloned() else {
        app.ui.inbox_dialog = None;
        return;
    };
    let p = app.p;
    crate::dialogs::scrim(ctx, &p, "inbox-dialog");
    // Esc closes it, as × does: a cancellation (the integrator is told).
    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
        if request.state == RequestState::Pending {
            actions.push(Action::RespondInbox { target, request: request_id, answer: dialogs::cancel() });
        }
        app.ui.inbox_dialog = None;
        return;
    }
    let target_name = app.client.state.target_name(&target);
    let key = (target.clone(), yaac_protocol::ThreadId::new(yaac_protocol::attach::INBOX_THREAD), request_id.clone());
    let mut draft = app.ui.dialog_drafts.remove(&key).unwrap_or_else(|| DialogDraft::new(&d.layout));
    let mut keep = true;
    egui::Window::new("inbox-dialog")
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .frame(egui::Frame::popup(&ctx.global_style()).fill(p.raised).inner_margin(egui::Margin::same(20)))
        .show(ctx, |ui| {
            ui.set_width(520.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(&d.title).font(theme::bold(size::SECTION + 1.5)));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let pending = request.state == RequestState::Pending;
                    let close = ui.add_enabled(pending, egui::Button::new("×").frame(false)).on_hover_text(format!("Close · {} is told you cancelled", d.source.name));
                    if close.clicked() {
                        actions.push(Action::RespondInbox { target: target.clone(), request: request_id.clone(), answer: dialogs::cancel() });
                        keep = false;
                    }
                    if widgets::link(ui, &p, "Later", theme::ui(size::UI)).on_hover_text("Hide it; it stays in Needs you").clicked() {
                        keep = false;
                    }
                });
            });
            source(ui, &p, &d, &target_name);
            ui.add_space(10.0);
            match &request.state {
                RequestState::Pending => {
                    egui::ScrollArea::vertical().max_height(ctx.content_rect().height() * 0.7).show(ui, |ui| {
                        if let Some(button) = layout(ui, &p, &d, &mut draft, request_id.as_str())
                            && let Some(answer) = press(&d, &mut draft, &button)
                        {
                            actions.push(Action::RespondInbox { target: target.clone(), request: request_id.clone(), answer });
                        }
                    });
                    if dialogs::buttons(&d.layout).is_empty() {
                        ui.add_space(8.0);
                        if widgets::plain(ui, &p, "Dismiss", None).clicked() {
                            actions.push(Action::RespondInbox { target: target.clone(), request: request_id.clone(), answer: dialogs::cancel() });
                            keep = false;
                        }
                    }
                    app.ui.visible_requests.push(RequestLocation::inbox(target.clone(), request_id.clone()));
                }
                RequestState::Submitted => {
                    let summary = app.client.state.inbox.local_summary(&target, &request_id).unwrap_or("your answer");
                    ui.label(format!("You chose: {summary}"));
                    ui.label(widgets::secondary(format!("Sent · waiting for {} to confirm", target_name), &p));
                }
                RequestState::Unknown => {
                    ui.label(RichText::new("Outcome unknown").font(theme::bold(size::SECTION)));
                    ui.label("YAAC lost contact while sending your answer. It may or may not have arrived.");
                    if widgets::plain(ui, &p, "Check latest status", None).clicked() {
                        actions.push(Action::CheckInbox(target.clone()));
                    }
                    ui.label(widgets::secondary("Your answer will not be sent again.", &p));
                }
                other => {
                    ui.label(widgets::secondary(other.as_str(), &p));
                }
            }
        });
    app.ui.dialog_drafts.insert(key, draft);
    if !keep {
        app.ui.inbox_dialog = None;
    }
}
