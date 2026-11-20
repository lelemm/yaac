//! Negotiation (overlap, no overlap both ways, frozen-core behaviour), the client session
//! driven by a scripted daemon, and the flows.

// Test code: small index casts and by-value helpers keep the tests readable.
#![allow(clippy::cast_possible_truncation, clippy::needless_pass_by_value, clippy::type_complexity)]

use serde_json::{Value, json};
use yaac_protocol::dialect::{self, OlderSide, SUPPORTED, negotiate, testing::range};
use yaac_protocol::flows::{self, FlowStep, OpenThread, OpenThreadFlow, StartThread};
use yaac_protocol::frame::{Frame, FrameDecoder, encode_data_frame, encode_message_frame};
use yaac_protocol::methods::{self, Method};
use yaac_protocol::rpc::{ErrorCode, ErrorObject, Message, RpcId};
use yaac_protocol::session::{ClientSession, SessionError, SessionEvent};
use yaac_protocol::*;

fn answer_json(n: &dialect::Negotiated) -> Value {
    serde_json::to_value(n.answer()).unwrap()
}

#[test]
fn overlap_agrees_on_the_lower_maximum() {
    // The example in docs/09: UI 5–8, daemon 3–6 → 6.
    let daemon = negotiate(range(3, 6), range(5, 8)).unwrap();
    assert_eq!(answer_json(&daemon), json!({ "min": 3, "max": 6, "agreed": 6 }));
    assert!(daemon.local_is_older() && !daemon.peer_is_older());
    let ui = dialect::confirm(range(5, 8), &serde_json::from_value(answer_json(&daemon)).unwrap()).unwrap();
    assert!(ui.peer_is_older(), "the UI adapts to the older daemon");
    // Touching ranges.
    assert!(negotiate(range(1, 3), range(3, 9)).is_ok());
    // Identical.
    assert!(!negotiate(SUPPORTED, SUPPORTED).unwrap().peer_is_older());
}

#[test]
fn no_overlap_in_either_direction_says_which_side_is_too_old() {
    // The daemon is too old for the UI.
    let err = negotiate(range(1, 2), range(5, 8)).unwrap_err();
    assert_eq!(err.older_side(), OlderSide::Local);
    let object = err.to_error_object();
    assert_eq!(object.code, ErrorCode::ProtocolUnsupported);
    assert_eq!(object.data.as_ref().unwrap()["tooOld"], "daemon");
    assert_eq!(object.data.as_ref().unwrap()["daemon"], json!({ "min": 1, "max": 2 }));
    assert_eq!(object.data.as_ref().unwrap()["client"], json!({ "min": 5, "max": 8 }));
    assert_eq!(dialect::describe_unsupported(&object), Some(OlderSide::Remote), "seen from the client");
    // The UI is too old for the daemon.
    let err = negotiate(range(7, 9), range(1, 4)).unwrap_err();
    assert_eq!(err.older_side(), OlderSide::Remote);
    assert_eq!(err.to_error_object().data.unwrap()["tooOld"], "client");
    assert!(err.message().contains("7–9") && err.message().contains("1–4"));
    // Inverted ranges never agree.
    assert!(negotiate(range(3, 1), range(1, 3)).is_err());
    // Not a protocol_unsupported error.
    assert_eq!(dialect::describe_unsupported(&ErrorObject::new(ErrorCode::ThreadInUse, "x")), None);
}

#[test]
fn a_daemon_that_agrees_the_wrong_version_is_refused() {
    let wrong: dialect::AgreedProtocol = serde_json::from_value(json!({ "min": 1, "max": 5, "agreed": 2 })).unwrap();
    assert!(dialect::confirm(range(1, 5), &wrong).is_err());
}

#[test]
fn the_supported_range_selects_the_current_dialect() {
    let n = negotiate(SUPPORTED, SUPPORTED).unwrap();
    let selected = dialect::select(&n).expect("current dialect");
    assert_eq!(selected.schema_hash(), dialect::CURRENT.schema_hash());
    assert_eq!(selected.schema_hash(), codegen::SCHEMA_HASH_V1);
    // A version with no dialect selects nothing.
    let future = negotiate(range(40, 41), range(40, 41)).unwrap();
    assert!(dialect::select(&future).is_none());
}

// ── A scripted daemon ─────────────────────────────────────────────────────────────────────

struct Daemon {
    decoder: FrameDecoder,
}

impl Daemon {
    fn new() -> Self {
        Self { decoder: FrameDecoder::new() }
    }

    /// Everything the session wants to send, as messages (data frames as `(stream, bytes)`).
    fn take(&mut self, session: &mut ClientSession) -> (Vec<Message>, Vec<(StreamId, Vec<u8>)>) {
        let mut messages = Vec::new();
        let mut data = Vec::new();
        while let Some(bytes) = session.poll_transmit() {
            for frame in self.decoder.decode(&bytes).unwrap() {
                match frame {
                    Frame::Message(payload) => messages.push(Message::from_json(&payload).unwrap()),
                    Frame::Data { stream, bytes } => data.push((stream, bytes)),
                }
            }
        }
        (messages, data)
    }
}

fn send(session: &mut ClientSession, message: &Message) -> Vec<SessionEvent> {
    let mut bytes = Vec::new();
    encode_message_frame(&message.to_json(), &mut bytes).unwrap();
    // Deliver in two pieces to exercise the session's incremental decoding.
    let split = bytes.len() / 2;
    let mut events = session.receive(&bytes[..split]).unwrap();
    events.extend(session.receive(&bytes[split..]).unwrap());
    events
}

fn client() -> ClientInfo {
    ClientInfo { name: "yaac".into(), version: "0.1.0".into(), host: Some("laptop".into()), id: Some(ClientId::new("c1")) }
}

fn initialize_result(agreed: Value, hash: &str) -> Value {
    json!({
        "protocol": agreed, "schemaHash": hash,
        "daemon": { "build": "0.1.0+test", "stateFormat": 1 },
        "target": { "os": "linux", "arch": "x86_64", "hostname": "devbox", "user": "me", "home": "/home/me" }
    })
}

fn ready_session() -> (ClientSession, Daemon) {
    let mut session = ClientSession::new(client());
    let mut daemon = Daemon::new();
    let (messages, _) = daemon.take(&mut session);
    let Message::Request { id, method, params } = &messages[0] else { panic!() };
    assert_eq!(method, "initialize", "initialize is the first message");
    let params: InitializeParams = serde_json::from_value(params.clone()).unwrap();
    let n = negotiate(SUPPORTED, params.protocol).unwrap();
    let events = send(
        &mut session,
        &Message::Response { id: id.clone(), result: initialize_result(answer_json(&n), codegen::SCHEMA_HASH_V1) },
    );
    let [SessionEvent::Ready(info)] = events.as_slice() else { panic!("{events:?}") };
    assert!(info.schema_hash_matches && !info.daemon_is_older);
    assert_eq!(info.target.hostname, "devbox");
    (session, daemon)
}

#[test]
fn nothing_but_the_frozen_calls_is_sent_before_the_handshake() {
    let mut session = ClientSession::new(client());
    let params = TurnInterruptParams { thread_id: ThreadId::new("t"), turn_id: TurnId::new("u"), hold: None };
    assert!(matches!(session.request::<methods::TurnInterrupt>(&params), Err(SessionError::NotReady(_))));
    assert!(session.request::<methods::DaemonStatus>(&Empty {}).is_ok(), "daemon/status is always allowed");
    assert!(session.request::<methods::Initialize>(&InitializeParams { protocol: SUPPORTED, client: client() }).is_err());
}

#[test]
fn after_a_failed_negotiation_only_status_and_handover_work() {
    let mut session = ClientSession::with_range(range(7, 9), client());
    let mut daemon = Daemon::new();
    let (messages, _) = daemon.take(&mut session);
    let Message::Request { id, .. } = &messages[0] else { panic!() };
    let error = negotiate(SUPPORTED, range(7, 9)).unwrap_err().to_error_object();
    let events = send(&mut session, &Message::Error { id: Some(id.clone()), error });
    let [SessionEvent::NegotiationFailed { older, error }] = events.as_slice() else { panic!("{events:?}") };
    assert_eq!(*older, Some(OlderSide::Remote), "the daemon is the side that is too old");
    assert_eq!(error.code, ErrorCode::ProtocolUnsupported);
    assert!(!session.is_ready());
    // The frozen calls still work …
    let status = session.request::<methods::DaemonStatus>(&Empty {}).unwrap();
    let handover = session
        .request::<methods::DaemonHandover>(&DaemonHandoverParams { when: HandoverWhen::Idle })
        .unwrap();
    // … and nothing else does.
    assert!(matches!(session.request::<methods::Ping>(&Empty {}), Err(SessionError::NotReady(_))));
    let (messages, _) = daemon.take(&mut session);
    assert_eq!(messages.iter().filter_map(Message::method).collect::<Vec<_>>(), ["daemon/status", "daemon/handover"]);
    // Their answers decode without a dialect.
    let events = send(
        &mut session,
        &Message::Response {
            id: status,
            result: json!({ "build": "0.0.1", "protocol": { "min": 1, "max": 1 }, "stateFormat": 1,
                             "busy": { "runningTurns": 1, "pendingRequests": 0, "attachedClients": 2 } }),
        },
    );
    let [SessionEvent::Response { outcome: Ok(result), method, .. }] = events.as_slice() else { panic!() };
    assert_eq!(*method, "daemon/status");
    let status: DaemonStatusResult = session.decode::<methods::DaemonStatus>(result.clone()).unwrap();
    assert_eq!(status.busy.running_turns, 1);
    let events = send(&mut session, &Message::Response { id: handover, result: json!({ "state": "scheduled" }) });
    let [SessionEvent::Response { outcome: Ok(result), .. }] = events.as_slice() else { panic!() };
    assert_eq!(session.decode::<methods::DaemonHandover>(result.clone()).unwrap().state, HandoverState::Scheduled);
}

#[test]
fn a_daemon_reporting_an_inconsistent_agreement_fails_the_handshake() {
    let mut session = ClientSession::new(client());
    let mut daemon = Daemon::new();
    let (messages, _) = daemon.take(&mut session);
    let Message::Request { id, .. } = &messages[0] else { panic!() };
    let events = send(
        &mut session,
        &Message::Response { id: id.clone(), result: initialize_result(json!({ "min": 1, "max": 9, "agreed": 9 }), "x") },
    );
    assert!(matches!(events.as_slice(), [SessionEvent::NegotiationFailed { .. }]));
}

#[test]
fn a_mismatched_schema_hash_is_reported_not_fatal() {
    let mut session = ClientSession::new(client());
    let mut daemon = Daemon::new();
    let (messages, _) = daemon.take(&mut session);
    let Message::Request { id, .. } = &messages[0] else { panic!() };
    let n = negotiate(SUPPORTED, SUPPORTED).unwrap();
    let events = send(&mut session, &Message::Response { id: id.clone(), result: initialize_result(answer_json(&n), "0000") });
    let [SessionEvent::Ready(info)] = events.as_slice() else { panic!() };
    assert!(!info.schema_hash_matches);
    assert!(session.is_ready());
}

#[test]
fn requests_notifications_peer_requests_and_data_after_the_handshake() {
    let (mut session, mut daemon) = ready_session();

    // A typed request goes out through the dialect, and its typed result comes back.
    let params = TurnStartParams {
        op_id: OpId::new("op-1"),
        thread_id: ThreadId::new("codex:1"),
        input: vec![InputPart::text("hello")],
        model: None,
        effort: None,
        delivery: None,
        mode: None,
    };
    let id = session.request::<methods::TurnStart>(&params).unwrap();
    let (messages, _) = daemon.take(&mut session);
    let [Message::Request { method, params: wire, .. }] = messages.as_slice() else { panic!() };
    assert_eq!(method, methods::TurnStart::NAME);
    assert_eq!(wire["input"][0], json!({ "type": "text", "text": "hello" }));
    let events = send(&mut session, &Message::Response { id, result: json!({ "turnId": "u1" }) });
    let [SessionEvent::Response { outcome: Ok(result), .. }] = events.as_slice() else { panic!() };
    assert_eq!(session.decode::<methods::TurnStart>(result.clone()).unwrap().turn_id, TurnId::new("u1"));

    // Errors pass through with their code.
    let id = session.request::<methods::Ping>(&Empty {}).unwrap();
    let events = send(
        &mut session,
        &Message::Error { id: Some(id), error: ErrorObject::new(ErrorCode::ThreadInUse, "in use on laptop-2") },
    );
    assert!(matches!(&events[..], [SessionEvent::Response { outcome: Err(e), .. }] if e.code == ErrorCode::ThreadInUse));

    // Notifications are decoded by the dialect.
    let events = send(
        &mut session,
        &Message::notification(
            "thread/event",
            json!({ "threadId": "codex:1", "epoch": 1, "seq": 5, "event": { "type": "controlChanged", "control": "mine" } }),
        ),
    );
    assert!(matches!(&events[..], [SessionEvent::Notification(dialect::Inbound::ThreadEvent(e))] if e.seq == Seq(5)));
    let events = send(&mut session, &Message::notification("zz/future", json!({ "a": 1 })));
    assert!(matches!(&events[..], [SessionEvent::Notification(dialect::Inbound::Other { .. })]));

    // An upload: data goes out under credit; `stream/credit` from the daemon releases more.
    session.open_stream(StreamId(3), 10).unwrap();
    session.send_data(StreamId(3), vec![7; 25]).unwrap();
    let (_, data) = daemon.take(&mut session);
    assert_eq!(data.iter().map(|(_, b)| b.len()).sum::<usize>(), 10);
    let _ = send(&mut session, &Message::notification("stream/credit", json!({ "stream": 3, "bytes": 100 })));
    let (_, data) = daemon.take(&mut session);
    assert_eq!(data.iter().map(|(_, b)| b.len()).sum::<usize>(), 15);

    // Inbound data frames are surfaced.
    let mut bytes = Vec::new();
    encode_data_frame(StreamId(8), b"thumb", &mut bytes).unwrap();
    assert!(matches!(&session.receive(&bytes).unwrap()[..], [SessionEvent::Data { stream: StreamId(8), .. }]));

    // A daemon-originated mesh request is surfaced and answered; an unknown one is refused.
    let relay = json!({ "message": { "id": "m1", "from": "a/codex:1", "to": "b/claude:2", "kind": "message",
                                      "text": "done", "sentAt": "2026-11-20T00:00:00Z", "seq": 1 } });
    let events = send(&mut session, &Message::request(RpcId::Number(1), "mesh/relay", relay));
    let [SessionEvent::PeerRequest { id, method, request: dialect::PeerRequest::MeshRelay(params) }] = events.as_slice()
    else {
        panic!("{events:?}")
    };
    assert_eq!(params.message.text, "done");
    session
        .respond(id.clone(), method, Ok(serde_json::to_value(MeshReceiptResult { receipt: Receipt { status: ReceiptStatus::Queued, reason: Some(ReceiptReason::TargetOffline), detail: None, handling: None } }).unwrap()))
        .unwrap();
    let events = send(&mut session, &Message::request(RpcId::Number(2), "zz/whatever", json!({})));
    assert!(events.is_empty());
    let (messages, _) = daemon.take(&mut session);
    assert!(matches!(&messages[0], Message::Response { id: RpcId::Number(1), result } if result["receipt"]["status"] == "queued"));
    assert!(matches!(&messages[1], Message::Error { id: Some(RpcId::Number(2)), error } if error.code == ErrorCode::MethodNotFound));

    // A response to nothing we sent is an error.
    let mut bytes = Vec::new();
    encode_message_frame(&Message::Response { id: RpcId::Number(999), result: json!({}) }.to_json(), &mut bytes).unwrap();
    assert!(matches!(session.receive(&bytes), Err(SessionError::UnknownResponse(_))));
}

#[test]
fn capabilities_are_folded_through_the_dialect_features() {
    let (session, _) = ready_session();
    let mut caps = vec![Capability::Steer, Capability::MeshTools, Capability::Unknown("zz".into())];
    session.fold_capabilities(&mut caps);
    assert_eq!(caps.len(), 3, "the current version withholds nothing");
    let withholding = dialect::FeatureSet::withholding(&[Capability::Steer]);
    withholding.fold(&mut caps);
    assert_eq!(caps, vec![Capability::MeshTools, Capability::Unknown("zz".into())]);
}

#[test]
fn start_thread_flow() {
    let (mut session, mut daemon) = ready_session();
    let command = StartThread {
        op_id: OpId::new("op-start"),
        instance: InstanceId::new("pi_1"),
        cwd: TargetPath::new("/repo"),
        project: None,
        agents: None,
        model: Some(ModelId::new("fake-model")),
        effort: None,
        mode: None,
        input: vec![InputPart::text("go")],
    };
    let id = flows::start_thread(&mut session, &command).unwrap();
    let (messages, _) = daemon.take(&mut session);
    let [Message::Request { method, params, .. }] = messages.as_slice() else { panic!() };
    assert_eq!(method, "thread/start");
    assert_eq!(params["opId"], "op-start");
    assert_eq!(params["model"], "fake-model");
    let events = send(&mut session, &Message::Response { id, result: json!({ "threadId": "fake:1", "turnId": "u1" }) });
    let [SessionEvent::Response { outcome: Ok(wire), .. }] = events.as_slice() else { panic!() };
    let started = flows::finish_start_thread(&session, wire.clone()).unwrap();
    assert_eq!(started.thread_id, ThreadId::new("fake:1"));
}

#[test]
fn open_thread_flow_replays_without_reading() {
    let (mut session, mut daemon) = ready_session();
    let command = OpenThread {
        thread_id: ThreadId::new("fake:1"),
        after: Some(JournalPosition { epoch: Epoch(3), seq: Seq(398) }),
        page_bytes: Some(65_536),
    };
    let mut flow = OpenThreadFlow::start(&mut session, command).unwrap();
    let (messages, _) = daemon.take(&mut session);
    let [Message::Request { method, params, .. }] = messages.as_slice() else { panic!() };
    assert_eq!(method, "thread/subscribe");
    assert_eq!(params, &json!({ "threadId": "fake:1", "epoch": 3, "afterSeq": 398 }));
    let reply = json!({ "mode": "replay", "epoch": 3, "fromSeq": 399, "headSeq": 431, "control": "free" });
    let step = flow.on_response(&mut session, Ok(reply)).unwrap();
    let FlowStep::Done(opened) = step else { panic!() };
    assert!(opened.snapshot.is_none());
    assert!(daemon.take(&mut session).0.is_empty(), "nothing read after a replay");
    assert!(flow.on_response(&mut session, Ok(json!({}))).is_err(), "finished");
}

#[test]
fn open_thread_flow_reads_a_snapshot_after_a_reset() {
    let (mut session, mut daemon) = ready_session();
    let command = OpenThread { thread_id: ThreadId::new("fake:1"), after: None, page_bytes: Some(65_536) };
    let mut flow = OpenThreadFlow::start(&mut session, command.clone()).unwrap();
    let _ = daemon.take(&mut session);
    let reset = json!({ "mode": "reset", "epoch": 4, "headSeq": 12, "control": "otherClient", "controlHolder": "laptop-2" });
    let FlowStep::Waiting(read_id) = flow.on_response(&mut session, Ok(reset)).unwrap() else { panic!() };
    assert_eq!(flow.waiting(), Some(&read_id));
    let (messages, _) = daemon.take(&mut session);
    let [Message::Request { method, params, .. }] = messages.as_slice() else { panic!() };
    assert_eq!(method, "thread/read");
    assert_eq!(params, &json!({ "threadId": "fake:1", "maxBytes": 65536 }));
    let page = json!({
        "items": [{ "id": "i1", "turnId": "u1", "status": "completed", "kind": "agentMessage", "text": "hi" },
                  { "id": "i0", "turnId": "u1", "status": "completed", "kind": "zzFuture", "x": 1 }],
        "position": { "epoch": 4, "seq": 12 }, "cursor": "c2", "uploads": []
    });
    let FlowStep::Done(opened) = flow.on_response(&mut session, Ok(page)).unwrap() else { panic!() };
    let snapshot = opened.snapshot.unwrap();
    assert_eq!(snapshot.items.len(), 2);
    assert_eq!(snapshot.position, JournalPosition { epoch: Epoch(4), seq: Seq(12) });
    assert_eq!(opened.subscribed.control, Control::OtherClient);
    // Older pages stay bound to the snapshot by the cursor.
    let _ = flows::read_older_page(&mut session, &command, snapshot.cursor.as_ref().unwrap()).unwrap();
    let (messages, _) = daemon.take(&mut session);
    let [Message::Request { params, .. }] = messages.as_slice() else { panic!() };
    assert_eq!(params["cursor"], "c2");
    // An error from the daemon ends the flow with that error.
    let mut flow = OpenThreadFlow::start(&mut session, command).unwrap();
    let err = flow.on_response(&mut session, Err(ErrorObject::new(ErrorCode::ThreadNotFound, "gone"))).unwrap_err();
    assert!(matches!(err, flows::FlowError::Remote(e) if e.code == ErrorCode::ThreadNotFound));
}
