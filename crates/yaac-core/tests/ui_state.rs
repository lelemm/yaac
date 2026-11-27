//! `hub/uiState/get` from the client side: the stored choices arrive once; an older hub
//! without the method gives nothing, silently.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use serde_json::json;
use yaac_core::client::Client;
use yaac_core::link::HubLink;
use yaac_protocol::ClientId;
use yaac_protocol::rpc::{ErrorCode, ErrorObject, Message};

#[derive(Clone)]
struct Hub {
    queue: Rc<RefCell<VecDeque<Message>>>,
    knows_get: bool,
}

impl HubLink for Hub {
    fn send(&mut self, message: Message) {
        let Message::Request { id, method, .. } = message else { return };
        let reply = match method.as_str() {
            "hub/uiState/get" if self.knows_get => Message::Response { id, result: json!({ "values": { "dismissedArtifacts": ["local/t1|u_artifact_md_1"] } }) },
            "hub/uiState/get" => Message::Error { id: Some(id), error: ErrorObject::new(ErrorCode::MethodNotFound, "unknown method") },
            _ => return,
        };
        self.queue.borrow_mut().push_back(reply);
    }

    fn poll(&mut self) -> Option<Message> {
        self.queue.borrow_mut().pop_front()
    }
}

fn client(knows_get: bool) -> Client {
    Client::new(Box::new(Hub { queue: Rc::default(), knows_get }), "0.1.0+1", ClientId::new("phone"))
}

#[test]
fn stored_choices_arrive_once_and_an_older_hub_gives_none_silently() {
    let mut c = client(true);
    c.load_ui_state(&["dismissedArtifacts", "grouping"]);
    assert!(c.take_ui_state().is_none(), "not before the answer");
    while c.pump() {}
    let got = c.take_ui_state().unwrap();
    assert_eq!(got, vec![("dismissedArtifacts".to_owned(), json!(["local/t1|u_artifact_md_1"]))]);
    assert!(c.take_ui_state().is_none(), "taken once");

    let mut old = client(false);
    old.load_ui_state(&["dismissedArtifacts"]);
    while old.pump() {}
    assert_eq!(old.take_ui_state(), Some(vec![]));
    assert!(old.errors.is_empty(), "no error is shown");
}
