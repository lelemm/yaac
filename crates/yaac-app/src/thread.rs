//! The thread view: header, banners, and the virtualised transcript.

use std::hash::{Hash, Hasher};

use eframe::egui::{self, Margin, Panel, Rect, Sense, UiBuilder, pos2, vec2};
use yaac_core::agentcall::{CallLine, Pill};
use yaac_core::activity::{ActivityLine, Block, LineMark, blocks, thought_text};
use yaac_core::diff::{FileDiff, LineKind};
use yaac_core::selection::Unit;
use yaac_core::attention::{AttentionMap, Mark};
use yaac_core::header::{Banner, LinkLine, Tone, header};
use yaac_core::transcript::Transcript;
use yaac_core::ThreadKey;
use yaac_protocol::{InputPart, ItemBody, ReceiptReason, ReceiptStatus};

use crate::app::{Action, YaacApp};
use crate::icons;
use crate::markdown;
use crate::seltext::SelCx;
use crate::theme::{self, Palette, size};
use crate::widgets;

fn chat_width(viewport: f32, preferred: f32) -> f32 {
    let max = viewport.max(0.0) * 0.95;
    preferred.clamp(360.0_f32.min(max), max)
}

pub(crate) fn composer_column(ui: &mut egui::Ui, preferred: f32, p: &Palette, contents: impl FnOnce(&mut egui::Ui)) -> Option<f32> {
    let viewport = ui.available_width();
    let width = chat_width(viewport, preferred);
    let spacing = (viewport - width) * 0.5;
    let row = ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.add_space(spacing);
        ui.allocate_ui_with_layout(vec2(width, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
            ui.set_width(width);
            contents(ui);
        }).response.rect
    });
    let edge = pos2(row.response.rect.left() + spacing + width, row.inner.center().y);
    let grip = ui.interact(Rect::from_center_size(edge, vec2(10.0, 32.0)), ui.scope_id().with("chat-width-grip"), Sense::drag())
        .on_hover_cursor(egui::CursorIcon::ResizeHorizontal).on_hover_text("Resize chat and composer");
    ui.painter().vline(edge.x, (edge.y - 10.0)..=(edge.y + 10.0), egui::Stroke::new(2.0, if grip.hovered() || grip.dragged() { p.text2 } else { p.border }));
    let resized = chat_width(viewport, width + grip.drag_delta().x * 2.0);
    if resized != width {
        ui.ctx().request_repaint();
        Some(resized)
    } else {
        None
    }
}

pub(crate) fn show(ui: &mut egui::Ui, app: &mut YaacApp, map: &AttentionMap, key: &ThreadKey, now: i64, actions: &mut Vec<Action>) {
    let p = app.p;
    Panel::top("thread-header")
        .show_separator_line(false)
        // The first line sits in the window's title row (D104); room is left for its buttons.
        .frame(egui::Frame::NONE.fill(p.bg).inner_margin(Margin { left: 28, right: 127, top: 7, bottom: 6 }))
        .show(ui, |ui| header_view(ui, app, map, key, actions));
    if app.browsers.areas.contains_key(key) {
        if let Some(chat) = crate::preview_ui::layout(ui, app, map, key) {
            // The chat column: the transcript and composer, then a docked Related agents
            // panel at its right; the floating pill stays inside it.
            app.ui.chat_column = Some(chat);
            let mut text = chat;
            // The transcript keeps its minimum width; without room for the panel beside it,
            // the pill floats in the chat column instead.
            if let Some(w) = app.ui.chat_dock_width.filter(|_| chat.width() - crate::preview_ui::CHAT_MIN >= yaac_core::prefs::AGENTS_MIN_WIDTH) {
                let w = w.min(chat.width() - crate::preview_ui::CHAT_MIN);
                text.max.x = chat.right() - w;
                app.ui.chat_dock = Some(egui::Rect::from_min_max(egui::pos2(text.right(), chat.top()), chat.max));
            } else if app.ui.chat_agents {
                // The floating pill keeps its own gutter at the column's right edge.
                text.max.x = (chat.right() - crate::agents_ui::PILL_GUTTER).max(chat.left());
            }
            ui.scope_builder(egui::UiBuilder::new().max_rect(text), |ui| {
                ui.set_clip_rect(text);
                body(ui, app, map, key, now, actions);
            });
        }
    } else {
        body(ui, app, map, key, now, actions);
    }
}

fn body(ui: &mut egui::Ui, app: &mut YaacApp, map: &AttentionMap, key: &ThreadKey, now: i64, actions: &mut Vec<Action>) {
    let p = app.p;
    Panel::bottom("thread-composer")
        .show_separator_line(false)
        .frame(egui::Frame::NONE.fill(p.bg).inner_margin(Margin { left: 0, right: 0, top: 6, bottom: 10 }))
        .show(ui, |ui| {
            if let Some(width) = composer_column(ui, app.ui.chat_width, &p, |ui| {
                crate::composer::running_commands(ui, app, key, actions);
                for error in app.client.errors.iter().filter(|e| e.key.as_ref() == Some(key)) {
                    crate::shell::error_notice(ui, app, error, actions);
                }
                match app.client.state.stopped_distro(&key.target) {
                    // A stopped WSL distro: the composer and control row give way to one line (D96).
                    Some(_) => {
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            widgets::icon(ui, 16.0, |pa, c, d| icons::warning(pa, c, d, p.needs, p.bg));
                            if let Some(notice) = yaac_core::header::composer_connection_notice(&app.client.state, &key.target) {
                                ui.label(notice);
                            }
                            if widgets::link(ui, &p, "Start it", theme::ui(size::UI)).clicked() {
                                actions.push(Action::Retry(key.target.clone()));
                            }
                        });
                        ui.add_space(8.0);
                    }
                    None => crate::composer::bottom(ui, app, map, key, actions),
                }
            }) {
                app.ui.chat_width = width;
            }
        });
    egui::CentralPanel::no_frame().show(ui, |ui| {
        if let Some(preview) = app.ui.dock_preview.target(key).cloned() {
            if let Some(summary) = app.client.state.summary(&preview) {
                ui.vertical_centered(|ui| ui.label(widgets::secondary(format!("Previewing {}", summary.title), &p)));
            }
            ui.push_id("docked-agent-preview", |ui| transcript(ui, app, map, &preview, now, actions));
        } else {
            transcript(ui, app, map, key, now, actions);
        }
    });
}

fn tone_color(p: &Palette, t: Tone) -> egui::Color32 {
    match t {
        Tone::NeedsYou => p.needs,
        Tone::Running => p.running,
        Tone::Failed => p.failed,
        Tone::Neutral => p.text2,
    }
}

fn header_view(ui: &mut egui::Ui, app: &mut YaacApp, map: &AttentionMap, key: &ThreadKey, actions: &mut Vec<Action>) {
    let p = app.p;
    let Some(h) = header(&app.client.state, map, key) else { return };
    ui.horizontal(|ui| {
        widgets::icon(ui, 22.0, |pa, c, d| icons::thread_mark(pa, c, d * 0.95, &h.display, &p));
        if h.display.read_only {
            widgets::icon(ui, 18.0, |pa, c, d| icons::lock(pa, c, d, p.text2));
        }
        ui.add_space(2.0);
        // The tag, and the update item when the title row shows one (D136).
        let tag_w = if h.tag.is_some() { 230.0 } else { 0.0 } + if h.budget.is_some() { 190.0 } else { 0.0 } + 44.0 + app.updater.title_width();
        ui.scope(|ui| {
            ui.set_max_width((ui.available_width() - tag_w).max(80.0));
            // Not selectable: the title is part of the title row, which drags the window.
            ui.add(egui::Label::new(egui::RichText::new(&h.title).font(theme::bold(size::TITLE))).truncate().selectable(false));
        });
        if let Some((tag, tone)) = &h.tag {
            ui.add_space(4.0);
            let c = tone_color(&p, *tone);
            egui::Frame::NONE.fill(p.wash(c)).corner_radius(4).inner_margin(Margin::symmetric(7, 2)).show(ui, |ui| {
                ui.add(egui::Label::new(egui::RichText::new(tag).font(theme::ui(size::SMALL)).color(c)).selectable(false));
            });
        }
        // The budget never hides what the thread does (D164): a quiet note beside its state.
        if let Some(root) = &h.budget {
            ui.add_space(6.0);
            ui.add(egui::Label::new(widgets::secondary("Agent budget reached", &p)).selectable(false))
                .on_hover_text("Agents working for this chat have used their budget: what they start is held back until you choose Continue. Your own turns are not affected.");
            if widgets::link(ui, &p, "Continue", theme::ui(size::SMALL)).clicked() {
                actions.push(Action::ContinueBudget(root.clone()));
            }
        }
    });
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.add_space(32.0);
        ui.label(widgets::secondary(h.breadcrumb.join(" · "), &p));
    });
    match &h.links {
        LinkLine::None => {}
        LinkLine::SpawnedBy { parent, target } => {
            ui.horizontal(|ui| {
                ui.add_space(32.0);
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.label(widgets::secondary("Spawned by", &p));
                link_part(ui, &p, &parent.title, parent.key.as_ref(), actions);
                widgets::icon(ui, 12.0, |pa, c, d| icons::arrow_out(pa, c, d, p.text2));
                ui.label(widgets::secondary(format!("on {target}"), &p));
            });
        }
        LinkLine::Ancestry(parts) => {
            ui.horizontal(|ui| {
                ui.add_space(32.0);
                ui.spacing_mut().item_spacing.x = 4.0;
                for (i, part) in parts.iter().enumerate() {
                    if i > 0 {
                        widgets::icon(ui, 10.0, |pa, c, d| icons::chevron(pa, c, d, false, p.text2));
                    }
                    match &part.key {
                        Some(_) => link_part(ui, &p, &part.title, part.key.as_ref(), actions),
                        None => {
                            ui.label(egui::RichText::new(&part.title).font(theme::bold(size::SMALL)));
                        }
                    }
                }
            });
        }
        LinkLine::ForkedFrom { source, detail } => {
            ui.horizontal(|ui| {
                ui.add_space(32.0);
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.label(widgets::secondary("Forked from", &p));
                link_part(ui, &p, &source.title, source.key.as_ref(), actions);
                widgets::icon(ui, 12.0, |pa, c, d| icons::arrow_out(pa, c, d, p.text2));
                ui.label(widgets::secondary(detail.clone(), &p));
            });
        }
    }
    for b in &h.banners {
        ui.add_space(8.0);
        egui::Frame::NONE.fill(p.faint).corner_radius(6).inner_margin(Margin::symmetric(12, 8)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| match b {
                Banner::InUseElsewhere { text, take_over } => {
                    widgets::icon(ui, 16.0, |pa, c, d| icons::info(pa, c, d, p.text2));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Take over is the primary action when offered (D140); Fork stays.
                        match take_over {
                            Some(t) => {
                                if t.busy {
                                    // In flight: progress, and nothing to click twice.
                                    let _ = widgets::primary(ui, &p, "Taking over…", None).on_hover_text(&t.tooltip);
                                    ui.add(egui::Spinner::new().size(14.0).color(p.text2));
                                } else if t.enabled {
                                    if widgets::primary_with_id(ui, &p, "Take over", egui::Id::unique(TAKE_OVER_BUTTON)).on_hover_text(&t.tooltip).clicked() {
                                        actions.push(Action::TakeOver(key.clone()));
                                    }
                                } else {
                                    // The reason is the line under the sentence.
                                    widgets::unavailable(ui, &p, "Take over", "");
                                }
                                if widgets::plain(ui, &p, "Fork to continue here", None).clicked() {
                                    actions.push(Action::OpenFork(key.clone()));
                                }
                            }
                            None => {
                                if widgets::primary(ui, &p, "Fork to continue here", None).clicked() {
                                    actions.push(Action::OpenFork(key.clone()));
                                }
                            }
                        }
                        // The sentence takes what the buttons leave, wrapped.
                        ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                            ui.add(egui::Label::new(text.as_str()).wrap());
                            // Why Take over cannot be used, or why the last attempt failed.
                            if let Some(t) = take_over {
                                if let Some(why) = &t.failure {
                                    ui.add(egui::Label::new(egui::RichText::new(format!("Not taken over: {why}")).font(theme::ui(size::SMALL)).color(p.failed)).wrap());
                                } else if !t.enabled && !t.busy {
                                    ui.add(egui::Label::new(widgets::secondary(t.tooltip.clone(), &p)).wrap());
                                }
                            }
                        });
                    });
                }
                Banner::Subagent { text, reason, parent } => {
                    widgets::icon(ui, 16.0, |pa, c, d| icons::info(pa, c, d, p.text2));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // The one to message instead (D151).
                        if let Some(k) = parent.as_ref().and_then(|p| p.key.clone())
                            && widgets::plain(ui, &p, "Open parent", None).clicked()
                        {
                            actions.push(Action::Open(k));
                        }
                        ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                            ui.add(egui::Label::new(text.as_str()).wrap());
                            if let Some(why) = reason {
                                ui.add(egui::Label::new(widgets::secondary(why.clone(), &p)).wrap());
                            }
                        });
                    });
                }
                Banner::OlderDaemon { text, update_now } => {
                    widgets::icon(ui, 16.0, |pa, c, d| icons::warning(pa, c, d, p.needs, p.faint));
                    ui.label(text);
                    if let Some(target) = update_now {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if widgets::plain(ui, &p, "Update now", None).clicked() {
                                actions.push(Action::UpdateDaemon(target.clone()));
                            }
                        });
                    }
                }
                Banner::StoreAway { text } => {
                    widgets::icon(ui, 16.0, |pa, c, d| icons::warning(pa, c, d, p.needs, p.faint));
                    ui.add(egui::Label::new(text.as_str()).wrap());
                }
            });
        });
    }
}

/// The banner's **Take over** button (D140), by a stable id.
pub(crate) const TAKE_OVER_BUTTON: &str = "thread-take-over";

pub(crate) fn link_part(ui: &mut egui::Ui, p: &Palette, title: &str, key: Option<&ThreadKey>, actions: &mut Vec<Action>) {
    match key {
        Some(k) => {
            if widgets::link(ui, p, title, theme::ui(size::SMALL)).clicked() {
                actions.push(Action::Open(k.clone()));
            }
        }
        None => {
            ui.label(egui::RichText::new(title).font(theme::ui(size::SMALL)).color(p.text));
        }
    }
}

/// Which block this is, whatever it says now: its kind and the item it starts at. A block
/// keeps this while its item streams, so its last measured height places it until it is
/// measured again (D152).
fn block_ident(b: &Block, t: &Transcript) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    std::mem::discriminant(b).hash(&mut h);
    // A sent message is known by its op id, so its own item takes over the echo's measured
    // height when it replaces it (D165).
    let id = |i: usize, h: &mut std::collections::hash_map::DefaultHasher| match t.items().get(i).map(|x| (&x.id, &x.body)) {
        Some((_, ItemBody::UserMessage(yaac_protocol::UserMessageItem { op_id: Some(op), .. }))) => ("op", op.as_str()).hash(h),
        other => other.map(|(id, _)| id).hash(h),
    };
    match b {
        Block::User { index } | Block::Task { index } | Block::Prose { index } | Block::Card { index } | Block::Handoff { index } => id(*index, &mut h),
        Block::Run { lines } => {
            if let Some(l) = lines.first() {
                l.id.hash(&mut h);
                l.diff.map(|d| d.file).hash(&mut h);
            }
        }
        Block::Folded { first, .. } => first.hash(&mut h),
        Block::TurnChanges { turn, .. } => turn.hash(&mut h),
        Block::Worked { turn, .. } => turn.hash(&mut h),
    }
    h.finish()
}

fn block_items(b: &Block) -> Vec<usize> {
    match b {
        Block::User { index } | Block::Task { index } | Block::Prose { index } | Block::Card { index } | Block::Handoff { index } => vec![*index],
        Block::Run { lines } => lines.iter().map(|l| l.index).collect(),
        Block::Folded { .. } | Block::TurnChanges { .. } | Block::Worked { .. } => vec![],
    }
}

/// The unit a block's text belongs to, in reading order.
fn block_unit(b: &Block, t: &Transcript) -> Unit {
    match b {
        Block::User { index } | Block::Task { index } | Block::Prose { index } | Block::Card { index } | Block::Handoff { index } => Unit::new(*index, 0),
        Block::Run { lines } => lines.first().map_or(Unit::new(0, 0), |l| Unit::new(l.index, l.diff.map_or(0, |d| d.file))),
        Block::Folded { first, .. } => Unit::new(t.index_of(first).unwrap_or(0), 0),
        Block::TurnChanges { turn, .. } => Unit::new(t.items().iter().rposition(|i| &i.turn_id == turn).unwrap_or(0), Unit::END_OF_TURN),
        // Right after the turn's prompt; the line itself is not selectable text.
        Block::Worked { turn, .. } => Unit::new(t.items().iter().position(|i| &i.turn_id == turn).unwrap_or(0), Unit::END_OF_TURN - 1),
    }
}

/// The raw text of a message item ("Copy message"), or `None` for other items.
fn message_text(item: &yaac_protocol::Item) -> Option<String> {
    match &item.body {
        ItemBody::UserMessage(u) => Some(yaac_core::message::copy_text(u)),
        ItemBody::AgentMessage(a) => Some(a.text.clone()),
        ItemBody::MeshReceived(r) => Some(r.message.text.clone()),
        ItemBody::MeshSent(s) => Some(s.message.text.clone()),
        _ => None,
    }
}

/// The text of an item's units as the transcript shows them, from the data alone: what a
/// selection copies for units that were never drawn (scrolled out of view, or skipped over).
fn item_units(blocks: &[Block], t: &Transcript, index: usize) -> Vec<(Unit, Vec<String>)> {
    let Some(item) = t.items().get(index) else { return Vec::new() };
    let md = |text: &str| {
        let parsed = markdown::parse(text);
        let refs: Vec<&markdown::Block> = parsed.iter().collect();
        markdown::plain_parts(&refs)
    };
    let mut out = Vec::new();
    for b in blocks {
        match b {
            // What the user typed keeps its line breaks (D157), as it is drawn.
            Block::User { index: i } if *i == index => {
                let parsed = markdown::parse_typed(&message_text(item).unwrap_or_default());
                out.push((Unit::new(index, 0), markdown::plain_parts(&parsed.iter().collect::<Vec<_>>())));
            }
            Block::Prose { index: i } | Block::Task { index: i } if *i == index => {
                out.push((Unit::new(index, 0), md(&message_text(item).unwrap_or_default())));
            }
            Block::Handoff { index: i } if *i == index => {
                if let ItemBody::Handoff(h) = &item.body {
                    out.push((Unit::new(index, 0), md(h.transcript.as_deref().unwrap_or(""))));
                }
            }
            Block::Card { index: i } if *i == index => {
                let text = match &item.body {
                    ItemBody::Request(r) => match &r.body {
                        yaac_protocol::RequestBody::Approval(a) => [Some(a.title.clone()), a.command.clone()].into_iter().flatten().collect::<Vec<_>>(),
                        _ => vec![item.kind().to_owned()],
                    },
                    ItemBody::Notice(n) => vec![n.text.clone()],
                    _ => Vec::new(),
                };
                out.push((Unit::new(index, 0), text));
            }
            Block::Run { lines } => {
                for l in lines.iter().filter(|l| l.index == index) {
                    let mut text = l.text.clone();
                    if let Some(d) = l.diff {
                        let (a, r) = yaac_core::diff::counts(d.added, d.removed);
                        text = format!("{text}  {a} {r}");
                    }
                    let mut parts = vec![text];
                    if l.expanded && let Some(thought) = thought_text(item) {
                        parts.extend(md(thought));
                    }
                    out.push((Unit::new(index, l.diff.map_or(0, |d| d.file)), parts));
                }
            }
            Block::Folded { first, text, .. } if t.index_of(first) == Some(index) => out.push((Unit::new(index, 0), vec![text.clone()])),
            Block::TurnChanges { turn, totals, .. } if t.items().iter().rposition(|i| &i.turn_id == turn) == Some(index) => {
                let (a, r) = yaac_core::diff::counts(totals.added, totals.removed);
                out.push((Unit::new(index, Unit::END_OF_TURN), vec![format!("{}  {a} {r}", totals.text())]));
            }
            _ => {}
        }
    }
    out
}

/// What a guess at a block's height needs from this pass's layout.
struct Metrics {
    width: f32,
    /// Points per character of prose, on average.
    char_w: f32,
    gap: f32,
}

impl Metrics {
    fn of(ui: &egui::Ui, width: f32) -> Self {
        const SAMPLE: &str = "The quick brown fox jumps over the lazy dog, then reads the file again.";
        let w = ui.fonts_mut(|f| f.layout_no_wrap(SAMPLE.to_owned(), theme::ui(size::PROSE), egui::Color32::WHITE).size().x);
        #[allow(clippy::cast_precision_loss, reason = "a short sample")]
        let char_w = w / SAMPLE.len() as f32;
        Self { width, char_w, gap: ui.spacing().item_spacing.y }
    }
}

/// A block's height before it is first drawn (D163, P251): from what it holds, at the
/// column's width, so the content's height and the scroll bar are near the truth. Kept per
/// block until its item or the width changes, so a long thread is not read again each frame.
fn guess(cache: &mut std::collections::HashMap<u64, (u64, f32, f32)>, ident: u64, b: &Block, t: &Transcript, m: &Metrics, flat: bool) -> f32 {
    #[allow(clippy::cast_precision_loss, reason = "a block's line count")]
    let lines = |n: usize| n as f32;
    let index = match b {
        Block::Run { lines: run } => {
            let mut height = lines(run.len()) * 22.0;
            for line in run.iter().filter(|line| !flat && line.expanded && line.mark == LineMark::Thought) {
                let Some(thought) = thought_text(&t.items()[line.index]) else { continue };
                let rev = t.item_rev(line.index);
                let mut thought_id = std::collections::hash_map::DefaultHasher::new();
                ("thought", &line.id).hash(&mut thought_id);
                let entry = cache.entry(thought_id.finish()).or_default();
                if entry.0 != rev || (entry.1 - m.width).abs() >= 0.5 {
                    let body = markdown::guess_height(thought, m.width - 30.0, m.char_w, size::PROSE, m.gap);
                    *entry = (rev, m.width, body + 10.0);
                }
                height += entry.2;
            }
            return height;
        }
        Block::Folded { .. } | Block::TurnChanges { .. } => return 22.0,
        Block::Worked { .. } => return crate::messages::WORKED_LINE,
        Block::Card { .. } => return 60.0,
        _ if flat => return 60.0,
        Block::User { index } | Block::Task { index } | Block::Prose { index } | Block::Handoff { index } => *index,
    };
    let rev = t.item_rev(index);
    if let Some((r, w, h)) = cache.get(&ident)
        && *r == rev
        && (*w - m.width).abs() < 0.5
    {
        return *h;
    }
    let Some(item) = t.items().get(index) else { return 60.0 };
    let text = |w: f32, s: &str| markdown::guess_height(s, w, m.char_w, size::PROSE, m.gap);
    let h = match (b, &item.body) {
        (Block::User { .. }, _) => {
            // In its bubble (D155): its share of the column, less the bubble's margins.
            let margin = crate::messages::BUBBLE_MARGIN;
            let max = (m.width * crate::messages::BUBBLE_SHARE).max(m.width.min(240.0));
            text(max - f32::from(margin.left + margin.right), &message_text(item).unwrap_or_default()) + f32::from(margin.top + margin.bottom)
        }
        // "From …" above a frame.
        (Block::Task { .. }, _) => 24.0 + 20.0 + text(m.width - 28.0, &message_text(item).unwrap_or_default()),
        (Block::Handoff { .. }, ItemBody::Handoff(h)) => text(m.width, h.transcript.as_deref().unwrap_or("")) + 34.0,
        _ => text(m.width, &message_text(item).unwrap_or_default()),
    };
    cache.insert(ident, (rev, m.width, h));
    h
}

/// Tests only: how one pass laid the transcript out.
#[cfg(test)]
#[derive(Clone, Debug, Default)]
pub(crate) struct Geom {
    /// The scroll offset the pass drew with, and the one it ended with.
    pub drawn_offset: f32,
    pub offset: f32,
    pub content: f32,
    pub view: f32,
    /// The bottom of the last block on screen.
    pub last_bottom: Option<f32>,
    /// Items whose block was drawn, and those placed at a guessed height (not measured as
    /// they are now).
    pub drawn: Vec<usize>,
    pub estimated: Vec<usize>,
    /// Each drawn block's identity and its top relative to the top of the view.
    pub placed: Vec<(u64, f32)>,
    /// Blocks measured for the first time: the height guessed for each, and the one drawn.
    pub guessed: Vec<(f32, f32)>,
}

pub(crate) fn transcript(ui: &mut egui::Ui, app: &mut YaacApp, map: &AttentionMap, key: &ThreadKey, now: i64, actions: &mut Vec<Action>) {
    let p = app.p;
    let state = &app.client.state;
    let Some(t) = app.client.state.transcripts.get(key) else { return };
    let preview = state.selected_thread().is_some_and(|owner| app.ui.dock_preview.target(owner) == Some(key));
    if !t.is_loaded() && t.items().is_empty() {
        ui.add_space(20.0);
        ui.vertical_centered(|ui| ui.label(widgets::secondary("Loading…", &p)));
        return;
    }
    let bl = blocks(state, map, key, &app.ui.expansion);
    // Finished turns fold into "Worked for …" and their final message (D156).
    let focus = state.focus_item.as_ref().filter(|(k, _)| k == key).map(|(_, id)| id);
    let bl = yaac_core::turns::collapse(t, key, &bl, &app.ui.worked, &app.ui.expansion, focus);
    let avail = ui.available_width();
    if std::env::var_os("YAAC_DEBUG_LAYOUT").is_some() {
        eprintln!("transcript rect {:?} avail {avail}", ui.max_rect());
    }
    let width = chat_width(avail, app.ui.chat_width);
    let x0 = ui.max_rect().left() + (avail - width) * 0.5;
    const GAP: f32 = 14.0;
    // Positions from measured heights. A block whose content changed since (a streaming
    // reply) keeps its last height until it is drawn and measured again, never a guess (D152);
    // only a block never drawn is guessed at.
    let idents: Vec<u64> = bl.iter().map(|b| block_ident(b, t)).collect();
    let mut heights = Vec::with_capacity(bl.len());
    let mut tops = Vec::with_capacity(bl.len());
    let mut y = 12.0;
    let metrics = Metrics::of(ui, width);
    for (i, b) in bl.iter().enumerate() {
        tops.push(y);
        let h = match app.ui.heights.get(&idents[i]) {
            Some(h) => *h,
            None => {
                // Tests only: the flat guesses of before (D163), to see that the view holds
                // still however wrong a guess is.
                #[cfg(test)]
                let flat = app.ui.flat_guesses;
                #[cfg(not(test))]
                let flat = false;
                guess(&mut app.ui.guesses, idents[i], b, t, &metrics, flat)
            }
        };
        heights.push(h);
        // A message keeps a line under it for its time (D154).
        y += h + GAP + crate::messages::stamp_space(b);
    }
    // The same room under the last block whatever it is, so the newest line does not move
    // when a message (with its time line, D154) gives way to an activity line.
    let total = y + 20.0 + bl.last().map_or(0.0, |b| crate::messages::STAMP_LINE - crate::messages::stamp_space(b));
    let mut sa = egui::ScrollArea::vertical().id_salt(("transcript", key.to_string())).auto_shrink([false, false]).stick_to_bottom(!t.history_away);
    if preview {
        sa = sa.vertical_scroll_offset((total - ui.available_height()).max(0.0));
    } else if let Some(a) = app.ui.restore.take() {
        // Back or forward: the same lines the user was reading, even if items arrived since.
        app.ui.scroll_end = false;
        app.ui.scroll_to = None;
        if let Some(i) = t.index_of(&a.item)
            && let Some(bi) = bl.iter().position(|b| block_unit(b, t).item >= i)
        {
            sa = sa.vertical_scroll_offset((tops[bi] + a.offset).max(0.0));
        }
    } else if let Some(item) = app.ui.timeline.go_to.take() {
        // A dash of the timeline, or Go to previous / next message (D153).
        if let Some(i) = t.index_of(&item)
            && let Some(bi) = bl.iter().position(|b| block_items(b).contains(&i))
        {
            app.ui.scroll_end = false;
            // Not stuck to the bottom this frame, or the view would be pulled back there.
            sa = sa.vertical_scroll_offset(yaac_core::timeline::scroll_target(tops[bi])).stick_to_bottom(false);
            // Heights measured around it may still move it: aim again for a few frames.
            app.ui.timeline.settle = Some((item.clone(), app.ui.timeline.settle.as_ref().filter(|(s, _)| *s == item).map_or(4, |(_, n)| *n)));
        } else if !t.is_loaded() {
            // Older history is still arriving: go there once it has.
            app.ui.timeline.go_to = Some(item);
        }
    } else if let Some(item) = app.ui.scroll_to.take() {
        if let Some(i) = t.index_of(&item)
            && let Some(bi) = bl.iter().position(|b| block_items(b).contains(&i))
        {
            sa = sa.vertical_scroll_offset((tops[bi] - 80.0).max(0.0));
        } else if !t.is_loaded() || t.loading_older {
            app.ui.scroll_to = Some(item);
        }
    } else {
        // At the end, the pass starts at the end of this layout (D152): egui's stick-to-bottom
        // only moves the offset when a pass ends, so content that grew would be drawn one pass
        // late, below the view. A jump to the end goes to the end itself, not past it.
        let view = app.ui.at_end.as_ref().filter(|(k, _)| k == key).map(|(_, v)| *v);
        if (t.is_loaded() && std::mem::take(&mut app.ui.scroll_end)) || view.is_some() {
            sa = sa.vertical_scroll_offset((total - view.unwrap_or_else(|| ui.available_height())).max(0.0));
        } else if let Some((_, id, off)) = app.ui.scroll_anchor.as_ref().filter(|(k, ..)| k == key)
            && let Some(i) = idents.iter().position(|x| x == id)
        {
            // Scrolled up: the block that was at the top of the view stays where it was on
            // screen (D163). Blocks above it measured since moved it in this layout; the
            // offset moves by as much, so the content does not jump under a still offset and
            // a wheel notch moves it by a notch, however wrong a guessed height was.
            sa = sa.vertical_scroll_offset((tops[i] + off).max(0.0));
        }
    }
    let mut changed = false;
    let mut actions_local = Vec::new();
    let original_selection = preview.then(|| std::mem::take(&mut app.ui.textsel));
    app.ui.textsel.begin(key);
    let mut area = ui.max_rect();
    let mut reading = None;
    #[cfg(test)]
    let mut geom = Geom::default();
    let out = sa.show_viewport(ui, |ui, view| {
        #[cfg(test)]
        {
            geom.drawn_offset = view.top();
        }
        ui.set_height(total);
        area = ui.clip_rect();
        let origin = ui.max_rect().top();
        for (i, b) in bl.iter().enumerate() {
            let top = tops[i];
            let est = heights[i];
            if top + est < view.top() - 300.0 || top > view.bottom() + 300.0 {
                continue;
            }
            let rect = Rect::from_min_size(pos2(x0, origin + top), vec2(width, est.max(1.0)));
            // A message's own right-click menu: under its text (added first, so the text and
            // any link in it stay on top).
            let menu = match b {
                Block::User { index } | Block::Task { index } | Block::Prose { index } => Some(*index),
                _ => None,
            };
            if let Some(index) = menu {
                let bg = ui.interact(rect, ui.scope_id().with(("message-menu", idents[i])), Sense::click());
                let has_sel = app.ui.textsel.selection().is_some();
                let mut copy_sel = false;
                crate::widgets::menu_popup(egui::Popup::context_menu(&bg), &p).show(|ui| {
                    if has_sel && crate::widgets::menu_item(ui, &p, "Copy", Some("Ctrl+C"), crate::widgets::ItemKind::Normal).clicked() {
                        copy_sel = true;
                        ui.close();
                    }
                    if ui.button("Copy message").clicked() {
                        if let Some(text) = t.items().get(index).and_then(message_text) {
                            ui.ctx().copy_text(text);
                        }
                        ui.close();
                    }
                });
                if copy_sel {
                    let text = app.ui.textsel.copy(|n| item_units(&bl, t, n));
                    ui.ctx().copy_text(text);
                }
            }
            let unit = block_unit(b, t);
            // Ids by identity, so a streaming block's widgets (an open menu, a hovered link)
            // live on through its updates.
            let r = ui.scope_builder(UiBuilder::new().max_rect(rect).id_salt(("block", idents[i])), |ui| {
                ui.set_width(width);
                render_block(ui, View { p, state: &app.client.state, ui: &mut app.ui, md: &mut app.md, images: &mut app.images }, key, t, b, unit, now, &mut actions_local);
            });
            let h = r.response.rect.height();
            // Its time, while it is hovered, in the line kept free under it (D154).
            let space = crate::messages::stamp_space(b);
            if space > 0.0 {
                let drawn = r.response.rect;
                let reach = Rect::from_min_max(drawn.min, pos2(drawn.right(), drawn.bottom() + space));
                let forced = block_items(b).first().and_then(|&n| t.items().get(n)).is_some_and(|it| app.ui.timeline.force_stamp.as_ref() == Some(&it.id));
                // Screenshots show a time only where asked, not wherever the pointer rests.
                if (forced || (app.shots.is_none() && ui.rect_contains_pointer(reach)))
                    && let Some(at) = crate::messages::message_time(b, t)
                {
                    crate::messages::stamp(ui, &p, drawn, at, now, app.zone, matches!(b, Block::User { .. }));
                }
            }
            #[cfg(test)]
            {
                geom.drawn.push(unit.item);
                geom.placed.push((idents[i], r.response.rect.top() - ui.clip_rect().top()));
                if !app.ui.heights.contains_key(&idents[i]) {
                    geom.estimated.push(unit.item);
                    geom.guessed.push((heights[i], h));
                }
                if i + 1 == bl.len() {
                    geom.last_bottom = Some(r.response.rect.bottom());
                }
            }
            // Under half a point is the same height: no new layout for it.
            let moved = app.ui.heights.get(&idents[i]).is_none_or(|old| (old - h).abs() > 0.5);
            if moved {
                app.ui.heights.insert(idents[i], h);
            }
            changed |= moved;
        }
        app.ui.textsel.autoscroll(ui, ui.clip_rect());
        // Where the user is reading: the first block reaching into the view.
        reading = bl.iter().enumerate().find(|(i, _)| tops[*i] + heights[*i] > view.top()).and_then(|(i, b)| {
            let item = t.items().get(block_unit(b, t).item)?.id.clone();
            Some(yaac_core::history::Anchor { item, offset: view.top() - tops[i] })
        });
    });
    // Re-measure changed blocks before showing the frame.
    if changed {
        ui.ctx().request_discard("a transcript block changed height");
        if !ui.ctx().will_discard() {
            ui.ctx().request_repaint();
        }
    }
    if let Some(selection) = original_selection {
        app.ui.textsel = selection;
        return;
    }
    let view_top = out.state.clamped_offset().y;
    let view_bottom = view_top + out.inner_rect.height();
    // Prefetch the adjacent page before the reader reaches either edge of the window.
    if t.is_loaded() && !t.loading_older && !t.loading_newer && app.ui.timeline.go_to.is_none() {
        let remaining = (total - view_bottom).max(0.0);
        if t.newer_cursor.is_some() && remaining < 220.0 && (view_top >= 220.0 || view_top > remaining) {
            actions.push(Action::NewerHistory(key.clone()));
        } else if t.older_cursor.is_some() && view_top < 220.0 {
            actions.push(Action::OlderHistory(key.clone()));
        } else if t.newer_cursor.is_some() && remaining < 220.0 {
            actions.push(Action::NewerHistory(key.clone()));
        }
    }
    // Dashes use the full prompt index; the loaded rows only determine the highlight.
    {
        use yaac_core::timeline as timeline;
        let entries = timeline::entries(t, &bl);
        let etops: Vec<f32> = entries
            .iter()
            .map(|e| bl.iter().position(|b| matches!(b, Block::User { index } | Block::Task { index } if *index == e.index)).map_or(0.0, |bi| tops[bi]))
            .collect();
        let visible = timeline::visible(&etops, view_top, view_bottom);
        let index = t.prompts.as_deref();
        let highlight = if let Some(index) = index {
            let id_at = |i| entries.get(i).and_then(|entry: &timeline::Entry| index.iter().position(|p| p.item_id == entry.item));
            match (id_at(visible.start), visible.end.checked_sub(1).and_then(id_at)) {
                (Some(first), Some(last)) => first..last + 1,
                _ => 0..0,
            }
        } else {
            visible
        };
        if let Some(item) = crate::timeline::show(ui, &mut app.ui.timeline, &p, area, &entries, index, highlight, now, app.zone) {
            if t.item(&item).is_none() {
                actions.push(Action::FocusHistory(key.clone(), item.clone()));
            }
            app.ui.timeline.go_to = Some(item);
            ui.ctx().request_repaint();
        }
        app.ui.timeline.last = Some((key.clone(), entries.into_iter().map(|e| e.item).zip(etops).collect(), view_top));
    }
    if matches!(app.ui.history.current().map(|e| &e.place), Some(yaac_core::history::Place::Thread(k)) if k == key) && reading.is_some() {
        app.ui.history.set_anchor(reading);
    }
    // Away from the newest message (D142): remember how many messages there were when the
    // view left the bottom, and offer a way back above the composer.
    let at_end = !t.history_away && out.state.clamped_offset().y + out.inner_rect.height() >= out.content_size.y - 8.0;
    // Exactly at the end: egui's own test for sticking to it.
    let stuck = !t.history_away && out.state.unclamped_offset().y + out.inner_rect.height() >= out.content_size.y - 0.5;
    app.ui.at_end = stuck.then(|| (key.clone(), out.inner_rect.height()));
    // What the next pass keeps in place (D163): the block reaching into the top of the view,
    // as a browser's scroll anchoring does. Its own growth (a reply streaming into it) is
    // below its top, so it does not move what is read either.
    let top = out.state.clamped_offset().y;
    app.ui.scroll_anchor = if stuck {
        None
    } else {
        (0..bl.len()).find(|&i| tops[i] + heights[i] > top).map(|i| (key.clone(), idents[i], top - tops[i]))
    };
    #[cfg(test)]
    {
        geom.offset = out.state.unclamped_offset().y;
        geom.content = out.content_size.y;
        geom.view = out.inner_rect.height();
        app.ui.geom.push(geom);
    }
    if std::env::var_os("YAAC_DEBUG_LAYOUT").is_some() {
        eprintln!("transcript offset {} view {} content {}", out.state.clamped_offset().y, out.inner_rect.height(), out.content_size.y);
    }
    let messages = t.items().iter().filter(|i| matches!(i.body, ItemBody::UserMessage(_) | ItemBody::AgentMessage(_))).count();
    if at_end {
        app.ui.away_from_end = None;
    } else if !app.ui.away_from_end.as_ref().is_some_and(|(k, _)| k == key) {
        app.ui.away_from_end = Some((key.clone(), messages));
    }
    if let Some((_, seen)) = app.ui.away_from_end.as_ref().filter(|(k, _)| k == key) {
        let fresh = messages.saturating_sub(*seen);
        let text = match (t.history_away, fresh) {
            (true, _) => "Latest messages".to_owned(),
            (_, 0) => "Jump to bottom".to_owned(),
            (_, 1) => "1 new message".to_owned(),
            (_, n) => format!("{n} new messages"),
        };
        // "3 new messages (Ctrl+End)", with the key as bound now (D131).
        let label = crate::commander::tip(app, &text, yaac_core::commands::Cmd::JumpToBottom);
        let font = theme::ui(size::SMALL);
        let g = ui.fonts_mut(|f| f.layout_no_wrap(label, font, p.text));
        let size = g.size() + vec2(28.0, 12.0);
        let r = Rect::from_center_size(pos2(area.center().x, area.bottom() - 14.0 - size.y / 2.0), size);
        let resp = ui.interact(r, ui.scope_id().with(("jump-to-bottom", key.to_string())), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
        let fill = if resp.hovered() { p.selected } else { p.raised };
        ui.painter().rect_filled(r.expand(1.0), size.y / 2.0, p.bg.gamma_multiply(0.3));
        ui.painter().rect_filled(r, size.y / 2.0, fill);
        ui.painter().galley(r.center() - g.size() / 2.0, g, p.text);
        if resp.clicked() {
            if t.history_away {
                actions.push(Action::LatestHistory(key.clone()));
            } else {
                app.ui.scroll_end = true;
            }
            ui.ctx().request_repaint();
        }
    }
    if app.ui.textsel.end(ui, area, |n| item_units(&bl, t, n)) {
        ui.ctx().request_repaint();
    }
    actions.extend(actions_local);
    // A prompt just gone to (D153): while heights around it are still being measured, aim
    // again, a few frames at most.
    match app.ui.timeline.settle.take() {
        Some((item, n)) if changed && n > 0 => {
            app.ui.timeline.settle = Some((item.clone(), n - 1));
            app.ui.timeline.go_to = Some(item);
        }
        _ => {}
    }
}

/// What block rendering needs, borrowed apart from the heights cache.
pub(crate) struct View<'a> {
    pub p: Palette,
    pub state: &'a yaac_core::state::AppState,
    pub ui: &'a mut crate::app::UiState,
    pub md: &'a mut markdown::Cache,
    pub images: &'a mut crate::images::Images,
}

impl View<'_> {
    /// The selection context for a unit.
    pub(crate) fn sel(&mut self, unit: Unit) -> SelCx<'_> {
        SelCx { ts: &mut self.ui.textsel, unit }
    }

    fn assistant_prose(&mut self, ui: &mut egui::Ui, unit: Unit, item: &yaac_protocol::Item, text: &str) {
        let p = self.p;
        let complete = item.status != yaac_protocol::ItemStatus::InProgress;
        let parsed = self.md.get(&item.id, text, complete).clone();
        let refs: Vec<&markdown::Block> = parsed.blocks().collect();
        markdown::show(ui, &mut self.sel(unit), &refs, &p, p.text, size::PROSE);
    }
}

#[allow(clippy::too_many_arguments)]
fn render_block(ui: &mut egui::Ui, mut v: View<'_>, key: &ThreadKey, t: &Transcript, b: &Block, unit: Unit, now: i64, actions: &mut Vec<Action>) {
    let p = v.p;
    match b {
        Block::User { index } => {
            let item = &t.items()[*index];
            let ItemBody::UserMessage(u) = &item.body else { return };
            let context = yaac_core::message::context(u);
            let context_open = v.ui.expansion.context.contains(&(key.clone(), item.id.clone()));
            if yaac_core::message::context_only(u) {
                // Only what the harness added for the model: a quiet line, no bubble.
                if context_disclosure(ui, &p, &context, context_open, true) {
                    v.ui.expansion.toggle_context(key, &item.id);
                }
                if context_open {
                    context_body(ui, &mut v.sel(unit), &p, &context);
                }
                return;
            }
            // The bubble hugs a message that is only short typed text; anything else takes
            // the bubble's full width (D155).
            let pieces = yaac_core::message::pieces(u);
            let plain = context.is_empty() && pieces.len() == 1 && !u.parts.iter().any(|x| matches!(x, InputPart::File(_) | InputPart::Image(_)));
            let natural = match pieces.first() {
                Some(yaac_core::message::Piece::Typed(text)) if plain => {
                    let parsed = v.md.get_typed(&item.id, text).clone();
                    let refs: Vec<&markdown::Block> = parsed.blocks().collect();
                    markdown::natural_width(ui, &refs, &p, size::PROSE)
                }
                _ => None,
            };
            crate::messages::bubble(ui, &p, natural, |ui| {
                let mut first = true;
                for (n, piece) in pieces.iter().enumerate() {
                    if !first {
                        ui.add_space(6.0);
                    }
                    first = false;
                    match piece {
                        yaac_core::message::Piece::Typed(text) => {
                            // Each typed part has its own parse; a message with one part keeps
                            // the item's own cache entry.
                            let id = if n == 0 { item.id.clone() } else { yaac_protocol::ItemId::new(format!("{}#{n}", item.id)) };
                            let parsed = v.md.get_typed(&id, text).clone();
                            let refs: Vec<&markdown::Block> = parsed.blocks().collect();
                            markdown::show(ui, &mut v.sel(unit), &refs, &p, p.text, size::PROSE);
                        }
                        yaac_core::message::Piece::Command(text) => command_chip(ui, &mut v.sel(unit), &p, text),
                        yaac_core::message::Piece::Pasted { part, text, lines, code } => {
                            let open = v.ui.expansion.pasted.contains(&(key.clone(), item.id.clone(), *part));
                            if pasted_block(ui, &mut v, unit, &item.id, *part, text, *lines, *code, open) {
                                v.ui.expansion.toggle_pasted(key, &item.id, *part);
                            }
                        }
                    }
                }
                let files: Vec<(&yaac_protocol::UploadId, Option<&String>, bool)> = u
                    .parts
                    .iter()
                    .filter_map(|x| match x {
                        InputPart::File(f) => Some((&f.upload_id, f.name.as_ref(), false)),
                        InputPart::Image(i) => Some((&i.upload_id, i.name.as_ref(), true)),
                        _ => None,
                    })
                    .collect();
                if !files.is_empty() {
                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        for (up, name, image) in files {
                            let gone = t.missing_uploads.contains(up);
                            // The file's own name when the part carries it.
                            let name = name.cloned().unwrap_or_else(|| upload_name(up.as_str()));
                            if image && !gone {
                                let source = crate::images::ImageSource::Target(key.target.clone(), up.clone());
                                if ui.button(&name).on_hover_text("Open image").clicked() {
                                    actions.push(Action::OpenImage(source, name));
                                }
                            } else {
                                chip(ui, &p, &name, image, gone);
                            }
                        }
                    });
                }
                if !context.is_empty() {
                    ui.add_space(4.0);
                    if context_disclosure(ui, &p, &context, context_open, false) {
                        v.ui.expansion.toggle_context(key, &item.id);
                    }
                    if context_open {
                        context_body(ui, &mut v.sel(unit), &p, &context);
                    }
                }
            });
            if let Some(pending) = t.pending_of(&item.id) {
                send_state(ui, &p, key, pending, actions);
            }
        }
        Block::Task { index } => {
            let item = &t.items()[*index];
            let ItemBody::MeshReceived(r) = &item.body else { return };
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.label(widgets::secondary("From", &p));
                peer_link(ui, &p, v.state, &r.from, Some(&r.message.id), false, actions);
                ui.label(widgets::secondary(peer_rest(&r.from), &p));
            });
            ui.add_space(4.0);
            widgets::faint_frame(&p).inner_margin(Margin::symmetric(14, 10)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                let parsed = v.md.get(&item.id, &r.message.text, true).clone();
                let refs: Vec<&markdown::Block> = parsed.blocks().collect();
                markdown::show(ui, &mut v.sel(unit), &refs, &p, p.text, size::PROSE);
            });
        }
        Block::Prose { index } => {
            let item = &t.items()[*index];
            let ItemBody::AgentMessage(a) = &item.body else { return };
            v.assistant_prose(ui, unit, item, &a.text);
            if item.status == yaac_protocol::ItemStatus::InProgress {
                // The cursor at the end of streaming prose.
                let (r, _) = ui.allocate_exact_size(vec2(8.0, 18.0), Sense::hover());
                ui.painter().rect_filled(Rect::from_min_size(r.min, vec2(7.0, 17.0)), 1.0, p.text2);
            }
        }
        Block::Card { index } => {
            let top = ui.cursor().min;
            crate::cards::show(ui, &mut v, key, t, *index, now, actions);
            let rect = Rect::from_min_max(top, pos2(ui.max_rect().right(), ui.min_rect().bottom()));
            v.sel(unit).whole(ui, rect);
        }
        Block::Handoff { index } => {
            let item = &t.items()[*index];
            let ItemBody::Handoff(h) = &item.body else { return };
            if let Some(text) = &h.transcript {
                let parsed = v.md.get(&item.id, text, true).clone();
                let refs: Vec<&markdown::Block> = parsed.blocks().collect();
                markdown::show(ui, &mut v.sel(unit), &refs, &p, p.text2, size::PROSE - 1.0);
            }
            ui.add_space(10.0);
            let src = v.state.summary(&ThreadKey::new(key.target.clone(), h.forked_from.clone())).map_or_else(|| h.forked_from.to_string(), |s| s.title.clone());
            let label = format!("Inherited from {src} · read-only above this line");
            let g = ui.fonts_mut(|f| f.layout_no_wrap(label, theme::ui(size::SMALL), p.text2));
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 20.0), Sense::hover());
            let gx = r.center().x - g.size().x / 2.0;
            ui.painter().hline(r.left()..=gx - 10.0, r.center().y, egui::Stroke::new(1.0, p.border));
            ui.painter().hline(gx + g.size().x + 10.0..=r.right(), r.center().y, egui::Stroke::new(1.0, p.border));
            ui.painter().galley(pos2(gx, r.center().y - g.size().y / 2.0), g, p.text2);
        }
        Block::Run { lines } => {
            let prose_gap = ui.spacing().item_spacing.y;
            ui.spacing_mut().item_spacing.y = 0.0;
            for l in lines {
                activity_line(ui, &mut v, key, t, l, now, prose_gap, actions);
            }
        }
        Block::Folded { first, text, failed, .. } => {
            let resp = line_row(ui, &mut v.sel(unit), &p, Row { state_mark: if *failed { Some(Mark::Failed) } else { None }, mark: LineMark::Other, text, highlighted: false, folded: true, counts: None, trailer: None, open: None });
            if resp.clicked() {
                v.ui.expansion.toggle_run(key, first);
            }
        }
        Block::TurnChanges { turn, totals, expanded, items } => {
            let resp = line_row(
                ui,
                &mut v.sel(unit),
                &p,
                Row {
                    state_mark: None,
                    mark: LineMark::Tool,
                    text: &totals.text(),
                    highlighted: false,
                    folded: true,
                    counts: Some((totals.added, totals.removed)),
                    trailer: Some(if *expanded { "hide all" } else { "show all" }),
                    open: None,
                },
            );
            if resp.clicked() {
                v.ui.expansion.toggle_turn(key, turn, items);
            }
        }
        Block::Worked { turn, text, failed, error, expanded } => {
            if crate::messages::worked_line(ui, &p, text, *failed, error.as_deref(), *expanded).clicked() {
                v.ui.worked.toggle(key, turn);
                // Closing a turn whose "show all" is open closes that too, or it would stay open.
                if *expanded && v.ui.expansion.turns.contains(&(key.clone(), turn.clone())) {
                    let items: Vec<yaac_protocol::ItemId> = t.items().iter().filter(|i| &i.turn_id == turn).map(|i| i.id.clone()).collect();
                    v.ui.expansion.toggle_turn(key, turn, &items);
                    if v.ui.worked.is_open(key, turn) {
                        v.ui.worked.toggle(key, turn);
                    }
                }
            }
        }
    }
}

/// Under the echo of a sent message (D165): "Sending…" quietly in the line kept free under it
/// (nothing moves when it goes); a failed send's error with **Edit** and **Retry**; an unknown
/// outcome's sentence with **Edit** and **Dismiss** — never a Retry.
fn send_state(ui: &mut egui::Ui, p: &Palette, key: &ThreadKey, pending: &yaac_core::sending::Pending, actions: &mut Vec<Action>) {
    let Some(note) = pending.note() else { return };
    if !pending.settled() {
        let drawn = ui.min_rect();
        let g = ui.fonts_mut(|f| f.layout_no_wrap(note, theme::ui(size::SMALL - 0.5), p.text2));
        let at = pos2(drawn.right() - g.size().x - 2.0, drawn.bottom() + (crate::messages::STAMP_LINE - g.size().y) / 2.0);
        ui.painter().galley(at, g, p.text2);
        return;
    }
    let color = if pending.can_retry() { p.failed } else { p.text2 };
    ui.add_space(4.0);
    ui.with_layout(egui::Layout::top_down(egui::Align::Max), |ui| {
        ui.label(egui::RichText::new(note).font(theme::ui(size::SMALL + 0.5)).color(color));
        ui.add_space(2.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if pending.can_retry() {
                if widgets::plain(ui, p, "Retry", None).clicked() {
                    actions.push(Action::RetrySend(key.clone(), pending.item.clone()));
                }
            } else if widgets::plain(ui, p, "Dismiss", None).clicked() {
                actions.push(Action::DismissSend(key.clone(), pending.item.clone()));
            }
            if widgets::plain(ui, p, "Edit", None).clicked() {
                actions.push(Action::EditSend(key.clone(), pending.item.clone()));
            }
        });
    });
}

fn upload_name(id: &str) -> String {
    id.rsplit('/').next().unwrap_or(id).to_owned()
}

fn chip(ui: &mut egui::Ui, p: &Palette, name: &str, image: bool, gone: bool) {
    egui::Frame::NONE.fill(p.raised).corner_radius(5).inner_margin(Margin::symmetric(8, 4)).show(ui, |ui| {
        ui.horizontal(|ui| {
            let c = if gone { p.text2 } else { p.text };
            widgets::icon(ui, 15.0, |pa, ce, d| if image && !gone { icons::image(pa, ce, d, c) } else { icons::file(pa, ce, d, c, gone) });
            ui.label(egui::RichText::new(name).color(c));
            if gone {
                ui.label(widgets::secondary("· no longer on the target", p));
            }
        });
    });
}

/// A command the user ran ("/model opus", "!ls"): a compact monochrome chip, its text
/// selectable.
fn command_chip(ui: &mut egui::Ui, sc: &mut SelCx<'_>, p: &Palette, text: &str) {
    ui.horizontal(|ui| {
        egui::Frame::NONE.fill(p.raised).corner_radius(5).inner_margin(Margin::symmetric(8, 3)).show(ui, |ui| {
            let font = theme::mono(size::MONO);
            let g = ui.fonts_mut(|f| f.layout(text.to_owned(), font, p.text, (ui.available_width() - 16.0).max(40.0)));
            sc.label(ui, g, p.text);
        });
    });
}

/// Pasted text in a user message: "Pasted text · N lines", collapsed; opened, the text as it
/// was pasted — plain text, or code when the paste is a fenced block. Returns true when its
/// header was clicked.
#[allow(clippy::too_many_arguments)]
fn pasted_block(ui: &mut egui::Ui, v: &mut View<'_>, unit: Unit, item: &yaac_protocol::ItemId, part: usize, text: &str, lines: usize, code: bool, open: bool) -> bool {
    let p = v.p;
    let mut clicked = false;
    let fr = egui::Frame::NONE.fill(p.bg).corner_radius(5).inner_margin(Margin::symmetric(8, 4)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        clicked = widgets::disclosure_row(ui, &p, &yaac_core::message::pasted_label(lines), open, ui.scope_id().with(("pasted", item, part)));
        if open {
            ui.add_space(2.0);
            if code {
                let id = yaac_protocol::ItemId::new(format!("{item}#paste{part}"));
                let parsed = v.md.get(&id, text, true).clone();
                let refs: Vec<&markdown::Block> = parsed.blocks().collect();
                markdown::show(ui, &mut v.sel(unit), &refs, &p, p.text, size::PROSE);
            } else {
                v.sel(unit).text(ui, &bounded(text), theme::ui(size::PROSE - 1.0), p.text);
            }
        }
    });
    if open && !code {
        markdown::copy_button(ui, &p, fr.response.rect, text, ui.scope_id().with(("pasted-copy", item, part)));
    }
    clicked
}

/// The "Context" disclosure of a user message (or, for a message that is only context, its
/// one quiet line). Returns true when clicked.
fn context_disclosure(ui: &mut egui::Ui, p: &Palette, ctx: &[yaac_core::message::Context<'_>], open: bool, standalone: bool) -> bool {
    let label = if standalone && ctx.len() == 1 { ctx[0].label.to_owned() } else { yaac_core::message::context_label(ctx.len()) };
    let id = ui.scope_id().with(("context", ctx.len(), ctx.first().map(|c| c.text.len())));
    ui.horizontal(|ui| widgets::disclosure_row(ui, p, &label, open, id)).inner
}

/// What the harness added for the model, each part under its name, as plain text.
fn context_body(ui: &mut egui::Ui, sc: &mut SelCx<'_>, p: &Palette, ctx: &[yaac_core::message::Context<'_>]) {
    egui::Frame::NONE.inner_margin(Margin { left: 16, right: 0, top: 2, bottom: 4 }).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 3.0;
        for (n, c) in ctx.iter().enumerate() {
            if n > 0 {
                ui.add_space(4.0);
            }
            sc.text(ui, c.label, theme::bold(size::SMALL), p.text2);
            sc.text(ui, &bounded(c.text), theme::ui(size::SMALL + 0.5), p.text2);
        }
    });
}

/// "Codex · devbox" after a peer's name.
fn peer_rest(peer: &yaac_protocol::MeshPeer) -> String {
    let mut parts = Vec::new();
    if let Some(pr) = &peer.provider {
        parts.push(pr.clone());
    }
    if let Some(t) = &peer.target_name {
        parts.push(t.clone());
    }
    if parts.is_empty() { String::new() } else { format!("· {}", parts.join(" · ")) }
}

/// A peer's name as a link: following it opens the other thread at the matching item. A
/// recipient that no longer exists is plain text.
fn peer_link(
    ui: &mut egui::Ui,
    p: &Palette,
    state: &yaac_core::state::AppState,
    peer: &yaac_protocol::MeshPeer,
    message: Option<&yaac_protocol::MessageId>,
    gone: bool,
    actions: &mut Vec<Action>,
) {
    let title = peer.title.clone().unwrap_or_else(|| peer.address.to_string());
    let key = state.key_of(&peer.address);
    match key.filter(|_| !gone) {
        Some(k) => {
            if widgets::link(ui, p, &title, theme::bold(size::SMALL)).clicked() {
                // Land on the matching item at the other end, when it is loaded.
                let target_item = message.and_then(|mid| {
                    state.transcripts.get(&k).and_then(|t| {
                        t.items().iter().find_map(|i| match &i.body {
                            ItemBody::MeshReceived(r) if &r.message.id == mid => Some(i.id.clone()),
                            ItemBody::MeshSent(s) if &s.message.id == mid => Some(i.id.clone()),
                            _ => None,
                        })
                    })
                });
                actions.push(Action::OpenAt(k, target_item));
            }
        }
        None => {
            ui.label(egui::RichText::new(title).font(theme::bold(size::SMALL)).color(p.text));
        }
    }
}

/// One activity line's look.
#[derive(Clone, Copy)]
struct Row<'a> {
    state_mark: Option<Mark>,
    mark: LineMark,
    text: &'a str,
    highlighted: bool,
    folded: bool,
    /// Lines added and removed, in the git colours after the text.
    counts: Option<(u32, u32)>,
    /// A quiet word at the end ("show all").
    trailer: Option<&'a str>,
    /// The line opens to show more: `Some(true)` while it is open (D158). An open line shows
    /// a down chevron and a faint fill, so it is clear where to click to close it.
    open: Option<bool>,
}

/// An activity line: one muted row. Its text is selectable (a drag selects; a click still
/// expands it).
fn line_row(ui: &mut egui::Ui, sc: &mut SelCx<'_>, p: &Palette, row: Row<'_>) -> egui::Response {
    let width = ui.available_width();
    let clickable = row.folded || row.open.is_some();
    let sense = if clickable { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 22.0), sense);
    if row.highlighted {
        ui.painter().rect_filled(rect, 4.0, p.selected);
    } else if clickable && resp.hovered() || row.open == Some(true) {
        ui.painter().rect_filled(rect, 4.0, p.faint);
    }
    let color = p.text2;
    let c = pos2(rect.left() + 9.0, rect.center().y);
    let pa = ui.painter();
    match (row.state_mark, row.mark) {
        (Some(m), _) => icons::mark(pa, c, 12.0, m, p),
        (None, LineMark::Sent) => icons::arrow_out(pa, c, 12.0, color),
        (None, LineMark::Received) => icons::arrow_in(pa, c, 12.0, color),
        (None, LineMark::Workers) => icons::people(pa, c, 12.0, color),
        (None, LineMark::Waiting) => icons::hourglass(pa, c, 11.0, color),
        (None, LineMark::Notice) => icons::info(pa, c, 11.0, color),
        (None, LineMark::Interrupted) => icons::stop_square(pa, c, 14.0, color),
        (None, _) if row.folded => icons::chevron(pa, c, 11.0, false, color),
        // Open: a down chevron. Closed: the quiet dot, a right chevron under the pointer.
        (None, _) if row.open == Some(true) => icons::chevron(pa, c, 11.0, true, color),
        (None, _) if row.open == Some(false) && resp.hovered() => icons::chevron(pa, c, 11.0, false, color),
        (None, _) => {
            pa.circle_filled(c, 2.0, color);
        }
    }
    let font = theme::ui(size::SMALL + 0.5);
    // The counts and the trailer take what they need; the text gets the rest.
    let extra: Vec<(String, egui::Color32)> = row
        .counts
        .map(|(a, r)| {
            let (a, r) = yaac_core::diff::counts(a, r);
            vec![(a, p.git_added), (r, p.git_deleted)]
        })
        .unwrap_or_default()
        .into_iter()
        .chain(row.trailer.map(|t| (format!("· {t}"), p.text2)))
        .collect();
    let extra_g: Vec<_> = extra.iter().map(|(s, c)| ui.fonts_mut(|f| f.layout_no_wrap(s.clone(), font.clone(), *c))).collect();
    let extra_w: f32 = extra_g.iter().map(|g| g.size().x + 8.0).sum();
    let g = crate::widgets::elided(ui, row.text, &font, color, width - 28.0 - extra_w);
    let gw = g.size().x;
    let y = rect.center().y - g.size().y / 2.0;
    sc.paint(ui, pos2(rect.left() + 22.0, y), g, color);
    let mut x = rect.left() + 22.0 + gw + 10.0;
    for (g, (_, c)) in extra_g.into_iter().zip(&extra) {
        let w = g.size().x;
        ui.painter().galley(pos2(x, y), g, *c);
        x += w + 6.0;
    }
    if clickable { resp.on_hover_cursor(egui::CursorIcon::PointingHand) } else { resp }
}

#[allow(clippy::too_many_arguments)]
fn activity_line(ui: &mut egui::Ui, v: &mut View<'_>, key: &ThreadKey, t: &Transcript, l: &ActivityLine, now: i64, prose_gap: f32, actions: &mut Vec<Action>) {
    let p = v.p;
    let item = &t.items()[l.index];
    let thought = thought_text(item);
    let unit = Unit::new(l.index, l.diff.map_or(0, |d| d.file));
    let state_mark = if l.failed { Some(Mark::Failed) } else { None };
    let resp = match &l.call {
        Some(call) => {
            let lead = if l.failed && let ItemBody::ToolCall(tc) = &item.body {
                failed_call_label(tc)
            } else {
                &call.lead
            };
            call_row(ui, &mut v.sel(unit), &p, l, call, lead, actions)
        }
        None => line_row(
            ui,
            &mut v.sel(unit),
            &p,
            Row { state_mark, mark: l.mark, text: &l.text, highlighted: l.highlighted, folded: false, counts: l.diff.map(|d| (d.added, d.removed)), trailer: None, open: (l.mark != LineMark::Thought || thought.is_some()).then_some(l.expanded) },
        ),
    };
    if resp.clicked() {
        match l.diff {
            Some(d) => v.ui.expansion.toggle_file(key, &l.id, d.file),
            None if thought.is_some() => v.ui.expansion.toggle_thought(key, &l.id),
            None => v.ui.expansion.toggle_line(key, &l.id),
        }
    }
    if !l.expanded {
        return;
    }
    if let Some(d) = l.diff
        && let ItemBody::ToolCall(tc) = &item.body
    {
        let rest = v.ui.expansion.rest.contains(&(key.clone(), l.id.clone(), d.file));
        let files = v.ui.diffs.get(&l.id, &tc.output);
        if let Some(f) = files.get(d.file) {
            ui.add_space(2.0);
            if diff_view(ui, &mut v.sel(unit), &p, f, rest, ui.scope_id().with(("diff", &l.id, d.file))) {
                v.ui.expansion.open_rest(key, &l.id, d.file);
            }
            ui.add_space(6.0);
        }
        return;
    }
    let fill = if l.highlighted { p.selected } else { egui::Color32::TRANSPARENT };
    if let (Some(call), ItemBody::ToolCall(tc)) = (&l.call, &item.body)
        && let Some(ac) = &tc.agent_call
    {
        let raw_open = v.ui.expansion.raw.contains(&(key.clone(), l.id.clone()));
        let mut toggle_raw = false;
        egui::Frame::NONE.fill(fill).corner_radius(4).inner_margin(Margin { left: 22, right: 8, top: 2, bottom: 8 }).show(ui, |ui| {
            ui.set_width(ui.available_width());
            toggle_raw = call_detail(ui, v, unit, t, item, tc, ac, call, raw_open, actions);
            tool_images(ui, v.images, key, &tc.images, actions);
        });
        if toggle_raw {
            v.ui.expansion.toggle_raw(key, &l.id);
        }
        return;
    }
    let body = egui::Frame::NONE.fill(fill).corner_radius(4).inner_margin(Margin { left: 22, right: 8, top: 2, bottom: 8 }).show(ui, |ui| {
        ui.set_width(ui.available_width());
        if let Some(text) = thought {
            ui.spacing_mut().item_spacing.y = prose_gap;
            v.assistant_prose(ui, unit, item, text);
            return;
        }
        ui.spacing_mut().item_spacing.y = 3.0;
        let small = theme::ui(size::SMALL + 0.5);
        let mut sc = SelCx { ts: &mut v.ui.textsel, unit };
        match &item.body {
            ItemBody::ToolCall(tc) => {
                // The line above already says it; repeat the title only when it says more.
                if tc.title.replace('`', "").trim() != l.text.replace('`', "").trim() {
                    sc.job(ui, markdown::inline_job(&tc.title, &small, p.text2, &p, ui.available_width()), p.text2);
                }
                if let Some(cwd) = &tc.cwd {
                    sc.text(ui, cwd.as_str(), small.clone(), p.text2);
                }
                if !tc.output.is_empty() {
                    let out = bounded(&tc.output);
                    let fr = egui::Frame::NONE.fill(p.code_bg).corner_radius(4).inner_margin(Margin::symmetric(8, 6)).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        sc.text(ui, &out, theme::mono(size::MONO - 1.0), p.text2);
                    });
                    markdown::copy_button(ui, &p, fr.response.rect, &tc.output, ui.scope_id().with(("out-copy", &item.id)));
                }
                if tc.output_truncated {
                    sc.text(ui, &format!("Output is longer ({} bytes); the rest is on the target.", tc.output_bytes.unwrap_or(0)), small.clone(), p.text2);
                }
                if let Some(c) = tc.exit_code {
                    let (label, color) = if c == 0 { ("Success".to_owned(), p.done) }
                        else { (format!("Failed (exit {c})"), p.failed) };
                    sc.text(ui, &label, small.clone(), color);
                }
                tool_images(ui, v.images, key, &tc.images, actions);
            }
            ItemBody::MeshSent(s) => {
                let gone = s.receipt.status == ReceiptStatus::Failed
                    && matches!(s.receipt.reason, Some(ReceiptReason::ThreadDeleted | ReceiptReason::UnknownAddress));
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    sc.text(ui, "To", small.clone(), p.text2);
                    peer_link(ui, &p, v.state, &s.to, Some(&s.message.id), gone, actions);
                    sc.text(ui, &peer_rest(&s.to), small.clone(), p.text2);
                });
                sc.job(ui, markdown::inline_job(&s.message.text, &small, p.text2, &p, ui.available_width()), p.text2);
                let target = s.to.target_name.clone().unwrap_or_default();
                let mut receipt = yaac_core::header::receipt_text(&s.receipt.status, s.receipt.reason.as_ref(), s.receipt.detail.as_deref(), &target);
                if s.receipt.status == ReceiptStatus::Delivered
                    && s.receipt.detail.is_none()
                    && let Some(ago) = yaac_core::time::ago(now, &s.message.sent_at)
                {
                    receipt.push_str(&format!(" · {ago}"));
                }
                let c = if s.receipt.status == ReceiptStatus::Failed { p.failed } else { p.text2 };
                ui.horizontal(|ui| {
                    let m = match s.receipt.status {
                        ReceiptStatus::Delivered => Mark::Done,
                        ReceiptStatus::Failed => Mark::Failed,
                        _ => Mark::Waiting,
                    };
                    widgets::mark(ui, &p, m, 12.0);
                    sc.text(ui, &receipt, small.clone(), c);
                });
            }
            ItemBody::MeshReceived(r) => {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    sc.text(ui, "From", small.clone(), p.text2);
                    peer_link(ui, &p, v.state, &r.from, Some(&r.message.id), false, actions);
                    sc.text(ui, &peer_rest(&r.from), small.clone(), p.text2);
                });
                sc.job(ui, markdown::inline_job(&r.message.text, &small, p.text2, &p, ui.available_width()), p.text2);
                if let Some(ago) = yaac_core::time::ago(now, &r.message.sent_at) {
                    sc.text(ui, &format!("Sent {ago}"), small.clone(), p.text2);
                }
            }
            ItemBody::MeshSpawn(sp) => {
                let map = AttentionMap::compute(v.state);
                for th in &sp.threads {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        let k = v.state.key_of(&th.peer.address);
                        let row = k.as_ref().and_then(|k| yaac_core::agents::row_for(v.state, &map, k, 0));
                        let mark = row.as_ref().map_or(Mark::Idle, |r| r.display.mark);
                        widgets::mark(ui, &p, mark, 12.0);
                        peer_link(ui, &p, v.state, &th.peer, None, false, actions);
                        let model = th.peer.model.as_ref().map(ToString::to_string).unwrap_or_default();
                        let activity = row.map(|r| r.activity).unwrap_or_default();
                        sc.text(ui, &format!("{} · {model} · {activity}", peer_rest(&th.peer).trim_start_matches("· ")), small.clone(), p.text2);
                    });
                }
            }
            ItemBody::MeshWait(w) => {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    sc.text(ui, if w.for_messages { "Ends on any message, or when these finish:" } else { "Waiting for:" }, small.clone(), p.text2);
                    for target in &w.targets {
                        peer_link(ui, &p, v.state, &target.peer, None, false, actions);
                    }
                });
            }
            ItemBody::Notice(n) => {
                sc.text(ui, &bounded(n.body.as_deref().unwrap_or(&n.text)), small.clone(), p.text2);
            }
            ItemBody::Plan(pl) => {
                for s in &pl.steps {
                    ui.horizontal(|ui| {
                        let m = if s.state == yaac_protocol::PlanStepState::Completed { Mark::Done } else { Mark::Idle };
                        widgets::mark(ui, &p, m, 11.0);
                        sc.text(ui, &s.text, small.clone(), p.text2);
                    });
                }
            }
            _ => {}
        }
    });
    // A guide down the open body, under the line's chevron: clicking it (or the line) closes
    // it, so the way back is in reach however long the output is (D158).
    let r = body.response.rect;
    let x = r.left() + 9.0;
    let guide = egui::Rect::from_min_max(pos2(r.left(), r.top()), pos2(r.left() + 18.0, r.bottom() - 4.0));
    let g = ui.interact(guide, ui.scope_id().with(("collapse-guide", &l.id)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Collapse");
    let stroke = egui::Stroke::new(if g.hovered() { 2.0 } else { 1.0 }, if g.hovered() { p.text2 } else { p.border });
    ui.painter().vline(x, (r.top() + 2.0)..=(r.bottom() - 6.0), stroke);
    if g.clicked() {
        if thought.is_some() {
            v.ui.expansion.toggle_thought(key, &l.id);
        } else {
            v.ui.expansion.toggle_line(key, &l.id);
        }
    }
}

// ── Agent calls (D150) ──────────────────────────────────────────────────────────────────

const PILL_H: f32 = 18.0;

/// A pill's width at `font`: logo, label, and its machine.
fn pill_size(ui: &egui::Ui, pill: &Pill, font: &egui::FontId, p: &Palette) -> (f32, std::sync::Arc<egui::Galley>, Option<std::sync::Arc<egui::Galley>>) {
    let label = ui.fonts_mut(|f| f.layout_no_wrap(pill.label.clone(), font.clone(), p.text));
    let machine = pill.machine.as_ref().map(|m| ui.fonts_mut(|f| f.layout_no_wrap(format!("· {m}"), font.clone(), p.text2)));
    let logo = if pill.kind.is_some() { 17.0 } else { 0.0 };
    let w = 8.0 + logo + label.size().x + machine.as_ref().map_or(0.0, |g| g.size().x + 4.0) + 8.0;
    (w, label, machine)
}

/// Draws one pill with its left edge at `x` on the row centred at `cy`: the harness logo, the
/// title, the machine; a pill that opens a thread is a link (hover fill, pointing hand), one
/// that does not is drawn the same, muted. Returns its width.
#[allow(clippy::too_many_arguments)]
fn draw_pill(ui: &mut egui::Ui, sc: &mut SelCx<'_>, p: &Palette, pill: &Pill, x: f32, cy: f32, font: &egui::FontId, id: egui::Id, actions: &mut Vec<Action>) -> f32 {
    let (w, label, machine) = pill_size(ui, pill, font, p);
    let rect = egui::Rect::from_min_size(pos2(x, cy - PILL_H / 2.0), vec2(w, PILL_H));
    #[cfg(test)]
    ui.ctx().data_mut(|d| d.insert_temp(egui::Id::unique(("test-pill", pill.label.as_str())), rect));
    let hovered = pill.key.is_some() && ui.rect_contains_pointer(rect);
    let fill = if hovered { p.selected } else { p.faint };
    ui.painter().rect_filled(rect, PILL_H / 2.0, fill);
    let mut tx = rect.left() + 8.0;
    if let Some(kind) = &pill.kind {
        crate::agents_ui::harness_mark(ui.painter(), pos2(tx + 6.0, cy), 13.0, Some(kind), None, false, p);
        tx += 17.0;
    }
    let ty = cy - label.size().y / 2.0;
    let lw = label.size().x;
    sc.paint(ui, pos2(tx, ty), label, if pill.key.is_some() { p.text } else { p.text2 });
    if let Some(m) = machine {
        sc.paint(ui, pos2(tx + lw + 4.0, ty), m, p.text2);
    }
    if let Some(k) = &pill.key {
        let resp = ui.interact(rect, id, Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
        if resp.clicked() {
            actions.push(Action::OpenAt(k.clone(), pill.at.clone()));
        }
    }
    w
}

/// An agent call's line: its mark, what it did, the other agents as pills, the message as a
/// quoted preview, and a quiet status at the end. A click on the row (outside a pill) expands
/// it; a pill opens its thread.
fn call_row(ui: &mut egui::Ui, sc: &mut SelCx<'_>, p: &Palette, l: &ActivityLine, call: &CallLine, lead: &str, actions: &mut Vec<Action>) -> egui::Response {
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 24.0), Sense::click());
    let mut row = ui.new_child(UiBuilder::new().max_rect(rect).id_salt(("call-row", &l.id)));
    row.set_clip_rect(ui.clip_rect().intersect(rect));
    let ui = &mut row;
    if l.highlighted {
        ui.painter().rect_filled(rect, 4.0, p.selected);
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, 4.0, p.faint);
    }
    let color = p.text2;
    let c = pos2(rect.left() + 9.0, rect.center().y);
    {
        let pa = ui.painter();
        match l.mark {
            _ if l.failed => icons::mark(pa, c, 12.0, Mark::Failed, p),
            LineMark::Sent => icons::arrow_out(pa, c, 12.0, color),
            LineMark::Received => icons::arrow_in(pa, c, 12.0, color),
            LineMark::Spawn => icons::branch(pa, c, 12.0, color),
            LineMark::Waiting => icons::hourglass(pa, c, 11.0, color),
            LineMark::Resumed => icons::refresh(pa, c, 11.0, color),
            _ => {
                pa.circle_filled(c, 2.0, color);
            }
        }
    }
    let font = theme::ui(size::SMALL + 0.5);
    let cy = rect.center().y;
    let right = rect.right() - 8.0;
    let mut x = rect.left() + 22.0;
    let text_at = |ui: &mut egui::Ui, sc: &mut SelCx<'_>, x: &mut f32, s: &str, col: egui::Color32, limit: f32| {
        let max = limit - *x;
        if s.is_empty() || max < 10.0 {
            return;
        }
        let g = crate::widgets::elided(ui, s, &font, col, max);
        let w = g.size().x;
        sc.paint(ui, pos2(*x, cy - g.size().y / 2.0), g, col);
        *x += w + 6.0;
    };
    text_at(ui, sc, &mut x, lead, color, right);
    let tail = if l.failed {
        "failed".to_owned()
    } else {
        call.meta.iter().chain(call.status.iter()).map(String::as_str).collect::<Vec<_>>().join(" · ")
    };
    // Cap trailing text when it shares the row with pills or a preview.
    let tail_max = if call.pills.is_empty() && call.after.is_empty() && call.preview.is_none() {
        right - x
    } else {
        (right - x) / 2.0
    };
    let tail_g = (!tail.is_empty() && tail_max >= 10.0).then(|| {
        crate::widgets::elided(ui, &format!("· {tail}"), &font, color, tail_max)
    });
    let content_right = right - tail_g.as_ref().map_or(0.0, |g| g.size().x + 6.0);
    for (n, pill) in call.pills.iter().enumerate() {
        let (w, _, _) = pill_size(ui, pill, &font, p);
        if x + w > content_right {
            text_at(ui, sc, &mut x, &format!("+{}", call.pills.len() - n), color, content_right);
            break;
        }
        x += draw_pill(ui, sc, p, pill, x, cy, &font, ui.scope_id().with(("pill", &l.id, n)), actions) + 5.0;
    }
    text_at(ui, sc, &mut x, &call.after, color, content_right);
    if let Some(pv) = &call.preview {
        text_at(ui, sc, &mut x, &format!("\u{201c}{pv}\u{201d}"), p.text2, content_right);
    }
    if let Some(g) = tail_g {
        let gy = cy - g.size().y / 2.0;
        sc.paint(ui, pos2(x, gy), g, color);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn failed_call_label(tc: &yaac_protocol::ToolCallItem) -> &str {
    tc.input.as_ref().and_then(|input| {
        ["tool", "name"].iter().filter_map(|key| input.get(key)?.as_str()).find(|name| !name.trim().is_empty())
    }).map_or(&tc.title, |name| {
        name.strip_prefix("mcp__").and_then(|name| name.split_once("__")).map_or(name, |(_, tool)| tool)
    })
}

/// An agent call expanded: who it was with (pills), the message or task in full, the receipt
/// or reason, what came back, and a "Raw call" disclosure with the original input and output.
/// Returns true when "Raw call" was clicked.
#[allow(clippy::too_many_arguments)]
fn call_detail(
    ui: &mut egui::Ui,
    v: &mut View<'_>,
    unit: Unit,
    t: &Transcript,
    item: &yaac_protocol::Item,
    tc: &yaac_protocol::ToolCallItem,
    ac: &yaac_protocol::AgentCall,
    call: &CallLine,
    raw_open: bool,
    actions: &mut Vec<Action>,
) -> bool {
    let p = v.p;
    let d = yaac_core::agentcall::detail(Some(t), tc, ac);
    let small = theme::ui(size::SMALL + 0.5);
    ui.spacing_mut().item_spacing.y = 4.0;
    if !call.pills.is_empty() {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), PILL_H + 4.0), Sense::hover());
        let cy = rect.center().y;
        let mut sc = v.sel(unit);
        let g = ui.fonts_mut(|f| f.layout_no_wrap(d.heading.clone(), small.clone(), p.text2));
        let mut x = rect.left() + g.size().x + 6.0;
        sc.paint(ui, pos2(rect.left(), cy - g.size().y / 2.0), g, p.text2);
        for (n, pill) in call.pills.iter().enumerate() {
            x += draw_pill(ui, &mut sc, &p, pill, x, cy, &small, ui.scope_id().with(("detail-pill", &item.id, n)), actions) + 5.0;
        }
    }
    let md = |v: &mut View<'_>, ui: &mut egui::Ui, suffix: &str, text: &str, color: egui::Color32| {
        let id = yaac_protocol::ItemId::new(format!("{}#{suffix}", item.id));
        let parsed = v.md.get(&id, text, true).clone();
        let refs: Vec<&markdown::Block> = parsed.blocks().collect();
        markdown::show(ui, &mut v.sel(unit), &refs, &p, color, size::SMALL + 1.0);
    };
    if let Some(text) = &d.text {
        md(v, ui, "call-text", text, p.text);
    }
    let failed = l_failed(item, ac);
    let status = d.status.as_deref().map(str::trim).filter(|status| !status.is_empty())
        .or_else(|| failed.then(|| first_nonempty_line(&d.raw_output)).flatten());
    if let Some(status) = status {
        let c = if failed { p.failed } else { p.text2 };
        v.sel(unit).text(ui, status, small.clone(), c);
    }
    if let Some(result) = &d.result
        && !(failed && status.is_some_and(|status| {
            result.trim() == status || (result.trim() == d.raw_output.trim() && first_nonempty_line(result) == Some(status))
        }))
    {
        ui.add_space(2.0);
        v.sel(unit).text(ui, "Result", theme::bold(size::SMALL), p.text2);
        md(v, ui, "call-result", &bounded(result), p.text);
    }
    ui.add_space(2.0);
    let clicked = widgets::disclosure_row(ui, &p, "Raw call", raw_open, ui.scope_id().with(("raw", &item.id)));
    if raw_open {
        for (label, body) in [("Input", &d.raw_input), ("Output", &d.raw_output)] {
            if body.is_empty() {
                continue;
            }
            v.sel(unit).text(ui, label, small.clone(), p.text2);
            let mut sc = v.sel(unit);
            let fr = egui::Frame::NONE.fill(p.code_bg).corner_radius(4).inner_margin(Margin::symmetric(8, 6)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                sc.text(ui, body, theme::mono(size::MONO - 1.0), p.text2);
            });
            markdown::copy_button(ui, &p, fr.response.rect, body, ui.scope_id().with(("raw-copy", &item.id, label)));
        }
        if tc.output_truncated {
            let notice = tc.output_bytes.map_or_else(
                || "Output is longer. The rest is on the target.".to_owned(),
                |bytes| format!("Output is longer ({bytes} bytes). The rest is on the target."),
            );
            v.sel(unit).text(ui, &notice, small, p.text2);
        }
    }
    clicked
}

fn tool_images(ui: &mut egui::Ui, images: &mut crate::images::Images, key: &ThreadKey, entries: &[yaac_protocol::ToolImage], actions: &mut Vec<Action>) {
    for (index, image) in entries.iter().enumerate() {
        let source = crate::images::ImageSource::Target(key.target.clone(), image.upload_id.clone());
        if ui.push_id(index, |ui| images.inline(ui, &source, &image.name)).inner {
            actions.push(Action::OpenImage(source, image.name.clone()));
        }
    }
}

fn first_nonempty_line(text: &str) -> Option<&str> {
    text.lines().map(str::trim).find(|line| !line.is_empty())
}

fn l_failed(item: &yaac_protocol::Item, ac: &yaac_protocol::AgentCall) -> bool {
    item.status == yaac_protocol::ItemStatus::Failed || ac.outcome == Some(yaac_protocol::AgentCallOutcome::Failed)
}

/// One file's unified diff, expanded in place: removed lines on a faint red ground, added on a
/// faint green one, context between, old and new line numbers in secondary text, its own copy
/// button, and "show the rest" for a long diff. Returns true when "show the rest" was chosen.
fn diff_view(ui: &mut egui::Ui, sc: &mut SelCx<'_>, p: &Palette, f: &FileDiff, rest_open: bool, id: egui::Id) -> bool {
    let font = theme::mono(size::MONO - 1.0);
    let row_h = (size::MONO - 1.0) * 1.6;
    let (shown, hidden) = yaac_core::diff::fold(f, rest_open);
    let mut open_rest = false;
    let max_no = f.hunks.iter().flat_map(|h| &h.lines).filter_map(|l| l.old.max(l.new)).max().unwrap_or(1);
    let digits = max_no.to_string().len().max(2);
    let num_w = ui.fonts_mut(|fo| fo.layout_no_wrap("0".repeat(digits), font.clone(), p.text2)).size().x;
    let fr = egui::Frame::NONE.fill(p.code_bg).corner_radius(5).inner_margin(Margin { left: 0, right: 0, top: 6, bottom: 6 }).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 0.0;
        if f.hunks.is_empty() {
            ui.horizontal(|ui| {
                ui.add_space(12.0);
                sc.text(ui, "Renamed without changes.", theme::ui(size::SMALL), p.text2);
            });
        }
        for h in &f.hunks[..shown] {
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::hover());
            let g = ui.fonts_mut(|fo| fo.layout_no_wrap(h.header.clone(), font.clone(), p.text2));
            sc.paint(ui, pos2(r.left() + 12.0 + 2.0 * (num_w + 10.0), r.center().y - g.size().y / 2.0), g, p.text2);
            for l in &h.lines {
                let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::hover());
                let (ground, sign, color) = match l.kind {
                    LineKind::Added => (Some(p.diff_ground(true)), "+", p.text),
                    LineKind::Removed => (Some(p.diff_ground(false)), "-", p.text),
                    LineKind::Context => (None, " ", p.text),
                    LineKind::NoNewline => (None, "", p.text2),
                };
                if let Some(c) = ground {
                    ui.painter().rect_filled(r, 0.0, c);
                }
                let pa = ui.painter();
                let cy = r.center().y;
                for (n, x) in [(l.old, r.left() + 12.0), (l.new, r.left() + 12.0 + num_w + 10.0)] {
                    if let Some(n) = n {
                        let g = ui.fonts_mut(|fo| fo.layout_no_wrap(format!("{n:>digits$}"), font.clone(), p.text2));
                        pa.galley(pos2(x, cy - g.size().y / 2.0), g, p.text2);
                    }
                }
                let x = r.left() + 12.0 + 2.0 * (num_w + 10.0);
                let g = crate::widgets::elided(ui, &format!("{sign}{}", l.text), &font, color, r.right() - x - 8.0);
                sc.paint(ui, pos2(x, cy - g.size().y / 2.0), g, color);
            }
        }
        if hidden > 0 {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.add_space(12.0 + 2.0 * (num_w + 10.0));
                let text = format!("Show the rest ({hidden} more lines)");
                if widgets::link(ui, p, &text, theme::ui(size::SMALL)).clicked() {
                    open_rest = true;
                }
            });
        }
    });
    markdown::copy_button(ui, p, fr.response.rect, &f.text, id.with("copy"));
    open_rest
}

/// Parsed diffs of file-change items, kept while their output is unchanged.
#[derive(Debug, Default)]
pub(crate) struct DiffCache {
    items: std::collections::HashMap<yaac_protocol::ItemId, (usize, std::rc::Rc<Vec<FileDiff>>)>,
}

impl DiffCache {
    pub(crate) fn get(&mut self, id: &yaac_protocol::ItemId, output: &str) -> std::rc::Rc<Vec<FileDiff>> {
        if let Some((len, d)) = self.items.get(id)
            && *len == output.len()
        {
            return d.clone();
        }
        let d = std::rc::Rc::new(yaac_core::diff::parse(output).unwrap_or_default());
        self.items.insert(id.clone(), (output.len(), d.clone()));
        d
    }
}

/// A bounded head and tail of a large output.
fn bounded(s: &str) -> String {
    const HEAD: usize = 3000;
    const TAIL: usize = 1500;
    if s.len() <= HEAD + TAIL {
        return s.to_owned();
    }
    let mut h = HEAD;
    while !s.is_char_boundary(h) {
        h -= 1;
    }
    let mut t = s.len() - TAIL;
    while !s.is_char_boundary(t) {
        t += 1;
    }
    format!("{}\n… {} bytes not shown …\n{}", &s[..h], t - h, &s[t..])
}

#[cfg(test)]
mod tests {
    use super::*;
    use yaac_core::demo::{Scene, threads};

    #[test]
    fn a_single_typed_part_keeps_line_breaks_when_the_bubble_is_measured() {
        let text = "First line\nSecond line\nThird line";
        let item = serde_json::from_value(serde_json::json!({
            "id": "typed-lines", "turnId": "turn", "status": "completed", "kind": "userMessage",
            "parts": [{ "type": "text", "text": text }],
        })).expect("a user message");
        let mut transcript = Transcript::new();
        transcript.upsert(item, 1);
        let ctx = egui::Context::default();
        let mut app = crate::app::tests::demo_app(&ctx);
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let view = View { p: Palette::DARK, state: &app.client.state, ui: &mut app.ui, md: &mut app.md, images: &mut app.images };
            render_block(ui, view, &threads::users(), &transcript, &Block::User { index: 0 }, Unit::new(0, 0), 0, &mut Vec::new());
        });
        output.textures_delta.clear();
        assert!(output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(line)
            if line.galley.text() == text && line.galley.rows.len() == 3)));
    }

    #[test]
    fn failed_agent_call_keeps_the_tool_label_and_wraps_one_error_summary() {
        let reason = "The directory service could not finish this request. ".repeat(20);
        let raw_output = format!("{}\n{}", reason.trim(), "More diagnostic information\n".repeat(160));
        let item: yaac_protocol::Item = serde_json::from_value(serde_json::json!({
            "id": "failed-call",
            "turnId": "turn",
            "status": "failed",
            "kind": "toolCall",
            "tool": "mcp",
            "title": "Looked up agents",
            "input": { "tool": "list_workers", "arguments": { "scope": "project" } },
            "output": raw_output,
            "agentCall": { "kind": "list", "outcome": "failed", "detail": reason, "result": raw_output },
        })).expect("a failed call");
        let ctx = egui::Context::default();
        let mut app = crate::app::tests::demo_app(&ctx);
        let key = threads::orchestrator();
        let mut t = Transcript::new();
        t.upsert(item.clone(), 1);
        let map = AttentionMap::compute(&app.client.state);
        let mut line = yaac_core::activity::line_of(&app.client.state, &map, &key, 0, &item).expect("activity line");
        let mut draw = |line: &ActivityLine, width, raw| {
            if raw {
                app.ui.expansion.toggle_raw(&key, &item.id);
            }
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(width, 2000.0))), ..Default::default() };
            let mut output = ctx.run_ui(input, |ui| {
                let mut view = View { p: Palette::DARK, state: &app.client.state, ui: &mut app.ui, md: &mut app.md, images: &mut app.images };
                activity_line(ui, &mut view, &key, &t, line, 0, 0.0, &mut Vec::new());
            });
            output.textures_delta.clear();
            output.shapes.into_iter().filter_map(|shape| {
                let egui::Shape::Text(text) = shape.shape else { return None };
                Some((text, shape.clip_rect))
            }).collect::<Vec<_>>()
        };
        for width in [132.0, 360.0] {
            let closed = draw(&line, width, false);
            let label = closed.iter().find(|(text, _)| text.galley.text() == "list_workers").expect("tool label");
            assert!(!label.0.galley.elided);
            assert!(closed.iter().all(|(text, clip)| text.pos.x >= clip.left() + 22.0 && text.pos.x + text.galley.size().x <= clip.right()));
            assert!(!closed.iter().any(|(text, _)| text.galley.text().contains(reason.trim())));
        }
        line.expanded = true;
        let expanded = draw(&line, 360.0, false);
        let summaries: Vec<_> = expanded.iter().filter(|(text, _)| text.galley.text().contains(reason.trim())).collect();
        assert_eq!(summaries.len(), 1);
        assert!(summaries[0].0.galley.rows.len() > 1);
        let raw = draw(&line, 360.0, true);
        assert!(raw.iter().any(|(text, _)| text.galley.text() == raw_output));
        assert!(raw.iter().any(|(text, _)| text.galley.text().contains("\"scope\": \"project\"")));
    }

    #[test]
    fn the_composer_column_is_centered_at_the_chat_width() {
        let ctx = egui::Context::default();
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 600.0))), ..Default::default() };
        let mut output = ctx.run_ui(input, |ui| {
            let viewport = ui.available_rect_before_wrap();
            for preferred in [100.0, 620.0, 2000.0] {
                let width = chat_width(viewport.width(), preferred);
                composer_column(ui, preferred, &Palette::DARK, |ui| {
                    assert!((ui.available_width() - width).abs() < 1.0);
                    let left = viewport.left() + (viewport.width() - width) * 0.5;
                    assert!((ui.max_rect().left() - left).abs() < 1.0);
                    ui.label("Composer");
                });
            }
        });
        output.textures_delta.clear();
    }

    /// The users-service transcript of screens 76–77, drawn once; returns each user block's
    /// measured height by item id.
    fn draw(app: &mut YaacApp, ctx: &egui::Context) -> std::collections::HashMap<String, f32> {
        let key = threads::users();
        let mut actions = Vec::new();
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 4000.0))), ..Default::default() };
        let mut out = ctx.run_ui(input, |ui| {
            let map = AttentionMap::compute(&app.client.state);
            transcript(ui, app, &map, &key, 0, &mut actions);
        });
        out.textures_delta.clear();
        let t = &app.client.state.transcripts[&key];
        let bl = blocks(&app.client.state, &AttentionMap::compute(&app.client.state), &key, &app.ui.expansion);
        bl.iter()
            .filter_map(|b| match b {
                Block::User { index } => {
                    let h = block_ident(b, t);
                    app.ui.heights.get(&h).map(|v| (t.items()[*index].id.to_string(), *v))
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_paste_opens_in_place_context_is_hidden_and_copy_leaves_it_out() {
        let ctx = egui::Context::default();
        let mut app = crate::app::tests::demo_app(&ctx);
        if let Some(h) = &app.demo {
            h.play(Scene::HarnessMarkup);
            h.drain();
        }
        app.pump_demo();
        let key = threads::users();
        app.open(&key, None);
        let t = &app.client.state.transcripts[&key];
        let um1 = t.items().iter().find(|i| i.id.as_str() == "um1").expect("um1").clone();
        let um3 = t.items().iter().find(|i| i.id.as_str() == "um3").expect("um3").clone();
        // Copy message: typed text and the paste, never the reminder; a command copies as typed.
        let copied = message_text(&um1).expect("text");
        assert!(copied.starts_with("The migration drops the last batch. Here is the log:\n\nINFO  migrate: batch 1/4"));
        assert!(copied.ends_with("Why does the retry skip it?"));
        assert!(!copied.contains("phone"), "context is not copied");
        assert_eq!(message_text(&um3).as_deref(), Some("/model opus-5.5"));
        // Collapsed, the 12-line paste is one row; opened, it is taller by its lines; the
        // context opens under it too.
        let closed = draw(&mut app, &ctx)["um1"];
        app.ui.expansion.toggle_pasted(&key, &"um1".into(), 1);
        let pasted = draw(&mut app, &ctx)["um1"];
        assert!(pasted > closed + 150.0, "{closed} → {pasted}");
        app.ui.expansion.toggle_context(&key, &"um1".into());
        let both = draw(&mut app, &ctx)["um1"];
        assert!(both > pasted + 20.0, "{pasted} → {both}");
        app.ui.expansion.toggle_pasted(&key, &"um1".into(), 1);
        app.ui.expansion.toggle_context(&key, &"um1".into());
        assert!((draw(&mut app, &ctx)["um1"] - closed).abs() < 0.5);
    }

    #[test]
    fn the_menu_copy_copies_the_selection() {
        let ctx = egui::Context::default();
        let mut app = crate::app::tests::demo_app(&ctx);
        if let Some(h) = &app.demo {
            h.play(Scene::HarnessMarkup);
            h.drain();
        }
        app.pump_demo();
        let key = threads::users();
        app.open(&key, None);
        let frame = |app: &mut YaacApp, events: Vec<egui::Event>| {
            let mut actions = Vec::new();
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 4000.0))), events, ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| {
                let map = AttentionMap::compute(&app.client.state);
                transcript(ui, app, &map, &key, 0, &mut actions);
            });
            out.textures_delta.clear();
            out.platform_output.commands.iter().find_map(|c| match c {
                egui::OutputCommand::CopyText(s) => Some(s.clone()),
                _ => None,
            })
        };
        for _ in 0..3 {
            frame(&mut app, Vec::new());
        }
        let t = &app.client.state.transcripts[&key];
        let index = t.items().iter().position(|i| i.id.as_str() == "um1").expect("um1");
        let unit = Unit::new(index, 0);
        let r = app.ui.textsel.drawn(unit).expect("um1 is drawn");
        let button = |pos, button, pressed| egui::Event::PointerButton { pos, button, pressed, modifiers: egui::Modifiers::NONE };
        // Drag across part of the first line.
        let (a, b) = (pos2(r.left() + 1.0, r.top() + 6.0), pos2(r.left() + 120.0, r.top() + 6.0));
        frame(&mut app, vec![egui::Event::PointerMoved(a), button(a, egui::PointerButton::Primary, true)]);
        frame(&mut app, vec![egui::Event::PointerMoved(b)]);
        frame(&mut app, vec![button(b, egui::PointerButton::Primary, false)]);
        let sel = app.ui.textsel.selection().expect("a selection");
        let want: String = "The migration drops the last batch.".chars().take(sel.focus.ch).collect();
        assert!(want.len() > 5, "{want:?}");
        // Right-click on it: the message menu opens over the transcript.
        frame(&mut app, vec![button(b, egui::PointerButton::Secondary, true)]);
        frame(&mut app, vec![button(b, egui::PointerButton::Secondary, false)]);
        frame(&mut app, Vec::new());
        assert!(app.ui.textsel.selection().is_some(), "a right-click keeps the selection");
        let menu = ctx
            .memory(|m| m.layer_ids().filter(|l| l.order == egui::Order::Foreground).find_map(|l| m.area_rect(l.id)))
            .expect("the menu is open");
        // Its first row is "Copy".
        let at = pos2(menu.center().x, menu.top() + 4.0 + 13.0);
        frame(&mut app, vec![egui::Event::PointerMoved(at)]);
        let pressed = frame(&mut app, vec![button(at, egui::PointerButton::Primary, true)]);
        let released = frame(&mut app, vec![button(at, egui::PointerButton::Primary, false)]);
        assert_eq!(pressed.or(released).as_deref(), Some(want.as_str()), "Copy copies the selection");
    }

    #[test]
    fn clicking_a_pill_opens_that_thread_and_clicking_the_line_expands_it() {
        let ctx = egui::Context::default();
        let mut app = crate::app::tests::demo_app(&ctx);
        if let Some(h) = &app.demo {
            h.play(Scene::AgentCalls);
            h.drain();
        }
        app.pump_demo();
        let key = threads::orchestrator();
        app.open(&key, None);
        let frame = |app: &mut YaacApp, events: Vec<egui::Event>| {
            let mut actions = Vec::new();
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 4000.0))), events, ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| {
                let map = AttentionMap::compute(&app.client.state);
                transcript(ui, app, &map, &key, 0, &mut actions);
            });
            out.textures_delta.clear();
            actions
        };
        for _ in 0..3 {
            frame(&mut app, Vec::new());
        }
        let rect: Rect = ctx.data(|d| d.get_temp(egui::Id::unique(("test-pill", "billing-service")))).expect("the billing-service pill was drawn");
        let at = rect.center();
        let click = |at: egui::Pos2| {
            vec![
                egui::Event::PointerMoved(at),
                egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE },
                egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE },
            ]
        };
        frame(&mut app, vec![egui::Event::PointerMoved(at)]);
        let actions = frame(&mut app, click(at));
        assert!(
            actions.iter().any(|a| matches!(a, Action::OpenAt(k, _) if *k == threads::billing())),
            "the pill opens billing-service"
        );
        // The last billing-service pill drawn is the wait line's ("reported back").
        assert!(!app.ui.expansion.lines.contains(&(key.clone(), "ac5".into())), "a pill click does not expand the line");
        // Beside the pill, the row expands; expanded, the full message and "Raw call" show.
        let beside = pos2(rect.right() + 200.0, at.y);
        frame(&mut app, vec![egui::Event::PointerMoved(beside)]);
        frame(&mut app, click(beside));
        assert!(app.ui.expansion.lines.contains(&(key.clone(), "ac5".into())), "the line opens");
    }
}
