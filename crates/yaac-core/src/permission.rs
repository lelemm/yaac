//! **SEAM (wire variant missing).** The permission prompt of screen 25: "This agent wants to
//! work with other agents", with the orchestration's budget editable in the same decision.
//!
//! The protocol has no request kind for it yet. Until it does, it travels as a request whose
//! `requestKind` is [`PERMISSION_KIND`] — which the protocol's open union keeps as
//! `RequestBody::Unknown` — and is answered the same way. This module is the only place that
//! knows that string; replace it with the real variant when `yaac-protocol` gains one.

use serde_json::{Map, Value, json};
use yaac_protocol::{RequestBody, RequestResponse, UnknownVariant};

/// The provisional `requestKind`.
pub const PERMISSION_KIND: &str = "meshPermission";

/// The budget fields of the prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BudgetFields {
    /// Agent-started turns.
    pub agent_turns: u32,
    /// Threads spawned.
    pub threads: u32,
    /// Hours without the user.
    pub hours: u32,
}

/// What the agent asks to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionRequest {
    /// One plain line per action ("Start 3 threads — <provider> · <model> on local and devbox").
    pub actions: Vec<String>,
    /// Proposed budget.
    pub budget: BudgetFields,
}

/// The user's decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermissionDecision {
    /// Allow, with the budget as edited.
    Allow,
    /// Deny.
    Deny,
    /// Allow, and stop asking in this chat.
    AlwaysAllow,
}

fn num(v: Option<&Value>, default: u32) -> u32 {
    v.and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok()).unwrap_or(default)
}

/// Read a permission request from a request body.
pub fn parse(body: &RequestBody) -> Option<PermissionRequest> {
    let RequestBody::Unknown(u) = body else { return None };
    if u.tag("requestKind") != PERMISSION_KIND {
        return None;
    }
    let actions = u
        .fields
        .get("actions")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect())
        .unwrap_or_default();
    let b = u.fields.get("budget");
    Some(PermissionRequest {
        actions,
        budget: BudgetFields {
            agent_turns: num(b.and_then(|b| b.get("agentTurns")), 100),
            threads: num(b.and_then(|b| b.get("threadsSpawned")), 20),
            hours: num(b.and_then(|b| b.get("hours")), 12),
        },
    })
}

/// Build a permission request body (demo data and tests).
pub fn body(actions: &[&str], budget: BudgetFields) -> RequestBody {
    let mut fields = Map::new();
    fields.insert("requestKind".into(), json!(PERMISSION_KIND));
    fields.insert("actions".into(), json!(actions));
    fields.insert(
        "budget".into(),
        json!({ "agentTurns": budget.agent_turns, "threadsSpawned": budget.threads, "hours": budget.hours }),
    );
    RequestBody::Unknown(UnknownVariant { fields })
}

/// The answer to send.
pub fn response(decision: PermissionDecision, budget: BudgetFields) -> RequestResponse {
    let word = match decision {
        PermissionDecision::Allow => "allow",
        PermissionDecision::Deny => "deny",
        PermissionDecision::AlwaysAllow => "alwaysAllow",
    };
    let mut fields = Map::new();
    fields.insert("requestKind".into(), json!(PERMISSION_KIND));
    fields.insert("decision".into(), json!(word));
    fields.insert(
        "budget".into(),
        json!({ "agentTurns": budget.agent_turns, "threadsSpawned": budget.threads, "hours": budget.hours }),
    );
    RequestResponse::Unknown(UnknownVariant { fields })
}

/// "Allowed", "Denied", … for a permission answer.
pub fn summary(answer: &RequestResponse) -> Option<String> {
    let RequestResponse::Unknown(u) = answer else { return None };
    (u.tag("requestKind") == PERMISSION_KIND).then(|| {
        match u.fields.get("decision").and_then(Value::as_str) {
            Some("deny") => "Denied".to_owned(),
            Some("alwaysAllow") => "Always allowed in this chat".to_owned(),
            _ => "Allowed".to_owned(),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_the_open_union() {
        let b = BudgetFields { agent_turns: 100, threads: 20, hours: 12 };
        let body = body(&["Start 3 threads"], b);
        let wire = serde_json::to_value(&body).unwrap();
        let back: RequestBody = serde_json::from_value(wire).unwrap();
        assert_eq!(parse(&back), Some(PermissionRequest { actions: vec!["Start 3 threads".into()], budget: b }));
        let r = response(PermissionDecision::Deny, b);
        assert_eq!(summary(&r).as_deref(), Some("Denied"));
    }
}
