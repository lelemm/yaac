//! The hub's voice call relay (docs/plan/voice-call.md), against the Rust fake daemon:
//! `hub/voiceCall/*` becomes `voiceCall/*` on the target's one connection, and the answer,
//! state and transcripts reach only the window that started the call.

mod support;

use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use support::{Client, FakeDaemon, Home, wait_until};
use yaac_protocol::attach::{ConnectionStatus, names};
use yaac_protocol::rpc::{ErrorCode, Message};
use yaac_protocol::{Capability, TargetId, methods};

fn call_daemon(home: &Home, name: &str) -> FakeDaemon {
    let daemon = FakeDaemon::new(name);
    daemon.set_features(vec![Capability::VoiceCall]);
    daemon.add_thread("t1");
    home.fakes.add(&daemon);
    daemon
}

fn start(c: &Client, target: &TargetId) -> String {
    let r = c
        .call(names::VOICE_CALL_START, json!({ "target": target, "opId": "op-call-1", "threadId": "t1", "sdpOffer": "v=offer" }))
        .expect("call start");
    r["callId"].as_str().unwrap().to_owned()
}

/// Every notification a client receives, in order.
struct Seen {
    client: Client,
    all: Arc<Mutex<Vec<Message>>>,
}

impl Seen {
    fn new(client: Client) -> Self {
        Self { client, all: Arc::default() }
    }
    fn of(&self, prefix: &str) -> Vec<(String, Value)> {
        self.all.lock().unwrap().extend(self.client.drain());
        self.all
            .lock()
            .unwrap()
            .iter()
            .filter_map(|m| match m {
                Message::Notification { method, params } if method.starts_with(prefix) => Some((method.clone(), params.clone())),
                _ => None,
            })
            .collect()
    }
}

#[test]
fn a_call_is_relayed_on_the_one_connection_to_the_starting_window_only() {
    let home = Home::new();
    let daemon = call_daemon(&home, "box");
    let hub = home.start();
    let c = hub.client();
    let observer = hub.client();
    let target = c.add_target("box").id;
    c.wait_status(&target, ConnectionStatus::Connected);
    let connects = daemon.connects();

    let call = start(&c, &target);
    let sent = daemon.call_requests();
    assert_eq!(sent[0].0, methods::names::VOICE_CALL_START);
    assert_eq!(sent[0].1, json!({ "opId": "op-call-1", "threadId": "t1", "sdpOffer": "v=offer" }), "the envelope's target stays in the hub");

    let seen = Seen::new(c);
    wait_until("answer and active", || seen.of("hub/voiceCall/").len() >= 3);
    daemon.originate_notification(methods::names::VOICE_CALL_TRANSCRIPT, json!({ "callId": call, "role": "user", "text": "hi", "final": true }));
    daemon.originate_notification(methods::names::VOICE_CALL_TRANSCRIPT, json!({ "callId": "call-unknown", "role": "user", "text": "not ours", "final": true }));
    wait_until("transcript", || seen.of("hub/voiceCall/").len() >= 4);
    seen.client.call(names::VOICE_CALL_END, json!({ "target": target, "callId": call })).unwrap();
    wait_until("ended", || seen.of("hub/voiceCall/").len() >= 5);
    assert_eq!(
        seen.of("hub/voiceCall/"),
        vec![
            (names::VOICE_CALL_STATE.to_owned(), json!({ "target": target, "callId": call, "state": "connecting" })),
            (names::VOICE_CALL_ANSWER.to_owned(), json!({ "target": target, "callId": call, "sdpAnswer": "v=answer" })),
            (names::VOICE_CALL_STATE.to_owned(), json!({ "target": target, "callId": call, "state": "active" })),
            (names::VOICE_CALL_TRANSCRIPT.to_owned(), json!({ "target": target, "callId": call, "role": "user", "text": "hi", "final": true })),
            (names::VOICE_CALL_STATE.to_owned(), json!({ "target": target, "callId": call, "state": "ended" })),
        ]
    );
    assert_eq!(daemon.call_requests().last().unwrap(), &(methods::names::VOICE_CALL_END.to_owned(), json!({ "callId": call })));
    assert!(
        observer.drain().iter().all(|m| !matches!(m, Message::Notification { method, .. } if method.starts_with("hub/voiceCall/"))),
        "only the starting window hears the call"
    );
    // The call is over: a second end is answered by the hub and not sent.
    let before = daemon.call_requests().len();
    seen.client.call(names::VOICE_CALL_END, json!({ "target": target, "callId": call })).unwrap();
    assert_eq!(daemon.call_requests().len(), before);
    assert_eq!(daemon.connects(), connects, "nothing new was opened to the target");
    hub.shutdown();
}

#[test]
fn a_target_without_the_feature_refuses_calls_and_the_envelope_is_refused() {
    let home = Home::new();
    let daemon = FakeDaemon::new("old");
    daemon.add_thread("t1");
    home.fakes.add(&daemon);
    let hub = home.start();
    let c = hub.client();
    let target = c.add_target("old").id;
    c.wait_status(&target, ConnectionStatus::Connected);
    let e = c.call(names::VOICE_CALL_START, json!({ "target": target, "opId": "op", "threadId": "t1", "sdpOffer": "v=offer" })).unwrap_err();
    assert_eq!(e.code, ErrorCode::CapabilityMissing);
    let e = c.routed(&target, methods::names::VOICE_CALL_START, json!({ "opId": "op", "threadId": "t1", "sdpOffer": "v=offer" })).unwrap_err();
    assert!(e.message.contains("hub voice call methods"), "{e:?}");
    assert!(daemon.call_requests().is_empty());
    hub.shutdown();
}

#[test]
fn another_window_cannot_end_a_call_and_a_window_that_leaves_hangs_up() {
    let home = Home::new();
    let daemon = call_daemon(&home, "box");
    let hub = home.start();
    let c = hub.client();
    let other = hub.client();
    let target = c.add_target("box").id;
    c.wait_status(&target, ConnectionStatus::Connected);
    let call = start(&c, &target);
    let e = other.call(names::VOICE_CALL_END, json!({ "target": target, "callId": call })).unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidParams);
    assert_eq!(daemon.call_requests().len(), 1);
    drop(c);
    wait_until("end on the target", || {
        daemon.call_requests().last().is_some_and(|(m, p)| m == methods::names::VOICE_CALL_END && p["callId"] == call.as_str())
    });
    hub.shutdown();
}

#[test]
fn a_dropped_target_connection_ends_the_call_for_its_window() {
    let home = Home::new();
    let daemon = call_daemon(&home, "box");
    let hub = home.start();
    let c = hub.client();
    let target = c.add_target("box").id;
    c.wait_status(&target, ConnectionStatus::Connected);
    let call = start(&c, &target);
    let seen = Seen::new(c);
    wait_until("active", || seen.of("hub/voiceCall/").len() >= 3);
    daemon.kill_connections();
    wait_until("ended", || {
        seen.of(names::VOICE_CALL_STATE).iter().any(|(_, p)| p["state"] == "ended" && p["callId"] == call.as_str() && p["error"].is_string())
    });
    hub.shutdown();
}
