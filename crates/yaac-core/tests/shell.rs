//! `!command` in the composer (P344, P346): a `!` draft on a thread that can run one is sent
//! as `turn/shellCommand` (queued while a turn runs, D166); elsewhere `!` is plain text.
mod common;

use common::{boot, settle};
use serde_json::Value;
use yaac_core::attention::AttentionMap;
use yaac_core::client::Client;
use yaac_core::compose;
use yaac_core::demo::{DemoHub, Scene, threads};
use yaac_core::header::{ComposerView, composer, shell_mode};
use yaac_core::ThreadKey;
use yaac_protocol::{Capability, InputPart, TextRole};

fn sent(hub: &DemoHub, method: &str) -> Vec<Value> {
    hub.requests().into_iter().filter(|(_, m, _)| m == method).map(|(_, _, p)| p).collect()
}

fn type_in(c: &mut Client, k: &ThreadKey, text: &str) {
    c.state.drafts.entry(k.clone()).or_default().text = text.into();
}

/// Gives the thread `userShellCommand`, or takes it away.
fn shell_capable(c: &mut Client, k: &ThreadKey, on: bool) {
    let caps = &mut c.state.transcripts.get_mut(k).unwrap().effective.as_mut().expect("effective").capabilities;
    caps.retain(|x| *x != Capability::UserShellCommand);
    if on {
        caps.push(Capability::UserShellCommand);
    }
}

fn opened(running: bool) -> (Client, DemoHub, ThreadKey) {
    let (mut c, hub) = boot();
    let k = threads::billing();
    c.open(&k);
    settle(&mut c, &hub);
    if running {
        hub.play(Scene::QueueWhileRunning);
        settle(&mut c, &hub);
    }
    (c, hub, k)
}

#[test]
fn a_bang_draft_runs_as_a_shell_command_only_where_the_thread_can() {
    let (mut c, hub, k) = opened(false);
    shell_capable(&mut c, &k, false);
    type_in(&mut c, &k, "!ls");
    let map = AttentionMap::compute(&c.state);
    assert!(matches!(composer(&c.state, &map, &k), ComposerView::Shown { shell: None, .. }), "no `!` mode without the capability");
    assert!(c.send(&k));
    assert!(sent(&hub, "turn/shellCommand").is_empty());
    let p = sent(&hub, "turn/start").pop().expect("plain message");
    assert_eq!(p["input"][0]["text"], "!ls", "a leading `!` is plain text there");

    let (mut c, hub, k) = opened(false);
    shell_capable(&mut c, &k, true);
    type_in(&mut c, &k, "!  git status --short  ");
    let map = AttentionMap::compute(&c.state);
    let ComposerView::Shown { shell: Some(mode), .. } = composer(&c.state, &map, &k) else { panic!("`!` mode") };
    assert_eq!(mode.run_label, "Run");
    assert!(mode.ready);
    assert!(mode.hint.starts_with("Runs here, then ") && mode.hint.ends_with(" answers"), "{}", mode.hint);
    assert!(mode.prompt.contains(" · ") || mode.folder.is_empty(), "{}", mode.prompt);
    assert!(c.send(&k));
    assert!(sent(&hub, "turn/start").is_empty(), "a command is never sent as a message");
    let p = sent(&hub, "turn/shellCommand").pop().expect("turn/shellCommand");
    assert_eq!(p["command"], "git status --short");
    assert_eq!(p["delivery"], "queue", "should a turn start first, it waits rather than being refused");
    assert!(p["opId"].as_str().is_some_and(|o| !o.is_empty()));
    assert!(c.state.drafts.get(&k).is_none_or(|d| d.is_empty()), "the draft clears");
    // The echo shows the command at once, as the `!command` the transcript will hold.
    let echo = &c.state.transcripts[&k].pending()[0];
    assert_eq!(echo.parts, vec![InputPart::text_as("!git status --short", TextRole::Command, None)]);
    assert_eq!(echo.method, "turn/shellCommand", "a Retry sends the same request");
}

#[test]
fn while_a_turn_runs_it_is_queue_run_and_send_now_also_queues() {
    let (mut c, hub, k) = opened(true);
    shell_capable(&mut c, &k, true);
    type_in(&mut c, &k, "!cargo test");
    assert_eq!(shell_mode(&c.state, &k).expect("mode").run_label, "Queue run");
    assert!(c.send_now(&k));
    let p = sent(&hub, "turn/shellCommand").pop().expect("turn/shellCommand");
    assert_eq!(p["delivery"], "queue", "a command never goes into a running turn");
    assert!(sent(&hub, "turn/start").is_empty());
}

#[test]
fn an_empty_bang_sends_nothing_and_leaving_the_mode_keeps_the_text() {
    let (mut c, hub, k) = opened(false);
    shell_capable(&mut c, &k, true);
    type_in(&mut c, &k, "!   ");
    assert!(!shell_mode(&c.state, &k).expect("mode").ready);
    assert!(!c.send(&k));
    assert!(sent(&hub, "turn/shellCommand").is_empty() && sent(&hub, "turn/start").is_empty());
    type_in(&mut c, &k, &compose::leave_shell_mode("!npm test"));
    assert_eq!(c.state.drafts[&k].text, "npm test");
    assert!(shell_mode(&c.state, &k).is_none(), "the normal composer again");
}

#[test]
fn earlier_commands_are_this_threads_bang_messages_user_run_items_and_queued_ones() {
    let (mut c, _hub, k) = opened(false);
    let rev = c.state.next_stamp();
    let t = c.state.transcripts.get_mut(&k).unwrap();
    let mk = |id: &str, body: yaac_protocol::ItemBody| yaac_protocol::Item {
        id: yaac_protocol::ItemId::new(id),
        turn_id: yaac_protocol::TurnId::new("t-sh"),
        status: yaac_protocol::ItemStatus::Completed,
        started_at: None,
        completed_at: None,
        body,
    };
    t.upsert(mk("u1", yaac_protocol::ItemBody::UserMessage(yaac_protocol::UserMessageItem { parts: vec![InputPart::text_as("!ls", TextRole::Command, None)], op_id: None })), rev);
    let tool = |title: &str| yaac_protocol::ToolCallItem {
        tool: yaac_protocol::ToolKind::Command,
        title: title.into(),
        input: None,
        output: String::new(),
        output_truncated: false,
        output_bytes: None,
        exit_code: Some(0),
        cwd: None,
        agent_call: None,
        skill: None,
        images: vec![],
        source: Some(yaac_protocol::ToolCallSource::UserShell),
        duration_ms: Some(3),
    };
    t.upsert(mk("c1", yaac_protocol::ItemBody::ToolCall(tool("ls"))), rev);
    t.upsert(mk("c2", yaac_protocol::ItemBody::ToolCall(tool("git log -1"))), rev);
    t.upsert(mk("u2", yaac_protocol::ItemBody::UserMessage(yaac_protocol::UserMessageItem { parts: vec![InputPart::text("!not a command part")], op_id: None })), rev);
    let mut agents = tool("cargo build");
    agents.source = None;
    t.upsert(mk("c3", yaac_protocol::ItemBody::ToolCall(agents)), rev);
    let history = compose::shell_history(&c.state, &k);
    let tail: Vec<&str> = history.iter().map(String::as_str).rev().take(2).collect();
    assert_eq!(tail, ["git log -1", "ls"], "newest last; the live pair counted once; the agent's own commands left out");
    assert!(!history.iter().any(|h| h == "cargo build" || h == "not a command part"));
}
