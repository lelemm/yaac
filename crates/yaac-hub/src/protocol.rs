//! Hub-specific additions to the attach protocol of [`yaac_protocol::attach`].
//!
//! `yaac_protocol::attach` defines the core (`hub/hello`, `hub/shutdown`, targets, connection
//! state, the routing envelope) and says the hub and window packages refine it. These are the
//! refinements this hub serves. The window and the hub are always the same build, so these
//! are not versioned either.
//!
//! ## How a window follows a thread
//!
//! 1. `hub/request { target, method: "thread/subscribe", params: { threadId, epoch?, afterSeq? } }`.
//!    The hub holds the one upstream subscription (shared by every window) and answers once
//!    the thread is in the cache: `replay` with nothing to replay when the window's position
//!    is exactly the cache's, otherwise `reset` with the cache's position.
//! 2. On `reset`, read the thread from the cache file — items and position in one read
//!    transaction ([`crate::cache::Cache::thread`]).
//! 3. Apply `hub/notification { method: "thread/event" }` events whose position is strictly
//!    after the one you read. The hub forwards an event only after committing it, so nothing
//!    is missed and nothing is applied twice.
//! 4. [`names::THREAD_RESET`] means the hub rebuilt the thread (its journal was reset): read
//!    it from the cache again.
//!
//! `thread/ack` from a window is answered and ignored: the hub acknowledges what it has
//! committed itself.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use yaac_protocol::{ClientId, JournalPosition, OpId, TargetId, ThreadId, ThreadSummary};

use crate::settings::HubSettings;

/// Additional attach method names.
pub mod names {
    /// Hub → window notification: a cached thread was rebuilt; re-read it from the cache.
    pub const THREAD_RESET: &str = "hub/thread/reset";
    /// Hub → window notification: the target list changed (added, renamed, removed,
    /// enabled); params are a `TargetsListResult`.
    pub const TARGETS_CHANGED: &str = "hub/targets/changed";
    /// Hub → window notification `{ target }`: a `thread/sync` page landed in the cache.
    pub const INDEX_CHANGED: &str = "hub/index/changed";
    /// Hub → window notification `{ target }`: the target's providers in the cache were replaced.
    pub const PROVIDERS_CHANGED: &str = "hub/providers/changed";
    /// Window → hub: find threads across every target in the cached index, by id or by a
    /// fuzzy title match, one page at a time ([`crate::find`]).
    pub const THREADS_FIND: &str = "hub/threads/find";
    /// Window → hub `{ id, target? }`: find a thread by id on every connected target (or one),
    /// asking each `yaacd` — which asks its harnesses — when no index has it
    /// (`thread/lookup`). Result: [`LookupResult`].
    pub const THREADS_LOOKUP: &str = "hub/threads/lookup";
    /// Window → hub `{ id }`: the target's recent connection steps, for diagnostics
    /// ([`TargetLogResult`]). Never message content.
    pub const TARGETS_LOG: &str = "hub/targets/log";
    /// Window → hub: every thread's view preferences (pin, mute, dismiss: D92), from the hub's
    /// durable store ([`ViewGetResult`]).
    pub const VIEW_GET: &str = "hub/view/get";
    /// Window → hub: change one thread's view preferences ([`ViewSetParams`] → [`ViewSetResult`]).
    pub const VIEW_SET: &str = "hub/view/set";
    /// Window → hub: change several at once, e.g. "Dismiss all" of a temporary project
    /// ([`ViewSetManyParams`] → [`ViewGetResult`] with the entries changed).
    pub const VIEW_SET_MANY: &str = "hub/view/setMany";
    /// Hub → window notification: view preferences changed ([`ViewChangedParams`]), so every
    /// window agrees.
    pub const VIEW_CHANGED: &str = "hub/view/changed";
    /// Hub → window notification `{ target }`: a target's discovery caps in the cache changed.
    pub const DISCOVERY_CHANGED: &str = "hub/discovery/changed";
    /// Window → hub `{ id }`: a target's projects, from the cache (`{ projects }`).
    pub const PROJECTS_LIST: &str = "hub/projects/list";
    /// Hub → window notification `{ target }`: a target's projects in the cache were replaced
    /// (on connect). Changes while connected also arrive as routed `project/updated`.
    pub const PROJECTS_CHANGED: &str = "hub/projects/changed";
    /// Window → hub: the hub's status (build, attached windows, why it is staying up, files).
    pub const STATUS: &str = "hub/status";
    /// Window → hub: read the hub settings.
    pub const SETTINGS_GET: &str = "hub/settings/get";
    /// Window → hub: change the hub settings (a patch).
    pub const SETTINGS_UPDATE: &str = "hub/settings/update";
    /// Window → hub: reconnect a target now (user request), skipping the backoff.
    pub const TARGETS_RECONNECT: &str = "hub/targets/reconnect";
    /// Window → hub: clear the cache ("clear cache"). Drafts are not touched.
    pub const CACHE_CLEAR: &str = "hub/cache/clear";
    /// Window → hub: store a `ui_state` value in the cache (the hub is its only writer).
    pub const UI_STATE_SET: &str = "hub/uiState/set";
    /// Window → hub: list drafts.
    pub const DRAFTS_LIST: &str = "hub/drafts/list";
    /// Window → hub: store a draft.
    pub const DRAFTS_PUT: &str = "hub/drafts/put";
    /// Window → hub: delete a draft.
    pub const DRAFTS_DELETE: &str = "hub/drafts/delete";
    /// Window → hub: list operations whose outcome is not yet known or was uncertain.
    pub const OPERATIONS_LIST: &str = "hub/operations/list";
    /// Window → hub: the user has seen an uncertain operation; forget it.
    pub const OPERATIONS_DISMISS: &str = "hub/operations/dismiss";
    /// Window → hub, test builds only (`HubConfig::test_methods`): drop a target's connection
    /// as if the link had died.
    pub const DEBUG_DROP_CONNECTION: &str = "hub/debug/dropConnection";
}

/// Error codes the hub adds (the `code` strings of [`yaac_protocol::rpc::ErrorObject`]).
pub mod codes {
    /// The window is newer than the hub: send `hub/shutdown`, then start the newer hub.
    pub const HUB_OUTDATED: &str = "hub_outdated";
    /// The window is older than the hub: tell the user to update YAAC. Never replace the hub.
    pub const WINDOW_OUTDATED: &str = "window_outdated";
    /// `hub/hello` must be the first message.
    pub const HELLO_REQUIRED: &str = "hello_required";
    /// The method is the hub's own business and cannot be routed (`initialize`, `mesh/deliver`…).
    pub const NOT_ROUTABLE: &str = "not_routable";
    /// A target name is empty or already used.
    pub const NAME_TAKEN: &str = "name_taken";
}

/// `hub/thread/reset` params.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadResetParams {
    /// The target.
    pub target: TargetId,
    /// The thread.
    pub thread_id: ThreadId,
    /// The position the rebuilt cache is valid at.
    pub position: JournalPosition,
}

/// Why the hub is staying up.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BusyReasons {
    /// Windows attached.
    pub windows: u32,
    /// Threads running or waiting for an answer, on enabled targets (last known state).
    pub active_threads: u32,
    /// Routed requests waiting for a target's answer.
    pub pending_requests: u32,
    /// Mesh messages queued in the relay, or receipts not yet handed back.
    pub messages_in_flight: u32,
}

/// `hub/status` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusResult {
    /// The hub's build.
    pub build: String,
    /// The hub's process id.
    pub pid: u32,
    /// Why it is staying up.
    pub busy: BusyReasons,
    /// The hub settings.
    pub settings: HubSettings,
    /// The cache file the window reads.
    pub cache_file: String,
    /// This host's client id (as sent to every `yaacd`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<ClientId>,
}

/// `hub/settings/update` params (a patch).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsUpdateParams {
    /// Keep running always.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub always: Option<bool>,
    /// Idle period before exit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_exit_secs: Option<u64>,
}

/// `hub/settings/get` and `hub/settings/update` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsResult {
    /// The settings now.
    pub settings: HubSettings,
}

/// `hub/targets/reconnect` and `hub/debug/dropConnection` params.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetRef {
    /// The target.
    pub id: TargetId,
}

/// `hub/uiState/set` params.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiStateSetParams {
    /// Key.
    pub key: String,
    /// Value; `null` deletes the key.
    pub value: Value,
}

/// A draft: unsent composer text and its attachments. User data, never part of the cache.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Draft {
    /// The window's key for the composer (a thread, or "new thread on target X").
    pub key: String,
    /// The target, when the draft belongs to one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<TargetId>,
    /// The thread, when the draft belongs to one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<ThreadId>,
    /// The text.
    pub text: String,
    /// Attachments, as the window describes them.
    #[serde(default)]
    pub attachments: Vec<Value>,
    /// Last change, milliseconds since the Unix epoch (set by the hub).
    #[serde(default)]
    pub updated_at_ms: i64,
}

/// `hub/drafts/list` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftsListResult {
    /// Every draft.
    pub drafts: Vec<Draft>,
}

/// `hub/drafts/delete` params.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftKey {
    /// The draft's key.
    pub key: String,
}

/// What the hub knows about a state-changing operation it routed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OperationState {
    /// Sent; no answer yet.
    Sent,
    /// The connection dropped before the answer: it may or may not have taken effect. Shown
    /// to the user; never re-sent automatically.
    Uncertain,
}

/// A routed operation still in flight (or uncertain). Its params are never stored: they may
/// hold a secret answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Operation {
    /// The operation id.
    pub op_id: OpId,
    /// The target.
    pub target: TargetId,
    /// The method.
    pub method: String,
    /// The thread, when the params named one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<ThreadId>,
    /// State.
    pub state: OperationState,
    /// When it was sent, milliseconds since the Unix epoch.
    pub created_at_ms: i64,
}

/// `hub/operations/list` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationsListResult {
    /// Operations.
    pub operations: Vec<Operation>,
}

/// `hub/operations/dismiss` params.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationRef {
    /// The operation.
    pub op_id: OpId,
}

/// `hub/threads/lookup` params.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LookupParams {
    /// The YAAC thread id or the harness's own conversation id.
    pub id: String,
    /// Only this target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<TargetId>,
}

/// One thread found by `hub/threads/lookup`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LookupHit {
    /// Its target.
    pub target: TargetId,
    /// Its summary (it is now in that target's index, and so in the cache).
    pub thread: ThreadSummary,
}

/// `hub/threads/lookup` result.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LookupResult {
    /// What was found.
    pub threads: Vec<LookupHit>,
    /// Targets that could not be asked (offline, or the lookup failed).
    #[serde(default)]
    pub unreachable: Vec<TargetId>,
}

/// One step of a target's connection, for the diagnostics view.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionStep {
    /// When, milliseconds since the Unix epoch.
    pub at_ms: i64,
    /// What: `connecting`, `opened`, `negotiated`, `ready`, `failed`, `closed`, `incompatible`,
    /// `disabled`, `offline`.
    pub step: String,
    /// Detail, shown as-is (a reason, the daemon's build). Never message content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// `hub/targets/log` result: oldest first, at most the last 50 steps.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetLogResult {
    /// The steps.
    pub steps: Vec<ConnectionStep>,
}

/// One thread's view preferences, kept by the hub on this computer (D92): user data, in the
/// durable store beside drafts, never in the disposable cache and never on the target. A
/// thread with none has no entry.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadView {
    /// The target.
    pub target: TargetId,
    /// The thread.
    pub thread_id: ThreadId,
    /// Pinned.
    #[serde(default)]
    pub pinned: bool,
    /// Muted.
    #[serde(default)]
    pub muted: bool,
    /// A timed mute ends then (RFC 3339). The hub keeps no timer: the window compares it with
    /// the time when it draws.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub muted_until: Option<String>,
    /// Dismissed: hidden by the window. The target keeps indexing it; search still finds it.
    #[serde(default)]
    pub dismissed: bool,
}

impl ThreadView {
    /// Whether the entry says anything (an empty one is removed).
    pub fn is_empty(&self) -> bool {
        !self.pinned && !self.muted && !self.dismissed && self.muted_until.is_none()
    }

    /// Kept by the user: the target's discovery caps must not cut it (`thread/keep`).
    pub fn keeps(&self) -> bool {
        self.pinned || self.muted
    }
}

/// `hub/view/get` result (also the `hub/view/setMany` result: the entries changed).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewGetResult {
    /// Every thread that has a preference.
    pub views: Vec<ThreadView>,
}

/// `hub/view/set` params: a patch of one thread's view preferences.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewSetParams {
    /// The target.
    pub target: TargetId,
    /// The thread.
    pub thread_id: ThreadId,
    /// Pin or unpin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned: Option<bool>,
    /// Mute or unmute (`false` also clears `mutedUntil`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub muted: Option<bool>,
    /// With `muted: true`: when the mute ends; `null`: until unmuted.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "double_option")]
    pub muted_until: Option<Option<String>>,
    /// Dismiss or restore.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dismissed: Option<bool>,
}

/// `hub/view/set` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewSetResult {
    /// The entry after the change.
    pub view: ThreadView,
}

/// `hub/view/setMany` params.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewSetManyParams {
    /// The changes.
    pub entries: Vec<ViewSetParams>,
}

/// `hub/view/changed` params: the entries as they are now (an empty one was cleared).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewChangedParams {
    /// The changed entries.
    pub views: Vec<ThreadView>,
}

/// Absent = `None`, `null` = `Some(None)`, a value = `Some(Some(value))`.
mod double_option {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub(super) fn serialize<T: Serialize, S: Serializer>(value: &Option<Option<T>>, serializer: S) -> Result<S::Ok, S::Error> {
        match value {
            Some(inner) => inner.serialize(serializer),
            None => serializer.serialize_none(),
        }
    }

    pub(super) fn deserialize<'de, T: Deserialize<'de>, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Option<T>>, D::Error> {
        Option::<T>::deserialize(deserializer).map(Some)
    }
}
