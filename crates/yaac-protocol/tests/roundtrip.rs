//! Every message type round-trips through JSON, including values this build does not know.
//!
//! Instances are generated from the protocol's own JSON Schema, so every params and result
//! type of every method — and everything nested in them — is exercised without a hand-written
//! sample per type. The generator also picks the schema's `x-yaac-unknown` members: unknown
//! enum strings (capabilities included) and unknown union variants (item kinds included),
//! which must come back exactly as sent. With `extras`, every plain object additionally gets
//! fields this build does not know, which must be ignored.

// Test code: small index casts and by-value helpers keep the tests readable.
#![allow(clippy::cast_possible_truncation, clippy::needless_pass_by_value, clippy::type_complexity)]

use serde_json::{Map, Value, json};
use yaac_protocol::codegen::current_schema;
use yaac_protocol::methods::{NOTIFICATIONS, REQUESTS};

struct Gen<'a> {
    defs: &'a Map<String, Value>,
    state: u64,
    extras: bool,
    depth: usize,
    unknown_seen: usize,
}

impl<'a> Gen<'a> {
    fn new(defs: &'a Map<String, Value>, seed: u64, extras: bool) -> Self {
        Self { defs, state: seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1, extras, depth: 0, unknown_seen: 0 }
    }

    fn next(&mut self) -> u64 {
        // xorshift64*
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }

    fn string(&mut self, prefix: &str) -> String {
        let len = self.below(8) as usize;
        let alphabet = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_/: .";
        let tail: String = (0..len).map(|_| alphabet[self.below(alphabet.len() as u64) as usize] as char).collect();
        format!("{prefix}{tail}")
    }

    fn any_scalar(&mut self) -> Value {
        match self.below(4) {
            0 => Value::String(self.string("v")),
            1 => json!(self.below(1000)),
            2 => Value::Bool(self.chance(50)),
            _ => json!({ "nested": self.string("n") }),
        }
    }

    fn value(&mut self, schema: &Value) -> Value {
        self.depth += 1;
        let out = self.value_inner(schema);
        self.depth -= 1;
        out
    }

    fn value_inner(&mut self, schema: &Value) -> Value {
        let Value::Object(map) = schema else {
            return self.any_scalar();
        };
        if let Some(Value::String(reference)) = map.get("$ref") {
            let name = reference.rsplit('/').next().unwrap_or_default();
            let def = self.defs.get(name).unwrap_or_else(|| panic!("missing def {name}")).clone();
            return self.value(&def);
        }
        if let Some(constant) = map.get("const") {
            return constant.clone();
        }
        if let Some(Value::Array(values)) = map.get("enum") {
            return values[self.below(values.len() as u64) as usize].clone();
        }
        if map.get("x-yaac-unknown").and_then(Value::as_bool) == Some(true) {
            return self.unknown(map);
        }
        let mut parts: Vec<Value> = Vec::new();
        let has_union = map.contains_key("anyOf") || map.contains_key("oneOf");
        if let Some(base) = self.typed(map, !has_union) {
            parts.push(base);
        }
        if let Some(Value::Array(all)) = map.get("allOf") {
            for member in all {
                parts.push(self.value(member));
            }
        }
        for key in ["anyOf", "oneOf"] {
            if let Some(Value::Array(members)) = map.get(key) {
                let member = members[self.below(members.len() as u64) as usize].clone();
                parts.push(self.value(&member));
            }
        }
        merge(parts)
    }

    /// A value for an `x-yaac-unknown` member: something this build does not know.
    fn unknown(&mut self, map: &Map<String, Value>) -> Value {
        self.unknown_seen += 1;
        if map.get("type").and_then(Value::as_str) == Some("string") {
            return Value::String(self.string("zzUnknown"));
        }
        let mut object = Map::new();
        if let Some(Value::Array(required)) = map.get("required") {
            for tag in required.iter().filter_map(Value::as_str) {
                object.insert(tag.to_owned(), Value::String(self.string("zzUnknown")));
            }
        }
        for i in 0..self.below(3) {
            object.insert(format!("zzField{i}"), self.any_scalar());
        }
        Value::Object(object)
    }

    fn typed(&mut self, map: &Map<String, Value>, plain: bool) -> Option<Value> {
        let names: Vec<&str> = match map.get("type")? {
            Value::String(t) => vec![t.as_str()],
            Value::Array(ts) => ts.iter().filter_map(Value::as_str).collect(),
            _ => return None,
        };
        let non_null: Vec<&str> = names.iter().copied().filter(|t| *t != "null").collect();
        let pick = if non_null.is_empty() || (names.contains(&"null") && self.chance(20)) {
            "null"
        } else {
            non_null[self.below(non_null.len() as u64) as usize]
        };
        Some(match pick {
            "null" => Value::Null,
            "boolean" => Value::Bool(self.chance(50)),
            "string" => Value::String(self.string("s")),
            "integer" => {
                let (lo, hi) = match map.get("format").and_then(Value::as_str) {
                    Some("uint8") => (0, 255),
                    Some("uint16") => (0, 65_535),
                    Some("uint32") => (0, u64::from(u32::MAX)),
                    Some("int32") => (0, i32::MAX as u64),
                    _ => (0, 1 << 53),
                };
                let lo = map.get("minimum").and_then(Value::as_u64).unwrap_or(lo).max(lo);
                let hi = map.get("maximum").and_then(Value::as_u64).unwrap_or(hi).min(hi);
                let n = if self.chance(50) { lo + self.below(10).min(hi - lo) } else { lo + self.below(hi - lo + 1) };
                json!(n)
            }
            "number" => json!(self.below(1000)),
            "array" => {
                let count = if self.depth > 8 { 0 } else { self.below(3) };
                let items = map.get("items").cloned().unwrap_or(Value::Bool(true));
                Value::Array((0..count).map(|_| self.value(&items)).collect())
            }
            "object" => self.object(map, plain),
            other => panic!("unexpected type {other}"),
        })
    }

    fn object(&mut self, map: &Map<String, Value>, plain: bool) -> Value {
        let mut object = Map::new();
        let required: Vec<&str> =
            map.get("required").and_then(Value::as_array).map(|r| r.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
        if let Some(Value::Object(props)) = map.get("properties") {
            for (name, prop) in props {
                let has_default = prop.get("default").is_some();
                let include = required.contains(&name.as_str()) || has_default || (self.depth <= 8 && self.chance(60));
                if include {
                    let value = self.value(prop);
                    object.insert(name.clone(), value);
                }
            }
            if plain && self.extras {
                object.insert("zzIgnoredField".to_owned(), json!({ "from": "the future" }));
            }
        } else if let Some(additional) = map.get("additionalProperties") {
            let count = if self.depth > 8 { 0 } else { self.below(3) };
            for _ in 0..count {
                let key = self.string("k");
                let value = self.value(additional);
                object.insert(key, value);
            }
        }
        Value::Object(object)
    }
}

fn merge(parts: Vec<Value>) -> Value {
    let mut parts = parts.into_iter();
    let Some(first) = parts.next() else { return Value::Null };
    let mut out = first;
    for part in parts {
        match (&mut out, part) {
            (Value::Object(acc), Value::Object(more)) => acc.extend(more),
            (_, other) => out = other,
        }
    }
    out
}

/// Removes `null` members (an absent optional and a `null` one mean the same) and the
/// injected unknown fields (which must be ignored).
fn normalise(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(key, v)| !v.is_null() && key.as_str() != "zzIgnoredField")
                .map(|(key, v)| (key.clone(), normalise(v)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(normalise).collect()),
        other => other.clone(),
    }
}

/// Every type reachable from the catalogue, `ITERATIONS` random instances each.
const ITERATIONS: u64 = 300;

fn run(extras: bool) {
    let schema = current_schema();
    let defs = schema["$defs"].as_object().expect("defs").clone();
    let mut cases = 0;
    let mut unknown = 0;
    let mut check = |label: &str, seed: u64, param_schema: &Value, roundtrip: fn(Value) -> Result<Value, String>| {
        let mut generator = Gen::new(&defs, seed, extras);
        let input = generator.value(param_schema);
        unknown += generator.unknown_seen;
        let output = roundtrip(input.clone()).unwrap_or_else(|err| {
            panic!("{label} (seed {seed}) did not decode: {err}\n{}", serde_json::to_string_pretty(&input).unwrap())
        });
        assert_eq!(
            normalise(&output),
            normalise(&input),
            "{label} (seed {seed}) changed in a round trip\ninput: {}\noutput: {}",
            serde_json::to_string_pretty(&input).unwrap(),
            serde_json::to_string_pretty(&output).unwrap()
        );
        // Re-encoding is stable.
        assert_eq!(roundtrip(output.clone()).unwrap(), output, "{label} (seed {seed}) is not stable");
        cases += 1;
    };
    let methods = schema["x-yaac-methods"].as_array().unwrap();
    for (info, wire) in REQUESTS.iter().zip(methods) {
        assert_eq!(wire["name"], info.name);
        for seed in 0..ITERATIONS {
            check(&format!("{} params", info.name), seed, &wire["params"], info.roundtrip_params);
            check(&format!("{} result", info.name), seed, &wire["result"], info.roundtrip_result);
        }
    }
    let notifications = schema["x-yaac-notifications"].as_array().unwrap();
    for (info, wire) in NOTIFICATIONS.iter().zip(notifications) {
        for seed in 0..ITERATIONS {
            check(&format!("{} params", info.name), seed, &wire["params"], info.roundtrip_params);
        }
    }
    assert!(cases > 10_000, "only {cases} cases");
    assert!(unknown > 1_000, "unknown values were barely exercised ({unknown})");
}

#[test]
fn every_message_type_round_trips_including_unknown_values() {
    run(false);
}

#[test]
fn unknown_fields_are_ignored_everywhere() {
    run(true);
}

#[test]
fn envelope_round_trips() {
    use yaac_protocol::rpc::{ErrorCode, ErrorObject, Message, RpcId};
    let messages = [
        Message::request(RpcId::Number(7), "turn/start", json!({ "threadId": "t" })),
        Message::Response { id: RpcId::String("a".into()), result: json!({ "turnId": "u" }) },
        Message::Error {
            id: Some(RpcId::Number(7)),
            error: ErrorObject::new(ErrorCode::ThreadInUse, "in use").with_data(json!({ "holder": "laptop" })),
        },
        Message::Error { id: None, error: ErrorObject::new(ErrorCode::from_wire("zz_future_code"), "?") },
        Message::notification("thread/event", json!({ "seq": 1 })),
    ];
    for message in messages {
        let bytes = message.to_json();
        assert_eq!(Message::from_json(&bytes).unwrap(), message);
    }
    // No `jsonrpc` member is written; one that is present is ignored.
    let bytes = Message::request(RpcId::Number(1), "ping", json!({})).to_json();
    assert_eq!(bytes, br#"{"id":1,"method":"ping","params":{}}"#);
    let with_jsonrpc = Message::from_json(br#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#).unwrap();
    assert_eq!(with_jsonrpc, Message::request(RpcId::Number(1), "ping", json!({})));
    // Unknown members of the envelope are ignored.
    let extra = Message::from_json(br#"{"method":"x","params":{},"zz":1}"#).unwrap();
    assert_eq!(extra, Message::notification("x", json!({})));
}

#[test]
fn malformed_envelopes_are_errors_not_panics() {
    use yaac_protocol::rpc::Message;
    for bad in [&b"[]"[..], b"1", b"\"x\"", b"{}", br#"{"id":1}"#, br#"{"method":3}"#, br#"{"result":1}"#, b"{not json", br#"{"id":{},"method":"x"}"#] {
        assert!(Message::from_json(bad).is_err(), "{}", String::from_utf8_lossy(bad));
    }
}

/// D110: `thread/delete` carries whether the harness's session goes too; an older host that
/// does not send it means YAAC's record only.
#[test]
fn thread_delete_carries_also_in_harness_and_defaults_to_the_record_only() {
    use yaac_protocol::types::ThreadDeleteParams;
    let p: ThreadDeleteParams = serde_json::from_value(serde_json::json!({ "opId": "op-1", "threadId": "t1", "alsoInHarness": true })).unwrap();
    assert!(p.also_in_harness);
    let back = serde_json::to_value(&p).unwrap();
    assert_eq!(back, serde_json::json!({ "opId": "op-1", "threadId": "t1", "alsoInHarness": true }));
    assert_eq!(serde_json::from_value::<ThreadDeleteParams>(back).unwrap(), p);
    let old: ThreadDeleteParams = serde_json::from_value(serde_json::json!({ "opId": "op-2", "threadId": "t2" })).unwrap();
    assert!(!old.also_in_harness);
    // The bool is always written (no skip): a reader never has to guess.
    assert_eq!(serde_json::to_value(&old).unwrap()["alsoInHarness"], false);
}

/// D140: `thread/takeOver` is an operation under `threadTakeOver`; the reason a holder will not
/// let go travels on the control event and the subscribe result, and an older peer that does
/// not send it means "can be taken over".
#[test]
fn take_over_round_trips_and_its_blocked_reason_is_optional() {
    use yaac_protocol::types::{ControlChangedEvent, ThreadTakeOverParams};
    use yaac_protocol::{Capability, Control, NoticeKind};
    let info = yaac_protocol::methods::request_info("thread/takeOver").expect("a known method");
    assert!(info.state_changing, "carries an operation id");
    let p: ThreadTakeOverParams = serde_json::from_value(json!({ "opId": "op-1", "threadId": "t1" })).unwrap();
    assert_eq!(serde_json::to_value(&p).unwrap(), json!({ "opId": "op-1", "threadId": "t1" }));
    let e: ControlChangedEvent = serde_json::from_value(json!({ "control": "external", "holder": "a background session", "takeOverBlocked": "stop it there first" })).unwrap();
    assert_eq!(e.control, Control::External);
    assert_eq!(e.take_over_blocked.as_deref(), Some("stop it there first"));
    let old: ControlChangedEvent = serde_json::from_value(json!({ "control": "external" })).unwrap();
    assert_eq!(old.take_over_blocked, None);
    assert!(serde_json::to_value(&old).unwrap().get("takeOverBlocked").is_none());
    assert_eq!(serde_json::from_value::<Capability>(json!("threadTakeOver")).unwrap(), Capability::ThreadTakeOver);
    assert_eq!(serde_json::from_value::<NoticeKind>(json!("takenOver")).unwrap(), NoticeKind::TakenOver);
}

/// D144: `thread/move` is a state-changing request (an operation id); its params name a
/// project (absent: forget the placement) and optionally a checkout; the result says whether
/// the working directory follows. A summary carries the placement, and a summary without one
/// (an older daemon) reads as "where its folder puts it".
#[test]
fn thread_move_round_trips_and_the_placement_is_optional() {
    use yaac_protocol::types::{ThreadMoveParams, ThreadMoveResult, ThreadSummary};
    let info = yaac_protocol::methods::request_info("thread/move").expect("thread/move is a request");
    assert!(info.state_changing, "it carries an operation id");
    let p: ThreadMoveParams = serde_json::from_value(json!({ "opId": "op-1", "threadId": "t1", "project": "pj_2", "worktree": "/srv/wt/x" })).unwrap();
    assert_eq!(p.project.as_ref().map(|p| p.as_str()), Some("pj_2"));
    let back = serde_json::to_value(&p).unwrap();
    assert_eq!(back, json!({ "opId": "op-1", "threadId": "t1", "project": "pj_2", "worktree": "/srv/wt/x" }));
    let clear: ThreadMoveParams = serde_json::from_value(json!({ "opId": "op-2", "threadId": "t1" })).unwrap();
    assert!(clear.project.is_none() && clear.worktree.is_none());
    assert_eq!(serde_json::to_value(&clear).unwrap(), json!({ "opId": "op-2", "threadId": "t1" }));
    let summary = json!({
        "id": "t1", "instance": "pi_1", "title": "x", "cwd": "/srv/api", "state": "idle", "control": "free",
        "originKind": "yaac", "createdAt": "2026-11-21T00:00:00Z", "updatedAt": "2026-11-21T00:00:00Z",
        "placement": { "project": "pj_2", "worktree": "/srv/wt/x", "worktreeName": "x" }
    });
    let r: ThreadMoveResult = serde_json::from_value(json!({ "thread": summary, "workingDirectory": "/srv/wt/x/api" })).unwrap();
    let placed = r.thread.placement.clone().expect("placement");
    assert_eq!(placed.worktree_name.as_deref(), Some("x"));
    assert_eq!(r.working_directory.as_ref().map(|p| p.as_str()), Some("/srv/wt/x/api"));
    let again: ThreadMoveResult = serde_json::from_value(serde_json::to_value(&r).unwrap()).unwrap();
    assert_eq!(again, r);
    let mut older = summary;
    older.as_object_mut().unwrap().remove("placement");
    let s: ThreadSummary = serde_json::from_value(older).unwrap();
    assert!(s.placement.is_none());
    assert!(serde_json::to_value(&s).unwrap().get("placement").is_none(), "absent stays absent");
    for cap in ["threadMove", "threadMoveWorkingDirectory"] {
        let c: yaac_protocol::Capability = serde_json::from_value(json!(cap)).unwrap();
        assert!(!matches!(c, yaac_protocol::Capability::Unknown(_)), "{cap} is known");
    }
}

/// D146: a mode's `autonomy` is optional (absent: unknown) and open; the spawn query carries
/// the parent's mode and an asked-for mode, and the reply a note — all absent from an older
/// peer, and absent stays absent.
#[test]
fn mode_autonomy_and_the_spawn_mode_fields_round_trip_and_are_optional() {
    use yaac_protocol::types::{ModeAutonomy, ModeInfo, SpawnThreadQuery, SpawnThreadReply};
    let m: ModeInfo = serde_json::from_value(json!({ "id": "bypass", "displayName": "Bypass", "skipsApprovals": true, "autonomy": "full" })).unwrap();
    assert_eq!(m.autonomy, Some(ModeAutonomy::Full));
    assert_eq!(serde_json::to_value(&m).unwrap()["autonomy"], json!("full"));
    let old: ModeInfo = serde_json::from_value(json!({ "id": "ask", "displayName": "Ask" })).unwrap();
    assert_eq!(old.autonomy, None);
    assert!(serde_json::to_value(&old).unwrap().get("autonomy").is_none());
    let future: ModeInfo = serde_json::from_value(json!({ "id": "x", "displayName": "X", "autonomy": "galaxyBrain" })).unwrap();
    assert_eq!(future.autonomy.as_ref().and_then(ModeAutonomy::rank), None, "an unknown level is never compared");
    assert_eq!(serde_json::to_value(&future).unwrap()["autonomy"], json!("galaxyBrain"));
    let ranks: Vec<Option<u8>> = ModeAutonomy::KNOWN.iter().map(ModeAutonomy::rank).collect();
    assert_eq!(ranks, (0..6).map(Some).collect::<Vec<_>>(), "declared least first");
    let q: SpawnThreadQuery = serde_json::from_value(json!({
        "instance": "pi_1", "text": "go", "mode": "auto",
        "parentMode": { "id": "bypass", "displayName": "Bypass", "skipsApprovals": true, "autonomy": "full" }
    }))
    .unwrap();
    assert_eq!(q.mode.as_ref().map(|m| m.as_str()), Some("auto"));
    assert_eq!(q.parent_mode.as_ref().map(|m| m.skips_approvals), Some(true));
    let plain: SpawnThreadQuery = serde_json::from_value(json!({ "instance": "pi_1", "text": "go" })).unwrap();
    let v = serde_json::to_value(&plain).unwrap();
    assert!(v.get("mode").is_none() && v.get("parentMode").is_none());
    let peer = json!({ "address": "tg_1/th_1", "title": "w" });
    let r: SpawnThreadReply = serde_json::from_value(json!({ "thread": peer, "turnId": "u1", "modeNote": "Runs in Bypass, as its parent." })).unwrap();
    assert_eq!(r.mode_note.as_deref(), Some("Runs in Bypass, as its parent."));
    let older: SpawnThreadReply = serde_json::from_value(json!({ "thread": peer, "turnId": "u1" })).unwrap();
    assert!(serde_json::to_value(&older).unwrap().get("modeNote").is_none());
}

#[test]
fn folder_trust_round_trips_and_the_switch_is_optional() {
    use yaac_protocol::{Capability, NoticeKind, ProviderInstanceConfig, ProviderInstanceUpdateParams};
    let c: Capability = serde_json::from_value(json!("folderTrust")).unwrap();
    assert_eq!(c, Capability::FolderTrust);
    let k: NoticeKind = serde_json::from_value(json!("folderTrust")).unwrap();
    assert_eq!(k, NoticeKind::FolderTrust);
    // An older yaacd's configuration has no switch: absent stays absent (the switch is on).
    let older = json!({ "id": "pi_codex", "kind": "codex", "enabled": true, "backend": "auto", "implicit": true });
    let cfg: ProviderInstanceConfig = serde_json::from_value(older).unwrap();
    assert_eq!(cfg.trust_folders, None);
    assert!(serde_json::to_value(&cfg).unwrap().get("trustFolders").is_none());
    let off: ProviderInstanceConfig = serde_json::from_value(json!({ "id": "pi_codex", "kind": "codex", "enabled": true, "backend": "auto", "trustFolders": false })).unwrap();
    assert_eq!(off.trust_folders, Some(false));
    assert_eq!(serde_json::from_value::<ProviderInstanceConfig>(serde_json::to_value(&off).unwrap()).unwrap(), off);
    let patch: ProviderInstanceUpdateParams = serde_json::from_value(json!({ "opId": "op_1", "id": "pi_codex", "trustFolders": false })).unwrap();
    assert_eq!(patch.trust_folders, Some(false));
    assert!(patch.enabled.is_none() && patch.name.is_none(), "a patch changes only what it names");
}

/// D151: a harness's own sub-agent is a thread whose summary carries `subagent` (its agent
/// type, the harness's status, and why it is read-only); its parent is `spawnedBy`. A summary
/// without it (an older daemon, an ordinary thread) reads as no sub-agent, and an unknown
/// status is kept.
#[test]
fn a_harness_subagent_round_trips_and_is_optional() {
    use yaac_protocol::types::{SubagentStatus, ThreadSummary};
    let summary = json!({
        "id": "t2", "instance": "pi_1", "title": "Explore the parser", "cwd": "/srv/api", "state": "running",
        "control": "external", "originKind": "programmatic", "createdAt": "2026-11-22T00:00:00Z",
        "updatedAt": "2026-11-22T00:00:00Z", "spawnedBy": "tg_1/t1",
        "subagent": { "agentId": "a1f0c2d3e4b5a6978", "agentType": "Explore", "status": "running", "readOnly": "Only the agent that started it can continue it." }
    });
    let s: ThreadSummary = serde_json::from_value(summary.clone()).unwrap();
    let sub = s.subagent.clone().expect("subagent");
    assert_eq!(sub.agent_type.as_deref(), Some("Explore"));
    assert_eq!(sub.agent_id.as_deref(), Some("a1f0c2d3e4b5a6978"));
    assert_eq!(sub.status, SubagentStatus::Running);
    assert!(sub.read_only.is_some());
    assert_eq!(serde_json::from_value::<ThreadSummary>(serde_json::to_value(&s).unwrap()).unwrap(), s);
    let mut older = summary.clone();
    older.as_object_mut().unwrap().remove("subagent");
    let o: ThreadSummary = serde_json::from_value(older).unwrap();
    assert!(o.subagent.is_none());
    assert!(serde_json::to_value(&o).unwrap().get("subagent").is_none(), "absent stays absent");
    let mut newer = summary;
    newer["subagent"] = json!({ "status": "paused" });
    let n: ThreadSummary = serde_json::from_value(newer).unwrap();
    assert!(matches!(n.subagent.unwrap().status, SubagentStatus::Unknown(_)));
    let c: yaac_protocol::Capability = serde_json::from_value(json!("subagentThreads")).unwrap();
    assert_eq!(c, yaac_protocol::Capability::SubagentThreads);
}
