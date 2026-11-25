//! The hub against fake daemons: caching, replay after a dropped connection, reset, restart,
//! several windows, the single-hub lock, target management, and the hub's lifecycle.

mod support;

use std::time::Duration;

use serde_json::json;
use support::fake_daemon::Subscribed;
use support::{BUILD, FakeDaemon, Home, cached_text, wait_until};
use yaac_hub::protocol::{codes, names as ext};
use yaac_hub::{ExitReason, HubError};
use yaac_protocol::attach::{self, ConnectionStatus};
use yaac_protocol::rpc::Message;
use yaac_protocol::{ItemBody, SubscribeMode, ThreadId, ThreadSubscribeResult};

fn thread_events(messages: &[Message]) -> Vec<(String, u64)> {
    messages
        .iter()
        .filter_map(|m| match m {
            Message::Notification { method, params } if method == attach::names::NOTIFICATION && params["method"] == "thread/event" => {
                Some((params["params"]["threadId"].as_str()?.to_owned(), params["params"]["seq"].as_u64()?))
            }
            _ => None,
        })
        .collect()
}

#[test]
fn a_turns_events_reach_the_cache() {
    let home = Home::new();
    let fake = FakeDaemon::new("alpha");
    home.fakes.add(&fake);
    fake.add_thread("t1");
    let first = fake.turn("t1", "u1", 5); // happened before anyone looked: a reset rebuilds it

    let hub = home.start();
    let c = hub.client();
    let target = c.add_target("alpha").id;
    c.wait_status(&target, ConnectionStatus::Connected);

    let answer: ThreadSubscribeResult = serde_json::from_value(c.subscribe(&target, "t1")).unwrap();
    let SubscribeMode::Reset(reset) = answer.mode else { panic!("expected a reset for a window with no position") };
    let head = fake.head("t1");
    assert_eq!(reset.head_seq, head.seq, "answered once the cache holds the snapshot");
    assert_eq!(cached_text(&home, &target, "t1", "u1-msg"), Some((first.clone(), head.epoch.0, head.seq.0)));

    // A live turn, streamed.
    let second = fake.turn("t1", "u2", 20);
    let head = fake.head("t1");
    wait_until("live turn cached", || cached_text(&home, &target, "t1", "u2-msg").is_some_and(|(_, _, seq)| seq == head.seq.0));
    assert_eq!(cached_text(&home, &target, "t1", "u2-msg").unwrap().0, second);
    assert_eq!(cached_text(&home, &target, "t1", "u1-msg").unwrap().0, first, "older items kept");

    // The window got every live event, in order, after the hub committed it.
    wait_until("events forwarded", || {
        let seqs: Vec<u64> = thread_events(&c.drain()).into_iter().map(|(_, s)| s).collect();
        !seqs.is_empty() && seqs.windows(2).all(|w| w[1] == w[0] + 1) && *seqs.last().unwrap() == head.seq.0
    });

    // The hub acknowledges what it committed.
    wait_until("acked", || fake.acks().iter().any(|a| a.seq == head.seq));

    // The thread index is cached too.
    let threads = home.cache().threads(&target).unwrap();
    assert_eq!(threads.len(), 1);
    assert_eq!(threads[0].id, ThreadId::new("t1"));
}

#[test]
fn a_connection_killed_mid_stream_loses_nothing_and_duplicates_nothing() {
    let home = Home::new();
    let fake = FakeDaemon::new("alpha");
    home.fakes.add(&fake);
    fake.add_thread("t1");
    let hub = home.start();
    let c = hub.client();
    let target = c.add_target("alpha").id;
    c.wait_status(&target, ConnectionStatus::Connected);
    c.subscribe(&target, "t1");

    // Stream a long turn from another thread, and kill the link twice while it streams.
    let streamer = {
        let fake = fake.clone();
        std::thread::spawn(move || {
            let mut text = String::new();
            fake.emit("t1", yaac_protocol::ThreadEvent::ItemStarted(yaac_protocol::ItemEvent {
                item: support::fake_daemon::agent_item("long", "u1", "", yaac_protocol::ItemStatus::InProgress),
            }));
            for i in 0..600 {
                let delta = format!("<{i}>");
                text.push_str(&delta);
                fake.emit(
                    "t1",
                    yaac_protocol::ThreadEvent::ItemDelta(yaac_protocol::ItemDeltaEvent {
                        item_id: yaac_protocol::ItemId::new("long"),
                        field: yaac_protocol::DeltaField::Text,
                        delta,
                    }),
                );
                if i % 10 == 0 {
                    std::thread::sleep(Duration::from_millis(3));
                }
            }
            text
        })
    };
    wait_until("some deltas cached", || cached_text(&home, &target, "t1", "long").is_some_and(|(t, _, _)| t.len() > 200));
    fake.kill_connections();
    std::thread::sleep(Duration::from_millis(30));
    wait_until("reconnected", || fake.connected());
    fake.kill_connections();
    assert!(!streamer.is_finished(), "both kills landed while the turn was streaming");
    let expected = streamer.join().unwrap();

    let head = fake.head("t1");
    wait_until("caught up", || cached_text(&home, &target, "t1", "long").is_some_and(|(_, _, seq)| seq == head.seq.0));
    let (text, epoch, seq) = cached_text(&home, &target, "t1", "long").unwrap();
    assert_eq!(text, expected, "every delta exactly once, in order");
    assert_eq!((epoch, seq), (head.epoch.0, head.seq.0));
    let replays = fake.subscribes().iter().filter(|(_, s)| matches!(s, Subscribed::Replay { .. })).count();
    assert!(replays >= 1, "reconnects resumed by replay: {:?}", fake.subscribes());
    wait_until("third connection", || fake.connects() >= 3);
}

#[test]
fn a_reset_rebuilds_the_cached_thread() {
    let home = Home::new();
    let fake = FakeDaemon::new("alpha");
    home.fakes.add(&fake);
    fake.add_thread("t1");
    fake.turn("t1", "u1", 3);
    let hub = home.start();
    let c = hub.client();
    let target = c.add_target("alpha").id;
    c.wait_status(&target, ConnectionStatus::Connected);
    c.subscribe(&target, "t1");
    let before = fake.head("t1");
    assert_eq!(cached_text(&home, &target, "t1", "u1-msg").unwrap().2, before.seq.0);

    // The daemon starts a new journal (the old one could not be migrated) while the link is down.
    fake.kill_connections();
    fake.new_epoch("t1");
    let text = fake.turn("t1", "u2", 4);
    let after = fake.head("t1");
    assert_ne!(after.epoch, before.epoch);

    wait_until("rebuilt at the new epoch", || {
        cached_text(&home, &target, "t1", "u2-msg").is_some_and(|(_, e, s)| e == after.epoch.0 && s == after.seq.0)
    });
    assert_eq!(cached_text(&home, &target, "t1", "u2-msg").unwrap().0, text);
    let cached = home.cache().thread(&target, &ThreadId::new("t1")).unwrap().unwrap();
    assert_eq!(cached.items, fake.items("t1"), "the cache is exactly the snapshot");
    assert!(fake.subscribes().iter().any(|(_, s)| *s == Subscribed::Reset { epoch: after.epoch.0 }));
    // The window is told to re-read.
    wait_until("reset notified", || c.drain().iter().any(|m| m.method() == Some(ext::THREAD_RESET)));

    // Events after the snapshot are applied on top of it.
    let more = fake.turn("t1", "u3", 2);
    let head = fake.head("t1");
    wait_until("live after reset", || cached_text(&home, &target, "t1", "u3-msg").is_some_and(|(_, _, s)| s == head.seq.0));
    assert_eq!(cached_text(&home, &target, "t1", "u3-msg").unwrap().0, more);
}

#[test]
fn the_cache_survives_a_hub_restart_and_the_next_hub_resumes_by_replay() {
    let home = Home::new();
    let fake = FakeDaemon::new("alpha");
    home.fakes.add(&fake);
    fake.add_thread("t1");
    let hub = home.start();
    let target = {
        let c = hub.client();
        let target = c.add_target("alpha").id;
        c.wait_status(&target, ConnectionStatus::Connected);
        c.subscribe(&target, "t1");
        fake.turn("t1", "u1", 10);
        let head = fake.head("t1");
        wait_until("cached", || cached_text(&home, &target, "t1", "u1-msg").is_some_and(|(_, _, s)| s == head.seq.0));
        target
    };
    assert_eq!(hub.shutdown().reason, ExitReason::Shutdown);

    // While no hub runs, the target keeps working.
    let text = fake.turn("t1", "u2", 7);
    let head = fake.head("t1");
    // The cache is still there, at the old position.
    let cached = cached_text(&home, &target, "t1", "u1-msg").expect("cache survived");
    assert!(cached.2 < head.seq.0);

    let hub = home.start();
    let c = hub.client();
    c.wait_status(&target, ConnectionStatus::Connected);
    assert_eq!(c.targets().len(), 1, "targets persisted");
    c.subscribe(&target, "t1");
    wait_until("caught up after restart", || cached_text(&home, &target, "t1", "u2-msg").is_some_and(|(_, _, s)| s == head.seq.0));
    assert_eq!(cached_text(&home, &target, "t1", "u2-msg").unwrap().0, text);
    let last = fake.subscribes().last().cloned().unwrap();
    assert_eq!(last.1, Subscribed::Replay { epoch: head.epoch.0, from: cached.2 + 1 }, "resumed from the stored position");
}

#[test]
fn a_second_window_sees_the_same_state() {
    let home = Home::new();
    let fake = FakeDaemon::new("alpha");
    home.fakes.add(&fake);
    fake.add_thread("t1");
    let hub = home.start();
    let a = hub.client();
    let target = a.add_target("alpha").id;
    a.wait_status(&target, ConnectionStatus::Connected);
    a.subscribe(&target, "t1");
    fake.turn("t1", "u1", 4);

    let b = hub.client();
    let hello = b.call(attach::names::HELLO, json!({ "build": BUILD })).unwrap();
    assert_eq!(hello["targets"], json!(a.targets()), "same targets and states");
    let head = fake.head("t1");
    wait_until("cached", || cached_text(&home, &target, "t1", "u1-msg").is_some_and(|(_, _, s)| s == head.seq.0));
    let answer: ThreadSubscribeResult = serde_json::from_value(b.subscribe(&target, "t1")).unwrap();
    match answer.mode {
        SubscribeMode::Reset(r) => assert_eq!(r.head_seq, head.seq),
        other => panic!("{other:?}"),
    }
    // One upstream subscription serves both windows.
    assert_eq!(fake.subscribes().len(), 1);
    // Both get the next turn's events; thread/read through the hub gives the same items.
    a.drain();
    b.drain();
    fake.turn("t1", "u2", 3);
    let head = fake.head("t1");
    for w in [&a, &b] {
        wait_until("forwarded", || thread_events(&w.drain()).iter().any(|(_, s)| *s == head.seq.0));
    }
    let read = b.routed(&target, "thread/read", json!({ "threadId": "t1" })).unwrap();
    assert!(read["items"].as_array().is_some_and(|i| !i.is_empty()));
    // Window A leaving does not end B's subscription.
    drop(a);
    std::thread::sleep(Duration::from_millis(100));
    fake.turn("t1", "u3", 1);
    let head = fake.head("t1");
    wait_until("still live for B", || cached_text(&home, &target, "t1", "u3-msg").is_some_and(|(_, _, s)| s == head.seq.0));
}

#[test]
fn a_second_hub_refuses_to_start() {
    let home = Home::new();
    let hub = home.start();
    match yaac_hub::run_hub(home.config()) {
        Err(HubError::AlreadyRunning(_)) => {}
        other => panic!("expected AlreadyRunning, got {other:?}"),
    }
    // The first hub is unaffected.
    let c = hub.client();
    assert!(c.targets().is_empty());
}

#[test]
fn targets_add_update_enable_and_remove_persist() {
    let home = Home::new();
    let alpha = FakeDaemon::new("alpha");
    let beta = FakeDaemon::new("beta");
    home.fakes.add(&alpha);
    home.fakes.add(&beta);
    alpha.add_thread("t1");
    let hub = home.start();
    let (a_id, b_id) = {
        let c = hub.client();
        let a = c.add_target("alpha");
        let b = c.add_target("beta");
        assert_ne!(a.id, b.id);
        let dup = c.call(attach::names::TARGETS_ADD, json!({ "name": " Alpha ", "transport": { "kind": "local" }, "enabled": true }));
        assert_eq!(dup.unwrap_err().code.as_str(), codes::NAME_TAKEN);
        c.wait_status(&a.id, ConnectionStatus::Connected);
        c.subscribe(&a.id, "t1");
        alpha.turn("t1", "u1", 2);
        let head = alpha.head("t1");
        wait_until("cached", || cached_text(&home, &a.id, "t1", "u1-msg").is_some_and(|(_, _, s)| s == head.seq.0));
        c.call(attach::names::TARGETS_ENABLE, json!({ "id": b.id, "enabled": false })).unwrap();
        c.wait_status(&b.id, ConnectionStatus::Disabled);
        wait_until("beta dropped", || !beta.connected());
        c.call(attach::names::TARGETS_UPDATE, json!({ "id": a.id, "name": "alpha" })).unwrap();
        (a.id, b.id)
    };
    hub.shutdown();

    let hub = home.start();
    let c = hub.client();
    let targets = c.targets();
    assert_eq!(targets.iter().map(|t| (t.id.clone(), t.name.as_str().to_owned(), t.enabled)).collect::<Vec<_>>(), vec![
        (a_id.clone(), "alpha".to_owned(), true),
        (b_id.clone(), "beta".to_owned(), false)
    ]);
    assert!(!beta.connected(), "a disabled target stays disconnected across restarts");
    assert!(home.cache().target_row_count(&a_id).unwrap() > 0);

    c.call(attach::names::TARGETS_REMOVE, json!({ "id": a_id })).unwrap();
    wait_until("rows deleted", || home.cache().target_row_count(&a_id).unwrap() == 0);
    wait_until("alpha dropped", || !alpha.connected());
    assert_eq!(c.targets().len(), 1);
    let settings = std::fs::read_to_string(home.paths().settings_file()).unwrap();
    assert!(!settings.contains(a_id.as_str()), "removed from the settings file");
    assert!(settings.contains(b_id.as_str()));
}

#[test]
fn builds_are_compared_at_hello() {
    let home = Home::new();
    let hub = home.start();
    // An older window is told to update and may not replace the hub.
    let old = yaac_hub::client::HubClient::connect(&hub.socket).unwrap();
    let e = old.request_blocking(attach::names::HELLO, json!({ "build": "0.0.9" }), support::WAIT).unwrap_err();
    assert_eq!(e.code.as_str(), codes::WINDOW_OUTDATED);
    let e = old.request_blocking(attach::names::SHUTDOWN, json!({}), support::WAIT).unwrap_err();
    assert_eq!(e.code.as_str(), codes::WINDOW_OUTDATED);
    // A request before hello is refused.
    let fresh = yaac_hub::client::HubClient::connect(&hub.socket).unwrap();
    let e = fresh.request_blocking(attach::names::TARGETS_LIST, json!({}), support::WAIT).unwrap_err();
    assert_eq!(e.code.as_str(), codes::HELLO_REQUIRED);
    // A newer window may only ask the hub to checkpoint and exit.
    let new = yaac_hub::client::HubClient::connect(&hub.socket).unwrap();
    let r = new.request_blocking(attach::names::HELLO, json!({ "build": "9.0.0" }), support::WAIT).unwrap();
    assert_eq!(r["build"], BUILD);
    let e = new.request_blocking(attach::names::TARGETS_LIST, json!({}), support::WAIT).unwrap_err();
    assert_eq!(e.code.as_str(), codes::HUB_OUTDATED);
    new.request_blocking(attach::names::SHUTDOWN, json!({}), support::WAIT).unwrap();
    let mut hub = hub;
    assert_eq!(hub.join().reason, ExitReason::Shutdown);
}

#[test]
fn an_idle_hub_exits_in_idle_exit_mode_and_always_is_the_default() {
    // The default is "always": a fresh hub writes it and does not exit when idle.
    let home = Home::new();
    let mut config = home.config();
    config.idle_exit = Some(Duration::from_millis(200));
    let hub = home.start_with(config);
    std::thread::sleep(Duration::from_millis(600));
    assert!(!hub.thread.as_ref().unwrap().is_finished(), "always is the default");
    assert_eq!(hub.client().call(ext::SETTINGS_GET, json!({})).unwrap()["settings"]["always"], true);
    hub.shutdown();

    // Idle-exit mode, the non-default option.
    let home = Home::new();
    let mut config = home.config();
    config.first_run_always = false;
    config.idle_exit = Some(Duration::from_millis(300));
    let mut hub = home.start_with(config.clone());
    assert_eq!(hub.join().reason, ExitReason::Idle);

    // Attached windows keep it up.
    let mut hub = home.start_with(config);
    let c = hub.client();
    std::thread::sleep(Duration::from_millis(700));
    assert!(!hub.thread.as_ref().unwrap().is_finished(), "a window is attached");
    // "always" keeps it up with no window.
    c.call(ext::SETTINGS_UPDATE, json!({ "always": true })).unwrap();
    drop(c);
    std::thread::sleep(Duration::from_millis(700));
    assert!(!hub.thread.as_ref().unwrap().is_finished(), "always");
    let c = hub.client();
    let status = c.call(ext::STATUS, json!({})).unwrap();
    assert_eq!(status["settings"]["always"], true);
    c.call(ext::SETTINGS_UPDATE, json!({ "always": false })).unwrap();
    drop(c);
    assert_eq!(hub.join().reason, ExitReason::Idle);
}

#[test]
fn a_running_thread_keeps_the_hub_up() {
    let home = Home::new();
    let fake = FakeDaemon::new("alpha");
    home.fakes.add(&fake);
    fake.add_thread("t1");
    let mut config = home.config();
    config.first_run_always = false;
    config.idle_exit = Some(Duration::from_millis(300));
    let mut hub = home.start_with(config);
    {
        let c = hub.client();
        let target = c.add_target("alpha").id;
        c.wait_status(&target, ConnectionStatus::Connected);
        // The index says the thread is running.
        let mut running = support::fake_daemon::summary("t1");
        running.state = yaac_protocol::ThreadState::Running;
        fake.originate_notification_thread_updated(&running);
        let status = || c.call(ext::STATUS, json!({})).unwrap();
        wait_until("running seen", || status()["busy"]["activeThreads"] == 1);
    }
    std::thread::sleep(Duration::from_millis(700));
    assert!(!hub.thread.as_ref().unwrap().is_finished(), "a running thread keeps it up");
    let mut idle = support::fake_daemon::summary("t1");
    idle.state = yaac_protocol::ThreadState::Idle;
    fake.originate_notification_thread_updated(&idle);
    assert_eq!(hub.join().reason, ExitReason::Idle);
}

#[test]
fn routed_requests_pass_through_and_the_cache_answers_while_offline() {
    let home = Home::new();
    let fake = FakeDaemon::new("alpha");
    home.fakes.add(&fake);
    fake.add_thread("t1");
    fake.turn("t1", "u0", 2);
    fake.turn("t1", "u1", 3);
    let hub = home.start();
    let c = hub.client();
    let target = c.add_target("alpha").id;
    c.wait_status(&target, ConnectionStatus::Connected);
    let sync = c.routed(&target, "thread/sync", json!({ "token": "" })).unwrap();
    assert_eq!(sync["changed"][0]["id"], "t1");
    c.subscribe(&target, "t1");
    let ping = c.routed(&target, "ping", json!({})).unwrap();
    assert_eq!(ping, json!({}));
    let e = c.routed(&target, "initialize", json!({})).unwrap_err();
    assert_eq!(e.code.as_str(), codes::NOT_ROUTABLE);
    let e = c.routed(&yaac_protocol::TargetId::new("nope"), "ping", json!({})).unwrap_err();
    assert_eq!(e.code.as_str(), "target_unknown");

    fake.set_offline(true);
    c.wait_status(&target, ConnectionStatus::Offline);
    let sync = c.routed(&target, "thread/sync", json!({ "token": "" })).unwrap();
    assert_eq!(sync["changed"][0]["id"], "t1", "served from the cache");
    let answer: ThreadSubscribeResult = serde_json::from_value(c.subscribe(&target, "t1")).unwrap();
    let head = fake.head("t1");
    assert!(matches!(answer.mode, SubscribeMode::Reset(r) if r.head_seq == head.seq));
    let read = c.routed(&target, "thread/read", json!({ "threadId": "t1", "maxBytes": 1 })).unwrap();
    assert_eq!(read["position"]["seq"], head.seq.0);
    assert!(read["cursor"].is_string(), "paged by bytes");
    let mut items = read["items"].as_array().unwrap().clone();
    let mut cursor = read["cursor"].clone();
    while cursor.is_string() {
        let page = c.routed(&target, "thread/read", json!({ "threadId": "t1", "maxBytes": 1, "cursor": cursor })).unwrap();
        items.extend(page["items"].as_array().unwrap().iter().cloned());
        cursor = page["cursor"].clone();
    }
    assert_eq!(items.len(), fake.items("t1").len());
    let e = c.routed(&target, "ping", json!({})).unwrap_err();
    assert_eq!(e.code.as_str(), "target_offline");

    // Back online: the open thread resumes by replay.
    fake.set_offline(false);
    c.call(ext::TARGETS_RECONNECT, json!({ "id": target })).unwrap();
    c.wait_status(&target, ConnectionStatus::Connected);
    let text = fake.turn("t1", "u2", 2);
    let head = fake.head("t1");
    wait_until("resumed", || cached_text(&home, &target, "t1", "u2-msg").is_some_and(|(_, _, s)| s == head.seq.0));
    assert_eq!(cached_text(&home, &target, "t1", "u2-msg").unwrap().0, text);
}

#[test]
fn drafts_are_durable_and_clear_cache_does_not_touch_them() {
    let home = Home::new();
    let fake = FakeDaemon::new("alpha");
    home.fakes.add(&fake);
    fake.add_thread("t1");
    let hub = home.start();
    let target = {
        let c = hub.client();
        let target = c.add_target("alpha").id;
        c.wait_status(&target, ConnectionStatus::Connected);
        c.subscribe(&target, "t1");
        fake.turn("t1", "u1", 2);
        let head = fake.head("t1");
        wait_until("cached", || cached_text(&home, &target, "t1", "u1-msg").is_some_and(|(_, _, s)| s == head.seq.0));
        c.call(ext::DRAFTS_PUT, json!({ "key": "k1", "target": target, "threadId": "t1", "text": "half a thought" })).unwrap();
        c.call(ext::UI_STATE_SET, json!({ "key": "collapsed", "value": ["g1"] })).unwrap();
        assert_eq!(home.cache().ui_state("collapsed").unwrap(), Some(json!(["g1"])));
        c.call(ext::CACHE_CLEAR, json!({})).unwrap();
        // Cleared, then rebuilt from the target (the thread is still open).
        let head = fake.head("t1");
        wait_until("rebuilt after clear", || cached_text(&home, &target, "t1", "u1-msg").is_some_and(|(_, _, s)| s == head.seq.0));
        assert_eq!(home.cache().ui_state("collapsed").unwrap(), None);
        target
    };
    hub.shutdown();
    // A cache with another schema is dropped and rebuilt; drafts are not.
    rusqlite_free_schema_bump(&home);
    let hub = home.start();
    let c = hub.client();
    let drafts = c.call(ext::DRAFTS_LIST, json!({})).unwrap();
    assert_eq!(drafts["drafts"][0]["text"], "half a thought");
    assert_eq!(drafts["drafts"][0]["target"], json!(target));
    assert!(home.cache().thread(&target, &ThreadId::new("t1")).unwrap().is_none(), "the old cache was dropped");
}

/// Simulates a cache written by a build with another schema: the hub must drop it.
fn rusqlite_free_schema_bump(home: &Home) {
    // `user_version` is a 4-byte big-endian integer at offset 60 of the database header.
    let path = home.paths().cache_file();
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[60..64].copy_from_slice(&999u32.to_be_bytes());
    std::fs::write(&path, bytes).unwrap();
}

#[test]
fn secret_answers_are_never_written_to_the_cache() {
    use yaac_protocol::{
        ItemStatus, Question, QuestionAnswer, QuestionAnswers, QuestionId, QuestionRequest, RequestBody, RequestId, RequestItem,
        RequestResolution, RequestResponse, RequestState,
    };
    let home = Home::new();
    let fake = FakeDaemon::new("alpha");
    home.fakes.add(&fake);
    fake.add_thread("t1");
    let hub = home.start();
    let c = hub.client();
    let target = c.add_target("alpha").id;
    c.wait_status(&target, ConnectionStatus::Connected);
    c.subscribe(&target, "t1");
    let q = |id: &str, secret: bool| Question {
        id: QuestionId::new(id),
        header: None,
        text: id.into(),
        options: vec![],
        multi_select: false,
        allow_other: true,
        secret,
    };
    let mut answers = std::collections::BTreeMap::new();
    answers.insert(QuestionId::new("token"), QuestionAnswer { selected: vec![], other: Some("hunter2".into()), redacted: false });
    answers.insert(QuestionId::new("name"), QuestionAnswer { selected: vec![], other: Some("Ada".into()), redacted: false });
    let item = yaac_protocol::Item {
        id: yaac_protocol::ItemId::new("r1"),
        turn_id: yaac_protocol::TurnId::new("u1"),
        status: ItemStatus::Completed,
        started_at: None,
        completed_at: None,
        body: ItemBody::Request(RequestItem {
            request_id: RequestId::new("q1"),
            blocking: true,
            state: RequestState::Resolved,
            resolution: Some(RequestResolution {
                by: yaac_protocol::AnsweredBy::Client,
                client_id: None,
                client_host: None,
                answer: Some(RequestResponse::Question(QuestionAnswers { answers })),
                at: None,
                automatic: None,
            }),
            body: RequestBody::Question(QuestionRequest { questions: vec![q("token", true), q("name", false)] }),
        }),
    };
    fake.emit("t1", yaac_protocol::ThreadEvent::ItemCompleted(yaac_protocol::ItemEvent { item }));
    let head = fake.head("t1");
    wait_until("cached", || home.cache().thread(&target, &ThreadId::new("t1")).unwrap().is_some_and(|t| t.position.seq == head.seq));
    let raw = std::fs::read(home.paths().cache_file()).unwrap();
    let wal = std::fs::read(format!("{}-wal", home.paths().cache_file().display())).unwrap_or_default();
    let has = |needle: &str| raw.windows(needle.len()).any(|w| w == needle.as_bytes()) || wal.windows(needle.len()).any(|w| w == needle.as_bytes());
    assert!(!has("hunter2"), "the secret answer reached the cache file");
    assert!(has("Ada"), "non-secret answers are cached");
}
