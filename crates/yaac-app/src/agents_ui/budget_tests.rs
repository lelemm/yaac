//! The budget line in the real widget (P352), on a headless egui context with the demo fleet:
//! both limits, which one ran out, and the duration's right-click menu.

use egui::{self, Event, Pos2, RawInput, Rect, pos2, vec2};
use yaac_core::agents::{PanelMode, agents_panel};
use yaac_core::attention::AttentionMap;
use yaac_core::demo::{Scene, threads};

use crate::app::{Action, YaacApp};

fn app(ctx: &egui::Context, scene: Option<Scene>) -> YaacApp {
    let mut app = crate::app::tests::demo_app(ctx);
    app.clock = crate::app::Clock::Story { base: app.now(), frozen: true };
    app.open(&threads::orchestrator(), None);
    if let (Some(scene), Some(hub)) = (scene, app.demo.clone()) {
        hub.play(scene);
        hub.drain();
    }
    app.pump_demo();
    app
}

/// One frame of the card or the docked panel; what it said (AccessKit labels and their
/// rectangles) and what it asked for.
fn frame(ctx: &egui::Context, app: &mut YaacApp, events: Vec<Event>, t: f64) -> (Vec<(String, Rect)>, Vec<Action>) {
    ctx.enable_accesskit();
    let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 1000.0))), events, time: Some(t), ..Default::default() };
    let mut actions = Vec::new();
    let thread = Rect::from_min_max(pos2(280.0, 40.0), pos2(1600.0, 900.0));
    let mut out = ctx.run_ui(input, |ui| {
        let map = AttentionMap::compute(&app.client.state);
        let key = app.client.state.selected_thread().cloned().unwrap();
        let panel = agents_panel(&app.client.state, &map, &key).unwrap();
        if app.ui.panel == PanelMode::Docked {
            egui::CentralPanel::default().show(ui, |ui| super::docked(ui, app, &panel, &mut actions));
        } else {
            super::floating(ui.ctx(), app, thread, &panel, &mut actions);
        }
    });
    out.textures_delta.clear();
    let nodes = out.platform_output.accesskit_update.take().map(|u| u.nodes).unwrap_or_default();
    let said = nodes
        .iter()
        .filter_map(|(_, n)| {
            let text = n.label().or(n.value())?.to_owned();
            let b = n.bounds()?;
            #[allow(clippy::cast_possible_truncation, reason = "points on a test screen")]
            let r = Rect::from_min_max(pos2(b.x0 as f32, b.y0 as f32), pos2(b.x1 as f32, b.y1 as f32));
            Some((text, r))
        })
        .collect();
    (said, actions)
}

fn find<'a>(said: &'a [(String, Rect)], text: &str) -> Option<&'a Rect> {
    said.iter().find(|(t, _)| t == text).map(|(_, r)| r)
}

fn texts(said: &[(String, Rect)]) -> Vec<&str> {
    said.iter().map(|(t, _)| t.as_str()).collect()
}

fn click(at: Pos2, button: egui::PointerButton) -> Vec<Event> {
    let b = |pressed| Event::PointerButton { pos: at, button, pressed, modifiers: egui::Modifiers::default() };
    vec![Event::PointerMoved(at), b(true), b(false)]
}

#[test]
fn the_card_shows_both_limits_and_which_ran_out() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, Some(Scene::BudgetOutOfTime));
    app.ui.panel = PanelMode::Card;
    let mut said = Vec::new();
    for i in 0..3 {
        said = frame(&ctx, &mut app, vec![], f64::from(i) / 10.0).0;
    }
    for line in ["41 of 100 agent turns", "14 h since you acted (limit 12 h)", "Paused: 12 h without you ·", "Continue"] {
        assert!(find(&said, line).is_some(), "{line:?} in {:?}", texts(&said));
    }
}

#[test]
fn out_of_turns_says_so_in_the_docked_panel() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, Some(Scene::BudgetReached));
    app.ui.panel = PanelMode::Docked;
    let mut said = Vec::new();
    for i in 0..3 {
        said = frame(&ctx, &mut app, vec![], f64::from(i) / 10.0).0;
    }
    for line in ["100 of 100 agent turns", "1 h since you acted · no time limit", "100 of 100 turns used ·"] {
        assert!(find(&said, line).is_some(), "{line:?} in {:?}", texts(&said));
    }
}

#[test]
fn right_click_on_the_duration_extends_the_limit() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, Some(Scene::BudgetTimeLimit));
    app.ui.panel = PanelMode::Docked;
    let mut said = Vec::new();
    for i in 0..3 {
        said = frame(&ctx, &mut app, vec![], f64::from(i) / 10.0).0;
    }
    let duration = *find(&said, "3 h since you acted (limit 12 h)").unwrap_or_else(|| panic!("{:?}", texts(&said)));
    frame(&ctx, &mut app, click(duration.center(), egui::PointerButton::Secondary), 0.5);
    said = frame(&ctx, &mut app, vec![], 0.6).0;
    assert!(find(&said, "Extend time limit…").is_some(), "the menu opened: {:?}", texts(&said));
    assert!(find(&said, "Remove time limit").is_some());
    let plus4 = *said.iter().find(|(t, _)| t.starts_with("+4 h")).map(|(_, r)| r).unwrap_or_else(|| panic!("{:?}", texts(&said)));
    let (_, actions) = frame(&ctx, &mut app, click(plus4.center(), egui::PointerButton::Primary), 0.7);
    let root = threads::orchestrator().address();
    assert!(
        actions.iter().any(|a| matches!(a, Action::SetTimeLimit(r, Some(secs)) if *r == root && *secs == 16 * 3600)),
        "+4 h on top of 12 h: {actions:?}"
    );
    // Not a person acting: Continue is not what it sends.
    assert!(!actions.iter().any(|a| matches!(a, Action::ContinueBudget(_))));
}
