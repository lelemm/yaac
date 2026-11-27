//! A phone newer than its PC asks the PC to update (`hub/update/request`,
//! docs/install.md#updating-from-the-phone): offered only for a newer build, sent once per tap,
//! progress followed, "PC is updating…" after the drop, and the matching build after reattach.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use yaac_core::client::Client;
use yaac_core::link::HubLink;
use yaac_core::state::{HelloOutcome, HostUpdatePhase};
use yaac_protocol::ClientId;
use yaac_protocol::attach::names;
use yaac_protocol::rpc::{ErrorCode, ErrorObject, Message, RpcId};

/// A hub of build `hub`: answers hello; everything else is left to the test.
#[derive(Clone)]
struct Hub {
    hub: String,
    sent: Arc<Mutex<Vec<(RpcId, String, Value)>>>,
    inbox: Arc<Mutex<VecDeque<Message>>>,
}

impl Hub {
    fn new(hub: &str) -> Self {
        Self { hub: hub.into(), sent: Arc::default(), inbox: Arc::default() }
    }
    fn requests(&self, method: &str) -> Vec<(RpcId, Value)> {
        self.sent.lock().unwrap().iter().filter(|(_, m, _)| m == method).map(|(id, _, p)| (id.clone(), p.clone())).collect()
    }
    fn push(&self, m: Message) {
        self.inbox.lock().unwrap().push_back(m);
    }
    fn progress(&self, params: Value) {
        self.push(Message::notification(names::UPDATE_PROGRESS, params));
    }
}

impl HubLink for Hub {
    fn send(&mut self, message: Message) {
        if let Message::Request { id, method, params } = message {
            if method == names::HELLO {
                self.inbox.lock().unwrap().push_back(Message::Response { id: id.clone(), result: json!({ "build": self.hub, "targets": [] }) });
            }
            self.sent.lock().unwrap().push((id, method, params));
        }
    }
    fn poll(&mut self) -> Option<Message> {
        self.inbox.lock().unwrap().pop_front()
    }
}

const PHONE: &str = "0.1.0+1060";

fn phone(hub: &Hub) -> Client {
    let mut c = Client::new(Box::new(hub.clone()), PHONE, ClientId::new("phone"));
    c.pump();
    c
}

#[test]
fn only_a_newer_phone_sends_the_request_and_only_once() {
    // The same build, or an older phone: nothing is sent.
    for hub_build in [PHONE, "0.1.0+1070"] {
        let hub = Hub::new(hub_build);
        let mut c = phone(&hub);
        c.request_host_update();
        c.pump();
        assert!(hub.requests(names::UPDATE_REQUEST).is_empty(), "hub {hub_build}");
        assert!(c.state.hub.host_update.is_none());
    }
    let hub = Hub::new("0.1.0+1050");
    let mut c = phone(&hub);
    assert_eq!(c.state.hub.outcome, HelloOutcome::HubOutdated { hub_build: "0.1.0+1050".into() });
    c.request_host_update();
    c.request_host_update();
    c.pump();
    let sent = hub.requests(names::UPDATE_REQUEST);
    assert_eq!(sent.len(), 1, "a second tap while it runs sends nothing");
    assert_eq!(sent[0].1["build"], PHONE);
    assert!(sent[0].1["opId"].is_string());
    assert_eq!(c.state.hub.host_update.as_ref().unwrap().phase, HostUpdatePhase::Requesting);
    // Refused: said, and nothing is sent again by itself.
    hub.push(Message::Error { id: Some(sent[0].0.clone()), error: ErrorObject::new(ErrorCode::from_wire("update_unavailable"), "There is no published release of YAAC 0.1.0+1060.") });
    c.pump();
    assert!(matches!(&c.state.hub.host_update.as_ref().unwrap().phase, HostUpdatePhase::Failed(m) if m.contains("no published release")));
    assert_eq!(hub.requests(names::UPDATE_REQUEST).len(), 1);
    // A new tap is a new request, with a new operation id.
    c.request_host_update();
    c.pump();
    let sent = hub.requests(names::UPDATE_REQUEST);
    assert_eq!(sent.len(), 2);
    assert_ne!(sent[0].1["opId"], sent[1].1["opId"]);
}

#[test]
fn the_phone_follows_the_update_and_lands_on_the_new_build() {
    let hub = Hub::new("0.1.0+1050");
    let mut c = phone(&hub);
    c.request_host_update();
    c.pump();
    let (id, params) = hub.requests(names::UPDATE_REQUEST).remove(0);
    let op = params["opId"].clone();
    hub.push(Message::Response { id, result: json!({ "build": PHONE, "size": 1000 }) });
    hub.progress(json!({ "opId": op, "build": PHONE, "phase": "downloading", "done": 400, "total": 1000 }));
    c.pump();
    assert_eq!(c.state.hub.host_update.as_ref().unwrap().phase, HostUpdatePhase::Downloading { done: 400, total: 1000 });
    hub.progress(json!({ "opId": op, "build": PHONE, "phase": "installing" }));
    c.pump();
    assert_eq!(c.state.hub.host_update.as_ref().unwrap().phase, HostUpdatePhase::Installing);
    // The installer stops the hub: the PC is updating.
    c.connection_lost();
    assert_eq!(c.state.hub.host_update.as_ref().unwrap().phase, HostUpdatePhase::Restarting);
    assert!(c.state.hub.host_update.as_ref().unwrap().phase.active(), "the app attaches again");
    // Back on the phone's build: attached, and the update is done.
    c.rebind(Box::new(Hub::new(PHONE)));
    c.pump();
    assert_eq!(c.state.hub.outcome, HelloOutcome::Attached);
    assert!(c.state.hub.host_update.is_none());

    // Had it come back on the old build, the phone says so (and offers it again).
    let hub = Hub::new("0.1.0+1050");
    let mut c = phone(&hub);
    c.request_host_update();
    c.pump();
    let op = hub.requests(names::UPDATE_REQUEST)[0].1["opId"].clone();
    hub.progress(json!({ "opId": op, "build": PHONE, "phase": "installing" }));
    c.pump();
    c.rebind(Box::new(Hub::new("0.1.0+1050")));
    c.pump();
    assert!(matches!(&c.state.hub.host_update.as_ref().unwrap().phase, HostUpdatePhase::Failed(m) if m.contains("did not go in")));
}

#[test]
fn a_window_on_the_pc_says_what_the_phone_asked_for() {
    let hub = Hub::new("0.1.0+1050");
    let mut w = Client::new(Box::new(hub.clone()), "0.1.0+1050", ClientId::new("window"));
    w.pump();
    assert_eq!(w.state.hub.outcome, HelloOutcome::Attached);
    hub.progress(json!({ "opId": "op-phone-1", "build": PHONE, "phase": "downloading", "done": 1, "total": 9 }));
    hub.progress(json!({ "opId": "op-phone-1", "build": PHONE, "phase": "downloading", "done": 5, "total": 9 }));
    hub.progress(json!({ "opId": "op-phone-1", "build": PHONE, "phase": "installing" }));
    w.pump();
    let notices: Vec<&str> = w.errors.iter().filter(|e| e.key.is_none()).map(|e| e.text.as_str()).collect();
    assert_eq!(notices.len(), 2, "one notice per step, not per chunk: {notices:?}");
    assert!(notices[1].starts_with("Updating YAAC to build 1060, requested from your phone"), "{notices:?}");
    assert!(!w.state.hub.host_update.as_ref().unwrap().requested_here);
}
