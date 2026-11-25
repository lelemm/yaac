//! Target-owned terminal sessions and their connection-local data streams.
use super::{StreamId, ThreadId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// An available shell on the target.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TerminalShell {
    /// Executable path.
    pub path: String,
    /// Shell display name.
    pub name: String,
}

/// Shell choices in preference order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TerminalShellsResult {
    /// Available executables, default first.
    pub shells: Vec<TerminalShell>,
}

/// Start a shell in a target working directory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TerminalOpenParams {
    /// Working directory on the target.
    pub cwd: String,
    /// Shell executable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,
    /// Thread that opened this terminal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<ThreadId>,
    /// Width in cells.
    pub cols: u16,
    /// Height in cells.
    pub rows: u16,
}

/// The new shell and its first attachment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TerminalOpenResult {
    /// Target-scoped terminal identifier.
    pub terminal: String,
    /// Connection-local bidirectional stream.
    pub stream: StreamId,
    /// Input bytes granted to the host.
    pub initial_credit: u64,
}

/// Attach a new stream to a running shell.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TerminalAttachParams {
    /// Target-scoped terminal identifier.
    pub terminal: String,
    /// Width in cells.
    pub cols: u16,
    /// Height in cells.
    pub rows: u16,
}

/// A new duplex attachment, with replay ahead of live output.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TerminalAttachResult {
    /// Connection-local bidirectional stream.
    pub stream: StreamId,
    /// Input bytes granted to the host.
    pub initial_credit: u64,
}

/// Set the shared PTY size.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TerminalResizeParams {
    /// Target-scoped terminal identifier.
    pub terminal: String,
    /// Width in cells.
    pub cols: u16,
    /// Height in cells.
    pub rows: u16,
}

/// Release one attachment, retaining the shell.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TerminalDetachParams {
    /// Connection-local attachment.
    pub stream: StreamId,
}

/// End a running shell.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TerminalCloseParams {
    /// Target-scoped terminal identifier.
    pub terminal: String,
}

/// A running target terminal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TerminalInfo {
    /// Target-scoped terminal identifier.
    pub terminal: String,
    /// Working directory on the target.
    pub cwd: String,
    /// Shell executable.
    pub shell: String,
    /// Most recent OSC title.
    pub title: String,
    /// Thread that opened this terminal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<ThreadId>,
    /// The name the user gave the terminal (`terminal/rename`), if any; it wins over `title`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// The active terminals owned by this daemon.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TerminalListResult {
    /// Active terminals.
    pub terminals: Vec<TerminalInfo>,
}

/// Final status after a shell exits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TerminalExitedParams {
    /// Target-scoped terminal identifier.
    pub terminal: String,
    /// Shell exit status.
    pub code: i32,
}

/// Title decoded from OSC 0 or 2.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TerminalTitleParams {
    /// Target-scoped terminal identifier.
    pub terminal: String,
    /// Most recent OSC title.
    pub title: String,
}

/// Terminal Rename Params.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TerminalRenameParams {
    /// Target-scoped terminal identifier.
    pub terminal: String,
    /// The user's name for the terminal; absent or empty goes back to the automatic title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Terminal Renamed Params.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TerminalRenamedParams {
    /// Target-scoped terminal identifier.
    pub terminal: String,
    /// The user's name; absent when it went back to the automatic title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}
