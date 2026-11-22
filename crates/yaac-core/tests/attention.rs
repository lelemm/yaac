//! Attention: precedence, propagation to ancestors across targets, and recomputation.
mod common;

use common::{boot, settle};
use yaac_core::attention::{Attention, AttentionMap, Mark, display};
use yaac_core::demo::{Scene, threads};
use yaac_core::header::{ComposerView, Tone, composer, header};
use yaac_core::hints::RequestKindWord;
use yaac_core::sidebar::needs_you;
use yaac_protocol::{ApprovalAnswer, RequestId, RequestResponse};

#[test]
fn a_pending_request_is_the_threads_own_needs_you_and_parents_carry_nothing() {
    let (c, _) = boot();
    let map = AttentionMap::compute(&c.state);
    let index = display(&c.state, &map, &threads::index());
    assert_eq!(index.mark, Mark::NeedsYou, "not running while it waits for the user");
    assert_eq!(index.attention, Some(Attention::Request(RequestKindWord::Approval)));
    // Parent on the same target, grandparent on another target: their own state only.
    assert_eq!(display(&c.state, &map, &threads::search()).mark, Mark::Running);
    assert_eq!(display(&c.state, &map, &threads::orchestrator()).mark, Mark::Running);
    let h = header(&c.state, &map, &threads::orchestrator()).unwrap();
    assert_eq!(h.tag, Some(("Running".to_owned(), Tone::Running)));
    let h = header(&c.state, &map, &threads::index()).unwrap();
    assert_eq!(h.tag, Some(("Needs you · approval".to_owned(), Tone::NeedsYou)));
    assert_eq!(display(&c.state, &map, &threads::billing()).mark, Mark::Done);
    assert_eq!(display(&c.state, &map, &threads::ci()).mark, Mark::Waiting);
}

#[test]
fn answering_recomputes_from_the_requests_still_open() {
    let (mut c, hub) = boot();
    let index = threads::index();
    c.open(&index);
    settle(&mut c, &hub);
    let resp = RequestResponse::Approval(ApprovalAnswer { decision: "allow".into(), reason: None });
    c.respond(&index, &RequestId::new("r-index"), &resp, "Allow once".into());
    // Submitted: still open until the agent acknowledges.
    c.pump();
    let map = AttentionMap::compute(&c.state);
    assert!(map.of(&index).is_some(), "submitted still holds the agent");
    assert!(matches!(composer(&c.state, &map, &index), ComposerView::Shown { dimmed_hint: Some(_), .. }));
    settle(&mut c, &hub);
    let map = AttentionMap::compute(&c.state);
    assert_eq!(map.of(&index), None);
    let list: Vec<_> = needs_you(&c.state, &map, common::NOW).into_iter().map(|e| e.title).collect();
    assert_eq!(list, ["Fix frame decoder"]);
    assert!(matches!(composer(&c.state, &map, &index), ComposerView::Shown { dimmed_hint: None, .. }));
}

#[test]
fn a_thread_held_elsewhere_is_never_needs_you_here() {
    let (c, _) = boot();
    let map = AttentionMap::compute(&c.state);
    let hs = threads::handshake();
    let d = display(&c.state, &map, &hs);
    assert!(d.read_only);
    assert_eq!(d.attention, None);
    assert_eq!(d.mark, Mark::Idle);
    assert!(matches!(composer(&c.state, &map, &hs), ComposerView::Absent { .. }));
    assert_eq!(header(&c.state, &map, &hs).unwrap().tag.unwrap().0, "Read-only");
}

#[test]
fn budget_reached_pauses_the_root_and_a_request_still_outranks_it() {
    let (mut c, hub) = boot();
    hub.play(Scene::BudgetReached);
    settle(&mut c, &hub);
    let map = AttentionMap::compute(&c.state);
    let orch = threads::orchestrator();
    assert_eq!(map.of(&orch), Some(Attention::BudgetReached));
    assert_eq!(display(&c.state, &map, &orch).mark, Mark::Paused);
    let list: Vec<_> = needs_you(&c.state, &map, common::NOW).into_iter().map(|e| (e.title, e.attention)).collect();
    assert!(list.contains(&("Migrate auth module · orchestrator".to_owned(), Attention::BudgetReached)));
    assert_eq!(list.len(), 3);
    assert_eq!(header(&c.state, &map, &orch).unwrap().tag.unwrap().0, "Paused · budget reached");
    // A request on the root outranks the budget.
    let fix = threads::fix();
    let mut s = c.state.clone();
    s.budgets.insert(fix.address(), s.budgets.values().next().unwrap().clone());
    let map = AttentionMap::compute(&s);
    assert_eq!(map.of(&fix), Some(Attention::Request(RequestKindWord::Approval)));
}

#[test]
fn an_arriving_request_is_announced_not_focused() {
    let (mut c, hub) = boot();
    c.open(&threads::orchestrator());
    settle(&mut c, &hub);
    assert!(c.state.arrivals.is_empty(), "replayed requests are not news");
    hub.play(Scene::RequestStates);
    settle(&mut c, &hub);
    assert_eq!(c.state.arrivals.len(), 1);
    assert_eq!(c.state.arrivals[0].key, threads::fix());
    assert_eq!(c.state.selected_thread(), Some(&threads::orchestrator()), "selection did not move");
    c.open(&threads::fix());
    assert!(c.state.arrivals.is_empty());
}

#[test]
fn a_permission_prompt_needs_you() {
    let (mut c, hub) = boot();
    hub.play(Scene::Permission);
    settle(&mut c, &hub);
    let map = AttentionMap::compute(&c.state);
    let orch = threads::orchestrator();
    assert_eq!(map.of(&orch), Some(Attention::Request(RequestKindWord::Permission)));
    assert_eq!(header(&c.state, &map, &orch).unwrap().tag.unwrap().0, "Needs you · permission");
    assert!(matches!(composer(&c.state, &map, &orch), ComposerView::Shown { dimmed_hint: Some(ref h), .. } if h == "Answer above to continue · a message sent now waits in the queue"),
        "its turn runs, so a message can still be queued behind the answer (D166)");
    assert_eq!(needs_you(&c.state, &map, common::NOW).len(), 3);
}
