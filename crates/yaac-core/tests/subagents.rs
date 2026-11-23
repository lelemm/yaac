//! Harness sub-agents (D151): Claude Code's own subagents appear under their parent — in the
//! agents panel beside its YAAC worker, not as sidebar rows — and open read-only with the
//! harness's reason and a way to the parent.

mod common;

use common::{NOW, boot, settle};
use yaac_core::agents::{Relation, Role, agents_panel};
use yaac_core::attention::{AttentionMap, Mark};
use yaac_core::demo::{Scene, threads};
use yaac_core::header::{Banner, header};
use yaac_core::sidebar::{Grouping, Open, Ordering, sections};
use yaac_core::thread_menu::take_over;

fn with_subagents() -> yaac_core::client::Client {
    let (mut c, hub) = boot();
    hub.play(Scene::HarnessSubagents);
    settle(&mut c, &hub);
    c
}

#[test]
fn subagents_are_workers_of_their_parent_beside_its_yaac_worker() {
    let c = with_subagents();
    let map = AttentionMap::compute(&c.state);
    let p = agents_panel(&c.state, &map, &threads::search()).expect("the panel");
    let workers: Vec<_> = p.workers.iter().map(|w| (w.title.as_str(), w.role, w.origin.as_deref(), w.subagent, w.display.mark)).collect();
    assert_eq!(
        workers,
        [
            ("index-rebuild", Role::Worker, Some("YAAC worker"), false, Mark::NeedsYou),
            ("Check the index schema", Role::Worker, Some("Claude subagent · general-purpose"), true, Mark::Done),
            ("Map the reindex script", Role::Worker, Some("Claude subagent · Explore"), true, Mark::Running),
        ]
    );
    assert_eq!(p.direct_workers, 3);
    // The pill's Workers cluster holds all three.
    let pill = yaac_core::agents::pill(&p, yaac_core::agents::PILL_MAX);
    let w = pill.clusters.iter().find(|c| c.relation == Relation::Workers).expect("workers");
    assert_eq!(w.rows.len(), 3);
    // Each row opens the subagent.
    assert!(p.workers.iter().all(|w| w.key.is_some()));
    // The parent's sidebar line counts them.
    assert_eq!(c.state.summary(&threads::search()).unwrap().children, 3);
}

#[test]
fn an_agent_call_pill_finds_the_subagent_by_its_harness_agent_id() {
    let c = with_subagents();
    let found = yaac_core::agentcall::resolve_subagent(&c.state, &threads::search(), "a7e1c0de5b2f4a913");
    assert_eq!(found, Some(threads::subagent_explore()));
    // Only beneath its own parent.
    assert_eq!(yaac_core::agentcall::resolve_subagent(&c.state, &threads::orchestrator(), "a7e1c0de5b2f4a913"), None);
}

#[test]
fn workers_are_listed_under_their_parents_instead_of_in_the_sidebar() {
    let mut c = with_subagents();
    let keys = |state: &yaac_core::state::AppState, filter| {
        let map = AttentionMap::compute(state);
        sections(state, &map, Grouping::default(), Ordering::default(), filter, Open { muted: true, archived: true }, NOW, yaac_core::time::Zone::UTC)
            .into_iter().flat_map(|s| s.rows.into_iter().map(|r| r.key)).collect::<Vec<_>>()
    };
    let root = threads::orchestrator();
    let billing = threads::billing();
    let search = threads::search();
    let index = threads::index();
    let explore = threads::subagent_explore();
    assert_ne!(billing.target, root.target, "a worker on a different machine");
    let listed = keys(&c.state, "");
    assert!(listed.contains(&root));
    for worker in [billing.clone(), search.clone(), index.clone(), explore.clone(), threads::subagent_schema()] {
        assert!(!listed.contains(&worker), "{worker:?} belongs under its parent");
    }
    assert!(keys(&c.state, "reindex script").is_empty());
    let map = AttentionMap::compute(&c.state);
    assert!(yaac_core::sidebar::needs_you(&c.state, &map, NOW).iter().any(|e| e.key == index));
    c.state.view.entry(billing.clone()).or_default().unseen = true;
    assert_eq!(yaac_core::commands::next_unseen(&c.state, &listed, None), Some(billing));
    c.state.remove_thread(&search);
    let listed = keys(&c.state, "");
    assert!(listed.contains(&explore) && listed.contains(&index), "workers remain accessible when their parent is missing");
}

#[test]
fn a_subagent_opens_read_only_with_the_reason_and_its_parent() {
    let mut c = with_subagents();
    let k = threads::subagent_explore();
    c.open(&k);
    let map = AttentionMap::compute(&c.state);
    let h = header(&c.state, &map, &k).expect("header");
    let (text, reason, parent) = h
        .banners
        .iter()
        .find_map(|b| match b {
            Banner::Subagent { text, reason, parent } => Some((text.clone(), reason.clone(), parent.clone())),
            _ => None,
        })
        .expect("the subagent banner");
    assert_eq!(text, "Claude subagent · Explore, started by search-service. You can read it, but not send to it.");
    assert!(reason.unwrap().contains("SendMessage"));
    assert_eq!(parent.unwrap().key, Some(threads::search()));
    // Not "open in another program", and nothing to take over.
    assert!(!h.banners.iter().any(|b| matches!(b, Banner::InUseElsewhere { .. })));
    assert!(take_over(&c.state, &k).is_none());
    assert!(!c.state.control(&k).may_act());
    assert_eq!(h.tag.map(|t| t.0).as_deref(), Some("Read-only"));
    // Its header names the parent as "Spawned by".
    assert!(matches!(h.links, yaac_core::header::LinkLine::SpawnedBy { .. } | yaac_core::header::LinkLine::Ancestry(_)));
}

#[test]
fn a_subagents_request_is_the_users_needs_you_and_answerable_while_it_stays_read_only() {
    // D162: what a read-only subagent asks follows its parent's writer, not its own hold.
    let mut c = with_subagents();
    let k = threads::subagent_explore();
    c.state.threads.get_mut(&k).unwrap().summary.state = yaac_protocol::ThreadState::NeedsAttention;
    assert!(!c.state.may_act(&k), "still read-only for sending");
    assert!(c.state.may_answer(&k));
    let map = AttentionMap::compute(&c.state);
    assert!(map.of(&k).is_some(), "it needs the user");
    let p = agents_panel(&c.state, &map, &threads::search()).expect("the panel");
    let w = p.workers.iter().find(|w| w.key.as_ref() == Some(&k)).expect("the subagent row");
    assert_eq!(w.display.mark, Mark::NeedsYou, "the needs-you mark on the parent's agents pill");
    let needs = yaac_core::sidebar::needs_you(&c.state, &map, NOW);
    assert!(needs.iter().any(|e| e.key == k), "listed in Needs you");
    // Its parent held by another program: answered there, not here.
    c.state.threads.get_mut(&threads::search()).unwrap().summary.control = yaac_protocol::Control::External;
    if let Some(t) = c.state.transcripts.get_mut(&threads::search()) {
        t.control = Some(yaac_protocol::Control::External);
    }
    assert!(!c.state.may_answer(&k));
    assert!(AttentionMap::compute(&c.state).of(&k).is_none());
}
