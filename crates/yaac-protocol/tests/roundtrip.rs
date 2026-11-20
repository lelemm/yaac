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
