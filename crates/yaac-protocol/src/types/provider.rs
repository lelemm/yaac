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
    /// How much this mode lets the agent do without the user's approval, on a scale shared by
    /// every provider. Filled in by each provider from what its harness documents for the
    /// mode; absent when it cannot say. The agent mesh compares modes on it, so that a worker
    /// is never started in a mode more permissive than its parent's (D146). Absent is unknown,
    /// never "least" — and a mode with `skipsApprovals` counts as `full` whatever this says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autonomy: Option<ModeAutonomy>,
}

open_enum! {
    /// What a mode lets the agent do without asking the user, least first. The order is the
    /// order of the variants: each level allows at least what the one before it allows. A
    /// value this build does not know is treated as unknown — never compared.
    pub enum ModeAutonomy fallback Other {
        /// It changes nothing (it plans, reads, and answers).
        ReadOnly = "readOnly",
        /// It reads freely; anything that changes something asks first (apart from what the
        /// user's own rules already allow).
        Ask = "ask",
        /// File edits proceed without asking; other changes ask.
        Edits = "edits",
        /// Edits and commands confined to the working directory proceed; anything beyond it
        /// asks.
        Workspace = "workspace",
        /// The harness itself approves or denies each action (a classifier or reviewer), and
        /// asks the user only when unsure.
        Judged = "judged",
        /// Everything proceeds; it never asks.
        Full = "full",
    }
}

impl ModeAutonomy {
    /// Position on the scale, least first; `None` for a value this build does not know.
    pub fn rank(&self) -> Option<u8> {
        Some(match self {
            ModeAutonomy::ReadOnly => 0,
            ModeAutonomy::Ask => 1,
            ModeAutonomy::Edits => 2,
            ModeAutonomy::Workspace => 3,
            ModeAutonomy::Judged => 4,
            ModeAutonomy::Full => 5,
            ModeAutonomy::Other(_) => return None,
        })
    }
}

/// A model, exactly as the harness listed it. Never hardcoded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    /// What the harness expects back.
    pub id: ModelId,
    /// The specific model behind an alias, when the harness reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_model: Option<ModelId>,
    /// Label to show.
    pub display_name: String,
    /// Longer description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// YAAC’s guidance for choosing this model, when documented or set by the user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub best_use: Option<ModelBestUse>,
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

/// A short model recommendation and its provenance.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ModelBestUse {
    /// The text shown in Settings and in agent model lists.
    pub summary: String,
    /// A user edit takes precedence over generated guidance.
    pub custom: bool,
}

/// Set (or clear) the recommendation override for a model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ModelBestUseSetParams {
    /// Operation id.
    pub op_id: OpId,
    /// An instance that advertises the model.
    pub instance: InstanceId,
    /// The harness’s stable model id.
    pub model: ModelId,
    /// The specific model ID displayed while editing, to catch aliases changing.
    pub expected_model: ModelId,
    /// A user edit, or null to restore the generated guidance.
    pub summary: Option<String>,
}

/// `providerInstance/setModelEnabled`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInstanceSetModelEnabledParams {
    pub op_id: OpId,
    pub instance: InstanceId,
    /// The model ID advertised by this instance.
    pub model: ModelId,
    pub enabled: bool,
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
    /// Models turned off in Settings, retained for Settings and existing threads.
    #[serde(default)]
    pub disabled_models: Vec<ModelInfo>,
    /// The model this instance uses for command approval explanations, if configured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_explanation_model: Option<ModelId>,
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
    /// Dictation through this instance (docs/plan/voice.md): the audio it takes when it can
    /// transcribe now (with the `transcription` capability), or why it cannot. Absent: the
    /// instance has no voice surface at all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcription: Option<super::TranscriptionOffer>,
}

impl ProviderDescriptor {
    /// Catalog metadata, including models an existing thread may still be using.
    pub fn find_model(&self, id: &ModelId) -> Option<&ModelInfo> {
        self.models.iter().chain(&self.disabled_models).find(|m| &m.id == id)
    }

    pub fn model_is_off(&self, id: &ModelId) -> bool {
        self.disabled_models.iter().any(|m| &m.id == id)
    }
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
    /// The harness home (P300): the folder the harness keeps its configuration, sessions and
    /// sign-in in, under the `configurableHome` capability. `yaacd` maps it to the provider's
    /// own variable (Codex: `CODEX_HOME`, Claude Code: `CLAUDE_CONFIG_DIR`). It wins over that
    /// variable in `env` and in the daemon's environment. Absent: the harness default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<TargetPath>,
    /// Whether this is an implicit default instance (exists without configuration).
    #[serde(default)]
    pub implicit: bool,
    /// "Trust folders YAAC works in" (D147): under the `folderTrust` capability, mark the
    /// folder of a thread YAAC starts in a user's project as trusted in the harness. Absent:
    /// on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trust_folders: Option<bool>,
    /// Model IDs turned off for this instance. New IDs are on by default.
    #[serde(default)]
    pub disabled_model_ids: Vec<ModelId>,
    /// Model selected for command approval explanations on this instance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_explanation_model: Option<ModelId>,
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
    /// The harness home (P300), under the `configurableHome` capability. Absent: the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<TargetPath>,
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
    /// New harness home (P300): absent = unchanged, `null` = back to the harness default.
    /// Under the `configurableHome` capability.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "crate::types::double_option")]
    #[schemars(with = "Option<TargetPath>")]
    pub home: Option<Option<TargetPath>>,
    /// The user confirmed that the new home is another store (P300). Without it, a `home`
    /// that leads to another store is refused with `store_change` and nothing changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirm_store_change: Option<bool>,
    /// New explanation model. Absent leaves it unchanged and null clears it.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "crate::types::double_option")]
    #[schemars(with = "Option<ModelId>")]
    pub approval_explanation_model: Option<Option<ModelId>>,
}

/// The `data` of a `store_change` error (P300): the instance's home now and the one asked
/// for, which lead to different stores.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StoreChangeData {
    /// The home in use now, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<TargetPath>,
    /// The home asked for (the harness default when it was cleared).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<TargetPath>,
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
