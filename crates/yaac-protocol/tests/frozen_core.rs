//! The frozen core, pinned by a byte-level fixture (docs/09-compatibility.md#the-frozen-core):
//! frames, `initialize`, a failed negotiation, and the two calls permitted after it. Every
//! build, forever, must reproduce `protocol/schema/frozen-core-v1.txt` exactly — the Rust side
//! here, the `yaacd` side against the same file.
//!
//! The bootstrap line that precedes these frames belongs to the transport and is pinned there.
//!
//! The fixture is written once with `YAAC_WRITE_FROZEN_CORE=1 cargo test -p yaac-protocol
//! --test frozen_core` when it does not exist, and never rewritten.

// Test code: small index casts and by-value helpers keep the tests readable.
#![allow(clippy::cast_possible_truncation, clippy::needless_pass_by_value, clippy::type_complexity)]

use serde_json::json;
use yaac_protocol::dialect::{SUPPORTED, negotiate, testing::range};
use yaac_protocol::frame::{FrameDecoder, encode_message_frame};
use yaac_protocol::methods;
use yaac_protocol::rpc::{Message, RpcId};
use yaac_protocol::session::{ClientSession, SessionEvent};
use yaac_protocol::*;

const FIXTURE: &str = "protocol/schema/frozen-core-v1.txt";

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn frame(message: &Message) -> Vec<u8> {
    let mut out = Vec::new();
    encode_message_frame(&message.to_json(), &mut out).unwrap();
    out
}

/// The exchange, as `(direction, note, frame bytes)`; `>` is host → daemon.
fn exchange() -> Vec<(char, &'static str, Vec<u8>)> {
    let client = ClientInfo {
        name: "yaac".into(),
        version: "9.0.0".into(),
        host: Some("laptop".into()),
        id: Some(ClientId::new("c_fixture")),
    };
    // A client from the far future (protocol 7–9) meets this daemon.
    let mut session = ClientSession::with_range(range(7, 9), client);
    let mut out = Vec::new();
    let initialize = session.poll_transmit().unwrap();
    out.push(('>', "initialize from a client that speaks 7-9", initialize));

    let error = negotiate(SUPPORTED, range(7, 9)).unwrap_err().to_error_object();
    let reply = frame(&Message::Error { id: Some(RpcId::Number(1)), error });
    let events = session.receive(&reply).unwrap();
    assert!(matches!(events.as_slice(), [SessionEvent::NegotiationFailed { .. }]));
    out.push(('<', "protocol_unsupported: the daemon is too old", reply));

    session.request::<methods::DaemonStatus>(&Empty {}).unwrap();
    out.push(('>', "daemon/status after the failed negotiation", session.poll_transmit().unwrap()));
    let status = DaemonStatusResult {
        build: "0.1.0+fixture".into(),
        protocol: SUPPORTED,
        state_format: 1,
        busy: DaemonBusy { running_turns: 1, pending_requests: 0, attached_clients: 1 },
    };
    out.push(('<', "daemon/status result", frame(&Message::Response { id: RpcId::Number(2), result: serde_json::to_value(status).unwrap() })));

    for (id, when, state, note) in [
        (3, HandoverWhen::Idle, HandoverState::Scheduled, "daemon/handover idle: scheduled"),
        (4, HandoverWhen::Idle, HandoverState::Scheduled, "daemon/handover idle again: idempotent"),
        (5, HandoverWhen::Now, HandoverState::Exiting, "daemon/handover now: exiting; the daemon then closes the stream"),
    ] {
        session.request::<methods::DaemonHandover>(&DaemonHandoverParams { when }).unwrap();
        out.push(('>', note, session.poll_transmit().unwrap()));
        let result = serde_json::to_value(DaemonHandoverResult { state }).unwrap();
        out.push(('<', note, frame(&Message::Response { id: RpcId::Number(id), result })));
    }
    out
}

fn render(exchange: &[(char, &str, Vec<u8>)]) -> String {
    let mut text = String::from(
        "# YAAC frozen core, protocol v1. Never edit; every build must reproduce these bytes.\n# One frame per line: direction (> host to daemon, < daemon to host), hex bytes. '#' lines are notes.\n",
    );
    for (dir, note, bytes) in exchange {
        text.push_str(&format!("# {note}\n{dir} {}\n", hex(bytes)));
    }
    text
}

#[test]
fn the_frozen_core_is_reproduced_byte_for_byte() {
    let path = codegen::repo_root().join(FIXTURE);
    let produced = render(&exchange());
    let committed = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(_) if std::env::var("YAAC_WRITE_FROZEN_CORE").as_deref() == Ok("1") => {
            std::fs::write(&path, &produced).unwrap();
            produced.clone()
        }
        Err(e) => panic!("missing {FIXTURE}: {e}"),
    };
    assert_eq!(produced.replace('\r', ""), committed.replace('\r', ""), "the frozen core changed: {FIXTURE}");
}

#[test]
fn the_fixture_frames_decode_as_the_messages_they_claim_to_be() {
    let text = std::fs::read_to_string(codegen::repo_root().join(FIXTURE)).unwrap();
    let mut decoder = FrameDecoder::new();
    let mut methods_seen = Vec::new();
    for line in text.lines().filter(|l| !l.starts_with('#')) {
        let (dir, hex) = line.split_once(' ').unwrap();
        let bytes: Vec<u8> = (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap()).collect();
        let frames = decoder.decode(&bytes).unwrap();
        assert_eq!(frames.len(), 1);
        let yaac_protocol::frame::Frame::Message(payload) = &frames[0] else { panic!() };
        let message = Message::from_json(payload).unwrap();
        if dir == ">" {
            methods_seen.push(message.method().unwrap().to_owned());
        }
    }
    assert_eq!(methods_seen, ["initialize", "daemon/status", "daemon/handover", "daemon/handover", "daemon/handover"]);
    // The first request's JSON, readable, for anyone implementing it by hand.
    let first = text.lines().nth(3).unwrap();
    assert!(first.starts_with("> "));
    let _ = json!({});
}
