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
    assert!(matches!(&parts[1], InputPart::Image(ImagePart { upload_id, .. }) if upload_id.as_str() == "u_01J"));
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

#[test]
fn summary_flags_git_status_and_projects() {
    let summary: ThreadSummary = exact(json!({
        "id": "t1", "instance": "pi_1", "title": "Fix", "preview": "", "cwd": "/home/u/code/yaac-wt/feature-x", "state": "waiting",
        "control": "free", "originKind": "yaac", "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-01T00:00:00Z",
        "children": 0, "peers": 0, "archived": false, "agents": "allow", "fast": false,
        "project": {
            "path": "/home/u/code/yaac", "displayName": "YAAC", "kind": "user", "id": "pj_1",
            "repo": { "repo": "/home/u/code/yaac/.git", "relative": ".", "root": "/home/u/code/yaac" }
        },
        "repo": { "repo": "/home/u/code/yaac/.git", "relative": ".", "root": "/home/u/code/yaac-wt/feature-x", "worktreeName": "feature-x" },
        "git": { "branch": "feature-x", "base": "origin/main", "added": 1, "modified": 2, "deleted": 3, "ahead": 4, "behind": 5, "removed": false }
    }));
    assert!(summary.agents.allows());
    // Pin, mute, and dismiss are the host's view preferences (D92): not in a summary, and a
    // summary from a daemon that still sends them decodes without them.
    let with_flags = json!({ "id": "t", "instance": "pi_1", "title": "x", "cwd": "/r", "state": "idle", "control": "free",
        "originKind": "yaac", "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-01T00:00:00Z", "muted": true, "pinned": true });
    let decoded: ThreadSummary = serde_json::from_value(with_flags).unwrap();
    let back = serde_json::to_value(&decoded).unwrap();
    assert!(back.get("muted").is_none() && back.get("pinned").is_none() && back.get("dismissed").is_none());
    assert_eq!(summary.state, ThreadState::Waiting);
    assert_eq!(summary.project.as_ref().unwrap().kind, Some(ProjectKind::User));
    assert_eq!(summary.git.as_ref().unwrap().ahead, 4);
    // A summary from an older daemon has none of them: unmuted, unpinned, agents ask.
    let old: ThreadSummary = serde_json::from_value(json!({
        "id": "t", "instance": "pi_1", "title": "x", "cwd": "/r", "state": "done", "control": "free",
        "originKind": "yaac", "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-01T00:00:00Z"
    }))
    .unwrap();
    assert!(old.git.is_none());
    assert_eq!(old.agents, AgentsPolicy::Ask);
    assert_eq!(old.state, ThreadState::Done);
    // An agents policy this build does not know is kept, and does not allow.
    let future: AgentsPolicy = serde_json::from_value(json!("zzSometimes")).unwrap();
    assert!(!future.allows());
}

#[test]
fn mesh_permission_request_and_answer() {
    let item: Item = exact(json!({
        "id": "request:r1", "turnId": "u1", "status": "inProgress", "startedAt": "2026-01-01T00:00:00Z",
        "kind": "request", "requestId": "r1", "state": "pending", "blocking": true, "requestKind": "meshPermission",
        "actions": [
            { "action": "startThreads", "summary": "Start 3 threads", "recipients": [],
              "threads": [{ "count": 3, "instance": "pi_2", "target": "tg_1", "targetName": "devbox" }] },
            { "action": "message", "summary": "Message CI integration on devbox", "threads": [],
              "recipients": [{ "address": "tg_1/t9", "title": "CI integration" }] }
        ],
        "budget": { "agentTurns": 100, "threadsSpawned": 20, "wallClockSecs": 43200 },
        "root": "tg_0/t1"
    }));
    let ItemBody::Request(r) = &item.body else { panic!("not a request") };
    let RequestBody::MeshPermission(p) = &r.body else { panic!("not a permission") };
    assert_eq!(p.actions.len(), 2);
    assert_eq!(p.actions[0].action, MeshActionKind::StartThreads);
    let answer: RequestRespondParams = exact(json!({
        "opId": "op", "threadId": "t1", "requestId": "r1",
        "response": { "requestKind": "meshPermission", "decision": "allow", "alwaysAllow": true,
                      "budget": { "agentTurns": 50, "threadsSpawned": 5, "wallClockSecs": 3600 } }
    }));
    let RequestResponse::MeshPermission(a) = &answer.response else { panic!() };
    assert!(a.always_allow);
    assert_eq!(a.decision, MeshPermissionDecision::Allow);
}

#[test]
fn projects_item_read_fork_only_usage_and_context() {
    let _: ProjectListResult = exact(json!({ "projects": [
        { "id": "pj_1", "name": "Billing API", "folder": "/srv/mono/services/billing", "createdAt": "2026-01-01T00:00:00Z",
          "repo": { "repo": "/srv/mono/.git", "relative": "services/billing", "root": "/srv/mono" }, "crossTargetKey": "host/mono" }
    ]}));
    let _: ProjectCreateParams = exact(json!({ "opId": "op", "folder": "/srv/x", "name": "X" }));
    let _: ThreadUpdateParams = exact(json!({ "opId": "op", "threadId": "t", "agents": "ask" }));
    let _: ItemReadResult = exact(json!({ "present": true, "data": "abc", "offset": 10, "nextOffset": 13, "totalBytes": 20, "more": true }));
    let _: ThreadForkOnlyParams = exact(json!({ "opId": "op", "source": "t", "instance": "pi_1" }));
    let _: MeshUsageParams = exact(json!({ "root": "tg_0/t1", "source": "tg_1", "agentTurns": 3, "threadsSpawned": 1 }));
    let read: ThreadReadResult = exact(json!({
        "items": [], "position": { "epoch": 1, "seq": 2 }, "uploads": [], "context": { "used": 10, "window": 100 }
    }));
    assert_eq!(read.context.unwrap().window, Some(100));
    let parts: Vec<InputPart> = exact(json!([{ "type": "file", "uploadId": "u_1", "name": "notes.txt", "mediaType": "text/plain" }]));
    assert!(matches!(&parts[0], InputPart::File(FilePart { name: Some(n), .. }) if n == "notes.txt"));
    for (name, op) in [("thread/update", true), ("thread/forkOnly", true), ("item/read", false), ("project/list", false),
        ("project/create", true), ("project/rename", true), ("project/delete", true), ("mesh/usage", false)] {
        assert_eq!(yaac_protocol::methods::request_info(name).unwrap().state_changing, op, "{name}");
    }
    assert!(yaac_protocol::methods::notification_info("project/updated").is_some());
}

#[test]
fn attach_hello_carries_the_client_id_and_target_state_enabled() {
    use yaac_protocol::attach::{HelloResult, TargetStateParams};
    let hello: HelloResult = serde_json::from_value(json!({ "build": "b", "targets": [], "clientId": "c_1" })).unwrap();
    assert_eq!(hello.client_id.as_ref().map(ClientId::as_str), Some("c_1"));
    // An older hub sends no identity: the client falls back to the address.
    assert!(hello.hub_id.is_none() && hello.host_name.is_none());
    let named: HelloResult = serde_json::from_value(json!({ "build": "b", "targets": [], "hubId": "h_1", "hostName": "DESK" })).unwrap();
    assert_eq!((named.hub_id.as_deref(), named.host_name.as_deref()), (Some("h_1"), Some("DESK")));
    assert_eq!(serde_json::to_value(&named).unwrap(), json!({ "build": "b", "targets": [], "hubId": "h_1", "hostName": "DESK" }));
    let state: TargetStateParams = serde_json::from_value(json!({ "id": "tg", "state": { "status": "disabled" }, "enabled": false })).unwrap();
    assert!(!state.enabled);
    let older: TargetStateParams = serde_json::from_value(json!({ "id": "tg", "state": { "status": "connected" } })).unwrap();
    assert!(older.enabled);
}

#[test]
fn preview_wire_shapes() {
    exact::<TunnelOpenParams>(json!({"port": 5173, "initialCredit": 123}));
    exact::<TunnelOpenResult>(json!({"stream": 21, "initialCredit": 456}));
    exact::<TunnelCloseParams>(json!({"stream": 21, "reason": "socket closed"}));
    exact::<PreviewPortsResult>(json!({"ports":[{"port":5173,"host":"::1","pid":4,"cwd":"/dev"}]}));
    exact::<attach::PreviewProxyResult>(json!({"socksPort":12345}));
    exact::<attach::PreviewListResult>(json!([{"target":"local","kind":"proxy","localPort":12345,"connections":1}]));
}

#[test]
fn browser_inspection_wire_shapes() {
    for (action, reply) in [
        (json!({"kind":"list"}), json!({"kind":"list","tabs":[{"tab":"a","url":"about:blank","title":"","loading":false}]})),
        (json!({"kind":"snapshot","tab":"a"}), json!({"kind":"snapshot","tree":"document"})),
        (json!({"kind":"screenshot","tab":"a","selector":"h1"}), json!({"kind":"screenshot","data":"aQ=="})),
        (json!({"kind":"console","tab":"a","cursor":1}), json!({"kind":"console","entries":[{"level":"log","text":"ok"}],"cursor":2})),
        (json!({"kind":"network","tab":"a"}), json!({"kind":"network","entries":[{"url":"/","method":"GET","status":200,"durationMs":1}],"cursor":1})),
        (json!({"kind":"future","extra":1}), json!({"kind":"future","value":true})),
    ] {
        exact::<BrowserCallParams>(json!({"threadId":"t","action":action}));
        exact::<attach::BrowserCallParams>(json!({"target":"tg","threadId":"t","action":action}));
        exact::<BrowserCallResult>(reply);
    }
    exact::<rpc::ErrorCode>(json!("no_window"));
}
