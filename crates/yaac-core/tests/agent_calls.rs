//! Agent calls in the transcript (D150): spawn, message, wait and a harness's own subagents as
//! one line each, with the other agent as a pill; the mesh items they echo folded away.
mod common;

use common::{boot, settle};
use yaac_core::ThreadKey;
use yaac_core::activity::{ActivityLine, Block, Expansion, LineMark, blocks};
use yaac_core::agentcall::{self, Pill};
use yaac_core::attention::AttentionMap;
use yaac_core::demo::{Scene, threads};
use yaac_protocol::{AgentRef, ItemBody};

fn lines(c: &yaac_core::client::Client, key: &ThreadKey) -> Vec<ActivityLine> {
    let map = AttentionMap::compute(&c.state);
    blocks(&c.state, &map, key, &Expansion::default())
        .into_iter()
        .flat_map(|b| match b {
            Block::Run { lines } => lines,
            _ => Vec::new(),
        })
        .collect()
}

fn line<'a>(ls: &'a [ActivityLine], id: &str) -> &'a ActivityLine {
    ls.iter().find(|l| l.id.as_str() == id).unwrap_or_else(|| panic!("no line {id}: {:?}", ls.iter().map(|l| &l.text).collect::<Vec<_>>()))
}

fn scene() -> (yaac_core::client::Client, yaac_core::demo::DemoHub) {
    let (mut c, hub) = boot();
    hub.play(Scene::AgentCalls);
    settle(&mut c, &hub);
    (c, hub)
}

#[test]
fn the_orchestrators_calls_read_as_lines_with_pills_and_their_echoes_are_folded_in() {
    let (c, _) = scene();
    let orch = threads::orchestrator();
    let ls = lines(&c, &orch);
    let texts: Vec<&str> = ls.iter().filter(|l| l.index >= 9).map(|l| l.text.as_str()).collect();
    assert_eq!(
        texts,
        [
            "Started Update check every 10 minutes · gpt-6.2-sol · medium · running",
            "billing-service · devbox · \u{201c}Check the refund race in test_refund_race before you touch the retry path: it fails one run in twenty on CI.\u{201d} · delivered",
            "billing-service · devbox reported back",
            "Waiting for Update check every 10 minutes, search-service · gpu-box …",
        ],
        "the meshSent, meshReceived and meshWait the calls caused are not shown twice"
    );
    assert_eq!(line(&ls, "ac3").mark, LineMark::Spawn);
    assert_eq!(line(&ls, "ac4").mark, LineMark::Sent);
    assert_eq!(line(&ls, "ac5").mark, LineMark::Received);
    assert!(line(&ls, "ac7").in_progress);
    for hidden in ["ac4s", "ac5r", "ac7w"] {
        assert!(ls.iter().all(|l| l.id.as_str() != hidden), "{hidden} is an echo");
    }
    // The pills: resolved threads, with their harness, machine (when not this one) and click
    // target — the message's pill lands on the message at the other end, when it is loaded.
    let send = line(&ls, "ac4").call.clone().unwrap();
    assert_eq!(
        send.pills,
        [Pill { label: "billing-service".into(), kind: Some("claude".into()), machine: Some("devbox".into()), key: Some(threads::billing()), at: None }]
    );
    let spawn = line(&ls, "ac3").call.clone().unwrap();
    assert_eq!(spawn.pills[0].kind.as_deref(), Some("codex"));
    assert_eq!(spawn.pills[0].machine, None, "same machine as the caller");
    assert!(spawn.pills[0].key.is_some(), "the new worker opens");
}

#[test]
fn claude_code_subagents_are_pills_with_the_harness_logo_and_no_thread_to_open() {
    let (c, _) = scene();
    let users = threads::users();
    let ls = lines(&c, &users);
    let texts: Vec<&str> = ls.iter().filter(|l| l.id.as_str().starts_with("ua")).map(|l| l.text.as_str()).collect();
    assert_eq!(
        texts,
        [
            "Subagent Fix the cache test · opus · medium · background",
            "Resumed subagent a59f490b · \u{201c}Daemon handover: the socket moved to $XDG_RUNTIME_DIR/yaac. Carry on from step 3 of the plan.\u{201d}",
            "Subagent Count the flaky tests · finished",
            "Fix the cache test · \u{201c}Also check the TTL clock: the test may assume a monotonic clock.\u{201d} · queued · it reads it at its next step",
        ]
    );
    let resumed = line(&ls, "ua3");
    assert_eq!(resumed.mark, LineMark::Resumed);
    let pill = &resumed.call.as_ref().unwrap().pills[0];
    assert_eq!((pill.label.as_str(), pill.kind.as_deref(), pill.key.as_ref()), ("a59f490b", Some("claude"), None), "an unresolved subagent: its short id, the caller's harness, not clickable");
    assert!(ls.iter().all(|l| !l.text.contains("success")), "never raw JSON in the line");
}

#[test]
fn a_harness_subagent_that_is_a_thread_resolves_through_the_one_lookup() {
    let (mut c, _) = scene();
    let users = threads::users();
    // D151: the subagent becomes a thread beneath its parent, keyed by the harness's agent id.
    let child = threads::billing();
    if let Some(rec) = c.state.threads.get_mut(&child) {
        rec.summary.spawned_by = Some(users.address());
        rec.summary.native_id = Some("f00dbabe12".into());
        rec.summary.title = "billing-service — iteration 1–2 – review".into();
    }
    let r = AgentRef { agent_id: Some("f00dbabe12".into()), ..AgentRef::default() };
    assert_eq!(agentcall::resolve_subagent(&c.state, &users, "f00dbabe12"), Some(child.clone()));
    assert_eq!(agentcall::resolve(&c.state, &users, &r), Some(child.clone()));
    assert_eq!(agentcall::resolve_subagent(&c.state, &threads::orchestrator(), "f00dbabe12"), None, "only beneath the caller");
    let ls = lines(&c, &users);
    let pill = &line(&ls, "ua2").call.as_ref().unwrap().pills[0];
    assert_eq!(pill.key.as_ref(), Some(&child), "the Agent call's pill now opens the subagent's thread");
    assert_eq!(pill.label, "billing-service · iteration 1–2 · review", "spaced separators use dots in the pill");
    // "Its parent" resolves to the caller's own parent.
    let parent = AgentRef { parent: true, ..AgentRef::default() };
    assert_eq!(agentcall::resolve(&c.state, &child, &parent), Some(users));
}

#[test]
fn an_expanded_call_shows_the_full_text_the_result_and_the_raw_call() {
    let (c, _) = scene();
    let users = threads::users();
    let t = c.state.transcripts.get(&users).unwrap();
    let item = t.items().iter().find(|i| i.id.as_str() == "ua4").unwrap();
    let ItemBody::ToolCall(tc) = &item.body else { panic!() };
    let d = agentcall::detail(Some(t), tc, tc.agent_call.as_ref().unwrap());
    assert_eq!(d.heading, "Task for");
    assert!(d.text.as_deref().unwrap().starts_with("How many tests"));
    assert!(d.result.as_deref().unwrap().contains("`cache::expires_after_ttl`"));
    assert!(d.raw_input.contains("\"subagent_type\": \"Explore\""));
    assert!(d.raw_output.contains("agentId: 0a0b0c0d9e"));
    // A YAAC message's receipt in full, from its live meshSent.
    let orch = threads::orchestrator();
    let t = c.state.transcripts.get(&orch).unwrap();
    let item = t.items().iter().find(|i| i.id.as_str() == "ac4").unwrap();
    let ItemBody::ToolCall(tc) = &item.body else { panic!() };
    let d = agentcall::detail(Some(t), tc, tc.agent_call.as_ref().unwrap());
    assert_eq!(d.heading, "To");
    assert!(d.status.unwrap().starts_with("Delivered"));
}

/// D164: a running `Agent` call is named by its description, and once Claude Code says which
/// subagent it started, opens that subagent's conversation under its title.
#[test]
fn a_running_agent_call_is_named_by_its_description_and_then_by_its_subagent() {
    let (mut c, _) = scene();
    let users = threads::users();
    let map = AttentionMap::compute(&c.state);
    let item: yaac_protocol::Item = serde_json::from_value(serde_json::json!({
        "id": "toolu_a", "turnId": "t", "status": "inProgress", "kind": "toolCall", "tool": "subagent", "title": "Subagent Fix false Paused",
        "input": { "name": "Agent", "description": "Fix false Paused", "subagent_type": "general-purpose", "model": "opus", "effort": "medium", "prompt": "Fix it." },
        "agentCall": { "kind": "spawn", "title": "Fix false Paused", "text": "Fix it.", "model": "opus", "effort": "medium", "peers": [{ "name": "Fix false Paused" }] }
    }))
    .unwrap();
    let ItemBody::ToolCall(tc) = &item.body else { panic!("a tool call") };
    let l = agentcall::line(&c.state, &map, &users, None, &item, tc.agent_call.as_ref().unwrap());
    assert_eq!(l.plain(), "Subagent Fix false Paused · opus · medium · running…");

    // Linked to its subagent (task_started): the pill opens it, under its title.
    let child = threads::billing();
    if let Some(rec) = c.state.threads.get_mut(&child) {
        rec.summary.spawned_by = Some(users.address());
        rec.summary.native_id = Some("a5b6c7d8e9f0a1b2c".into());
    }
    let mut linked = tc.agent_call.clone().unwrap();
    linked.peers[0].agent_id = Some("a5b6c7d8e9f0a1b2c".into());
    let l = agentcall::line(&c.state, &AttentionMap::compute(&c.state), &users, None, &item, &linked);
    assert_eq!(l.pills[0].key.as_ref(), Some(&child));
    assert_eq!(l.pills[0].label, "billing-service");
    assert!(l.plain().ends_with("running…"), "{}", l.plain());
}
