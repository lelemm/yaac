//! The composer (markdown styled in place), the control row, the fork menu, and the
//! new-thread draft.

use std::sync::Arc;

use eframe::egui::{self, Key, KeyboardShortcut, Modifiers, Margin, text::LayoutJob};
use yaac_core::ThreadKey;
use yaac_core::attention::AttentionMap;
use yaac_core::controls::{Choice, ControlRow, ControlRowView, StartedInputs, draft_row, started_row};
use yaac_core::header::{ChipView, ComposerView, composer, short_provider};
use yaac_core::images::ImageRoute;
use yaac_core::markstyle::{self, Style};
use yaac_core::mode_confirm::ModeTarget;
use yaac_core::compose::{self, EnterAction};
use yaac_core::prefs::SendKey;

use crate::app::{Action, Menu, YaacApp};
use crate::icons;
use crate::theme::{self, Palette, size};
use crate::widgets;

/// The composer layouter: markdown styled in place, syntax left visible, bounded work.
fn layout(ui: &egui::Ui, text: &str, wrap: f32, p: &Palette) -> Arc<egui::Galley> {
    let mut job = LayoutJob::default();
    job.wrap.max_width = wrap;
    for span in markstyle::style(text, 64 * 1024) {
        let (font, color) = match span.style {
            Style::Plain => (theme::ui(size::PROSE - 0.5), p.text),
            Style::Marker => (theme::ui(size::PROSE - 0.5), p.text2),
            Style::Heading(_) => (theme::bold(size::PROSE + 2.0), p.text),
            Style::Bold => (theme::bold(size::PROSE - 0.5), p.text),
            Style::Italic => (theme::ui(size::PROSE - 0.5), p.text),
            Style::Code | Style::CodeBlock => (theme::mono(size::MONO + 0.5), p.text),
        };
        let mut fmt = egui::TextFormat { font_id: font, color, ..Default::default() };
        if span.style == Style::Italic {
            fmt.italics = true;
        }
        if matches!(span.style, Style::Code | Style::CodeBlock) {
            fmt.background = p.code_bg;
        }
        job.append(&text[span.range], 0.0, fmt);
    }
    ui.fonts_mut(|f| f.layout_job(job))
}

fn track_ime(ui: &egui::Ui, composing: &mut bool) {
    ui.input(|i| {
        for e in &i.events {
            if let egui::Event::Ime(ime) = e {
                match ime {
                    egui::ImeEvent::Preedit { text, .. } => *composing = !text.is_empty(),
                    egui::ImeEvent::Commit(_) => *composing = false,
                    _ => {}
                }
            }
        }
    });
}

/// The text box. Returns true when Enter (not Shift+Enter, never during an input-method
/// composition) asked to send.
#[allow(clippy::too_many_arguments, reason = "one call site per composer; a struct would only rename the arguments")]
fn text_box(ui: &mut egui::Ui, text: &mut String, id: egui::Id, placeholder: &str, p: &Palette, composing: &mut bool, enabled: bool, focus: &mut bool, send_key: SendKey) -> bool {
    track_ime(ui, composing);
    let focused = ui.memory(|m| m.has_focus(id));
    let newline = match send_key {
        SendKey::Enter => KeyboardShortcut::new(Modifiers::SHIFT, Key::Enter),
        SendKey::CtrlEnter => KeyboardShortcut::new(Modifiers::NONE, Key::Enter),
    };
    // Enter is decided here with exact modifiers (egui's shortcut matching ignores an extra
    // Shift, which made Shift+Enter send). A press that sends, or that does nothing, is taken
    // out of the input so the text box does not also break the line.
    let mut send = false;
    if focused {
        ui.input_mut(|i| {
            let comp = *composing;
            i.events.retain(|e| match e {
                egui::Event::Key { key: Key::Enter, pressed: true, modifiers, .. } => {
                    match compose::enter(send_key, modifiers.shift, modifiers.command || modifiers.ctrl, comp) {
                        EnterAction::Send => {
                            send |= enabled;
                            false
                        }
                        EnterAction::Newline => true,
                        EnterAction::None => comp,
                    }
                }
                _ => true,
            });
        });
    }
    let pp = *p;
    let mut layouter = move |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap: f32| layout(ui, buf.as_str(), wrap, &pp);
    // Two lines empty; one more per line of text (wrapped lines counted) up to five; then the
    // text scrolls inside.
    let width = ui.available_width();
    let lines = if text.is_empty() { 1 } else { layout(ui, text, width - 8.0, p).rows.len() };
    let (rows, _) = compose::rows(lines);
    let row_h = ui.fonts_mut(|f| f.row_height(&theme::ui(size::PROSE - 0.5)));
    let te = egui::TextEdit::multiline(text)
        .id(id)
        .frame(egui::Frame::NONE)
        .hint_text(placeholder)
        .font(theme::ui(size::PROSE - 0.5))
        .desired_rows(rows)
        .desired_width(width)
        .return_key(newline)
        .interactive(enabled)
        .layouter(&mut layouter);
    // Always inside the same scroll area (it shrinks to the text below five lines): the
    // widget tree does not change when the sixth line arrives, so the text box's own "keep
    // the caret in view" reaches it on that very frame (D102).
    let out = egui::ScrollArea::vertical()
        .id_salt(("composer-scroll", id))
        .max_height(row_h * compose::MAX_ROWS as f32 + 2.0)
        .auto_shrink([false, true])
        .show(ui, |ui| ui.add(te));
    ui.data_mut(|d| d.insert_temp(id.with("scroll-offset"), out.state.clamped_offset().y));
    let r = out.inner;
    if *focus {
        r.request_focus();
        *focus = false;
    }
    send
}

fn chips(ui: &mut egui::Ui, p: &Palette, chips: &[ChipView]) {
    if chips.is_empty() {
        return;
    }
    ui.horizontal_wrapped(|ui| {
        for c in chips {
            egui::Frame::NONE.fill(p.raised).corner_radius(5).inner_margin(Margin::symmetric(8, 5)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    let image = c.image;
                    widgets::icon(ui, 15.0, |pa, ce, d| if image { icons::image(pa, ce, d, p.text) } else { icons::file(pa, ce, d, p.text, false) });
                    match c.route {
                        ImageRoute::AsFileCannotSee => {
                            widgets::icon(ui, 13.0, |pa, ce, d| icons::warning(pa, ce, d, p.needs, p.raised));
                            ui.label(&c.name);
                            ui.label(widgets::secondary("— this model cannot see images; it will be sent as a file", p));
                        }
                        ImageRoute::Ask => {
                            ui.label(&c.name);
                            ui.label(widgets::secondary("send as", p));
                            widgets::plain(ui, p, "image", None);
                            widgets::plain(ui, p, "file", None);
                        }
                        _ => {
                            ui.label(&c.name);
                        }
                    }
                    if let yaac_core::header::ChipStatus::Uploading { fraction: Some(pr), .. } = c.status {
                        let (r, _) = ui.allocate_exact_size(egui::vec2(90.0, 4.0), egui::Sense::hover());
                        ui.painter().rect_filled(r, 2.0, p.faint);
                        ui.painter().rect_filled(egui::Rect::from_min_size(r.min, egui::vec2(90.0 * pr, 4.0)), 2.0, p.text2);
                        ui.label(widgets::secondary(format!("{}%", (pr * 100.0).round()), p));
                    }
                });
            });
        }
    });
    ui.add_space(4.0);
}

/// Everything under the transcript of a started thread.
pub(crate) fn bottom(ui: &mut egui::Ui, app: &mut YaacApp, map: &AttentionMap, key: &ThreadKey, actions: &mut Vec<Action>) {
    let p = app.p;
    // An arriving request does not take focus: it is announced here.
    if let Some(a) = app.client.state.arrivals.last() {
        let title = app.client.state.summary(&a.key).map_or_else(String::new, |s| s.title.clone());
        let k = a.key.clone();
        ui.horizontal(|ui| {
            widgets::icon(ui, 14.0, |pa, c, d| icons::warning(pa, c, d, p.needs, p.bg));
            let r = ui.add(egui::Label::new(widgets::secondary(format!("{title} needs {} {} · Ctrl+J to go there", article(a.kind.word()), a.kind.word()), &p)).sense(egui::Sense::click()));
            if r.clicked() {
                actions.push(Action::OpenRequest(k));
            }
        });
        ui.add_space(4.0);
    }
    for e in app.client.errors.iter().filter(|e| e.key.as_ref() == Some(key)) {
        ui.label(egui::RichText::new(&e.text).font(theme::ui(size::SMALL)).color(p.failed));
    }
    match composer(&app.client.state, map, key) {
        ComposerView::Absent { holder_line } => {
            egui::Frame::NONE.fill(p.faint).corner_radius(6).inner_margin(Margin::symmetric(12, 10)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    widgets::icon(ui, 15.0, |pa, c, d| icons::info(pa, c, d, p.text2));
                    ui.label(widgets::secondary(holder_line, &p));
                });
            });
        }
        ComposerView::Shown { placeholder, dimmed_hint, stop, chips: chip_views, ready, .. } => {
            let dimmed = dimmed_hint.is_some();
            let id = egui::Id::unique(("composer", key.to_string()));
            let mut send = false;
            let inner = widgets::faint_frame(&p).inner_margin(Margin::symmetric(14, 10)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                if dimmed {
                    ui.multiply_opacity(0.6);
                }
                chips(ui, &p, &chip_views);
                ui.horizontal(|ui| {
                    let btn_w = 96.0;
                    ui.vertical(|ui| {
                        ui.set_width(ui.available_width() - btn_w);
                        let draft = app.client.state.drafts.entry(key.clone()).or_default();
                        send = text_box(ui, &mut draft.text, id, &placeholder, &p, &mut app.ui.ime_composing, !dimmed, &mut app.ui.focus_composer, app.ui.send_key);
                        if let Some(h) = &dimmed_hint {
                            ui.label(widgets::secondary(h, &p));
                        }
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if stop && !dimmed {
                            let r = widgets::plain_colored(ui, &p, "Stop", p.failed);
                            icons::stop_square(ui.painter(), egui::pos2(r.rect.left() + 2.0, r.rect.center().y), 0.0, p.failed);
                            if r.clicked() {
                                actions.push(Action::Stop(key.clone()));
                            }
                        } else if !dimmed {
                            let has_text = app.client.state.drafts.get(key).is_some_and(|d| !d.text.trim().is_empty());
                            let r = if has_text && ready { widgets::primary(ui, &p, "Send", Some("Enter")) } else { widgets::plain(ui, &p, "Send", Some("Enter")) };
                            if r.clicked() {
                                send = true;
                            }
                        }
                    });
                });
            });
            if ui.memory(|m| m.has_focus(id)) {
                widgets::focus_ring(ui, inner.response.rect, &p);
            }
            if send && !stop {
                actions.push(Action::Send(key.clone()));
            }
            // Esc stops the running turn from the composer.
            if stop && ui.memory(|m| m.has_focus(id)) && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
                actions.push(Action::Stop(key.clone()));
            }
        }
    }
    ui.add_space(6.0);
    let s = app.client.state.summary(key).cloned();
    let Some(s) = s else { return };
    let t = app.client.state.transcripts.get(key);
    let providers = app.client.state.target(&key.target).map(|t| t.providers.clone()).unwrap_or_default();
    let target_name = app.client.state.target_name(&key.target);
    let view = started_row(StartedInputs {
        providers: &providers,
        instance: &s.instance,
        target_name: &target_name,
        effective: t.and_then(|t| t.effective.as_ref()),
        summary_model: s.model.as_ref(),
        next: app.client.state.next_settings.get(key),
        context: t.and_then(|t| t.context.as_ref()),
        may_act: app.client.state.may_act(key),
        agents: s.spawned_by.is_none().then(|| app.client.state.agents_policy(key)),
    });
    if let ControlRowView::Row(row) = view {
        control_row(ui, app, &row, &Target::Thread(key.clone()), actions);
    }
}

fn article(word: &str) -> &'static str {
    if word.starts_with(['a', 'e', 'i', 'o', 'u']) { "an" } else { "a" }
}

#[derive(Clone)]
enum Target {
    Thread(ThreadKey),
    Draft(u64),
}

fn choice_menu<T: Clone + PartialEq>(
    ui: &mut egui::Ui,
    app: &mut YaacApp,
    menu: &Menu,
    choice: &Choice<T>,
    on_pick: impl Fn(T) -> Action,
    actions: &mut Vec<Action>,
) {
    let p = app.p;
    let r = if choice.warn { widgets::selector_warn(ui, &p, &choice.label) } else { widgets::selector(ui, &p, &choice.label, true) };
    if r.clicked() {
        app.ui.toggle_menu(menu.clone());
    }
    let mut open = app.ui.menu.as_ref() == Some(menu);
    egui::Popup::from_response(&r)
        .kind(egui::PopupKind::Menu)
        .align(egui::RectAlign::TOP_START)
        .open_bool(&mut open)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
        .show(|ui| {
            ui.set_min_width(180.0);
            for (id, label, is_default) in &choice.options {
                let ticked = choice.current.as_ref() == Some(id);
                let text = if *is_default { format!("{label}   (default)") } else { label.clone() };
                if ui.selectable_label(ticked, text).clicked() {
                    actions.push(on_pick(id.clone()));
                }
            }
        });
    if !open && app.ui.menu.as_ref() == Some(menu) {
        app.ui.menu = None;
    }
}

fn control_row(ui: &mut egui::Ui, app: &mut YaacApp, row: &ControlRow, target: &Target, actions: &mut Vec<Action>) {
    let p = app.p;
    // Where the picker's list came from: the target now, or the cache.
    let footer = {
        let st = &app.client.state;
        let tid = match target {
            Target::Thread(k) => Some(k.target.clone()),
            Target::Draft(id) => st.new_threads.iter().find(|d| d.id == *id).map(|d| d.target.clone()),
        };
        tid.and_then(|t| st.target(&t)).map(|t| {
            let when = if t.providers_fresh { "updated just now" } else { "from the cache" };
            format!("Read from {} · {when}", t.entry.name)
        })
    };
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        if let Some(m) = &row.model {
            let r = widgets::selector_pair(ui, &p, &m.instance_label, &m.model_label, m.changeable || !m.groups.is_empty());
            if r.clicked() {
                app.ui.toggle_menu(Menu::Model);
            }
            let mut open = app.ui.menu == Some(Menu::Model);
            egui::Popup::from_response(&r)
                .kind(egui::PopupKind::Menu)
                .align(egui::RectAlign::TOP_START)
                .open_bool(&mut open)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
                .show(|ui| {
                    ui.set_min_width(300.0);
                    for g in &m.groups {
                        if let Some(reason) = &g.reason {
                            // The whole group is unavailable: its name in the unavailable
                            // style, and the reason beside it.
                            widgets::unavailable(ui, &p, &g.name, reason);
                            ui.add_space(4.0);
                            continue;
                        }
                        ui.label(egui::RichText::new(&g.name).font(theme::bold(size::UI)));
                        for model in &g.models {
                            ui.horizontal(|ui| {
                                let mut text = model.name.clone();
                                if model.is_default {
                                    text.push_str("   default");
                                }
                                let r = ui.add_enabled(model.enabled, egui::Button::selectable(model.selected, text));
                                if model.image.is_supported() {
                                    widgets::icon(ui, 14.0, |pa, c, d| icons::image(pa, c, d, p.text2));
                                }
                                if let Some(e) = &model.effort_range {
                                    ui.label(widgets::secondary(e, &p));
                                }
                                if r.clicked() {
                                    actions.push(match target {
                                        Target::Thread(k) => Action::SetModel(k.clone(), model.id.clone()),
                                        Target::Draft(id) => Action::DraftModel(*id, g.instance.clone(), model.id.clone()),
                                    });
                                }
                            });
                        }
                        ui.add_space(4.0);
                    }
                    if let Some(f) = &footer {
                        ui.add_space(2.0);
                        ui.label(widgets::secondary(f, &p));
                    }
                });
            if !open && app.ui.menu == Some(Menu::Model) {
                app.ui.menu = None;
            }
        }
        if let Some(e) = &row.effort {
            let t = target.clone();
            choice_menu(ui, app, &Menu::Effort, e, move |id| match &t {
                Target::Thread(k) => Action::SetEffort(k.clone(), id),
                Target::Draft(d) => Action::DraftEffort(*d, id),
            }, actions);
        }
        if let Some(m) = &row.mode {
            let t = target.clone();
            choice_menu(ui, app, &Menu::Mode, m, move |id| match &t {
                Target::Thread(k) => Action::ChooseMode(ModeTarget::Thread(k.clone()), id),
                Target::Draft(d) => Action::ChooseMode(ModeTarget::Draft(*d), id),
            }, actions);
        }
        if let (Some(a), Target::Thread(k)) = (&row.agents, target) {
            let k = k.clone();
            choice_menu(ui, app, &Menu::Agents, a, move |pol| Action::SetAgents(k.clone(), pol), actions);
        }
        if let Some(pct) = row.context_percent {
            ui.add_space(12.0);
            let (r, _) = ui.allocate_exact_size(egui::vec2(64.0, 26.0), egui::Sense::hover());
            let bar = egui::Rect::from_center_size(r.center(), egui::vec2(64.0, 4.0));
            ui.painter().rect_filled(bar, 2.0, p.faint);
            ui.painter().rect_filled(egui::Rect::from_min_size(bar.min, egui::vec2(64.0 * f32::from(pct) / 100.0, 4.0)), 2.0, p.text2);
            ui.label(widgets::secondary(format!("{pct}%"), &p)).on_hover_text("How full the context window is");
        }
        if row.fork
            && let Target::Thread(k) = target
        {
            ui.add_space(8.0);
            if widgets::plain(ui, &p, "Fork", None).clicked() {
                actions.push(Action::OpenFork(k.clone()));
            }
        }
    });
}

/// A new thread before its first message.
pub(crate) fn draft_view(ui: &mut egui::Ui, app: &mut YaacApp, id: u64, actions: &mut Vec<Action>) {
    let p = app.p;
    let Some(d) = app.client.state.new_threads.iter().find(|d| d.id == id).cloned() else { return };
    let target_name = app.client.state.target_name(&d.target);
    egui::Panel::top("draft-header").show_separator_line(false).frame(egui::Frame::NONE.inner_margin(Margin { left: 28, right: 28, top: 14, bottom: 6 })).show(ui, |ui| {
        ui.label(egui::RichText::new("New thread").font(theme::bold(size::TITLE)));
        let project = d.project.as_ref().and_then(|pid| app.client.state.projects.iter().find(|x| &x.id == pid)).map(|x| x.name.clone());
        let crumb = match project {
            Some(name) => format!("{target_name} · {name}"),
            None => format!("{target_name} · {}", d.cwd.as_ref().map_or_else(|| "no project yet".to_owned(), ToString::to_string)),
        };
        ui.label(widgets::secondary(crumb, &p));
    });
    egui::Panel::bottom("draft-composer").show_separator_line(false).frame(egui::Frame::NONE.inner_margin(Margin { left: 28, right: 28, top: 6, bottom: 10 })).show(ui, |ui| {
        // One selector: the project (a new thread starts in the root of its folder).
        let project_label = d
            .project
            .as_ref()
            .and_then(|pid| app.client.state.projects.iter().find(|x| &x.id == pid))
            .map_or_else(|| format!("Project: none — {target_name}"), |x| format!("Project: {} — {}", x.name, app.client.state.target_name(&x.target)));
        let r = widgets::selector(ui, &p, &project_label, true);
        if r.clicked() {
            app.ui.toggle_menu(Menu::Project);
        }
        let mut open = app.ui.menu == Some(Menu::Project);
        let groups = yaac_core::first_run::project_picker(&app.client.state, d.project.as_deref());
        let mut new_project = false;
        egui::Popup::from_response(&r).kind(egui::PopupKind::Menu).align(egui::RectAlign::TOP_START).open_bool(&mut open).close_behavior(egui::PopupCloseBehavior::CloseOnClick).show(|ui| {
            ui.set_min_width(280.0);
            for g in &groups {
                ui.horizontal(|ui| {
                    ui.label(widgets::secondary(&g.name, &p));
                    if let Some(n) = &g.note {
                        ui.label(widgets::secondary(format!("· {n}"), &p));
                    }
                });
                for x in &g.projects {
                    let r = ui.selectable_label(x.selected, &x.name).on_hover_text(&x.folder);
                    if r.clicked() {
                        actions.push(Action::DraftProject(id, x.id.clone()));
                    }
                }
                ui.add_space(4.0);
            }
            if ui.selectable_label(false, "New project…").clicked() {
                new_project = true;
            }
        });
        if !open && app.ui.menu == Some(Menu::Project) {
            app.ui.menu = None;
        }
        if new_project {
            let folder = d.cwd.as_ref().map_or_else(|| "~".to_owned(), ToString::to_string);
            let mut cp = crate::sidebar::create_project_for(app, &d.target, &folder);
            cp.for_draft = Some(id);
            app.ui.create_project = Some(cp);
        }
        ui.add_space(6.0);
        let providers = app.client.state.target(&d.target).map(|t| t.providers.clone()).unwrap_or_default();
        let name = d
            .instance
            .as_ref()
            .and_then(|i| providers.iter().find(|p| &p.instance == i))
            .or_else(|| providers.iter().find(|p| p.status == yaac_protocol::ProviderStatus::Ready))
            .map_or("the agent".to_owned(), |p| short_provider(&p.display_name, p.name.as_deref()));
        let mut send = false;
        let eid = egui::Id::unique(("draft", id));
        let inner = widgets::faint_frame(&p).inner_margin(Margin::symmetric(14, 10)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(ui.available_width() - 96.0);
                    if let Some(dd) = app.client.state.new_threads.iter_mut().find(|x| x.id == id) {
                        send = text_box(ui, &mut dd.draft.text, eid, &format!("Message {name}…"), &p, &mut app.ui.ime_composing, true, &mut app.ui.focus_composer, app.ui.send_key);
                    }
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::primary(ui, &p, "Send", Some("Enter")).clicked() {
                        send = true;
                    }
                });
            });
        });
        if ui.memory(|m| m.has_focus(eid)) {
            widgets::focus_ring(ui, inner.response.rect, &p);
        }
        if send {
            actions.push(Action::StartDraft(id));
        }
        ui.add_space(6.0);
        let row = draft_row(&providers, &target_name, &d);
        control_row(ui, app, &row, &Target::Draft(id), actions);
    });
    egui::CentralPanel::no_frame().show(ui, |ui| {
        ui.centered_and_justified(|ui| ui.label(widgets::secondary("Describe what you want done.", &p)));
    });
}

#[cfg(test)]
mod tests {
    //! The text box on a headless egui context with simulated keys.
    use eframe::egui::{Event, RawInput};

    use super::*;

    fn key(k: Key, modifiers: Modifiers) -> Event {
        Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers }
    }

    /// One frame of the text box (focused) with `events`; returns (sent, text, height).
    fn frame(ctx: &egui::Context, text: &mut String, events: Vec<Event>, composing: bool) -> (bool, f32) {
        let input = RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 400.0))),
            events,
            ..Default::default()
        };
        let id = egui::Id::unique("composer-test");
        let mut sent = false;
        let mut h = 0.0;
        let mut out = ctx.run_ui(input, |ui| {
            ui.memory_mut(|m| m.request_focus(id));
            let mut ime = composing;
            let mut focus = false;
            let top = ui.cursor().top();
            sent = text_box(ui, text, id, "Message", &Palette::DARK, &mut ime, true, &mut focus, SendKey::Enter);
            h = ui.min_rect().bottom() - top;
        });
        out.textures_delta.clear();
        (sent, h)
    }

    #[test]
    fn shift_enter_breaks_the_line_and_enter_sends() {
        let ctx = egui::Context::default();
        let mut text = String::new();
        frame(&ctx, &mut text, vec![], false);
        frame(&ctx, &mut text, vec![Event::Text("one".into())], false);
        let (sent, _) = frame(&ctx, &mut text, vec![key(Key::Enter, Modifiers::SHIFT)], false);
        assert!(!sent, "Shift+Enter must not send");
        frame(&ctx, &mut text, vec![Event::Text("two".into())], false);
        assert_eq!(text, "one\ntwo");
        // Enter while an input method composes belongs to the input method.
        let (sent, _) = frame(&ctx, &mut text, vec![key(Key::Enter, Modifiers::NONE)], true);
        assert!(!sent);
        let (sent, _) = frame(&ctx, &mut text, vec![key(Key::Enter, Modifiers::NONE)], false);
        assert!(sent, "Enter sends");
        assert_eq!(text, "one\ntwo", "and adds no line break");
    }

    #[test]
    fn past_five_lines_the_caret_stays_in_view() {
        let ctx = egui::Context::default();
        let mut text = String::new();
        frame(&ctx, &mut text, vec![], false);
        let id = egui::Id::unique("composer-test");
        let offset = |ctx: &egui::Context| ctx.data(|d| d.get_temp::<f32>(id.with("scroll-offset"))).unwrap_or(0.0);
        let mut last = 0.0;
        for i in 1..=9 {
            frame(&ctx, &mut text, vec![Event::Text(format!("line {i}"))], false);
            frame(&ctx, &mut text, vec![key(Key::Enter, Modifiers::SHIFT)], false);
            let o = offset(&ctx);
            if i >= 5 {
                assert!(o > last, "line {i}: the box scrolled to the caret ({o} after {last})");
            }
            last = o;
        }
        assert_eq!(text.lines().count(), 9);
    }

    #[test]
    fn grows_a_line_at_a_time_to_five_then_stops() {
        let ctx = egui::Context::default();
        let mut text = String::new();
        let mut heights = Vec::new();
        for lines in 1..=8 {
            text = (1..=lines).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
            frame(&ctx, &mut text, vec![], false);
            let (_, h) = frame(&ctx, &mut text, vec![], false);
            heights.push(h);
        }
        // 1 and 2 lines: the same two-line box; then one step per line to five; then flat.
        assert!((heights[0] - heights[1]).abs() < 1.0, "{heights:?}");
        for w in heights[1..5].windows(2) {
            assert!(w[1] > w[0] + 5.0, "{heights:?}");
        }
        for w in heights[4..].windows(2) {
            assert!((w[1] - w[0]).abs() < 1.0, "{heights:?}");
        }
        // Back to empty: two lines again.
        text.clear();
        frame(&ctx, &mut text, vec![], false);
        let (_, h) = frame(&ctx, &mut text, vec![], false);
        assert!((h - heights[0]).abs() < 1.0);
    }
}
