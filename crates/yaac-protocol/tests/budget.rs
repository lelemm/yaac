//! The orchestration budget (P351, P352): exact wire shapes of `mesh/budget`.

use serde_json::json;
use yaac_protocol::attach::DaemonSummary;
use yaac_protocol::{MeshBudget, MeshBudgetAction, MeshBudgetParams, MeshBudgetResult};

#[test]
fn the_default_budget_has_no_time_limit() {
    let b = MeshBudget::default();
    assert_eq!((b.agent_turns, b.wall_clock_secs), (100, u64::from(u32::MAX)));
    assert!(!b.time_limited());
    assert!(MeshBudget { wall_clock_secs: 12 * 3600, ..b }.time_limited());
    // The value every build reads as a plain number.
    assert_eq!(serde_json::to_value(b).unwrap()["wallClockSecs"], json!(4_294_967_295_u64));
}

#[test]
fn set_time_limit_carries_a_limit_or_null() {
    let set: MeshBudgetParams = serde_json::from_value(json!({"opId":"op-1","root":"t_a/th","action":"setTimeLimit","wallClockSecs":57600})).unwrap();
    assert_eq!(set.action, MeshBudgetAction::SetTimeLimit);
    assert_eq!(set.wall_clock_secs, Some(Some(57_600)));
    let remove: MeshBudgetParams = serde_json::from_value(json!({"opId":"op-2","root":"t_a/th","action":"setTimeLimit","wallClockSecs":null})).unwrap();
    assert_eq!(remove.wall_clock_secs, Some(None));
    assert_eq!(serde_json::to_value(&remove).unwrap()["wallClockSecs"], json!(null), "remove is an explicit null");
    let status: MeshBudgetParams = serde_json::from_value(json!({"opId":"op-3","root":"t_a/th","action":"status"})).unwrap();
    assert_eq!(status.wall_clock_secs, None);
    assert!(serde_json::to_value(&status).unwrap().get("wallClockSecs").is_none(), "absent for other actions");
    // An older build reads an action it does not know as Unknown, never as Continue.
    assert_eq!(MeshBudgetAction::from_wire("setTimeLimit"), MeshBudgetAction::SetTimeLimit);
    assert_eq!(MeshBudgetAction::from_wire("later"), MeshBudgetAction::Unknown("later".into()));
}

#[test]
fn a_budget_result_says_when_a_person_last_acted() {
    let wire = json!({
        "root": "t_a/th",
        "allowance": {"agentTurns":100,"threadsSpawned":20,"wallClockSecs":43200},
        "used": {"agentTurns":41,"threadsSpawned":3,"wallClockSecs":50400},
        "exhausted": true, "held": 2, "actedAt": "2026-11-20T00:00:00Z",
    });
    let r: MeshBudgetResult = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(r.acted_at.as_ref().map(|t| t.as_str()), Some("2026-11-20T00:00:00Z"));
    assert_eq!(serde_json::to_value(&r).unwrap(), wire);
    let mut older = wire;
    older.as_object_mut().unwrap().remove("actedAt");
    let r: MeshBudgetResult = serde_json::from_value(older).unwrap();
    assert_eq!(r.acted_at, None);
}

#[test]
fn an_older_hub_summary_cannot_set_the_time_limit() {
    let legacy: DaemonSummary = serde_json::from_value(json!({"build":"old","older":true,"schemaHashMatches":true})).unwrap();
    assert!(!legacy.budget_time_limit);
}
