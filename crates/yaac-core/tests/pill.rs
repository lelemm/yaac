//! D168: the agents pill with many agents — alive agents first, one "more" item, and the
//! docked panel's fold.

use yaac_core::agents::{
    AgentRow, AgentsPanel, DOCK_FOLD_MIN, DockEntry, Liveness, PILL_MAX, Rail, Relation, Role, SpawnedBy, card_groups, clusters, docked_entries, more_label, pill, pill_cap,
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
        display: ThreadDisplay { mark, attention: None, read_only: false, unseen: false, fast: false, ultrafast: false },
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
        siblings: Vec::new(),
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
fn the_pill_orders_needs_you_then_running_then_waiting_then_finished() {
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
    let layout = pill(&p, p.workers.len());
    assert_eq!(titles(&layout.clusters[0].rows), ["paused", "needs", "run-new", "run-old", "waiting", "new-done", "failed", "old-done", "idle", "off"]);
    assert!(Liveness::Waiting.alive() && !Liveness::Finished.alive());
}

#[test]
fn unseen_finished_come_before_seen_ones() {
    let seen = row("seen", Role::Worker, Mark::Done, 9, 0);
    let mut unseen = row("unseen", Role::Worker, Mark::Done, 1, 0);
    unseen.display.unseen = true;
    let p = panel(None, vec![seen, unseen], Vec::new());
    assert_eq!(titles(&pill(&p, PILL_MAX).clusters[0].rows), ["unseen", "seen"]);
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
fn related_agent_rows_use_activity_order_and_fold_only_the_inactive_tail() {
    let p = panel(
        None,
        vec![
            row("older running", Role::Worker, Mark::Running, 6, 1),
            row("oldest finished", Role::Worker, Mark::Done, 1, 0),
            row("recent finished", Role::Worker, Mark::Done, 9, 0),
            row("older finished", Role::Worker, Mark::Done, 2, 0),
            row("recent running", Role::Worker, Mark::Running, 8, 0),
            row("finished", Role::Worker, Mark::Done, 3, 1),
            row("inactive", Role::Worker, Mark::Idle, 4, 0),
        ],
        Vec::new(),
    );
    let cl = clusters(&p);
    assert_eq!(titles(&cl[0].rows), [
        "recent running", "older running", "inactive", "recent finished",
        "finished", "older finished", "oldest finished",
    ]);
    assert_eq!(cl[0].rows[3].activity_age(70).as_deref(), Some("1m"));
    assert_eq!(cl[0].rows[0].activity_age(8).as_deref(), Some("now"));
    let mut rows = Vec::new();
    let mut fold = None;
    for entry in docked_entries(&cl[0]) {
        match entry {
            DockEntry::Row(row) => rows.push(row.title.as_str()),
            DockEntry::Fold { rows, word } => fold = Some((titles(&rows), word)),
        }
    }
    assert_eq!(rows, ["recent running", "older running"]);
    assert_eq!(fold, Some((vec!["inactive".to_owned(), "recent finished".into(), "finished".into(), "older finished".into(), "oldest finished".into()], "inactive")));
    assert_eq!(DOCK_FOLD_MIN, 4);
}

#[test]
fn expanded_groups_move_a_completed_agent_to_the_read_tier_without_changing_recency() {
    let mut older_unread = row("older unread", Role::Worker, Mark::Done, 10, 0);
    let mut newer_unread = row("newer unread", Role::Worker, Mark::Done, 80, 0);
    older_unread.display.unseen = true;
    newer_unread.display.unseen = true;
    let mut p = panel(
        None,
        vec![
            newer_unread,
            row("read", Role::Worker, Mark::Done, 50, 0),
            row("running", Role::Worker, Mark::Running, 95, 0),
            older_unread,
            row("idle", Role::Worker, Mark::Idle, 85, 0),
            row("needs attention", Role::Worker, Mark::NeedsYou, 90, 0),
        ],
        Vec::new(),
    );
    p.siblings = p.workers.clone();
    let expected = ["newer unread", "older unread", "running", "needs attention", "idle", "read"];
    assert_eq!(titles(&clusters(&p)[0].rows), expected);
    assert_eq!(card_groups(&p)[0].0, "Siblings");
    assert_eq!(titles(&card_groups(&p)[0].1), expected);

    p.workers[0].display.unseen = false;
    p.siblings[0].display.unseen = false;
    let expected = ["older unread", "running", "needs attention", "idle", "newer unread", "read"];
    assert_eq!(titles(&clusters(&p)[0].rows), expected);
    assert_eq!(titles(&card_groups(&p)[0].1), expected);
    assert_eq!(titles(&pill(&p, PILL_MAX).clusters[0].rows), [
        "needs attention", "running", "older unread", "newer unread", "read", "idle",
    ]);
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
    // Seven of the eight alive agents fill the cap; the 23 others are "more", one of them
    // alive (users-service, waiting on its subagents, D188).
    assert_eq!((pl.icons(), pl.more.hidden, pl.more.alive), (7, 23, 1));
    assert!(pl.clusters.iter().flat_map(|c| c.rows.iter()).all(|r| Liveness::of(r).alive()));
    assert_eq!(pl.clusters[0].rows[0].title, "index-rebuild", "needs you first");
    let clusters = clusters(&p);
    let workers = clusters.iter().find(|c| c.relation == Relation::Workers).unwrap();
    let docked: Vec<_> = docked_entries(workers).into_iter().flat_map(|entry| match entry {
        DockEntry::Row(row) => vec![row],
        DockEntry::Fold { rows, .. } => rows,
    }).collect();
    assert_eq!(docked, workers.rows);
    // Completed read agents go after all non-completed rows, even when a completion is newer.
    let first_read_completion = docked.iter().position(|row| row.display.mark == Mark::Done && !row.display.unseen).unwrap();
    assert!(docked[first_read_completion..].iter().all(|row| row.display.mark == Mark::Done && !row.display.unseen));
    assert!(docked[first_read_completion..].windows(2).all(|pair| pair[0].updated >= pair[1].updated));
}

/// D188: a thread whose turn ended while its harness subagents still work shows as waiting
/// (the hourglass and "Waiting"), not idle with no mark, wherever it is drawn.
#[test]
fn a_parent_waiting_on_its_harness_subagents_shows_as_waiting() {
    use yaac_core::attention::{AttentionMap, display};
    use yaac_core::demo::{Scene, threads};
    use yaac_core::header::header;
    let (mut c, hub) = common::boot();
    hub.play(Scene::ManyAgents);
    common::settle(&mut c, &hub);
    let users = threads::users();
    let running = c.state.threads.keys().filter(|k| c.state.summary(k).is_some_and(|s| {
        s.subagent.as_ref().is_some_and(|a| a.status == yaac_protocol::SubagentStatus::Running) && s.spawned_by.as_ref() == Some(&users.address())
    })).count();
    assert!(running > 0, "users-service has running subagents");
    let map = AttentionMap::compute(&c.state);
    assert_eq!(display(&c.state, &map, &users).mark, Mark::Waiting);
    assert_eq!(header(&c.state, &map, &users).unwrap().tag.map(|t| t.0).as_deref(), Some("Waiting"));
    let p = yaac_core::agents::agents_panel(&c.state, &map, &threads::orchestrator()).unwrap();
    let row = p.all().find(|r| r.key.as_ref() == Some(&users)).unwrap();
    assert_eq!(Liveness::of(row), Liveness::Waiting);
}
