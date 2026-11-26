//! The cards that need the user — approval, question, permission, form — in every request
//! state, and the budget notice. A faint fill and a 3 px coloured left edge; no outline.
//! Keys act only while the card has focus.

use egui::{self, Key, Modifiers};
use yaac_core::ThreadKey;
use yaac_core::agents::root_of;
use yaac_core::attention::Mark;
use yaac_core::permission::{self, PermissionDecision};
use yaac_core::requests::{self, CardState, SECRET_MASK, card_state, decision_keys, started_by_agent};
use yaac_core::transcript::Transcript;
use yaac_protocol::{
    ApprovalAnswer, DecisionKind, ItemBody, QuestionAnswer, QuestionAnswers, RequestBody, RequestItem, RequestResponse,
};

use crate::app::Action;
use crate::icons;
use crate::theme::{self, size};
use crate::thread::View;
use crate::widgets;

pub(crate) fn show(ui: &mut egui::Ui, v: &mut View<'_>, viewing: &ThreadKey, key: &ThreadKey, t: &Transcript, index: usize, now: i64, actions: &mut Vec<Action>) {
    let item = &t.items()[index];
    match &item.body {
        ItemBody::Request(r) => request(ui, v, viewing, key, t, item, r, now, actions),
        ItemBody::Notice(n) => budget(ui, v, key, &n.text, item, actions),
        _ => {}
    }
}

fn budget(ui: &mut egui::Ui, v: &mut View<'_>, key: &ThreadKey, text: &str, item: &yaac_protocol::Item, actions: &mut Vec<Action>) {
    let p = v.p;
    let root = root_of(v.state, key);
    let exhausted = root.as_ref().and_then(|r| v.state.budgets.get(r)).is_some_and(|b| b.exhausted);
    let edge = if exhausted { p.needs } else { p.text2 };
    let focus = v.ui.focus_card.as_ref() == Some(&item.id);
    widgets::card(ui, &p, edge, |ui| {
        ui.horizontal_top(|ui| {
            widgets::mark(ui, &p, if exhausted { Mark::Paused } else { Mark::Done }, 18.0);
            ui.vertical(|ui| {
                let (head, rest) = text.split_once(':').unwrap_or((text, ""));
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    ui.label(egui::RichText::new(format!("{head}:")).font(theme::bold(size::UI + 0.5)));
                    ui.label(egui::RichText::new(rest).font(theme::ui(size::UI + 0.5)));
                });
                if !exhausted {
                    ui.label(widgets::secondary("Continued · the orchestration has a new allowance.", &p));
                    return;
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let c = widgets::primary(ui, &p, "Continue (+100 turns)", Some("Enter"));
                    if focus {
                        c.request_focus();
                        v.ui.focus_card = None;
                    }
                    if c.clicked()
                        && let Some(r) = &root
                    {
                        actions.push(Action::ContinueBudget(r.clone()));
                    }
                    if widgets::plain(ui, &p, "Review workers", None).clicked() {
                        actions.push(Action::Panel(yaac_core::agents::PanelInput::Dock));
                    }
                    widgets::link(ui, &p, "Change budgets", theme::ui(size::UI));
                });
            });
        });
    });
}

#[allow(clippy::too_many_arguments)]
fn request(ui: &mut egui::Ui, v: &mut View<'_>, viewing: &ThreadKey, key: &ThreadKey, t: &Transcript, item: &yaac_protocol::Item, r: &RequestItem, now: i64, actions: &mut Vec<Action>) {
    let p = v.p;
    let may_act = v.state.may_answer(key);
    let local = t.local_answers.get(&r.request_id).map(|l| l.summary.as_str());
    let target = v.state.target_name(&key.target);
    let st = card_state(r, may_act, v.state.client_id.as_ref(), local, &target, now);
    if let CardState::Automatic { text, detail } = &st {
        automatic(ui, v, item, text, detail);
        return;
    }
    let perm = permission::parse(&r.body);
    let edge = match &st {
        CardState::Pending { .. } | CardState::OutcomeUnknown => p.needs,
        CardState::Confirmed { .. } => p.done,
        _ => p.text2,
    };
    let top = ui.next_widget_position();
    widgets::card(ui, &p, edge, |ui| {
        let mut view_item;
        let item = if viewing != key {
            let header = yaac_core::copies::header(v.state, key);
            ui.horizontal_wrapped(|ui| {
                ui.label(widgets::secondary(header.text(), &p));
                if ui.link("Open").on_hover_text("Open worker thread").clicked() {
                    actions.push(Action::OpenAt(key.clone(), Some(item.id.clone())));
                }
            });
            view_item = item.clone();
            view_item.id = yaac_core::copies::copy_id(key, &item.id);
            &view_item
        } else { item };
        if let Some(place) = yaac_core::copies::answered_where(v.state, key, r, viewing) {
            ui.label(widgets::secondary(place, &p));
        }
        let title = match (&r.body, &perm) {
            (_, Some(_)) => "This agent wants to work with other agents".to_owned(),
            (RequestBody::Approval(a), _) => requests::approval_title(a).to_owned(),
            (RequestBody::Question(_), _) => "Question".to_owned(),
            (RequestBody::Form(_), _) => "Form".to_owned(),
            _ => "Request".to_owned(),
        };
        match &st {
            CardState::Pending { actionable } => {
                ui.horizontal(|ui| {
                    widgets::icon(ui, 18.0, |pa, c, d| match &r.body {
                        RequestBody::Approval(_) => icons::terminal(pa, c, d, p.text2),
                        _ => icons::warning(pa, c, d * 0.9, p.needs, p.raised),
                    });
                    ui.label(egui::RichText::new(&title).font(theme::bold(size::SECTION)));
                });
                ui.add_space(6.0);
                if !actionable {
                    body_summary(ui, v, r, perm.as_ref());
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        widgets::mark(ui, &p, Mark::NeedsYou, 14.0);
                        ui.label(widgets::secondary("Waiting for an answer in the other program.", &p));
                    });
                    return;
                }
                match (&r.body, &perm) {
                    (_, Some(pr)) => permission_body(ui, v, viewing, key, r, pr, item, actions),
                    (RequestBody::Approval(a), _) => {
                        if a.command_explanation.is_none() && requests::explanation_input(a).is_some() {
                            if v.state.hub.explanations.main.is_none() {
                                if ui.link(requests::CHOOSE_EXPLANATION).clicked() { actions.push(Action::SettingsPage(yaac_core::settings::Page::Approvals)); }
                            } else if t.explanations.get(&r.request_id) != Some(&false) { ui.label("Explaining…"); }
                        }
                        command_details(ui, v, a, item.id.as_str(), v.state.project_of(key).map(|p| p.folder.as_str()));
                        if let Some(cwd) = &a.cwd {
                            ui.horizontal(|ui| {
                                widgets::icon(ui, 14.0, |pa, c, d| icons::folder(pa, c, d, p.text2));
                                ui.label(egui::RichText::new(cwd.as_str()).font(theme::mono(size::MONO)).color(p.text2));
                            });
                        }
                        for ch in &a.changes {
                            ui.label(egui::RichText::new(format!("{} {}", ch.change, ch.path)).font(theme::mono(size::MONO)).color(p.text2));
                        }
                        if let Some(reason) = requests::approval_reason(a) {
                            ui.label(widgets::secondary(reason, &p));
                        }
                        if let Some(scope) = &a.scope {
                            ui.label(widgets::secondary(scope, &p));
                        }
                        if started_by_agent(t, &item.turn_id) {
                            ui.label(widgets::secondary("Started by an agent, not by you", &p));
                        }
                        ui.add_space(8.0);
                        approval_buttons(ui, v, viewing, key, r, &a.decisions, item, actions);
                    }
                    (RequestBody::Question(q), _) => question_body(ui, v, viewing, key, r, q, item, actions),
                    (RequestBody::Form(f), _) => {
                        ui.label(&f.message);
                        ui.label(widgets::secondary("This form must be answered in the harness itself.", &p));
                    }
                    _ => {
                        ui.label(widgets::secondary("This request must be answered in the harness itself.", &p));
                    }
                }
            }
            CardState::Submitted { summary } => {
                header_line(ui, &p, "Submitted", None);
                ui.label(format!("You chose: {summary}"));
                ui.horizontal(|ui| {
                    widgets::icon(ui, 14.0, |pa, c, d| icons::dashed_circle(pa, c, d * 0.4, egui::Stroke::new(1.4, p.text2)));
                    ui.label(widgets::secondary("Sent · waiting for the agent to confirm", &p));
                });
            }
            CardState::Confirmed { summary, when } => {
                header_line(ui, &p, "Confirmed", Some(Mark::Done));
                ui.horizontal_wrapped(|ui| {
                    ui.label(egui::RichText::new("Answered by you").font(theme::bold(size::UI)));
                    ui.label(format!("· {summary}"));
                    if let Some(w) = when {
                        ui.label(widgets::secondary(format!("· {w}"), &p));
                    }
                });
            }
            CardState::ResolvedElsewhere { text } => {
                header_line(ui, &p, "Resolved elsewhere", None);
                ui.label(text);
            }
            CardState::Expired => {
                header_line(ui, &p, "Expired", Some(Mark::Failed));
                ui.label("No longer needed · the turn ended before this was answered.");
            }
            CardState::OutcomeUnknown => {
                ui.horizontal(|ui| {
                    widgets::icon(ui, 18.0, |pa, c, d| icons::question(pa, c, d, p.needs));
                    ui.label(egui::RichText::new("Outcome unknown").font(theme::bold(size::SECTION)));
                });
                ui.label("YAAC lost contact while sending your answer. It may or may not have arrived.");
                ui.add_space(4.0);
                if widgets::plain(ui, &p, "Check latest status", None).clicked() {
                    actions.push(Action::CheckLatest(key.clone()));
                }
                ui.label(widgets::secondary("Your answer will not be sent again.", &p));
            }
            CardState::Other(s) => {
                header_line(ui, &p, s, None);
            }
            // Drawn as a line above, never as a card.
            CardState::Automatic { .. } => {}
        }
    });
    let rect = egui::Rect::from_min_max(top, egui::pos2(ui.max_rect().right(), ui.min_rect().bottom()));
    if r.state == yaac_protocol::RequestState::Pending && ui.is_rect_visible(rect) {
        v.ui.visible_requests.push(yaac_protocol::attach::RequestLocation {
            target: key.target.clone(), thread_id: key.thread.clone(), request_id: r.request_id.clone(),
        });
    }
}

/// A tool approval `yaacd` gave because the thread's mode never asks (P342): one quiet line
/// that opens to the harness's reason and the full input.
fn automatic(ui: &mut egui::Ui, v: &mut View<'_>, item: &yaac_protocol::Item, text: &str, detail: &str) {
    let p = v.p;
    let id = egui::Id::unique(("auto-approval", item.id.as_str()));
    let open = ui.data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
    if detail.is_empty() {
        ui.label(widgets::secondary(text, &p));
        return;
    }
    if widgets::disclosure_row(ui, &p, text, open, id) {
        ui.data_mut(|d| d.insert_temp(id, !open));
    }
    if open {
        ui.indent(id, |ui| {
            for line in detail.lines() {
                ui.label(egui::RichText::new(line).font(theme::mono(size::MONO)).color(p.text2));
            }
        });
    }
}

fn header_line(ui: &mut egui::Ui, p: &crate::theme::Palette, text: &str, mark: Option<Mark>) {
    ui.horizontal(|ui| {
        if let Some(m) = mark {
            widgets::mark(ui, p, m, 16.0);
        }
        ui.label(egui::RichText::new(text).font(theme::bold(size::UI + 0.5)));
    });
}

fn command_details(ui: &mut egui::Ui, v: &View<'_>, approval: &yaac_protocol::ApprovalRequest, id: &str, project: Option<&str>) {
    if let Some(explanation) = &approval.command_explanation {
        ui.label(widgets::secondary(requests::EXPLANATION_LABEL, &v.p));
        ui.label(explanation);
    }
    if let Some(command) = &approval.command {
        let (first, label) = requests::command_preview(command);
        if let Some(warning) = requests::command_warning(command, approval.cwd.as_ref().map(|p| p.as_str()), project) {
            ui.label(egui::RichText::new(format!("⚠ {warning}")).color(v.p.needs));
        }
        let id = egui::Id::unique(("approval-command", id));
        let mut open = ui.data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
        if widgets::disclosure_row(ui, &v.p, if open { "Hide command" } else { &label }, open, id) {
            open = !open;
            ui.data_mut(|d| d.insert_temp(id, open));
        }
        let text = egui::RichText::new(if open { command } else { first }).font(theme::mono(size::MONO));
        if open { ui.label(text); } else { ui.add(egui::Label::new(text).truncate()); }
    }
}

fn body_summary(ui: &mut egui::Ui, v: &View<'_>, r: &RequestItem, perm: Option<&permission::PermissionRequest>) {
    let p = v.p;
    match (&r.body, perm) {
        (_, Some(pr)) => {
            for a in &pr.actions {
                ui.label(a);
            }
        }
        (RequestBody::Approval(a), _) => {
            command_details(ui, v, a, r.request_id.as_str(), None);
        }
        (RequestBody::Question(q), _) => {
            for qq in &q.questions {
                ui.label(&qq.text);
            }
        }
        _ => {
            ui.label(widgets::secondary("A request", &p));
        }
    }
}

fn approval_buttons(
    ui: &mut egui::Ui,
    v: &mut View<'_>,
    viewing: &ThreadKey,
    key: &ThreadKey,
    r: &RequestItem,
    decisions: &[yaac_protocol::ApprovalDecision],
    item: &yaac_protocol::Item,
    actions: &mut Vec<Action>,
) {
    let p = v.p;
    let keys = decision_keys(decisions);
    let mut card_focused = false;
    let mut responses = Vec::new();
    ui.horizontal(|ui| {
        for (i, d) in decisions.iter().enumerate() {
            let label = d.label.as_str();
            let resp = if keys[i] == Some("Enter") {
                widgets::primary(ui, &p, label, keys[i])
            } else {
                widgets::plain(ui, &p, label, keys[i])
            };
            if i == 0 && v.ui.focus_card.as_ref() == Some(&item.id) {
                resp.request_focus();
                v.ui.focus_card = None;
            }
            card_focused |= resp.has_focus();
            responses.push((resp.clicked(), d.clone()));
            if resp.has_focus() {
                // The Commander's Allow and Deny act on this card only (D130).
                v.ui.card_focus = Some(item.id.clone());
            }
        }
    });
    // Esc denies only while this card has focus; Enter acts on the focused button (the first
    // allow, by default). Neither ever reaches the composer.
    let esc = card_focused && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
    for (i, (clicked, d)) in responses.into_iter().enumerate() {
        let pressed = clicked || (esc && keys[i] == Some("Esc"));
        if pressed {
            let verb = match d.kind {
                DecisionKind::Deny | DecisionKind::Abort => "Denied",
                _ => "Allowed",
            };
            actions.push(Action::Respond { viewing: viewing.clone(),
                key: key.clone(),
                request: r.request_id.clone(),
                response: RequestResponse::Approval(ApprovalAnswer { decision: d.id.clone(), reason: None }),
                summary: format!("{} ({verb})", d.label),
            });
        }
    }
}

fn question_body(
    ui: &mut egui::Ui,
    v: &mut View<'_>,
    viewing: &ThreadKey,
    key: &ThreadKey,
    r: &RequestItem,
    q: &yaac_protocol::QuestionRequest,
    item: &yaac_protocol::Item,
    actions: &mut Vec<Action>,
) {
    let p = v.p;
    let draft = v.ui.questions.entry((key.clone(), r.request_id.clone())).or_default();
    for qq in &q.questions {
        ui.horizontal(|ui| {
            if let Some(h) = &qq.header {
                egui::Frame::NONE.fill(p.faint).corner_radius(4).inner_margin(egui::Margin::symmetric(7, 2)).show(ui, |ui| {
                    ui.label(egui::RichText::new(h).font(theme::bold(size::SMALL)));
                });
            }
            ui.label(&qq.text);
        });
        let sel = draft.selected.entry(qq.id.clone()).or_default();
        for o in &qq.options {
            let mut on = sel.contains(&o.label);
            let changed = if qq.multi_select {
                ui.checkbox(&mut on, &o.label).changed()
            } else {
                ui.radio(on, &o.label).clicked() && {
                    on = true;
                    true
                }
            };
            if changed {
                if !qq.multi_select {
                    sel.clear();
                }
                if on {
                    sel.insert(o.label.clone());
                } else {
                    sel.remove(&o.label);
                }
            }
        }
        if qq.allow_other || qq.options.is_empty() {
            let other = draft.other.entry(qq.id.clone()).or_default();
            // A secret is masked while it is typed, and lives only in this widget's memory.
            let te = egui::TextEdit::singleline(other)
                .password(qq.secret)
                .hint_text(if qq.options.is_empty() { "Your answer…" } else { "Something else…" })
                .frame(egui::Frame::NONE.fill(p.faint).corner_radius(5).inner_margin(egui::Margin::symmetric(8, 5)))
                .desired_width(ui.available_width());
            ui.add(te);
        }
        ui.add_space(6.0);
    }
    ui.horizontal(|ui| {
        let send = widgets::primary(ui, &p, "Send answers", Some("Enter"));
        if v.ui.focus_card.as_ref() == Some(&item.id) {
            send.request_focus();
            v.ui.focus_card = None;
        }
        widgets::link(ui, &p, "Answer in my own words", theme::ui(size::UI));
        if send.clicked() {
            let draft = v.ui.questions.get(&(key.clone(), r.request_id.clone())).cloned().unwrap_or_default();
            let mut answers = std::collections::BTreeMap::new();
            let mut parts = Vec::new();
            for qq in &q.questions {
                let selected: Vec<String> = draft.selected.get(&qq.id).map(|s| s.iter().cloned().collect()).unwrap_or_default();
                let other = draft.other.get(&qq.id).cloned().filter(|s| !s.is_empty());
                if qq.secret {
                    parts.push(SECRET_MASK.to_owned());
                } else {
                    parts.extend(selected.iter().cloned());
                    if let Some(o) = &other {
                        parts.push(o.clone());
                    }
                }
                answers.insert(qq.id.clone(), QuestionAnswer { selected, other, redacted: false });
            }
            actions.push(Action::Respond { viewing: viewing.clone(),
                key: key.clone(),
                request: r.request_id.clone(),
                response: RequestResponse::Question(QuestionAnswers { answers }),
                summary: parts.join(" · "),
            });
        }
    });
}

fn permission_body(
    ui: &mut egui::Ui,
    v: &mut View<'_>,
    viewing: &ThreadKey,
    key: &ThreadKey,
    r: &RequestItem,
    pr: &permission::PermissionRequest,
    item: &yaac_protocol::Item,
    actions: &mut Vec<Action>,
) {
    let p = v.p;
    ui.label(widgets::secondary("It will:", &p));
    for a in &pr.actions {
        ui.label(a);
    }
    ui.add_space(8.0);
    ui.label(egui::RichText::new("Budget for this orchestration").font(theme::bold(size::UI)));
    let b = v.ui.budgets.entry((key.clone(), r.request_id.clone())).or_insert(pr.budget);
    ui.horizontal(|ui| {
        // Hours 0 is no time limit (P351).
        for (label, val, min) in [("Agent turns", &mut b.agent_turns, 1), ("Hours (0 = no limit)", &mut b.hours, 0)] {
            ui.label(widgets::secondary(label, &p));
            ui.add(egui::DragValue::new(val).range(min..=10_000));
            ui.add_space(10.0);
        }
    });
    ui.label(widgets::secondary("When a budget runs out, work pauses and asks you. Nothing is cancelled.", &p));
    ui.add_space(8.0);
    let budget = *b;
    let mut decision = None;
    let mut focused = false;
    ui.horizontal(|ui| {
        let allow = widgets::primary(ui, &p, "Allow", Some("Enter"));
        if v.ui.focus_card.as_ref() == Some(&item.id) {
            allow.request_focus();
            v.ui.focus_card = None;
        }
        focused |= allow.has_focus();
        if allow.clicked() {
            decision = Some(PermissionDecision::Allow);
        }
        let deny = widgets::plain(ui, &p, "Deny", Some("Esc"));
        focused |= deny.has_focus();
        if deny.clicked() {
            decision = Some(PermissionDecision::Deny);
        }
        let always = widgets::link(ui, &p, "Always allow in this chat", theme::ui(size::UI));
        focused |= always.has_focus();
        if always.clicked() {
            decision = Some(PermissionDecision::AlwaysAllow);
        }
    });
    if focused && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
        decision = Some(PermissionDecision::Deny);
    }
    if let Some(d) = decision {
        let response = permission::response(d, budget);
        let summary = permission::summary(&response).unwrap_or_default();
        if d == PermissionDecision::AlwaysAllow {
            actions.push(Action::SetAgents(key.clone(), yaac_core::state::AgentsPolicy::Allow));
        }
        actions.push(Action::Respond { viewing: viewing.clone(), key: key.clone(), request: r.request_id.clone(), response, summary });
    }
}
