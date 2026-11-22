//! Rate limits across targets (D161): the last report of every provider instance on every
//! target, grouped by account. The same account used on two targets (a Claude sign-in on WSL
//! and on devbox) is one entry, with the most recent report's windows; a target that went
//! offline keeps its last report, and an account no connected target reports is `stale`.
//!
//! Kept in the cache's `ui_state` under [`STORE_KEY`] so the last values survive the hub
//! restarting (the cache is disposable: losing it only loses offline accounts until their
//! targets connect again).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use yaac_protocol::attach::{AccountRateLimits, RateLimitsResult};
use yaac_protocol::{InstanceId, InstanceRateLimits, TargetId};

/// The `ui_state` key the book is kept under.
pub(crate) const STORE_KEY: &str = "hub.rateLimits";

/// One instance's last report.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Kept {
    pub(crate) target: TargetId,
    pub(crate) report: InstanceRateLimits,
    /// Order of arrival in this hub (breaks ties between reports made in the same second).
    #[serde(default)]
    pub(crate) seq: u64,
}

/// Every instance's last report.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RateLimitBook {
    entries: BTreeMap<String, Kept>,
    #[serde(skip)]
    seq: u64,
}

fn slot(target: &TargetId, instance: &InstanceId) -> String {
    format!("{target}\u{1f}{instance}")
}

impl RateLimitBook {
    /// The book as stored (unreadable or absent: empty).
    pub(crate) fn from_stored(value: Option<serde_json::Value>) -> Self {
        let mut book: Self = value.and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
        book.seq = book.entries.values().map(|k| k.seq).max().unwrap_or(0);
        book
    }

    /// What to store.
    pub(crate) fn to_stored(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }

    /// One instance's new report. Returns whether anything changed.
    pub(crate) fn update(&mut self, target: &TargetId, report: InstanceRateLimits) -> bool {
        let key = slot(target, &report.instance);
        if self.entries.get(&key).is_some_and(|k| k.report == report) {
            return false;
        }
        self.seq += 1;
        self.entries.insert(key, Kept { target: target.clone(), report, seq: self.seq });
        true
    }

    /// A target's full answer: replaces every report of that target (an instance it no longer
    /// lists is gone). Returns whether anything changed.
    pub(crate) fn replace_target(&mut self, target: &TargetId, reports: Vec<InstanceRateLimits>) -> bool {
        let listed: BTreeSet<String> = reports.iter().map(|r| slot(target, &r.instance)).collect();
        let before = self.entries.len();
        self.entries.retain(|k, kept| &kept.target != target || listed.contains(k));
        let mut changed = self.entries.len() != before;
        for r in reports {
            changed |= self.update(target, r);
        }
        changed
    }

    /// Whether a target reported anything.
    pub(crate) fn has_target(&self, target: &TargetId) -> bool {
        self.entries.values().any(|k| &k.target == target)
    }

    /// Keeps only these targets' reports (at start: targets removed meanwhile are dropped).
    pub(crate) fn only(mut self, targets: &[TargetId]) -> Self {
        self.entries.retain(|_, kept| targets.contains(&kept.target));
        self
    }

    /// Forgets a removed target. Returns whether anything changed.
    pub(crate) fn remove_target(&mut self, target: &TargetId) -> bool {
        let before = self.entries.len();
        self.entries.retain(|_, kept| &kept.target != target);
        self.entries.len() != before
    }

    /// One entry per account that reports at least one window, ordered by kind and key.
    /// `online`: whether a target is connected now.
    pub(crate) fn accounts(&self, online: impl Fn(&TargetId) -> bool) -> RateLimitsResult {
        let mut groups: BTreeMap<(String, String), Vec<&Kept>> = BTreeMap::new();
        for kept in self.entries.values() {
            let Some(account) = &kept.report.account else { continue };
            groups.entry((kept.report.kind.to_string(), account.key.clone())).or_default().push(kept);
        }
        let mut accounts = Vec::new();
        for ((_, key), mut group) in groups {
            // The most recent report wins (by when the harness made it, then by arrival).
            group.sort_by(|a, b| (&a.report.at, a.seq).cmp(&(&b.report.at, b.seq)));
            let newest = group.last().expect("a group has a member");
            if newest.report.windows.is_empty() {
                continue;
            }
            let account = newest.report.account.as_ref().expect("grouped by account");
            let targets: BTreeSet<TargetId> = group.iter().map(|k| k.target.clone()).collect();
            let stale = !targets.iter().any(&online);
            accounts.push(AccountRateLimits {
                key,
                kind: newest.report.kind.clone(),
                label: account.label.clone(),
                email: account.email.clone(),
                windows: newest.report.windows.clone(),
                at: newest.report.at.clone(),
                targets: targets.into_iter().collect(),
                stale,
            });
        }
        RateLimitsResult { accounts }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use yaac_protocol::{ProviderKind, RateLimitAccount, RateLimitWindow, Timestamp};

    fn report(instance: &str, kind: &str, key: &str, used: u32, at: &str) -> InstanceRateLimits {
        InstanceRateLimits {
            instance: InstanceId::new(instance),
            kind: ProviderKind::new(kind),
            account: Some(RateLimitAccount { key: key.into(), label: Some("Plan".into()), email: None }),
            windows: vec![RateLimitWindow {
                id: "weekly".into(),
                label: "weekly".into(),
                scope: None,
                used_percent: used,
                resets_at: Some(Timestamp::new("2026-11-25T15:00:00Z")),
                window_minutes: Some(10080),
            }],
            at: Timestamp::new(at),
        }
    }

    fn t(s: &str) -> TargetId {
        TargetId::new(s)
    }

    #[test]
    fn the_same_account_on_two_targets_is_one_entry_with_the_newest_values() {
        let mut b = RateLimitBook::default();
        assert!(b.update(&t("wsl"), report("pi_claude", "claude", "claude:aaaa", 40, "2026-11-22T13:00:00Z")));
        assert!(b.update(&t("devbox"), report("pi_claude", "claude", "claude:aaaa", 55, "2026-11-22T13:05:00Z")));
        let r = b.accounts(|_| true);
        assert_eq!(r.accounts.len(), 1);
        let a = &r.accounts[0];
        assert_eq!(a.windows[0].used_percent, 55);
        assert_eq!(a.targets, vec![t("devbox"), t("wsl")]);
        assert!(!a.stale);
        // An older report arriving later does not win.
        b.update(&t("wsl"), report("pi_claude", "claude", "claude:aaaa", 41, "2026-11-22T13:01:00Z"));
        assert_eq!(b.accounts(|_| true).accounts[0].windows[0].used_percent, 55);
        // The same report again changes nothing.
        assert!(!b.update(&t("devbox"), report("pi_claude", "claude", "claude:aaaa", 55, "2026-11-22T13:05:00Z")));
    }

    #[test]
    fn different_accounts_and_kinds_stay_apart() {
        let mut b = RateLimitBook::default();
        b.update(&t("wsl"), report("pi_claude", "claude", "claude:aaaa", 40, "2026-11-22T13:00:00Z"));
        b.update(&t("devbox"), report("pi_claude", "claude", "claude:bbbb", 70, "2026-11-22T13:00:00Z"));
        b.update(&t("wsl"), report("pi_codex", "codex", "codex:aaaa", 55, "2026-11-22T13:00:00Z"));
        let r = b.accounts(|_| true);
        let keys: Vec<(&str, &str)> = r.accounts.iter().map(|a| (a.kind.as_str(), a.key.as_str())).collect();
        assert_eq!(keys, vec![("claude", "claude:aaaa"), ("claude", "claude:bbbb"), ("codex", "codex:aaaa")]);
    }

    #[test]
    fn an_offline_target_keeps_its_last_values_marked_stale() {
        let mut b = RateLimitBook::default();
        b.update(&t("wsl"), report("pi_claude", "claude", "claude:aaaa", 40, "2026-11-22T13:00:00Z"));
        b.update(&t("devbox"), report("pi_codex", "codex", "codex:aaaa", 55, "2026-11-22T13:00:00Z"));
        let r = b.accounts(|id| id.as_str() == "wsl");
        assert!(!r.accounts[0].stale);
        assert!(r.accounts[1].stale);
        assert_eq!(r.accounts[1].windows[0].used_percent, 55);
        // Shared with a connected target: not stale.
        b.update(&t("wsl"), report("pi_codex", "codex", "codex:aaaa", 50, "2026-11-22T12:00:00Z"));
        let r = b.accounts(|id| id.as_str() == "wsl");
        assert!(!r.accounts[1].stale);
        assert_eq!(r.accounts[1].windows[0].used_percent, 55, "the newest report still wins");
        // Survives a restart through the store; a removed target is forgotten.
        let mut again = RateLimitBook::from_stored(Some(b.to_stored()));
        assert_eq!(again.accounts(|_| false), b.accounts(|_| false));
        assert!(again.remove_target(&t("devbox")));
        assert_eq!(again.accounts(|_| true).accounts[1].windows[0].used_percent, 50);
    }

    #[test]
    fn a_full_answer_drops_instances_no_longer_listed_and_accounts_without_windows_are_hidden() {
        let mut b = RateLimitBook::default();
        b.update(&t("wsl"), report("pi_a", "claude", "claude:aaaa", 40, "2026-11-22T13:00:00Z"));
        b.update(&t("wsl"), report("pi_b", "codex", "codex:aaaa", 40, "2026-11-22T13:00:00Z"));
        let mut empty = report("pi_b", "codex", "codex:aaaa", 0, "2026-11-22T13:10:00Z");
        empty.windows.clear();
        assert!(b.replace_target(&t("wsl"), vec![empty]));
        assert!(b.accounts(|_| true).accounts.is_empty());
        let mut none = report("pi_c", "codex", "x", 1, "2026-11-22T13:10:00Z");
        none.account = None;
        b.update(&t("wsl"), none);
        assert!(b.accounts(|_| true).accounts.is_empty());
    }
}
