//! The demo hub's rate limits (D161): one Claude account used on two targets, its five-hour
//! window used up and its weekly one at 55%; one Codex account at 55% of its week. Plan names
//! and emails are made up.

use serde_json::{Value, json};

use super::fleet::Fleet;

/// `hub/rateLimits/get`'s answer. A fresh install (no threads yet) has none.
pub(super) fn accounts(f: &Fleet) -> Value {
    if f.threads.is_empty() {
        return json!({ "accounts": [] });
    }
    let day = 86_400;
    json!({ "accounts": [
        {
            "key": "claude:demo0001", "kind": "claude", "label": "Max", "email": "dev@example.com",
            "windows": [
                { "id": "session", "label": "5h", "usedPercent": 100, "resetsAt": f.ts(53 * 60 + 20), "windowMinutes": 300 },
                { "id": "weekly", "label": "weekly", "usedPercent": 55, "resetsAt": f.ts(2 * day + 5 * 3600), "windowMinutes": 10080 },
                { "id": "weekly:Fable", "label": "weekly", "scope": "Fable", "usedPercent": 31, "resetsAt": f.ts(2 * day + 5 * 3600), "windowMinutes": 10080 }
            ],
            "at": f.ts(-120), "targets": ["local", "devbox"], "stale": false
        },
        {
            "key": "codex:demo0001", "kind": "codex", "label": "Pro", "email": "dev@example.com",
            "windows": [
                { "id": "primary", "label": "weekly", "usedPercent": 55, "resetsAt": f.ts(4 * day + 7 * 3600), "windowMinutes": 10080 }
            ],
            "at": f.ts(-300), "targets": ["local", "devbox", "gpu-box"], "stale": false
        }
    ]})
}
