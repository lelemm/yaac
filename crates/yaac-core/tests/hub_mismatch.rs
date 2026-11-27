//! After a build mismatch the hub refuses every request with the same reason. Only the
//! hello's answer may say so (as the hello outcome, which each window words for its case);
//! the other refused requests must not repeat it as errors.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use serde_json::json;
use yaac_core::client::Client;
use yaac_core::link::HubLink;
use yaac_core::state::HelloOutcome;
use yaac_protocol::ClientId;
use yaac_protocol::rpc::{ErrorObject, Message};

/// A hub older than this window: it refuses the hello, then everything else.
#[derive(Clone, Default)]
struct OlderHub(Rc<RefCell<VecDeque<Message>>>);

impl HubLink for OlderHub {
    fn send(&mut self, message: Message) {
        if let Message::Request { id, .. } = message {
            let error = ErrorObject::new(yaac_protocol::rpc::ErrorCode::from_wire("hub_outdated"), "the running hub is older than this window; send hub/shutdown");
            self.0.borrow_mut().push_back(Message::Error { id: Some(id), error });
        }
    }

    fn poll(&mut self) -> Option<Message> {
        self.0.borrow_mut().pop_front()
    }
}

#[test]
fn a_refused_hello_is_the_only_place_the_mismatch_is_reported() {
    let hub = OlderHub::default();
    let mut client = Client::new(Box::new(hub), "0.1.0+2", ClientId::new("window"));
    while client.pump() {}
    assert!(matches!(client.state.hub.outcome, HelloOutcome::HubOutdated { .. }));
    // The window keeps asking for things (here: the request feed and the rate limits).
    client.subscribe_request_feed();
    client.notify("hub/notifications/visible", json!([]));
    client.preview_request("hub/preview/list", json!({}));
    client.set_visible_requests(vec![]);
    while client.pump() {}
    assert!(client.errors.is_empty(), "refused requests are not reported again: {:?}", client.errors.iter().map(|e| &e.text).collect::<Vec<_>>());
    assert!(client.take_preview_replies().is_empty(), "nor answered as failures");
}
