//! D168: the agents pill with many agents — alive agents first, one "more" item, and the
//! docked panel's fold.

use yaac_core::agents::{
    AgentRow, AgentsPanel, DOCK_FOLD_MIN, DockEntry, Liveness, PILL_MAX, Rail, Relation, Role, SpawnedBy, clusters, docked_entries, more_label, pill, pill_cap,
};
use yaac_core::attention::{Mark, ThreadDisplay};

fn row(title: &str, role: Role, mark: Mark, updated: i64, depth: u8) -> AgentRow {
    AgentRow {
        role,
        kind: Some("claude".into()),
        last_action: String::new(),
        flow: None,
        finished: mark == Mark::Done,
        key: None,
        title: title.into(),
        provider: None,
        model: None,
        target: "local".into(),
        display: ThreadDisplay { mark, attention: None, read_only: false, unseen: false, fast: false },
        activity: String::new(),
        depth,
        git: None,
        origin: None,
        subagent: false,
        updated,
    }
}

fn panel(parent: Option<AgentRow>, workers: Vec<AgentRow>, contacts: Vec<AgentRow>) -> AgentsPanel {
    let direct_workers = workers.iter().filter(|w| w.depth == 0).count();
    AgentsPanel {
        budget: None,
        root: None,
        spawned_by: parent.map_or(SpawnedBy::You, |p| SpawnedBy::Agent(Box::new(p))),
        rail: Rail { count: direct_workers + contacts.len() },
        workers,
        direct_workers,
        contacts,
    }
}

fn titles(rows: &[&AgentRow]) -> Vec<String> {
    rows.iter().map(|r| r.title.clone()).collect()
}

/// An orchestrator with 30 workers: 1 needs you, 3 running, 2 waiting, 24 finished or idle.
fn thirty() -> AgentsPanel {
    let mut w = Vec::new();
    for i in 0..24 {
        let mark = if i % 3 == 0 { Mark::Idle } else { Mark::Done };
        w.push(row(&format!("done-{i:02}"), Role::Worker, mark, 1000 + i, 0));
    }
    w.insert(5, row("run-a", Role::Worker, Mark::Running, 10, 0));
    w.insert(9, row("wait-a", Role::Worker, Mark::Waiting, 50, 0));
    w.insert(12, row("run-b", Role::Worker, Mark::Running, 30, 0));
    w.insert(14, row("needs", Role::Worker, Mark::NeedsYou, 5, 0));
    w.insert(20, row("wait-b", Role::Worker, Mark::Waiting, 60, 0));
    w.push(row("run-c", Role::Worker, Mark::Running, 20, 0));
    panel(None, w, Vec::new())
}

#[test]
fn needs_you_then_running_then_waiting_then_finished_most_recent_first() {
    let p = panel(
        None,
        vec![
            row("old-done", Role::Worker, Mark::Done, 1, 0),
            row("idle", Role::Worker, Mark::Idle, 99, 0),
            row("run-old", Role::Worker, Mark::Running, 2, 0),
            row("waiting", Role::Worker, Mark::Waiting, 3, 0),
            row("new-done", Role::Worker, Mark::Done, 9, 0),
            row("failed", Role::Worker, Mark::Failed, 5, 0),
            row("run-new", Role::Worker, Mark::Running, 8, 0),
            row("needs", Role::Worker, Mark::NeedsYou, 0, 0),
            row("off", Role::Worker, Mark::Offline, 50, 0),
            row("paused", Role::Worker, Mark::Paused, 4, 0),
        ],
        Vec::new(),
    );
    let cl = clusters(&p);
    assert_eq!(titles(&cl[0].rows), ["paused", "needs", "run-new", "run-old", "waiting", "new-done", "failed", "old-done", "idle", "off"]);
    assert!(Liveness::Waiting.alive() && !Liveness::Finished.alive());
}

#[test]
fn unseen_finished_come_before_seen_ones() {
    let seen = row("seen", Role::Worker, Mark::Done, 9, 0);
    let mut unseen = row("unseen", Role::Worker, Mark::Done, 1, 0);
    unseen.display.unseen = true;
    let p = panel(None, vec![seen, unseen], Vec::new());
    assert_eq!(titles(&clusters(&p)[0].rows), ["unseen", "seen"]);
}

#[test]
fn a_small_fleet_draws_everyone() {
    let p = panel(None, (0..PILL_MAX as i64).map(|i| row(&format!("d{i}"), Role::Worker, Mark::Done, i, 0)).collect(), Vec::new());
    let pl = pill(&p, PILL_MAX);
    assert_eq!((pl.icons(), pl.more.hidden), (PILL_MAX, 0));
}

#[test]
fn thirty_agents_draw_the_alive_ones_and_count_the_rest_in_more() {
    let p = thirty();
    let pl = pill(&p, PILL_MAX);
    // Every alive agent (6 of a cap of 7) and nothing finished; the 24 others in "more".
    let shown: Vec<_> = pl.clusters.iter().flat_map(|c| titles(&c.rows)).collect();
    assert_eq!(shown, ["needs", "run-b", "run-c", "run-a", "wait-b", "wait-a"]);
    assert_eq!((pl.more.hidden, pl.more.alive, pl.more.needs_you), (24, 0, 0));
    assert_eq!(more_label(&pl.more), "more (24)");
    assert!(pl.shows(pl.clusters[0].rows[0]));
}

#[test]
fn when_alive_agents_alone_pass_the_cap_more_says_how_many_of_the_hidden_are_alive() {
    let p = thirty();
    let pl = pill(&p, 4);
    let shown: Vec<_> = pl.clusters.iter().flat_map(|c| titles(&c.rows)).collect();
    // The most urgent first: needs you, then running.
    assert_eq!(shown, ["needs", "run-b", "run-c", "run-a"]);
    assert_eq!((pl.more.hidden, pl.more.alive), (26, 2));
    assert_eq!(more_label(&pl.more), "more (26 · 2 alive)");
}

#[test]
fn the_parent_keeps_its_icon_and_clusters_stay_in_order() {
    let mut p = thirty();
    p.spawned_by = SpawnedBy::Agent(Box::new(row("parent", Role::Parent, Mark::Done, 0, 0)));
    p.contacts = vec![row("contact-done", Role::Contact, Mark::Done, 1, 0), row("contact-run", Role::Contact, Mark::Running, 100, 0)];
    let pl = pill(&p, PILL_MAX);
    let rel: Vec<_> = pl.clusters.iter().map(|c| c.relation).collect();
    assert_eq!(rel, [Relation::SpawnedBy, Relation::Workers, Relation::InContact]);
    assert_eq!(titles(&pl.clusters[0].rows), ["parent"]);
    // Seven icons: the parent, then the six most urgent across relations (the running contact
    // is more recent than every running worker, so it is in; the older waiting worker is not).
    assert_eq!(pl.icons(), PILL_MAX);
    assert_eq!(titles(&pl.clusters[2].rows), ["contact-run"]);
    assert_eq!(titles(&pl.clusters[1].rows), ["needs", "run-b", "run-c", "run-a", "wait-b"]);
    assert_eq!((pl.more.hidden, pl.more.alive), (26, 1));
    // Nothing alive but the parent: only the parent, and "more".
    let q = panel(Some(row("parent", Role::Parent, Mark::Done, 0, 0)), (0..12).map(|i| row(&format!("d{i}"), Role::Worker, Mark::Done, i, 0)).collect(), Vec::new());
    let pl = pill(&q, PILL_MAX);
    assert_eq!((pl.icons(), pl.more.hidden), (1, 12));
}

#[test]
fn the_cap_scales_with_the_room() {
    assert_eq!(pill_cap(1000.0, 32.0, 150.0), PILL_MAX);
    assert_eq!(pill_cap(150.0 + 32.0 * 4.5, 32.0, 150.0), 4);
    assert_eq!(pill_cap(10.0, 32.0, 150.0), 2);
}

#[test]
fn the_docked_panel_keeps_the_tree_alive_first_and_folds_the_finished() {
    let p = panel(
        None,
        vec![
            row("a-done", Role::Worker, Mark::Done, 1, 0),
            row("a-kid-run", Role::Worker, Mark::Running, 1, 1),
            row("b-done", Role::Worker, Mark::Done, 2, 0),
            row("c-run", Role::Worker, Mark::Running, 9, 0),
            row("c-kid-done", Role::Worker, Mark::Done, 1, 1),
            row("c-kid-wait", Role::Worker, Mark::Waiting, 1, 1),
            row("d-done", Role::Worker, Mark::Done, 3, 0),
            row("e-done", Role::Worker, Mark::Done, 4, 0),
            row("f-done", Role::Worker, Mark::Done, 5, 0),
        ],
        Vec::new(),
    );
    let cl = clusters(&p);
    let mut rows = Vec::new();
    let mut fold = None;
    for x in docked_entries(&cl[0], &p) {
        match x {
            DockEntry::Row(r) => rows.push(r.title.clone()),
            DockEntry::Fold { rows, word } => fold = Some((titles(&rows), word)),
        }
    }
    // Subtrees with someone alive first (by their most urgent agent: running before running
    // that is older), children under their parent and ordered the same way; the quiet ones
    // folded, most recent first.
    assert_eq!(rows, ["c-run", "c-kid-wait", "c-kid-done", "a-done", "a-kid-run"]);
    assert_eq!(fold, Some((vec!["f-done".to_owned(), "e-done".into(), "d-done".into(), "b-done".into()], "finished")));
    assert_eq!(DOCK_FOLD_MIN, 4);
    // Fewer than that: listed after the alive ones, not folded.
    let q = panel(None, vec![row("x-done", Role::Worker, Mark::Done, 1, 0), row("y-run", Role::Worker, Mark::Running, 1, 0)], Vec::new());
    let cl = clusters(&q);
    let e = docked_entries(&cl[0], &q);
    assert!(matches!(&e[..], [DockEntry::Row(a), DockEntry::Row(b)] if a.title == "y-run" && b.title == "x-done"));
}

mod common;

#[test]
fn the_demo_orchestrator_with_thirty_agents() {
    use yaac_core::agents::agents_panel;
    use yaac_core::attention::AttentionMap;
    use yaac_core::demo::{Scene, threads};
    let (mut c, hub) = common::boot();
    hub.play(Scene::ManyAgents);
    common::settle(&mut c, &hub);
    let map = AttentionMap::compute(&c.state);
    let p = agents_panel(&c.state, &map, &threads::orchestrator()).unwrap();
    assert_eq!(p.all().count(), 30);
    let pl = pill(&p, PILL_MAX);
    // Seven alive agents fill the cap; the 23 others are "more".
    assert_eq!((pl.icons(), pl.more.hidden, pl.more.alive), (7, 23, 0));
    assert!(pl.clusters.iter().flat_map(|c| c.rows.iter()).all(|r| Liveness::of(r).alive()));
    assert_eq!(pl.clusters[0].rows[0].title, "index-rebuild", "needs you first");
    // The docked Workers heading folds its finished subtrees.
    let cl = clusters(&p);
    let w = cl.iter().find(|c| c.relation == Relation::Workers).unwrap();
    let fold = docked_entries(w, &p).into_iter().find_map(|e| match e {
        DockEntry::Fold { rows, word } => Some((rows.len(), word)),
        DockEntry::Row(_) => None,
    });
    assert_eq!(fold, Some((17, "inactive")), "billing-service and 16 Codex subagents; one is stopped");
}
