//! The permission prompt of screen 25: "This agent wants to work with other agents", with the
//! orchestration's budget editable in the same decision. It travels as the protocol's
//! `meshPermission` request and is answered with a `meshPermission` answer; this module turns
//! them into the card's terms.

use yaac_protocol::{
    MeshActionKind, MeshBudget, MeshPermissionAction, MeshPermissionAnswer, MeshPermissionDecision, MeshPermissionRequest,
    RequestBody, RequestResponse,
};

/// The budget fields of the prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BudgetFields {
    /// Agent-started turns.
    pub agent_turns: u32,
    /// Hours without the user; 0 means no time limit (P351).
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

fn hours(secs: u64) -> u32 {
    if secs >= MeshBudget::NO_TIME_LIMIT {
        return 0;
    }
    u32::try_from(secs.div_ceil(3600)).unwrap_or(u32::MAX)
}

fn budget_of(b: BudgetFields) -> MeshBudget {
    // Older daemons still read the legacy allowance. Send its maximum wire value.
    let wall_clock_secs = if b.hours == 0 { MeshBudget::NO_TIME_LIMIT } else { u64::from(b.hours) * 3600 };
    MeshBudget { agent_turns: b.agent_turns, threads_spawned: u32::MAX, wall_clock_secs }
}

/// Read a permission request from a request body.
pub fn parse(body: &RequestBody) -> Option<PermissionRequest> {
    let RequestBody::MeshPermission(r) = body else { return None };
    Some(PermissionRequest {
        actions: r.actions.iter().map(|a| a.summary.clone()).collect(),
        budget: BudgetFields { agent_turns: r.budget.agent_turns, hours: hours(r.budget.wall_clock_secs) },
    })
}

/// Build a permission request body (demo data and tests).
pub fn body(actions: &[&str], budget: BudgetFields) -> RequestBody {
    RequestBody::MeshPermission(MeshPermissionRequest {
        actions: actions
            .iter()
            .map(|a| MeshPermissionAction {
                action: if a.starts_with("Message") { MeshActionKind::Message } else { MeshActionKind::StartThreads },
                summary: (*a).to_owned(),
                threads: Vec::new(),
                recipients: Vec::new(),
            })
            .collect(),
        budget: budget_of(budget),
        root: None,
    })
}

/// The answer to send.
pub fn response(decision: PermissionDecision, budget: BudgetFields) -> RequestResponse {
    RequestResponse::MeshPermission(MeshPermissionAnswer {
        decision: if decision == PermissionDecision::Deny { MeshPermissionDecision::Deny } else { MeshPermissionDecision::Allow },
        budget: (decision != PermissionDecision::Deny).then(|| budget_of(budget)),
        always_allow: decision == PermissionDecision::AlwaysAllow,
    })
}

/// "Allowed", "Denied", … for a permission answer.
pub fn summary(answer: &RequestResponse) -> Option<String> {
    let RequestResponse::MeshPermission(a) = answer else { return None };
    Some(match (&a.decision, a.always_allow) {
        (MeshPermissionDecision::Deny, _) => "Denied".to_owned(),
        (_, true) => "Always allowed in this chat".to_owned(),
        _ => "Allowed".to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_the_open_union() {
        let b = BudgetFields { agent_turns: 100, hours: 12 };
        let body = body(&["Start 3 threads"], b);
        let wire = serde_json::to_value(&body).unwrap();
        let back: RequestBody = serde_json::from_value(wire).unwrap();
        assert_eq!(parse(&back), Some(PermissionRequest { actions: vec!["Start 3 threads".into()], budget: b }));
        let r = response(PermissionDecision::Deny, b);
        assert_eq!(summary(&r).as_deref(), Some("Denied"));
    }

    /// P351: hours 0 in the prompt is no time limit, both ways.
    #[test]
    fn zero_hours_is_no_time_limit() {
        let none = BudgetFields { agent_turns: 100, hours: 0 };
        let back: RequestBody = serde_json::from_value(serde_json::to_value(body(&["Start a thread"], none)).unwrap()).unwrap();
        assert_eq!(parse(&back).unwrap().budget, none);
        assert_eq!(budget_of(none).wall_clock_secs, MeshBudget::NO_TIME_LIMIT);
        assert_eq!(budget_of(BudgetFields { agent_turns: 100, hours: 12 }).wall_clock_secs, 12 * 3600);
        assert_eq!(hours(MeshBudget::NO_TIME_LIMIT), 0);
        assert_eq!(hours(MeshBudget::default().wall_clock_secs), 0, "the default budget has no time limit");
        assert_eq!(hours(5400), 2, "a part hour rounds up");
    }
}
