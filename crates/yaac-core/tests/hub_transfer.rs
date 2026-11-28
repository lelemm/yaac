//! Moving the hub to another machine (docs/15-hub-transfer.md), as a client sees it: the
//! candidates, one start per confirmation with an operation id, progress, a refusal said and
//! never re-sent, and `hub/moved` kept for the window to follow.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use yaac_core::client::Client;
use yaac_core::link::HubLink;
use yaac_core::state::HubTransferPhase;
use yaac_protocol::attach::names;
use yaac_protocol::rpc::{ErrorCode, ErrorObject, Message, RpcId};
use yaac_protocol::{ClientId, TargetId};

const BUILD: &str = "0.1.0+1060";

#[derive(Clone, Default)]
struct Hub {
    sent: Arc<Mutex<Vec<(RpcId, String, Value)>>>,
    inbox: Arc<Mutex<VecDeque<Message>>>,
}

impl Hub {
    fn requests(&self, method: &str) -> Vec<(RpcId, Value)> {
        self.sent.lock().unwrap().iter().filter(|(_, m, _)| m == method).map(|(id, _, p)| (id.clone(), p.clone())).collect()
    }
    fn push(&self, m: Message) {
        self.inbox.lock().unwrap().push_back(m);
    }
}

impl HubLink for Hub {
    fn send(&mut self, message: Message) {
        if let Message::Request { id, method, params } = message {
            if method == names::HELLO {
                self.push(Message::Response { id: id.clone(), result: json!({ "build": BUILD, "targets": [], "hubId": "h-pc", "hostName": "PC" }) });
            }
            self.sent.lock().unwrap().push((id, method, params));
        }
    }
    fn poll(&mut self) -> Option<Message> {
        self.inbox.lock().unwrap().pop_front()
    }
}

fn window(hub: &Hub) -> Client {
    let mut c = Client::new(Box::new(hub.clone()), BUILD, ClientId::new("window"));
    c.pump();
    c
}

#[test]
fn candidates_arrive_with_their_reasons() {
    let hub = Hub::default();
    let mut c = window(&hub);
    c.load_transfer_candidates();
    c.pump();
    let (id, _) = hub.requests(names::TRANSFER_CANDIDATES).remove(0);
    hub.push(Message::Response { id, result: json!({ "candidates": [
        { "targetId": "box", "label": "box", "eligible": true },
        { "targetId": "wsl", "label": "Ubuntu", "eligible": false, "reason": "A hub inside WSL would share Windows' Tailscale address" }
    ] }) });
    c.pump();
    let list = c.state.hub.transfer_candidates.clone().unwrap().unwrap();
    assert_eq!(list.len(), 2);
    assert!(list[0].eligible);
    assert!(list[1].reason.as_deref().unwrap().contains("WSL"));
}

#[test]
fn a_transfer_is_sent_once_followed_and_a_refusal_is_never_resent() {
    let hub = Hub::default();
    let mut c = window(&hub);
    let box_ = TargetId::new("box");
    c.start_hub_transfer(&box_);
    c.start_hub_transfer(&box_);
    c.pump();
    let sent = hub.requests(names::TRANSFER_START);
    assert_eq!(sent.len(), 1, "a second confirmation while one runs sends nothing");
    assert_eq!(sent[0].1["targetId"], "box");
    let op = sent[0].1["opId"].clone();
    assert!(op.is_string());
    hub.push(Message::Response { id: sent[0].0.clone(), result: json!({}) });
    hub.push(Message::notification(names::TRANSFER_PROGRESS, json!({ "opId": op, "targetId": "box", "phase": "copying", "done": 10, "total": 40 })));
    c.pump();
    let t = c.state.hub.transfer.clone().unwrap();
    assert_eq!((t.phase, t.done, t.total), (HubTransferPhase::Copying, Some(10), Some(40)));
    hub.push(Message::notification(names::TRANSFER_PROGRESS, json!({ "opId": op, "targetId": "box", "phase": "failed", "message": "Tailscale is not running on box" })));
    c.pump();
    let t = c.state.hub.transfer.clone().unwrap();
    assert_eq!(t.phase, HubTransferPhase::Failed);
    assert_eq!(t.message.as_deref(), Some("Tailscale is not running on box"));
    assert_eq!(hub.requests(names::TRANSFER_START).len(), 1, "nothing is tried again by itself");

    // Refused at start: said, not re-sent; a new confirmation is a new operation.
    c.start_hub_transfer(&box_);
    c.pump();
    let sent = hub.requests(names::TRANSFER_START);
    assert_ne!(sent[0].1["opId"], sent[1].1["opId"]);
    hub.push(Message::Error { id: Some(sent[1].0.clone()), error: ErrorObject::new(ErrorCode::from_wire("transfer_not_eligible"), "box cannot take the hub.") });
    c.pump();
    let t = c.state.hub.transfer.clone().unwrap();
    assert_eq!((t.phase, t.message.as_deref()), (HubTransferPhase::Failed, Some("box cannot take the hub.")));
    assert_eq!(hub.requests(names::TRANSFER_START).len(), 2);
}

#[test]
fn hub_moved_is_kept_for_the_window() {
    let hub = Hub::default();
    let mut c = window(&hub);
    assert!(c.state.hub.moved.is_none());
    hub.push(Message::notification(names::MOVED, json!({ "hubId": "h-pc", "address": "100.64.0.9:47380", "hostName": "box" })));
    c.pump();
    let moved = c.state.hub.moved.clone().unwrap();
    assert_eq!((moved.hub_id.as_str(), moved.address.as_str(), moved.host_name.as_str()), ("h-pc", "100.64.0.9:47380", "box"));
}
