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
use yaac_protocol::rpc::ErrorObject;
use yaac_protocol::{ClientId, JournalPosition, OpId, Sha256Hex, TargetId, ThreadId, ThreadSummary, UploadId};

use crate::settings::HubSettings;

mod ssh;
pub use ssh::*;

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
    /// Window → hub: which threads this window shows now ([`ViewShowingParams`] → `{}`). The
    /// hub marks a thread `unseen` when its turn ends while no window shows it (D114).
    pub const VIEW_SHOWING: &str = "hub/view/showing";
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
    /// Window → hub: reconnect a target now (user request), skipping the backoff. Never
    /// starts a stopped WSL distro.
    pub const TARGETS_RECONNECT: &str = "hub/targets/reconnect";
    /// Window → hub: **Start it** — connect a target whose machine is not running (status
    /// `stopped`), which boots the WSL distro. `{ id }`; answered `{}` once the target is
    /// connected, or with an error carrying the reason (code `start_failed`).
    pub const TARGETS_START: &str = "hub/targets/start";
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

    // ── Uploads (docs/06-attachments.md) — see [`super::UploadStartParams`] ─────────────
    /// Window → hub: upload a local file, or a draft's stored clipboard bytes, to a target
    /// ([`super::UploadStartParams`] → [`super::UploadStartResult`]). The hub reads and hashes
    /// the file itself; progress and the outcome arrive as notifications.
    pub const UPLOAD_START: &str = "hub/upload/start";
    /// Window → hub: stop an upload ([`super::UploadHandleParams`] → `{}`). The outcome is
    /// still reported by `hub/upload/done`, with the error `upload_cancelled`.
    pub const UPLOAD_CANCEL: &str = "hub/upload/cancel";
    /// Window → hub: every unfinished upload ([`super::UploadListResult`]), for a window that
    /// attached while uploads were running.
    pub const UPLOAD_LIST: &str = "hub/upload/list";
    /// Hub → every window: an upload's progress ([`super::UploadProgressParams`]).
    pub const UPLOAD_PROGRESS: &str = "hub/upload/progress";
    /// Hub → every window: an upload finished, failed, or was cancelled
    /// ([`super::UploadDoneParams`]). Sent exactly once per handle.
    pub const UPLOAD_DONE: &str = "hub/upload/done";
    /// Window → hub: start storing bytes that exist only in the window's memory (a clipboard
    /// image) in the durable draft store ([`super::BlobBeginParams`] → [`super::BlobBeginResult`]).
    pub const BLOB_BEGIN: &str = "hub/drafts/blob/begin";
    /// Window → hub: the next bounded chunk of a blob ([`super::BlobAppendParams`] →
    /// [`super::BlobAppendResult`]). At most [`super::BLOB_CHUNK_MAX`] bytes before encoding.
    pub const BLOB_APPEND: &str = "hub/drafts/blob/append";
    /// Window → hub: all bytes sent ([`super::BlobRef`] → [`super::DraftBlob`]).
    pub const BLOB_END: &str = "hub/drafts/blob/end";
    /// Window → hub: the stored blobs, of one draft or all ([`super::BlobListParams`] →
    /// [`super::BlobListResult`]).
    pub const BLOB_LIST: &str = "hub/drafts/blob/list";
    /// Window → hub: forget a blob (its chip was removed) ([`super::BlobRef`] → `{}`).
    pub const BLOB_DELETE: &str = "hub/drafts/blob/delete";

    // ── SSH setup, verify, and revoke (docs/02-architecture.md#setting-up-an-ssh-connection) ──
    // Each runs short `ssh` sessions from the hub, one after another, and only while the
    // server's target has no live connection (refused with `target_connected` otherwise).
    /// Window → hub: contact the server and read its host key ([`super::SshConnParams`] →
    /// [`super::SshHostKey`]). Unreachable: code `ssh_unreachable`.
    pub const SSH_REACH: &str = "hub/ssh/reach";
    /// Window → hub: the user confirmed this host key; trust it (in YAAC's own known-hosts
    /// file) and try signing in with no password ([`super::SshProbeParams`] →
    /// [`super::SshProbeResult`]).
    pub const SSH_PROBE: &str = "hub/ssh/probe";
    /// Window → hub: sign in once with a password or a key file, generate YAAC's own key,
    /// and append its public half to `authorized_keys` ([`super::SshSignInParams`] →
    /// [`super::SshInstalledKey`]). The password is used once and wiped; the session stays
    /// open until `verify` or `removeKey` decides whether the line stays.
    pub const SSH_SIGN_IN: &str = "hub/ssh/signIn";
    /// Window → hub: sign in again with YAAC's key alone ([`super::SshConnParams`] → `{}`).
    /// During setup, success keeps the line; on its own, it checks that password-less access
    /// still works.
    pub const SSH_VERIFY: &str = "hub/ssh/verify";
    /// Window → hub: after a failed verify, remove the line just added and confirm it is gone
    /// ([`super::SshConnParams`] → `{}`).
    pub const SSH_REMOVE_KEY: &str = "hub/ssh/removeKey";
    /// Window → hub: **Revoke** — close the target's connection, remove exactly YAAC's line
    /// (matched by the key itself), and delete YAAC's key ([`super::SshConnParams`] →
    /// [`super::SshRevokeResult`]). Refused with `nothing_to_revoke` when YAAC installed
    /// nothing; a no-op when it was already revoked. Unreachable: code `ssh_unreachable`,
    /// and the key is kept.
    pub const SSH_REVOKE: &str = "hub/ssh/revoke";
    /// Window → hub: the user removed YAAC's line by hand; delete YAAC's key
    /// ([`super::SshConnParams`] → `{}`).
    pub const SSH_REMOVED_BY_HAND: &str = "hub/ssh/removedByHand";
    /// Window → hub: what YAAC did on each SSH server ([`super::SshRecordsResult`]).
    pub const SSH_RECORDS: &str = "hub/ssh/records";
    /// Hub → window notification: the SSH records changed; ask `hub/ssh/records` again.
    pub const SSH_CHANGED: &str = "hub/ssh/changed";
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
    /// The upload was cancelled (`hub/upload/cancel`).
    pub const UPLOAD_CANCELLED: &str = "upload_cancelled";
    /// The hub could not read the file to upload (missing, a directory, no permission), or it
    /// changed while it was being uploaded.
    pub const FILE_UNREADABLE: &str = "file_unreadable";
    /// No such draft blob, or it is not complete.
    pub const BLOB_NOT_FOUND: &str = "blob_not_found";
    /// `hub/targets/start` did not end connected; the message says why.
    pub const START_FAILED: &str = "start_failed";
    /// An SSH setup step was asked for while the server's target is connected (or another
    /// setup step for it is running).
    pub const TARGET_CONNECTED: &str = "target_connected";
    /// The SSH server could not be reached (or no longer accepts YAAC's key, for Revoke).
    pub const SSH_UNREACHABLE: &str = "ssh_unreachable";
    /// An SSH setup step failed; the message is the plain-words reason or the SSH error.
    pub const SSH_FAILED: &str = "ssh_failed";
    /// Revoke on a server where YAAC installed nothing.
    pub const NOTHING_TO_REVOKE: &str = "nothing_to_revoke";
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
    /// Uploads not yet finished.
    #[serde(default)]
    pub uploads: u32,
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
    /// A turn ended while no window showed the thread (D114). Set by the hub; cleared by the
    /// window that opens it.
    #[serde(default)]
    pub unseen: bool,
}

impl ThreadView {
    /// Whether the entry says anything (an empty one is removed).
    pub fn is_empty(&self) -> bool {
        !self.pinned && !self.muted && !self.dismissed && !self.unseen && self.muted_until.is_none()
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
    /// Mark or clear "finished while nobody looked" (D114).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unseen: Option<bool>,
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

/// One thread a window shows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShownThread {
    /// The target.
    pub target: TargetId,
    /// The thread.
    pub thread_id: ThreadId,
}

/// `hub/view/showing` params: everything this window shows now (replaces the last report).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewShowingParams {
    /// The threads on screen.
    #[serde(default)]
    pub threads: Vec<ShownThread>,
}

/// `hub/view/changed` params: the entries as they are now (an empty one was cleared).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewChangedParams {
    /// The changed entries.
    pub views: Vec<ThreadView>,
}

// ── Uploads ───────────────────────────────────────────────────────────────────────────────
//
// The window never opens a connection to a target and never puts a file into a JSON message
// (docs/06-attachments.md). It names the file; the hub, on the same computer, reads it,
// hashes it on its worker pool, and sends it on the target's one connection as bulk data
// under credit, behind control traffic. Bytes that exist only in the window's memory (a
// clipboard image) are first stored in the hub's durable draft store as a *blob*, in bounded
// chunks, and uploaded from there — so they survive a hub restart and can be sent again when
// the draft's target changes.
//
// ```text
// hub/drafts/blob/begin { draft, name, size, mediaType? }      → { blobId }      (clipboard only)
// hub/drafts/blob/append { blobId, offset, dataBase64 } …      → { received }
// hub/drafts/blob/end { blobId }                               → DraftBlob
// hub/upload/start { target, draft?, path | blobId, name?, mediaType? } → { handle }
//   ← hub/upload/progress { handle, target, draft?, uploadId?, phase, sent, total } …
//   ← hub/upload/done { handle, target, draft?, uploadId?, name, size, mediaType?, sha256?, skipped, error? }
// turn/start { input: [ { type: "image" | "file", uploadId, name, mediaType } … ] } (hub/request)
// ```
//
// An `uploadId` is only meaningful on the target that issued it: when a draft's target
// changes, the window calls `hub/upload/start` again for the new target (content addressing
// makes a repeat cheap: `skipped` says no bytes were sent).

/// The largest chunk `hub/drafts/blob/append` accepts, before base64.
pub const BLOB_CHUNK_MAX: usize = 512 * 1024;

/// `hub/upload/start` params. Exactly one of `path` and `blobId`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadStartParams {
    /// The target to upload to.
    pub target: TargetId,
    /// The window's key for the composer the attachment belongs to (as in [`Draft::key`]).
    /// Carried back in progress and done notifications; nothing else.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft: Option<String>,
    /// An absolute path to a file on this computer. The hub reads it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// A blob in the draft store (`hub/drafts/blob/*`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob_id: Option<String>,
    /// The name the agent sees. Default: the file's name, or the blob's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The media type, when the window knows better than the hub's sniffing (which recognises
    /// PNG, JPEG, GIF, WebP and PDF by content). The target sniffs again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
}

/// `hub/upload/start` result: the hub's handle for this upload. Errors found later (the file
/// is unreadable, too large, the target dropped) arrive in `hub/upload/done`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadStartResult {
    /// The handle (`hu_…`), unique within this hub's run.
    pub handle: String,
}

/// `hub/upload/cancel` params.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadHandleParams {
    /// The handle.
    pub handle: String,
}

/// Where an upload is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UploadPhase {
    /// The hub is reading the file to compute its SHA-256.
    Hashing,
    /// Waiting for a free upload slot on the target.
    Queued,
    /// `upload/begin` sent.
    Beginning,
    /// Sending bytes.
    Sending,
    /// All bytes sent; the target is verifying them (`upload/end`).
    Verifying,
}

/// `hub/upload/progress` params (also each entry of `hub/upload/list`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadProgressParams {
    /// The handle.
    pub handle: String,
    /// The target.
    pub target: TargetId,
    /// The draft, as given to `hub/upload/start`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft: Option<String>,
    /// The target's id for the upload, once `upload/begin` has answered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upload_id: Option<UploadId>,
    /// The phase.
    pub phase: UploadPhase,
    /// Bytes handed to the connection so far.
    pub sent: u64,
    /// The file's size (0 until it is known).
    pub total: u64,
}

/// `hub/upload/list` result.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadListResult {
    /// Unfinished uploads, oldest first.
    pub uploads: Vec<UploadProgressParams>,
}

/// `hub/upload/done` params: exactly one per handle.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadDoneParams {
    /// The handle.
    pub handle: String,
    /// The target.
    pub target: TargetId,
    /// The draft, as given to `hub/upload/start`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft: Option<String>,
    /// On success: the id to put in `image` and `file` input parts on this target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upload_id: Option<UploadId>,
    /// The name sent (the target sanitises it; `name` in the journaled part is the final one).
    pub name: String,
    /// Bytes.
    pub size: u64,
    /// The media type the hub determined (by content), or the window's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    /// The content's full SHA-256, once computed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<Sha256Hex>,
    /// The target already had this content: no bytes were sent.
    #[serde(default)]
    pub skipped: bool,
    /// Why it failed: `limit_exceeded`, `file_unreadable`, `blob_not_found`, `target_offline`,
    /// `upload_mismatch`, `upload_cancelled`, or a target's error passed through. The message
    /// is meant to be shown as-is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorObject>,
}

/// `hub/drafts/blob/begin` params.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlobBeginParams {
    /// The draft (composer key) it belongs to; deleting the draft deletes its blobs.
    pub draft: String,
    /// The name it will be uploaded under (e.g. `screenshot-2026-11-20-1405.png`).
    pub name: String,
    /// The media type, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    /// Total bytes that will be appended.
    pub size: u64,
}

/// `hub/drafts/blob/begin` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlobBeginResult {
    /// The blob (`b_…`).
    pub blob_id: String,
}

/// `hub/drafts/blob/append` params. Chunks must arrive in order (`offset` = bytes so far).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlobAppendParams {
    /// The blob.
    pub blob_id: String,
    /// Where this chunk starts.
    pub offset: u64,
    /// The bytes, standard base64 with padding; at most [`BLOB_CHUNK_MAX`] before encoding.
    pub data_base64: String,
}

impl BlobAppendParams {
    /// Splits `bytes` into append requests of at most [`BLOB_CHUNK_MAX`] bytes each.
    pub fn chunks<'a>(blob_id: &'a str, bytes: &'a [u8]) -> impl Iterator<Item = BlobAppendParams> + 'a {
        use base64::Engine as _;
        bytes.chunks(BLOB_CHUNK_MAX).enumerate().map(move |(i, chunk)| BlobAppendParams {
            blob_id: blob_id.to_owned(),
            offset: (i * BLOB_CHUNK_MAX) as u64,
            data_base64: base64::engine::general_purpose::STANDARD.encode(chunk),
        })
    }
}

/// `hub/drafts/blob/append` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlobAppendResult {
    /// Bytes stored so far.
    pub received: u64,
}

/// `hub/drafts/blob/end` and `hub/drafts/blob/delete` params.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlobRef {
    /// The blob.
    pub blob_id: String,
}

/// A complete blob in the draft store (`hub/drafts/blob/end` result).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftBlob {
    /// The blob.
    pub blob_id: String,
    /// Its draft.
    pub draft: String,
    /// Its name.
    pub name: String,
    /// Its media type, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    /// Bytes.
    pub size: u64,
    /// Full SHA-256.
    pub sha256: Sha256Hex,
}

/// `hub/drafts/blob/list` params.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlobListParams {
    /// Only this draft's blobs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft: Option<String>,
}

/// `hub/drafts/blob/list` result.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlobListResult {
    /// Complete blobs, oldest first.
    pub blobs: Vec<DraftBlob>,
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
