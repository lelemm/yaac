//! Provider instances and what they offer (docs/04-providers.md).

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{Capability, EffortId, InstanceId, ModeId, ModelCapability, ModelId, OpId, ProviderKind, TargetPath, Timestamp};
use crate::macros::open_enum;

open_enum! {
    /// The backend an instance actually runs on.
    pub enum BackendKind fallback Unknown {
        /// The harness's own daemon (Codex app-server daemon).
        Daemon = "daemon",
        /// An SDK running the harness as a child of `yaacd` (Claude Agent SDK).
        Sdk = "sdk",
    }
}

open_enum! {
    /// Whether `yaacd` found the harness's backend process (a daemon) already running or
    /// started it itself. Display and diagnostics only.
    pub enum BackendOrigin fallback Unknown {
        /// It was running when `yaacd` connected to it.
        FoundRunning = "foundRunning",
        /// `yaacd` started it.
        StartedByYaac = "startedByYaac",
    }
}

open_enum! {
    /// The backend an instance is configured to use.
    pub enum BackendPreference fallback Unknown {
        /// Let `yaacd` choose.
        Auto = "auto",
        /// The harness's daemon only.
        Daemon = "daemon",
        /// The SDK only.
        Sdk = "sdk",
    }
}

open_enum! {
    /// Whether an instance can be used, and if not, why. The reasons are distinct because they
    /// need different fixes.
    pub enum ProviderStatus fallback Unknown {
        /// Usable.
        Ready = "ready",
        /// The harness is not installed on the target.
        NotInstalled = "notInstalled",
        /// The harness is too old for YAAC (`harnessVersion` says what was found).
        TooOld = "tooOld",
        /// The harness is not signed in.
        NeedsAuth = "needsAuth",
        /// The instance cannot be reached (nothing listening on its socket).
        Unreachable = "unreachable",
        /// `yaacd` tried to start the harness daemon and could not.
        StartFailed = "startFailed",
        /// Access to the harness or its socket was denied.
        AccessDenied = "accessDenied",
        /// The instance is disabled in its configuration.
        Disabled = "disabled",
        /// Being probed; not known yet.
        Probing = "probing",
    }
}

/// Which conversations an instance owns. Two instances with equal `key`s see the same
/// conversations; native fork is possible only within one store.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StoreIdentity {
    /// Opaque comparison key computed by `yaacd`. `None` while the store is not yet known
    /// (for example before the first successful probe); two unknown stores are never equal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// The store's home directory on the target, when it has one (Codex: `CODEX_HOME`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<TargetPath>,
    /// The signed-in account, as the harness reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
}

impl StoreIdentity {
    /// Whether both are known and the same store.
    pub fn same_store(&self, other: &StoreIdentity) -> bool {
        matches!((&self.key, &other.key), (Some(a), Some(b)) if a == b)
    }
}

/// An effort level a model offers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EffortInfo {
    /// What the harness expects back.
    pub id: EffortId,
    /// Label to show; when absent, show the id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Longer description, when the harness gives one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// A permission / approval / sandbox mode on offer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ModeInfo {
    /// What the harness expects back.
    pub id: ModeId,
    /// Label to show.
    pub display_name: String,
    /// Longer description, when the harness gives one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Whether this is the harness's default mode.
    #[serde(default)]
    pub is_default: bool,
    /// True when this mode lets the agent act without asking — it bypasses approval prompts
    /// altogether. The UI must ask the user to confirm before switching a thread to it, and
    /// must keep it visibly marked while it is active.
    #[serde(default)]
    pub skips_approvals: bool,
}

/// A model, exactly as the harness listed it. Never hardcoded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    /// What the harness expects back.
    pub id: ModelId,
    /// Label to show.
    pub display_name: String,
    /// Longer description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Whether this is the harness's default model.
    #[serde(default)]
    pub is_default: bool,
    /// Whether the harness lists it as hidden.
    #[serde(default)]
    pub hidden: bool,
    /// Model-level capabilities, each supported, unsupported, or unknown. A capability not
    /// listed is unknown.
    #[serde(default)]
    pub capabilities: Vec<ModelCapability>,
    /// Effort levels, in the order the harness reported them.
    #[serde(default)]
    pub efforts: Vec<EffortInfo>,
    /// The default effort, when the harness reports one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_effort: Option<EffortId>,
    /// Context window in tokens, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u32>,
}

/// Everything the host knows about one provider instance.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderDescriptor {
    /// The instance.
    pub instance: InstanceId,
    /// Its kind. Display and diagnostics only — never branch on it.
    pub kind: ProviderKind,
    /// User-given instance name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// "Codex", or "Codex · work".
    pub display_name: String,
    /// The backend in use.
    pub backend: BackendKind,
    /// Version of the process actually serving, not just the CLI on `PATH`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness_version: Option<String>,
    /// Which conversations this instance owns.
    pub store: StoreIdentity,
    /// Whether it can be used.
    pub status: ProviderStatus,
    /// Human-readable detail for a status other than `ready`, shown as-is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_message: Option<String>,
    /// What this instance can do, on this backend.
    #[serde(default)]
    pub capabilities: Vec<Capability>,
    /// Models, fetched from the harness.
    #[serde(default)]
    pub models: Vec<ModelInfo>,
    /// Whether `models` is a complete listing. A model is only ever declared gone after a
    /// complete listing.
    #[serde(default)]
    pub models_complete: bool,
    /// Modes on offer.
    #[serde(default)]
    pub modes: Vec<ModeInfo>,
    /// Bumps whenever anything above changes. Monotonic per instance.
    pub revision: u64,
    /// For a backend that is a separate process (a daemon): whether `yaacd` found it running
    /// or started it. Absent when there is no such process or it is not known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_origin: Option<BackendOrigin>,
    /// When `models` was last read from the harness on the target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models_read_at: Option<Timestamp>,
}

/// Endpoint settings of an instance.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderEndpoint {
    /// An explicit control socket. An instance defined by one is connect-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub socket: Option<TargetPath>,
}

/// The stored configuration of one instance, kept on the target by `yaacd`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInstanceConfig {
    /// Stable, generated by `yaacd`.
    pub id: InstanceId,
    /// The provider kind.
    pub kind: ProviderKind,
    /// Optional user-given name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Whether it is enabled.
    pub enabled: bool,
    /// Backend preference.
    pub backend: BackendPreference,
    /// Endpoint settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<ProviderEndpoint>,
    /// Extra environment for the harness (e.g. `CODEX_HOME`).
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Whether this is an implicit default instance (exists without configuration).
    #[serde(default)]
    pub implicit: bool,
    /// "Trust folders YAAC works in" (D147): under the `folderTrust` capability, mark the
    /// folder of a thread YAAC starts in a user's project as trusted in the harness. Absent:
    /// on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trust_folders: Option<bool>,
}

/// `provider/list` — no parameters.
pub type ProviderListParams = super::Empty;

/// `provider/list` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderListResult {
    /// Every instance on the target.
    pub providers: Vec<ProviderDescriptor>,
}

/// `provider/refresh`: re-probe one instance, or all when `instance` is absent.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderRefreshParams {
    /// The instance; all when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<InstanceId>,
}

/// `provider/updated` notification: an instance's descriptor changed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderUpdatedParams {
    /// The new descriptor; its `revision` is higher than any sent before for this instance.
    pub provider: ProviderDescriptor,
}

/// `providerInstance/list` — no parameters.
pub type ProviderInstanceListParams = super::Empty;

/// `providerInstance/list` result: the stored configurations, including implicit defaults.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInstanceListResult {
    /// Every configured or implicit instance.
    pub instances: Vec<ProviderInstanceConfig>,
}

/// `providerInstance/add`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInstanceAddParams {
    /// Operation id.
    pub op_id: OpId,
    /// The provider kind.
    pub kind: ProviderKind,
    /// Optional user-given name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Whether it starts enabled.
    pub enabled: bool,
    /// Backend preference.
    pub backend: BackendPreference,
    /// Endpoint settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<ProviderEndpoint>,
    /// Extra environment.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// Result of `providerInstance/add` and `providerInstance/update`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInstanceResult {
    /// The stored configuration after the change.
    pub instance: ProviderInstanceConfig,
}

/// `providerInstance/update`: a patch. Absent fields are unchanged, so an older client never
/// erases settings it does not know about.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInstanceUpdateParams {
    /// Operation id.
    pub op_id: OpId,
    /// The instance.
    pub id: InstanceId,
    /// New name: absent = unchanged, `null` = remove the name.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "crate::types::double_option")]
    #[schemars(with = "Option<String>")]
    pub name: Option<Option<String>>,
    /// New enabled flag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// New backend preference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<BackendPreference>,
    /// New endpoint: absent = unchanged, `null` = remove it.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "crate::types::double_option")]
    #[schemars(with = "Option<ProviderEndpoint>")]
    pub endpoint: Option<Option<ProviderEndpoint>>,
    /// New environment, replacing the old one entirely.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<BTreeMap<String, String>>,
    /// New "Trust folders YAAC works in" switch (D147).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trust_folders: Option<bool>,
}

/// `providerInstance/remove`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInstanceRemoveParams {
    /// Operation id.
    pub op_id: OpId,
    /// The instance.
    pub id: InstanceId,
}
