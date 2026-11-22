//! Rate limits (D161): exact wire shapes of `provider/rateLimits`, its notification, and the
//! hub's `hub/rateLimits/*`.

use serde_json::json;
use yaac_protocol::attach::{self, RateLimitsGetParams, RateLimitsResult};
use yaac_protocol::methods::{self, names};
use yaac_protocol::{InstanceRateLimits, ProviderRateLimitsParams, ProviderRateLimitsResult};

#[test]
fn provider_rate_limits_round_trip() {
    let wire = json!({
        "instances": [{
            "instance": "pi_claude",
            "kind": "claude",
            "account": { "key": "claude:0123456789abcdef", "label": "Claude Ultra", "email": "someone@example.com" },
            "windows": [
                { "id": "session", "label": "5h", "usedPercent": 100, "resetsAt": "2026-11-22T14:00:00Z", "windowMinutes": 300 },
                { "id": "weekly:Fable", "label": "weekly", "scope": "Fable", "usedPercent": 55 }
            ],
            "at": "2026-11-22T13:07:00Z"
        }, {
            "instance": "pi_codex", "kind": "codex", "windows": [], "at": "2026-11-22T13:07:00Z"
        }]
    });
    let r: ProviderRateLimitsResult = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(r.instances[0].windows[1].scope.as_deref(), Some("Fable"));
    assert!(r.instances[1].account.is_none());
    assert_eq!(serde_json::to_value(&r).unwrap(), wire);
    // The notification carries one instance; missing windows decode as none.
    let one: InstanceRateLimits = serde_json::from_value(json!({ "instance": "pi", "kind": "codex", "at": "2026-11-22T13:07:00Z" })).unwrap();
    assert!(one.windows.is_empty());
    // `refresh` defaults to false.
    assert!(!serde_json::from_value::<ProviderRateLimitsParams>(json!({})).unwrap().refresh);
}

#[test]
fn rate_limit_methods_are_reads() {
    assert!(!methods::request_info(names::PROVIDER_RATE_LIMITS).unwrap().state_changing);
    assert!(methods::notification_info(names::PROVIDER_RATE_LIMITS_UPDATED).is_some());
}

#[test]
fn hub_rate_limits_round_trip() {
    let wire = json!({
        "accounts": [{
            "key": "codex:feedface", "kind": "codex", "label": "Pro",
            "windows": [{ "id": "primary", "label": "weekly", "usedPercent": 55, "resetsAt": "2026-11-26T10:00:00Z", "windowMinutes": 10080 }],
            "at": "2026-11-22T13:07:00Z", "targets": ["t_wsl", "t_devbox"], "stale": true
        }]
    });
    let r: RateLimitsResult = serde_json::from_value(wire.clone()).unwrap();
    assert!(r.accounts[0].stale);
    assert_eq!(serde_json::to_value(&r).unwrap(), wire);
    assert!(!serde_json::from_value::<RateLimitsGetParams>(json!({})).unwrap().refresh);
    assert_eq!(attach::names::RATE_LIMITS_GET, "hub/rateLimits/get");
}
