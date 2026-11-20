//! Session methods: `initialize`, `daemon/status`, `daemon/handover`, `ping`,
//! `diagnostics/get`. `initialize`, `daemon/status`, and `daemon/handover` are part of the
//! **frozen core** (docs/09-compatibility.md#the-frozen-core): their shapes never change.
//!
//! The protocol-range types come from [`crate::dialect`]: their numbers cannot be read
//! anywhere else in the program.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{ClientId, TargetPath};
use crate::dialect::{AgreedProtocol, ProtocolRange};
use crate::macros::open_enum;

/// The client's identity, sent in `initialize`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClientInfo {
    /// Program name (`yaac`).
    pub name: String,
    /// Program build.
    pub version: String,
    /// Host name, shown to other clients as "In use on …".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// Stable id of this host's client, so `yaacd` can tell clients apart across reconnects.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<ClientId>,
}

/// `initialize` params. Frozen core.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    /// The protocol versions the client speaks.
    pub protocol: ProtocolRange,
    /// Who the client is.
    pub client: ClientInfo,
}

/// The daemon's build and on-disk state format.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DaemonInfo {
    /// Which code is running (`0.7.2+a1b2c3`).
    pub build: String,
    /// What is on disk. Separate from the protocol version.
    pub state_format: u32,
}

/// Facts about the target.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TargetFacts {
    /// Operating system (`linux`, `macos`, `windows`).
    pub os: String,
    /// Architecture (`x86_64`, `aarch64`).
    pub arch: String,
    /// Host name.
    pub hostname: String,
    /// User `yaacd` runs as.
    pub user: String,
    /// That user's home directory.
    pub home: TargetPath,
}

/// `initialize` result. Frozen core.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    /// The daemon's range and the agreed version.
    pub protocol: AgreedProtocol,
    /// Hash of the frozen baseline schema of the agreed version. A diagnostic: a mismatch
    /// means one side has a wrong idea of what that version is.
    pub schema_hash: String,
    /// Build and state format.
    pub daemon: DaemonInfo,
    /// Target facts.
    pub target: TargetFacts,
}

/// `daemon/status` — no parameters. Frozen core; valid even after a failed negotiation.
pub type DaemonStatusParams = super::Empty;

/// What the daemon is busy with.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DaemonBusy {
    /// Turns running.
    pub running_turns: u32,
    /// Requests waiting for an answer.
    pub pending_requests: u32,
    /// Clients attached.
    pub attached_clients: u32,
}

/// `daemon/status` result. Frozen core.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DaemonStatusResult {
    /// Which code is running.
    pub build: String,
    /// The protocol versions it speaks.
    pub protocol: ProtocolRange,
    /// What is on disk.
    pub state_format: u32,
    /// What it is busy with.
    pub busy: DaemonBusy,
}

open_enum! {
    /// When the daemon should step aside.
    pub enum HandoverWhen fallback Unknown {
        /// As soon as no turn is running and no request is pending. Attached clients do not
        /// hold it back.
        Idle = "idle",
        /// Now: running turns are interrupted and marked as interrupted by an update.
        Now = "now",
    }
}

/// `daemon/handover` params. Frozen core; valid even after a failed negotiation; idempotent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DaemonHandoverParams {
    /// When.
    pub when: HandoverWhen,
}

open_enum! {
    /// What the daemon will do about a hand-over.
    pub enum HandoverState fallback Unknown {
        /// It will exit once idle. Repeating the request changes nothing.
        Scheduled = "scheduled",
        /// It is exiting now; it closes the connection after sending this response.
        Exiting = "exiting",
    }
}

/// `daemon/handover` result: sent before the daemon closes the connection. Frozen core.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DaemonHandoverResult {
    /// What happens next.
    pub state: HandoverState,
}

/// `ping` — no parameters.
pub type PingParams = super::Empty;
/// `ping` result — empty; the client measures the round trip.
pub type PingResult = super::Empty;

/// `diagnostics/get` — no parameters.
pub type DiagnosticsGetParams = super::Empty;

/// One section of a diagnostics report.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticSection {
    /// Section title (`daemon`, an instance's display name, …).
    pub title: String,
    /// Redacted lines. Never message content.
    pub lines: Vec<String>,
}

/// `diagnostics/get` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsGetResult {
    /// Sections, in display order.
    pub sections: Vec<DiagnosticSection>,
}
