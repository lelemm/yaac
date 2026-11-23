use serde_json::json;
use yaac_protocol::methods::names as m;

use super::{Call, Client};
use crate::{
    key::ThreadKey,
    running_commands::{self, CommandKey, CommandLog},
};

impl Client {
    /// Open this command's log panel or close it if already open.
    pub fn toggle_command_log(&mut self, view: &ThreadKey, id: &str) {
        if self
            .command_log
            .as_ref()
            .is_some_and(|log| &log.view == view && log.command.id == id)
        {
            self.command_log = None;
            return;
        }
        let Some(row) = running_commands::rows(&self.state, view)
            .into_iter()
            .find(|row| row.command.id == id)
        else {
            return;
        };
        self.command_log = Some(CommandLog {
            view: view.clone(),
            command: row.key(),
            title: row.command.command.clone(),
            result: None,
            error: None,
            next_read: 0,
        });
    }

    /// Poll the open, visible log with at most one read in flight.
    pub fn refresh_command_log(&mut self, view: &ThreadKey, now: i64) {
        let Some(log) = self
            .command_log
            .as_mut()
            .filter(|l| &l.view == view && now >= l.next_read)
        else {
            return;
        };
        let command = log.command.clone();
        let is_listed = running_commands::rows(&self.state, view)
            .iter()
            .any(|row| row.key() == command);
        if !is_listed
            || self
                .pending
                .values()
                .any(|call| matches!(call, Call::CommandLogs(key) if *key == command))
        {
            return;
        }
        log.next_read = now + 1;
        let target = command.owner.target.clone();
        let params = json!({ "threadId": command.owner.thread, "commandId": command.id });
        self.routed(&target, m::COMMAND_LOGS, params, Call::CommandLogs(command));
    }

    /// Whether a Stop request for this process is awaiting acknowledgment.
    pub fn stopping_command(&self, command: &CommandKey) -> bool {
        self.pending.values().any(
            |call| matches!(call, Call::CommandStop { command: pending, .. } if pending == command),
        )
    }

    /// Stop a process only if it is still visible here and the owning harness supports it.
    pub fn stop_command(&mut self, view: &ThreadKey, id: &str) {
        let Some(row) = running_commands::rows(&self.state, view)
            .into_iter()
            .find(|row| row.command.id == id && row.can_stop)
        else {
            return;
        };
        let command = row.key();
        if self.stopping_command(&command) {
            return;
        }
        let op = self.op_id();
        let target = command.owner.target.clone();
        let params =
            json!({ "opId": op, "threadId": command.owner.thread, "commandId": command.id });
        self.routed(
            &target,
            m::COMMAND_STOP,
            params,
            Call::CommandStop {
                view: view.clone(),
                command,
            },
        );
    }
}
