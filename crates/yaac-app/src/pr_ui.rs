//! Pull requests in the window (D119, D120): the badge at the right of a row's first line,
//! its menu and its `+N` list, and the Create PR modal. The badge's state comes from
//! [`yaac_core::pr`]; this module only draws and turns clicks into [`Action`]s.
//!
//! The badge is an almost square, monochrome box: `PR#` and a number (blank: create), `+N`
//! when the branch has more. Hints, all in the secondary ink: a dashed outline for a draft,
//! a merge mark for merged, a struck-through number for closed; a small cross when checks
//! fail and a small dotted ring while they run (passing draws nothing).

use eframe::egui::{self, Align2, Margin, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, pos2, vec2};
use yaac_core::ThreadKey;
use yaac_core::pr::{AgentDraft, BadgeAction, BadgeClick, BadgeHint, PrBadge, Prepare, badge_menu, click, describe};
use yaac_protocol::ChecksState;

use crate::app::{Action, Menu, YaacApp};
use crate::icons;
use crate::theme::{self, Palette, size};
use crate::widgets;

/// The badge's height.
pub(crate) const BADGE_H: f32 = 17.0;
const PAD: f32 = 5.0;

fn font() -> egui::FontId {
    theme::ui(size::SMALL - 0.5)
}

/// Widths of the badge's parts: (label + number, `+N`).
fn widths(ui: &egui::Ui, b: &PrBadge) -> (f32, f32) {
    let w = |s: &str| ui.fonts_mut(|f| f.layout_no_wrap(s.to_owned(), font(), egui::Color32::WHITE)).size().x;
    // The number's room is the same with or without a number, so badges line up.
    let digits = w("00000").max(w(&b.number_text()));
    let hint = if b.checks.is_some() || b.hint == Some(BadgeHint::Merged) { 10.0 } else { 0.0 };
    let main = PAD + w("PR#") + 4.0 + digits + hint + PAD;
    let more = b.more_text().map_or(0.0, |m| w(&m) + PAD + 1.0);
    (main, more)
}

/// Draws the badge with its right edge at `right`, centred on `cy`; handles click,
/// right-click, and the `+N` hover list. Returns its left edge.
#[allow(clippy::too_many_arguments, reason = "a row's badge needs the row's place")]
pub(crate) fn badge(ui: &mut egui::Ui, app: &mut YaacApp, key: &ThreadKey, b: &PrBadge, right: f32, cy: f32, salt: &str, actions: &mut Vec<Action>) -> f32 {
    let p = app.p;
    let (main_w, more_w) = widths(ui, b);
    let left = right - main_w - more_w;
    let outer = Rect::from_min_max(pos2(left, cy - BADGE_H / 2.0), pos2(right, cy + BADGE_H / 2.0));
    let main = Rect::from_min_max(outer.min, pos2(left + main_w, outer.bottom()));
    let more = Rect::from_min_max(pos2(main.right(), outer.top()), outer.max);
    let id = egui::Id::unique(("pr-badge", salt, key.to_string()));
    let resp = ui.interact(main, id, Sense::click());
    let more_resp = (more_w > 0.0).then(|| ui.interact(more, id.with("more"), Sense::click()));
    let menu_open = app.ui.menu == Some(Menu::PrBadge(key.clone()));
    let hovered = resp.hovered() || more_resp.as_ref().is_some_and(egui::Response::hovered) || menu_open;
    let ink = if b.enabled { p.text2 } else { p.unavailable };
    let strong = if b.enabled { p.text } else { p.unavailable };
    if hovered && b.enabled {
        ui.painter().rect_filled(outer, 3.0, p.faint);
    }
    let stroke = Stroke::new(1.0, if hovered && b.enabled { p.text2 } else { p.border });
    if b.hint == Some(BadgeHint::Draft) {
        dashed_rect(ui.painter(), outer.shrink(0.5), Stroke::new(1.0, p.text2));
    } else {
        ui.painter().rect_stroke(outer, 3.0, stroke, StrokeKind::Inside);
    }
    let f = font();
    let label = ui.fonts_mut(|fs| fs.layout_no_wrap("PR#".to_owned(), f.clone(), ink));
    let mut x = main.left() + PAD;
    let lw = label.size().x;
    ui.painter().galley(pos2(x, cy - label.size().y / 2.0), label, ink);
    x += lw + 4.0;
    if !b.enabled {
        // Unavailable here (the CLI is missing or signed out): a small lock, the reason on hover.
        icons::lock(ui.painter(), pos2(x + 5.0, cy), 10.0, ink);
    } else if let Some(pr) = &b.primary {
        let dim = matches!(b.hint, Some(BadgeHint::Merged | BadgeHint::Closed));
        let col = if dim { ink } else { strong };
        let g = ui.fonts_mut(|fs| fs.layout_no_wrap(pr.number.to_string(), f.clone(), col));
        let gw = g.size().x;
        ui.painter().galley(pos2(x, cy - g.size().y / 2.0), g, col);
        if b.hint == Some(BadgeHint::Closed) {
            ui.painter().hline(x..=x + gw, cy, Stroke::new(1.0, ink));
        }
        x += gw + 3.0;
        if b.hint == Some(BadgeHint::Merged) {
            icons::fork(ui.painter(), pos2(x + 3.5, cy), 9.0, ink);
        } else {
            match b.checks {
                Some(ChecksState::Failing) => icons::close(ui.painter(), pos2(x + 3.5, cy), 8.0, ink),
                Some(ChecksState::Pending) => icons::dashed_circle(ui.painter(), pos2(x + 3.5, cy), 3.0, Stroke::new(1.0, ink)),
                _ => {}
            }
        }
    }
    if let Some(m) = b.more_text() {
        ui.painter().vline(more.left(), (outer.top() + 3.0)..=(outer.bottom() - 3.0), Stroke::new(1.0, p.border));
        let g = ui.fonts_mut(|fs| fs.layout_no_wrap(m, f.clone(), ink));
        ui.painter().galley(pos2(more.left() + (more.width() - g.size().x) / 2.0, cy - g.size().y / 2.0), g, ink);
    }

    // Click: the PR in the browser, or the create modal.
    let resp = resp.on_hover_text(b.tooltip());
    if resp.clicked() {
        match click(b) {
            BadgeClick::Open(url) => actions.push(Action::OpenUrl(url)),
            BadgeClick::Create => actions.push(Action::OpenCreatePr(key.clone())),
            BadgeClick::Nothing => {}
        }
    }
    if let Some(r) = &more_resp
        && r.clicked()
        && let Some(url) = b.primary.as_ref().map(|p| p.url.clone())
    {
        actions.push(Action::OpenUrl(url));
    }
    // Right-click: Create a new PR, Copy link (and the list is asked for again).
    if resp.secondary_clicked() || more_resp.as_ref().is_some_and(egui::Response::secondary_clicked) {
        app.ui.menu = Some(Menu::PrBadge(key.clone()));
        actions.push(Action::PrRefresh(key.clone()));
    }
    if app.ui.menu == Some(Menu::PrBadge(key.clone())) {
        let mut open = true;
        let mut picked = None;
        widgets::menu_popup(egui::Popup::from_response(&resp), &p)
            .kind(egui::PopupKind::Menu)
            .align(egui::RectAlign::BOTTOM_END)
            .open_bool(&mut open)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
            .show(|ui| {
                ui.set_min_width(160.0);
                for item in badge_menu(b) {
                    let kind = if item.enabled { widgets::ItemKind::Normal } else { widgets::ItemKind::Disabled };
                    let mut r = widgets::menu_item(ui, &p, item.label, None, kind);
                    if let Some(why) = &item.reason {
                        r = r.on_disabled_hover_text(why);
                    }
                    if r.clicked() {
                        picked = Some(item.action);
                    }
                }
            });
        match picked {
            Some(BadgeAction::Create) => actions.push(Action::OpenCreatePr(key.clone())),
            Some(BadgeAction::CopyLink) => {
                if let Some(pr) = &b.primary {
                    ui.ctx().copy_text(pr.url.clone());
                }
            }
            None => {}
        }
        if !open || picked.is_some() {
            app.ui.menu = None;
        }
    }
    // Hover on +N: the others, each clickable; the list stays while the pointer is on it.
    if let Some(r) = &more_resp {
        let slot = (key.clone(), salt.to_owned());
        let pointer = ui.ctx().pointer_hover_pos();
        let over_list = app.ui.pr_more.as_ref().is_some_and(|(s, rect)| *s == slot && pointer.is_some_and(|pt| rect.expand(4.0).contains(pt)));
        let forced = salt == "side" && app.ui.pr_more_force.as_ref() == Some(key);
        if r.hovered() || over_list || forced {
            let rect = more_list(ui.ctx(), &p, &b.others, Pos2::new(outer.left(), outer.bottom() + 3.0), &salt_id(key, salt), actions);
            app.ui.pr_more = Some((slot, rect));
        } else if app.ui.pr_more.as_ref().is_some_and(|(s, _)| *s == slot) {
            app.ui.pr_more = None;
        }
    }
    left
}

fn salt_id(key: &ThreadKey, salt: &str) -> String {
    format!("{salt}:{key}")
}

/// The `+N` list: one row per other pull request, `#n · state · checks — title`; clicking
/// opens it. Returns the list's rectangle.
fn more_list(ctx: &egui::Context, p: &Palette, others: &[yaac_protocol::PrSummary], top_left: Pos2, id: &str, actions: &mut Vec<Action>) -> Rect {
    let area = egui::Area::new(egui::Id::unique(("pr-more", id)))
        .order(egui::Order::Foreground)
        .pivot(Align2::LEFT_TOP)
        .fixed_pos(top_left)
        .show(ctx, |ui| {
            egui::Frame::popup(&ctx.global_style()).fill(p.raised).inner_margin(Margin::same(4)).show(ui, |ui| {
                ui.set_max_width(360.0);
                ui.label(widgets::secondary("Also from this branch", p));
                for pr in others {
                    let (r, rect) = widgets::menu_row(ui, p, false, true);
                    let text = format!("{} — {}", describe(pr), pr.title);
                    let g = widgets::elided(ui, &text, &theme::ui(size::SMALL), p.text, rect.width() - 16.0);
                    ui.painter().galley(pos2(rect.left() + 8.0, rect.center().y - g.size().y / 2.0), g, p.text);
                    if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        actions.push(Action::OpenUrl(pr.url.clone()));
                    }
                }
            });
        });
    area.response.rect
}

fn dashed_rect(painter: &egui::Painter, r: Rect, s: Stroke) {
    let pts = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
    painter.extend(egui::Shape::dashed_line(&pts, s, 3.0, 2.0));
}

// ── The Create PR modal (D120) ─────────────────────────────────────────────────────────

/// Target branch, title, description, Draft, "Ask the agent to write it", the push-first
/// notice, and Create / Cancel.
pub(crate) fn create_dialog(ctx: &egui::Context, app: &mut YaacApp, actions: &mut Vec<Action>) {
    let p = app.p;
    let Some(mut d) = app.client.state.create_pr.take() else { return };
    crate::dialogs::scrim(ctx, &p, "create-pr");
    let mut keep = true;
    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) && !d.creating {
        keep = false;
    }
    let ask_blocked = d.ask_blocked(&app.client.state);
    let title_of = app.client.state.summary(&d.key).map(|s| s.title.clone()).unwrap_or_default();
    egui::Window::new("Create pull request")
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .order(egui::Order::Foreground)
        .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
        .frame(egui::Frame::popup(&ctx.global_style()).fill(p.raised).inner_margin(Margin::same(20)))
        .show(ctx, |ui| {
            ui.set_width(560.0);
            ui.label(RichText::new("Create pull request").font(theme::bold(size::SECTION + 1.5)));
            ui.add_space(2.0);
            let from = d.ready().and_then(|r| r.branch.clone()).unwrap_or_default();
            let repo = d.ready().and_then(|r| r.access.repo.as_ref()).map(|r| r.slug.clone()).unwrap_or_default();
            ui.label(widgets::secondary(if from.is_empty() { title_of.clone() } else { format!("{from} · {repo}") }, &p));
            ui.add_space(12.0);
            if let Prepare::Failed(e) = &d.prepare {
                ui.label(RichText::new(e).color(p.failed));
                ui.add_space(8.0);
            }
            // Target branch.
            ui.horizontal(|ui| {
                ui.label(RichText::new("Into").color(p.text2));
                ui.add_space(6.0);
                let label = if d.base.is_empty() { "Choose a branch".to_owned() } else { d.base.clone() };
                let r = widgets::selector(ui, &p, &label, d.ready().is_some() && !d.creating);
                if r.clicked() {
                    d.base_open = !d.base_open;
                }
                if d.base_open {
                    let mut open = true;
                    widgets::menu_popup(egui::Popup::from_response(&r), &p)
                        .kind(egui::PopupKind::Menu)
                        .align(egui::RectAlign::BOTTOM_START)
                        .open_bool(&mut open)
                        .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
                        .show(|ui| {
                            ui.set_min_width(240.0);
                            egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                                let default = d.ready().and_then(|r| r.default_branch.clone());
                                for b in d.branches() {
                                    let (r, rect) = widgets::menu_row(ui, &p, b == d.base, true);
                                    let text = if Some(&b) == default.as_ref() { format!("{b}  (default)") } else { b.clone() };
                                    let g = widgets::elided(ui, &text, &theme::ui(size::UI), p.text, rect.width() - 16.0);
                                    ui.painter().galley(pos2(rect.left() + 8.0, rect.center().y - g.size().y / 2.0), g, p.text);
                                    if r.clicked() {
                                        actions.push(Action::PrBase(b));
                                    }
                                }
                            });
                        });
                    if !open {
                        d.base_open = false;
                    }
                }
            });
            ui.add_space(10.0);
            let writing = matches!(d.agent, Some(AgentDraft::Sending | AgentDraft::Writing(_)));
            ui.add_enabled_ui(!writing && !d.creating, |ui| {
                ui.label(RichText::new("Title").color(p.text2));
                let t = boxed(ui, &p, |ui| ui.add(egui::TextEdit::singleline(&mut d.title).frame(egui::Frame::NONE).desired_width(f32::INFINITY).hint_text("What this pull request does")));
                if t.changed() {
                    d.edited = true;
                }
                ui.add_space(8.0);
                ui.label(RichText::new("Description").color(p.text2));
                let b = boxed(ui, &p, |ui| ui.add(egui::TextEdit::multiline(&mut d.body).frame(egui::Frame::NONE).desired_width(f32::INFINITY).desired_rows(8).hint_text("Why, and what changed (markdown)")));
                if b.changed() {
                    d.edited = true;
                }
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.add_enabled_ui(!d.creating, |ui| ui.checkbox(&mut d.draft, "Draft"));
                ui.add_space(12.0);
                let ask = ui.add_enabled(ask_blocked.is_none(), egui::Button::new(RichText::new("Ask the agent to write it").color(p.primary)).frame(false));
                let ask = match &ask_blocked {
                    Some(why) => ask.on_disabled_hover_text(why),
                    None => ask.on_hover_text("Sends this thread's agent a message asking for a title and a description (it costs a turn); the reply fills them in."),
                };
                if ask.clicked() {
                    actions.push(Action::AskAgentPr);
                }
                match &d.agent {
                    Some(AgentDraft::Sending | AgentDraft::Writing(_)) => {
                        ui.add(egui::Spinner::new().size(12.0).color(p.text2));
                        ui.label(widgets::secondary("The agent is writing…", &p));
                    }
                    Some(AgentDraft::Failed(e)) => {
                        ui.label(RichText::new(e).color(p.failed).font(theme::ui(size::SMALL)));
                    }
                    None => {}
                }
            });
            ui.add_space(10.0);
            if let Some(n) = d.push_notice() {
                notice(ui, &p, &n, false);
            }
            if let Some(w) = d.uncommitted_warning() {
                notice(ui, &p, &w, true);
            }
            if let Some(e) = &d.error {
                ui.label(RichText::new(e).color(p.failed));
                ui.add_space(4.0);
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let blocked = d.create_blocked();
                let create = ui.add_enabled_ui(blocked.is_none(), |ui| widgets::primary(ui, &p, if d.creating { "Creating…" } else { "Create" }, None)).inner;
                let create = match &blocked {
                    Some(why) if !d.creating => create.on_disabled_hover_text(why),
                    _ => create,
                };
                if create.clicked() {
                    actions.push(Action::CreatePr);
                }
                if widgets::plain(ui, &p, "Cancel", Some("Esc")).clicked() && !d.creating {
                    keep = false;
                }
            });
        });
    if keep {
        app.client.state.create_pr = Some(d);
    }
}

/// A text field on a faint fill (controls carry no outline, D58).
fn boxed(ui: &mut egui::Ui, p: &Palette, add: impl FnOnce(&mut egui::Ui) -> egui::Response) -> egui::Response {
    egui::Frame::new().fill(p.faint).corner_radius(5.0).inner_margin(Margin::symmetric(8, 6)).show(ui, add).inner
}

fn notice(ui: &mut egui::Ui, p: &Palette, text: &str, warn: bool) {
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
        if warn {
            icons::warning(ui.painter(), r.center(), 14.0, p.needs, p.raised);
        } else {
            icons::info(ui.painter(), r.center(), 14.0, p.text2);
        }
        ui.label(widgets::secondary(text, p));
    });
    ui.add_space(2.0);
}
