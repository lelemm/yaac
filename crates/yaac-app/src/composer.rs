//! The composer (markdown styled in place), the control row, the fork menu, and the
//! new-thread draft.

use std::sync::Arc;

use eframe::egui::{self, Key, KeyboardShortcut, Modifiers, Margin, text::LayoutJob};
use yaac_core::ThreadKey;
use yaac_core::attention::AttentionMap;
use yaac_core::controls::{Choice, ControlRow, ControlRowView, Speed, ULTRAFAST_WARNING, draft_row, thread_row};
use yaac_core::attachments::{DraftRef, Phase, Source};
use yaac_core::header::{ChipStatus, ChipView, ComposerView, composer, short_provider};
use yaac_core::state::SendAs;
use yaac_core::images::ImageRoute;
use yaac_core::markstyle::{self, Style};
use yaac_core::mode_confirm::ModeTarget;
use yaac_core::compose::{self, EnterAction};
use yaac_core::prefs::SendKey;

use crate::skills_ui;
use crate::app::{Action, Menu, YaacApp};
use crate::files::ThumbKey;
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

/// What the keyboard asked of the text box.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Submit {
    /// Nothing.
    None,
    /// Send; while a turn runs, queue (Enter, or Ctrl+Enter with that send key).
    Send,
    /// Send now (Ctrl+Enter, or Ctrl+Shift+Enter with the Ctrl+Enter send key) — D166.
    SendNow,
}

/// The text box. Returns what Enter (not Shift+Enter, never during an input-method
/// composition) asked.
#[allow(clippy::too_many_arguments, reason = "one call site per composer; a struct would only rename the arguments")]
fn text_box(ui: &mut egui::Ui, text: &mut String, id: egui::Id, placeholder: &str, p: &Palette, composing: &mut bool, enabled: bool, focus: &mut bool, send_key: SendKey) -> Submit {
    track_ime(ui, composing);
    let focused = ui.memory(|m| m.has_focus(id));
    let newline = match send_key {
        SendKey::Enter => KeyboardShortcut::new(Modifiers::SHIFT, Key::Enter),
        SendKey::CtrlEnter => KeyboardShortcut::new(Modifiers::NONE, Key::Enter),
    };
    // Enter is decided here with exact modifiers (egui's shortcut matching ignores an extra
    // Shift, which made Shift+Enter send). A press that sends, or that does nothing, is taken
    // out of the input so the text box does not also break the line.
    let mut send = Submit::None;
    if focused {
        ui.input_mut(|i| {
            let comp = *composing;
            i.events.retain(|e| match e {
                egui::Event::Key { key: Key::Enter, pressed: true, modifiers, .. } => {
                    match compose::enter(send_key, modifiers.shift, modifiers.command || modifiers.ctrl, comp) {
                        EnterAction::Send => {
                            if enabled {
                                send = Submit::Send;
                            }
                            false
                        }
                        EnterAction::SendNow => {
                            if enabled {
                                send = Submit::SendNow;
                            }
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
    ui.data_mut(|d| {
        d.insert_temp(id.with("scroll-offset"), out.state.clamped_offset().y);
        d.insert_temp(id.with("file-picker-anchor"), out.inner.rect);
    });
    let r = out.inner;
    if *focus {
        r.request_focus();
        *focus = false;
    }
    send
}

#[derive(Clone, Default)]
struct FilePicker {
    selected: usize,
    token: Option<(usize, String)>,
    dismissed: bool,
    search_at: f64,
    requested: bool,
    scope: Option<(yaac_protocol::TargetId, yaac_protocol::TargetPath)>,
}

fn caret_token(ui: &egui::Ui, id: egui::Id, text: &str) -> Option<(std::ops::Range<usize>, String)> {
    let range = egui::TextEdit::load_state(ui.ctx(), id)?.cursor.char_range()?;
    if !range.is_empty() {
        return None;
    }
    let caret = text.char_indices().nth(range.primary.index.into()).map_or(text.len(), |(i, _)| i);
    compose::file_token(text, caret).map(|(range, query)| (range, query.to_owned()))
}

#[allow(clippy::too_many_arguments, reason = "the composer editor receives each enabled draft here")]
fn file_text_box(ui: &mut egui::Ui, app: &mut YaacApp, draft: &DraftRef, text: &mut String, id: egui::Id, placeholder: &str, enabled: bool) -> Submit {
    let state_id = id.with("file-picker");
    let mut picker = ui.data_mut(|data| data.get_temp::<FilePicker>(state_id)).unwrap_or_default();
    let mut choose = false;
    let mut scroll_selection = false;
    if enabled && !app.ui.ime_composing && picker.token.is_some() && !picker.dismissed && ui.memory(|m| m.has_focus(id)) {
        ui.input_mut(|input| {
            input.events.retain(|event| {
                let egui::Event::Key { key, pressed: true, modifiers, .. } = event else { return true };
                if !modifiers.is_none() {
                    return true;
                }
                match key {
                    Key::Escape => picker.dismissed = true,
                    Key::ArrowDown => {
                        picker.selected = picker.selected.saturating_add(1);
                        scroll_selection = true;
                    }
                    Key::ArrowUp => {
                        picker.selected = picker.selected.saturating_sub(1);
                        scroll_selection = true;
                    }
                    Key::Enter | Key::Tab => choose = true,
                    _ => return true,
                }
                false
            });
        });
    }
    let p = app.p;
    let submit = text_box(ui, text, id, placeholder, &p, &mut app.ui.ime_composing, enabled, &mut app.ui.focus_composer, app.ui.send_key);
    let active = enabled && !app.ui.ime_composing && (ui.memory(|m| m.has_focus(id))
        || (picker.token.is_some() && ui.input(|i| i.pointer.any_down() || i.pointer.any_released())));
    let token = active.then(|| caret_token(ui, id, text)).flatten();
    let identity = token.as_ref().map(|(range, query)| (range.start, query.clone()));
    let now = ui.input(|input| input.time);
    let scope = identity.as_ref().and_then(|_| app.client.composer_cwd(draft));
    if identity != picker.token || scope != picker.scope {
        picker = FilePicker { token: identity, scope, search_at: now + 0.12, ..Default::default() };
        choose = false;
    }
    if let Some((range, query)) = token.filter(|_| !picker.dismissed) {
        ui.memory_mut(|m| m.set_focus_lock_filter(id, egui::EventFilter {
            tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true,
        }));
        if !picker.requested && picker.scope.is_some() {
            if now >= picker.search_at {
                app.client.search_composer_files(draft, &query);
                picker.requested = true;
            } else {
                ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(picker.search_at - now));
            }
        }
        let search = app.client.composer_files(draft).filter(|search| search.params.query == query);
        let result = search.and_then(|search| search.result.as_ref());
        let entries = result.and_then(|r| r.as_ref().ok()).map_or(&[][..], |r| r.entries.as_slice());
        picker.selected = picker.selected.min(entries.len().saturating_sub(1));
        let mut pick = choose.then(|| entries.get(picker.selected)).flatten();
        if let Some(anchor) = ui.data(|data| data.get_temp::<egui::Rect>(id.with("file-picker-anchor"))) {
            widgets::menu_popup(egui::Popup::new(state_id, ui.ctx().clone(), anchor, ui.layer_id()), &p)
                .align(egui::RectAlign::TOP_START)
                .open(true)
                .show(|ui| {
                    ui.set_width(340.0_f32.min(anchor.width()));
                    if entries.is_empty() {
                        let message = match (search, result) {
                            (None, _) if picker.scope.is_none() => "Choose a working directory to find files.",
                            (_, None) => "Finding files…",
                            (_, Some(Err(error))) => error,
                            (_, Some(Ok(result))) if result.truncated => "No matches in the files searched.",
                            _ => "No matching files.",
                        };
                        ui.label(widgets::secondary(message, &p));
                    }
                    if result.is_some_and(|r| r.as_ref().is_ok_and(|r| r.truncated)) && !entries.is_empty() {
                        ui.label(widgets::secondary("Keep typing to narrow the search.", &p));
                    }
                    egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
                        for (i, file) in entries.iter().enumerate() {
                            let response = ui.selectable_label(i == picker.selected, &file.name).on_hover_text(file.path.as_str());
                            if response.clicked() {
                                pick = Some(file);
                            }
                            if scroll_selection && i == picker.selected {
                                response.scroll_to_me(None);
                            }
                        }
                    });
                });
        }
        if let Some(file) = pick {
            let mut reference = app.client.composer_file_reference(draft, file.path.as_str());
            if text[range.end..].chars().next().is_none_or(|c| !c.is_whitespace()) {
                reference.push(' ');
            }
            let caret = text[..range.start].chars().count() + reference.chars().count();
            text.replace_range(range, &reference);
            if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), id) {
                state.cursor.set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(caret))));
                state.store(ui.ctx(), id);
            }
            ui.memory_mut(|m| m.request_focus(id));
            picker = FilePicker::default();
        }
    }
    ui.data_mut(|data| data.insert_temp(state_id, picker));
    submit
}

/// The focus id of a composer's text box.
pub(crate) fn composer_id(d: &DraftRef) -> egui::Id {
    match d {
        DraftRef::Thread(k) => egui::Id::unique(("composer", k.to_string())),
        DraftRef::New(id) => egui::Id::unique(("draft", *id)),
    }
}

/// The chips above the text: a thumbnail (images) or a file mark, the name, the upload's
/// progress or error, the image routing, and a remove control.
fn chips(ui: &mut egui::Ui, app: &mut YaacApp, d: &DraftRef, chips: &[ChipView], actions: &mut Vec<Action>) {
    let p = app.p;
    // A long text paste waiting for a choice, and anything the clipboard could not give.
    if app.files.large_paste.as_ref().is_some_and(|lp| &lp.draft == d) {
        let len = app.files.large_paste.as_ref().map_or(0, |lp| lp.text.len());
        ui.horizontal_wrapped(|ui| {
            ui.label(widgets::secondary(format!("The pasted text is long ({}).", human_size(len as u64)), &p));
            if widgets::plain(ui, &p, "Attach as a file", None).clicked() {
                actions.push(Action::LargePaste(true));
            }
            if widgets::plain(ui, &p, "Paste as text", None).clicked() {
                actions.push(Action::LargePaste(false));
            }
        });
        ui.add_space(4.0);
    }
    if let Some(n) = app.files.notice.clone() {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(n).font(theme::ui(size::SMALL)).color(p.failed));
            if widgets::icon_button(ui, &p, 12.0, "Dismiss", icons::close).clicked() {
                app.files.notice = None;
            }
        });
    }
    if chips.is_empty() {
        return;
    }
    let sources: Vec<(u64, Option<Source>)> = app.client.draft(d).map(|x| x.attachments.iter().map(|a| (a.id, a.source.clone())).collect()).unwrap_or_default();
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
        for c in chips {
            let key = match sources.iter().find(|(id, _)| *id == c.id).and_then(|(_, s)| s.clone()) {
                Some(Source::Path { path }) => ThumbKey::Path(path),
                _ => ThumbKey::Chip(d.clone(), c.id),
            };
            let thumb = app.files.thumb(&key);
            // A chip is one piece: start a new row when this one would not fit (a frame cannot
            // wrap by itself).
            let status_text = match &c.status {
                ChipStatus::Storing => "saving…".to_owned(),
                ChipStatus::Uploading { fraction: Some(_), .. } => "0000000000000 100%".to_owned(),
                ChipStatus::Uploading { .. } => "uploading…".to_owned(),
                ChipStatus::Failed { message, retry } => format!("{message}{}", if *retry { " Retry" } else { "" }),
                ChipStatus::Ready => match c.route {
                    ImageRoute::AsFileCannotSee => "00 this model cannot see images; it will be sent as a file".to_owned(),
                    ImageRoute::Ask => "send as image file 0000".to_owned(),
                    _ => String::new(),
                },
            };
            let text_w = ui.fonts_mut(|f| {
                let font = theme::ui(size::UI);
                f.layout_no_wrap(c.name.clone(), font.clone(), p.text).size().x + f.layout_no_wrap(status_text, font, p.text).size().x
            });
            let est = text_w + 90.0;
            if ui.cursor().left() > ui.max_rect().left() + 1.0 && ui.cursor().left() + est > ui.max_rect().right() {
                ui.end_row();
            }
            egui::Frame::NONE.fill(p.raised).corner_radius(5).inner_margin(Margin { left: 6, right: 2, top: 3, bottom: 3 }).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    match &thumb {
                        Some(t) => {
                            let sz = t.size_vec2();
                            let h = 24.0;
                            let w = (sz.x / sz.y.max(1.0) * h).clamp(16.0, 40.0);
                            ui.add(egui::Image::new((t.id(), egui::vec2(w, h))).corner_radius(3));
                        }
                        None => {
                            let image = c.image;
                            widgets::icon(ui, 16.0, |pa, ce, dd| if image { icons::image(pa, ce, dd, p.text2) } else { icons::file(pa, ce, dd, p.text2, false) });
                        }
                    }
                    let failed = matches!(c.status, ChipStatus::Failed { .. });
                    ui.label(egui::RichText::new(&c.name).color(if failed { p.text2 } else { p.text }));
                    match &c.status {
                        ChipStatus::Storing => {
                            ui.label(widgets::secondary("saving…", &p));
                        }
                        ChipStatus::Uploading { fraction: Some(f), .. } => {
                            let (r, _) = ui.allocate_exact_size(egui::vec2(64.0, 4.0), egui::Sense::hover());
                            ui.painter().rect_filled(r, 2.0, p.faint);
                            ui.painter().rect_filled(egui::Rect::from_min_size(r.min, egui::vec2(64.0 * f, 4.0)), 2.0, p.text2);
                            ui.label(widgets::secondary(format!("{}%", (f * 100.0).round()), &p));
                        }
                        ChipStatus::Uploading { phase, .. } => {
                            let word = match phase {
                                Phase::Hashing => "reading…",
                                Phase::Queued => "waiting…",
                                Phase::Beginning | Phase::Sending => "uploading…",
                                Phase::Verifying => "checking…",
                            };
                            ui.label(widgets::secondary(word, &p));
                        }
                        ChipStatus::Failed { message, retry } => {
                            ui.label(egui::RichText::new(message).font(theme::ui(size::SMALL)).color(p.failed));
                            if *retry && widgets::link(ui, &p, "Retry", theme::ui(size::SMALL)).clicked() {
                                actions.push(Action::RetryAttachment(d.clone(), c.id));
                            }
                        }
                        ChipStatus::Ready => match c.route {
                            ImageRoute::AsFileCannotSee => {
                                widgets::icon(ui, 13.0, |pa, ce, dd| icons::warning(pa, ce, dd, p.needs, p.raised));
                                ui.label(widgets::secondary("this model cannot see images; it will be sent as a file", &p));
                            }
                            ImageRoute::Ask => {
                                // YAAC does not guess: the user chooses.
                                ui.spacing_mut().item_spacing.x = 4.0;
                                ui.label(widgets::secondary("send as", &p));
                                if widgets::link(ui, &p, "image", theme::ui(size::SMALL)).on_hover_text("The model will see it").clicked() {
                                    actions.push(Action::SendAs(d.clone(), c.id, SendAs::Image));
                                }
                                ui.label(widgets::secondary("·", &p));
                                if widgets::link(ui, &p, "file", theme::ui(size::SMALL)).on_hover_text("The agent gets its path").clicked() {
                                    actions.push(Action::SendAs(d.clone(), c.id, SendAs::File));
                                }
                            }
                            ImageRoute::AsImage | ImageRoute::AsFile => {}
                        },
                    }
                    let tip = if c.status.pending() { "Remove (cancels the upload)" } else { "Remove" };
                    if widgets::icon_button(ui, &p, 12.0, tip, icons::close).clicked() {
                        actions.push(Action::RemoveAttachment(d.clone(), c.id));
                    }
                });
            });
        }
    });
    ui.add_space(6.0);
}

pub(crate) fn human_size(bytes: u64) -> String {
    #[allow(clippy::cast_precision_loss, reason = "a rounded size for people")]
    let b = bytes as f64;
    if b >= 1024.0 * 1024.0 { format!("{:.1} MB", b / 1024.0 / 1024.0) } else { format!("{:.0} KB", (b / 1024.0).ceil()) }
}

/// The attachment button: the system's file picker, several files at once.
fn attach_button(ui: &mut egui::Ui, app: &YaacApp, d: &DraftRef, actions: &mut Vec<Action>) {
    let p = app.p;
    let tip = if app.files.picking { "The file picker is open" } else { "Attach files (or drop them here, or paste)" };
    if widgets::icon_button(ui, &p, 16.0, tip, icons::paperclip).clicked() && !app.files.picking {
        actions.push(Action::AttachPick(d.clone()));
    }
}

/// The send button: filled when the message can go; otherwise plain, saying why.
fn send_button(ui: &mut egui::Ui, p: &Palette, has_content: bool, blocked: Option<&str>) -> bool {
    match blocked {
        Some(why) if has_content => {
            let r = widgets::plain(ui, p, "Send", Some("Enter")).on_hover_text(why);
            let _ = r.clicked();
            false
        }
        _ if has_content => widgets::primary(ui, p, "Send", Some("Enter")).clicked(),
        _ => widgets::plain(ui, p, "Send", Some("Enter")).clicked(),
    }
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
            let r = ui.add(egui::Label::new(widgets::secondary(format!("{title} needs {} {} · {} to go there", article(a.kind.word()), a.kind.word(), app.ui.bindings.label(yaac_core::commands::Cmd::NeedsNext)), &p)).sense(egui::Sense::click()));
            if r.clicked() {
                actions.push(Action::OpenRequest(k));
            }
        });
        ui.add_space(4.0);
    }
    for e in app.client.errors.iter().filter(|e| e.key.as_ref() == Some(key)) {
        ui.label(egui::RichText::new(&e.text).font(theme::ui(size::SMALL)).color(p.failed));
    }
    let compacting = app.client.compacting(key);
    if compacting {
        ui.label(widgets::secondary("Compacting context…", &p));
    }
    let controls = thread_row(&app.client.state, key);
    if matches!(&controls, ControlRowView::Row(row) if row.fast.is_some_and(|speed| speed.ultrafast_on)) {
        ui.label(egui::RichText::new(ULTRAFAST_WARNING).font(theme::ui(size::SMALL)).color(p.needs));
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
        ComposerView::Shown { placeholder, dimmed_hint, stop, chips: chip_views, blocked, running, send_now, editing, .. } => {
            let running = running || compacting;
            let send_now = send_now.filter(|_| !compacting);
            // A request waiting on the user dims the composer only when no turn runs; while one
            // runs, a message can still be queued behind the answer (D166).
            let dimmed = dimmed_hint.is_some() && !running;
            queue_strip(ui, app, key, send_now, actions);
            let d = DraftRef::Thread(key.clone());
            let id = composer_id(&d);
            let mut submit = Submit::None;
            let (queue_key, now_key) = match app.ui.send_key {
                SendKey::Enter => ("Enter", "Ctrl+Enter"),
                SendKey::CtrlEnter => ("Ctrl+Enter", "Ctrl+Shift+Enter"),
            };
            let inner = widgets::faint_frame(&p).inner_margin(Margin::symmetric(14, 10)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                if dimmed {
                    ui.multiply_opacity(0.6);
                }
                if !editing {
                    chips(ui, app, &d, &chip_views, actions);
                }
                // One row whatever runs: the text, and the buttons beside it, so the composer
                // keeps its height when a turn starts or ends (the transcript keeps its place).
                // Send now and Stop name their keys in their tooltips, to leave the text room.
                let label_w = |ui: &egui::Ui, s: &str, bold: bool| {
                    let font = if bold { theme::bold(size::UI) } else { theme::ui(size::UI) };
                    ui.fonts_mut(|f| f.layout_no_wrap(s.to_owned(), font, p.text).size().x) + 24.0
                };
                let btn_w = if editing {
                    label_w(ui, "Save", true) + 24.0 + label_w(ui, "Cancel edit", false) + 12.0
                } else if running {
                    32.0 + label_w(ui, "Queue", true) + 24.0 + send_now.map_or(0.0, |l| label_w(ui, l, false)) + if stop { label_w(ui, "Stop", false) } else { 0.0 } + 16.0
                } else {
                    130.0
                };
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(ui.available_width() - btn_w);
                        submit = text_column(ui, app, key, &d, id, &placeholder, dimmed, editing, actions);
                        if editing {
                            ui.label(widgets::secondary("Editing a queued message", &p));
                        } else if let Some(h) = &dimmed_hint {
                            ui.label(widgets::secondary(h, &p));
                        } else if let Some(why) = blocked.as_ref().filter(|_| !chip_views.is_empty()) {
                            ui.label(widgets::secondary(why, &p));
                        }
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if dimmed {
                            return;
                        }
                        if editing {
                            let can = app.client.state.queue_edits.get(key).is_some_and(|e| e.input().is_some());
                            let save = if can { widgets::primary(ui, &p, "Save", Some(queue_key)) } else { widgets::plain(ui, &p, "Save", Some(queue_key)) };
                            if save.clicked() && can {
                                actions.push(Action::Send(key.clone()));
                            }
                            if widgets::plain(ui, &p, "Cancel edit", None).on_hover_text("Leave it as it was (Esc)").clicked() {
                                actions.push(Action::QueueEditEnd(key.clone()));
                            }
                            return;
                        }
                        let has = app.client.state.drafts.get(key).is_some_and(|x| !x.text.trim().is_empty() || !x.attachments.is_empty());
                        if !running {
                            if send_button(ui, &p, has, blocked.as_deref()) {
                                submit = Submit::Send;
                            }
                            attach_button(ui, app, &d, actions);
                            return;
                        }
                        if stop && widgets::plain(ui, &p, "Stop", None).on_hover_text("Stop the running turn; nothing is sent (Esc)").clicked() {
                            actions.push(Action::Stop(key.clone()));
                        }
                        if let Some(label) = send_now {
                            let what = if label == yaac_core::queue::SEND_NOW { "Add it to the running turn" } else { "Stop the running turn and start a new one with this message" };
                            let r = widgets::plain(ui, &p, label, None).on_hover_text(format!("{what} ({now_key})"));
                            if r.clicked() && has && blocked.is_none() {
                                submit = Submit::SendNow;
                            }
                        }
                        let tip = "Delivered when the running turn ends";
                        let r = match blocked.as_deref() {
                            Some(why) if has => widgets::plain(ui, &p, "Queue", Some(queue_key)).on_hover_text(why),
                            _ if has => widgets::primary(ui, &p, "Queue", Some(queue_key)).on_hover_text(tip),
                            _ => widgets::plain(ui, &p, "Queue", Some(queue_key)).on_hover_text(tip),
                        };
                        if r.clicked() && has && blocked.is_none() {
                            submit = Submit::Send;
                        }
                        // Attachments can be prepared while a turn runs.
                        attach_button(ui, app, &d, actions);
                    });
                });
            });
            if ui.memory(|m| m.has_focus(id)) {
                widgets::focus_ring(ui, inner.response.rect, &p);
            }
            let compact_command = !editing && app.client.draft(&d).is_some_and(|draft| compose::is_compact_command(&draft.text));
            match submit {
                Submit::None => {}
                _ if compact_command => actions.push(Action::Compact(key.clone())),
                // Saving an edit, sending, or (while a turn runs) queueing.
                Submit::Send => actions.push(Action::Send(key.clone())),
                Submit::SendNow if running && !editing && send_now.is_some() => actions.push(Action::SendNow(key.clone())),
                // Not offered (no turn, or a request waits): an ordinary send, or a queue.
                Submit::SendNow => actions.push(Action::Send(key.clone())),
            }
            // Esc leaves an edit; otherwise it stops the running turn from the composer.
            if (editing || stop) && ui.memory(|m| m.has_focus(id)) && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
                actions.push(if editing { Action::QueueEditEnd(key.clone()) } else { Action::Stop(key.clone()) });
            }
        }
    }
    ui.add_space(6.0);
    if let ControlRowView::Row(row) = controls {
        control_row(ui, app, &row, &Target::Thread(key.clone()), actions);
    }
}

/// The text box of a started thread's composer, on its draft — or, while a queued message is
/// being edited, on that edit (D166).
#[allow(clippy::too_many_arguments, reason = "the two layouts of one composer share it")]
fn text_column(ui: &mut egui::Ui, app: &mut YaacApp, key: &ThreadKey, d: &DraftRef, id: egui::Id, placeholder: &str, dimmed: bool, editing: bool, actions: &mut Vec<Action>) -> Submit {
    if editing && let Some(edit) = app.client.state.queue_edits.get_mut(key) {
        let mut text = std::mem::take(&mut edit.text);
        let submit = file_text_box(ui, app, d, &mut text, id, "The queued message…", true);
        if let Some(edit) = app.client.state.queue_edits.get_mut(key) {
            edit.text = text;
        }
        return submit;
    }
    let comp = yaac_core::skills::for_thread(&app.client.state, key);
    let mut text = std::mem::take(&mut app.client.state.drafts.entry(key.clone()).or_default().text);
    let had = text.clone();
    if let Some(c) = &comp && !dimmed {
        skills_ui::keys(ui, &mut app.ui.skills, id, c, &mut text, &mut app.ui.focus_composer);
    }
    let submit = file_text_box(ui, app, d, &mut text, id, placeholder, !dimmed);
    if let Some(c) = &comp && !dimmed {
        let p = app.p;
        skills_ui::show(ui, &mut app.ui.skills, id, c, &app.client.state, &mut text, &mut app.ui.focus_composer, &p, actions);
    }
    if text != had {
        actions.push(Action::DraftEdited(d.clone()));
    }
    app.client.state.drafts.entry(key.clone()).or_default().text = text;
    submit
}

/// The id of a queue strip button (`what`: "now", "edit", "cancel"), for tests.
pub(crate) fn strip_button_id(what: &str, queue_id: &str) -> egui::Id {
    egui::Id::unique(("queue-strip", what, queue_id))
}

/// A small text button of the queue strip.
fn strip_button(ui: &mut egui::Ui, p: &Palette, label: &str, id: egui::Id, enabled: bool) -> egui::Response {
    let ink = if enabled { p.text } else { p.unavailable };
    let g = ui.fonts_mut(|f| f.layout_no_wrap(label.to_owned(), theme::ui(size::SMALL), ink));
    let (_, rect) = ui.allocate_space(egui::vec2(g.size().x + 12.0, 22.0));
    let r = ui.interact(rect, id, if enabled { egui::Sense::click() } else { egui::Sense::hover() });
    if enabled && r.hovered() {
        ui.painter().rect_filled(rect, 4.0, p.faint);
    }
    ui.painter().galley(egui::pos2(rect.left() + 6.0, rect.center().y - g.size().y / 2.0), g, ink);
    if enabled { r.on_hover_cursor(egui::CursorIcon::PointingHand) } else { r }
}

pub(crate) fn running_commands(ui: &mut egui::Ui, app: &mut YaacApp, key: &ThreadKey, actions: &mut Vec<Action>) {
    app.tick_running_commands(ui.ctx(), key);
    let rows = yaac_core::running_commands::rows(&app.client.state, key);
    let log = app.client.command_log.as_ref().filter(|log| &log.view == key);
    if rows.is_empty() && log.is_none() {
        return;
    }
    let p = app.p;
    let expanded = !app.ui.commands_collapsed.contains(key);
    let label = format!("Running commands · {}", rows.len());
    let id = ui.scope_id().with(("running-commands-toggle", key));
    if widgets::disclosure_row(ui, &p, &label, expanded, id) {
        actions.push(Action::ToggleCommands(key.clone()));
    }
    if !expanded {
        return;
    }
    egui::ScrollArea::vertical().id_salt(("running-commands", key)).max_height(112.0).show(ui, |ui| {
        for row in &rows {
            ui.push_id(&row.command.id, |ui| {
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if row.can_stop {
                            let waiting = app.client.stopping_command(&row.key());
                            if ui.add_enabled(!waiting, egui::Button::new(if waiting { "Stopping…" } else { "Stop" })).clicked() {
                                actions.push(Action::CommandStop(key.clone(), row.command.id.clone()));
                            }
                        }
                        if widgets::plain(ui, &p, "Logs", None).clicked() {
                            actions.push(Action::CommandLogs(key.clone(), row.command.id.clone()));
                        }
                        ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                            let command = row.command.command.lines().next().unwrap_or("");
                            ui.add(egui::Label::new(egui::RichText::new(command).monospace().size(11.0).color(p.text2)).truncate()).on_hover_text(&row.command.command);
                            ui.add(egui::Label::new(widgets::secondary(row.detail(app.now()), &p)).truncate());
                        });
                    });
                });
            });
        }
    });
    if let Some(log) = log {
        ui.separator();
        ui.horizontal(|ui| {
            if widgets::plain(ui, &p, "Close logs", None).clicked() {
                actions.push(Action::CommandLogs(key.clone(), log.command.id.clone()));
            }
            ui.add(egui::Label::new(widgets::secondary(log.title.lines().next().unwrap_or(""), &p)).truncate());
        });
        let listed = rows.iter().any(|row| row.key() == log.command);
        if let Some(status) = log.status(listed) {
            ui.label(widgets::secondary(status, &p));
        }
        if let Some(mut output) = log.result.as_ref().and_then(|r| r.output.as_deref()).filter(|o| !o.is_empty()) {
            egui::ScrollArea::vertical().id_salt(("command-logs", &log.command.id)).max_height(145.0).stick_to_bottom(true).show(ui, |ui| {
                ui.add(egui::TextEdit::multiline(&mut output).font(egui::FontId::monospace(11.0)).text_color(p.text2).desired_width(f32::INFINITY));
            });
        }
    }
    ui.add_space(4.0);
}

/// The queued messages above the composer (D166), in delivery order: a one-line preview,
/// **Send now**, **Edit** and ✕ each. Gone when nothing waits.
fn queue_strip(ui: &mut egui::Ui, app: &YaacApp, key: &ThreadKey, send_now: Option<&'static str>, actions: &mut Vec<Action>) {
    let rows = yaac_core::queue::strip(&app.client.state, key);
    if rows.is_empty() {
        return;
    }
    let p = app.p;
    let may = app.client.state.may_act(key);
    let when = if yaac_core::queue::running(&app.client.state, key) { "sent when the turn ends" } else { "sent in order" };
    egui::Frame::NONE.inner_margin(Margin { left: 14, right: 6, top: 0, bottom: 4 }).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(widgets::secondary(format!("Queued · {} · {when}", rows.len()), &p));
        for r in &rows {
            ui.horizontal(|ui| {
                ui.set_height(24.0);
                ui.spacing_mut().item_spacing.x = 4.0;
                if r.busy {
                    ui.multiply_opacity(0.5);
                }
                let free = !r.busy && may;
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (_, rect) = ui.allocate_space(egui::vec2(22.0, 22.0));
                    let cancel = ui.interact(rect, strip_button_id("cancel", &r.queue_id), if free { egui::Sense::click() } else { egui::Sense::hover() });
                    if cancel.hovered() && free {
                        ui.painter().rect_filled(rect, 4.0, p.faint);
                    }
                    icons::close(ui.painter(), rect.center(), 12.0, if cancel.hovered() { p.text } else { p.text2 });
                    if cancel.on_hover_text("Cancel: it will not be sent").clicked() && free {
                        actions.push(Action::QueueCancel(key.clone(), r.queue_id.clone()));
                    }
                    if r.editing {
                        ui.label(widgets::secondary("editing below", &p));
                    } else if strip_button(ui, &p, "Edit", strip_button_id("edit", &r.queue_id), free).clicked() && free {
                        actions.push(Action::QueueEdit(key.clone(), r.queue_id.clone()));
                    }
                    if let Some(label) = send_now
                        && strip_button(ui, &p, label, strip_button_id("now", &r.queue_id), free).clicked()
                        && free
                    {
                        actions.push(Action::QueuePromote(key.clone(), r.queue_id.clone()));
                    }
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        // The text gives way to the attachment count beside it.
                        let extra_w = r.extra.as_ref().map_or(0.0, |x| ui.fonts_mut(|f| f.layout_no_wrap(x.clone(), theme::ui(size::SMALL), p.text2).size().x) + 8.0);
                        ui.scope(|ui| {
                            ui.set_max_width((ui.available_width() - extra_w).max(40.0));
                            ui.add(egui::Label::new(egui::RichText::new(&r.preview).font(theme::ui(size::UI)).color(p.text)).truncate());
                        });
                        if let Some(x) = &r.extra {
                            ui.label(widgets::secondary(x, &p));
                        }
                    });
                });
            });
        }
    });
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
    crate::widgets::menu_popup(egui::Popup::from_response(&r), &p)
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
            crate::widgets::menu_popup(egui::Popup::from_response(&r), &p)
                .kind(egui::PopupKind::Menu)
                .align(egui::RectAlign::TOP_START)
                .open_bool(&mut open)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
                .show(|ui| {
                    ui.set_min_width(300.0);
                    ui.set_max_width(360.0);
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
                            // One full-width menu row: the name (and "default"), the image mark,
                            // and the effort range right-aligned.
                            let (r, rect) = widgets::menu_row(ui, &p, model.selected, model.enabled);
                            let ink = if model.enabled { p.text } else { p.unavailable };
                            let font = theme::ui(size::UI);
                            let mut x = rect.left() + 10.0;
                            let g1 = ui.fonts_mut(|f| f.layout_no_wrap(model.name.clone(), font.clone(), ink));
                            ui.painter().galley(egui::pos2(x, rect.center().y - g1.size().y / 2.0), g1.clone(), ink);
                            x += g1.size().x + 8.0;
                            if model.is_default {
                                let gd = ui.fonts_mut(|f| f.layout_no_wrap("default".to_owned(), theme::ui(size::SMALL), p.text2));
                                ui.painter().galley(egui::pos2(x, rect.center().y - gd.size().y / 2.0), gd.clone(), p.text2);
                                x += gd.size().x + 8.0;
                            }
                            if model.image.is_supported() {
                                icons::image(ui.painter(), egui::pos2(x + 7.0, rect.center().y), 14.0, p.text2);
                            }
                            if let Some(e) = &model.effort_range {
                                let ge = ui.fonts_mut(|f| f.layout_no_wrap(e.clone(), theme::ui(size::SMALL), p.text2));
                                ui.painter().galley(egui::pos2(rect.right() - 10.0 - ge.size().x, rect.center().y - ge.size().y / 2.0), ge, p.text2);
                            }
                            if r.clicked() {
                                actions.push(match target {
                                    Target::Thread(k) => Action::SetModel(k.clone(), model.id.clone()),
                                    Target::Draft(id) => Action::DraftModel(*id, g.instance.clone(), model.id.clone()),
                                });
                            }
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
        if let Some(speed) = row.fast {
            let draft = match target {
                Target::Thread(k) => DraftRef::Thread(k.clone()),
                Target::Draft(id) => DraftRef::New(*id),
            };
            let trigger = widgets::speed_control(ui, &p, speed);
            if speed.offers_ultrafast {
                if trigger.clicked() {
                    app.ui.toggle_menu(Menu::Speed);
                }
                let mut open = app.ui.menu == Some(Menu::Speed);
                widgets::menu_popup(egui::Popup::from_response(&trigger), &p)
                    .kind(egui::PopupKind::Menu).open_bool(&mut open).close_behavior(egui::PopupCloseBehavior::CloseOnClick)
                    .show(|ui| {
                        for option in speed.options() {
                            if ui.selectable_label(speed.current() == option, option.label()).clicked() {
                                actions.push(Action::ChooseSpeed(draft.clone(), option));
                            }
                        }
                    });
                if !open && app.ui.menu == Some(Menu::Speed) {
                    app.ui.menu = None;
                }
            } else if trigger.clicked() {
                actions.push(Action::ChooseSpeed(draft, if speed.on { Speed::Standard } else { Speed::Fast }));
            }
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
        if let Some(n) = &row.note {
            ui.add_space(8.0);
            ui.label(widgets::secondary(n, &p));
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
    egui::Panel::top("draft-header").show_separator_line(false).frame(egui::Frame::NONE.inner_margin(Margin { left: 28, right: 127, top: 9, bottom: 6 })).show(ui, |ui| {
        ui.label(egui::RichText::new("New thread").font(theme::bold(size::TITLE)));
        let project = d.project.as_ref().and_then(|pid| app.client.state.projects.iter().find(|x| &x.id == pid)).map(|x| x.name.clone());
        let crumb = match project {
            Some(name) => format!("{target_name} · {name}"),
            None => format!("{target_name} · {}", d.cwd.as_ref().map_or_else(|| "no project yet".to_owned(), ToString::to_string)),
        };
        ui.label(widgets::secondary(crumb, &p));
    });
    let preferred = app.ui.chat_width;
    let contents = |ui: &mut egui::Ui| {
        if d.ultrafast && app.composer_speed(&DraftRef::New(id)).is_some_and(|speed| speed.ultrafast_on) {
            ui.label(egui::RichText::new(ULTRAFAST_WARNING).font(theme::ui(size::SMALL)).color(p.needs));
        }
        // One selector: the project (a new thread starts in the root of its folder).
        let project_label = d
            .project
            .as_ref()
            .and_then(|pid| app.client.state.projects.iter().find(|x| &x.id == pid))
            .map_or_else(|| format!("Project: none · {target_name}"), |x| format!("Project: {} · {}", x.name, app.client.state.target_name(&x.target)));
        let r = widgets::selector(ui, &p, &project_label, true);
        if r.clicked() {
            app.ui.toggle_menu(Menu::Project);
        }
        let mut open = app.ui.menu == Some(Menu::Project);
        let groups = yaac_core::first_run::project_picker(&app.client.state, d.project.as_deref());
        let mut new_project = false;
        crate::widgets::menu_popup(egui::Popup::from_response(&r), &p).kind(egui::PopupKind::Menu).align(egui::RectAlign::TOP_START).open_bool(&mut open).close_behavior(egui::PopupCloseBehavior::CloseOnClick).show(|ui| {
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
        let mut send = Submit::None;
        let dref = DraftRef::New(id);
        let eid = composer_id(&dref);
        let support = yaac_core::attachments::draft_image_support(&app.client.state, &d);
        let chip_views = yaac_core::attachments::chips(&d.draft.attachments, &support, Some(&d.target));
        let blocked = yaac_core::attachments::blocked(&chip_views);
        let has = !d.draft.text.trim().is_empty() || !d.draft.attachments.is_empty();
        let comp = yaac_core::skills::for_new_thread(&app.client.state, &d);
        let inner = widgets::faint_frame(&p).inner_margin(Margin::symmetric(14, 10)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            chips(ui, app, &dref, &chip_views, actions);
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(ui.available_width() - 130.0);
                    let mut text = d.draft.text.clone();
                    if let Some(c) = &comp {
                        skills_ui::keys(ui, &mut app.ui.skills, eid, c, &mut text, &mut app.ui.focus_composer);
                    }
                    send = file_text_box(ui, app, &dref, &mut text, eid, &format!("Message {name}…"), true);
                    if let Some(c) = &comp {
                        skills_ui::show(ui, &mut app.ui.skills, eid, c, &app.client.state, &mut text, &mut app.ui.focus_composer, &p, actions);
                    }
                    if compose::is_compact_command(&text) {
                        ui.label(widgets::secondary("Start the conversation before compacting context.", &p));
                    }
                    if text != d.draft.text && let Some(dd) = app.client.state.new_threads.iter_mut().find(|x| x.id == id) {
                        dd.draft.text = text;
                        actions.push(Action::DraftEdited(dref.clone()));
                    }
                    if let Some(why) = blocked.as_ref().filter(|_| !chip_views.is_empty()) {
                        ui.label(widgets::secondary(why, &p));
                    }
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if send_button(ui, &p, has, blocked.as_deref()) {
                        send = Submit::Send;
                    }
                    attach_button(ui, app, &dref, actions);
                });
            });
        });
        if ui.memory(|m| m.has_focus(eid)) {
            widgets::focus_ring(ui, inner.response.rect, &p);
        }
        // A new thread runs nothing yet: Send now is an ordinary send.
        if send != Submit::None {
            actions.push(Action::StartDraft(id));
        }
        ui.add_space(6.0);
        let row = draft_row(&providers, &target_name, &d);
        control_row(ui, app, &row, &Target::Draft(id), actions);
    };
    let panel = egui::Panel::bottom("draft-composer")
        .show_separator_line(false)
        .frame(egui::Frame::NONE.inner_margin(Margin { left: 0, right: 0, top: 6, bottom: 10 }));
    if let Some(width) = panel.show(ui, |ui| crate::thread::composer_column(ui, preferred, &p, contents)).inner {
        app.ui.chat_width = width;
    }
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
    fn frame(ctx: &egui::Context, text: &mut String, events: Vec<Event>, composing: bool) -> (Submit, f32) {
        let input = RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 400.0))),
            events,
            ..Default::default()
        };
        let id = egui::Id::unique("composer-test");
        let mut sent = Submit::None;
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
        assert_eq!(sent, Submit::None, "Shift+Enter must not send");
        frame(&ctx, &mut text, vec![Event::Text("two".into())], false);
        assert_eq!(text, "one\ntwo");
        // Enter while an input method composes belongs to the input method.
        let (sent, _) = frame(&ctx, &mut text, vec![key(Key::Enter, Modifiers::NONE)], true);
        assert_eq!(sent, Submit::None);
        let (sent, _) = frame(&ctx, &mut text, vec![key(Key::Enter, Modifiers::NONE)], false);
        assert_eq!(sent, Submit::Send, "Enter sends");
        assert_eq!(text, "one\ntwo", "and adds no line break");
        let (sent, _) = frame(&ctx, &mut text, vec![key(Key::Enter, Modifiers::COMMAND)], false);
        assert_eq!(sent, Submit::SendNow, "Ctrl+Enter sends now");
        assert_eq!(text, "one\ntwo");
    }

    #[test]
    fn file_picker_tab_inserts_a_target_path_at_the_caret() {
        let ctx = egui::Context::default();
        let mut app = crate::app::tests::demo_app(&ctx);
        let thread = yaac_core::demo::threads::users();
        let draft = DraftRef::Thread(thread.clone());
        let id = composer_id(&draft);
        let cwd = app.client.state.summary(&thread).unwrap().cwd.to_string();
        let mut text = "Read  later".to_owned();
        let mut at = 0.0;
        let mut draw = |events, initial: bool| {
            at += 0.15;
            let input = RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 400.0))),
                time: Some(at),
                events,
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| {
                if initial {
                    ui.memory_mut(|m| m.request_focus(id));
                    let mut state = egui::text_edit::TextEditState::default();
                    state.cursor.set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(5))));
                    state.store(ui.ctx(), id);
                }
                assert_eq!(file_text_box(ui, &mut app, &draft, &mut text, id, "Message", true), Submit::None);
            });
            out.textures_delta.clear();
            app.client.pump();
        };
        draw(vec![], true);
        draw(vec![Event::Text("@notes".into())], false);
        draw(vec![], false);
        draw(vec![], false);
        draw(vec![key(Key::Tab, Modifiers::NONE)], false);
        assert_eq!(text, format!("Read `{cwd}/docs/design notes.md` later"));
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

    /// The skill picker over the text box: `/` lists, arrows move, Enter picks (and does not
    /// send), Esc closes, and a path never opens it.
    #[test]
    fn skill_picker_opens_on_a_leading_slash_and_a_pick_replaces_the_trigger() {
        use yaac_core::skills::{Composer, SkillCaps, SkillScopeKey, SkillsState};
        use yaac_protocol::{InstanceId, SkillInfo, TargetId};
        let scope = SkillScopeKey { target: TargetId::new("t"), instance: InstanceId::new("i"), cwd: "/w".into() };
        let comp = Composer { scope: scope.clone(), caps: SkillCaps { skills: true, inline: true }, provider: "Codex".into(), builtins: vec![] };
        let skill = |n: &str| SkillInfo { name: n.into(), description: "d".into(), path: format!("/s/{n}/SKILL.md"), scope: None, argument_hint: None };
        let mut state = yaac_core::state::AppState::default();
        state.skills.insert(scope, SkillsState::Ready(vec![skill("plan"), skill("planet"), skill("lint")]));
        let ctx = egui::Context::default();
        let id = egui::Id::unique("skills-test");
        let mut picker = skills_ui::Picker::default();
        let mut text = String::new();
        let run = |text: &mut String, picker: &mut skills_ui::Picker, events: Vec<Event>| -> (Submit, Vec<Action>) {
            let input = RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 400.0))), events, ..Default::default() };
            let mut sent = Submit::None;
            let mut actions = Vec::new();
            let mut out = ctx.run_ui(input, |ui| {
                ui.memory_mut(|m| m.request_focus(id));
                let (mut ime, mut focus) = (false, false);
                skills_ui::keys(ui, picker, id, &comp, text, &mut focus);
                sent = text_box(ui, text, id, "Message", &Palette::DARK, &mut ime, true, &mut focus, SendKey::Enter);
                skills_ui::show(ui, picker, id, &comp, &state, text, &mut focus, &Palette::DARK, &mut actions);
            });
            out.textures_delta.clear();
            (sent, actions)
        };
        run(&mut text, &mut picker, vec![]);
        run(&mut text, &mut picker, vec![Event::Text("/pl".into())]);
        assert_eq!(text, "/pl");
        run(&mut text, &mut picker, vec![]);
        // Down moves to the second row; Enter picks it and does not send.
        run(&mut text, &mut picker, vec![key(Key::ArrowDown, Modifiers::NONE)]);
        let (sent, _) = run(&mut text, &mut picker, vec![key(Key::Enter, Modifiers::NONE)]);
        assert_eq!(sent, Submit::None, "Enter picked a skill; it did not send");
        assert_eq!(text, "$planet ", "Codex names the skill inline");
        // The picker is closed now: Enter sends.
        run(&mut text, &mut picker, vec![]);
        let (sent, _) = run(&mut text, &mut picker, vec![key(Key::Enter, Modifiers::NONE)]);
        assert_eq!(sent, Submit::Send);

        // Esc closes the picker and keeps the text; a path never opens one.
        let mut text = String::new();
        run(&mut text, &mut picker, vec![Event::Text("/li".into())]);
        run(&mut text, &mut picker, vec![]);
        run(&mut text, &mut picker, vec![key(Key::Escape, Modifiers::NONE)]);
        assert_eq!(text, "/li");
        let (sent, _) = run(&mut text, &mut picker, vec![key(Key::Enter, Modifiers::NONE)]);
        assert_eq!(sent, Submit::Send, "a dismissed picker no longer takes Enter");
        let mut text = String::new();
        run(&mut text, &mut picker, vec![Event::Text("/home/me".into())]);
        run(&mut text, &mut picker, vec![]);
        let (sent, _) = run(&mut text, &mut picker, vec![key(Key::Enter, Modifiers::NONE)]);
        assert_eq!((sent, text.as_str()), (Submit::Send, "/home/me"));
    }
}
