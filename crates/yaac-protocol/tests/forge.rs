//! The pull-request methods (D119–D121): exact wire shapes, and open enums that keep values a
//! newer daemon may add.

use serde_json::json;
use yaac_protocol::methods::{self, ForgePrCreate, Method, names};
use yaac_protocol::{ChecksState, ForgeCapability, ForgeKind, ForgePrsResult, PrState};

#[test]
fn forge_prs_result_round_trips_with_unknown_values() {
    let wire = json!({
        "root": "/home/u/yaac",
        "branch": "feature/pr-badge",
        "access": {
            "repo": { "kind": "gitea", "host": "github.com", "slug": "lelemm/yaac", "webUrl": "https://github.com/lelemm/yaac", "fromFork": false },
            "capabilities": ["prList", "prCreate", "prTeleport"]
        },
        "prs": [{ "number": 33342, "title": "Badge", "state": "open", "draft": true, "url": "https://github.com/lelemm/yaac/pull/33342",
                  "head": "feature/pr-badge", "base": "main", "checks": "failing" },
                { "number": 7, "title": "Old", "state": "queued", "draft": false, "url": "u", "head": "h", "base": "b" }],
        "at": "2026-11-20T10:00:00Z"
    });
    let r: ForgePrsResult = serde_json::from_value(wire.clone()).unwrap();
    let access = &r.access;
    assert_eq!(access.repo.as_ref().unwrap().kind, ForgeKind::Unknown("gitea".into()));
    assert!(access.capabilities.contains(&ForgeCapability::PrCreate));
    assert_eq!(access.capabilities[2], ForgeCapability::Unknown("prTeleport".into()));
    assert_eq!(r.prs[0].checks, Some(ChecksState::Failing));
    assert_eq!(r.prs[1].state, PrState::Unknown("queued".into()));
    assert_eq!(serde_json::to_value(&r).unwrap(), wire);
}

#[test]
fn create_is_state_changing_and_the_rest_are_not() {
    const { assert!(ForgePrCreate::INFO.state_changing) };
    for name in [names::FORGE_PRS, names::FORGE_PREPARE, names::FORGE_PR_GET] {
        assert!(!methods::request_info(name).unwrap().state_changing, "{name}");
    }
    assert!(methods::notification_info(names::FORGE_PRS_CHANGED).is_some());
}
