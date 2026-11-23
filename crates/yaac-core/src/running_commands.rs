//! Running command rows shared by the composers.

use std::collections::BTreeSet;

use yaac_protocol::{CommandLogsResult, RunningCommand};

use crate::{key::ThreadKey, state::AppState, time};

/// A command is identified by its owning thread and its run id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandKey {
    /// The thread that launched the command.
    pub owner: ThreadKey,
    /// The daemon's process-run id.
    pub id: String,
}

/// A command on the selected thread or any of its reachable workers.
#[derive(Debug)]
pub struct CommandRow<'a> {
    /// The owning thread.
    pub owner: ThreadKey,
    /// The owner title.
    pub title: &'a str,
    /// The target label.
    pub target: String,
    /// The live process.
    pub command: &'a RunningCommand,
    /// Whether the local window can issue the stop.
    pub can_stop: bool,
}

impl CommandRow<'_> {
    /// The command's routing key.
    pub fn key(&self) -> CommandKey {
        CommandKey {
            owner: self.owner.clone(),
            id: self.command.id.clone(),
        }
    }

    /// The row's owner, target, and time since the first observation.
    pub fn detail(&self, now: i64) -> String {
        let location = format!("Run by {} · {}", self.title, self.target);
        match time::secs(&self.command.observed_at) {
            Some(at) => format!(
                "{location} · {} observed",
                time::duration(now.saturating_sub(at))
            ),
            None => location,
        }
    }
}

/// Only connected owners can attest to process liveness. A disconnected parent still sees
/// the live commands of its connected workers.
pub fn rows<'a>(state: &'a AppState, root: &ThreadKey) -> Vec<CommandRow<'a>> {
    let children = state.children_index();
    let mut todo = vec![root.clone()];
    let mut seen = BTreeSet::new();
    let mut rows = Vec::new();
    while let Some(owner) = todo.pop() {
        if !seen.insert(owner.clone()) {
            continue;
        }
        if let Some(kids) = children.get(&owner.address()) {
            todo.extend(kids.iter().rev().cloned());
        }
        if !state
            .target(&owner.target)
            .is_some_and(|t| t.connected() && t.commands_current)
        {
            continue;
        }
        let Some(summary) = state.summary(&owner) else {
            continue;
        };
        if summary.unavailable.is_some() {
            continue;
        }
        for command in summary.running_commands.as_deref().unwrap_or_default() {
            let controller = command
                .controller
                .as_ref()
                .map(|id| ThreadKey::new(owner.target.clone(), id.clone()));
            let can_stop = command.can_stop && state.may_act(controller.as_ref().unwrap_or(&owner));
            rows.push(CommandRow {
                owner: owner.clone(),
                title: &summary.title,
                target: state.target_name(&owner.target),
                command,
                can_stop,
            });
        }
    }
    rows
}

/// An open log panel retains its last output when the process leaves the live list.
#[derive(Debug)]
pub struct CommandLog {
    /// The view where this log was opened, which may be the owner's ancestor.
    pub view: ThreadKey,
    /// Where the process runs.
    pub command: CommandKey,
    /// The displayed shell input.
    pub title: String,
    /// The last output tail received.
    pub result: Option<CommandLogsResult>,
    /// Failure of the last read, if any.
    pub error: Option<String>,
    pub(crate) next_read: i64,
}

impl CommandLog {
    /// Human-readable state when output hasn't arrived or the view is no longer live.
    pub fn status(&self, listed: bool) -> Option<&str> {
        if let Some(error) = &self.error {
            Some(error)
        } else if !listed {
            Some("This command is no longer reported running.")
        } else if let Some(result) = &self.result {
            match result.output.as_deref() {
                None => Some("The harness hasn’t exposed output for this command."),
                Some("") => Some("No output yet."),
                Some(_) if result.truncated => Some("Showing the most recent output."),
                Some(_) => None,
            }
        } else {
            Some("Loading output…")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use yaac_protocol::attach::{ConnectionStatus, TargetStateParams};

    #[test]
    fn parents_show_connected_workers_with_their_own_controls() {
        let root = ThreadKey::new("local", "root");
        let worker = ThreadKey::new("remote", "worker");
        let mut state = AppState::new();
        state.set_targets(serde_json::from_value(json!([
            {"id": "local", "name": "This machine", "enabled": true, "transport": {"kind":"local"}, "state": {"status":"connected"}},
            {"id": "remote", "name": "Builder", "enabled": true, "transport": {"kind":"ssh", "host":"builder"}, "state": {"status":"connected"}}
        ])).unwrap());
        state.upsert_summary(
            &root.target,
            serde_json::from_value(json!({
                "id":"root", "instance":"instance", "title":"Parent", "cwd":"/work",
                "state":"idle", "control":"readOnly", "originKind":"yaac",
                "createdAt":"2026-11-23T12:00:00Z", "updatedAt":"2026-11-23T12:00:00Z",
            }))
            .unwrap(),
        );
        let worker_summary = serde_json::from_value(json!({
            "id":"worker", "instance":"instance", "title":"Worker", "cwd":"/work",
            "state":"done", "control":"free", "originKind":"yaac", "spawnedBy":root.address(),
            "createdAt":"2026-11-23T12:00:00Z", "updatedAt":"2026-11-23T12:00:00Z",
            "runningCommands":[{"id":"proc", "command":"cargo test", "canStop":true, "observedAt":"2026-11-23T12:00:00Z"}]
        })).unwrap();
        state.upsert_summary(&worker.target, worker_summary);
        state.target_mut(&worker.target).unwrap().commands_current = true;
        let rows = super::rows(&state, &root);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].owner, worker);
        assert!(rows[0].can_stop);
        assert_eq!(
            rows[0].detail(time::parse_rfc3339("2026-11-23T12:00:10Z").unwrap()),
            "Run by Worker · Builder · 10s observed"
        );
        let mut connection = state.target(&worker.target).unwrap().entry.state.clone();
        connection.status = ConnectionStatus::Offline;
        state.apply_target_state(TargetStateParams {
            id: worker.target.clone(),
            state: connection,
            enabled: true,
        });
        assert!(super::rows(&state, &root).is_empty());
        let mut connection = state.target(&worker.target).unwrap().entry.state.clone();
        connection.status = ConnectionStatus::Connected;
        state.apply_target_state(TargetStateParams {
            id: worker.target,
            state: connection,
            enabled: true,
        });
        assert!(
            super::rows(&state, &root).is_empty(),
            "cached observations need a fresh sync"
        );
    }
}
