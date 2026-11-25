use super::*;
use yaac_core::agents::{self, AgentRow, DockEntry, Flow, PanelMode, Relation, SpawnedBy};

fn row_id(row: &AgentRow) -> String {
    row.key.as_ref().map_or_else(
        || {
            format!(
                "peer:{}:{}:{}",
                row.target,
                row.provider.as_deref().unwrap_or_default(),
                row.title
            )
        },
        |key| key.address().to_string(),
    )
}

fn row(app: &YaacApp, row: &AgentRow, now: i64) -> Value {
    let git = row.git.as_ref().map(|git| {
        let changes = [
            (git.added, "+"), (git.modified, "~"), (git.deleted, "−"),
        ].into_iter().filter(|(n, _)| *n > 0).map(|(n, icon)| format!("{icon}{n}")).collect::<Vec<_>>().join(" ");
        json!({ "branch": git.branch, "changes": changes, "ahead": git.ahead, "behind": git.behind })
    });
    let logo = row.kind.as_deref().and_then(crate::logos::for_kind);
    let logo_color = logo
        .and_then(|(_, color)| color)
        .map(|c| format!("#{:02X}{:02X}{:02X}", c.r(), c.g(), c.b()));
    let link = row.open_key();
    let badge = link.and_then(|key| pull_requests::badge(&app.client.state, key));
    let actions = link.map_or_else(Vec::new, |key| sidebar_view::menu(app, key));
    json!({
        "id": row_id(row), "title": row.title, "canOpen": link.is_some(), "self": row.is_self, "provider": row.provider,
        "model": row.model, "target": row.target, "origin": row.origin, "subagent": row.subagent,
        "lastAction": row.last_action, "activity": row.activity, "age": row.activity_age(now), "mark": sidebar_view::tone(row.display.mark),
        "markLabel": row.display.mark.word(), "attention": row.display.attention.map(|a| a.word()),
        "unseen": row.display.unseen, "finished": row.finished, "readOnly": row.display.read_only,
        "fast": row.display.fast, "ultrafast": row.display.ultrafast, "git": git, "actions": actions,
        "logo": logo.map(|(logo, _)| crate::logos::source(logo).0), "logoColor": logo_color, "badge": badge,
        "flow": row.flow.map(|flow| match flow { Flow::Sending => "sending", Flow::Receiving => "receiving" }),
    })
}

pub(super) fn view(app: &YaacApp, map: &AttentionMap, ctx: Option<&egui::Context>) -> Value {
    let Some(thread) = selected_thread(app) else {
        return json!({ "visible": false });
    };
    let Some(panel) = agents_panel(&app.client.state, map, thread) else {
        return json!({ "visible": false, "threadId": thread.address() });
    };
    let now = app.now();
    let docked = app.ui.panel == PanelMode::Docked;
    let card_groups = agents::card_groups(&panel);
    if let Some(ctx) = ctx {
        if docked {
            crate::agents_ui::schedule_activity_ages(ctx, app, panel.all().chain(&panel.siblings), now);
        } else {
            crate::agents_ui::schedule_activity_ages(ctx, app, card_groups.iter().flat_map(|(_, rows)| rows.iter().copied()), now);
        }
    }
    let docked_clusters = agents::docked_clusters(&panel);
    let groups: Vec<_> = agents::clusters(&panel)
        .iter()
        .map(|cluster| {
            let (id, count, glyph) = match cluster.relation {
                Relation::SpawnedBy => ("spawnedBy", cluster.rows.len(), "↑"),
                Relation::Workers => ("workers", panel.direct_workers, "↓"),
                Relation::InContact => ("contacts", cluster.rows.len(), "↔"),
            };
            let mut rows = Vec::new();
            let mut folded = Vec::new();
            let mut fold_label = None;
            let docked = docked_clusters.iter().find(|c| c.relation == cluster.relation);
            for entry in docked.into_iter().flat_map(agents::docked_entries) {
                match entry {
                    DockEntry::Row(r) => rows.push(row_id(r)),
                    DockEntry::Fold { rows, word } => {
                        fold_label = Some(format!("{} {word}", rows.len()));
                        folded.extend(rows.into_iter().map(row_id));
                    }
                }
            }
            json!({
                "id": id, "label": cluster.relation.heading(), "glyph": glyph, "count": count,
                "rows": cluster.rows.iter().map(|r| row(app, r, now)).collect::<Vec<_>>(),
                "docked": { "rowIds": rows, "foldIds": folded, "foldLabel": fold_label,
                    "count": if cluster.relation == Relation::Workers { count } else { docked.map_or(0, |c| c.rows.len()) } },
            })
        })
        .collect();
    let pill = agents::pill(&panel, agents::PILL_MAX);
    let pill_rows: Vec<_> = pill
        .clusters
        .iter()
        .flat_map(|g| g.rows.iter())
        .map(|r| row_id(r))
        .collect();
    let first_hidden = agents::clusters(&panel)
        .into_iter()
        .flat_map(|g| g.rows)
        .find(|r| !pill.shows(r))
        .map(row_id);
    let budget = panel.budget.as_ref().map(|b| {
        if let (Some(ctx), Some(secs)) = (ctx, b.next_change(now)) {
            ctx.request_repaint_after(std::time::Duration::from_secs(secs));
        }
        // Both limits (P352); the duration's right-click menu sets or extends the time limit.
        let menu = b.time_menu(now);
        json!({
            "id": b.root, "text": b.text(now), "turns": b.turns_text(), "time": b.time_text(now),
            "exhausted": b.exhausted, "reached": b.reached(),
            "ranOut": b.ran_out().map(|r| match r { agents::RanOut::Turns => "turns", agents::RanOut::Time => "time", agents::RanOut::Unknown => "unknown" }),
            "action": b.exhausted.then(|| sidebar_view::action("continueBudget", "Continue", true)),
            "timeMenu": {
                "title": menu.title, "unavailable": menu.unavailable,
                "choices": menu.choices.iter().enumerate().map(|(i, c)| json!({
                    "index": i, "label": c.label, "detail": c.detail, "remove": c.wall_clock_secs.is_none(),
                })).collect::<Vec<_>>(),
                "customDefault": 2,
            },
        })
    });
    let root = panel.root.as_ref().map(|(title, key)| json!({ "id": key.as_ref().map(ThreadKey::address), "title": title, "canOpen": key.is_some() }));
    let can_mark_read_card = card_groups.iter().flat_map(|(_, rows)| rows).any(|r| r.display.unseen && r.key.is_some());
    let card_groups: Vec<_> = card_groups.into_iter().map(|(label, rows)| {
        json!({ "label": label, "rowIds": rows.into_iter().map(row_id).collect::<Vec<_>>() })
    }).collect();
    json!({
        "visible": !groups.is_empty() || budget.is_some() || matches!(panel.spawned_by, SpawnedBy::Agent(_)),
        "threadId": thread.address(), "mode": if docked { "docked" } else { "pill" },
        "pillPosition": app.ui.pill_position,
        "canDock": !docked, "canUndock": docked, "canMarkRead": panel.all().chain(&panel.siblings).any(|r| r.display.unseen && r.key.is_some()),
        "groups": groups, "budget": budget, "root": root,
        "cardGroups": card_groups, "canMarkReadCard": can_mark_read_card,
        "siblings": agents::sibling_rows(&panel).into_iter().map(|r| row(app, r, now)).collect::<Vec<_>>(),
        "pill": { "rowIds": pill_rows, "hidden": pill.more.hidden, "alive": pill.more.alive,
            "needsYou": pill.more.needs_you, "firstHiddenId": first_hidden },
    })
}

#[cfg(test)]
mod budget_tests {
    //! The budget line in the WPF view (P352): both limits, which ran out, the time-limit menu.
    use super::*;
    use yaac_core::demo::{Scene, threads};

    fn app(ctx: &egui::Context, scene: Scene) -> YaacApp {
        let mut app = crate::app::tests::demo_app(ctx);
        app.clock = crate::app::Clock::Story { base: app.now(), frozen: true };
        app.open(&threads::orchestrator(), None);
        if let Some(hub) = app.demo.clone() {
            hub.play(scene);
            hub.drain();
        }
        app.pump_demo();
        app
    }

    fn budget(app: &YaacApp) -> Value {
        view(app, &AttentionMap::compute(&app.client.state), None)["budget"].clone()
    }

    #[test]
    fn the_view_says_both_limits_and_which_ran_out() {
        let ctx = egui::Context::default();
        let app = app(&ctx, Scene::BudgetOutOfTime);
        let b = budget(&app);
        assert_eq!(b["turns"], "41 of 100 agent turns");
        assert_eq!(b["time"], "14 h since you acted (limit 12 h)");
        assert_eq!(b["reached"], "Paused: 12 h without you");
        assert_eq!(b["ranOut"], "time");
        assert_eq!(b["action"]["id"], "continueBudget");
        assert_eq!(b["timeMenu"]["title"], "Extend time limit…");
        assert_eq!(b["timeMenu"]["unavailable"], Value::Null);
        let labels: Vec<_> = b["timeMenu"]["choices"].as_array().unwrap().iter().map(|c| c["label"].as_str().unwrap().to_owned()).collect();
        assert_eq!(labels, ["+1 h", "+4 h", "+12 h", "+24 h", "Remove time limit"]);

        let app = self::app(&ctx, Scene::BudgetReached);
        let b = budget(&app);
        assert_eq!(b["reached"], "100 of 100 turns used");
        assert_eq!(b["ranOut"], "turns");
        assert_eq!(b["time"], "1 h since you acted · no time limit");
        assert_eq!(b["timeMenu"]["title"], "Set time limit…");
        assert!(b["timeMenu"]["unavailable"].is_string(), "this target's yaacd does not advertise budgetTimeLimit");
    }

    #[test]
    fn the_menu_sets_extends_and_removes_the_limit() {
        let ctx = egui::Context::default();
        let mut app = app(&ctx, Scene::BudgetOutOfTime);
        let root = threads::orchestrator().address().to_string();
        // +4 h from the 14 h already passed frees it.
        crate::wpf::dispatch(&mut app, &ctx, &mut None, "agents.timeLimit", &json!({ "id": root, "choice": 1 })).unwrap();
        app.pump_demo();
        let b = budget(&app);
        assert_eq!(b["time"], "14 h since you acted (limit 18 h)");
        assert_eq!(b["exhausted"], false);
        // Any number of hours.
        crate::wpf::dispatch(&mut app, &ctx, &mut None, "agents.timeLimit", &json!({ "id": root, "hours": 7 })).unwrap();
        app.pump_demo();
        assert_eq!(budget(&app)["time"], "14 h since you acted (limit 25 h)");
        // Nonsense is ignored: another root, no hours, too many.
        for payload in [json!({ "id": "local/other", "choice": 0 }), json!({ "id": root, "hours": 0 }), json!({ "id": root, "hours": 1_000_000 }), json!({ "id": root })] {
            crate::wpf::dispatch(&mut app, &ctx, &mut None, "agents.timeLimit", &payload).unwrap();
            app.pump_demo();
            assert_eq!(budget(&app)["time"], "14 h since you acted (limit 25 h)", "{payload}");
        }
        // Remove (the last choice).
        crate::wpf::dispatch(&mut app, &ctx, &mut None, "agents.timeLimit", &json!({ "id": root, "choice": 4 })).unwrap();
        app.pump_demo();
        assert_eq!(budget(&app)["time"], "14 h since you acted · no time limit");
    }
}
