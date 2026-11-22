//! Plan rate limits (D161): which account a provider instance runs as, and how much of each of
//! that account's usage windows is used. Reported by instances with the `rateLimits`
//! capability, from what the harness itself says (Codex: `account/read` and
//! `account/rateLimits/*`; Claude Code: `accountInfo()`, its usage report, and the stream's
//! `rate_limit_event`). Nothing here names a model or a plan: labels are the harness's.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{InstanceId, ProviderKind, Timestamp};

/// The account an instance runs as.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitAccount {
    /// Stable across targets for the same account of the same provider kind, and opaque: a
    /// digest of the harness's account identifiers, never the identifiers themselves. The hub
    /// shows one group per key.
    pub key: String,
    /// The plan as the harness names it ("Claude Ultra", "Pro"), for display.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// The sign-in's email, when the harness reports one. Shown only in the panel's tooltip on
    /// the user's own computer; never logged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

/// One usage window of an account.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitWindow {
    /// Stable within the account, for merging updates (`session`, `weekly`, `weekly:Fable`).
    pub id: String,
    /// Short display label: `5h`, `weekly`.
    pub label: String,
    /// What a scoped window counts (a model or a surface, as the harness labels it). Absent:
    /// the window counts everything on the account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// Share of the window used, in whole percent (0–100; a harness reporting a fraction is
    /// rounded).
    pub used_percent: u32,
    /// When the window resets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<Timestamp>,
    /// The window's length in minutes, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_minutes: Option<u32>,
}

/// What one instance reports: `provider/rateLimits` lists these, and
/// `provider/rateLimits/updated` carries one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InstanceRateLimits {
    /// The instance.
    pub instance: InstanceId,
    /// Its provider kind (display and grouping only: an account key is per kind).
    pub kind: ProviderKind,
    /// The account, once known. Without one the hub cannot group the report and keeps it out
    /// of the panel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<RateLimitAccount>,
    /// The windows, in the harness's order. Empty: the harness reported none (an API key, a
    /// cloud provider, or nothing read yet).
    #[serde(default)]
    pub windows: Vec<RateLimitWindow>,
    /// When the harness last reported any of this.
    pub at: Timestamp,
}

/// `provider/rateLimits` params.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderRateLimitsParams {
    /// Read the harness again instead of answering with the last report. `yaacd` still holds
    /// at least five minutes between reads of one instance.
    #[serde(default)]
    pub refresh: bool,
}

/// `provider/rateLimits` result.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderRateLimitsResult {
    /// One entry per instance with the `rateLimits` capability that has reported anything.
    pub instances: Vec<InstanceRateLimits>,
}
