//! Checks that the current schema is an **additive** change of a frozen baseline: what an
//! older peer at the same version would still read correctly.
//!
//! Allowed: new definitions, new optional properties, new enum values and union variants
//! (enums and unions are open), new methods and notifications.
//!
//! Not allowed without a version bump: a removed definition, property, method, or enum value;
//! a property that became required or stopped being required; a changed type, reference, or
//! constant; a method whose params or result type changed.

use std::collections::BTreeSet;

use serde_json::{Map, Value};

/// Every incompatibility found, as readable paths. Empty means compatible.
pub fn check(baseline: &Value, current: &Value) -> Vec<String> {
    let mut errors = Vec::new();
    let empty = Map::new();
    let base_defs = baseline.get("$defs").and_then(Value::as_object).unwrap_or(&empty);
    let cur_defs = current.get("$defs").and_then(Value::as_object).unwrap_or(&empty);
    for (name, base) in base_defs {
        match cur_defs.get(name) {
            None => errors.push(format!("$defs/{name}: definition removed")),
            Some(cur) => schema(base, cur, &format!("$defs/{name}"), &mut errors),
        }
    }
    for list in ["x-yaac-methods", "x-yaac-notifications"] {
        catalogue(baseline.get(list), current.get(list), list, &mut errors);
    }
    errors
}

fn catalogue(base: Option<&Value>, cur: Option<&Value>, list: &str, errors: &mut Vec<String>) {
    let entries = |v: Option<&Value>| v.and_then(Value::as_array).cloned().unwrap_or_default();
    let cur = entries(cur);
    for base in entries(base) {
        let name = base.get("name").and_then(Value::as_str).unwrap_or("?");
        let Some(found) = cur.iter().find(|m| m.get("name").and_then(Value::as_str) == Some(name)) else {
            errors.push(format!("{list}/{name}: removed"));
            continue;
        };
        for key in ["params", "result", "direction"] {
            if base.get(key) != found.get(key) {
                errors.push(format!("{list}/{name}: {key} changed"));
            }
        }
    }
}

fn types(schema: &Value) -> BTreeSet<String> {
    match schema.get("type") {
        Some(Value::String(t)) => [t.clone()].into(),
        Some(Value::Array(ts)) => ts.iter().filter_map(Value::as_str).map(str::to_owned).collect(),
        _ => BTreeSet::new(),
    }
}

fn required(schema: &Value) -> BTreeSet<String> {
    schema
        .get("required")
        .and_then(Value::as_array)
        .map(|r| r.iter().filter_map(Value::as_str).map(str::to_owned).collect())
        .unwrap_or_default()
}

fn schema(base: &Value, cur: &Value, path: &str, errors: &mut Vec<String>) {
    if let (Value::Bool(b), Value::Bool(c)) = (base, cur) {
        if b != c {
            errors.push(format!("{path}: changed from {b} to {c}"));
        }
        return;
    }
    if base.get("$ref") != cur.get("$ref") {
        errors.push(format!("{path}: reference changed"));
        return;
    }
    if base.get("const") != cur.get("const") {
        errors.push(format!("{path}: constant changed"));
    }
    if types(base) != types(cur) {
        errors.push(format!("{path}: type changed from {:?} to {:?}", types(base), types(cur)));
    }
    if let Some(base_enum) = base.get("enum").and_then(Value::as_array) {
        let cur_enum = cur.get("enum").and_then(Value::as_array).cloned().unwrap_or_default();
        for value in base_enum {
            if !cur_enum.contains(value) {
                errors.push(format!("{path}: enum value {value} removed"));
            }
        }
    }
    let empty = Map::new();
    let base_props = base.get("properties").and_then(Value::as_object).unwrap_or(&empty);
    let cur_props = cur.get("properties").and_then(Value::as_object).unwrap_or(&empty);
    for (name, base_prop) in base_props {
        match cur_props.get(name) {
            None => errors.push(format!("{path}.{name}: property removed")),
            Some(cur_prop) => schema(base_prop, cur_prop, &format!("{path}.{name}"), errors),
        }
    }
    let (base_req, cur_req) = (required(base), required(cur));
    for name in cur_req.difference(&base_req) {
        errors.push(format!("{path}.{name}: became required"));
    }
    for name in base_req.difference(&cur_req) {
        errors.push(format!("{path}.{name}: is no longer required"));
    }
    for key in ["items", "additionalProperties"] {
        match (base.get(key), cur.get(key)) {
            (Some(b), Some(c)) => schema(b, c, &format!("{path}/{key}"), errors),
            (Some(_), None) => errors.push(format!("{path}/{key}: removed")),
            _ => {}
        }
    }
    if let Some(base_all) = base.get("allOf").and_then(Value::as_array) {
        let cur_all = cur.get("allOf").and_then(Value::as_array).cloned().unwrap_or_default();
        if base_all.len() != cur_all.len() {
            errors.push(format!("{path}/allOf: changed"));
        } else {
            for (i, (b, c)) in base_all.iter().zip(&cur_all).enumerate() {
                schema(b, c, &format!("{path}/allOf/{i}"), errors);
            }
        }
    }
    for key in ["anyOf", "oneOf"] {
        let Some(base_members) = base.get(key).and_then(Value::as_array) else { continue };
        let cur_members = cur.get(key).and_then(Value::as_array).cloned().unwrap_or_default();
        // Every member of the baseline must still be accepted by some current member.
        for (i, member) in base_members.iter().enumerate() {
            let matched = cur_members.iter().any(|candidate| {
                let mut scratch = Vec::new();
                schema(member, candidate, "", &mut scratch);
                scratch.is_empty()
            });
            if !matched {
                // Report why the member in the same position no longer fits, when there is one.
                match cur_members.get(i) {
                    Some(same) => schema(member, same, &format!("{path}/{key}/{i}"), errors),
                    None => errors.push(format!("{path}/{key}/{i}: member removed")),
                }
            }
        }
    }
}
