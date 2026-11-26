//! The timeline at the left edge of the transcript (D153): a narrow column of short muted
//! dashes, one per prompt, the ones in view longer and brighter. Hovering a dash shows a small
//! card with that prompt's first line and time; clicking it scrolls there. Nothing here
//! animates: the column is redrawn with the transcript, which repaints only on a cause
//! (a scroll, the pointer, new items).
//!
//! What to draw comes from `yaac_core::timeline`; this module only paints and hit-tests.

use egui::{self, Rect, Sense, pos2, vec2};
use yaac_core::ThreadKey;
use yaac_core::time::Zone;
use yaac_core::timeline::{self as tl, Entry};
use yaac_protocol::{ItemId, TimelinePrompt};

use crate::app::YaacApp;
use crate::theme::{self, Palette, size};

/// The column's width, from the transcript's left edge.
pub(crate) const COLUMN: f32 = 24.0;
/// Room kept free above and below the dashes.
const PAD: f32 = 28.0;
/// A dash's length, and a lit one's.
const DASH: f32 = 7.0;
const DASH_LIT: f32 = 12.0;

/// The thread, each prompt with its top in the transcript, and the view's top, as last drawn.
pub(crate) type Drawn = (ThreadKey, Vec<(ItemId, f32)>, f32);

/// The timeline's view state.
#[derive(Debug, Default)]
pub(crate) struct TimelineUi {
    /// As last drawn: the thread, each prompt with its top in the transcript, and the view's
    /// top — what "Go to previous / next message" works from.
    pub last: Option<Drawn>,
    /// Scroll the transcript to this prompt on the next frame.
    pub go_to: Option<ItemId>,
    /// The prompt last gone to, and how many more frames to aim at it while heights settle.
    pub settle: Option<(ItemId, u8)>,
    /// Screenshots: show this dash's card as if it were hovered.
    pub force_hover: Option<usize>,
    /// Screenshots and tests: show this message's time as if it were hovered.
    pub force_stamp: Option<ItemId>,
    /// The dashes as last drawn, in screen coordinates (tests click them).
    pub dashes: Vec<Rect>,
    /// An expanded dash when several prompts share one mark.
    group: Option<(usize, ItemId)>,
}

/// "Go to previous / next message" (Alt+PageUp / Alt+PageDown): the prompt before or after the
/// view, from what was last drawn.
pub(crate) fn go(app: &mut YaacApp, key: &ThreadKey, forward: bool) {
    let Some((k, entries, view_top)) = &app.ui.timeline.last else { return };
    if k != key {
        return;
    }
    let tops: Vec<f32> = entries.iter().map(|(_, t)| *t).collect();
    let to = if forward { tl::next(&tops, *view_top) } else { tl::prev(&tops, *view_top) };
    if let Some(i) = to {
        app.ui.timeline.go_to = Some(entries[i].0.clone());
    } else if let Some(t) = app.client.state.transcripts.get(key)
        && let Some(prompts) = &t.prompts
    {
        let edge = if forward { entries.last() } else { entries.first() };
        let index = edge.and_then(|(id, _)| prompts.iter().position(|p| &p.item_id == id));
        let next = match (index, forward) {
            (Some(i), true) => prompts.get(i + 1),
            (Some(i), false) => i.checked_sub(1).and_then(|j| prompts.get(j)),
            (None, false) if !t.history_away => prompts.last(),
            _ => None,
        };
        if let Some(item) = next.map(|p| p.item_id.clone()) {
            app.ui.timeline.go_to = Some(item.clone());
            app.client.focus_history_item(key, item);
        }
    }
}

/// Draw the column at the left of `area` (the transcript's visible rectangle). `tops` are the
/// prompts' tops in the transcript; `view` its visible range. Returns the prompt clicked.
#[allow(clippy::too_many_arguments, reason = "the drawing context of one column")]
pub(crate) fn show(ui: &mut egui::Ui, ui_state: &mut TimelineUi, p: &Palette, area: Rect, entries: &[Entry], index: Option<&[TimelinePrompt]>, in_view: std::ops::Range<usize>, now: i64, zone: Zone) -> Option<ItemId> {
    ui_state.dashes.clear();
    let col = Rect::from_min_max(pos2(area.left() + 4.0, area.top() + PAD), pos2(area.left() + COLUMN, area.bottom() - PAD));
    let layout = tl::layout(index.map_or(entries.len(), <[TimelinePrompt]>::len), col.height());
    if layout.dashes.is_empty() {
        ui_state.group = None;
        return None;
    }
    let prompt = |i: usize| {
        if let Some(index) = index {
            index.get(i).map(|p: &TimelinePrompt| (&p.item_id, p.title.as_str(), p.at.as_ref().and_then(yaac_core::time::secs)))
        } else {
            entries.get(i).map(|e| (&e.item, e.title.as_str(), e.at))
        }
    };
    let resp = ui.interact(col, ui.scope_id().with("timeline"), Sense::click());
    let hovered = resp.hover_pos().and_then(|pos| tl::dash_at(&layout, pos.y - col.top())).or(ui_state.force_hover);
    if hovered.is_some() && resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let painter = ui.painter_at(col.expand(2.0));
    let x = col.left() + 4.0;
    for (i, d) in layout.dashes.iter().enumerate() {
        let lit = tl::lit(d, &in_view);
        let hot = hovered == Some(i);
        let (len, color, w) = match (lit, hot) {
            (_, true) => (DASH_LIT, p.text, 2.0),
            (true, false) => (DASH_LIT, p.text, 1.5),
            (false, false) => (DASH, p.text2.gamma_multiply(0.55), 1.5),
        };
        let y = col.top() + d.y;
        painter.hline(x..=x + len, y, egui::Stroke::new(w, color));
        ui_state.dashes.push(Rect::from_center_size(pos2(x + DASH_LIT / 2.0, y), vec2(DASH_LIT + 4.0, layout.pitch.max(3.0))));
    }
    let mut clicked = None;
    if let Some(i) = hovered
        && let Some(d) = layout.dashes.get(i)
        && let Some((id, title, at)) = prompt(d.first)
    {
        if resp.clicked() {
            if d.first == d.last {
                clicked = Some(id.clone());
                ui_state.group = None;
            } else {
                ui_state.group = Some((i, id.clone()));
            }
        }
        if ui_state.group.is_none() {
            card(ui, p, pos2(col.right() + 6.0, col.top() + d.y), title, at, d.last - d.first, now, zone);
        }
    }
    if let Some((i, id)) = ui_state.group.as_ref()
        && let Some(d) = layout.dashes.get(*i)
        && prompt(d.first).is_some_and(|(first, _, _)| first == id)
    {
        let mut open = true;
        crate::widgets::menu_popup(egui::Popup::from_response(&resp), p)
            .anchor(ui_state.dashes[*i])
            .align(egui::RectAlign::RIGHT_START)
            .open_bool(&mut open)
            .show(|ui| {
                ui.set_width(300.0);
                egui::ScrollArea::vertical().max_height(288.0).show_rows(ui, 24.0, d.last - d.first + 1, |ui, range| {
                    for i in range {
                        if let Some((id, title, _)) = prompt(d.first + i)
                            && ui.add(egui::Button::new(title).truncate()).clicked()
                        {
                            clicked = Some(id.clone());
                        }
                    }
                });
            });
        if !open || clicked.is_some() {
            ui_state.group = None;
        }
    } else {
        ui_state.group = None;
    }
    clicked
}

/// The hover card: the prompt's first line and when it was sent; "and N more" for a dash
/// that stands for several.
#[allow(clippy::too_many_arguments, reason = "the drawing context of one card")]
fn card(ui: &egui::Ui, p: &Palette, at: egui::Pos2, title: &str, stamp: Option<i64>, more: usize, now: i64, zone: Zone) {
    let ctx = ui.ctx().clone();
    egui::Area::new(egui::Id::unique("timeline-card")).order(egui::Order::Tooltip).fixed_pos(pos2(at.x, at.y - 24.0)).interactable(false).show(&ctx, |ui| {
        egui::Frame::popup(ui.style()).fill(p.raised).corner_radius(8).inner_margin(egui::Margin::symmetric(10, 7)).show(ui, |ui| {
            ui.set_max_width(320.0);
            let g = crate::widgets::elided(ui, title, &theme::ui(size::UI), p.text, 300.0);
            let (r, _) = ui.allocate_exact_size(g.size(), Sense::hover());
            ui.painter().galley(r.min, g, p.text);
            let mut sub = stamp.map(|a| yaac_core::time::message_time(now, a, zone)).unwrap_or_default();
            if more > 0 {
                if !sub.is_empty() {
                    sub.push_str(" · ");
                }
                sub.push_str(&format!("and {more} more"));
            }
            if !sub.is_empty() {
                ui.label(egui::RichText::new(sub).font(theme::ui(size::SMALL)).color(p.text2));
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use egui::{self, Event, PointerButton, Pos2, Rect, pos2, vec2};
    use yaac_core::attention::AttentionMap;
    use yaac_core::demo::{Scene, threads};

    use crate::app::YaacApp;

    const NOW: i64 = crate::app::STORY_NOW;

    /// The long conversation of screens 100–104, open.
    fn app(ctx: &egui::Context) -> YaacApp {
        let mut app = crate::app::tests::demo_app(ctx);
        if let Some(h) = &app.demo {
            h.play(Scene::LongConversation);
            h.drain();
        }
        app.pump_demo();
        app.open(&threads::parser(), None);
        for _ in 0..20 {
            app.pump_demo();
        }
        app
    }

    /// One frame of the transcript in a 1000 × 800 window; returns every text drawn, with
    /// where.
    fn frame(app: &mut YaacApp, ctx: &egui::Context, events: Vec<Event>) -> Vec<(String, Rect)> {
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 800.0))), events, ..Default::default() };
        let key = threads::parser();
        let mut actions = Vec::new();
        let mut out = ctx.run_ui(input, |ui| {
            let map = AttentionMap::compute(&app.client.state);
            crate::thread::transcript(ui, app, &map, &key, NOW, &mut actions);
        });
        app.apply(actions);
        app.pump_demo();
        out.textures_delta.clear();
        let mut texts = Vec::new();
        for c in std::mem::take(&mut out.shapes) {
            if let egui::epaint::Shape::Text(t) = c.shape {
                texts.push((t.galley.text().to_owned(), t.visual_bounding_rect()));
            }
        }
        texts
    }

    fn settle(app: &mut YaacApp, ctx: &egui::Context) -> Vec<(String, Rect)> {
        let mut last = Vec::new();
        for _ in 0..8 {
            last = frame(app, ctx, vec![]);
        }
        last
    }

    fn click(app: &mut YaacApp, ctx: &egui::Context, at: Pos2) {
        let press = |pressed| Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        frame(app, ctx, vec![Event::PointerMoved(at)]);
        frame(app, ctx, vec![press(true)]);
        frame(app, ctx, vec![press(false)]);
    }

    fn prompt_top(app: &YaacApp, id: &yaac_protocol::ItemId) -> Option<f32> {
        let (_, entries, view_top) = app.ui.timeline.last.as_ref().expect("drawn");
        entries.iter().find(|(item, _)| item == id).map(|(_, top)| top - view_top)
    }

    fn view_tops(app: &YaacApp) -> Vec<f32> {
        let (_, entries, view_top) = app.ui.timeline.last.as_ref().unwrap();
        entries.iter().map(|(_, top)| top - view_top).collect()
    }

    #[test]
    fn clicking_a_dash_scrolls_to_its_prompt() {
        let ctx = egui::Context::default();
        let mut app = app(&ctx);
        settle(&mut app, &ctx);
        let dashes = app.ui.timeline.dashes.clone();
        let key = threads::parser();
        let transcript = &app.client.state.transcripts[&key];
        let prompts = transcript.prompts.as_ref().unwrap();
        let index = prompts.windows(2).rposition(|pair| transcript.item(&pair[0].item_id).is_none()).expect("an unloaded prompt");
        let selected = prompts[index].item_id.clone();
        let next = prompts[index + 1].item_id.clone();
        assert_eq!(dashes.len(), prompts.len());
        assert!(transcript.item(&selected).is_none(), "the dash exists before its message is loaded");
        let items = transcript.items();
        let oldest_turn = items.iter().take_while(|item| item.turn_id == items[0].turn_id).count();
        assert!(items.len() - oldest_turn < 120, "only the boundary turn can extend the page");

        click(&mut app, &ctx, dashes[index].center());
        settle(&mut app, &ctx);
        let at = prompt_top(&app, &selected).expect("the position was loaded");
        assert!((at - yaac_core::timeline::MARGIN).abs() < 5.0, "prompt at the top of the view: {at}");
        assert!(app.demo.as_ref().unwrap().requests().iter().any(|(_, method, params)| method == "thread/read" && params["around"] == selected.as_str()));

        crate::timeline::go(&mut app, &key, true);
        settle(&mut app, &ctx);
        assert!((prompt_top(&app, &next).unwrap() - yaac_core::timeline::MARGIN).abs() < 2.0);
        crate::timeline::go(&mut app, &key, false);
        settle(&mut app, &ctx);
        assert!((prompt_top(&app, &selected).unwrap() - yaac_core::timeline::MARGIN).abs() < 5.0);
    }

    #[test]
    fn hovering_a_message_shows_its_time_without_moving_anything() {
        let ctx = egui::Context::default();
        let mut app = app(&ctx);
        let texts = settle(&mut app, &ctx);
        let tops = view_tops(&app);
        let (_, bubble) = texts.iter().find(|(s, _)| s == "Merge it when CI is green.").expect("the last prompt").clone();
        // Away from it: no time under it.
        let away = frame(&mut app, &ctx, vec![Event::PointerMoved(pos2(500.0, 5.0))]);
        let time = yaac_core::time::message_time(NOW, NOW - 7 * 60, app.zone);
        assert!(!away.iter().any(|(s, _)| *s == time), "{time} shown without hover");
        let over = frame(&mut app, &ctx, vec![Event::PointerMoved(bubble.center())]);
        let shown = frame(&mut app, &ctx, vec![Event::PointerMoved(bubble.center())]);
        let stamp = shown.iter().chain(&over).find(|(s, _)| *s == time).map(|(_, r)| *r).expect("the time under the hovered message");
        assert!(stamp.top() >= bubble.bottom(), "under the message");
        assert!((stamp.right() - bubble.right()).abs() < 24.0, "under the bubble's right edge");
        // The same text, in the same places.
        assert_eq!(view_tops(&app), tops);
        let (_, again) = shown.iter().find(|(s, _)| s == "Merge it when CI is green.").expect("still there");
        assert_eq!(*again, bubble);
    }

    #[test]
    fn a_finished_turn_shows_worked_for_and_opens_on_click() {
        let ctx = egui::Context::default();
        let mut app = app(&ctx);
        let texts = settle(&mut app, &ctx);
        // The newest turn: one line and its answer; its commands are hidden.
        let (_, line) = texts.iter().rev().find(|(s, _)| s.starts_with("Worked for")).expect("a Worked for line").clone();
        assert!(texts.iter().any(|(s, _)| s == "Merged #128."));
        let commands = |t: &[(String, Rect)]| t.iter().filter(|(s, _)| s.starts_with("Ran cargo test")).count();
        assert_eq!(commands(&texts), 0, "the work is folded");
        click(&mut app, &ctx, line.center());
        let opened = settle(&mut app, &ctx);
        assert!(app.ui.worked.is_open(&threads::parser(), &"p-long-31".into()));
        assert!(commands(&opened) >= 1, "the work shows again");
        assert!(opened.iter().any(|(s, _)| s == "Merged #128."), "the answer is still there");
    }
}
