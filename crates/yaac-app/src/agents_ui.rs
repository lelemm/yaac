//! The agents panel in its three states: a pill floating over the thread's right edge (the
//! default), a card on hover, or docked as a fixed panel. Every state groups the related
//! agents by relation (D118) — *Spawned by*, *Workers*, *In contact with* — from one grouping
//! in core. The card adds Siblings. Arrows show only while a message is in flight,
//! and are static: an idle fleet costs no CPU.

use eframe::egui::{self, Align2, Margin, Rect, Sense, Stroke, pos2, vec2};
use yaac_core::agents::{AgentRow, AgentsPanel, BudgetLine, DockEntry, Flow, Overflow, PanelInput, PanelMode, PillLayout, Relation, card_groups, clusters, docked_clusters, docked_entries, pill, pill_cap, sibling_rows};
use yaac_core::attention::Mark;
use yaac_core::hover::Region;

use crate::app::{Action, Menu, YaacApp};
use crate::icons;
use crate::theme::{self, Palette, size};
use crate::widgets;

#[derive(Debug, Default)]
pub(crate) struct DockPreview {
    owner: Option<yaac_core::ThreadKey>,
    hovered: Option<yaac_core::ThreadKey>,
    due: Option<f64>,
    armed: bool,
}

impl DockPreview {
    pub(crate) fn target(&self, owner: &yaac_core::ThreadKey) -> Option<&yaac_core::ThreadKey> {
        (self.armed && self.owner.as_ref() == Some(owner)).then_some(self.hovered.as_ref()).flatten()
    }

    fn hover(&mut self, owner: Option<yaac_core::ThreadKey>, hovered: Option<yaac_core::ThreadKey>, now: f64) {
        if self.owner != owner {
            *self = Self::default();
            self.owner = owner;
        } else {
            self.advance(now);
        }
        let hovered = hovered.filter(|key| self.owner.as_ref().is_some_and(|owner| key != owner));
        if self.hovered != hovered {
            self.hovered = hovered;
            self.due = (!self.armed && self.hovered.is_some()).then_some(now + 2.0);
        }
    }

    fn advance(&mut self, now: f64) -> Option<yaac_core::ThreadKey> {
        if self.due.is_some_and(|due| now >= due) {
            self.due = None;
            self.armed = true;
            return self.hovered.clone();
        }
        None
    }
}

/// Update only the preview. The normal open-thread path owns navigation and read marks.
pub(crate) fn dock_hover(app: &mut YaacApp, ctx: &egui::Context, inside: bool, hovered: Option<yaac_core::ThreadKey>) {
    let before = app.ui.dock_preview.hovered.clone().filter(|_| app.ui.dock_preview.armed);
    let owner = if inside && app.ui.panel == PanelMode::Docked && !app.ui.settings {
        app.client.state.selected_thread().cloned()
    } else {
        None
    };
    let now = app.started.elapsed().as_secs_f64();
    app.ui.dock_preview.hover(owner, hovered, now);
    if let Some(key) = &app.ui.dock_preview.hovered
        && app.ui.dock_preview.armed && before.as_ref() != Some(key)
    {
        app.client.watch_transcript(key);
    }
    poll_dock_hover(app, ctx);
}

pub(crate) fn poll_dock_hover(app: &mut YaacApp, ctx: &egui::Context) {
    let preview = &mut app.ui.dock_preview;
    if app.ui.settings || app.ui.panel != PanelMode::Docked || preview.owner.as_ref() != app.client.state.selected_thread() {
        *preview = DockPreview::default();
        return;
    }
    let now = app.started.elapsed().as_secs_f64();
    if let Some(key) = preview.advance(now) {
        app.client.watch_transcript(&key);
    }
    if let Some(due) = preview.due {
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(due - now));
    }
}

/// A harness's logo (the table in `logos`), or its initial in a circle; the needs-you mark
/// on its corner, and the unseen ring around it (D114).
fn harness_icon(painter: &egui::Painter, c: egui::Pos2, d: f32, row: &AgentRow, p: &Palette) {
    if row.display.unseen {
        painter.circle_stroke(c, d * 0.62, Stroke::new(1.6, p.unseen));
    }
    harness_mark(painter, c, d, row.kind.as_deref(), row.provider.as_deref(), row.finished, p);
    if row.display.attention.is_some() {
        let corner = c + vec2(d * 0.38, d * 0.32);
        painter.circle_filled(corner, d * 0.3, p.raised);
        icons::exclaim(painter, corner, d * 0.5, p.failed);
    }
}

/// The mark of a provider kind: its logo (in its own colour, or the text colour), dimmed when
/// `dim`; an unknown kind's initial in a circle.
pub(crate) fn harness_mark(painter: &egui::Painter, c: egui::Pos2, d: f32, kind: Option<&str>, provider: Option<&str>, dim: bool, p: &Palette) {
    match kind.and_then(crate::logos::for_kind) {
        Some((logo, own)) => {
            let color = own.unwrap_or(p.text);
            let color = if dim { color.gamma_multiply(0.45) } else { color };
            crate::logos::draw(painter, c, d * 0.9, logo, color);
        }
        None => {
            let color = if dim { p.text2.gamma_multiply(0.6) } else { p.text };
            painter.circle_stroke(c, d * 0.42, Stroke::new(1.4, color));
            let initial = provider.or(kind).and_then(|s| s.chars().next()).unwrap_or('?');
            painter.text(c, Align2::CENTER_CENTER, initial.to_uppercase().to_string(), theme::bold(d * 0.5), color);
        }
    }
}

/// The tiny muted glyph that heads a relation's cluster in the pill (D118).
fn relation_glyph(painter: &egui::Painter, c: egui::Pos2, relation: Relation, p: &Palette) {
    match relation {
        Relation::SpawnedBy => icons::arrow_vertical(painter, c, 11.0, p.text2, true),
        Relation::Workers => icons::branch(painter, c, 11.0, p.text2),
        Relation::InContact => icons::swap(painter, c, 11.0, p.text2),
    }
}

fn arrow(painter: &egui::Painter, c: egui::Pos2, flow: Flow, p: &Palette) {
    let s = 4.0;
    let w = Stroke::new(1.6, p.text);
    match flow {
        Flow::Sending => {
            painter.line_segment([pos2(c.x, c.y + s), pos2(c.x, c.y - s)], w);
            painter.line(vec![pos2(c.x - 3.0, c.y - s + 3.0), pos2(c.x, c.y - s), pos2(c.x + 3.0, c.y - s + 3.0)], w);
        }
        Flow::Receiving => {
            painter.line_segment([pos2(c.x, c.y - s), pos2(c.x, c.y + s)], w);
            painter.line(vec![pos2(c.x - 3.0, c.y + s - 3.0), pos2(c.x, c.y + s), pos2(c.x + 3.0, c.y + s - 3.0)], w);
        }
    }
}

/// The pill's height: icons, a glyph per cluster, a hairline between clusters, and the
/// "more" item under its own hairline (D168).
fn pill_height(pl: &PillLayout<'_>) -> f32 {
    let n = pl.clusters.len();
    let more = if pl.more.hidden > 0 { more_height(&pl.more) + if n > 0 { SEP_H } else { 0.0 } } else { 0.0 };
    pl.icons() as f32 * PILL_ROW + n as f32 * GLYPH_H + n.saturating_sub(1) as f32 * SEP_H + more + 12.0
}

/// The "more" item: "more" over "(23)", and "4 alive" under them when alive agents are hidden.
fn more_height(m: &Overflow) -> f32 {
    if m.alive > 0 { MORE_H + MORE_LINE } else { MORE_H }
}

/// How many icons the pill may draw in a thread view this tall: at most `PILL_MAX`, fewer when
/// the pill would take more than 60% of the height (D168).
pub(crate) fn pill_cap_for(thread: Rect) -> usize {
    pill_cap(thread.height() * 0.6, PILL_ROW, 3.0 * GLYPH_H + 3.0 * SEP_H + MORE_H + MORE_LINE + 12.0)
}

/// The card's height: a heading per cluster and every row, but never taller than the thread
/// view less a margin (the list scrolls then).
fn card_height(groups: &[(&str, Vec<&AgentRow>)], thread: Rect) -> f32 {
    let rows: usize = groups.iter().map(|(_, rows)| rows.len()).sum();
    (rows as f32 * CARD_ROW + groups.len() as f32 * CARD_HEAD + CARD_CHROME).min((thread.height() - 2.0 * CARD_MARGIN).max(CARD_CHROME + CARD_ROW))
}

/// The pill and card regions for a panel in a thread view (what the hover rule uses).
#[cfg(test)]
pub(crate) fn regions_for(thread: Rect, panel: &AgentsPanel) -> (Rect, Rect) {
    floating_regions(thread, pill_height(&pill(panel, pill_cap_for(thread))), card_height(&card_groups(panel), thread), 0.0)
}

/// The pill's "more" item (D168), in `r`.
fn more_item(painter: &egui::Painter, r: Rect, m: &Overflow, p: &Palette) {
    let cx = r.center().x;
    let mut y = r.top() + 4.0;
    let mut line = |text: String, font: egui::FontId, color: egui::Color32| {
        let g = painter.layout_no_wrap(text, font, color);
        painter.galley(pos2(cx - g.size().x / 2.0, y), g.clone(), color);
        y += g.size().y;
    };
    line("more".to_owned(), theme::ui(size::SMALL - 1.0), p.text2);
    line(format!("({})", m.hidden), theme::bold(size::SMALL), p.text);
    if m.alive > 0 {
        line(format!("{} alive", m.alive), theme::ui(size::SMALL - 2.0), p.text2);
    }
    if m.needs_you > 0 {
        let corner = pos2(r.right() - 3.0, r.top() + 5.0);
        painter.circle_filled(corner, 6.0, p.raised);
        icons::exclaim(painter, corner, 10.0, p.failed);
    }
}

/// The pill, or the card while hovered. Floats over the thread; takes no layout column.
pub(crate) fn floating(ctx: &egui::Context, app: &mut YaacApp, thread: Rect, panel: &AgentsPanel, actions: &mut Vec<Action>) {
    let p = app.p;
    let card = app.ui.panel == PanelMode::Card;
    let now = app.now();
    let all = card_groups(panel);
    if card {
        schedule_activity_ages(ctx, app, all.iter().flat_map(|(_, rows)| rows.iter().copied()), now);
    }
    let pl = pill(panel, pill_cap_for(thread));
    let first_hidden = app.ui.card_to_hidden.then(|| {
        clusters(panel).into_iter().flat_map(|cluster| cluster.rows).find(|row| !pl.shows(row))
    }).flatten();
    let spent = panel.budget.as_ref().is_some_and(|b| b.exhausted);
    let (pill_extra, card_extra) = if spent { (BUDGET_H, BUDGET_H + 4.0) } else { (0.0, 0.0) };
    let pill_h = pill_height(&pl) + pill_extra;
    let card_h = card_height(&all, thread) + card_extra;
    let travel = ((thread.height() - pill_h.max(card_h)) / 2.0 - 4.0).max(0.0);
    let offset = app.ui.agents_offset.clamp(-travel, travel);
    let (strip, card_rect) = floating_regions(thread, pill_h, card_h, offset);
    let width = if card { CARD_W } else { PILL_W };
    let pos = if card { card_rect.min } else { strip.min };
    let mut any_focus = false;
    let mut to_hidden = app.ui.card_to_hidden && card;
    let mut more_rect = None;
    let resp = egui::Area::new(egui::Id::unique("agents-floating")).order(egui::Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
        let radius = if card { 12 } else { 22 };
        egui::Frame::popup(ui.style()).fill(p.raised).corner_radius(radius).inner_margin(Margin::same(if card { 10 } else { 6 })).show(ui, |ui| {
            ui.set_width(width - if card { 20.0 } else { 12.0 });
            ui.spacing_mut().item_spacing.y = 0.0;
            if card {
                ui.set_min_height(card_rect.height() - 20.0);
                ui.set_max_height(card_rect.height() - 20.0);
                ui.horizontal(|ui| {
                    let (grip, drag) = ui.allocate_exact_size(vec2(16.0, 20.0), Sense::drag());
                    let drag = drag.on_hover_cursor(egui::CursorIcon::ResizeVertical).on_hover_text("Move Related agents");
                    let ink = if drag.hovered() || drag.dragged() { p.text } else { p.text2 };
                    for y in [-3.0, 0.0, 3.0] {
                        let center = grip.center() + vec2(0.0, y);
                        ui.painter().hline((center.x - 4.0)..=(center.x + 4.0), center.y, Stroke::new(1.0, ink));
                    }
                    if drag.dragged() {
                        any_focus = true;
                        app.ui.agents_offset = (offset + drag.drag_delta().y).clamp(-travel, travel);
                        ctx.request_repaint();
                    }
                    ui.label(egui::RichText::new("Related agents").font(theme::bold(size::UI)));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let d = widgets::plain(ui, &p, "Dock", None).on_hover_text("Pin to the side");
                        any_focus |= d.has_focus();
                        if d.clicked() {
                            actions.push(Action::Panel(PanelInput::Dock));
                        }
                        any_focus |= mark_all_as_read(ui, &p, all.iter().flat_map(|(_, rows)| rows.iter().copied()), actions);
                    });
                });
                ui.add_space(4.0);
                if let Some(b) = panel.budget.as_ref().filter(|b| b.exhausted) {
                    budget_continue(ui, &p, b, actions);
                    ui.add_space(4.0);
                }
                // The list uses the shared relation order and scrolls when taller than the card (D168).
                let rest = (card_rect.bottom() - 10.0 - ui.cursor().top()).max(CARD_ROW);
                egui::ScrollArea::vertical().id_salt("agents-card").max_height(rest).min_scrolled_height(rest).auto_shrink([false, true]).show(ui, |ui| {
                    for (heading, rows) in &all {
                        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), CARD_HEAD), Sense::hover());
                        let g = ui.fonts_mut(|f| f.layout_no_wrap((*heading).to_owned(), theme::bold(size::SMALL), p.text2));
                        ui.painter().galley(pos2(r.left() + 4.0, r.bottom() - g.size().y - 3.0), g, p.text2);
                        for &row in rows {
                            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), CARD_ROW), Sense::click());
                            if to_hidden && first_hidden.is_some_and(|hidden| {
                                std::ptr::eq(hidden, row) || (row.key.is_some() && row.key == hidden.key)
                            }) {
                                // Opened from "more": the first agent the pill hides at the top.
                                ui.scroll_to_rect(r.expand2(vec2(0.0, 6.0)), Some(egui::Align::TOP));
                                to_hidden = false;
                            }
                            any_focus |= resp.has_focus();
                            card_row(ui, r, &resp, row, &p, now);
                            if resp.clicked()
                                && let Some(k) = &row.key
                            {
                                actions.push(Action::Open(k.clone()));
                            }
                        }
                    }
                });
            } else {
                ui.vertical_centered(|ui| {
                    if spent {
                        // The budget, on the pill too (D164): its card has Continue.
                        widgets::mark(ui, &p, Mark::Paused, BUDGET_H).on_hover_text("Agent budget reached · Continue in this card");
                    }
                    for (i, c) in pl.clusters.iter().enumerate() {
                        if i > 0 {
                            hairline(ui, &p);
                        }
                        let (r, resp) = ui.allocate_exact_size(vec2(32.0, GLYPH_H), Sense::hover());
                        relation_glyph(ui.painter(), r.center(), c.relation, &p);
                        resp.on_hover_text(c.relation.heading());
                        for row in &c.rows {
                            let (r, resp) = ui.allocate_exact_size(vec2(32.0, PILL_ROW), Sense::click());
                            any_focus |= resp.has_focus();
                            harness_icon(ui.painter(), r.center(), 22.0, row, &p);
                            if let Some(f) = row.flow {
                                arrow(ui.painter(), pos2(r.right() - 1.0, r.top() + 8.0), f, &p);
                            }
                            if resp.clicked()
                                && let Some(k) = &row.key
                            {
                                actions.push(Action::Open(k.clone()));
                            }
                        }
                    }
                    if pl.more.hidden > 0 {
                        if !pl.clusters.is_empty() {
                            hairline(ui, &p);
                        }
                        let (r, _) = ui.allocate_exact_size(vec2(32.0, more_height(&pl.more)), Sense::hover());
                        more_item(ui.painter(), r, &pl.more, &p);
                        more_rect = Some(r);
                    }
                });
            }
        });
    });
    let _ = resp;
    app.ui.card_to_hidden = to_hidden;
    if !card {
        app.ui.pill_icons = pl.icons();
        app.ui.pill_more = more_rect;
    }
    if app.shots.is_some() {
        return;
    }
    // Open and close from fixed regions and a short delay (yaac_core::hover), never from the
    // area's own outline, which grows and shrinks with the state.
    let pointer = ctx.pointer_hover_pos();
    let region = |r: Rect| Region::new((r.left(), r.top()), (r.right(), r.bottom()));
    let now = ctx.input(|i| i.time);
    let step = app.ui.hover.step(pointer.map(|p| (p.x, p.y)), region(strip), region(card_rect), any_focus, now);
    if let Some(s) = step.wake_in {
        // The pending close: one scheduled repaint, not a running loop.
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(s));
    }
    match (app.ui.panel, step.open) {
        (PanelMode::Pill, true) => {
            // Reached from the "more" item: the card shows the hidden agents (D168).
            app.ui.card_to_hidden = pointer.is_some_and(|q| app.ui.pill_more.is_some_and(|m| m.expand(2.0).contains(q)));
            actions.push(Action::Panel(PanelInput::Enter));
            ctx.request_repaint();
        }
        (PanelMode::Card, false) => {
            app.ui.card_to_hidden = false;
            actions.push(Action::Panel(PanelInput::Leave));
            ctx.request_repaint();
        }
        _ => {}
    }
}

/// A short hairline between the pill's clusters, and above its "more" item.
fn hairline(ui: &mut egui::Ui, p: &Palette) {
    let (r, _) = ui.allocate_exact_size(vec2(32.0, SEP_H), Sense::hover());
    let y = ui.painter().round_to_pixel_center(r.center().y);
    ui.painter().hline((r.center().x - 10.0)..=(r.center().x + 10.0), y, Stroke::new(1.0, p.border));
}

/// One row of the card: icon, name, and one line with the last thing it did.
fn card_row(ui: &egui::Ui, r: Rect, resp: &egui::Response, row: &AgentRow, p: &Palette, now: i64) {
    if resp.hovered() || resp.has_focus() {
        ui.painter().rect_filled(r, 6.0, p.faint);
    }
    if row.display.unseen {
        crate::sidebar::unseen_outline(ui, r, p);
    }
    harness_icon(ui.painter(), pos2(r.left() + 16.0, r.center().y), 20.0, row, p);
    let text_w = r.width() - 60.0;
    let name_color = if row.finished { p.text2 } else { p.text };
    let right = r.right() - if row.flow.is_some() { 24.0 } else { 8.0 };
    let title_right = draw_activity_age(ui, row, now, right, r.top() + 13.0, p);
    let g = widgets::elided(ui, &row.title, &theme::bold(size::UI), name_color, (title_right - r.left() - 34.0).max(1.0));
    ui.painter().galley(pos2(r.left() + 34.0, r.top() + 5.0), g, name_color);
    let second = row.origin.as_ref().filter(|_| row.subagent).map_or_else(|| row.last_action.clone(), |o| format!("{o} · {}", row.last_action));
    let g = widgets::elided(ui, &second, &theme::ui(size::SMALL), p.text2, text_w);
    ui.painter().galley(pos2(r.left() + 34.0, r.top() + 24.0), g, p.text2);
    if let Some(f) = row.flow {
        arrow(ui.painter(), pos2(r.right() - 12.0, r.center().y), f, p);
    }
}

fn draw_activity_age(ui: &egui::Ui, row: &AgentRow, now: i64, right: f32, y: f32, p: &Palette) -> f32 {
    let Some(age) = row.activity_age(now) else { return right };
    let label = ui.fonts_mut(|fonts| fonts.layout_no_wrap(age, theme::ui(size::SMALL), p.text2));
    let left = right - label.size().x;
    ui.painter().galley(pos2(left, y - label.size().y / 2.0), label, p.text2);
    left - 6.0
}

pub(crate) fn schedule_activity_ages<'a>(ctx: &egui::Context, app: &YaacApp, rows: impl Iterator<Item = &'a AgentRow>, now: i64) {
    if matches!(app.clock, crate::app::Clock::Story { frozen: true, .. }) {
        return;
    }
    let next = rows.filter(|row| row.updated != 0).map(|row| {
        let elapsed = now.saturating_sub(row.updated);
        if elapsed < 60 {
            60 - elapsed
        } else {
            let unit = if elapsed < 3600 { 60 } else if elapsed < 86_400 { 3600 } else { 86_400 };
            unit - elapsed % unit
        }
    }).min();
    if let Some(seconds) = next.and_then(|n| u64::try_from(n).ok()) {
        ctx.request_repaint_after(std::time::Duration::from_secs(seconds));
    }
}

const PILL_W: f32 = 44.0;
/// The budget's row: the paused mark in the pill, "Agent budget reached · Continue" in the card.
const BUDGET_H: f32 = 18.0;
const PILL_ROW: f32 = 32.0;
/// A cluster's glyph row in the pill.
const GLYPH_H: f32 = 16.0;
/// The hairline between clusters.
const SEP_H: f32 = 9.0;
/// The "more" item's two lines, and its third.
const MORE_H: f32 = 36.0;
const MORE_LINE: f32 = 12.0;
const CARD_W: f32 = 300.0;
const CARD_ROW: f32 = 44.0;
/// A relation's heading in the card.
const CARD_HEAD: f32 = 24.0;
/// The card's title line and margins.
const CARD_CHROME: f32 = 64.0;
/// The card keeps this far from the thread view's top and bottom.
const CARD_MARGIN: f32 = 12.0;

/// The pill's strip and the card's region, from the layout alone: both share the thread's
/// right edge (less a 12 pt margin) and are centred on the same line.
fn floating_regions(thread: Rect, pill_h: f32, card_h: f32, offset: f32) -> (Rect, Rect) {
    let right = thread.right() - 12.0;
    let cy = thread.center().y + offset;
    let strip = Rect::from_min_max(pos2(right - PILL_W, cy - pill_h / 2.0), pos2(right, cy + pill_h / 2.0));
    let card = Rect::from_min_max(pos2(right - CARD_W, cy - card_h / 2.0), pos2(right, cy + card_h / 2.0));
    (strip, card)
}

const DOCK_LINE: f32 = 17.0;

/// One docked row, drawn in a fixed rectangle so it can keep its hover colour while its menu
/// is open (D108), carry the unseen outline (D114), and stay readable when narrow: the title
/// is ellipsised and every mark keeps its place (D115).
fn docked_row(ui: &mut egui::Ui, app: &mut YaacApp, row: &AgentRow, now: i64, actions: &mut Vec<Action>) -> Option<yaac_core::ThreadKey> {
    let p = app.p;
    let lines = 2 + usize::from(!row.activity.is_empty()) + usize::from(row.git.is_some());
    let h = lines as f32 * DOCK_LINE + 10.0;
    let (outer, resp) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::click());
    let rect = Rect::from_min_max(outer.min, pos2(outer.right(), outer.bottom() - 2.0));
    let menu_open = row.key.as_ref().is_some_and(|k| app.ui.menu == Some(Menu::Context(k.clone())));
    if row.key.is_some() && (resp.hovered() || menu_open) {
        ui.painter().rect_filled(rect, 5.0, p.faint);
    }
    if row.display.unseen {
        crate::sidebar::unseen_outline(ui, rect, &p);
    }
    let left = rect.left() + 8.0;
    let right = rect.right() - 8.0;
    let mut y = rect.top() + 5.0 + DOCK_LINE / 2.0;
    // Line 1: state mark, title, then the state word and the arrow at the right.
    icons::thread_mark(ui.painter(), pos2(left + 7.0, y), 14.0, &row.display, &p);
    let mut rx = draw_activity_age(ui, row, now, right, y, &p);
    if let Some(f) = row.flow {
        arrow(ui.painter(), pos2(rx - 5.0, y), f, &p);
        rx -= 14.0;
    }
    if row.display.mark != Mark::Idle {
        let g = ui.fonts_mut(|f| f.layout_no_wrap(row.display.mark.word().to_owned(), theme::ui(size::SMALL), p.mark(row.display.mark)));
        rx -= g.size().x;
        ui.painter().galley(pos2(rx, y - g.size().y / 2.0), g, p.mark(row.display.mark));
        rx -= 6.0;
    }
    let tx = left + 20.0;
    let color = if row.finished { p.text2 } else { p.text };
    // The PR badge (D119), before the state word; the title gives way.
    if let Some(k) = row.key.clone() {
        app.ui.pr_visible.push(k.clone());
        if let Some(b) = yaac_core::pr::badge_of(&app.client.state, &k) {
            rx = crate::pr_ui::badge(ui, app, &k, &b, rx, y, "dock", actions) - 6.0;
        }
    }
    let room = yaac_core::sidebar::title_width(rx - tx, &[], 4.0);
    let g = widgets::elided(ui, &row.title, &theme::ui(size::UI), color, room.max(1.0));
    let tw = g.size().x;
    ui.painter().galley(pos2(tx, y - g.size().y / 2.0), g, color);
    if resp.hovered() && row.key.is_some() {
        ui.painter().hline(tx..=tx + tw, y + 8.0, Stroke::new(1.0, color));
    }
    // Line 2: the harness logo, branch, and git counts, as in the sidebar.
    if let Some(l2) = &row.git {
        y += DOCK_LINE;
        crate::sidebar::git_marks(ui, &p, l2, left + 14.0, y, right, &theme::ui(size::SMALL - 0.5));
    }
    // Then provider · model · target, and the latest activity.
    let mut parts: Vec<String> = Vec::new();
    // A harness's own sub-agent says so ("Claude subagent · Explore"), a mesh worker too (D151).
    parts.extend(row.origin.clone());
    parts.extend(row.provider.clone().filter(|_| !row.subagent));
    parts.extend(row.model.clone());
    parts.push(row.target.clone());
    y += DOCK_LINE;
    let g = widgets::elided(ui, &parts.join(" · "), &theme::ui(size::SMALL), p.text2, (right - tx).max(1.0));
    ui.painter().galley(pos2(tx, y - g.size().y / 2.0), g, p.text2);
    if !row.activity.is_empty() {
        y += DOCK_LINE;
        let g = widgets::elided(ui, &row.activity, &theme::ui(size::SMALL), p.text2, (right - tx).max(1.0));
        ui.painter().galley(pos2(tx, y - g.size().y / 2.0), g, p.text2);
    }
    if let Some(k) = row.key.clone() {
        if resp.clicked() {
            actions.push(Action::Open(k.clone()));
        }
        crate::sidebar::row_menu(ui, app, &k, &resp, now, actions);
    }
    resp.hovered().then(|| row.key.clone()).flatten()
}

/// "Agent budget reached · Continue" (D164), under the budget in the card and the docked panel.
fn budget_continue(ui: &mut egui::Ui, p: &Palette, b: &BudgetLine, actions: &mut Vec<Action>) {
    if !b.exhausted {
        return;
    }
    ui.horizontal(|ui| {
        ui.label(widgets::secondary("Agent budget reached", p));
        if widgets::link(ui, p, "Continue", theme::ui(size::SMALL)).clicked() {
            actions.push(Action::ContinueBudget(b.root.clone()));
        }
    });
}

fn mark_all_as_read<'a>(ui: &mut egui::Ui, p: &Palette, rows: impl Iterator<Item = &'a AgentRow>, actions: &mut Vec<Action>) -> bool {
    let unseen: Vec<_> = rows.filter(|row| row.display.unseen).filter_map(|row| row.key.as_ref()).collect();
    if unseen.is_empty() {
        return false;
    }
    let button = widgets::plain(ui, p, "Mark all as read", None);
    if button.clicked() {
        actions.push(Action::MarkRead(unseen.into_iter().cloned().collect()));
    }
    button.has_focus()
}

/// Docked: "Related agents" (D115), the rows under the three quiet headings (D118); no
/// boxes, no rules.
pub(crate) fn docked(ui: &mut egui::Ui, app: &mut YaacApp, panel: &AgentsPanel, actions: &mut Vec<Action>) -> Option<yaac_core::ThreadKey> {
    let p = app.p;
    let now = app.now();
    schedule_activity_ages(ui.ctx(), app, panel.all().chain(&panel.siblings), now);
    ui.spacing_mut().interact_size.y = 16.0;
    ui.spacing_mut().item_spacing.y = 2.0;
    let w = ui.available_width();
    ui.set_max_width(w);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Related agents").font(theme::bold(size::SECTION + 1.0)));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let r = widgets::plain(ui, &p, "Undock", None).on_hover_text("Back to the pill");
            if r.clicked() {
                actions.push(Action::Panel(PanelInput::Undock));
            }
        });
    });
    mark_all_as_read(ui, &p, panel.all().chain(&panel.siblings), actions);
    if let Some(b) = &panel.budget {
        ui.horizontal(|ui| {
            if b.exhausted {
                widgets::mark(ui, &p, Mark::Paused, 14.0);
            }
            let c = if b.exhausted { p.needs } else { p.text2 };
            // Wrapped, so a narrow panel never grows past its edge (D115).
            ui.add(egui::Label::new(egui::RichText::new(&b.text).font(theme::ui(size::SMALL)).color(c)).wrap());
        });
        budget_continue(ui, &p, b, actions);
    }
    ui.add_space(14.0);
    let groups = docked_clusters(panel);
    let parent = groups.iter().find(|group| group.relation == Relation::SpawnedBy).and_then(|group| group.rows.first());
    let mut hovered = egui::Panel::bottom("agents-docked-parent")
        .show_separator_line(false)
        .frame(egui::Frame::NONE.inner_margin(Margin { left: 0, right: 0, top: 8, bottom: 0 }))
        .show(ui, |ui| {
            heading(ui, &p, Relation::SpawnedBy.heading(), None);
            if let Some((title, _)) = &panel.root {
                ui.label(widgets::secondary(format!("root: {title}"), &p));
            }
            if let Some(parent) = parent {
                docked_row(ui, app, parent, now, actions)
            } else {
                ui.label(widgets::secondary("started by you", &p));
                None
            }
        }).inner;
    egui::ScrollArea::vertical().id_salt("agents-docked").auto_shrink([false, false]).show(ui, |ui| {
        let siblings = sibling_rows(panel);
        if !siblings.is_empty() {
            heading(ui, &p, "Siblings", None);
            for row in siblings {
                if let Some(key) = docked_row(ui, app, row, now, actions) {
                    hovered = Some(key);
                }
            }
            ui.add_space(10.0);
        }
        for c in groups.iter().filter(|group| group.relation != Relation::SpawnedBy) {
            let count = if c.relation == Relation::Workers { panel.direct_workers } else { c.rows.len() };
            heading(ui, &p, c.relation.heading(), Some(count));
            for e in docked_entries(c) {
                match e {
                    DockEntry::Row(row) => {
                        if let Some(key) = docked_row(ui, app, row, now, actions) {
                            hovered = Some(key);
                        }
                    }
                    DockEntry::Fold { rows, word } => {
                        let i = c.relation as usize;
                        let open = app.ui.dock_folds_open[i];
                        let line = fold_line(ui, &p, &format!("{} {word}", rows.len()), open);
                        if std::mem::take(&mut app.ui.dock_to_fold) {
                            line.scroll_to_me(Some(egui::Align::BOTTOM));
                        }
                        if line.clicked() {
                            app.ui.dock_folds_open[i] = !open;
                        }
                        if open {
                            for row in rows {
                                if let Some(key) = docked_row(ui, app, row, now, actions) {
                                    hovered = Some(key);
                                }
                            }
                        }
                    }
                }
            }
            ui.add_space(10.0);
        }
    });
    hovered
}

/// The docked panel's fold line: a chevron and "12 finished", quiet, clickable.
fn fold_line(ui: &mut egui::Ui, p: &Palette, text: &str, open: bool) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), DOCK_LINE + 8.0), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(r, 5.0, p.faint);
    }
    icons::chevron(ui.painter(), pos2(r.left() + 15.0, r.center().y), 14.0, open, p.text2);
    let g = ui.fonts_mut(|f| f.layout_no_wrap(text.to_owned(), theme::ui(size::SMALL), p.text2));
    ui.painter().galley(pos2(r.left() + 28.0, r.center().y - g.size().y / 2.0), g, p.text2);
    resp.on_hover_text(if open { "Fold them" } else { "Show them" })
}

fn heading(ui: &mut egui::Ui, p: &Palette, text: &str, n: Option<usize>) {
    let t = match n {
        Some(n) => format!("{text} ({n})"),
        None => text.to_owned(),
    };
    ui.label(egui::RichText::new(t).font(theme::bold(size::SMALL)).color(p.text2));
    ui.add_space(4.0);
}

#[cfg(test)]
mod tests {
    //! The real widget on a headless egui context, with the demo fleet and simulated pointer
    //! positions: the card must not flicker, and must not keep the window repainting.
    use eframe::egui::{Event, Pos2, RawInput, ViewportId, vec2};
    use yaac_core::agents::agents_panel;
    use yaac_core::attention::AttentionMap;

    use super::*;

    fn app(ctx: &egui::Context) -> YaacApp {
        let launch = crate::Launch {
            demo: Some(yaac_core::demo::Scenario::Fleet),
            theme: crate::theme::ThemeChoice::Dark,
            shots: None,
            host_windows: false,
            #[cfg(feature = "hub")]
            attach: None,
            display: crate::display::DisplayState::default(),
            shell: crate::tray::Shell::new(),
            tray: false,
            updater: crate::updater::Updater::off("tests"),
        };
        let mut app = YaacApp::from_ctx(ctx, launch);
        app.clock = crate::app::Clock::Story { base: app.now(), frozen: true };
        app
    }

    /// One frame with the pointer at `p`; returns the panel mode after it and the repaint
    /// delay egui was asked for.
    fn frame(ctx: &egui::Context, app: &mut YaacApp, thread: Rect, p: Option<Pos2>, t: f64) -> (PanelMode, std::time::Duration) {
        // An event only when the pointer moves (a frame without input is what an idle window
        // sees).
        let last: Option<Pos2> = ctx.input(|i| i.pointer.latest_pos());
        let events = match p {
            Some(p) if last != Some(p) => vec![Event::PointerMoved(p)],
            Some(_) => vec![],
            None => vec![Event::PointerGone],
        };
        let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 1000.0))), events, time: Some(t), ..Default::default() };
        let mut actions = Vec::new();
        let mut out = ctx.run_ui(input, |ui| {
            let map = AttentionMap::compute(&app.client.state);
            let key = app.client.state.selected_thread().cloned().unwrap();
            let panel = agents_panel(&app.client.state, &map, &key).unwrap();
            floating(ui.ctx(), app, thread, &panel, &mut actions);
        });
        out.textures_delta.clear();
        app.apply(actions);
        (app.ui.panel, out.viewport_output[&ViewportId::ROOT].repaint_delay)
    }

    #[test]
    fn the_card_stays_open_while_the_pointer_moves_over_it_and_closes_once_after_leaving() {
        let ctx = egui::Context::default();
        let mut app = app(&ctx);
        let thread = Rect::from_min_max(pos2(280.0, 0.0), pos2(1600.0, 1000.0));
        let panel = {
            let map = AttentionMap::compute(&app.client.state);
            agents_panel(&app.client.state, &map, app.client.state.selected_thread().unwrap()).unwrap()
        };
        let (strip, card) = regions_for(thread, &panel);
        // Idle, pointer far away: the pill, and (once egui's first-frame layout passes are
        // done) nothing keeps the window awake.
        for i in 0..10 {
            frame(&ctx, &mut app, thread, Some(pos2(600.0, 500.0)), f64::from(i) * 0.1 - 2.0);
        }
        let (mode, delay) = frame(&ctx, &mut app, thread, Some(pos2(600.0, 500.0)), 0.0);
        assert_eq!(mode, PanelMode::Pill);
        assert!(delay > std::time::Duration::from_secs(60), "idle repaints: {delay:?}");
        // Onto the pill: the card opens.
        let (mode, _) = frame(&ctx, &mut app, thread, Some(strip.center()), 0.1);
        assert_eq!(mode, PanelMode::Card);
        // Across the card, frame by frame, including over parts far from the pill: it never
        // closes (the owner's flicker).
        let mut t = 0.1;
        for i in 0..40 {
            t += 0.016;
            let f = i as f32 / 39.0;
            let p = pos2(card.left() + 4.0 + (card.width() - 8.0) * (1.0 - f), card.top() + 4.0 + (card.height() - 8.0) * f);
            let (mode, _) = frame(&ctx, &mut app, thread, Some(p), t);
            assert_eq!(mode, PanelMode::Card, "closed at {p:?}");
        }
        // Resting over the card: open, and idle (no repaint pending) once settled.
        let mut d = std::time::Duration::ZERO;
        for k in 0..5 {
            let (mode, delay) = frame(&ctx, &mut app, thread, Some(card.center()), t + 0.1 * f64::from(k) + 0.1);
            assert_eq!(mode, PanelMode::Card);
            d = delay;
        }
        assert!(d > std::time::Duration::from_secs(60), "{d:?}");
        t += 0.6;
        // Out: still open for a quarter of a second, with one scheduled repaint to close it.
        let (mode, _) = frame(&ctx, &mut app, thread, Some(pos2(600.0, 500.0)), t + 0.01);
        assert_eq!(mode, PanelMode::Card);
        // (The frame of the move and egui's hover-out pass repaint for themselves; after that
        // the window is woken only by the pending close.)
        frame(&ctx, &mut app, thread, Some(pos2(600.0, 500.0)), t + 0.05);
        let (mode, delay) = frame(&ctx, &mut app, thread, Some(pos2(600.0, 500.0)), t + 0.1);
        assert_eq!(mode, PanelMode::Card);
        assert!(delay <= std::time::Duration::from_millis(250) && delay > std::time::Duration::ZERO, "{delay:?}");
        let (mode, _) = frame(&ctx, &mut app, thread, Some(pos2(600.0, 500.0)), t + 0.2);
        assert_eq!(mode, PanelMode::Card);
        let (mode, _) = frame(&ctx, &mut app, thread, Some(pos2(600.0, 500.0)), t + 0.3);
        assert_eq!(mode, PanelMode::Pill);
        // Closed and idle again: once egui's own short layout and fade passes are over, no
        // repaint is pending.
        let mut delay = std::time::Duration::ZERO;
        for i in 0..6 {
            delay = frame(&ctx, &mut app, thread, Some(pos2(600.0, 500.0)), t + 0.4 + f64::from(i) * 0.2).1;
        }
        assert!(delay > std::time::Duration::from_secs(60), "{delay:?}");
    }

    /// D168: an orchestrator with 30 related agents draws at most `PILL_MAX` icons and one
    /// "more (23)" item, and hovering that item opens the card scrolled to the hidden ones.
    #[test]
    fn thirty_agents_draw_at_most_the_cap_and_more_opens_the_card_at_the_hidden_ones() {
        let ctx = egui::Context::default();
        let mut app = app(&ctx);
        if let Some(h) = app.demo.clone() {
            h.play(yaac_core::demo::Scene::ManyAgents);
            h.drain();
        }
        app.pump_demo();
        app.open(&yaac_core::demo::threads::orchestrator(), None);
        let thread = Rect::from_min_max(pos2(280.0, 0.0), pos2(1600.0, 1000.0));
        let mut more_text = false;
        for i in 0..4 {
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 1000.0))),
                events: vec![Event::PointerMoved(pos2(600.0, 500.0))],
                time: Some(f64::from(i) * 0.1),
                ..Default::default()
            };
            let mut actions = Vec::new();
            let mut out = ctx.run_ui(input, |ui| {
                let map = AttentionMap::compute(&app.client.state);
                let key = app.client.state.selected_thread().cloned().unwrap();
                let panel = agents_panel(&app.client.state, &map, &key).unwrap();
                assert_eq!(panel.all().count(), 30);
                floating(ui.ctx(), &mut app, thread, &panel, &mut actions);
            });
            out.textures_delta.clear();
            more_text = out.shapes.iter().any(|c| matches!(&c.shape, egui::Shape::Text(t) if t.galley.text() == "(23)"));
            app.apply(actions);
        }
        assert_eq!(app.ui.panel, PanelMode::Pill);
        assert_eq!(app.ui.pill_icons, yaac_core::agents::PILL_MAX);
        assert!(more_text, "the more item says (23)");
        let more = app.ui.pill_more.expect("the more item");
        // The pill fits the view, and the card does too (it scrolls).
        let panel = {
            let map = AttentionMap::compute(&app.client.state);
            agents_panel(&app.client.state, &map, app.client.state.selected_thread().unwrap()).unwrap()
        };
        let (strip, card) = regions_for(thread, &panel);
        assert!(strip.height() < thread.height() * 0.6 && strip.contains(more.center()));
        assert!(card.top() >= thread.top() && card.bottom() <= thread.bottom());
        // Onto "more": the card opens, asked to show the hidden agents, and does on its next
        // frame.
        let (mode, _) = frame(&ctx, &mut app, thread, Some(more.center()), 1.0);
        assert_eq!(mode, PanelMode::Card);
        assert!(app.ui.card_to_hidden);
        let (mode, _) = frame(&ctx, &mut app, thread, Some(more.center()), 1.05);
        assert_eq!(mode, PanelMode::Card);
        assert!(!app.ui.card_to_hidden, "scrolled to the first hidden agent");
    }

    #[test]
    fn mark_all_as_read_clears_related_unseen_marks_in_one_update() {
        use yaac_core::state::ViewPatch;

        let ctx = egui::Context::default();
        let mut app = app(&ctx);
        let demo = app.demo.clone().unwrap();
        demo.play(yaac_core::demo::Scene::ManyAgents);
        demo.drain();
        app.pump_demo();
        let root = yaac_core::demo::threads::orchestrator();
        app.open(&root, None);
        let related = {
            let map = AttentionMap::compute(&app.client.state);
            let panel = agents_panel(&app.client.state, &map, &root).unwrap();
            panel.all().filter(|row| row.display.mark == Mark::Done).filter_map(|row| row.key.clone()).collect::<Vec<_>>()
        };
        assert!(related.len() > yaac_core::agents::PILL_MAX);
        let unrelated = yaac_core::demo::threads::fix();
        assert!(!related.contains(&unrelated));
        for key in related.iter().chain(std::iter::once(&unrelated)) {
            app.client.set_view(key, ViewPatch { unseen: Some(true), pinned: Some(true), ..Default::default() });
        }
        app.pump_demo();
        let request_start = demo.requests().len();
        let thread = Rect::from_min_max(pos2(280.0, 0.0), pos2(1600.0, 440.0));
        let mut draw = |events| {
            let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 1000.0))), events, ..Default::default() };
            let mut actions = Vec::new();
            let mut out = ctx.run_ui(input, |ui| {
                let map = AttentionMap::compute(&app.client.state);
                let panel = agents_panel(&app.client.state, &map, &root).unwrap();
                floating(ui.ctx(), &mut app, thread, &panel, &mut actions);
            });
            out.textures_delta.clear();
            app.apply(actions);
            out.shapes.iter().find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.text() == "Mark all as read" => Some(t.pos + t.galley.size() / 2.0),
                _ => None,
            })
        };
        draw(vec![Event::PointerMoved(pos2(1566.0, 220.0))]);
        draw(Vec::new());
        let at = draw(Vec::new()).expect("Mark all as read is in the card header");
        draw(vec![Event::PointerMoved(at)]);
        for pressed in [true, false] {
            draw(vec![Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE }]);
        }
        assert!(draw(Vec::new()).is_none(), "all related agents are read");
        app.pump_demo();
        assert_eq!(app.client.state.selected_thread(), Some(&root));
        assert!(app.client.state.is_unseen(&unrelated));
        for key in &related {
            let view = &app.client.state.view[key];
            assert!(!view.unseen && view.pinned);
        }
        let requests = demo.requests();
        let updates: Vec<_> = requests[request_start..].iter().filter(|(_, method, _)| method == yaac_core::client::hub_names::VIEW_SET_MANY).collect();
        assert_eq!(updates.len(), 1);
        let entries = updates[0].2["entries"].as_array().unwrap();
        assert_eq!(entries.len(), related.len());
        assert!(entries.iter().all(|entry| entry["unseen"] == false && entry.get("pinned").is_none()));
    }

    #[test]
    fn docked_parent_stays_at_the_bottom_during_scroll() {
        fn text_y(shapes: &[egui::epaint::ClippedShape], text: &str) -> f32 {
            shapes.iter().find_map(|shape| match &shape.shape {
                egui::Shape::Text(label) if label.galley.text() == text => Some(label.pos.y),
                _ => None,
            }).unwrap_or_else(|| panic!("{text} was not drawn"))
        }

        let ctx = egui::Context::default();
        let mut app = crate::app::tests::demo_app(&ctx);
        let key = yaac_core::demo::threads::users();
        let map = AttentionMap::compute(&app.client.state);
        let mut panel = agents_panel(&app.client.state, &map, &key).unwrap();
        let mut example = panel.contacts.iter().chain(&panel.siblings).next().unwrap().clone();
        example.display.mark = Mark::Running;
        panel.workers = vec![example; 20];
        panel.siblings.clear();
        let mut render = |events| {
            let mut output = ctx.run_ui(RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(380.0, 300.0))),
                events,
                ..Default::default()
            }, |ui| {
                docked(ui, &mut app, &panel, &mut Vec::new());
            });
            output.textures_delta.clear();
            output
        };
        render(vec![Event::PointerMoved(pos2(130.0, 110.0))]);
        let before = render(vec![]);
        let parent = text_y(&before.shapes, "Spawned by");
        assert!(parent > 180.0, "footer is not near the bottom: {parent}");
        let workers = text_y(&before.shapes, &format!("Workers ({})", panel.direct_workers));
        let after = render(vec![Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point, delta: vec2(0.0, -25.0),
            modifiers: egui::Modifiers::NONE, phase: egui::TouchPhase::Move, source: egui::MouseWheelSource::Wheel,
        }]);
        let settled = render(vec![]);
        assert_eq!(text_y(&after.shapes, "Spawned by"), parent);
        assert_eq!(text_y(&settled.shapes, "Spawned by"), parent);
        assert!(text_y(&settled.shapes, &format!("Workers ({})", panel.direct_workers)) < workers);
    }

    #[test]
    fn docked_preview_waits_then_switches_rows_without_selecting_or_marking_read() {
        use yaac_core::demo::threads;
        let ctx = egui::Context::default();
        let mut app = crate::app::tests::demo_app(&ctx);
        let owner = threads::orchestrator();
        let first = threads::users();
        let second = threads::billing();
        app.open(&owner, None);
        app.ui.panel = PanelMode::Docked;
        app.client.state.view.entry(first.clone()).or_default().unseen = true;
        dock_hover(&mut app, &ctx, true, Some(first.clone()));
        assert_eq!(app.ui.dock_preview.target(&owner), None);
        app.started -= std::time::Duration::from_secs(2);
        poll_dock_hover(&mut app, &ctx);
        assert_eq!(app.ui.dock_preview.target(&owner), Some(&first));
        assert!(app.client.state.transcripts.contains_key(&first));
        dock_hover(&mut app, &ctx, true, None);
        assert_eq!(app.ui.dock_preview.target(&owner), None);
        dock_hover(&mut app, &ctx, true, Some(second.clone()));
        assert_eq!(app.ui.dock_preview.target(&owner), Some(&second));
        assert_eq!(app.client.state.selected_thread(), Some(&owner));
        assert!(app.client.state.is_unseen(&first));
        dock_hover(&mut app, &ctx, false, None);
        dock_hover(&mut app, &ctx, true, Some(second));
        assert_eq!(app.ui.dock_preview.target(&owner), None);
    }
}
