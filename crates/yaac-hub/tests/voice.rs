//! The hub's voice relay (docs/plan/voice.md), against the Rust fake daemon: the hub chooses
//! the transcriber (P392), `hub/voice/*` becomes `voice/*` on that target's one connection with
//! the stream renumbered, audio is forwarded under the target's credit, and the words reach
//! only the window that started.

mod support;

use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use support::{Client, FakeDaemon, Home, wait_until};
use yaac_protocol::attach::{ConnectionStatus, names};
use yaac_protocol::rpc::{ErrorCode, Message};
use yaac_protocol::{Capability, StreamId, TargetId, methods};

/// A descriptor whose instance transcribes (`ready`) or says why not.
fn provider(instance: &str, name: &str, ready: bool) -> Value {
    let transcription = if ready {
        json!({ "format": { "format": "pcm16le", "sampleRate": 16000, "channels": 1 } })
    } else {
        json!({ "unavailable": format!("{name} needs a sign-in for dictation.") })
    };
    let capabilities = if ready { json!(["threadList", "transcription"]) } else { json!(["threadList"]) };
    json!({ "instance": instance, "kind": "fake", "displayName": name, "backend": "sdk", "store": { "key": null },
            "status": "ready", "capabilities": capabilities, "revision": 1, "transcription": transcription })
}

fn daemon_with(home: &Home, name: &str, providers: Vec<Value>) -> FakeDaemon {
    let daemon = FakeDaemon::new(name);
    daemon.set_features(vec![Capability::Transcription]);
    daemon.set_providers(providers);
    daemon.add_thread("t1");
    home.fakes.add(&daemon);
    daemon
}

fn voice_daemon(home: &Home, name: &str) -> FakeDaemon {
    daemon_with(home, name, vec![provider("pi_one", "Alpha", true)])
}

/// `hub/voice/start` once the hub knows the target's instances. A refusal for want of a
/// transcriber sends nothing to any target, so asking again is not a retry of anything.
fn start_with(c: &Client, params: &Value) -> Result<Value, yaac_protocol::rpc::ErrorObject> {
    let mut last = None;
    wait_until("a transcriber", || match c.call(names::VOICE_START, params.clone()) {
        Err(e) if e.code == ErrorCode::CapabilityMissing && e.message.contains("no machine") => false,
        Err(e) if e.code == ErrorCode::CapabilityMissing && e.message.contains("connected machines") => false,
        other => {
            last = Some(other);
            true
        }
    });
    last.unwrap()
}

fn start(c: &Client, target: &TargetId) -> (StreamId, Value) {
    let r = start_with(c, &json!({ "target": target, "instance": "pi_one", "opId": "op-voice-1", "threadId": "t1", "language": "pt-BR" }))
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
    assert_eq!(result["target"], json!(target));
    assert_eq!(result["instance"], "pi_one");
    assert_eq!(result["label"], "Alpha on box");
    assert_eq!(result["initialCredit"], 8);
    assert_eq!(result["format"], json!({ "format": "pcm16le", "sampleRate": 16000, "channels": 1 }));
    let calls = daemon.voice_calls();
    assert_eq!(calls[0].0, methods::names::VOICE_START);
    assert_eq!(calls[0].1["threadId"], "t1");
    assert_eq!(calls[0].1["instance"], "pi_one");
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
    // Its instance says it transcribes, but the connection lacks the feature: folded away.
    daemon.set_providers(vec![provider("pi_one", "Alpha", true)]);
    let e = c.call(names::VOICE_START, json!({ "target": target, "opId": "op", "threadId": "t1" })).unwrap_err();
    assert_eq!(e.code, ErrorCode::CapabilityMissing);
    let e = c.routed(&target, methods::names::VOICE_START, json!({ "opId": "op", "instance": "pi_one" })).unwrap_err();
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

#[test]
fn the_composers_own_target_is_preferred() {
    let home = Home::new();
    let near = daemon_with(&home, "near", vec![provider("pi_b", "Beta", true), provider("pi_a", "Alpha", true)]);
    let far = daemon_with(&home, "far", vec![provider("pi_c", "Gamma", true)]);
    let hub = home.start();
    let c = hub.client();
    let a = c.add_target("near").id;
    let b = c.add_target("far").id;
    c.wait_status(&a, ConnectionStatus::Connected);
    c.wait_status(&b, ConnectionStatus::Connected);
    // The thread's own instance first, though another sorts before it.
    let r = start_with(&c, &json!({ "target": a, "instance": "pi_b", "threadId": "t1", "opId": "op-1" })).expect("start");
    assert_eq!((r["target"].clone(), r["instance"].clone()), (json!(a), json!("pi_b")));
    assert_eq!(near.voice_calls()[0].1["threadId"], "t1");
    assert!(far.voice_calls().is_empty());
    hub.shutdown();
}

#[test]
fn another_connected_target_transcribes_when_the_own_one_cannot() {
    let home = Home::new();
    // The composer's target has an instance with a voice surface that is not ready.
    let own = daemon_with(&home, "own", vec![provider("pi_codex", "Codex", false)]);
    let other = daemon_with(&home, "other", vec![provider("pi_claude", "Claude", true)]);
    let hub = home.start();
    let c = hub.client();
    let a = c.add_target("own").id;
    let b = c.add_target("other").id;
    c.wait_status(&a, ConnectionStatus::Connected);
    c.wait_status(&b, ConnectionStatus::Connected);
    let connects = (own.connects(), other.connects());
    let mut r = None;
    wait_until("the other target's transcriber", || {
        r = c.call(names::VOICE_START, json!({ "target": a, "instance": "pi_codex", "threadId": "t1", "opId": "op-x" })).ok();
        r.is_some()
    });
    let r = r.unwrap();
    assert_eq!(r["target"], json!(b));
    assert_eq!(r["label"], "Claude on other");
    let (method, params) = other.voice_calls().remove(0);
    assert_eq!(method, methods::names::VOICE_START);
    assert_eq!(params["instance"], "pi_claude");
    assert!(params.get("threadId").is_none(), "another target does not know the thread");
    assert!(own.voice_calls().is_empty());
    // Audio and stop go to the transcriber's target, over its existing connection.
    let stream: StreamId = serde_json::from_value(r["stream"].clone()).unwrap();
    c.link.send_data(stream, b"hi".to_vec()).unwrap();
    wait_until("audio on the other target", || other.voice_audio(7001) == b"hi");
    c.call(names::VOICE_STOP, json!({ "target": b, "stream": stream, "bytes": 2 })).unwrap();
    assert_eq!((own.connects(), other.connects()), connects, "nothing new was opened");
    hub.shutdown();
}

#[test]
fn no_transcriber_anywhere_is_refused_with_the_reason() {
    let home = Home::new();
    let daemon = daemon_with(&home, "box", vec![provider("pi_one", "Claude", false)]);
    let hub = home.start();
    let c = hub.client();
    let target = c.add_target("box").id;
    c.wait_status(&target, ConnectionStatus::Connected);
    let mut why = String::new();
    wait_until("the reason", || {
        let e = c.call(names::VOICE_START, json!({ "target": target, "opId": "op" })).unwrap_err();
        assert_eq!(e.code, ErrorCode::CapabilityMissing);
        why = e.message;
        why.contains("sign-in")
    });
    assert_eq!(why, "No transcriber available. Claude on box: Claude needs a sign-in for dictation.");
    assert!(daemon.voice_calls().is_empty());
    hub.shutdown();
}
