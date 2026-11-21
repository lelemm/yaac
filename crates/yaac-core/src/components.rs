//! Which of YAAC's components need updating, checked as YAAC starts (D135).
//!
//! Every artefact of one build carries the same build id after its own version
//! (`<version>+<id>`, `scripts/build-id.sh`). The window knows its own build; `hub/hello` says
//! whether the running hub is the same build, older (the window replaces it), or newer; and the
//! hub, on connecting to each target, compares that target's `yaacd` with its own build and
//! reports an older one ([`yaac_protocol::attach::DaemonSummary::older`]). This module only
//! gathers those answers into one list: nothing here asks the network itself.

use crate::state::{AppState, HelloOutcome};

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
        let c = match &t.entry.state.daemon {
            Some(d) if d.older => Component {
                name,
                build: Some(d.build.clone()),
                status: ComponentStatus::Outdated("older build than this YAAC".into()),
            },
            Some(d) => Component { name, build: Some(d.build.clone()), status: ComponentStatus::Current },
            None => Component { name, build: None, status: ComponentStatus::Unknown(format!("not connected ({})", t.entry.state.status.as_str())) },
        };
        out.push(c);
    }
    out
}

/// How many components need updating.
pub fn outdated(state: &AppState, window_build: &str) -> usize {
    components(state, window_build).iter().filter(|c| matches!(c.status, ComponentStatus::Outdated(_))).count()
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use yaac_protocol::attach::TargetEntry;

    use super::{ComponentStatus as S, components, outdated};
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
}
