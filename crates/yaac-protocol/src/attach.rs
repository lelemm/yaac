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

/// Every attach method name.
pub mod names {
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
    /// Hub → window browser request.
    pub const BROWSER_CALL: &str = "hub/browser/call";
    /// Preview listener management.
    pub const PREVIEW_PROXY: &str = "hub/preview/proxy";
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
    /// Window → hub: ask an older hub to checkpoint its relay queue and exit.
    pub const SHUTDOWN: &str = "hub/shutdown";
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
    /// Either way: carry a YAAC notification to or from a target.
    pub const NOTIFICATION: &str = "hub/notification";
    /// Window → hub: the plan rate limits of every account used on any target, one entry per
    /// account (D161) ([`RateLimitsGetParams`] → [`RateLimitsResult`]).
    pub const RATE_LIMITS_GET: &str = "hub/rateLimits/get";
    /// Hub → window notification: those accounts changed ([`RateLimitsResult`]).
    pub const RATE_LIMITS_CHANGED: &str = "hub/rateLimits/changed";
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
