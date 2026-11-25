//! Queue or send now while a turn runs (D166): the queue from the snapshot and its events,
//! the requests the window sends, and what the composer offers.
mod common;

use common::{boot, settle};
use serde_json::Value;
use yaac_core::attention::AttentionMap;
use yaac_core::client::Client;
use yaac_core::demo::{DemoHub, Scene, threads};
use yaac_core::header::{ComposerView, composer};
use yaac_core::queue;
use yaac_core::ThreadKey;
use yaac_protocol::ItemBody;

/// billing-service (Claude Code, cannot steer) working, with two messages queued.
fn running() -> (Client, DemoHub, ThreadKey) {
    let (mut c, hub) = boot();
    let k = threads::billing();
    c.open(&k);
    settle(&mut c, &hub);
    hub.play(Scene::QueueWhileRunning);
    settle(&mut c, &hub);
    (c, hub, k)
}

fn type_in(c: &mut Client, k: &ThreadKey, text: &str) {
    c.state.drafts.entry(k.clone()).or_default().text = text.into();
}

fn sent(hub: &DemoHub, method: &str) -> Vec<Value> {
    hub.requests().into_iter().filter(|(_, m, _)| m == method).map(|(_, _, p)| p).collect()
}

fn queue_ids(c: &Client, k: &ThreadKey) -> Vec<String> {
    c.state.transcripts[k].queued.iter().map(|q| q.queue_id.clone()).collect()
}

fn user_texts(c: &Client, k: &ThreadKey) -> Vec<String> {
    c.state.transcripts[k]
        .items()
        .iter()
        .filter_map(|i| match &i.body {
            ItemBody::UserMessage(u) => Some(u.parts.iter().filter_map(|p| match p {
                yaac_protocol::InputPart::Text(t) => Some(t.text.clone()),
                _ => None,
            }).collect::<String>()),
            _ => None,
        })
        .collect()
}

#[test]
fn the_queue_comes_from_the_events_and_from_a_fresh_snapshot() {
    let (mut c, hub, k) = running();
    assert_eq!(queue_ids(&c, &k), ["bq-1", "bq-2"], "from queueChanged added, in order");
    let strip = queue::strip(&c.state, &k);
    assert_eq!(strip[0].preview, "Also log each retry at debug level, with the attempt number.");
    // A fresh window (no position) gets them in the subscription's snapshot.
    let mut c2 = Client::new(Box::new(hub.clone()), "test", yaac_protocol::ClientId::new(yaac_core::demo::DEMO_CLIENT));
    settle(&mut c2, &hub);
    c2.open(&k);
    settle(&mut c2, &hub);
    assert_eq!(queue_ids(&c2, &k), ["bq-1", "bq-2"]);
    // Delivered at the turn's end: it leaves the queue and is the user's message of the next turn.
    hub.finish_turn(&k);
    c.pump();
    assert_eq!(queue_ids(&c, &k), ["bq-2"]);
    assert_eq!(user_texts(&c, &k).last().map(String::as_str), Some("Also log each retry at debug level, with the attempt number."));
    settle(&mut c, &hub);
    assert!(queue_ids(&c, &k).is_empty(), "each turn's end delivers the next");
}

#[test]
fn enter_while_running_queues_and_the_draft_clears() {
    let (mut c, hub, k) = running();
    type_in(&mut c, &k, "And bump the crate version.");
    assert!(c.send(&k));
    let p = sent(&hub, "turn/start").pop().expect("turn/start");
    assert_eq!(p["delivery"], "queue");
    assert!(p["opId"].as_str().is_some_and(|o| !o.is_empty()));
    assert!(c.state.drafts.get(&k).is_none_or(|d| d.is_empty()), "the draft clears");
    c.pump();
    assert_eq!(queue_ids(&c, &k).len(), 3, "the strip shows it, last");
    assert_eq!(queue::strip(&c.state, &k)[2].preview, "And bump the crate version.");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
}

#[test]
fn send_now_asks_now_and_the_daemon_interrupts_where_it_cannot_steer() {
    let (mut c, hub, k) = running();
    let map = AttentionMap::compute(&c.state);
    let ComposerView::Shown { running, send_now, stop, .. } = composer(&c.state, &map, &k) else { panic!("composer") };
    assert!(running && stop);
    assert_eq!(send_now, Some(yaac_core::queue::SEND_NOW_INTERRUPTS), "Claude Code cannot steer");
    type_in(&mut c, &k, "Stop: use the shared retry helper instead.");
    assert!(c.send_now(&k));
    assert_eq!(sent(&hub, "turn/start").pop().unwrap()["delivery"], "now");
    assert!(sent(&hub, "turn/steer").is_empty(), "the window does not decide steer itself");
    c.pump();
    let t = &c.state.transcripts[&k];
    assert_ne!(t.active_turn.as_ref().map(|t| t.as_str()), Some("b-run"), "interrupted, and a new turn started");
    assert_eq!(user_texts(&c, &k).last().map(String::as_str), Some("Stop: use the shared retry helper instead."));
}

#[test]
fn a_codex_thread_steers_and_says_send_now() {
    let (mut c, hub) = boot();
    let k = threads::notes();
    c.open(&k);
    settle(&mut c, &hub);
    // Start a turn, then send now while it streams.
    type_in(&mut c, &k, "Draft the release notes.");
    assert!(c.send(&k));
    // The demo answers a started turn a tick later (the echo shows "Sending…" meanwhile, D165).
    c.pump();
    hub.tick();
    c.pump();
    let map = AttentionMap::compute(&c.state);
    let ComposerView::Shown { send_now, .. } = composer(&c.state, &map, &k) else { panic!() };
    assert_eq!(send_now, Some(yaac_core::queue::SEND_NOW_STEERS));
    type_in(&mut c, &k, "Keep it under 200 words.");
    assert!(c.send_now(&k));
    c.pump();
    let turn = c.state.transcripts[&k].active_turn.clone();
    let last = c.state.transcripts[&k].items().last().cloned().unwrap();
    assert!(matches!(last.body, ItemBody::UserMessage(_)), "steered into the running turn");
    assert_eq!(Some(last.turn_id), turn);
}

#[test]
fn edit_cancel_and_send_now_on_queued_messages() {
    let (mut c, hub, k) = running();
    // Edit: loads into the composer's edit; Enter (send) saves it with queueEdit.
    c.begin_queue_edit(&k, "bq-1");
    assert!(queue::strip(&c.state, &k)[0].editing);
    assert_eq!(c.state.queue_edits[&k].text, "Also log each retry at debug level, with the attempt number.");
    c.state.queue_edits.get_mut(&k).unwrap().text = "Also log each retry at info level.".into();
    assert!(c.send(&k));
    let p = sent(&hub, "turn/queueEdit").pop().expect("queueEdit");
    assert_eq!(p["queueId"], "bq-1");
    assert_eq!(p["input"][0]["text"], "Also log each retry at info level.");
    assert!(sent(&hub, "turn/start").is_empty(), "saving an edit sends no new message");
    assert!(c.state.queue_busy(&k, "bq-1"), "its buttons wait");
    c.pump();
    assert!(!c.state.queue_busy(&k, "bq-1"));
    assert_eq!(queue::strip(&c.state, &k)[0].preview, "Also log each retry at info level.");
    // Cancel.
    c.queue_cancel(&k, "bq-2");
    assert_eq!(sent(&hub, "turn/queueCancel").pop().unwrap()["queueId"], "bq-2");
    c.pump();
    assert_eq!(queue_ids(&c, &k), ["bq-1"]);
    // Send now: promoted out of the queue, delivered at once (interrupting).
    c.queue_promote(&k, "bq-1");
    let p = sent(&hub, "turn/queuePromote").pop().unwrap();
    assert_eq!(p["queueId"], "bq-1");
    c.pump();
    assert!(queue_ids(&c, &k).is_empty());
    assert_eq!(user_texts(&c, &k).last().map(String::as_str), Some("Also log each retry at info level."));
    // Each request had its own operation id.
    let ops: std::collections::BTreeSet<String> = hub
        .requests()
        .into_iter()
        .filter(|(_, m, _)| m.starts_with("turn/queue"))
        .map(|(_, _, p)| p["opId"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(ops.len(), 3);
}

#[test]
fn an_edit_of_a_message_delivered_meanwhile_gives_the_text_back() {
    let (mut c, hub, k) = running();
    c.begin_queue_edit(&k, "bq-1");
    c.state.queue_edits.get_mut(&k).unwrap().text = "edited too late".into();
    hub.finish_turn(&k);
    c.pump();
    assert!(c.send(&k));
    settle(&mut c, &hub);
    assert_eq!(c.state.drafts[&k].text, "edited too late", "nothing typed is lost");
    assert!(c.errors.iter().any(|e| e.text.contains("delivered before your edit was saved")), "{:?}", c.errors);
}

#[test]
fn while_a_request_waits_only_queue() {
    let (mut c, hub, k) = running();
    // An approval arrives in the running turn.
    hub.ask_approval(&k);
    settle(&mut c, &hub);
    assert!(queue::awaiting_user(&c.state, &k));
    let map = AttentionMap::compute(&c.state);
    let ComposerView::Shown { send_now, dimmed_hint, running, .. } = composer(&c.state, &map, &k) else { panic!() };
    assert!(running);
    assert_eq!(send_now, None, "only Queue");
    assert!(dimmed_hint.is_some_and(|h| h.contains("waits in the queue")));
    // Ctrl+Enter still only queues; Send now on a queued message is not asked.
    type_in(&mut c, &k, "after you answer: also update the changelog");
    assert!(c.send_now(&k));
    assert_eq!(sent(&hub, "turn/start").pop().unwrap()["delivery"], "queue");
    c.queue_promote(&k, "bq-1");
    assert!(sent(&hub, "turn/queuePromote").is_empty());
}

#[test]
fn a_lost_answer_is_said_and_never_sent_again() {
    let (mut c, hub, k) = running();
    hub.lose_next_answer();
    c.queue_cancel(&k, "bq-1");
    settle(&mut c, &hub);
    assert!(c.errors.iter().any(|e| e.text.contains("will not be asked again")), "{:?}", c.errors);
    assert_eq!(sent(&hub, "turn/queueCancel").len(), 1, "not re-sent");
    assert!(!c.state.queue_busy(&k, "bq-1"));
}
