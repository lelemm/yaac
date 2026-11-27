//! The agents panel and the header link lines.
mod common;

use common::{boot, settle};
use yaac_core::agents::{BudgetLine, Relation, SpawnedBy, agents_panel, card_groups, clusters, sibling_rows, span};
use yaac_core::attention::{AttentionMap, Mark};
use yaac_core::demo::{Scene, threads};
use yaac_core::header::{LinkLine, header};

#[test]
fn orchestrator_panel_has_three_sections_and_a_budget() {
    let (c, _) = boot();
    let map = AttentionMap::compute(&c.state);
    let p = agents_panel(&c.state, &map, &threads::orchestrator()).unwrap();
    let b = p.budget.unwrap();
    assert_eq!(b.text(common::NOW), "14 of 100 agent turns · 1 h since you acted · no time limit");
    assert_eq!(b.reached(), None);
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
    assert!(matches!(&p.spawned_by, SpawnedBy::Agent(r) if r.title == "Migrate auth module · orchestrator" && r.display.mark == Mark::Running));
    assert!(p.workers.is_empty() && p.contacts.is_empty(), "the parent is not a contact");
    let card = card_groups(&p);
    assert_eq!(card.iter().map(|(label, _)| *label).collect::<Vec<_>>(), ["Siblings", "Spawned by"]);
    assert_eq!(card[0].1.iter().filter_map(|row| row.key.clone()).collect::<std::collections::BTreeSet<_>>(),
        [threads::users(), threads::search(), threads::billing()].into_iter().collect());
    assert_eq!(clusters(&p).len(), 1, "siblings are outside the pill's relation clusters");
    assert!(agents_panel(&c.state, &map, &threads::fix()).is_none(), "no relationships: hidden");
    let p = agents_panel(&c.state, &map, &threads::index()).unwrap();
    assert_eq!(p.root.unwrap().0, "Migrate auth module · orchestrator");
    assert!(p.siblings.is_empty(), "an only child has no Siblings section");
}

#[test]
fn switching_siblings_keeps_the_same_order_and_moves_the_self_row() {
    let (mut c, _) = boot();
    let members = [threads::users(), threads::billing(), threads::search()];
    let timestamp = c.state.summary(&members[0]).unwrap().updated_at.clone();
    for key in &members {
        // Force a tie to catch ordering that happens to put whichever thread is self last.
        let summary = &mut c.state.threads.get_mut(key).unwrap().summary;
        summary.state = yaac_protocol::ThreadState::Idle;
        summary.updated_at = timestamp.clone();
    }
    let map = AttentionMap::compute(&c.state);
    let mut expected = members.clone();
    expected.sort();
    for selected in &members {
        let panel = agents_panel(&c.state, &map, selected).unwrap();
        let siblings = sibling_rows(&panel);
        assert_eq!(siblings, card_groups(&panel)[0].1);
        assert_eq!(siblings.iter().map(|row| row.key.clone().unwrap()).collect::<Vec<_>>(), expected);
        for row in siblings {
            assert_eq!(row.is_self, row.key.as_ref() == Some(selected));
            assert_eq!(row.open_key().is_none(), row.is_self);
        }
    }
}

#[test]
fn reading_a_completed_worker_moves_it_to_the_quiet_tier() {
    let (c, _) = boot();
    let map = AttentionMap::compute(&c.state);
    let mut panel = agents_panel(&c.state, &map, &threads::orchestrator()).unwrap();
    for (row, (updated, unseen)) in panel.workers.iter_mut().zip([(10, true), (50, false), (5, false), (1, false)]) {
        row.updated = updated;
        row.display.unseen = unseen;
    }
    let ordered = |panel: &yaac_core::agents::AgentsPanel| {
        clusters(panel).into_iter().find(|group| group.relation == Relation::Workers).unwrap()
            .rows.into_iter().map(|row| row.title.clone()).collect::<Vec<_>>()
    };
    // Alive first (index-rebuild needs you, search-service is running) even though both are
    // older, then the unread completion (users-service), then the read one.
    assert_eq!(ordered(&panel), ["index-rebuild", "search-service", "users-service", "billing-service"]);
    panel.workers[0].display.unseen = false;
    assert_eq!(ordered(&panel), ["index-rebuild", "search-service", "billing-service", "users-service"]);
}

#[test]
fn link_lines() {
    let (c, _) = boot();
    let map = AttentionMap::compute(&c.state);
    match header(&c.state, &map, &threads::billing()).unwrap().links {
        LinkLine::SpawnedBy { parent, target } => {
            assert_eq!((parent.title.as_str(), target.as_str()), ("Migrate auth module · orchestrator", "local"));
            assert!(parent.key.is_some());
        }
        other => panic!("{other:?}"),
    }
    match header(&c.state, &map, &threads::index()).unwrap().links {
        LinkLine::Ancestry(parts) => {
            let titles: Vec<_> = parts.iter().map(|p| p.title.as_str()).collect();
            assert_eq!(titles, ["Migrate auth module · orchestrator", "search-service", "index-rebuild"]);
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
    use yaac_core::agents::{Flow, PILL_MAX, PanelInput as I, PanelMode as M, Role, pill};
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
    let pl = pill(&p, PILL_MAX);
    assert_eq!(pl.more.hidden, 0);
    let icons: Vec<_> = pl.clusters.iter().flat_map(|c| c.rows.iter().copied()).collect();
    let summary: Vec<_> = icons.iter().map(|r| (r.title.as_str(), r.role, r.flow, r.finished, r.kind.as_deref())).collect();
    assert_eq!(
        summary,
        [
            // Alive first (D168): needs you, running, then the finished, most recent first.
            ("index-rebuild", Role::Worker, None, false, Some("claude")),
            ("search-service", Role::Worker, Some(Flow::Sending), false, Some("claude")),
            ("billing-service", Role::Worker, None, true, Some("claude")),
            ("users-service", Role::Worker, None, true, Some("claude")),
            ("CI integration", Role::Contact, None, false, Some("codex")),
            ("Release notes", Role::Contact, None, false, Some("codex")),
        ]
    );
    assert_eq!(icons[0].last_action, "Waiting for your approval");
    assert_eq!(icons[2].last_action, "merged, 14 tests pass");
    assert!(p.in_flight());
    // From the other end, the same message is coming in.
    let p = agents_panel(&c.state, &map, &threads::search()).unwrap();
    let parent = p.all().next().unwrap();
    assert_eq!((parent.role, parent.flow), (Role::Parent, Some(Flow::Receiving)));
    // Nothing in flight: no arrows, no repaint.
    let p = agents_panel(&c.state, &map, &threads::billing()).unwrap();
    assert!(!p.in_flight());
}

fn line(turns_used: u32, since: u64, limit: Option<u64>, exhausted: bool) -> BudgetLine {
    BudgetLine {
        turns_used,
        turn_limit: 100,
        since_acted_secs: since,
        acted_at: None,
        time_limit_secs: limit,
        exhausted,
        can_set_time_limit: true,
        root: threads::orchestrator().address(),
    }
}

/// P352: the line shows both limits, and says which one ran out.
#[test]
fn the_budget_line_shows_both_limits_and_which_ran_out() {
    let unlimited = line(41, 14 * 3600 + 100, None, false);
    assert_eq!(unlimited.text(0), "41 of 100 agent turns · 14 h since you acted · no time limit");
    assert_eq!(unlimited.reached(), None);
    let limited = line(41, 3 * 3600, Some(12 * 3600), false);
    assert_eq!(limited.text(0), "41 of 100 agent turns · 3 h since you acted (limit 12 h)");
    let by_time = line(41, 14 * 3600, Some(12 * 3600), true);
    assert_eq!(by_time.reached().as_deref(), Some("Paused: 12 h without you"));
    let by_turns = line(100, 3600, Some(12 * 3600), true);
    assert_eq!(by_turns.reached().as_deref(), Some("100 of 100 turns used"));
    let both = line(100, 14 * 3600, Some(12 * 3600), true);
    assert_eq!(both.reached().as_deref(), Some("100 of 100 turns used"), "turns first, as the target checks them");
    // Exhausted without either limit met here (an older target, a report in between).
    assert_eq!(line(3, 60, None, true).reached().as_deref(), Some("Agent budget reached"));
    assert_eq!([span(0), span(59), span(25 * 60), span(90 * 60), span(14 * 3600 + 59)], ["< 1 min", "< 1 min", "25 min", "1 h 30 min", "14 h"]);
}

/// The time since a person acted counts on from the target's `actedAt` between reports.
#[test]
fn the_time_since_acting_counts_on_from_acted_at() {
    let mut b = line(1, 3600, Some(12 * 3600), false);
    b.acted_at = Some(1_000_000);
    assert_eq!(b.since_acted(1_000_000 + 5 * 3600), 5 * 3600);
    assert_eq!(b.since_acted(1_000_000), 3600, "never less than the target said");
    assert_eq!(b.next_change(1_000_000 + 5 * 3600 + 15), Some(45), "the next minute");
    assert_eq!(b.next_change(1_000_000 + 11 * 3600 + 15), Some(3585), "past 10 h, the next hour");
    b.acted_at = None;
    assert_eq!(b.since_acted(1_000_000 + 5 * 3600), 3600, "an older target: as of its report");
    assert_eq!(b.next_change(1_000_000), None, "nothing to count on");
}

/// P352: right-click on the duration sets or extends the limit; it never resets the clock.
#[test]
fn the_time_limit_menu_sets_extends_and_removes() {
    let unlimited = line(41, 14 * 3600 + 20 * 60, None, false);
    let m = unlimited.time_menu(0);
    assert_eq!(m.title, "Set time limit…");
    let got: Vec<_> = m.choices.iter().map(|c| (c.label.as_str(), c.detail.as_str(), c.wall_clock_secs)).collect();
    // From the next whole hour of the time already passed.
    assert_eq!(got, [("+1 h", "limit 16 h", Some(16 * 3600)), ("+4 h", "limit 19 h", Some(19 * 3600)), ("+12 h", "limit 27 h", Some(27 * 3600)), ("+24 h", "limit 39 h", Some(39 * 3600))]);
    assert_eq!(m.unavailable, None);

    // Limited, not reached: on top of the limit, available before it is reached.
    let limited = line(41, 3 * 3600, Some(12 * 3600), false);
    let m = limited.time_menu(0);
    assert_eq!(m.title, "Extend time limit…");
    assert_eq!(m.choices[1].wall_clock_secs, Some(16 * 3600));
    let last = m.choices.last().unwrap();
    assert_eq!((last.label.as_str(), last.wall_clock_secs), ("Remove time limit", None));
    assert_eq!(limited.added(0, 7), 19 * 3600, "custom hours");

    // Reached by time: on top of the time passed, so +1 h really frees it.
    let reached = line(41, 14 * 3600, Some(12 * 3600), true);
    assert_eq!(reached.time_menu(0).choices[0].wall_clock_secs, Some(15 * 3600));

    // An older yaacd on the root's target cannot do it: the menu says why.
    let mut old = limited;
    old.can_set_time_limit = false;
    assert!(old.time_menu(0).unavailable.is_some());
}

/// P352 end to end in the demo: out of time, extended past it, then the limit removed.
#[test]
fn a_time_limit_set_from_the_window_reaches_the_roots_target() {
    let (mut c, hub) = boot();
    hub.play(Scene::BudgetOutOfTime);
    settle(&mut c, &hub);
    let orch = threads::orchestrator();
    let budget = |c: &yaac_core::client::Client| agents_panel(&c.state, &AttentionMap::compute(&c.state), &orch).unwrap().budget.unwrap();
    let b = budget(&c);
    assert_eq!(b.reached().as_deref(), Some("Paused: 12 h without you"));
    assert_eq!(b.text(0), "41 of 100 agent turns · 14 h since you acted (limit 12 h)");
    assert!(b.can_set_time_limit, "this yaacd advertises budgetTimeLimit");
    let extend = b.time_menu(0).choices[1].wall_clock_secs;
    assert_eq!(extend, Some(18 * 3600), "+4 h on top of the 14 h passed");
    c.set_time_limit(&b.root, extend);
    settle(&mut c, &hub);
    let b = budget(&c);
    assert!(!b.exhausted, "freed");
    assert_eq!(b.text(0), "41 of 100 agent turns · 14 h since you acted (limit 18 h)", "the clock ran on");
    c.set_time_limit(&b.root, None);
    settle(&mut c, &hub);
    assert_eq!(budget(&c).text(0), "41 of 100 agent turns · 14 h since you acted · no time limit");
    assert_eq!(budget(&c).time_menu(0).title, "Set time limit…");
}
