//! The hub's voice relay (docs/plan/voice.md), against the Rust fake daemon: `hub/voice/*`
//! becomes `voice/*` on the target's one connection with the stream renumbered, audio is
//! forwarded under the target's credit, and the words reach only the window that started.

mod support;

use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use support::{Client, FakeDaemon, Home, wait_until};
use yaac_protocol::attach::{ConnectionStatus, names};
use yaac_protocol::rpc::{ErrorCode, Message};
use yaac_protocol::{Capability, StreamId, TargetId, methods};

fn voice_daemon(home: &Home, name: &str) -> FakeDaemon {
    let daemon = FakeDaemon::new(name);
    daemon.set_features(vec![Capability::VoiceInput]);
    daemon.add_thread("t1");
    home.fakes.add(&daemon);
    daemon
}

fn start(c: &Client, target: &TargetId) -> (StreamId, Value) {
    let r = c
        .call(names::VOICE_START, json!({ "target": target, "opId": "op-voice-1", "threadId": "t1", "language": "pt-BR" }))
        .expect("voice start");
    (serde_json::from_value(r["stream"].clone()).unwrap(), r)
}

/// Collects every notification a client receives, so several waits can look at them.
struct Seen {
    client: Client,
    all: Arc<Mutex<Vec<Message>>>,
}

impl Seen {
    fn new(client: Client) -> Self {
        Self { client, all: Arc::default() }
    }
    fn find(&self, method: &str) -> Option<Value> {
        self.all.lock().unwrap().extend(self.client.drain());
        self.all.lock().unwrap().iter().find_map(|m| match m {
            Message::Notification { method: m, params } if m == method => Some(params.clone()),
            _ => None,
        })
    }
    fn wait(&self, method: &str) -> Value {
        let mut found = None;
        wait_until(method, || {
            found = self.find(method);
            found.is_some()
        });
        found.unwrap()
    }
}

#[test]
fn dictation_is_relayed_on_the_one_connection_with_credit_and_renumbered_streams() {
    let home = Home::new();
    let daemon = voice_daemon(&home, "box");
    let hub = home.start();
    let c = hub.client();
    let observer = hub.client();
    let target = c.add_target("box").id;
    c.wait_status(&target, ConnectionStatus::Connected);
    let connects = daemon.connects();

    let (stream, result) = start(&c, &target);
    assert_eq!(result["initialCredit"], 8);
    assert_eq!(result["format"], json!({ "format": "pcm16le", "sampleRate": 16000, "channels": 1 }));
    let calls = daemon.voice_calls();
    assert_eq!(calls[0].0, methods::names::VOICE_START);
    assert_eq!(calls[0].1["threadId"], "t1");
    assert_eq!(calls[0].1["opId"], "op-voice-1");
    assert!(calls[0].1.get("target").is_none(), "the envelope's target stays in the hub");
    assert_ne!(stream, StreamId(7001), "the window sees a hub-local stream");

    let seen = Seen::new(c);
    seen.client.link.send_data(stream, b"hello wo".to_vec()).unwrap();
    wait_until("first audio", || daemon.voice_audio(7001) == b"hello wo");
    // More credit from the target reaches the window on its own stream number.
    daemon.originate_notification(methods::names::STREAM_CREDIT, json!({ "stream": 7001, "bytes": 4 }));
    let credit = seen.wait("stream/credit");
    assert_eq!(credit, json!({ "stream": stream, "bytes": 4 }));
    seen.client.link.send_data(stream, b"rld!".to_vec()).unwrap();
    wait_until("all audio", || daemon.voice_audio(7001) == b"hello world!");

    seen.client.call(names::VOICE_STOP, json!({ "target": target, "stream": stream, "bytes": 12 })).unwrap();
    let stop = daemon.voice_calls().pop().unwrap();
    assert_eq!(stop, (methods::names::VOICE_STOP.to_owned(), json!({ "stream": 7001, "bytes": 12 })));
    let words = seen.wait(names::VOICE_TRANSCRIPT);
    assert_eq!(words, json!({ "target": target, "stream": stream, "text": "hello world!", "final": true }));
    let ended = seen.wait(names::VOICE_ENDED);
    assert_eq!(ended, json!({ "target": target, "stream": stream }));
    assert!(
        observer.drain().iter().all(|m| !matches!(m, Message::Notification { method, .. } if method.starts_with("hub/voice/"))),
        "only the starting window hears the words"
    );
    // The stream is gone: a second stop is refused by the hub, not sent.
    let again = seen.client.call(names::VOICE_STOP, json!({ "target": target, "stream": stream, "bytes": 12 })).unwrap_err();
    assert_eq!(again.code, ErrorCode::InvalidParams);
    assert_eq!(daemon.connects(), connects, "nothing new was opened to the target");
    hub.shutdown();
}

#[test]
fn a_target_without_the_feature_offers_no_voice_and_the_envelope_is_refused() {
    let home = Home::new();
    let daemon = FakeDaemon::new("old");
    daemon.add_thread("t1");
    home.fakes.add(&daemon);
    let hub = home.start();
    let c = hub.client();
    let target = c.add_target("old").id;
    c.wait_status(&target, ConnectionStatus::Connected);
    let e = c.call(names::VOICE_START, json!({ "target": target, "opId": "op", "threadId": "t1" })).unwrap_err();
    assert_eq!(e.code, ErrorCode::CapabilityMissing);
    let e = c.routed(&target, methods::names::VOICE_START, json!({ "opId": "op", "threadId": "t1" })).unwrap_err();
    assert!(e.message.contains("hub voice methods"), "{e:?}");
    assert!(daemon.voice_calls().is_empty());
    hub.shutdown();
}

#[test]
fn a_window_that_leaves_cancels_its_dictation() {
    let home = Home::new();
    let daemon = voice_daemon(&home, "box");
    let hub = home.start();
    let c = hub.client();
    let target = c.add_target("box").id;
    c.wait_status(&target, ConnectionStatus::Connected);
    start(&c, &target);
    drop(c);
    wait_until("cancel on the target", || {
        daemon.voice_calls().last().is_some_and(|(m, p)| m == methods::names::VOICE_CANCEL && p["stream"] == 7001)
    });
    hub.shutdown();
}

#[test]
fn overrunning_the_credit_cancels_and_tells_the_window() {
    let home = Home::new();
    let daemon = voice_daemon(&home, "box");
    let hub = home.start();
    let c = hub.client();
    let target = c.add_target("box").id;
    c.wait_status(&target, ConnectionStatus::Connected);
    let (stream, _) = start(&c, &target);
    let seen = Seen::new(c);
    seen.client.link.send_data(stream, vec![0; 9]).unwrap();
    let ended = seen.wait(names::VOICE_ENDED);
    assert_eq!(ended["stream"], json!(stream));
    assert!(ended["error"].is_string());
    wait_until("cancel on the target", || daemon.voice_calls().iter().any(|(m, _)| m == methods::names::VOICE_CANCEL));
    assert!(daemon.voice_audio(7001).is_empty(), "nothing past the credit is forwarded");
    hub.shutdown();
}

#[test]
fn a_dropped_connection_ends_the_dictation_with_an_error() {
    let home = Home::new();
    let daemon = voice_daemon(&home, "box");
    let hub = home.start();
    let c = hub.client();
    let target = c.add_target("box").id;
    c.wait_status(&target, ConnectionStatus::Connected);
    let (stream, _) = start(&c, &target);
    let seen = Seen::new(c);
    daemon.kill_connections();
    let ended = seen.wait(names::VOICE_ENDED);
    assert_eq!(ended["stream"], json!(stream));
    assert!(ended["error"].as_str().unwrap().contains("dropped"));
    hub.shutdown();
}
