//! The wire shapes the docs show, decoded and re-encoded exactly; and explicit forward
//! compatibility checks for unknown kinds, values, capabilities, and fields.

// Test code: small index casts and by-value helpers keep the tests readable.
#![allow(clippy::cast_possible_truncation, clippy::needless_pass_by_value, clippy::type_complexity)]

use serde_json::{Value, json};
use yaac_protocol::methods::{Method, REQUESTS};
use yaac_protocol::*;

fn roundtrip<T: serde::Serialize + serde::de::DeserializeOwned>(wire: Value) -> (T, Value) {
    let typed: T = serde_json::from_value(wire.clone()).unwrap_or_else(|e| panic!("{e}\n{wire:#}"));
    let back = serde_json::to_value(&typed).unwrap();
    (typed, back)
}

fn exact<T: serde::Serialize + serde::de::DeserializeOwned>(wire: Value) -> T {
    let (typed, back) = roundtrip::<T>(wire.clone());
    assert_eq!(back, wire);
    typed
}

#[test]
fn input_parts_from_the_docs() {
    let parts: Vec<InputPart> = exact(json!([
        { "type": "text", "text": "Why does this test fail?" },
        { "type": "image", "uploadId": "u_01J" },
        { "type": "file", "uploadId": "u_01K" }
    ]));
    assert_eq!(parts[0], InputPart::text("Why does this test fail?"));
    assert!(matches!(&parts[1], InputPart::Image(ImagePart { upload_id }) if upload_id.as_str() == "u_01J"));
}

#[test]
fn question_request_item_from_the_docs() {
    let item: Item = exact(json!({
        "id": "i1", "turnId": "t1", "status": "inProgress",
        "kind": "request", "requestId": "r1", "requestKind": "question", "blocking": true, "state": "pending",
        "questions": [{
            "id": "q1", "header": "Approach", "text": "Which fix do you want?",
            "options": [{ "label": "Patch decoder", "description": "…" }],
            "multiSelect": false, "allowOther": true, "secret": false
        }]
    }));
    let ItemBody::Request(request) = &item.body else { panic!("not a request") };
    assert_eq!(request.state, RequestState::Pending);
    let RequestBody::Question(question) = &request.body else { panic!("not a question") };
    assert!(question.questions[0].allow_other);
    assert_eq!(item.kind(), "request");
}

#[test]
fn approval_with_offered_decisions_and_its_answer() {
    let item: Item = exact(json!({
        "id": "i2", "turnId": "t1", "status": "inProgress",
        "kind": "request", "requestId": "r2", "requestKind": "approval", "blocking": true, "state": "resolved",
        "resolution": { "by": "elsewhere", "answer": { "requestKind": "approval", "decision": "deny", "reason": "no" } },
        "approvalKind": "command", "title": "Run tests", "command": "cargo test", "cwd": "/repo", "changes": [],
        "decisions": [
            { "id": "allow", "label": "Allow once", "kind": "allow", "acceptsReason": false },
            { "id": "session", "label": "Allow for session", "kind": "allowSession", "acceptsReason": false },
            { "id": "deny", "label": "Deny", "kind": "deny", "acceptsReason": true }
        ]
    }));
    let ItemBody::Request(request) = item.body else { panic!() };
    assert_eq!(request.resolution.unwrap().by, AnsweredBy::Elsewhere);
}

#[test]
fn thread_event_from_the_docs() {
    let event: ThreadEventParams = exact(json!({
        "threadId": "codex:abc", "epoch": 3, "seq": 412,
        "event": { "type": "itemDelta", "itemId": "i1", "field": "text", "delta": "Hel" }
    }));
    assert_eq!(event.position_after(), JournalPosition { epoch: Epoch(3), seq: Seq(412) });
    let collapsed: ThreadEventParams = exact(json!({
        "threadId": "codex:abc", "epoch": 3, "seq": 400,
        "event": { "type": "itemUpdated", "throughSeq": 431,
                   "item": { "id": "i1", "turnId": "t", "status": "completed", "kind": "agentMessage", "text": "Hello" } }
    }));
    assert_eq!(collapsed.position_after().seq, Seq(431));
    assert!(collapsed.position_after().admits(Epoch(3), Seq(432)));
    assert!(!collapsed.position_after().admits(Epoch(3), Seq(431)));
    assert!(!collapsed.position_after().admits(Epoch(4), Seq(432)));
}

#[test]
fn subscribe_results_from_the_docs() {
    let replay: ThreadSubscribeResult =
        exact(json!({ "mode": "replay", "epoch": 3, "fromSeq": 399, "headSeq": 431, "control": "free" }));
    assert!(matches!(replay.mode, SubscribeMode::Replay(SubscribeReplay { from_seq: Seq(399), .. })));
    let reset: ThreadSubscribeResult = exact(json!({ "mode": "reset", "epoch": 4, "headSeq": 12, "control": "free" }));
    assert!(matches!(reset.mode, SubscribeMode::Reset(_)));
    let future: ThreadSubscribeResult = exact(json!({ "mode": "zzStream", "epoch": 4, "control": "zzBorrowed" }));
    assert!(!future.mode.is_known());
    assert!(!future.control.may_act(), "an unknown control state is treated as in use");
}

#[test]
fn thread_sync_from_the_docs() {
    let params: ThreadSyncParams = exact(json!({ "token": "idx3:8841" }));
    assert_eq!(params.token.as_str(), "idx3:8841");
    let result: ThreadSyncResult = exact(json!({
        "changed": [], "removed": ["codex:gone"], "token": "idx3:8912", "more": false, "full": false, "staleInstances": []
    }));
    assert_eq!(result.removed.len(), 1);
}

#[test]
fn mesh_message_from_the_docs() {
    let message: MeshMessage = exact(json!({
        "id": "m_01JD", "from": "local/codex:0198", "to": "devbox/claude:7f3c", "kind": "message",
        "text": "The schema migration is merged. You can resume the integration tests.",
        "sentAt": "2026-11-20T18:04:11Z", "seq": 1
    }));
    assert_eq!(message.to.target(), Some(TargetId::new("devbox")));
    assert_eq!(message.to.thread(), Some(ThreadId::new("claude:7f3c")));
    // `inReplyTo: null` in the doc example means the same as absent.
    let (_, back) = roundtrip::<MeshMessage>(json!({
        "id": "m", "from": "a/b", "to": "c/d", "kind": "result", "text": "", "inReplyTo": null, "sentAt": "t", "seq": 2
    }));
    assert!(back.get("inReplyTo").is_none());
    // An address without the expected shape is kept, not rejected.
    let odd = Address("no-slash".into());
    assert_eq!(odd.target(), None);
    let receipt: MeshReceiptResult = exact(json!({ "receipt": { "status": "queued", "reason": "targetOffline" } }));
    assert_eq!(receipt.receipt.reason, Some(ReceiptReason::TargetOffline));
}

#[test]
fn unknown_capabilities_are_kept_as_plain_strings() {
    let caps: Vec<Capability> = exact(json!(["meshTools", "imageInput", "zzTeleport"]));
    assert_eq!(caps[0], Capability::MeshTools);
    assert_eq!(caps[2], Capability::Unknown("zzTeleport".into()));
    assert_eq!(serde_json::to_value(&caps[2]).unwrap(), json!("zzTeleport"));
    assert!(has_capability(&caps, &Capability::ImageInput));
    assert!(!Capability::Unknown("x".into()).is_known());
    // Every known capability is a camelCase string and decodes back to itself.
    for cap in Capability::KNOWN {
        assert_eq!(Capability::from_wire(cap.as_str()), *cap);
        assert!(cap.as_str().chars().next().unwrap().is_ascii_lowercase());
    }
}

#[test]
fn support_is_three_valued_and_open() {
    let values: Vec<Support> = exact(json!(["supported", "unsupported", "unknown", "zzPartial"]));
    assert!(values[0].is_supported());
    assert!(!values[1].is_supported() && !values[1].is_unknown());
    assert!(values[2].is_unknown());
    assert!(values[3].is_unknown(), "a value this build does not know counts as unknown, never as yes");
}

#[test]
fn unknown_item_kinds_round_trip_exactly() {
    let wire = json!({
        "id": "i9", "turnId": "t9", "status": "completed",
        "kind": "zzHologram", "frames": [1, 2, 3], "caption": { "text": "future" }
    });
    let item: Item = exact(wire);
    assert_eq!(item.kind(), "zzHologram");
    assert!(!item.body.is_known());
    let ItemBody::Unknown(unknown) = &item.body else { panic!() };
    assert_eq!(unknown.fields["frames"], json!([1, 2, 3]));
}

#[test]
fn unknown_variants_nested_in_flattened_bodies_round_trip() {
    // An unknown request kind inside a known item kind.
    let item: Item = exact(json!({
        "id": "i", "turnId": "t", "status": "inProgress", "kind": "request",
        "requestId": "r", "blocking": true, "state": "zzWaitingForBiometrics",
        "requestKind": "zzSignature", "document": "contract.pdf"
    }));
    let ItemBody::Request(request) = &item.body else { panic!() };
    assert_eq!(request.state, RequestState::Other("zzWaitingForBiometrics".into()));
    assert_eq!(request.body.tag(), "zzSignature");
    // Unknown events, unknown input parts, unknown mesh queries.
    let _: ThreadEventParams = exact(json!({ "threadId": "t", "epoch": 1, "seq": 2, "event": { "type": "zzWeather", "sunny": true } }));
    let _: Vec<InputPart> = exact(json!([{ "type": "zzAudio", "uploadId": "u", "seconds": 3 }]));
    let _: MeshQueryParams = exact(json!({ "caller": "a/b", "target": "c", "query": { "tool": "zzTeleport", "to": "mars" } }));
}

#[test]
fn unknown_fields_are_ignored_not_errors() {
    let summary: ThreadSummary = serde_json::from_value(json!({
        "id": "codex:1", "instance": "pi_1", "title": "Fix", "cwd": "/repo", "state": "idle", "control": "free",
        "originKind": "yaac", "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-01T00:00:00Z",
        "zzMood": "optimistic", "project": { "path": "/repo/.git", "displayName": "repo", "zzStars": 5 }
    }))
    .unwrap();
    let back = serde_json::to_value(&summary).unwrap();
    assert!(back.get("zzMood").is_none());
    assert_eq!(back["project"], json!({ "path": "/repo/.git", "displayName": "repo" }));
    assert_eq!(summary.children, 0);
    assert_eq!(summary.preview, "");
}

#[test]
fn malformed_known_variants_are_errors() {
    // A known kind with a missing required field is an error (it is not "unknown").
    assert!(serde_json::from_value::<Item>(json!({ "id": "i", "turnId": "t", "status": "completed", "kind": "agentMessage" })).is_err());
    // A tagged object without its tag is an error.
    assert!(serde_json::from_value::<InputPart>(json!({ "text": "x" })).is_err());
    assert!(serde_json::from_value::<InputPart>(json!({ "type": 3 })).is_err());
}

#[test]
fn initialize_shapes_from_the_docs() {
    let params: InitializeParams = serde_json::from_value(json!({
        "protocol": { "min": 5, "max": 8 }, "client": { "name": "yaac", "version": "0.9.0", "host": "laptop" }
    }))
    .unwrap();
    assert_eq!(
        serde_json::to_value(&params).unwrap(),
        json!({ "protocol": { "min": 5, "max": 8 }, "client": { "name": "yaac", "version": "0.9.0", "host": "laptop" } })
    );
    let result: InitializeResult = serde_json::from_value(json!({
        "protocol": { "min": 3, "max": 6, "agreed": 6 }, "schemaHash": "9f2c41d0",
        "daemon": { "build": "0.7.2+a1b2c3", "stateFormat": 4 },
        "target": { "os": "linux", "arch": "x86_64", "hostname": "devbox", "user": "me", "home": "/home/me" }
    }))
    .unwrap();
    assert_eq!(serde_json::to_value(&result).unwrap()["protocol"], json!({ "min": 3, "max": 6, "agreed": 6 }));
}

#[test]
fn every_state_changing_request_carries_an_op_id() {
    let schema = yaac_protocol::codegen::current_schema();
    let defs = &schema["$defs"];
    for info in REQUESTS {
        if !info.state_changing {
            continue;
        }
        let params = (info.params_type)();
        let required = defs[&params]["required"].as_array().cloned().unwrap_or_default();
        assert!(required.contains(&json!("opId")), "{} ({params}) must require opId", info.name);
    }
    for name in ["thread/start", "turn/start", "turn/steer", "thread/fork", "request/respond"] {
        assert!(yaac_protocol::methods::request_info(name).unwrap().state_changing, "{name}");
    }
    assert_eq!(methods::TurnStart::NAME, "turn/start");
}

#[test]
fn method_names_are_unique_and_catalogued_once() {
    let mut names: Vec<&str> = REQUESTS.iter().map(|m| m.name).collect();
    names.extend(yaac_protocol::methods::NOTIFICATIONS.iter().map(|n| n.name));
    let count = names.len();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), count, "duplicate method name");
    for frozen in ["initialize", "daemon/status", "daemon/handover"] {
        let info = yaac_protocol::methods::request_info(frozen).unwrap();
        assert!(info.frozen);
        assert!(yaac_protocol::methods::allowed_before_initialize(frozen));
    }
    assert!(!yaac_protocol::methods::allowed_before_initialize("turn/start"));
    // Mesh direction exceptions.
    assert_eq!(yaac_protocol::methods::request_info("mesh/relay").unwrap().direction, methods::Direction::DaemonToHost);
    assert_eq!(yaac_protocol::methods::request_info("mesh/query").unwrap().direction, methods::Direction::Both);
}
