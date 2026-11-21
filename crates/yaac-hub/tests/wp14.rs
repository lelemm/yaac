//! WP14 (D114): the hub marks a thread unseen when its turn ends while no attached window
//! shows it. The mark is a view preference: it survives a restart and reaches every window.

mod support;

use serde_json::json;
use support::{FakeDaemon, Home, wait_until};
use yaac_protocol::ThreadState;
use yaac_protocol::attach::ConnectionStatus;

fn unseen(c: &support::Client, thread: &str) -> bool {
    c.call("hub/view/get", json!({})).unwrap()["views"].as_array().unwrap().iter().any(|v| v["threadId"] == thread && v["unseen"] == true)
}

fn run_turn(alpha: &FakeDaemon, thread: &str) {
    let mut s = support::fake_daemon::summary(thread);
    s.state = ThreadState::Running;
    alpha.originate_notification_thread_updated(&s);
    s.state = ThreadState::Idle;
    alpha.originate_notification_thread_updated(&s);
}

#[test]
fn a_turn_that_ends_unseen_is_marked_only_when_no_window_shows_the_thread() {
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
        a = c.add_target("alpha").id;
        c.wait_status(&a, ConnectionStatus::Connected);
        c.call("hub/view/showing", json!({ "threads": [{ "target": a, "threadId": "t1" }] })).unwrap();
        run_turn(&alpha, "t1");
        run_turn(&alpha, "t2");
        wait_until("t2 marked unseen", || unseen(&c, "t2"));
        assert!(!unseen(&c, "t1"), "the window shows t1");
        // A state that is not the end of a turn marks nothing; a first sighting neither.
        let mut s = support::fake_daemon::summary("t3");
        s.state = ThreadState::Idle;
        alpha.originate_notification_thread_updated(&s);
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(!unseen(&c, "t3"));
        // The window moves to t3: a turn that ends in t1 now is unseen.
        c.call("hub/view/showing", json!({ "threads": [{ "target": a, "threadId": "t3" }] })).unwrap();
        run_turn(&alpha, "t1");
        wait_until("t1 marked unseen", || unseen(&c, "t1"));
        hub.shutdown();
    }
    // It survives a restart; opening clears it (the window's `hub/view/set`).
    let hub = home.start();
    let c = hub.client();
    assert!(unseen(&c, "t1") && unseen(&c, "t2"));
    c.call("hub/view/set", json!({ "target": a, "threadId": "t2", "unseen": false })).unwrap();
    assert!(!unseen(&c, "t2"));
    // A thread gone from its target loses the mark.
    c.wait_status(&a, ConnectionStatus::Connected);
    alpha.remove_thread("t1");
    wait_until("t1's mark dropped", || !unseen(&c, "t1"));
}

#[test]
fn with_no_window_attached_every_finished_turn_is_unseen() {
    let home = Home::new();
    let alpha = FakeDaemon::new("alpha");
    alpha.add_thread("t1");
    home.fakes.add(&alpha);
    let hub = home.start();
    let a = {
        let c = hub.client();
        let a = c.add_target("alpha").id;
        c.wait_status(&a, ConnectionStatus::Connected);
        c.call("hub/view/showing", json!({ "threads": [{ "target": a, "threadId": "t1" }] })).unwrap();
        a
    };
    // The window that showed t1 has gone.
    std::thread::sleep(std::time::Duration::from_millis(200));
    run_turn(&alpha, "t1");
    let c = hub.client();
    wait_until("t1 marked unseen", || unseen(&c, "t1"));
    let _ = a;
}
