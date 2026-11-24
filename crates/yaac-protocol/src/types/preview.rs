//! Web previews and the in-app browser (docs/11-preview.md).
use super::StreamId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Open a TCP connection to any host the target reaches (P323).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TunnelOpenParams {
    /// Target TCP port.
    pub port: u16,
    /// Host name or address, resolved on the target; defaults to 127.0.0.1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// Bytes granted to the peer for this direction.
    pub initial_credit: u64,
}

/// The connected tunnel and the initial allowance for host-to-target bytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TunnelOpenResult {
    /// Bidirectional data stream.
    pub stream: StreamId,
    /// Bytes granted to the peer for this direction.
    pub initial_credit: u64,
}

/// Close the socket at the other end of a tunnel.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TunnelCloseParams {
    /// Bidirectional data stream.
    pub stream: StreamId,
    /// Optional diagnostic when a socket failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// A listening socket owned by the target user.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PreviewPort {
    /// Target TCP port.
    pub port: u16,
    /// Listening address, including wildcard binds.
    pub host: String,
    /// Listening process id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// Listening process command.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Working directory of the listening process.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

/// Listeners discovered on request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PreviewPortsResult {
    /// Ports.
    pub ports: Vec<PreviewPort>,
}

use crate::macros::{open_enum, open_union};
use super::{Empty, ThreadId};

/// A request from the thread authenticated by yaacd.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BrowserCallParams {
    /// Derived from the harness attachment ticket.
    pub thread_id: ThreadId,
    /// Operation on this thread’s tabs.
    pub action: BrowserAction,
}

/// State of one open tab.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BrowserTab {
    /// Opaque identifier scoped to the calling thread.
    pub tab: String,
    /// Current page URL.
    pub url: String,
    /// Page title.
    pub title: String,
    /// Whether navigation is in progress.
    pub loading: bool,
}

/// Open tabs in the calling thread.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BrowserTabs {
    /// Tabs in display order.
    pub tabs: Vec<BrowserTab>,
}

/// Identify a tab.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BrowserTabParams {
    /// Opaque identifier scoped to the calling thread.
    pub tab: String,
}

/// Capture a tab or an element.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BrowserScreenshotParams {
    /// Opaque identifier scoped to the calling thread.
    pub tab: String,
    /// CSS selector to capture.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,
}

/// A PNG suitable for MCP image content.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BrowserScreenshot {
    /// Base64 encoded PNG.
    pub data: String,
}

/// Accessible text tree with element references.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BrowserSnapshot {
    /// Serialized accessible tree.
    pub tree: String,
}

/// Read a tab’s bounded diagnostics buffer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BrowserLogParams {
    /// Opaque identifier scoped to the calling thread.
    pub tab: String,
    /// Resume position returned by the preceding read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<u64>,
}

/// Console message or uncaught exception.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BrowserConsoleEntry {
    /// Console severity or exception.
    pub level: String,
    /// Message text.
    pub text: String,
}

/// Metadata for a request observed by the tab.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BrowserNetworkEntry {
    /// Request URL.
    pub url: String,
    /// HTTP method.
    pub method: String,
    /// HTTP response status if received.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    /// Elapsed milliseconds if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// Failure description if the request failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Console entries since the requested cursor.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BrowserConsole {
    /// Recorded messages.
    pub entries: Vec<BrowserConsoleEntry>,
    /// Next read position.
    pub cursor: u64,
}

/// Requests since the requested cursor.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BrowserNetwork {
    /// Recorded requests.
    pub entries: Vec<BrowserNetworkEntry>,
    /// Next read position.
    pub cursor: u64,
}

/// Open a tab in the calling thread's browser area.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BrowserOpenParams {
    /// Address: a URL, `host:port`, or a bare port (`5173` means `http://localhost:5173/`).
    pub url: String,
}

open_enum! {
    /// A history step or reload.
    pub enum BrowserHistory fallback Unknown {
        /// Previous history entry.
        Back = "back",
        /// Next history entry.
        Forward = "forward",
        /// Reload the current page.
        Reload = "reload",
    }
}

/// Load an address or move in history; exactly one of `url` and `history`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BrowserNavigateParams {
    /// Opaque identifier scoped to the calling thread.
    pub tab: String,
    /// Address to load, as for `open`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// History step or reload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history: Option<BrowserHistory>,
}

/// Click an element (snapshot reference or CSS selector) or a viewport point in CSS pixels.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BrowserClickParams {
    /// Opaque identifier scoped to the calling thread.
    pub tab: String,
    /// Element reference from the latest snapshot.
    #[serde(default, rename = "ref", skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    /// CSS selector; the first match is used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,
    /// Horizontal viewport position, with `y`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<i32>,
    /// Vertical viewport position, with `x`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<i32>,
}

/// Type text into an element, or into the focused element when none is named.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BrowserTypeParams {
    /// Opaque identifier scoped to the calling thread.
    pub tab: String,
    /// Text to insert.
    pub text: String,
    /// Element reference from the latest snapshot.
    #[serde(default, rename = "ref", skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    /// CSS selector; the first match is used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,
    /// Replace the element's current value instead of appending.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clear: Option<bool>,
}

/// Press a key, optionally with modifiers (`Enter`, `Tab`, `Control+a`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BrowserPressParams {
    /// Opaque identifier scoped to the calling thread.
    pub tab: String,
    /// Key name, `+`-joined with modifiers.
    pub key: String,
    /// Element to focus first, by snapshot reference.
    #[serde(default, rename = "ref", skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    /// Element to focus first, by CSS selector.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,
}

/// Evaluate JavaScript in the page.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BrowserEvalParams {
    /// Opaque identifier scoped to the calling thread.
    pub tab: String,
    /// An expression; a promise is awaited.
    pub expression: String,
}

/// The JSON value of an evaluated expression.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BrowserEvalResult {
    /// JSON result; `null` for `undefined` and values JSON cannot hold.
    #[serde(default)]
    pub value: serde_json::Value,
}

open_union! {
    /// Browser request on the calling thread's tabs.
    pub enum BrowserAction tag "kind" {
        /// List.
        List(Empty) = "list",
        /// Screenshot.
        Screenshot(BrowserScreenshotParams) = "screenshot",
        /// Snapshot.
        Snapshot(BrowserTabParams) = "snapshot",
        /// Console.
        Console(BrowserLogParams) = "console",
        /// Network.
        Network(BrowserLogParams) = "network",
        /// Open a tab without switching the user's view; waits (bounded) for the load.
        Open(BrowserOpenParams) = "open",
        /// Close a tab.
        Close(BrowserTabParams) = "close",
        /// Navigate; waits (bounded) for the load.
        Navigate(BrowserNavigateParams) = "navigate",
        /// Click.
        Click(BrowserClickParams) = "click",
        /// Type text.
        Type(BrowserTypeParams) = "type",
        /// Press a key.
        Press(BrowserPressParams) = "press",
        /// Evaluate JavaScript.
        Eval(BrowserEvalParams) = "eval",
    }
}

open_union! {
    /// Browser response; `kind` matches the request.
    pub enum BrowserCallResult tag "kind" {
        /// List.
        List(BrowserTabs) = "list",
        /// Screenshot.
        Screenshot(BrowserScreenshot) = "screenshot",
        /// Snapshot.
        Snapshot(BrowserSnapshot) = "snapshot",
        /// Console.
        Console(BrowserConsole) = "console",
        /// Network.
        Network(BrowserNetwork) = "network",
        /// The new tab, after its load finished or the wait bound passed (`loading` true).
        Open(BrowserTab) = "open",
        /// Closed.
        Close(Empty) = "close",
        /// The tab after the load finished or the wait bound passed (`loading` true).
        Navigate(BrowserTab) = "navigate",
        /// Clicked.
        Click(Empty) = "click",
        /// Typed.
        Type(Empty) = "type",
        /// Pressed.
        Press(Empty) = "press",
        /// Evaluated.
        Eval(BrowserEvalResult) = "eval",
    }
}
