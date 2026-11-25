//! A fake daemon and fake windows exercising browser requests over both hub links.

mod support;

use serde_json::{Value, json};
use support::{Client, FakeDaemon, Home, wait_until};
use yaac_protocol::attach::{self, ConnectionStatus};
use yaac_protocol::methods;
use yaac_protocol::rpc::{ErrorCode, ErrorObject, Message, RpcId};

fn list_tabs(daemon: &FakeDaemon) -> i64 {
    daemon
        .originate(
            methods::names::BROWSER_CALL,
            json!({
                "threadId": "thread-with-tabs",
                "action": { "kind": "list" },
            }),
        )
        .expect("send browser/call")
}

fn request(window: &Client) -> (RpcId, Value) {
    let mut found = None;
    wait_until("hub/browser/call", || {
        found = window.drain().into_iter().find_map(|msg| match msg {
            Message::Request { id, method, params } if method == attach::names::BROWSER_CALL => Some((id, params)),
            _ => None,
        });
        found.is_some()
    });
    found.unwrap()
}

fn answer(daemon: &FakeDaemon, id: i64) -> Result<Value, ErrorObject> {
    wait_until("browser answer at the daemon", || daemon.peer_answer(id).is_some());
    daemon.peer_answer(id).unwrap()
}

#[test]
fn a_browser_request_uses_the_first_window_that_accepts() {
    let home = Home::new();
    let daemon = FakeDaemon::new("target");
    home.fakes.add(&daemon);
    let hub = home.start();
    let first = hub.client();
    let target = first.add_target("target").id;
    first.wait_status(&target, ConnectionStatus::Connected);
    let second = hub.client();

    let peer = list_tabs(&daemon);
    let (id, params) = request(&first);
    assert_eq!(
        params,
        json!({
            "target": target,
            "threadId": "thread-with-tabs",
            "action": { "kind": "list" },
        })
    );

    // An answer on the other attach socket cannot complete the first window's request.
    second
        .link
        .send(Message::Response {
            id: id.clone(),
            result: json!({ "kind": "list", "tabs": [] }),
        })
        .unwrap();
    second.call(attach::names::PREVIEW_LIST, json!({})).unwrap();
    assert!(daemon.peer_answer(peer).is_none());
    first
        .link
        .send(Message::Error {
            id: Some(id.clone()),
            error: ErrorObject::new(ErrorCode::MethodNotFound, "browser unavailable here"),
        })
        .unwrap();

    let (next, retry) = request(&second);
    assert_ne!(next, id);
    assert_eq!(retry, params);
    let result = json!({
        "kind": "list",
        "tabs": [{ "tab": "opaque", "url": "http://localhost:5173/", "title": "Preview", "loading": false }],
    });
    second
        .link
        .send(Message::Response {
            id: next,
            result: result.clone(),
        })
        .unwrap();
    assert_eq!(answer(&daemon, peer).unwrap(), result);
    hub.shutdown();
}

#[test]
fn a_browser_request_without_an_attached_window_fails_immediately() {
    let home = Home::new();
    let daemon = FakeDaemon::new("local");
    home.fakes.add(&daemon);
    let mut config = home.config();
    config.seed_local_target = true;
    let hub = home.start_with(config);
    wait_until("fake daemon connection", || daemon.connected());

    // Connected is not yet initialised: a request that arrives before the hub has finished its
    // `initialize` handshake is answered `NotInitialized`. Ask again until the handshake is
    // done (each try is a new request), then the answer must be `NoWindow` at once.
    let deadline = std::time::Instant::now() + support::WAIT;
    let error = loop {
        let peer = list_tabs(&daemon);
        let error = answer(&daemon, peer).unwrap_err();
        if error.code != ErrorCode::NotInitialized || std::time::Instant::now() > deadline {
            break error;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    assert_eq!(error.code, ErrorCode::NoWindow);
    assert_eq!(error.message, "No YAAC window is open; the browser needs one");
    hub.shutdown();
}

#[test]
fn a_browser_request_finishes_when_its_window_detaches() {
    let home = Home::new();
    let daemon = FakeDaemon::new("target");
    home.fakes.add(&daemon);
    let hub = home.start();
    let first = hub.client();
    let target = first.add_target("target").id;
    first.wait_status(&target, ConnectionStatus::Connected);
    let second = hub.client();

    let peer = list_tabs(&daemon);
    request(&first);
    drop(first);
    let error = answer(&daemon, peer).unwrap_err();
    assert_eq!(error.code, ErrorCode::InternalError);
    assert!(error.message.contains("window handling the browser request closed"));
    assert!(second.drain().iter().all(|msg| !matches!(msg, Message::Request { .. })));
    hub.shutdown();
}
