//! The attach protocol: window ⇄ hub (docs/10-agent-mesh.md#the-hub).
//!
//! The window and the hub are always the same build — they exchange builds in `hub/hello`,
//! and a newer window replaces an older hub (`hub/shutdown`) while an older window is told to
//! update — so this protocol has **no dialects and no versions**. It uses the same frames and
//! JSON-RPC shapes as the YAAC protocol ([`crate::frame`], [`crate::rpc::Message`]).
//!
//! YAAC protocol traffic to and from a target travels in a routing envelope: the window sends
//! `hub/request` naming a target; the hub sends the inner request on that target's
//! connection (through that connection's dialect) and answers with the target's result in
//! current shape. Notifications from a target reach the window as `hub/notification`.
//!
//! This is deliberately minimal; the hub and window packages refine it.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::macros::{open_enum, open_union};
use crate::types::{ClientId, Empty, TargetId, TargetName};

/// The UI state key for submitted composer messages.
pub const COMPOSER_HISTORY: &str = "composerHistory";
/// The most recent messages eligible for composer recall.
pub const COMPOSER_HISTORY_LIMIT: usize = 10;
/// The maximum size in UTF-8 bytes of a message eligible for composer recall.
pub const COMPOSER_HISTORY_ENTRY_BYTES: usize = 16 * 1024;

/// Keep recall exact and bound both saved and legacy history.
pub fn history_entry(text: &str) -> bool {
    text.len() <= COMPOSER_HISTORY_ENTRY_BYTES && !text.trim().is_empty()
}

/// Read the newest valid entries from a stored history list.
pub fn history_entries(value: &Value) -> impl Iterator<Item = &str> {
    value.as_array().into_iter().flatten().filter_map(Value::as_str)
        .filter(|text| history_entry(text)).take(COMPOSER_HISTORY_LIMIT)
}

/// Append one newly submitted message to the hub's stored composer history.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComposerHistoryPush {
    pub op_id: crate::OpId,
    pub text: String,
}

/// A pending request as identified across targets and windows.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestLocation {
    pub target: TargetId,
    pub thread_id: crate::ThreadId,
    pub request_id: crate::RequestId,
}

/// Destination for a desktop action. A rejected or uncertain answer opens the card with
/// the original error so that no answer is silently retried.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationOpen {
    pub location: RequestLocation,
    pub item_id: crate::ItemId,
    pub error: Option<String>,
}

/// The hub's request reminder as a list, for a client that draws its own notifications (the
/// Android client, P355): exactly the unseen requests the desktop reminder counts, oldest
/// first, with the decisions the toast offers. Sent as `hub/notifications/requests` to clients
/// that asked with `hub/notifications/subscribe`, and as that method's answer.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestFeed {
    /// Changes whenever the list or an entry changes; an answer names the revision it saw.
    pub revision: u64,
    pub items: Vec<RequestFeedItem>,
}

/// One unseen request of a [`RequestFeed`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestFeedItem {
    pub location: RequestLocation,
    /// The request's journal item (where Open lands).
    pub item_id: crate::ItemId,
    /// The thread's title as the hub knows it (a worker that is no list row included).
    pub worker: String,
    /// `Approval`, `Question`, `Form`, `Login` or `Request`.
    pub kind: String,
    /// A short title (at most 64 characters).
    pub title: String,
    /// Present only for a tool approval with a plain allow and deny.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decisions: Option<FeedDecisions>,
    /// When the request was made (Unix milliseconds).
    pub created_ms: i64,
}

/// How the request reminder names a pending request: its kind (`Approval`, `Question`,
/// `Form`, `Login`, `Request`), a one-line title of at most 64 characters, and the ids of a
/// plain allow and deny when it offers both (an approval the toast and the feed can answer).
/// Shared by the hub and the demo hub so both describe a request alike.
pub fn reminder_entry(r: &crate::RequestItem) -> (&'static str, String, Option<(String, String)>) {
    use crate::{DecisionKind, RequestBody};
    let (kind, title, decisions) = match &r.body {
        RequestBody::Approval(a) => {
            let plain = |kind| a.decisions.iter().find(|d| d.kind == kind && !d.accepts_reason).map(|d| d.id.clone());
            let title = if a.details.as_ref().and_then(|d| d["kind"].as_str()) == Some("writeStdin") {
                "Send input to a running command".into()
            } else if a.command.is_some() {
                "Run a command".into()
            } else {
                a.title.clone()
            };
            ("Approval", title, plain(DecisionKind::Allow).zip(plain(DecisionKind::Deny)))
        }
        RequestBody::Form(form) if form.schema.as_ref().is_some_and(|s| s["url"].is_string()) => ("Login", form.message.clone(), None),
        RequestBody::Form(f) => ("Form", f.message.clone(), None),
        RequestBody::Question(q) => ("Question", q.questions.first().map_or_else(|| "Agent question".into(), |q| q.text.clone()), None),
        _ => ("Request", "Input needed".into(), None),
    };
    (kind, one_line(&title, 64), decisions)
}

/// `value` on one line of at most `limit` characters (whitespace collapsed, `…` when cut).
pub fn one_line(value: &str, limit: usize) -> String {
    let mut words = value.split_whitespace();
    let first = words.next().unwrap_or_default();
    let mut chars = first.chars().chain(words.flat_map(|word| std::iter::once(' ').chain(word.chars())));
    let mut short: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() {
        short.pop();
        short.push('…');
    }
    short
}

/// The decision ids behind a feed entry's Allow once and Deny.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedDecisions {
    pub allow: String,
    pub deny: String,
}

/// `hub/notifications/respond`: what the user did on one feed entry. With no decision it is
/// the toast's Open (the entry is marked seen; the client opens it). Refused when `revision`
/// is not the current one, so a list that changed under the user's finger is never answered.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedRespondParams {
    pub revision: u64,
    pub location: RequestLocation,
    /// `allow` or `deny`; absent for Open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<String>,
}

/// Hub-owned desktop notification preferences.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct NotificationSettings {
    pub enabled: bool,
    pub sound: bool,
    pub sound_choice: NotificationSound,
    pub include_questions: bool,
}

impl Default for NotificationSettings {
    fn default() -> Self {
        Self { enabled: true, sound: true, sound_choice: NotificationSound::Reminder, include_questions: true }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NotificationSound { Reminder, Default }

/// Every attach method name.
pub mod names {
    /// Window → hub. The pending cards actually visible in this focused window.
    pub const NOTIFICATIONS_VISIBLE: &str = "hub/notifications/visible";
    /// Hub → window. Open a notification at its exact request.
    pub const NOTIFICATIONS_OPEN: &str = "hub/notifications/open";
    /// Window → hub. Receive the request reminder's list ([`super::RequestFeed`]) now and
    /// whenever it changes.
    pub const NOTIFICATIONS_SUBSCRIBE: &str = "hub/notifications/subscribe";
    /// Hub → window. The request reminder's list changed ([`super::RequestFeed`]).
    pub const NOTIFICATIONS_REQUESTS: &str = "hub/notifications/requests";
    /// Window → hub. Answer or open one feed entry ([`super::FeedRespondParams`]).
    pub const NOTIFICATIONS_RESPOND: &str = "hub/notifications/respond";
    /// Terminal open on the selected target.
    pub const TERMINAL_OPEN: &str = "hub/terminal/open";
    /// Terminal attach on the selected target.
    pub const TERMINAL_ATTACH: &str = "hub/terminal/attach";
    /// Terminal resize on the selected target.
    pub const TERMINAL_RESIZE: &str = "hub/terminal/resize";
    /// Terminal close on the selected target.
    pub const TERMINAL_CLOSE: &str = "hub/terminal/close";
    /// Terminal shells on the selected target.
    pub const TERMINAL_SHELLS: &str = "hub/terminal/shells";
    /// Terminal list on the selected target.
    pub const TERMINAL_LIST: &str = "hub/terminal/list";
    /// Terminal title on the selected target.
    pub const TERMINAL_TITLE: &str = "hub/terminal/title";
    /// Terminal exited on the selected target.
    pub const TERMINAL_EXITED: &str = "hub/terminal/exited";
    /// Terminal rename on the selected target (P343).
    pub const TERMINAL_RENAME: &str = "hub/terminal/rename";
    /// Terminal renamed on the selected target (P343).
    pub const TERMINAL_RENAMED: &str = "hub/terminal/renamed";
    /// Window → hub. Start a dictation for a composer ([`HubVoiceStartParams`] →
    /// [`HubVoiceStartResult`]): the hub chooses the transcriber
    /// ([`crate::transcription::choose`]) and answers with it and a hub-local stream. Audio
    /// follows as data frames on that stream, under credit (docs/plan/voice.md).
    pub const VOICE_START: &str = "hub/voice/start";
    /// Window → hub. No more audio: the final transcript and `hub/voice/ended` follow.
    pub const VOICE_STOP: &str = "hub/voice/stop";
    /// Window → hub. Drop the dictation and its words; `hub/voice/ended` follows.
    pub const VOICE_CANCEL: &str = "hub/voice/cancel";
    /// Hub → the window that started it. A dictation's whole text so far.
    pub const VOICE_TRANSCRIPT: &str = "hub/voice/transcript";
    /// Hub → the window that started it. The dictation is over; its stream is closed.
    pub const VOICE_ENDED: &str = "hub/voice/ended";
    /// Window → hub. Start a voice call ([`super::TargetVoiceCall`]`<VoiceCallStartParams>` →
    /// `VoiceCallStartResult`; docs/plan/voice-call.md).
    pub const VOICE_CALL_START: &str = "hub/voiceCall/start";
    /// Window → hub. End a voice call; `hub/voiceCall/state ended` follows.
    pub const VOICE_CALL_END: &str = "hub/voiceCall/end";
    /// Hub → the starting window. The SDP answer.
    pub const VOICE_CALL_ANSWER: &str = "hub/voiceCall/answer";
    /// Hub → the starting window. The call's state; `ended` is the last message.
    pub const VOICE_CALL_STATE: &str = "hub/voiceCall/state";
    /// Hub → the starting window. What is being said.
    pub const VOICE_CALL_TRANSCRIPT: &str = "hub/voiceCall/transcript";
    /// Read the hub-owned skill catalog.
    pub const CATALOG_SKILLS: &str = "hub/catalog/skills";
    /// Create or edit a user-owned skill.
    pub const CATALOG_SKILL_SAVE: &str = "hub/catalog/skill/save";
    /// Remove a user-owned skill or override.
    pub const CATALOG_SKILL_REMOVE: &str = "hub/catalog/skill/remove";
    /// A catalog edit was committed.
    pub const CATALOG_SKILLS_CHANGED: &str = "hub/catalog/skillsChanged";
    /// Hub → window theme request.
    pub const THEME_CALL: &str = "hub/theme/call";
    /// Hub → window browser request.
    pub const BROWSER_CALL: &str = "hub/browser/call";
    /// Preview listener management.
    pub const PREVIEW_PROXY: &str = "hub/preview/proxy";
    /// Window → hub. A tunnel to the target over this attach connection (P380).
    pub const PREVIEW_TUNNEL: &str = "hub/preview/tunnel";
    /// Either way. A tunnel over the attach connection closed.
    pub const PREVIEW_TUNNEL_CLOSE: &str = "hub/preview/tunnelClose";
    /// Preview listener management.
    pub const PREVIEW_PROXY_CLOSE: &str = "hub/preview/proxyClose";
    /// Preview listener management.
    pub const PREVIEW_OPEN: &str = "hub/preview/open";
    /// Preview listener management.
    pub const PREVIEW_CLOSE: &str = "hub/preview/close";
    /// Preview listener management.
    pub const PREVIEW_LIST: &str = "hub/preview/list";
    /// Window → hub, first message: exchange builds.
    pub const HELLO: &str = "hub/hello";
    /// Window → hub: store one submission in the shared composer history.
    pub const COMPOSER_HISTORY_PUSH: &str = "hub/composer/historyPush";
    /// Window → hub: ask an older hub to checkpoint its relay queue and exit.
    pub const SHUTDOWN: &str = "hub/shutdown";
    /// Client over Tailscale → hub, frozen: install a newer YAAC build on the hub's computer
    /// ([`super::HostUpdateParams`] → [`super::HostUpdateResult`]).
    pub const UPDATE_REQUEST: &str = "hub/update/request";
    /// Hub → clients, frozen: [`super::HostUpdateProgress`].
    pub const UPDATE_PROGRESS: &str = "hub/update/progress";
    /// Window → hub: list targets.
    pub const TARGETS_LIST: &str = "hub/targets/list";
    /// Window → hub: add a target.
    pub const TARGETS_ADD: &str = "hub/targets/add";
    /// Window → hub: change a target.
    pub const TARGETS_UPDATE: &str = "hub/targets/update";
    /// Window → hub: remove a target.
    pub const TARGETS_REMOVE: &str = "hub/targets/remove";
    /// Window → hub: enable or disable a target.
    pub const TARGETS_ENABLE: &str = "hub/targets/enable";
    /// Window → hub: **Update now** — ask the target's older `yaacd` to hand over at once
    /// ([`TargetsUpdateDaemonParams`] → [`TargetsUpdateDaemonResult`]). Running turns are
    /// interrupted, marked as interrupted by an update; the hub reconnects through the normal
    /// path and the newest installed build serves. Refused unless the hub holds a newer build
    /// for that target ([`DaemonSummary::can_update`]).
    pub const TARGETS_UPDATE_DAEMON: &str = "hub/targets/updateDaemon";
    /// Hub → window notification: a target's connection state changed.
    pub const TARGET_STATE: &str = "hub/target/state";
    /// Window → hub: carry a YAAC request to a target; answered with the target's result.
    pub const REQUEST: &str = "hub/request";
    /// Window → hub: resize a target upload for inline display.
    /// See [`super::HubUploadPreviewParams`] and [`super::HubUploadPreviewResult`].
    pub const UPLOAD_PREVIEW: &str = "hub/upload/preview";
    /// Either way: carry a YAAC notification to or from a target.
    pub const NOTIFICATION: &str = "hub/notification";
    /// Window → hub: the plan rate limits of every account used on any target, one entry per
    /// account (D161) ([`RateLimitsGetParams`] → [`RateLimitsResult`]).
    pub const RATE_LIMITS_GET: &str = "hub/rateLimits/get";
    /// Hub → window notification: those accounts changed ([`RateLimitsResult`]).
    pub const RATE_LIMITS_CHANGED: &str = "hub/rateLimits/changed";
}

/// Largest edge the hub will produce for an inline upload preview.
pub const UPLOAD_PREVIEW_MAX_EDGE: u32 = 220;
/// Largest PNG the hub will send for an inline upload preview, before base64.
pub const UPLOAD_PREVIEW_MAX_BYTES: usize = 72 * 1024;

/// `hub/upload/preview`: a thumbnail of an image stored on a target.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HubUploadPreviewParams {
    pub target: TargetId,
    pub upload_id: crate::UploadId,
    /// Upper bound on the longer edge, in pixels. Must be positive.
    pub max_edge: u32,
}

/// `hub/upload/preview` result, sent in the bulk queue.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HubUploadPreviewResult {
    /// PNG bytes, standard base64 with padding.
    pub png_base64: String,
}

/// Terminal request or notification on the window–hub link.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TargetTerminal<P> {
    /// Selected target.
    pub target: TargetId,
    /// Corresponding target-side terminal payload.
    #[serde(flatten)]
    pub params: P,
}

/// Voice request or notification on the window–hub link: the target-side payload with the
/// target beside it, and the stream renumbered to the window's hub-local one. The target is the
/// transcriber's ([`HubVoiceStartResult::target`]), not necessarily the composer's.
pub type TargetVoice<P> = TargetTerminal<P>;

/// `hub/voice/start` params: the composer the words are for. The hub chooses who transcribes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HubVoiceStartParams {
    /// Operation identity, passed on to the target's `voice/start`.
    pub op_id: crate::OpId,
    /// The composer's target, when it has one (a new-thread draft may not).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<TargetId>,
    /// The composer's instance: the thread's, or the one a draft chose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<crate::InstanceId>,
    /// The composer's thread, when it is a thread's (context only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<crate::ThreadId>,
    /// The spoken language as a code, when the user chose one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

/// `hub/voice/start` result: who transcribes, and the audio stream.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HubVoiceStartResult {
    /// The transcriber's target: stop, cancel and the notifications name this one.
    pub target: TargetId,
    /// The transcribing instance.
    pub instance: crate::InstanceId,
    /// "Claude on wsl: Ubuntu".
    pub label: String,
    /// The target's answer, with the stream renumbered to a hub-local one.
    #[serde(flatten)]
    pub opened: crate::VoiceStartResult,
}

/// Voice call request or notification on the window–hub link: the target-side payload with
/// the target beside it (`callId` unchanged).
pub type TargetVoiceCall<P> = TargetTerminal<P>;

/// `hub/hello` params.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HelloParams {
    /// The window's build. If it differs from the hub's, the two are not the same build: a
    /// newer window replaces the hub, an older one tells the user to update.
    pub build: String,
}

/// `hub/hello` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HelloResult {
    /// The hub's build.
    pub build: String,
    /// The targets and their connection states.
    pub targets: Vec<TargetEntry>,
    /// This window's client id: the id the hub presents to every target, so the window can
    /// recognise its own answers ("answered by you") and its own turns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<ClientId>,
    /// This hub install's stable id (kept in its `hub.json`), so a client that saved two
    /// addresses of one hub can tell they are the same hub (docs/14-android.md#several-hubs).
    /// Absent from an older hub: the client then knows the hub by its address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hub_id: Option<String>,
    /// The hub machine's name (on Windows `COMPUTERNAME`), to label the hub. Absent from an
    /// older hub.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_name: Option<String>,
}

// ── Updating the hub's computer from a newer client (frozen) ────────────────────────────────
//
// `hub/update/request` and `hub/update/progress` belong to the attach protocol's frozen core
// with `hub/hello` and `hub/shutdown` (docs/09-compatibility.md#the-frozen-core): any hub,
// forever, accepts them from a client of another build, so a phone that is newer than the PC
// can bring the PC up to its build. Fields are only ever added; unknown `phase` values are
// shown as they are.

/// `hub/update/request`: install YAAC `build` on the hub's computer (window, hub and the
/// `yaacd` bundle). Only from a client admitted over Tailscale, only for a build newer than
/// the hub's, and only one attempt per request: an uncertain outcome is never sent again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostUpdateParams {
    /// The build to install, `<version>+<id>` (normally the client's own).
    pub build: String,
    /// One per tap. The same id again while that attempt runs is answered with its state.
    pub op_id: crate::OpId,
}

/// `hub/update/request` result: the release is published and carries an installer; the
/// download has started. What follows arrives as `hub/update/progress`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostUpdateResult {
    /// The build being installed.
    pub build: String,
    /// The installer's size in bytes.
    pub size: u64,
}

/// Hub → every client that sent `hub/hello`, whatever its build: how an update of the hub's
/// computer stands.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostUpdateProgress {
    pub op_id: crate::OpId,
    pub build: String,
    /// `checking` · `downloading` · `installing` (the installer is running: the hub, the
    /// window and this connection stop shortly) · `failed`. Open: show others as they are.
    pub phase: String,
    #[serde(default)]
    pub done: u64,
    #[serde(default)]
    pub total: u64,
    /// Why it failed (`failed`), a sentence for the user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// The phases of [`HostUpdateProgress`].
pub mod host_update {
    pub const CHECKING: &str = "checking";
    pub const DOWNLOADING: &str = "downloading";
    pub const INSTALLING: &str = "installing";
    pub const FAILED: &str = "failed";

    /// Error codes of `hub/update/request`.
    pub mod codes {
        /// Not from a client admitted over Tailscale (a window on the hub's own computer
        /// updates itself).
        pub const NOT_ALLOWED: &str = "update_not_allowed";
        /// The build is not newer than the hub's (never a downgrade), or not a release build.
        pub const NOT_NEWER: &str = "update_not_newer";
        /// This host cannot install YAAC this way (the installer is for Windows).
        pub const UNSUPPORTED: &str = "update_unsupported";
        /// No published release of that build with an installer (or GitHub could not be asked).
        pub const UNAVAILABLE: &str = "update_unavailable";
        /// Another update is already running or installing.
        pub const IN_PROGRESS: &str = "update_in_progress";
    }
}

/// `hub/shutdown` params.
pub type ShutdownParams = Empty;
/// `hub/shutdown` result: sent once the relay queue is checkpointed, just before the hub exits.
pub type ShutdownResult = Empty;

open_union! {
    /// How the hub reaches a target.
    pub enum TargetTransport tag "kind" {
        /// This machine.
        Local(LocalTransport) = "local",
        /// Over SSH.
        Ssh(SshTransport) = "ssh",
        /// A WSL distribution.
        Wsl(WslTransport) = "wsl",
    }
}

/// The local machine.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LocalTransport {}

/// An SSH target: *user@host:port*.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SshTransport {
    /// Host name or alias.
    pub host: String,
    /// Port, when not the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// User, when not the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
}

/// A WSL target: *user in a distro*.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WslTransport {
    /// Distribution name.
    pub distro: String,
    /// User, when not the distro's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
}

open_enum! {
    /// A target connection's state.
    pub enum ConnectionStatus fallback Unknown {
        /// Being established (or re-established).
        Connecting = "connecting",
        /// Connected and the handshake succeeded.
        Connected = "connected",
        /// Not connected; retrying.
        Offline = "offline",
        /// Disabled by the user.
        Disabled = "disabled",
        /// The target's machine is not running — a stopped WSL distro — and is never started
        /// behind the user's back (D35, D96). Its threads are unusable until it runs; the window
        /// offers **Start it** (`hub/targets/start`). The detail says which distro.
        Stopped = "stopped",
        /// Connected, but no common protocol version.
        Incompatible = "incompatible",
    }
}

/// What the hub knows about the daemon on a connected target. No version numbers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonSummary {
    /// Whether the daemon supports loopback preview tunnels.
    #[serde(default)]
    pub port_forward: bool,
    /// Whether the daemon supports the worktree browser and file diffs.
    #[serde(default)]
    pub file_view: bool,
    /// Target supports interactive PTYs.
    #[serde(default)]
    pub terminal: bool,
    /// The daemon can set or remove an orchestration's time limit (`mesh/budget`
    /// `setTimeLimit`, P352).
    #[serde(default)]
    pub budget_time_limit: bool,
    /// Reason the native terminal backend could not load.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_unavailable_reason: Option<String>,
    /// The daemon's build.
    pub build: String,
    /// The daemon is older than this build: show "will be updated when idle" and offer
    /// **Update now**.
    pub older: bool,
    /// The daemon's schema hash matched (a mismatch is a build error, for diagnostics).
    pub schema_hash_matches: bool,
    /// The hub holds a newer numbered build for this target than the one serving, so it can
    /// replace it: at idle by itself, or at once with **Update now**.
    #[serde(default)]
    pub can_update: bool,
}

open_enum! {
    /// Where the update of a target's older `yaacd` stands.
    pub enum DaemonUpdatePhase fallback Unknown {
        /// The daemon was asked to hand over when idle; it is busy (a running turn, or a
        /// request waiting for the user).
        Waiting = "waiting",
        /// The daemon is handing over (or has, and the hub is reconnecting).
        Updating = "updating",
        /// After the hand-over the same older build serves again. The hub does not ask again
        /// for this pair of builds (no blind retries); the detail says what happened.
        Stuck = "stuck",
    }
}

/// The update of a target's older `yaacd`: kept by the hub per target while it is under way,
/// across the reconnect, and cleared once the newer build serves.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonUpdate {
    /// The build that was serving.
    pub from: String,
    /// The build the hub installed for it.
    pub to: String,
    /// Where it stands.
    pub phase: DaemonUpdatePhase,
    /// **Update now** (running turns interrupted), not a hand-over at idle.
    #[serde(default)]
    pub forced: bool,
    /// Text to show as-is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// A target's connection state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionState {
    /// The state.
    pub status: ConnectionStatus,
    /// Text to show as-is (why it is offline, which side is too old, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// The daemon, once connected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daemon: Option<DaemonSummary>,
    /// An update of the target's older `yaacd` under way (kept while it reconnects).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update: Option<DaemonUpdate>,
}

/// A target as the hub stores it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetEntry {
    /// Immutable id, assigned by the hub.
    pub id: TargetId,
    /// The user's label; unique.
    pub name: TargetName,
    /// How it is reached.
    pub transport: TargetTransport,
    /// Whether the hub keeps it connected.
    pub enabled: bool,
    /// Its connection state now.
    pub state: ConnectionState,
}

/// `hub/targets/list` params.
pub type TargetsListParams = Empty;

/// `hub/targets/list` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetsListResult {
    /// Every target.
    pub targets: Vec<TargetEntry>,
}

/// `hub/targets/add` params.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetsAddParams {
    /// The label; must be unique.
    pub name: TargetName,
    /// How to reach it.
    pub transport: TargetTransport,
    /// Whether to connect.
    pub enabled: bool,
}

/// Result of `hub/targets/add`, `hub/targets/update`, `hub/targets/enable`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetResult {
    /// The target after the change.
    pub target: TargetEntry,
}

/// `hub/targets/update` params: a patch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetsUpdateParams {
    /// The target.
    pub id: TargetId,
    /// New label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<TargetName>,
    /// New transport.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<TargetTransport>,
}

/// `hub/targets/remove` params. Removing a target deletes its cache rows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetsRemoveParams {
    /// The target.
    pub id: TargetId,
}

/// `hub/targets/enable` params.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetsEnableParams {
    /// The target.
    pub id: TargetId,
    /// Enabled or not.
    pub enabled: bool,
}

/// `hub/targets/updateDaemon` params.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetsUpdateDaemonParams {
    /// The target.
    pub id: TargetId,
}

/// `hub/targets/updateDaemon` result: the daemon's answer to the hand-over.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetsUpdateDaemonResult {
    /// What the daemon does next (`exiting`).
    pub state: crate::HandoverState,
}

/// `hub/target/state` notification.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetStateParams {
    /// The target.
    pub id: TargetId,
    /// Its new state.
    pub state: ConnectionState,
    /// Whether the hub keeps it connected (the user's switch), as now.
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

/// `hub/request` params: a YAAC request for a target. The result is the target's result in
/// current shape (or its error, passed through).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutedRequest {
    /// The target.
    pub target: TargetId,
    /// The YAAC method.
    pub method: String,
    /// Its params, in current shape.
    pub params: Value,
}

/// `hub/notification` params: a YAAC notification to or from a target.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutedNotification {
    /// The target.
    pub target: TargetId,
    /// The YAAC method.
    pub method: String,
    /// Its params, in current shape.
    pub params: Value,
}

/// `hub/rateLimits/get` params.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitsGetParams {
    /// The window shows the panel and wants fresh values: the hub asks each connected target
    /// to read its harnesses again — at most every five minutes per target — and what they
    /// answer comes as `hub/rateLimits/changed`.
    #[serde(default)]
    pub refresh: bool,
}

/// One account's rate limits, gathered from every target that uses it (D161).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountRateLimits {
    /// The account key ([`crate::RateLimitAccount::key`]); unique per provider kind.
    pub key: String,
    /// The provider kind (for the logo and colour only).
    pub kind: crate::ProviderKind,
    /// The plan as the harness names it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// The sign-in's email, for the tooltip only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// The windows of the most recent report about this account.
    pub windows: Vec<crate::RateLimitWindow>,
    /// When that report was made.
    pub at: crate::Timestamp,
    /// Every target that reported this account.
    pub targets: Vec<TargetId>,
    /// No connected target reports this account now: the values are the last known.
    #[serde(default)]
    pub stale: bool,
}

/// `hub/rateLimits/get` result and `hub/rateLimits/changed` params.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitsResult {
    /// One entry per account that reports at least one window.
    pub accounts: Vec<AccountRateLimits>,
}

/// `hub/preview/tunnel`: one TCP connection to `host:port` on the target, carried over this
/// attach connection's data frames instead of a hub listener (the Android client, P380). The
/// hub does one `tunnel/open` on the target and relays both ways under credit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewTunnelParams {
    pub target: TargetId,
    /// Resolved on the target; absent means `127.0.0.1`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    pub port: u16,
    /// What the client may receive before it grants more with `stream/credit`.
    pub initial_credit: u64,
}

/// `hub/preview/tunnel` result: the attach stream and what the client may send on it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewTunnelResult {
    pub stream: crate::StreamId,
    pub initial_credit: u64,
}

/// `hub/preview/tunnelClose`, either way: the tunnel on `stream` is closed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewTunnelCloseParams {
    pub stream: crate::StreamId,
}

/// Start or reuse the target’s SOCKS5 proxy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewProxyParams {
    /// Target id.
    pub target: TargetId,
}

/// The loopback listener of the target’s SOCKS5 proxy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewProxyResult {
    /// Local SOCKS5 port.
    pub socks_port: u16,
}

/// Start or reuse a plain forward for the system browser.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewOpenParams {
    /// Target id.
    pub target: TargetId,
    /// Target TCP port.
    pub port: u16,
    /// Target listen or loopback address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
}

/// A local forward ready for the system browser.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewOpenResult {
    /// URL the system browser can open.
    pub url: String,
    /// Local port.
    pub local_port: u16,
}

/// Close a single forwarded port.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewCloseParams {
    /// Target id.
    pub target: TargetId,
    /// Target TCP port.
    pub port: u16,
}

/// An open hub preview listener.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewListener {
    /// Target id.
    pub target: TargetId,
    /// Kind.
    pub kind: PreviewKind,
    /// Target TCP port.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// Local port.
    pub local_port: u16,
    /// Current connection count.
    pub connections: u32,
}

/// Kind of local listener.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PreviewKind {
    /// Target-wide SOCKS5 listener.
    Proxy,
    /// A single forwarded port.
    Forward,
}
/// Close a proxy on the target.
pub type PreviewProxyCloseParams = PreviewProxyParams;
/// Every open listener.
pub type PreviewListResult = Vec<PreviewListener>;
/// Closing a proxy has no result fields.
pub type PreviewProxyCloseResult = Empty;
/// Closing a forward has no result fields.
pub type PreviewCloseResult = Empty;
/// Listing listeners takes no arguments.
pub type PreviewListParams = Empty;

/// Hub → window. The hub supplies the source connection’s target.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserCallParams {
    /// Source target.
    pub target: TargetId,
    /// Thread and action from its daemon.
    #[serde(flatten)]
    pub call: crate::BrowserCallParams,
}

/// The window’s reply passes through the hub.
pub type BrowserCallResult = crate::BrowserCallResult;
