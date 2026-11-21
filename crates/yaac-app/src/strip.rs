//! The collapsed sidebar: a narrow vertical strip (D94, redesigned by D103). Top to bottom:
//! the YAAC mark, expand, a new-thread button, back and forward side by side, then the list —
//! each section's name written vertically (reading bottom to top; a pin icon for *Pinned*),
//! one harness logo per thread with its state as a small badge on the logo's corner (the
//! red needs-you exclamation in place of the badge when the thread needs the user, a
//! cornflower ring when its last turn ended unseen), and a hairline
//! after each section's last thread — and Settings at the bottom. There is no *Needs you*
//! section. The wheel scrolls it. Hovering a thread shows a card with everything its full row
//! shows, with the agents card's no-flicker rule (`yaac_core::hover`).

use eframe::egui::{self, Rect, Sense, pos2, vec2};
use yaac_core::attention::AttentionMap;
use yaac_core::hover::Region;
use yaac_core::sidebar::{Open, sections};
use yaac_core::strip::{StripEntry, entries};

use crate::app::{Action, YaacApp};
use crate::icons;
use crate::theme::{self, size};

const CELL: f32 = 32.0;
const CARD_W: f32 = 300.0;

/// Draw the strip in `side`.
pub(crate) fn show(ui: &mut egui::Ui, app: &mut YaacApp, map: &AttentionMap, now: i64, side: Rect, actions: &mut Vec<Action>) {
    let p = app.p;
    let cx = side.center().x;
    let button = |ui: &mut egui::Ui, y: f32, id: &str, tip: &str| {
        let r = Rect::from_center_size(pos2(cx, y), vec2(CELL, CELL - 4.0));
        let resp = ui.interact(r, egui::Id::unique(("strip", id)), Sense::click()).on_hover_text(tip);
        if resp.hovered() || resp.has_focus() {
            ui.painter().rect_filled(r, 6.0, p.faint);
        }
        (r, resp)
    };
    // The mark.
    let g = ui.fonts_mut(|f| f.layout_no_wrap("Y".to_owned(), theme::bold(18.0), p.text));
    ui.painter().galley(pos2(cx - g.size().x / 2.0, side.top() + 12.0), g, p.text);
    let mut y = side.top() + 56.0;
    let (r, resp) = button(ui, y, "expand", "Expand the sidebar (Ctrl+B)");
    icons::sidebar(ui.painter(), r.center(), 16.0, p.text2);
    if resp.clicked() {
        app.ui.sidebar.toggle();
    }
    y += CELL;
    let (r, resp) = button(ui, y, "new", "New thread (Ctrl+N)");
    icons::plus(ui.painter(), r.center(), 16.0, p.text);
    if resp.clicked() {
        actions.push(Action::NewThread);
    }
    y += CELL - 12.0;
    // Back and forward on one row.
    ui.scope_builder(egui::UiBuilder::new().max_rect(Rect::from_min_size(pos2(cx - 20.0, y), vec2(40.0, 24.0))).id_salt("strip-nav"), |ui| {
        crate::nav::buttons(ui, app, actions, true);
    });
    y += 30.0;
    // The Needs-you button (D117), under the history buttons, while some thread needs the user.
    if !yaac_core::sidebar::needs_you(&app.client.state, map, now).is_empty() {
        ui.scope_builder(egui::UiBuilder::new().max_rect(Rect::from_min_size(pos2(cx - 20.0, y), vec2(44.0, 26.0))).id_salt("strip-needs"), |ui| {
            ui.horizontal(|ui| crate::sidebar::needs_button(ui, app, map, now, actions));
        });
        y += 30.0;
    }
    y += 4.0;
    // The list, scrolled by the wheel.
    let footer = 40.0;
    let list = Rect::from_min_max(pos2(side.left(), y), pos2(side.right(), side.bottom() - footer));
    let open = Open { muted: app.ui.muted_open, archived: app.ui.archived_open };
    let secs = sections(&app.client.state, map, app.ui.grouping, app.ui.ordering, "", open, now, app.zone);
    let list_entries = entries(&secs);
    let mut hovered: Option<(yaac_core::ThreadKey, Rect)> = None;
    ui.scope_builder(egui::UiBuilder::new().max_rect(list).id_salt("strip-list"), |ui| {
        ui.set_clip_rect(list);
        egui::ScrollArea::vertical().id_salt("strip-scroll").auto_shrink([false, false]).scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            for e in &list_entries {
                match e {
                    StripEntry::Separator => {
                        let (r, _) = ui.allocate_exact_size(vec2(list.width(), 9.0), Sense::hover());
                        let yy = ui.painter().round_to_pixel_center(r.center().y);
                        ui.painter().hline((cx - 12.0)..=(cx + 12.0), yy, egui::Stroke::new(1.0, p.border));
                    }
                    StripEntry::Label { pinned: true, .. } => {
                        let (r, resp) = ui.allocate_exact_size(vec2(list.width(), 22.0), Sense::hover());
                        icons::pin(ui.painter(), r.center(), 14.0, p.text2);
                        resp.on_hover_text("Pinned");
                    }
                    StripEntry::Label { text, collapsed, .. } => {
                        // The section's name, rotated to read bottom to top.
                        let g = crate::widgets::elided(ui, text, &theme::ui(size::SMALL - 0.5), p.text2, 110.0);
                        let len = g.size().x;
                        let (r, resp) = ui.allocate_exact_size(vec2(list.width(), len + 12.0), Sense::click());
                        let pos = pos2(cx - g.size().y / 2.0, r.bottom() - 6.0);
                        ui.painter().add(egui::epaint::TextShape::new(pos, g, p.text2).with_angle(-std::f32::consts::FRAC_PI_2));
                        if *collapsed && resp.on_hover_text(format!("{text} (collapsed)")).clicked() {
                            app.ui.sidebar.toggle();
                        }
                    }
                    StripEntry::Thread { key, mark, selected, needs_you, unseen, fast } => {
                        let (r, resp) = ui.allocate_exact_size(vec2(list.width(), CELL - 2.0), Sense::click());
                        let cell = Rect::from_center_size(r.center(), vec2(CELL, CELL - 4.0));
                        let under = if *selected {
                            ui.painter().rect_filled(cell, 6.0, p.selected);
                            p.selected
                        } else if resp.hovered() {
                            ui.painter().rect_filled(cell, 6.0, p.faint);
                            p.faint
                        } else {
                            p.panel
                        };
                        // The harness's logo, and the thread's state on its corner.
                        let provider = app.client.state.provider_of(key);
                        let kind = provider.map(|d| d.kind.as_str().to_owned());
                        let name = provider.map(|d| d.display_name.clone());
                        let c = cell.center() - vec2(1.5, 1.5);
                        crate::agents_ui::harness_mark(ui.painter(), c, 18.0, kind.as_deref(), name.as_deref(), false, &p);
                        let corner = c + vec2(7.5, 7.0);
                        if *unseen {
                            // The unseen ring around the logo (D114).
                            ui.painter().circle_stroke(c, 12.0, egui::Stroke::new(1.5, p.unseen));
                        }
                        if *needs_you {
                            ui.painter().circle_filled(corner, 6.5, under);
                            icons::exclaim(ui.painter(), corner, 11.5, p.failed);
                        } else {
                            ui.painter().circle_filled(corner, 5.5, under);
                            icons::mark_fast(ui.painter(), corner, 9.5, *mark, *fast, &p);
                        }
                        if resp.hovered() {
                            hovered = Some((key.clone(), cell));
                        }
                        // The card follows its icon as the strip scrolls.
                        if let Some((k, r)) = &mut app.ui.strip_hover_key
                            && k == key
                        {
                            *r = cell;
                        }
                        if resp.clicked() {
                            actions.push(Action::Open(key.clone()));
                        }
                    }
                }
            }
        });
    });
    let (r, resp) = button(ui, side.bottom() - 22.0, "settings", "Settings (Ctrl+,)");
    icons::gear(ui.painter(), r.center(), 17.0, p.text2);
    if resp.clicked() {
        actions.push(Action::Settings(true));
    }
    hover_card(ui, app, &secs, hovered.as_ref(), now, actions);
}

fn card_rect(icon: Rect) -> Rect {
    Rect::from_min_size(pos2(icon.right() + 10.0, icon.center().y - 34.0), vec2(CARD_W, 68.0))
}

/// The card beside a hovered icon: the thread's full row. Opens on the icon; stays while the
/// pointer is over the card or the icon; closes a quarter of a second after leaving both.
fn hover_card(ui: &mut egui::Ui, app: &mut YaacApp, secs: &[yaac_core::sidebar::Section], hovered: Option<&(yaac_core::ThreadKey, Rect)>, now: i64, actions: &mut Vec<Action>) {
    let ctx = ui.ctx().clone();
    if let Some((k, r)) = hovered
        && app.ui.strip_hover_key.as_ref().is_none_or(|(hk, _)| hk != k)
    {
        // A new icon: the card moves to it at once.
        app.ui.strip_hover_key = Some((k.clone(), *r));
        app.ui.strip_hover.open();
    }
    let Some((key, icon)) = app.ui.strip_hover_key.clone() else { return };
    let card = card_rect(icon);
    let region = |r: Rect| Region::new((r.left(), r.top()), (r.right(), r.bottom()));
    let pointer = ctx.pointer_hover_pos().map(|p| (p.x, p.y));
    let forced = app.shots.is_some();
    let step = if forced {
        yaac_core::hover::HoverStep { open: true, wake_in: None }
    } else {
        app.ui.strip_hover.step(pointer, region(icon), region(card), false, ctx.input(|i| i.time))
    };
    if let Some(s) = step.wake_in {
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(s));
    }
    if !step.open {
        app.ui.strip_hover_key = None;
        ctx.request_repaint();
        return;
    }
    let Some((si, ri)) = secs.iter().enumerate().find_map(|(si, s)| s.rows.iter().position(|r| r.key == key).map(|ri| (si, ri))) else { return };
    let p = app.p;
    egui::Area::new(egui::Id::unique("strip-card")).order(egui::Order::Foreground).fixed_pos(card.min).show(&ctx, |ui| {
        egui::Frame::popup(ui.style()).fill(p.raised).corner_radius(10).inner_margin(egui::Margin::same(6)).show(ui, |ui| {
            ui.set_width(CARD_W - 12.0);
            let h = if secs[si].rows[ri].line2.is_some() { 44.0 } else { 28.0 };
            let (r, _) = ui.allocate_exact_size(vec2(CARD_W - 12.0, h), Sense::hover());
            crate::sidebar::row(ui, app, &secs[si], ri, r, None, now, actions);
        });
    });
}
