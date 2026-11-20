//! The agents panel and the header link lines.
mod common;

use common::{boot, settle};
use yaac_core::agents::{SpawnedBy, agents_panel};
use yaac_core::attention::{AttentionMap, Mark};
use yaac_core::demo::{Scene, threads};
use yaac_core::header::{LinkLine, header};

#[test]
fn orchestrator_panel_has_three_sections_and_a_budget() {
    let (c, _) = boot();
    let map = AttentionMap::compute(&c.state);
    let p = agents_panel(&c.state, &map, &threads::orchestrator()).unwrap();
    assert_eq!(p.budget.unwrap().text, "14 of 100 agent turns · 4 of 20 threads");
    assert_eq!(p.spawned_by, SpawnedBy::You);
    let workers: Vec<_> = p.workers.iter().map(|w| (w.depth, w.title.as_str(), w.display.mark)).collect();
    assert_eq!(
        workers,
        [(0, "users-service", Mark::Done), (0, "billing-service", Mark::Done), (0, "search-service", Mark::Running), (1, "index-rebuild", Mark::NeedsYou)]
    );
    assert_eq!(p.direct_workers, 3);
    assert_eq!(p.workers[3].activity, "needs you · approval");
    let contacts: Vec<_> = p.contacts.iter().map(|w| (w.title.as_str(), w.display.mark)).collect();
    assert_eq!(contacts, [("CI integration", Mark::Waiting), ("Release notes", Mark::Offline)]);
    assert_eq!(p.rail.count, 5);
}

#[test]
fn worker_panel_and_hidden_panel() {
    let (c, _) = boot();
    let map = AttentionMap::compute(&c.state);
    let p = agents_panel(&c.state, &map, &threads::billing()).unwrap();
    assert!(matches!(&p.spawned_by, SpawnedBy::Agent(r) if r.title == "Migrate auth module — orchestrator" && r.display.mark == Mark::Running));
    assert!(p.workers.is_empty() && p.contacts.is_empty(), "the parent is not a contact");
    assert!(agents_panel(&c.state, &map, &threads::fix()).is_none(), "no relationships: hidden");
    let p = agents_panel(&c.state, &map, &threads::index()).unwrap();
    assert_eq!(p.root.unwrap().0, "Migrate auth module — orchestrator");
}

#[test]
fn link_lines() {
    let (c, _) = boot();
    let map = AttentionMap::compute(&c.state);
    match header(&c.state, &map, &threads::billing()).unwrap().links {
        LinkLine::SpawnedBy { parent, target } => {
            assert_eq!((parent.title.as_str(), target.as_str()), ("Migrate auth module — orchestrator", "local"));
            assert!(parent.key.is_some());
        }
        other => panic!("{other:?}"),
    }
    match header(&c.state, &map, &threads::index()).unwrap().links {
        LinkLine::Ancestry(parts) => {
            let titles: Vec<_> = parts.iter().map(|p| p.title.as_str()).collect();
            assert_eq!(titles, ["Migrate auth module — orchestrator", "search-service", "index-rebuild"]);
            assert!(parts[2].key.is_none(), "the thread itself is not a link");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_deleted_recipient_is_not_a_contact() {
    let (mut c, hub) = boot();
    hub.play(Scene::StagingReconnects);
    settle(&mut c, &hub);
    let map = AttentionMap::compute(&c.state);
    let p = agents_panel(&c.state, &map, &threads::orchestrator()).unwrap();
    let contacts: Vec<_> = p.contacts.iter().map(|w| (w.title.as_str(), w.display.mark)).collect();
    assert_eq!(contacts, [("CI integration", Mark::Waiting), ("Release notes", Mark::Done)]);
}

#[test]
fn the_panel_state_machine_and_the_pill() {
    use yaac_core::agents::{Flow, PanelInput as I, PanelMode as M, Role, pill_icons};
    assert_eq!(M::default(), M::Pill);
    assert_eq!(M::Pill.next(I::Enter), M::Card);
    assert_eq!(M::Card.next(I::Leave), M::Pill);
    assert_eq!(M::Card.next(I::Dock), M::Docked);
    assert_eq!(M::Docked.next(I::Leave), M::Docked, "docked stays docked");
    assert_eq!(M::Docked.next(I::Enter), M::Docked);
    assert_eq!(M::Docked.next(I::Undock), M::Pill);
    assert!(M::Docked.takes_column() && !M::Card.takes_column() && !M::Pill.takes_column());

    let (c, _) = boot();
    let map = AttentionMap::compute(&c.state);
    let p = agents_panel(&c.state, &map, &threads::orchestrator()).unwrap();
    let (icons, more) = pill_icons(&p);
    assert_eq!(more, 0);
    let summary: Vec<_> = icons.iter().map(|r| (r.title.as_str(), r.role, r.flow, r.finished, r.kind.as_deref())).collect();
    assert_eq!(
        summary,
        [
            ("users-service", Role::Worker, None, true, Some("claude")),
            ("billing-service", Role::Worker, None, true, Some("claude")),
            ("search-service", Role::Worker, Some(Flow::Sending), false, Some("claude")),
            ("index-rebuild", Role::Worker, None, false, Some("claude")),
            ("CI integration", Role::Contact, None, false, Some("codex")),
            ("Release notes", Role::Contact, None, false, Some("codex")),
        ]
    );
    assert_eq!(icons[3].last_action, "Waiting for your approval");
    assert_eq!(icons[1].last_action, "merged, 14 tests pass");
    assert!(p.in_flight());
    // From the other end, the same message is coming in.
    let p = agents_panel(&c.state, &map, &threads::search()).unwrap();
    let parent = p.all().next().unwrap();
    assert_eq!((parent.role, parent.flow), (Role::Parent, Some(Flow::Receiving)));
    // Nothing in flight: no arrows, no repaint.
    let p = agents_panel(&c.state, &map, &threads::billing()).unwrap();
    assert!(!p.in_flight());
}
