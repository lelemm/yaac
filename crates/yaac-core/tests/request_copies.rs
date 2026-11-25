//! A copied request is routed to its worker, even from another target.
mod common;

use common::{NOW, settle};
use yaac_core::{client::Client, copies, demo::{DEMO_CLIENT, DemoHub, Scenario, Scene, threads}};
use yaac_protocol::{ApprovalAnswer, ClientId, RequestResponse, RequestState};

#[test]
fn cold_root_loads_worker_and_all_ancestors_share_its_answer() {
    let hub = DemoHub::new(Scenario::Fleet, NOW);
    let mut client = Client::new(Box::new(hub.clone()), "copies", ClientId::new(DEMO_CLIENT));
    settle(&mut client, &hub);
    let root = threads::orchestrator();
    let parent = threads::search();
    let worker = threads::index();
    assert_ne!(root.target, worker.target);
    assert!(!client.state.transcripts.contains_key(&worker));
    client.open(&root);
    client.set_showing(vec![root.clone()]);
    settle(&mut client, &hub);
    let copy = copies::open_copy_of(&client.state, &root, &worker).unwrap();
    assert_eq!(copy.depth, 2);
    assert_eq!(copies::open_copy_of(&client.state, &parent, &worker).unwrap().id(), copy.id());
    let before = client.state.transcripts[&root].items().to_vec();
    let turn = client.state.transcripts[&root].active_turn.clone();
    client.respond_from(&root, &worker, &copy.request, &RequestResponse::Approval(ApprovalAnswer { decision: "allow".into(), reason: None }), "Allow once".into());
    assert_eq!(copies::request_of(&client.state, &copy).unwrap().2.state, RequestState::Submitted);
    client.respond_from(&parent, &worker, &copy.request, &RequestResponse::Approval(ApprovalAnswer { decision: "deny".into(), reason: None }), "Deny".into());
    settle(&mut client, &hub);
    for ancestor in [&parent, &root] {
        let resolved = copies::find(&client.state, ancestor, &copy.id()).unwrap();
        assert!(!resolved.open);
        let place = copies::answered_where(&client.state, &worker, copies::request_of(&client.state, &resolved).unwrap().2, ancestor).unwrap();
        if ancestor == &root { assert_eq!(place, "Answered here"); }
        else { assert_eq!(place, format!("Answered in {}", client.state.summary(&root).unwrap().title)); }
    }
    assert_eq!(client.state.transcripts[&root].items(), before);
    assert_eq!(client.state.transcripts[&root].active_turn, turn);
    let requests = hub.requests();
    assert!(requests.iter().any(|(target, method, params)| target.as_ref() == Some(&worker.target)
        && method == "thread/read" && params["threadId"] == worker.thread.as_str() && params["maxItems"] == 1));
    let routes: Vec<_> = requests.iter().filter(|(_, method, _)| method == "request/respond").collect();
    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0].0.as_ref(), Some(&worker.target));
    assert_eq!(routes[0].2["threadId"], worker.thread.as_str());
    assert_eq!(routes[0].2["requestId"], copy.request.as_str());
    assert!(requests.iter().any(|(target, method, p)| method == "thread/unsubscribe"
        && target.as_ref() == Some(&worker.target) && p["threadId"] == worker.thread.as_str()));
    client.open(&worker);
    settle(&mut client, &hub);
    assert_eq!(hub.requests().iter().filter(|(t, method, p)| method == "thread/subscribe"
        && t.as_ref() == Some(&worker.target) && p["threadId"] == worker.thread.as_str()).count(), 2);
}

#[test]
fn harness_worker_uses_its_parent_chain_at_any_depth() {
    let (mut client, hub) = common::boot();
    hub.play(Scene::HarnessSubagents);
    settle(&mut client, &hub);
    let worker = threads::subagent_explore();
    client.watch_transcript(&worker);
    settle(&mut client, &hub);
    let parent = threads::search();
    let middle = threads::billing();
    let root = threads::orchestrator();
    client.state.threads.get_mut(&parent).unwrap().summary.spawned_by = Some(middle.address());
    client.open(&root);
    client.set_showing(vec![root.clone()]);
    settle(&mut client, &hub);
    let request = client.state.transcripts[&threads::index()].item(&"i3".into()).unwrap().clone();
    client.state.hints.entry(worker.clone()).or_default().observe_item(&request);
    client.state.transcripts.get_mut(&worker).unwrap().upsert(request, 1000);
    assert!(client.state.may_answer(&worker));
    assert!(!client.state.may_act(&worker));
    for (i, ancestor) in [&parent, &middle, &root].into_iter().enumerate() {
        let copy = copies::open_copy_of(&client.state, ancestor, &worker).unwrap();
        assert_eq!(copy.depth, i + 1);
    }
}

#[test]
fn open_copies_stack_by_depth_then_arrival_including_forms() {
    use yaac_protocol::{FormRequest, ItemBody, RequestBody};
    let (mut client, _) = common::boot();
    let root = threads::orchestrator();
    let child = threads::search();
    let grandchild = threads::index();
    let template = client.state.transcripts[&grandchild].item(&"i3".into()).unwrap().clone();
    for (id, time) in [("a-later", "2026-11-25T12:30:00.700Z"), ("z-earlier", "2026-11-25T12:30:00.100Z")] {
        let mut item = template.clone();
        item.id = id.into();
        item.started_at = Some(time.into());
        let ItemBody::Request(request) = &mut item.body else { unreachable!() };
        request.request_id = id.into();
        request.body = RequestBody::Form(FormRequest { message: "Choose a file".into(), schema: None, source: None });
        client.state.hints.entry(child.clone()).or_default().observe_item(&item);
        client.state.transcripts.get_mut(&child).unwrap().upsert(item, 1000);
    }
    let cards = copies::copies(&client.state, &root);
    let ids: Vec<_> = cards.iter().map(|c| c.item.as_str()).collect();
    assert_eq!(ids, ["z-earlier", "a-later", "i3"]);
    client.state.transcripts.get_mut(&child).unwrap().set_request_state(&"z-earlier".into(), RequestState::Submitted, 1001);
    assert_eq!(copies::open_copy_of(&client.state, &root, &child).unwrap().item.as_str(), "a-later");
}

#[test]
fn leaving_while_a_worker_subscription_is_in_flight_defers_its_read() {
    let hub = DemoHub::new(Scenario::Fleet, NOW);
    let mut client = Client::new(Box::new(hub.clone()), "copies", ClientId::new(DEMO_CLIENT));
    settle(&mut client, &hub);
    let root = threads::orchestrator();
    let worker = threads::index();
    client.open(&root);
    client.set_showing(vec![root.clone()]);
    client.set_showing(vec![]);
    settle(&mut client, &hub);
    assert!(!hub.requests().iter().any(|(target, method, params)| target.as_ref() == Some(&worker.target)
        && method == "thread/read" && params["threadId"] == worker.thread.as_str()));
    client.set_showing(vec![root.clone()]);
    settle(&mut client, &hub);
    assert!(copies::open_copy_of(&client.state, &root, &worker).is_some());
}
