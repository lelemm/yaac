//! Which of YAAC's components need updating, checked as YAAC starts (D135).
//!
//! Every artefact of one build carries the same build id after its own version
//! (`<version>+<id>`, `scripts/build-id.sh`). The window knows its own build; `hub/hello` says
//! whether the running hub is the same build, older (the window replaces it), or newer; and the
//! hub, on connecting to each target, compares that target's `yaacd` with its own build and
//! reports an older one ([`yaac_protocol::attach::DaemonSummary::older`]). This module only
//! gathers those answers into one list: nothing here asks the network itself.

use yaac_protocol::attach::DaemonUpdatePhase;
use yaac_protocol::{TargetId, ThreadState};

use crate::state::{AppState, HelloOutcome};
use crate::update::{Offer, ReleaseCheck, offer};

/// Where a component stands against this window's build.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ComponentStatus {
    /// The same build as this window (or not older).
    Current,
    /// Older: it needs updating. The text says what happens.
    Outdated(String),
    /// Not known yet, or not reachable; the text says why.
    Unknown(String),
}

/// One component: the window, the hub, or one target's `yaacd`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Component {
    /// "Window", "Hub", "yaacd on devbox".
    pub name: String,
    /// Its build (`<version>+<id>`), when known.
    pub build: Option<String>,
    /// Against this window's build.
    pub status: ComponentStatus,
}

/// Every component, from what the window and the hub know now. `window_build` is this
/// window's own build.
pub fn components(state: &AppState, window_build: &str) -> Vec<Component> {
    let mut out = vec![Component { name: "Window".into(), build: Some(window_build.to_owned()), status: ComponentStatus::Current }];
    let hub = match &state.hub.outcome {
        HelloOutcome::Attached => Component { name: "Hub".into(), build: state.hub.build.clone(), status: ComponentStatus::Current },
        HelloOutcome::HubOutdated { hub_build } => Component {
            name: "Hub".into(),
            build: Some(hub_build.clone()),
            status: ComponentStatus::Outdated("older than this window; it is replaced by this build's hub".into()),
        },
        HelloOutcome::WindowOutdated { message } => {
            // The hub is the newer one: it is this window that is out of date.
            out[0].status = ComponentStatus::Outdated(message.clone());
            Component { name: "Hub".into(), build: state.hub.build.clone(), status: ComponentStatus::Current }
        }
        HelloOutcome::Pending => Component { name: "Hub".into(), build: None, status: ComponentStatus::Unknown("not reached yet".into()) },
    };
    out.push(hub);
    for t in state.targets.iter().filter(|t| t.entry.enabled) {
        let name = format!("yaacd on {}", t.entry.name);
        let update = t.entry.state.update.as_ref();
        let c = match (&t.entry.state.daemon, update) {
            // An update under way, or one that did not take: shown even while reconnecting.
            (_, Some(u)) if u.phase == DaemonUpdatePhase::Updating => {
                Component { name, build: Some(u.from.clone()), status: ComponentStatus::Outdated(format!("updating to {}…", u.to)) }
            }
            (_, Some(u)) if u.phase == DaemonUpdatePhase::Stuck => Component {
                name,
                build: Some(u.from.clone()),
                status: ComponentStatus::Outdated(u.detail.clone().unwrap_or_else(|| format!("not updated to {}", u.to))),
            },
            (Some(d), _) if d.older => Component {
                name,
                build: Some(d.build.clone()),
                status: ComponentStatus::Outdated(
                    if d.can_update { "older build than this YAAC; it is updated when idle, or with Update now" } else { "older build than this YAAC" }.into(),
                ),
            },
            (Some(d), _) => Component { name, build: Some(d.build.clone()), status: ComponentStatus::Current },
            (None, _) if t.connected() => Component { name, build: None, status: ComponentStatus::Unknown("connected; its build is not reported yet".into()) },
            (None, _) => Component { name, build: None, status: ComponentStatus::Unknown(format!("not connected ({})", t.entry.state.status.as_str())) },
        };
        out.push(c);
    }
    out
}

/// The "YAAC (latest release)" row (D136): the latest release found as the window started,
/// and whether this build is behind it. Not counted by [`outdated`]: the title row offers the
/// update itself.
pub fn release(check: &ReleaseCheck, window_build: &str) -> Component {
    let name = "YAAC (latest release)".to_owned();
    match check {
        ReleaseCheck::Off(why) => Component { name, build: None, status: ComponentStatus::Unknown(why.clone()) },
        ReleaseCheck::Checking => Component { name, build: None, status: ComponentStatus::Unknown("checking…".into()) },
        ReleaseCheck::Unavailable(why) => Component { name, build: None, status: ComponentStatus::Unknown(why.clone()) },
        ReleaseCheck::Latest { version, build } => {
            let label = format!("{version}+{build}");
            let status = match offer(window_build, *build) {
                Offer::Newer => ComponentStatus::Outdated("this YAAC is behind; update from the title bar".into()),
                Offer::Same | Offer::Ahead => ComponentStatus::Current,
                Offer::Dev => ComponentStatus::Unknown("this is a development build; it is not updated from releases".into()),
            };
            Component { name, build: Some(label), status }
        }
    }
}

/// The turns **Update now** interrupts on a target: its threads with a turn in progress
/// (running, waiting on another agent, or waiting for the user).
pub fn turns_in_progress(state: &AppState, target: &TargetId) -> usize {
    state
        .threads
        .keys()
        .filter(|k| &k.target == target)
        .filter_map(|k| state.summary(k))
        .filter(|s| matches!(s.state, ThreadState::Running | ThreadState::Waiting | ThreadState::NeedsAttention))
        .count()
}

/// **Update now**'s confirmation: needed only when turns would be interrupted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateNowDialog {
    /// The target.
    pub target: TargetId,
    /// "Update yaacd on devbox now?"
    pub title: String,
    /// What it interrupts.
    pub body: String,
}

/// The confirmation for **Update now** on `target`, or `None` when nothing is in progress
/// there and it can go ahead at once (it is the user's explicit action).
pub fn update_now_dialog(state: &AppState, target: &TargetId) -> Option<UpdateNowDialog> {
    let n = turns_in_progress(state, target);
    if n == 0 {
        return None;
    }
    let name = state.target_name(target);
    let turns = if n == 1 { "1 turn is".to_owned() } else { format!("{n} turns are") };
    Some(UpdateNowDialog {
        target: target.clone(),
        title: format!("Update yaacd on {name} now?"),
        body: format!(
            "{turns} in progress on {name}. Updating now interrupts them; each is marked as interrupted by the update, and anything waiting for your answer in them is closed. To avoid that, let it update by itself once idle."
        ),
    })
}

/// How many components need updating.
pub fn outdated(state: &AppState, window_build: &str) -> usize {
    components(state, window_build).iter().filter(|c| matches!(c.status, ComponentStatus::Outdated(_))).count()
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use yaac_protocol::attach::TargetEntry;

    use super::{ComponentStatus as S, components, outdated, release};
    use crate::update::ReleaseCheck;
    use crate::state::{AppState, HelloOutcome};

    fn target(id: &str, enabled: bool, daemon: Option<(&str, bool)>) -> TargetEntry {
        let state = match daemon {
            Some((build, older)) => json!({ "status": "connected", "daemon": { "build": build, "older": older, "schemaHashMatches": true } }),
            None => json!({ "status": "offline" }),
        };
        serde_json::from_value(json!({ "id": id, "name": id, "transport": { "kind": "local" }, "enabled": enabled, "state": state })).unwrap()
    }

    #[test]
    fn the_window_the_hub_and_each_targets_daemon_are_listed_and_older_ones_counted() {
        let mut s = AppState::new();
        s.hub.outcome = HelloOutcome::Attached;
        s.hub.build = Some("0.1.0+42".into());
        s.set_targets(vec![
            target("local", true, Some(("0.1.0+42", false))),
            target("devbox", true, Some(("0.1.0+40", true))),
            target("gpu-box", true, None),
            target("off", false, None),
        ]);
        let c = components(&s, "0.1.0+42");
        let names: Vec<&str> = c.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["Window", "Hub", "yaacd on local", "yaacd on devbox", "yaacd on gpu-box"], "disabled targets are left out");
        assert_eq!(c[1].status, S::Current);
        assert_eq!(c[2].status, S::Current);
        assert!(matches!(c[3].status, S::Outdated(_)));
        assert_eq!(c[3].build.as_deref(), Some("0.1.0+40"));
        assert!(matches!(c[4].status, S::Unknown(_)));
        assert_eq!(outdated(&s, "0.1.0+42"), 1);
    }

    #[test]
    fn an_update_under_way_or_stuck_is_shown_even_while_reconnecting() {
        let mut s = AppState::new();
        s.hub.outcome = HelloOutcome::Attached;
        let mut updating = target("devbox", true, None);
        updating.state.update = Some(serde_json::from_value(json!({ "from": "0.1.0+40", "to": "0.1.0+42-ab", "phase": "updating" })).unwrap());
        let mut stuck = target("gpu", true, Some(("0.1.0+40", true)));
        stuck.state.update = Some(serde_json::from_value(json!({ "from": "0.1.0+40", "to": "0.1.0+42-ab", "phase": "stuck", "detail": "yaacd 0.1.0+40 started again" })).unwrap());
        s.set_targets(vec![updating, stuck]);
        let c = components(&s, "0.1.0+42");
        assert_eq!(c[2].status, S::Outdated("updating to 0.1.0+42-ab…".into()));
        assert_eq!(c[2].build.as_deref(), Some("0.1.0+40"));
        assert_eq!(c[3].status, S::Outdated("yaacd 0.1.0+40 started again".into()));
    }

    #[test]
    fn update_now_asks_first_only_when_turns_would_be_interrupted() {
        let mut s = AppState::new();
        s.set_targets(vec![target("devbox", true, Some(("0.1.0+40", true)))]);
        let t = yaac_protocol::TargetId::new("devbox");
        assert_eq!(super::update_now_dialog(&s, &t), None, "nothing running: no confirmation");
        for (id, state) in [("a", "running"), ("b", "needsAttention"), ("c", "idle")] {
            let summary: yaac_protocol::ThreadSummary = serde_json::from_value(json!({
                "id": id, "instance": "fake", "title": id, "cwd": "/w", "state": state, "control": "mine", "originKind": "yaac",
                "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-01T00:00:00Z"
            }))
            .unwrap();
            s.upsert_summary(&t, summary);
        }
        let d = super::update_now_dialog(&s, &t).expect("two turns in progress");
        assert_eq!(d.title, "Update yaacd on devbox now?");
        assert!(d.body.starts_with("2 turns are in progress on devbox."), "{}", d.body);
    }

    #[test]
    fn an_older_hub_or_an_older_window_is_the_one_to_update() {
        let mut s = AppState::new();
        s.hub.outcome = HelloOutcome::HubOutdated { hub_build: "0.1.0+41".into() };
        assert!(matches!(components(&s, "0.1.0+42")[1].status, S::Outdated(_)));
        s.hub.outcome = HelloOutcome::WindowOutdated { message: "update YAAC".into() };
        let c = components(&s, "0.1.0+41");
        assert_eq!(c[0].status, S::Outdated("update YAAC".into()));
        assert_eq!(c[1].status, S::Current);
        s.hub.outcome = HelloOutcome::Pending;
        assert!(matches!(components(&s, "0.1.0+42")[1].status, S::Unknown(_)));
        assert_eq!(outdated(&s, "0.1.0+42"), 0);
    }

    #[test]
    fn the_latest_release_row_says_whether_this_build_is_behind() {
        let latest = ReleaseCheck::Latest { version: "0.1.0".into(), build: 1005 };
        let r = release(&latest, "0.1.0+1004");
        assert_eq!(r.name, "YAAC (latest release)");
        assert_eq!(r.build.as_deref(), Some("0.1.0+1005"));
        assert!(matches!(r.status, S::Outdated(_)));
        assert_eq!(release(&latest, "0.1.0+1005").status, S::Current);
        assert_eq!(release(&latest, "0.1.0+1006").status, S::Current);
        assert!(matches!(release(&latest, "0.1.0+dev.ab").status, S::Unknown(_)));
        assert!(matches!(release(&ReleaseCheck::Unavailable("offline".into()), "0.1.0+1").status, S::Unknown(_)));
        assert_eq!(release(&ReleaseCheck::Checking, "0.1.0+1").build, None);
    }
}
