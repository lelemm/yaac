//! The rate-limit panel (D161): what the bottom of the sidebar shows about each account's plan
//! limits — from `hub/rateLimits/get` and `hub/rateLimits/changed`, one entry per account the
//! hub has seen on any target. Everything here is a function of that list, the target names,
//! and the time passed in; nothing reads a clock.
//!
//! An account is one group: its provider's logo, then one bar per window — each window that
//! counts the whole account, and a scoped one only when it is near its limit — with
//! "55% · 2d" (used · time until it resets) beside it. The colour is the provider's (looked up
//! by the window from the kind, like the logos), the text the failed colour near the limit.

use yaac_protocol::attach::AccountRateLimits;
use yaac_protocol::{ProviderKind, RateLimitWindow, TargetId};

use crate::time::{self, Zone};

/// Segments per bar.
pub const SEGMENTS: usize = 5;
/// At or above this share used, a window is near its limit.
pub const NEAR: u32 = 90;
/// The window asks the hub for fresh values at most this often while it shows the panel.
pub const REFRESH_SECS: i64 = 5 * 60;

/// What the window knows: the hub's accounts, and when it last asked for fresh values.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Accounts {
    /// One per account, as the hub sends them.
    pub accounts: Vec<AccountRateLimits>,
    /// When the panel last asked for a refresh (seconds; set when it is first drawn).
    pub asked: Option<i64>,
}

/// How much of one segment is used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Segment {
    /// Used up.
    Full,
    /// Partly used (drawn dimmer).
    Partial,
    /// Not used.
    Empty,
}

/// One window's bar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bar {
    /// "5h", "weekly", or "weekly (Fable)" for a scoped one.
    pub label: String,
    /// Share used, 0–100 (0 once the window has reset).
    pub percent: u32,
    /// The five segments.
    pub segments: [Segment; SEGMENTS],
    /// "55% · 2d".
    pub text: String,
    /// At or above [`NEAR`].
    pub near: bool,
}

/// One account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    /// The account key.
    pub key: String,
    /// The provider kind: the window looks up the logo and colour from it.
    pub kind: ProviderKind,
    /// The bars, shortest window first.
    pub bars: Vec<Bar>,
    /// The bar the collapsed strip shows: the most used.
    pub tightest: usize,
    /// The tooltip: plan and email, each window exactly with its reset in local time, and
    /// which targets use the account.
    pub tooltip: String,
    /// No connected target reports this account.
    pub stale: bool,
}

/// The panel. Empty: hidden.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Panel {
    /// Ordered by provider kind, then plan, then key: stable while values change.
    pub groups: Vec<Group>,
}

/// The segments of a share used.
pub fn segments(percent: u32) -> [Segment; SEGMENTS] {
    const STEP: u32 = 20; // 100 / SEGMENTS
    std::array::from_fn(|i| {
        let lo = STEP * u32::try_from(i).unwrap_or(u32::MAX);
        if percent >= lo.saturating_add(STEP) {
            Segment::Full
        } else if percent > lo {
            Segment::Partial
        } else {
            Segment::Empty
        }
    })
}

/// Time until a reset: "53m", "4h", "2d" (whole units, rounded down; at least "1m").
pub fn until(secs: i64) -> String {
    let m = (secs / 60).max(1);
    if m < 60 {
        format!("{m}m")
    } else if m < 24 * 60 {
        format!("{}h", m / 60)
    } else {
        format!("{}d", m / (24 * 60))
    }
}

/// Seconds until [`until`]'s text for `secs` changes (it drops a unit one second after a
/// whole number of units is left).
fn until_changes_in(secs: i64) -> i64 {
    let unit = if secs < 3600 {
        60
    } else if secs < 86_400 {
        3600
    } else {
        86_400
    };
    secs.rem_euclid(unit) + 1
}

fn reset_secs(w: &RateLimitWindow) -> Option<i64> {
    w.resets_at.as_ref().and_then(|t| time::parse_rfc3339(t.as_str()))
}

/// A window's share used now: nothing once it has reset (until the harness reports again).
fn used_now(w: &RateLimitWindow, now: i64) -> u32 {
    if reset_secs(w).is_some_and(|r| r <= now) { 0 } else { w.used_percent.min(100) }
}

fn label(w: &RateLimitWindow) -> String {
    match &w.scope {
        Some(s) => format!("{} ({s})", w.label),
        None => w.label.clone(),
    }
}

fn bar(w: &RateLimitWindow, now: i64) -> Bar {
    let percent = used_now(w, now);
    let text = match reset_secs(w) {
        Some(r) if r > now => format!("{percent}% · {}", until(r - now)),
        _ => format!("{percent}%"),
    };
    Bar { label: label(w), percent, segments: segments(percent), text, near: percent >= NEAR }
}

/// Windows the panel draws: the account-wide ones, and scoped ones near their limit.
fn shown(a: &AccountRateLimits, now: i64) -> Vec<&RateLimitWindow> {
    let mut ws: Vec<&RateLimitWindow> = a.windows.iter().filter(|w| w.scope.is_none() || used_now(w, now) >= NEAR).collect();
    // Shortest window first (5h above weekly); a scoped window after its account-wide one.
    ws.sort_by_key(|w| (w.window_minutes.unwrap_or(u32::MAX), w.scope.is_some()));
    ws
}

fn tooltip(a: &AccountRateLimits, names: &dyn Fn(&TargetId) -> String, now: i64, zone: Zone) -> String {
    let mut lines = Vec::new();
    let who = [a.label.clone(), a.email.clone()].into_iter().flatten().collect::<Vec<_>>().join(" · ");
    if !who.is_empty() {
        lines.push(who);
    }
    for w in &a.windows {
        let used = used_now(w, now);
        let when = match reset_secs(w) {
            Some(r) if r > now => format!("resets {}", time::moment_in(r, zone)),
            Some(r) => format!("reset {}", time::moment_in(r, zone)),
            None => "reset time not reported".to_owned(),
        };
        lines.push(format!("{}: {used}% used · {when}", label(w)));
    }
    let targets: Vec<String> = a.targets.iter().map(names).collect();
    if !targets.is_empty() {
        lines.push(format!("Used on {}", targets.join(", ")));
    }
    if a.stale {
        let at = time::parse_rfc3339(a.at.as_str()).map_or_else(String::new, |s| format!(", reported {}", time::moment_in(s, zone)));
        lines.push(format!("Not connected now: the last values known{at}"));
    }
    lines.join("\n")
}

/// The panel for these accounts at `now` (seconds). `names`: a target's name.
pub fn panel(accounts: &[AccountRateLimits], names: &dyn Fn(&TargetId) -> String, now: i64, zone: Zone) -> Panel {
    let mut ordered: Vec<&AccountRateLimits> = accounts.iter().collect();
    ordered.sort_by(|a, b| (a.kind.as_str(), &a.label, &a.key).cmp(&(b.kind.as_str(), &b.label, &b.key)));
    let groups = ordered
        .into_iter()
        .filter_map(|a| {
            let bars: Vec<Bar> = shown(a, now).into_iter().map(|w| bar(w, now)).collect();
            // The most used; the first of equals.
            let tightest = bars.iter().enumerate().max_by_key(|(i, b)| (b.percent, std::cmp::Reverse(*i)))?.0;
            Some(Group { key: a.key.clone(), kind: a.kind.clone(), bars, tightest, tooltip: tooltip(a, names, now, zone), stale: a.stale })
        })
        .collect();
    Panel { groups }
}

/// Seconds until any text of the panel changes (at least a minute), or `None` when nothing
/// counts down. The window schedules one repaint for then — never a continuous one.
pub fn next_change(accounts: &[AccountRateLimits], now: i64) -> Option<i64> {
    accounts
        .iter()
        .flat_map(|a| shown(a, now))
        .filter_map(reset_secs)
        .filter(|r| *r > now)
        .map(|r| until_changes_in(r - now).min(r - now))
        .min()
        .map(|s| s.max(60))
}

#[cfg(test)]
mod tests {
    use super::*;
    use yaac_protocol::Timestamp;

    const NOW: i64 = 1_795_352_400; // 2026-11-22T13:00:00Z

    fn ts(secs: i64) -> Timestamp {
        let d = time::local(secs, Zone::UTC);
        Timestamp::new(format!("{:04}-{:02}-{:02}T{:02}:{:02}:00Z", d.year, d.month, d.day, d.hour, d.minute))
    }

    fn win(id: &str, label: &str, scope: Option<&str>, used: u32, resets_in: i64, minutes: u32) -> RateLimitWindow {
        RateLimitWindow {
            id: id.into(),
            label: label.into(),
            scope: scope.map(Into::into),
            used_percent: used,
            resets_at: Some(ts(NOW + resets_in)),
            window_minutes: Some(minutes),
        }
    }

    fn account(kind: &str, key: &str, label: &str, windows: Vec<RateLimitWindow>, stale: bool) -> AccountRateLimits {
        AccountRateLimits {
            key: key.into(),
            kind: ProviderKind::new(kind),
            label: Some(label.into()),
            email: Some("someone@example.com".into()),
            windows,
            at: ts(NOW),
            targets: vec![TargetId::new("t_wsl"), TargetId::new("t_devbox")],
            stale,
        }
    }

    fn names(t: &TargetId) -> String {
        t.as_str().trim_start_matches("t_").to_owned()
    }

    fn demo() -> Vec<AccountRateLimits> {
        vec![
            account("codex", "codex:1", "Pro", vec![win("primary", "weekly", None, 55, 4 * 86_400 + 600, 10080)], false),
            account(
                "claude",
                "claude:1",
                "Claude Ultra",
                vec![
                    win("weekly", "weekly", None, 55, 2 * 86_400 + 3600, 10080),
                    win("session", "5h", None, 100, 53 * 60, 300),
                    win("weekly:Fable", "weekly", Some("Fable"), 31, 2 * 86_400, 10080),
                ],
                false,
            ),
        ]
    }

    #[test]
    fn segments_fill_in_fifths_with_one_partial() {
        use Segment::*;
        assert_eq!(segments(0), [Empty; 5]);
        assert_eq!(segments(55), [Full, Full, Partial, Empty, Empty]);
        assert_eq!(segments(60), [Full, Full, Full, Empty, Empty]);
        assert_eq!(segments(1), [Partial, Empty, Empty, Empty, Empty]);
        assert_eq!(segments(100), [Full; 5]);
    }

    #[test]
    fn time_until_reset_is_minutes_hours_or_days() {
        assert_eq!(until(53 * 60 + 30), "53m");
        assert_eq!(until(10), "1m");
        assert_eq!(until(4 * 3600 + 59 * 60), "4h");
        assert_eq!(until(2 * 86_400 + 3600), "2d");
        assert_eq!(until_changes_in(53 * 60 + 30), 31);
        assert_eq!(until_changes_in(4 * 3600 + 120), 121);
        assert_eq!(until_changes_in(2 * 86_400), 1, "2d becomes 1d a second later");
    }

    #[test]
    fn the_panel_orders_by_provider_and_shows_the_account_wide_windows_shortest_first() {
        let p = panel(&demo(), &names, NOW, Zone::UTC);
        let kinds: Vec<&str> = p.groups.iter().map(|g| g.kind.as_str()).collect();
        assert_eq!(kinds, vec!["claude", "codex"]);
        let claude = &p.groups[0];
        let texts: Vec<(&str, &str, bool)> = claude.bars.iter().map(|b| (b.label.as_str(), b.text.as_str(), b.near)).collect();
        assert_eq!(texts, vec![("5h", "100% · 53m", true), ("weekly", "55% · 2d", false)], "the scoped window under 90% is only in the tooltip");
        assert_eq!(claude.tightest, 0);
        assert_eq!(p.groups[1].bars[0].text, "55% · 4d");
        assert!(claude.tooltip.contains("Claude Ultra · someone@example.com"));
        assert!(claude.tooltip.contains("5h: 100% used · resets Sun 22 Nov, 13:53"), "{}", claude.tooltip);
        assert!(claude.tooltip.contains("weekly (Fable): 31% used"));
        assert!(claude.tooltip.contains("Used on wsl, devbox"));
        assert!(!claude.tooltip.contains("Not connected"));
    }

    #[test]
    fn a_scoped_window_near_its_limit_gets_a_bar_and_a_reset_window_reads_zero() {
        let mut a = demo();
        a[1].windows[2].used_percent = 95;
        a[1].windows[1].resets_at = Some(ts(NOW - 60));
        let p = panel(&a, &names, NOW, Zone::UTC);
        let texts: Vec<&str> = p.groups[0].bars.iter().map(|b| b.text.as_str()).collect();
        assert_eq!(texts, vec!["0%", "55% · 2d", "95% · 2d"]);
        assert_eq!(p.groups[0].bars[2].label, "weekly (Fable)");
        assert_eq!(p.groups[0].tightest, 2);
    }

    #[test]
    fn stale_accounts_say_so_and_an_account_without_windows_is_hidden() {
        let mut a = demo();
        a[0].stale = true;
        a[1].windows.clear();
        let p = panel(&a, &names, NOW, Zone::UTC);
        assert_eq!(p.groups.len(), 1);
        assert!(p.groups[0].stale);
        assert!(p.groups[0].tooltip.contains("Not connected now"));
        assert!(panel(&[], &names, NOW, Zone::UTC).groups.is_empty());
    }

    #[test]
    fn the_countdown_wakes_at_most_once_a_minute() {
        assert_eq!(next_change(&demo(), NOW), Some(60), "53m changes within the minute");
        let codex_only = vec![demo().remove(0)];
        assert_eq!(next_change(&codex_only, NOW), Some(601), "4d 10m left: 3d once less than 4d is left");
        assert_eq!(next_change(&[], NOW), None);
    }
}
