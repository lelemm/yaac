//! Hub transfer over the old hub's existing daemon connection (docs/15-hub-transfer.md).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{Empty, OpId, Sha256Hex, StreamId, TargetPath};

/// `hubHost/probe` params.
pub type HubHostProbeParams = Empty;

/// This target's Tailscale state for hub transfer eligibility.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HubHostTailscale {
    pub running: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub login: Option<String>,
    pub ips: Vec<String>,
}

/// WSL details reported by a daemon running inside a distribution.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HubHostWsl {
    /// The registered distribution name, or `WSL` if unavailable.
    pub distro: String,
    /// What the PC needs to run a moved hub here as a systemd system service
    /// (docs/15-hub-transfer.md#into-this-pcs-wsl-p416). Absent from a `yaacd` that predates it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service: Option<HubHostWslService>,
}

/// The facts for the hub's system unit in a WSL distro (`/etc/systemd/system/yaac-hub.service`,
/// installed from Windows as root through `wsl.exe -u root`; `yaacd` itself never has root).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HubHostWslService {
    /// systemd is the distro's system manager (`[boot] systemd=true` in `/etc/wsl.conf`).
    pub systemd: bool,
    /// The user `yaacd` runs as: the hub's service runs as this user (`User=`).
    pub user: String,
    pub uid: u32,
    /// The hub profile's data directory (`bin/yaac` is under it), as `yaacd` finds it.
    pub data_dir: String,
    /// `KEY=value` entries for the service, as `yaacd` has them: `HOME`, the `YAAC_*_DIR`
    /// profile directories, `YAAC_HOME` when set, `PATH`.
    pub environment: Vec<String>,
}

/// The outcome recorded after a previous hand-over.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HubHostLastTransfer {
    pub op_id: OpId,
    /// `started`, `placed` (startup delegated), or `failed`.
    pub status: String,
    /// The reason startup failed, when applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// `hubHost/probe` result: the facts the old hub uses to decide eligibility.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HubHostProbeResult {
    pub os: String,
    pub arch: String,
    pub tailscale: HubHostTailscale,
    /// The hub's attach port is available on the target's Tailscale addresses.
    pub port_free: bool,
    pub hub_running: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wsl: Option<HubHostWsl>,
    /// The build of a YAAC already installed on this target that can run the hub there without
    /// `hubHost/install` (Windows: the `yaac.exe` in `YAAC_EXE`, from `yaac hub-info`). Absent:
    /// none (on Windows the target is then not a candidate).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hub_build: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_transfer: Option<HubHostLastTransfer>,
}

/// `hubHost/install`: download and verify the exact hub release binary on the target.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HubHostInstallParams {
    pub op_id: OpId,
    pub url: String,
    pub sha256: Sha256Hex,
    pub size: u64,
}

/// `hubHost/install` result: the verified executable's path on the target.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HubHostInstallResult {
    pub path: TargetPath,
}

/// `hubHost/put`: open a credited stream for one of the transfer's staged files.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HubHostPutParams {
    pub op_id: OpId,
    pub name: String,
    pub size: u64,
    pub sha256: Sha256Hex,
}

/// `hubHost/put` result: the daemon verifies the file after receiving its declared size.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HubHostPutResult {
    /// Connection-local stream for file bytes from the hub.
    pub stream: StreamId,
    /// Initial byte credit. More arrives via `stream/credit`.
    pub credit: u64,
}

/// `hubHost/putComplete`: one credited file stream was verified and staged or failed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HubHostPutCompleteParams {
    pub stream: StreamId,
    /// The error when staging failed. Absent after a successful put.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// `hubHost/start`: validate the transfer and arm file placement on connection close.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HubHostStartParams {
    pub op_id: OpId,
    /// The transfer requests deferred file placement with `true`.
    pub arm_on_close: bool,
    /// Start the hub and configure autostart after placement. Defaults to true.
    #[serde(default = "start_hub_by_default")]
    pub start_hub: bool,
}

fn start_hub_by_default() -> bool {
    true
}

/// `hubHost/start` result: the transfer is ready and placement is armed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HubHostStartResult {
    pub armed: bool,
}

/// `hubHost/abort`: discard the transfer's staging and disarm startup.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HubHostAbortParams {
    pub op_id: OpId,
}

/// `hubHost/abort` result.
pub type HubHostAbortResult = Empty;

/// `hubHost/progress`: download progress for `hubHost/install`, in bytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HubHostProgressParams {
    pub op_id: OpId,
    pub done: u64,
    pub total: u64,
}
