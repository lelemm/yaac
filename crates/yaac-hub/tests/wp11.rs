//! The hub's part of the discovery, dismiss, and diagnostics follow-ups: `dismissed` and the
//! discovery caps in the cache, an id lookup that misses the index routed to the targets
//! (`hub/threads/lookup`, and `hub/threads/find` falling through to it), the connection-step
//! log, and the desktop notification when a thread needs the user and no window is attached.

mod support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;
use support::{FakeDaemon, Home, wait_until};
use yaac_hub::notify::Notifier;
use yaac_protocol::attach::ConnectionStatus;
use yaac_protocol::{DiscoveryCaps, ThreadState};

#[derive(Default)]
struct Recorder(Mutex<Vec<(String, String)>>);

impl Notifier for Recorder {
    fn notify(&self, title: &str, body: &str) {
        self.0.lock().unwrap().push((title.to_owned(), body.to_owned()));
    }
}

#[test]
fn the_discovery_caps_are_carried_in_the_cache() {
    let home = Home::new();
    let alpha = FakeDaemon::new("alpha");
    alpha.set_caps(DiscoveryCaps { enabled: true, max_age_days: 10, max_threads: 7, ..DiscoveryCaps::default() });
    home.fakes.add(&alpha);
    let hub = home.start();
    let c = hub.client();
    let a = c.add_target("alpha").id;
    c.wait_status(&a, ConnectionStatus::Connected);
    wait_until("the caps read on connect", || home.cache().discovery(&a).unwrap().is_some_and(|d| d.max_threads == 7 && d.max_age_days == 10));
    // A change on the target is pushed and kept.
    alpha.originate_notification("discovery/updated", json!({ "caps": { "enabled": false, "maxAgeDays": 30, "maxThreads": 50 } }));
    wait_until("the changed caps", || home.cache().discovery(&a).unwrap().is_some_and(|d| !d.enabled));
}

/// Pin, mute (timed or not), and dismiss are the host's view preferences (D92): in the hub's
/// durable store, shared by every window, told to the targets only as "keep these threads".
#[test]
fn view_preferences_live_in_the_hub_persist_reach_every_window_and_keep_threads_on_the_target() {
    let home = Home::new();
    let alpha = FakeDaemon::new("alpha");
    alpha.add_thread("t1");
    alpha.add_thread("t2");
    alpha.add_thread("t3");
    home.fakes.add(&alpha);
    let a;
    {
        let hub = home.start();
        let c = hub.client();
        let other = hub.client();
        a = c.add_target("alpha").id;
        c.wait_status(&a, ConnectionStatus::Connected);
        wait_until("an empty keep set on connect", || alpha.keeps().last().is_some_and(Vec::is_empty));
        let r = c.call("hub/view/set", json!({ "target": a, "threadId": "t1", "pinned": true })).unwrap();
        assert_eq!(r["view"]["pinned"], true, "{r}");
        c.call("hub/view/set", json!({ "target": a, "threadId": "t2", "muted": true, "mutedUntil": "2030-01-01T00:00:00Z" })).unwrap();
        let many = c
            .call("hub/view/setMany", json!({ "entries": [
                { "target": a, "threadId": "t3", "dismissed": true },
                { "target": a, "threadId": "t4", "dismissed": true }
            ] }))
            .unwrap();
        assert_eq!(many["views"].as_array().unwrap().len(), 2);
        // The other window hears every change.
        wait_until("the second window to hear of it", || {
            other.drain().iter().any(|m| matches!(m, yaac_protocol::rpc::Message::Notification { method, params } if method == "hub/view/changed" && params["views"][0]["dismissed"] == true))
        });
        // The target is told only which threads to keep: the pinned and the muted ones.
        wait_until("the keep set", || {
            alpha.keeps().last().is_some_and(|k| {
                let mut k: Vec<&str> = k.iter().map(|t| t.as_str()).collect();
                k.sort_unstable();
                k == ["t1", "t2"]
            })
        });
        // A timed mute is just stored; unmuting clears the end too.
        c.call("hub/view/set", json!({ "target": a, "threadId": "t2", "mutedUntil": "2031-01-01T00:00:00Z" })).unwrap();
        let got = c.call("hub/view/get", json!({})).unwrap();
        let t2 = got["views"].as_array().unwrap().iter().find(|v| v["threadId"] == "t2").cloned().unwrap();
        assert_eq!(t2["mutedUntil"], "2031-01-01T00:00:00Z", "{t2}");
        // "Clear cache" does not touch them.
        c.call("hub/cache/clear", json!({})).unwrap();
        assert_eq!(c.call("hub/view/get", json!({})).unwrap()["views"].as_array().unwrap().len(), 4);
        hub.shutdown();
    }
    // They survive a hub restart, and the keep set is sent again on connect.
    let before = alpha.keeps().len();
    let hub = home.start();
    let c = hub.client();
    let got = c.call("hub/view/get", json!({})).unwrap();
    let views = got["views"].as_array().unwrap();
    assert_eq!(views.len(), 4, "{got}");
    assert!(views.iter().any(|v| v["threadId"] == "t1" && v["pinned"] == true));
    assert!(views.iter().any(|v| v["threadId"] == "t3" && v["dismissed"] == true));
    wait_until("the keep set again", || alpha.keeps().len() > before && alpha.keeps().last().is_some_and(|k| k.len() == 2));
    // A thread gone from its target loses its pin; unmuting clears the timed end.
    alpha.remove_thread("t1");
    wait_until("t1's pin dropped", || {
        c.call("hub/view/get", json!({})).unwrap()["views"].as_array().unwrap().iter().all(|v| v["threadId"] != "t1")
    });
    let r = c.call("hub/view/set", json!({ "target": a, "threadId": "t2", "muted": false })).unwrap();
    assert_eq!(r["view"]["muted"], false);
    assert!(r["view"].get("mutedUntil").is_none(), "{r}");
    assert!(c.call("hub/view/set", json!({ "target": "nope", "threadId": "t9", "pinned": true })).is_err());
    // Removing the target removes its entries.
    c.call(yaac_protocol::attach::names::TARGETS_REMOVE, json!({ "id": a })).unwrap();
    wait_until("the target's entries removed", || c.call("hub/view/get", json!({})).unwrap()["views"].as_array().unwrap().is_empty());
}

#[test]
fn an_id_no_index_has_is_looked_up_on_the_targets() {
    let home = Home::new();
    let alpha = FakeDaemon::new("alpha");
    let beta = FakeDaemon::new("beta");
    let mut old = support::fake_daemon::summary("t_0123456789abcdef");
    old.native_id = Some("019a7c2e-0000-7000-8000-00000000beef".into());
    old.title = "An old conversation".into();
    beta.hide(old);
    home.fakes.add(&alpha);
    home.fakes.add(&beta);
    let hub = home.start();
    let c = hub.client();
    let a = c.add_target("alpha").id;
    let b = c.add_target("beta").id;
    c.wait_status(&a, ConnectionStatus::Connected);
    c.wait_status(&b, ConnectionStatus::Connected);
    // Words of a title never cause a lookup.
    let words = c.call("hub/threads/find", json!({ "query": "old conversation" })).unwrap();
    assert_eq!(words["results"].as_array().unwrap().len(), 0);
    assert!(alpha.lookups().is_empty() && beta.lookups().is_empty());
    // An exact-looking id the index does not have: every target is asked.
    let found = c.call("hub/threads/find", json!({ "query": "019a7c2e-0000-7000-8000-00000000beef" })).unwrap();
    assert_eq!(found["results"][0]["thread"]["id"], "t_0123456789abcdef", "{found}");
    assert_eq!(found["results"][0]["target"], json!(b));
    assert_eq!(alpha.lookups(), vec!["019a7c2e-0000-7000-8000-00000000beef".to_owned()]);
    assert_eq!(beta.lookups().len(), 1);
    // The window's own lookup, on one target.
    let direct = c.call("hub/threads/lookup", json!({ "id": "t_0123456789abcdef", "target": b })).unwrap();
    assert_eq!(direct["threads"][0]["target"], json!(b), "{direct}");
    assert_eq!(direct["threads"][0]["thread"]["title"], "An old conversation");
    assert_eq!(alpha.lookups().len(), 1, "only the named target was asked");
    let none = c.call("hub/threads/lookup", json!({ "id": "t_ffffffffffffffff" })).unwrap();
    assert!(none["threads"].as_array().unwrap().is_empty());
}

#[test]
fn the_hub_keeps_a_log_of_connection_steps() {
    let home = Home::new();
    let alpha = FakeDaemon::new("alpha");
    home.fakes.add(&alpha);
    let hub = home.start();
    let c = hub.client();
    let a = c.add_target("alpha").id;
    c.wait_status(&a, ConnectionStatus::Connected);
    let log = c.call("hub/targets/log", json!({ "id": a })).unwrap();
    let steps: Vec<&str> = log["steps"].as_array().unwrap().iter().map(|s| s["step"].as_str().unwrap()).collect();
    for want in ["connecting", "opened", "negotiated", "ready"] {
        assert!(steps.contains(&want), "{want} in {steps:?}");
    }
    assert!(log["steps"][0]["atMs"].as_i64().unwrap() > 0);
    alpha.kill_connections();
    wait_until("a closed step", || {
        let log = c.call("hub/targets/log", json!({ "id": a })).unwrap();
        log["steps"].as_array().unwrap().iter().any(|s| s["step"] == "closed")
    });
    assert!(c.call("hub/targets/log", json!({ "id": "nope" })).is_err());
}

#[test]
fn a_thread_that_needs_the_user_raises_a_desktop_notification_only_with_no_window() {
    let home = Home::new();
    let alpha = FakeDaemon::new("alpha");
    alpha.add_thread("t1");
    alpha.add_thread("t2");
    home.fakes.add(&alpha);
    let recorder = Arc::new(Recorder::default());
    let mut config = home.config();
    config.notifier = Some(recorder.clone());
    let hub = home.start_with(config);
    let a = {
        let c = hub.client();
        let a = c.add_target("alpha").id;
        c.wait_status(&a, ConnectionStatus::Connected);
        // With a window attached: no desktop notification.
        let mut s = support::fake_daemon::summary("t1");
        s.state = ThreadState::NeedsAttention;
        alpha.originate_notification_thread_updated(&s);
        wait_until("t1 cached as needing the user", || {
            home.cache().threads(&a).unwrap().iter().any(|t| t.id.as_str() == "t1" && t.state == ThreadState::NeedsAttention)
        });
        a
    };
    std::thread::sleep(Duration::from_millis(200)); // the window has gone
    assert!(recorder.0.lock().unwrap().is_empty());
    let mut s = support::fake_daemon::summary("t2");
    s.state = ThreadState::NeedsAttention;
    s.title = "Deploy".into();
    alpha.originate_notification_thread_updated(&s);
    wait_until("the notification", || !recorder.0.lock().unwrap().is_empty());
    let (title, body) = recorder.0.lock().unwrap()[0].clone();
    assert!(title.contains("needs you"), "{title}");
    assert!(body.contains("Deploy") && body.contains("alpha"), "{body}");
    // The same state again is not a new notification.
    alpha.originate_notification_thread_updated(&s);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(recorder.0.lock().unwrap().len(), 1);
    let _ = a;
}
