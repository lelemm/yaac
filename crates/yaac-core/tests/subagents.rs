//! Harness sub-agents (D151): Claude Code's own subagents appear under their parent — in the
//! agents panel beside its YAAC worker, not as sidebar rows — and open read-only with the
//! harness's reason and a way to the parent.

mod common;

use common::{NOW, boot, settle};
use yaac_core::agents::{Relation, Role, agents_panel, clusters};
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
    let pill = clusters(&p, Some(8));
    let w = pill.iter().find(|c| c.relation == Relation::Workers).expect("workers");
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
fn subagents_are_not_sidebar_rows_while_their_parent_is_listed() {
    let c = with_subagents();
    let map = AttentionMap::compute(&c.state);
    let secs = sections(&c.state, &map, Grouping::default(), Ordering::default(), "", Open { muted: true, archived: true }, NOW, yaac_core::time::Zone::UTC);
    let keys: Vec<_> = secs.iter().flat_map(|s| s.rows.iter().map(|r| r.key.clone())).collect();
    assert!(keys.contains(&threads::search()));
    assert!(!keys.contains(&threads::subagent_explore()));
    assert!(!keys.contains(&threads::subagent_schema()));
    // A filter that matches a subagent's title still does not list it.
    let secs = sections(&c.state, &map, Grouping::default(), Ordering::default(), "reindex script", Open::default(), NOW, yaac_core::time::Zone::UTC);
    assert!(secs.iter().all(|s| s.rows.iter().all(|r| r.key != threads::subagent_explore())));
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
