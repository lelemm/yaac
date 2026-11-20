//! The thread view: header, banners, and the virtualised transcript.

use std::hash::{Hash, Hasher};

use eframe::egui::{self, Margin, Panel, Rect, Sense, UiBuilder, pos2, vec2};
use yaac_core::activity::{ActivityLine, Block, LineMark, blocks};
use yaac_core::attention::{AttentionMap, Mark};
use yaac_core::header::{Banner, LinkLine, Tone, header};
use yaac_core::transcript::Transcript;
use yaac_core::ThreadKey;
use yaac_protocol::{InputPart, ItemBody, ReceiptReason, ReceiptStatus};

use crate::app::{Action, Menu, YaacApp};
use crate::icons;
use crate::markdown;
use crate::theme::{self, Palette, size};
use crate::widgets;

const COLUMN: f32 = 860.0;

pub(crate) fn show(ui: &mut egui::Ui, app: &mut YaacApp, map: &AttentionMap, key: &ThreadKey, now: i64, actions: &mut Vec<Action>) {
    let p = app.p;
    Panel::top("thread-header")
        .show_separator_line(false)
        .frame(egui::Frame::NONE.fill(p.bg).inner_margin(Margin { left: 28, right: 28, top: 14, bottom: 6 }))
        .show(ui, |ui| header_view(ui, app, map, key, actions));
    Panel::bottom("thread-composer")
        .show_separator_line(false)
        .frame(egui::Frame::NONE.fill(p.bg).inner_margin(Margin { left: 28, right: 28, top: 6, bottom: 10 }))
        .show(ui, |ui| crate::composer::bottom(ui, app, map, key, actions));
    egui::CentralPanel::no_frame().show(ui, |ui| transcript(ui, app, map, key, now, actions));
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
        widgets::mark(ui, &p, h.display.mark, 22.0);
        if h.display.read_only {
            widgets::icon(ui, 18.0, |pa, c, d| icons::lock(pa, c, d, p.text2));
        }
        ui.add_space(2.0);
        let tag_w = if h.tag.is_some() { 230.0 } else { 0.0 };
        ui.scope(|ui| {
            ui.set_max_width((ui.available_width() - tag_w).max(80.0));
            ui.add(egui::Label::new(egui::RichText::new(&h.title).font(theme::bold(size::TITLE))).truncate());
        });
        if let Some((tag, tone)) = &h.tag {
            ui.add_space(4.0);
            let c = tone_color(&p, *tone);
            egui::Frame::NONE.fill(p.wash(c)).corner_radius(4).inner_margin(Margin::symmetric(7, 2)).show(ui, |ui| {
                ui.label(egui::RichText::new(tag).font(theme::ui(size::SMALL)).color(c));
            });
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
                widgets::icon(ui, 12.0, |pa, c, d| icons::arrow_out(pa, c, d, p.link));
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
                widgets::icon(ui, 12.0, |pa, c, d| icons::arrow_out(pa, c, d, p.link));
                ui.label(widgets::secondary(detail.clone(), &p));
            });
        }
    }
    for b in &h.banners {
        ui.add_space(8.0);
        egui::Frame::NONE.fill(p.faint).corner_radius(6).inner_margin(Margin::symmetric(12, 8)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| match b {
                Banner::InUseElsewhere { text } => {
                    widgets::icon(ui, 16.0, |pa, c, d| icons::info(pa, c, d, p.text2));
                    ui.label(text);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if widgets::primary(ui, &p, "Fork to continue here", None).clicked() {
                            actions.push(Action::OpenFork(key.clone()));
                        }
                    });
                }
                Banner::OlderDaemon { text } => {
                    widgets::icon(ui, 16.0, |pa, c, d| icons::warning(pa, c, d, p.needs, p.faint));
                    ui.label(text);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        widgets::plain(ui, &p, "Update now", None);
                    });
                }
            });
        });
    }
}

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

fn block_hash(b: &Block, t: &Transcript, width: f32, dark: bool) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    std::mem::discriminant(b).hash(&mut h);
    width.to_bits().hash(&mut h);
    dark.hash(&mut h);
    let idx = |i: usize, h: &mut std::collections::hash_map::DefaultHasher| {
        i.hash(h);
        t.item_rev(i).hash(h);
    };
    match b {
        Block::User { index } | Block::Task { index } | Block::Prose { index } | Block::Card { index } | Block::Handoff { index } => idx(*index, &mut h),
        Block::Run { lines } => {
            for l in lines {
                idx(l.index, &mut h);
                l.expanded.hash(&mut h);
                l.highlighted.hash(&mut h);
                l.text.hash(&mut h);
            }
        }
        Block::Folded { first, count, text, .. } => {
            first.hash(&mut h);
            count.hash(&mut h);
            text.hash(&mut h);
        }
    }
    h.finish()
}

fn block_items(b: &Block) -> Vec<usize> {
    match b {
        Block::User { index } | Block::Task { index } | Block::Prose { index } | Block::Card { index } | Block::Handoff { index } => vec![*index],
        Block::Run { lines } => lines.iter().map(|l| l.index).collect(),
        Block::Folded { .. } => vec![],
    }
}

fn transcript(ui: &mut egui::Ui, app: &mut YaacApp, map: &AttentionMap, key: &ThreadKey, now: i64, actions: &mut Vec<Action>) {
    let p = app.p;
    let state = &app.client.state;
    let Some(t) = app.client.state.transcripts.get(key) else { return };
    if !t.is_loaded() && t.items().is_empty() {
        ui.add_space(20.0);
        ui.vertical_centered(|ui| ui.label(widgets::secondary("Loading…", &p)));
        return;
    }
    let bl = blocks(state, map, key, &app.ui.expansion);
    let avail = ui.available_width();
    if std::env::var_os("YAAC_DEBUG_LAYOUT").is_some() {
        eprintln!("transcript rect {:?} avail {avail}", ui.max_rect());
    }
    let width = (avail - 56.0).clamp(200.0, COLUMN);
    let x0 = ui.max_rect().left() + ((avail - width) / 2.0).max(28.0);
    const GAP: f32 = 14.0;
    // Positions from measured (or estimated) heights.
    let hashes: Vec<u64> = bl.iter().map(|b| block_hash(b, t, width, p.dark)).collect();
    let mut tops = Vec::with_capacity(bl.len());
    let mut y = 12.0;
    for (i, b) in bl.iter().enumerate() {
        tops.push(y);
        let est = match b {
            Block::Run { lines } => lines.len() as f32 * 22.0,
            Block::Folded { .. } => 22.0,
            _ => 60.0,
        };
        y += app.ui.heights.get(&hashes[i]).copied().unwrap_or(est) + GAP;
    }
    let total = y + 20.0;
    let mut sa = egui::ScrollArea::vertical().id_salt(("transcript", key.to_string())).auto_shrink([false, false]).stick_to_bottom(true);
    if let Some(item) = app.ui.scroll_to.take() {
        if let Some(i) = t.index_of(&item)
            && let Some(bi) = bl.iter().position(|b| block_items(b).contains(&i))
        {
            sa = sa.vertical_scroll_offset((tops[bi] - 80.0).max(0.0));
        }
    } else if app.ui.scroll_end {
        app.ui.scroll_end = false;
        sa = sa.vertical_scroll_offset(total);
    }
    let mut changed = false;
    let mut actions_local = Vec::new();
    sa.show_viewport(ui, |ui, view| {
        ui.set_height(total);
        let origin = ui.max_rect().top();
        for (i, b) in bl.iter().enumerate() {
            let top = tops[i];
            let est = app.ui.heights.get(&hashes[i]).copied().unwrap_or(60.0);
            if top + est < view.top() - 300.0 || top > view.bottom() + 300.0 {
                continue;
            }
            let rect = Rect::from_min_size(pos2(x0, origin + top), vec2(width, est.max(1.0)));
            let r = ui.scope_builder(UiBuilder::new().max_rect(rect).id_salt(("block", hashes[i])), |ui| {
                ui.set_width(width);
                render_block(ui, View { p, state: &app.client.state, ui: &mut app.ui, md: &mut app.md }, key, t, b, now, &mut actions_local);
            });
            let h = r.response.rect.height();
            if (app.ui.heights.get(&hashes[i]).copied().unwrap_or(-1.0) - h).abs() > 0.5 {
                app.ui.heights.insert(hashes[i], h);
                changed = true;
            }
        }
    });
    actions.extend(actions_local);
    if changed {
        ui.ctx().request_repaint();
    }
}

/// What block rendering needs, borrowed apart from the heights cache.
pub(crate) struct View<'a> {
    pub p: Palette,
    pub state: &'a yaac_core::state::AppState,
    pub ui: &'a mut crate::app::UiState,
    pub md: &'a mut markdown::Cache,
}

fn render_block(ui: &mut egui::Ui, mut v: View<'_>, key: &ThreadKey, t: &Transcript, b: &Block, now: i64, actions: &mut Vec<Action>) {
    let p = v.p;
    match b {
        Block::User { index } => {
            let item = &t.items()[*index];
            let ItemBody::UserMessage(u) = &item.body else { return };
            widgets::faint_frame(&p).inner_margin(Margin::symmetric(14, 10)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                let text: String = u.parts.iter().filter_map(|x| if let InputPart::Text(t) = x { Some(t.text.as_str()) } else { None }).collect::<Vec<_>>().join("\n");
                if !text.is_empty() {
                    let parsed = v.md.get(&item.id, &text, true);
                    let refs: Vec<&markdown::Block> = parsed.blocks().collect();
                    markdown::show(ui, &refs, &p, p.text, size::PROSE);
                }
                let files: Vec<(&yaac_protocol::UploadId, bool)> = u
                    .parts
                    .iter()
                    .filter_map(|x| match x {
                        InputPart::File(f) => Some((&f.upload_id, false)),
                        InputPart::Image(i) => Some((&i.upload_id, true)),
                        _ => None,
                    })
                    .collect();
                if !files.is_empty() {
                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        for (up, image) in files {
                            let gone = t.missing_uploads.contains(up);
                            chip(ui, &p, &upload_name(up.as_str()), image, gone);
                        }
                    });
                }
            });
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
                let parsed = v.md.get(&item.id, &r.message.text, true);
                let refs: Vec<&markdown::Block> = parsed.blocks().collect();
                markdown::show(ui, &refs, &p, p.text, size::PROSE);
            });
        }
        Block::Prose { index } => {
            let item = &t.items()[*index];
            let ItemBody::AgentMessage(a) = &item.body else { return };
            let streaming = item.status == yaac_protocol::ItemStatus::InProgress;
            let parsed = v.md.get(&item.id, &a.text, !streaming);
            let refs: Vec<&markdown::Block> = parsed.blocks().collect();
            markdown::show(ui, &refs, &p, p.text, size::PROSE);
            if streaming {
                // The cursor at the end of streaming prose.
                let (r, _) = ui.allocate_exact_size(vec2(8.0, 18.0), Sense::hover());
                ui.painter().rect_filled(Rect::from_min_size(r.min, vec2(7.0, 17.0)), 1.0, p.text2);
            }
        }
        Block::Card { index } => crate::cards::show(ui, &mut v, key, t, *index, now, actions),
        Block::Handoff { index } => {
            let item = &t.items()[*index];
            let ItemBody::Handoff(h) = &item.body else { return };
            if let Some(text) = &h.transcript {
                let parsed = v.md.get(&item.id, text, true);
                let refs: Vec<&markdown::Block> = parsed.blocks().collect();
                markdown::show(ui, &refs, &p, p.text2, size::PROSE - 1.0);
            }
            ui.add_space(10.0);
            let src = v.state.summary(&ThreadKey::new(key.target.clone(), h.forked_from.clone())).map_or_else(|| h.forked_from.to_string(), |s| s.title.clone());
            let label = format!("Inherited from {src} — read-only above this line");
            let g = ui.fonts_mut(|f| f.layout_no_wrap(label, theme::ui(size::SMALL), p.text2));
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 20.0), Sense::hover());
            let gx = r.center().x - g.size().x / 2.0;
            ui.painter().hline(r.left()..=gx - 10.0, r.center().y, egui::Stroke::new(1.0, p.border));
            ui.painter().hline(gx + g.size().x + 10.0..=r.right(), r.center().y, egui::Stroke::new(1.0, p.border));
            ui.painter().galley(pos2(gx, r.center().y - g.size().y / 2.0), g, p.text2);
        }
        Block::Run { lines } => {
            ui.spacing_mut().item_spacing.y = 0.0;
            for l in lines {
                activity_line(ui, &mut v, key, t, l, now, actions);
            }
        }
        Block::Folded { first, text, failed, .. } => {
            let resp = line_row(ui, &p, if *failed { Some(Mark::Failed) } else { None }, LineMark::Other, text, *failed, false, true);
            if resp.clicked() {
                v.ui.expansion.toggle_run(key, first);
            }
        }
    }
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
                ui.label(widgets::secondary("— no longer on the target", p));
            }
        });
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

#[allow(clippy::too_many_arguments)]
fn line_row(ui: &mut egui::Ui, p: &Palette, state_mark: Option<Mark>, mark: LineMark, text: &str, failed: bool, highlighted: bool, folded: bool) -> egui::Response {
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 22.0), Sense::click());
    if highlighted {
        ui.painter().rect_filled(rect, 4.0, p.selected);
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, 4.0, p.faint);
    }
    let color = if failed { p.failed } else { p.text2 };
    let c = pos2(rect.left() + 9.0, rect.center().y);
    let pa = ui.painter();
    match (state_mark, mark) {
        (Some(m), _) => icons::mark(pa, c, 12.0, m, p),
        (None, LineMark::Sent) => icons::arrow_out(pa, c, 12.0, color),
        (None, LineMark::Received) => icons::arrow_in(pa, c, 12.0, color),
        (None, LineMark::Workers) => icons::people(pa, c, 12.0, color),
        (None, LineMark::Waiting) => icons::clock(pa, c, 11.0, color),
        (None, LineMark::Notice) => icons::info(pa, c, 11.0, color),
        (None, _) if folded => icons::chevron(pa, c, 11.0, false, color),
        (None, _) => {
            pa.circle_filled(c, 2.0, color);
        }
    }
    let g = crate::widgets::elided(ui, text, &theme::ui(size::SMALL + 0.5), color, width - 28.0);
    ui.painter().galley(pos2(rect.left() + 22.0, rect.center().y - g.size().y / 2.0), g, color);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn activity_line(ui: &mut egui::Ui, v: &mut View<'_>, key: &ThreadKey, t: &Transcript, l: &ActivityLine, now: i64, actions: &mut Vec<Action>) {
    let p = v.p;
    let item = &t.items()[l.index];
    let state_mark = if l.failed { Some(Mark::Failed) } else { None };
    let resp = line_row(ui, &p, state_mark, l.mark, &l.text, l.failed, l.highlighted, false);
    if resp.clicked() {
        v.ui.expansion.toggle_line(key, &l.id);
    }
    if !l.expanded {
        return;
    }
    let fill = if l.highlighted { p.selected } else { egui::Color32::TRANSPARENT };
    egui::Frame::NONE.fill(fill).corner_radius(4).inner_margin(Margin { left: 22, right: 8, top: 2, bottom: 8 }).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 3.0;
        let small = theme::ui(size::SMALL + 0.5);
        let muted = |s: &str| egui::RichText::new(s).font(theme::ui(size::SMALL + 0.5)).color(p.text2);
        match &item.body {
            ItemBody::Reasoning(r) => {
                ui.label(muted(&r.reasoning));
            }
            ItemBody::ToolCall(tc) => {
                ui.label(markdown::inline_job(&tc.title, &small, p.text2, &p, ui.available_width()));
                if let Some(cwd) = &tc.cwd {
                    ui.label(muted(cwd.as_str()));
                }
                if !tc.output.is_empty() {
                    let out = bounded(&tc.output);
                    egui::Frame::NONE.fill(p.code_bg).corner_radius(4).inner_margin(Margin::symmetric(8, 6)).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.label(egui::RichText::new(out).font(theme::mono(size::MONO - 1.0)).color(p.text2));
                    });
                }
                if tc.output_truncated {
                    ui.label(muted(&format!("Output is longer ({} bytes); the rest is on the target.", tc.output_bytes.unwrap_or(0))));
                }
                if let Some(c) = tc.exit_code {
                    ui.label(muted(&format!("exit {c}")));
                }
            }
            ItemBody::MeshSent(s) => {
                let gone = s.receipt.status == ReceiptStatus::Failed
                    && matches!(s.receipt.reason, Some(ReceiptReason::ThreadDeleted | ReceiptReason::UnknownAddress));
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    ui.label(muted("To"));
                    peer_link(ui, &p, v.state, &s.to, Some(&s.message.id), gone, actions);
                    ui.label(muted(&peer_rest(&s.to)));
                });
                ui.label(markdown::inline_job(&s.message.text, &small, p.text2, &p, ui.available_width()));
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
                    ui.label(egui::RichText::new(receipt).font(small.clone()).color(c));
                });
            }
            ItemBody::MeshReceived(r) => {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    ui.label(muted("From"));
                    peer_link(ui, &p, v.state, &r.from, Some(&r.message.id), false, actions);
                    ui.label(muted(&peer_rest(&r.from)));
                });
                ui.label(markdown::inline_job(&r.message.text, &small, p.text2, &p, ui.available_width()));
                if let Some(ago) = yaac_core::time::ago(now, &r.message.sent_at) {
                    ui.label(muted(&format!("Sent {ago}")));
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
                        ui.label(muted(&format!("{} · {model} · {activity}", peer_rest(&th.peer).trim_start_matches("· "))));
                    });
                }
            }
            ItemBody::MeshWait(w) => {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    ui.label(muted(if w.for_messages { "Ends on any message, or when these finish:" } else { "Waiting for:" }));
                    for target in &w.targets {
                        peer_link(ui, &p, v.state, &target.peer, None, false, actions);
                    }
                });
            }
            ItemBody::Notice(n) => {
                ui.label(muted(&n.text));
            }
            ItemBody::Plan(pl) => {
                for s in &pl.steps {
                    ui.horizontal(|ui| {
                        let m = if s.state == yaac_protocol::PlanStepState::Completed { Mark::Done } else { Mark::Idle };
                        widgets::mark(ui, &p, m, 11.0);
                        ui.label(muted(&s.text));
                    });
                }
            }
            _ => {}
        }
    });
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
