//! The hub's part of the discovery, dismiss, and diagnostics follow-ups: `dismissed` and the
//! discovery caps in the cache, an id lookup that misses the index routed to the targets
//! (`hub/threads/lookup`, and `hub/threads/find` falling through to it), the connection-step
//! log, and desktop notifications for requests outside the focused window.

mod support;

use std::sync::{Arc, Mutex};

use serde_json::json;
use support::{FakeDaemon, Home, wait_until};
use yaac_hub::notify::Notifier;
use yaac_protocol::attach::ConnectionStatus;
use yaac_protocol::{DiscoveryCaps, ThreadState};

#[derive(Default)]
struct Recorder(Mutex<Vec<(yaac_hub::notify::Notification, yaac_hub::notify::Activate, yaac_hub::notify::Updates)>>, std::sync::atomic::AtomicUsize);

impl Notifier for Recorder {
    fn launch_window(&self) { self.1.fetch_add(1, std::sync::atomic::Ordering::SeqCst); }
    fn show(&self, notice: yaac_hub::notify::Notification, activate: yaac_hub::notify::Activate, updates: yaac_hub::notify::Updates) {
        self.0.lock().unwrap().push((notice, activate, updates));
    }
}

impl Recorder {
    fn current(&self, title: &str) -> yaac_hub::notify::Notification {
        let mut result = None;
        wait_until("expected reminder contents", || {
            let mut posted = self.0.lock().unwrap();
            let Some((notice, _, updates)) = posted.last_mut() else { return false };
            while let Ok(next) = updates.try_recv() { *notice = next; }
            if notice.title == title { result = Some(notice.clone()); }
            result.is_some()
        });
        result.unwrap()
    }

    fn hit(&self, revision: u64, action: &str) {
        (self.0.lock().unwrap().last().unwrap().1)(revision, action);
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
fn a_pending_request_on_a_closed_thread_notifies_even_with_an_attached_window() {
    let home = Home::new();
    let alpha = FakeDaemon::new("alpha");
    alpha.add_thread("t1");
    home.fakes.add(&alpha);
    let recorder = Arc::new(Recorder::default());
    let mut config = home.config();
    config.notifier = Some(recorder.clone());
    let hub = home.start_with(config);
    let c = hub.client();
    let target = c.add_target("alpha").id;
    c.wait_status(&target, ConnectionStatus::Connected);
    for req in ["first", "second"] {
        let item = serde_json::from_value(json!({"kind":"request","id":req,"turnId":"turn","status":"completed",
            "requestId":req,"state":"pending","requestKind":"question","questions":[]})).unwrap();
        alpha.emit("t1", yaac_protocol::ThreadEvent::ItemStarted(yaac_protocol::ItemEvent { item }));
    }
    let mut summary = support::fake_daemon::summary("t1");
    summary.state = ThreadState::NeedsAttention;
    alpha.originate_notification_thread_updated(&summary);
    let summary = recorder.current("2 requests need you");
    drop(c);
    recorder.hit(summary.revision, "default");
    wait_until("Open starts a window", || recorder.1.load(std::sync::atomic::Ordering::SeqCst) > 0);
    let single = recorder.current("YAAC needs you");
    recorder.hit(single.revision, "default");
    let next = hub.client();
    next.call("hub/notifications/visible", json!([])).unwrap();
    let mut opened = Vec::new();
    wait_until("new window receives both queued destinations", || {
        for m in next.drain() {
            if let yaac_protocol::rpc::Message::Notification { method, params } = m {
                if method == "hub/notifications/open" {
                    assert_eq!(params["itemId"], params["location"]["requestId"]);
                    opened.push(params["itemId"].as_str().unwrap().to_owned());
                }
            }
        }
        opened.len() == 2
    });
    assert_eq!(opened, ["first", "second"]);
    assert_eq!(recorder.1.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[test]
fn toast_buttons_use_the_original_thread_request_and_offered_decision_once() {
    use yaac_protocol::{ItemEvent, ThreadEvent, RequestResponse};
    let home = Home::new();
    let alpha = FakeDaemon::new("alpha");
    alpha.add_thread("worker");
    home.fakes.add(&alpha);
    let desktop = Arc::new(Recorder::default());
    let mut config = home.config();
    config.notifier = Some(desktop.clone());
    let hub = home.start_with(config);
    let c = hub.client();
    let target = c.add_target("alpha").id;
    c.wait_status(&target, ConnectionStatus::Connected);
    let prefs = json!({"enabled":true,"sound":false,"soundChoice":"default","includeQuestions":true});
    let result = c.call("hub/settings/update", json!({"notifications":prefs})).unwrap();
    assert_eq!(result["settings"]["notifications"], prefs);
    let saved = yaac_hub::settings::Settings::load(&home.paths().settings_file()).unwrap().unwrap();
    assert_eq!(serde_json::to_value(saved.hub.notifications).unwrap(), prefs);
    c.call("hub/notifications/visible", json!([{"target": target,"threadId":"worker","requestId":"visible-copy"}])).unwrap();
    for request in ["visible-copy", "open-this", "outside-viewport"] {
        let item = serde_json::from_value(json!({"kind":"request","id":request,"turnId":"turn","status":"completed",
            "requestId":request,"state":"pending","requestKind":"approval","approvalKind":"fileChange","title":request,
            "decisions":[{"kind":"allow","id":"provider-permit","label":"Allow once"},{"kind":"deny","id":"provider-refuse","label":"Deny"}]})).unwrap();
        alpha.emit("worker", ThreadEvent::ItemStarted(ItemEvent { item }));
    }
    let mut summary = support::fake_daemon::summary("worker");
    summary.state = ThreadState::NeedsAttention;
    alpha.originate_notification_thread_updated(&summary);
    let summary = desktop.current("2 requests need you");
    assert!(!summary.decisions);
    assert_eq!(summary.settings, saved.hub.notifications);
    assert!(!summary.play_sound);
    desktop.hit(summary.revision, "deny");
    desktop.hit(summary.revision, "default");
    let single = desktop.current("YAAC needs you");
    assert!(single.body.contains("outside-viewport"));
    assert!(single.decisions);
    desktop.hit(summary.revision, "deny");
    desktop.hit(single.revision, "allow");
    desktop.hit(single.revision, "allow");
    wait_until("native Open routes to the exact request item", || c.drain().iter().any(|m|
        matches!(m, yaac_protocol::rpc::Message::Notification {method,params}
        if method == "hub/notifications/open" && params["location"]["threadId"] == "worker" && params["itemId"] == "open-this")));
    wait_until("native Allow reaches the daemon", || !alpha.request_answers().is_empty());
    let answers = alpha.request_answers();
    assert_eq!(answers.len(),1);
    assert_eq!(answers[0].thread_id.as_str(), "worker");
    assert_eq!(answers[0].request_id.as_str(), "outside-viewport");
    assert!(matches!(&answers[0].response, RequestResponse::Approval(a) if a.decision == "provider-permit"));
    assert!(!answers[0].op_id.as_str().is_empty());
}

/// One native handle covers independent requests, even when targets reuse request ids.
#[test]
fn one_summary_walks_workers_oldest_first_and_restores_the_last_approvals_buttons() {
    use yaac_protocol::{ItemEvent, ThreadEvent};
    let home = Home::new();
    let a = FakeDaemon::new("alpha");
    let b = FakeDaemon::new("beta");
    a.add_thread("same");
    a.add_thread("other");
    b.add_thread("same");
    home.fakes.add(&a);
    home.fakes.add(&b);
    let desktop = Arc::new(Recorder::default());
    let mut cfg = home.config();
    cfg.notifier = Some(desktop.clone());
    let hub = home.start_with(cfg);
    let c = hub.client();
    let alpha = c.add_target("alpha").id;
    let beta = c.add_target("beta").id;
    c.wait_status(&alpha, ConnectionStatus::Connected);
    c.wait_status(&beta, ConnectionStatus::Connected);
    c.call("hub/notifications/visible",json!([])).unwrap();
    c.subscribe(&alpha,"same");
    c.subscribe(&alpha,"other");
    c.subscribe(&beta,"same");

    let members = [(&a,&alpha,"same","one"),(&a,&alpha,"same","two"),(&a,&alpha,"other","one"),(&b,&beta,"same","one")];
    let raise = |daemon:&FakeDaemon, thread:&str, req:&str| {
        let item = serde_json::from_value(json!({"kind":"request","id":req,"turnId":"turn","status":"completed",
            "requestId":req,"state":"pending","requestKind":"approval","approvalKind":"fileChange","title":req,
            "decisions":[{"kind":"allow","id":"yes","label":"Allow once"},{"kind":"deny","id":"no","label":"Deny"}]})).unwrap();
        daemon.emit(thread,ThreadEvent::ItemStarted(ItemEvent{item}));
    };
    let resolve = |daemon:&FakeDaemon, thread:&str, req:&str| {
        daemon.emit(thread,ThreadEvent::RequestChanged(serde_json::from_value(json!({
            "itemId":req,"requestId":req,"state":"resolved"})).unwrap()));
    };
    let mut revisions = Vec::new();
    for (i,(daemon,_,thread,req)) in members.iter().enumerate() {
        raise(daemon, thread, req);
        let title = if i == 0 { "YAAC needs you".into() } else { format!("{} requests need you", i+1) };
        let n = desktop.current(&title);
        assert_eq!(n.decisions, i == 0);
        revisions.push(n.revision);
    }
    assert_eq!(desktop.0.lock().unwrap().len(), 1);
    resolve(&a,"same","two");
    let mut n = desktop.current("3 requests need you");
    assert!(!n.play_sound);
    desktop.hit(revisions[0], "allow");
    for (target, thread, count) in [(&alpha,"same",2),(&alpha,"other",1)] {
        desktop.hit(n.revision,"deny");
        desktop.hit(n.revision,"default");
        wait_until("Open reaches the oldest unseen survivor",|| c.drain().iter().any(|m| {
            let yaac_protocol::rpc::Message::Notification{method,params}=m else{return false};
            if method!="hub/notifications/open" {return false;}
            let loc=&params["location"];
            assert_eq!(loc["target"],target.as_str());
            assert_eq!(loc["threadId"],thread);
            assert_eq!(loc["requestId"],"one");
            assert_eq!(params["itemId"],"one");
            true
        }));
        let title=if count==1 {"YAAC needs you".into()} else {format!("{count} requests need you")};
        n=desktop.current(&title);
        assert!(!n.play_sound);
    }
    assert!(n.decisions);
    desktop.hit(revisions[0],"deny");
    desktop.hit(n.revision,"deny");
    desktop.hit(n.revision,"allow");
    wait_until("only the last approval is answered", || b.request_answers().len()==1);
    assert_eq!(b.request_answers()[0].thread_id.as_str(),"same");
    assert_eq!(b.request_answers()[0].request_id.as_str(),"one");
    assert!(a.request_answers().is_empty());
    wait_until("zero unseen requests withdraws the reminder",||
        matches!(desktop.0.lock().unwrap()[0].2.try_recv(),Err(tokio::sync::mpsc::error::TryRecvError::Disconnected)));
    assert_eq!(desktop.0.lock().unwrap().len(),1);
}
