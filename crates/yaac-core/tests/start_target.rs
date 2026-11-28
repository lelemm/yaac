//! **Start it** on a stopped WSL distro asks the hub to start it (`hub/targets/start`, D96),
//! not to reconnect: a reconnect never boots a stopped distro, so the button did nothing.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use yaac_core::client::Client;
use yaac_core::link::HubLink;
use yaac_protocol::rpc::{ErrorCode, ErrorObject, Message, RpcId};
use yaac_protocol::{ClientId, TargetId};

#[derive(Clone, Default)]
struct Hub {
    sent: Arc<Mutex<Vec<(RpcId, String, Value)>>>,
    inbox: Arc<Mutex<VecDeque<Message>>>,
}

impl HubLink for Hub {
    fn send(&mut self, message: Message) {
        if let Message::Request { id, method, params } = message {
            self.sent.lock().unwrap().push((id, method, params));
        }
    }
    fn poll(&mut self) -> Option<Message> {
        self.inbox.lock().unwrap().pop_front()
    }
}

#[test]
fn start_it_sends_targets_start_once_and_shows_a_refusal() {
    let hub = Hub::default();
    let mut c = Client::new(Box::new(hub.clone()), "0.1.0+1", ClientId::new("w"));
    c.pump();
    c.start_target(&TargetId::from("wsl-ubuntu"));
    let sent: Vec<_> = hub.sent.lock().unwrap().iter().filter(|(_, m, _)| m.starts_with("hub/targets/")).cloned().collect();
    assert_eq!(sent.len(), 1, "{sent:?}");
    let (id, method, params) = &sent[0];
    assert_eq!(method, "hub/targets/start");
    assert_eq!(params, &json!({ "id": "wsl-ubuntu" }));
    hub.inbox.lock().unwrap().push_back(Message::Error {
        id: Some(id.clone()),
        error: ErrorObject::new(ErrorCode::InternalError, "wsl.exe failed"),
    });
    c.pump();
    assert!(c.errors.iter().any(|e| e.text.contains("did not start: wsl.exe failed")), "{:?}", c.errors.iter().map(|e| &e.text).collect::<Vec<_>>());
    assert_eq!(hub.sent.lock().unwrap().iter().filter(|(_, m, _)| m == "hub/targets/start").count(), 1, "never re-sent");
}
