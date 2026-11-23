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

fn row(app: &YaacApp, row: &AgentRow) -> Value {
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
    let badge = row.key.as_ref().and_then(|k| yaac_core::pr::badge_of(&app.client.state, k)).and_then(|b| {
        (b.primary.is_some() || b.can_create).then(|| {
            let label = if b.primary.is_some() { b.number_text() } else { "Create PR".to_owned() };
            json!({ "label": label, "detail": b.tooltip(), "action": sidebar_view::action("pullRequest", &label, b.enabled) })
        })
    });
    let actions = row
        .key
        .as_ref()
        .map_or_else(Vec::new, |key| sidebar_view::menu(app, key));
    json!({
        "id": row_id(row), "title": row.title, "canOpen": row.key.is_some(), "provider": row.provider,
        "model": row.model, "target": row.target, "origin": row.origin, "subagent": row.subagent,
        "lastAction": row.last_action, "activity": row.activity, "mark": sidebar_view::tone(row.display.mark),
        "markLabel": row.display.mark.word(), "attention": row.display.attention.map(|a| a.word()),
        "unseen": row.display.unseen, "finished": row.finished, "readOnly": row.display.read_only,
        "fast": row.display.fast, "ultrafast": row.display.ultrafast, "depth": row.depth, "git": git, "actions": actions,
        "logo": logo.map(|(logo, _)| crate::logos::source(logo).0), "logoColor": logo_color, "badge": badge,
        "flow": row.flow.map(|flow| match flow { Flow::Sending => "sending", Flow::Receiving => "receiving" }),
    })
}

pub(super) fn view(app: &YaacApp, map: &AttentionMap) -> Value {
    let Some(thread) = selected_thread(app) else {
        return json!({ "visible": false });
    };
    let Some(panel) = agents_panel(&app.client.state, map, thread) else {
        return json!({ "visible": false, "threadId": thread.address() });
    };
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
            for entry in agents::docked_entries(cluster, &panel) {
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
                "rows": cluster.rows.iter().map(|r| row(app, r)).collect::<Vec<_>>(),
                "docked": { "rowIds": rows, "foldIds": folded, "foldLabel": fold_label },
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
        json!({
            "id": b.root, "text": b.text, "exhausted": b.exhausted,
            "action": b.exhausted.then(|| sidebar_view::action("continueBudget", "Continue", true)),
        })
    });
    let root = panel.root.as_ref().map(|(title, key)| json!({ "id": key.as_ref().map(ThreadKey::address), "title": title, "canOpen": key.is_some() }));
    let docked = app.ui.panel == PanelMode::Docked;
    json!({
        "visible": !groups.is_empty() || budget.is_some() || matches!(panel.spawned_by, SpawnedBy::Agent(_)),
        "threadId": thread.address(), "mode": if docked { "docked" } else { "pill" },
        "canDock": !docked, "canUndock": docked, "canMarkRead": panel.all().any(|r| r.display.unseen && r.key.is_some()),
        "groups": groups, "budget": budget, "root": root,
        "pill": { "rowIds": pill_rows, "hidden": pill.more.hidden, "alive": pill.more.alive,
            "needsYou": pill.more.needs_you, "firstHiddenId": first_hidden },
    })
}
