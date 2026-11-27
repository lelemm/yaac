//! The window's side of the attach protocol: turns user actions into hub messages, applies the
//! hub's answers and notifications to [`AppState`], and remembers what each outstanding
//! request was for. No I/O of its own: everything goes through a [`HubLink`].

use std::collections::HashMap;

use serde_json::{Value, json};
use yaac_protocol::attach::{self, HelloResult, RoutedNotification, RoutedRequest, TargetStateParams};
use yaac_protocol::methods::names as m;
use yaac_protocol::rpc::{ErrorCode, ErrorObject, Message, RpcId};
use yaac_protocol::{
    Address, ClientId, Control, InstanceId, ItemBody, ItemId, MeshBudgetAction, MeshBudgetResult, NoticeKind,
    OpId, ProviderListResult, ProviderUpdatedParams, RequestId, RequestResponse, RequestState, SubscribeMode,
    TargetId, ThreadEvent, ThreadEventParams, ThreadId, ThreadReadResult, ThreadRemovedParams, ThreadState,
    ThreadSubscribeResult, ThreadSummary, ThreadSyncResult, ThreadUpdatedParams, Timestamp,
};

use crate::agents::root_of;
use crate::attachments::DraftRef;
use crate::key::ThreadKey;
use crate::link::HubLink;
use crate::mode_confirm::{ModePick, ModeTarget};
use crate::state::{AppState, Draft, ForkPreviewState, FsListingState, HelloOutcome, NewThreadDraft, Selection};

/// Hub-side attach methods this client uses beyond `yaac_protocol::attach` (the hub's own
/// additions; the names are the hub's, repeated here because this crate cannot depend on it).
pub mod hub_names {
    /// Read hub settings.
    pub const SETTINGS_GET: &str = "hub/settings/get";
    /// Change hub settings.
    pub const SETTINGS_UPDATE: &str = "hub/settings/update";
    /// Clear the cache (never drafts).
    pub const CACHE_CLEAR: &str = "hub/cache/clear";
    /// Store a `ui_state` value.
    pub const UI_STATE_SET: &str = "hub/uiState/set";
    /// Read `ui_state` values (a client on another machine than the hub).
    pub const UI_STATE_GET: &str = "hub/uiState/get";
    // SEAM (D92): the hub's view preferences — pin, mute (with `mutedUntil`), dismiss — kept
    // on the user's computer, not on the thread. The attach methods are being added to
    // `yaac-protocol` by the daemon/hub package; until they land, their names live here and
    // the demo hub answers them. Connect at merge: move these to `attach::names` and decode
    // with the protocol's own types.
    /// Every thread's view preferences.
    pub const VIEW_GET: &str = "hub/view/get";
    /// Change one thread's view preferences.
    pub const VIEW_SET: &str = "hub/view/set";
    /// Change several threads' view preferences in one request (a bulk mute, D138).
    pub const VIEW_SET_MANY: &str = "hub/view/setMany";
    /// View preferences changed (another window, or a timed mute the hub cleared).
    pub const VIEW_CHANGED: &str = "hub/view/changed";
    /// Which threads this window shows (D114): the hub marks a thread unseen when its turn
    /// ends and no window shows it.
    pub const VIEW_SHOWING: &str = "hub/view/showing";
    /// SEAM: bringing an archived thread back. `yaac-protocol` has `thread/archive` but no
    /// way back yet; the name is the one the daemon package is expected to add.
    pub const THREAD_UNARCHIVE: &str = "thread/unarchive";
    /// Reconnect a target now.
    pub const TARGETS_RECONNECT: &str = "hub/targets/reconnect";
    /// The target list changed.
    pub const TARGETS_CHANGED: &str = "hub/targets/changed";
    /// A target's cached index changed.
    pub const INDEX_CHANGED: &str = "hub/index/changed";
    /// A target's cached providers changed.
    pub const PROVIDERS_CHANGED: &str = "hub/providers/changed";
    /// Find threads by id or fuzzy title across every cached target.
    pub const THREADS_FIND: &str = "hub/threads/find";
    // SSH setup, verify, and revoke (`yaac_hub::protocol::names::SSH_*`).
    /// Contact the server and read its host key.
    pub const SSH_REACH: &str = "hub/ssh/reach";
    /// Trust the confirmed host key and try signing in with no password.
    pub const SSH_PROBE: &str = "hub/ssh/probe";
    /// Sign in once and install YAAC's key.
    pub const SSH_SIGN_IN: &str = "hub/ssh/signIn";
    /// Sign in with YAAC's key alone.
    pub const SSH_VERIFY: &str = "hub/ssh/verify";
    /// Remove the line just added (after a failed verify).
    pub const SSH_REMOVE_KEY: &str = "hub/ssh/removeKey";
    /// Revoke YAAC's key.
    pub const SSH_REVOKE: &str = "hub/ssh/revoke";
    /// The user removed YAAC's line by hand.
    pub const SSH_REMOVED_BY_HAND: &str = "hub/ssh/removedByHand";
    /// What YAAC did on each SSH server.
    pub const SSH_RECORDS: &str = "hub/ssh/records";
    /// Those records changed.
    pub const SSH_CHANGED: &str = "hub/ssh/changed";
    /// The WSL distros installed on this PC (`yaac_hub::protocol::names::WSL_DISTROS`).
    pub const WSL_DISTROS: &str = "hub/wsl/distros";
    /// A target's cached projects were replaced.
    pub const PROJECTS_CHANGED: &str = "hub/projects/changed";
    /// The window is newer than the hub.
    pub const HUB_OUTDATED: &str = "hub_outdated";
    /// The window is older than the hub.
    pub const WINDOW_OUTDATED: &str = "window_outdated";
}

/// What an outstanding request was for.
#[derive(Clone, Debug, PartialEq)]
enum Call {
    ModeChanged { key: ThreadKey, root: ThreadKey, batch: OpId },
    Hello,
    View,
    Targets,
    Providers(TargetId),
    Sync { target: TargetId, connected: bool },
    Subscribe(ThreadKey),
    Read(ThreadKey),
    ReadAround(ThreadKey, ItemId),
    ReadOlder(ThreadKey, String),
    ReadNewer(ThreadKey, String),
    /// A turn exceeded one wire page. Keep the complete visible window until the rest arrives.
    HistoryTail { read: Box<Call>, page: Box<ThreadReadResult> },
    Budget,
    Respond { key: ThreadKey, request_id: RequestId },
    /// `turn/start` or `turn/steer` of the composer's message; `op` is its echo (D165).
    TurnStart { key: ThreadKey, draft: Draft, op: OpId },
    ThreadStart { draft: NewThreadDraft },
    Fork { target: TargetId },
    /// `thread/delete`.
    Delete(ThreadKey),
    /// `thread/takeOver` (D140).
    TakeOver(ThreadKey),
    Compact { key: ThreadKey, command: Option<String> },
    FsSearch(u64),
    ImageChunk(u64),
    /// Generated artifact transfer.
    ArtifactChunk(u64),
    /// One `thread/archive` of a bulk Archive (D138).
    BulkArchive { batch: u64, key: ThreadKey },
    /// One `thread/move` (D144); `before`: the summary to put back if it is refused.
    Move { batch: u64, key: ThreadKey, before: Box<ThreadSummary> },
    ForkPreview { key: ThreadKey, instance: InstanceId },
    SessionDiscovery { target: TargetId, write: bool },
    InstanceList(TargetId),
    InstanceChanged(TargetId),
    /// A harness-home change (P300); a `store_change` refusal asks the user.
    InstanceHome { target: TargetId, instance: InstanceId, home: Option<String> },
    FsList { target: TargetId, path: String },
    /// `skills/list` for a composer's scope.
    Skills(crate::skills::SkillScopeKey),
    HubSettings,
    /// `hub/notifications/subscribe`.
    RequestFeed,
    /// `hub/uiState/get`.
    UiStateGet,
    /// `hub/notifications/respond`: where to open the request if an answer fails.
    FeedRespond { open: attach::NotificationOpen, answered: bool },
    Explain(ThreadKey, ItemId, RequestId),
    Projects(TargetId),
    ProjectCreate { target: TargetId, for_draft: Option<u64> },
    Find { query: String },
    Shutdown,
    TargetChanged,
    /// **Update now** (`hub/targets/updateDaemon`) for this target.
    UpdateDaemon(TargetId),
    /// `hub/upload/start` for one attempt of one chip.
    UploadStart { draft: DraftRef, id: u64, attempt: u32 },
    /// `hub/drafts/blob/begin`: the chunks wait here for the blob id.
    BlobBegin { draft: DraftRef, id: u64, chunks: Vec<String> },
    /// `hub/drafts/blob/append` (only a failure matters).
    BlobAppend { draft: DraftRef, id: u64 },
    /// `hub/drafts/blob/end`.
    BlobEnd { draft: DraftRef, id: u64 },
    /// `hub/upload/list` after restoring drafts.
    UploadList,
    /// `hub/drafts/list` after attaching.
    DraftsList,
    /// `hub/rateLimits/get` (D161).
    RateLimits,
    /// A `hub/ssh/*` step: the answer goes to the setup backend under this id.
    Ssh(RpcId),
    Preview(RpcId),
    Troubleshoot(RpcId),
    CatalogList,
    CatalogEdit(RpcId),
    /// `hub/ssh/records`.
    SshRecords,
    /// `hub/wsl/distros`.
    WslDistros,
    /// `forge/prs` for a thread's row (D119).
    ForgePrs(ThreadKey),
    /// `forge/prepare` for the Create PR modal (D120).
    ForgePrepare(ThreadKey),
    /// `forge/pr/create`.
    ForgeCreate(ThreadKey),
    /// The `turn/start` of "Ask the agent to write it".
    ForgeAsk(ThreadKey),
    /// `turn/queueEdit`, `turn/queueCancel` or `turn/queuePromote` on a queued message (D166).
    Queue { key: ThreadKey, queue_id: String, op: queue::QueueOp },
    CommandLogs(crate::running_commands::CommandKey),
    CommandStop { view: ThreadKey, command: crate::running_commands::CommandKey },
    Ignore,
}

mod forge;
mod limits;
mod queue;
mod running_commands;
mod ssh;
mod uploads;
mod wsl;

/// A problem to show the user (as a notice, never a dialog).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientError {
    /// Stable for this notice's lifetime, even when other notices are dismissed.
    pub id: u64,
    /// What it concerns.
    pub key: Option<ThreadKey>,
    /// Text.
    pub text: String,
}

/// Replies can arrive out of order, including replies from different targets.
#[derive(Default)]
struct ModeChangeResults {
    remaining: usize,
    applied: usize,
    failed: usize,
    offline: Vec<String>,
}

/// The window's attach-protocol client.
pub struct Client {
    /// The state everything is drawn from.
    pub state: AppState,
    link: Box<dyn HubLink>,
    next_id: i64,
    pending: HashMap<RpcId, Call>,
    mode_results: HashMap<OpId, ModeChangeResults>,
    ops: u64,
    /// This window run's own part of every operation id (D159): ids must never repeat across
    /// runs, because `yaacd` remembers them and answers a repeated id with the old outcome.
    run: String,
    build: String,
    warm: Vec<ThreadKey>,
    /// Errors that have not been dismissed.
    pub errors: Vec<ClientError>,
    next_error: u64,
    /// The composer’s most recent file lookup.
    pub composer_files: Option<ComposerFiles>,
    /// Bounded image reads returned by the hub.
    pub image_chunks: Vec<(u64, Result<yaac_protocol::UploadReadResult, String>)>,
    /// Generated artifact transfer.
    pub artifact_chunks: Vec<(u64, Result<yaac_protocol::UploadReadResult, String>)>,
    /// The command log the user has opened in the composer.
    pub command_log: Option<crate::running_commands::CommandLog>,
    /// The last attachment id handed out.
    attach_ids: u64,
    /// Drafts changed since they were last stored in the hub.
    dirty: std::collections::BTreeSet<DraftRef>,
    /// Upload notifications for handles not known yet (their `hub/upload/start` answer has
    /// not arrived), oldest first, bounded.
    orphans: std::collections::VecDeque<(String, Value)>,
    /// The threads last reported to the hub as shown (`hub/view/showing`, D114); `None`
    /// until the first report after attaching.
    showing: Option<Vec<ThreadKey>>,
    visible_requests: Option<Vec<attach::RequestLocation>>,
    pub notification_open: std::collections::VecDeque<attach::NotificationOpen>,
    /// Subscribe to the request feed on every attach (`subscribe_request_feed`).
    request_feed: bool,
    /// Preview proxy, forward and discovery results for the window.
    preview_replies: Vec<(RpcId, Result<Value, String>)>,
    troubleshoot_reply: Option<(RpcId, Result<yaac_protocol::TroubleshootResult, String>)>,
    catalog_reply: Option<(RpcId, Result<Value, String>)>,
    terminal_notes: Vec<(String, Value)>,
    /// Attach streams that are tunnels (`hub/preview/tunnel`, P380), their data and notes kept
    /// apart from the terminals'.
    tunnel_streams: std::collections::HashSet<yaac_protocol::StreamId>,
    tunnel_data: Vec<(yaac_protocol::StreamId, Vec<u8>)>,
    terminal_data: Vec<(yaac_protocol::StreamId, Vec<u8>)>,
    tunnel_notes: Vec<(String, Value)>,
    /// The window-side choices read with `hub/uiState/get`, until taken.
    ui_state: Option<Vec<(String, Value)>>,
    /// Agent browser calls from the hub (`hub/browser/call`), not yet taken by the window.
    pub theme_calls: Vec<(RpcId, yaac_protocol::ThemeCallParams)>,
    browser_calls: Vec<(RpcId, attach::BrowserCallParams)>,
    /// Answers to `hub/ssh/*` steps, for the setup backend.
    ssh_replies: Vec<(RpcId, Result<Value, ErrorObject>)>,
    /// The hub's SSH records, once it has answered `hub/ssh/records`.
    ssh_wire: Option<Vec<ssh::SshWire>>,
    /// The host's time zone, for the dates of SSH records (D91).
    pub ssh_zone: crate::time::Zone,
    /// Bulk Archives still waiting for answers (D138).
    batches: HashMap<u64, ArchiveBatch>,
    next_batch: u64,
    /// The report of the last bulk Archive that did not simply succeed, until dismissed.
    pub bulk_report: Option<crate::bulk::BulkReport>,
    /// Moves still waiting for answers (D144), by batch: each thread's outcome.
    moves: HashMap<u64, Vec<(ThreadKey, Option<crate::bulk::Outcome>)>>,
    /// The bottom bar's notice after a move that needed no confirmation, with its Undo
    /// (D144), until dismissed or replaced.
    pub move_notice: Option<crate::moving::MoveNotice>,
}

/// The composer’s current read-only file search.
#[derive(Debug)]
pub struct ComposerFiles {
    /// Draft that requested this search.
    pub draft: DraftRef,
    /// Target serving this lookup.
    pub target: TargetId,
    /// Last requested query and working directory.
    pub params: yaac_protocol::FsSearchParams,
    /// Results, or the reason the target could not search. `None` while loading.
    pub result: Option<Result<yaac_protocol::FsSearchResult, String>>,
    id: u64,
}

/// A bulk Archive in flight: each thread's outcome, in the order asked; `None` while waiting.
#[derive(Clone, Debug, Default)]
struct ArchiveBatch {
    outcomes: Vec<(ThreadKey, Option<crate::bulk::Outcome>)>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client").field("pending", &self.pending.len()).finish_non_exhaustive()
    }
}

impl Client {
    /// Attach through `link`: sends `hub/hello` at once.
    pub fn new(link: Box<dyn HubLink>, build: impl Into<String>, client_id: ClientId) -> Self {
        let mut c = Self {
            state: AppState::new(),
            link,
            next_id: 1,
            pending: HashMap::new(),
            ops: 0,
            run: String::new(),
            build: build.into(),
            warm: Vec::new(),
            errors: Vec::new(),
            next_error: 0,
            mode_results: HashMap::new(),
            composer_files: None,
            image_chunks: Vec::new(),
            artifact_chunks: Vec::new(),
            command_log: None,
            attach_ids: 0,
            dirty: std::collections::BTreeSet::new(),
            orphans: std::collections::VecDeque::new(),
            showing: None,
            visible_requests: None,
            notification_open: Default::default(),
            request_feed: false,
            preview_replies: Vec::new(),
            troubleshoot_reply: None,
            catalog_reply: None,
            terminal_notes: Vec::new(),
            tunnel_streams: Default::default(),
            tunnel_data: Vec::new(),
            terminal_data: Vec::new(),
            tunnel_notes: Vec::new(),
            ui_state: None,
            theme_calls: Vec::new(),
            browser_calls: Vec::new(),
            ssh_replies: Vec::new(),
            ssh_wire: None,
            ssh_zone: crate::time::Zone::UTC,
            batches: HashMap::new(),
            next_batch: 0,
            bulk_report: None,
            moves: HashMap::new(),
            move_notice: None,
        };
        c.state.client_id = Some(client_id);
        let params = json!({ "build": c.build });
        c.request(attach::names::HELLO, params, Call::Hello);
        c
    }

    /// Threads to subscribe to as soon as their target's index is known, so their hints and
    /// transcripts are warm (the window's least-recently-used set from its last run).
    pub fn set_warm(&mut self, keys: Vec<ThreadKey>) {
        self.warm = keys;
    }

    /// Apply everything the hub has sent. Returns whether anything arrived.
    pub fn pump(&mut self) -> bool {
        let mut any = false;
        for _ in 0..100_000 {
            let Some(msg) = self.link.poll() else { break };
            any = true;
            self.handle(msg);
        }
        if any {
            // Targets may have been added or changed: show the hub's SSH records on them.
            self.map_ssh_records();
            if self.state.copies.dirty { self.follow_copies(); }
        }
        any
    }

    fn request(&mut self, method: &str, params: Value, call: Call) -> RpcId {
        let id = RpcId::Number(self.next_id);
        self.next_id += 1;
        self.pending.insert(id.clone(), call);
        self.link.send(Message::request(id.clone(), method, params));
        id
    }

    /// Queue a preview attach request on the existing hub link.
    pub fn preview_request(&mut self, method: &str, params: Value) -> RpcId {
        self.request(method, params, Call::Preview(RpcId::Number(self.next_id)))
    }

    /// Queue an attach-protocol notification for the hub (e.g. `stream/credit` for a terminal's
    /// output stream).
    pub fn notify(&mut self, method: &str, params: Value) {
        self.link.send(Message::notification(method, params));
    }

    /// Terminal input on its data stream (docs/13-terminal.md). Never blocks.
    pub fn send_terminal_data(&mut self, stream: yaac_protocol::StreamId, bytes: Vec<u8>) {
        self.link.send_data(stream, bytes);
    }

    /// Terminal output that has arrived, in order.
    pub fn take_terminal_data(&mut self) -> Vec<(yaac_protocol::StreamId, Vec<u8>)> {
        self.sort_data();
        std::mem::take(&mut self.terminal_data)
    }

    fn sort_data(&mut self) {
        while let Some((stream, bytes)) = self.link.poll_data() {
            if self.tunnel_streams.contains(&stream) {
                self.tunnel_data.push((stream, bytes));
            } else {
                self.terminal_data.push((stream, bytes));
            }
        }
    }

    /// A tunnel's attach stream (from a `hub/preview/tunnel` answer): its data and notes go to
    /// [`Client::take_tunnel_data`] and [`Client::take_tunnel_notes`] from now on.
    pub fn track_tunnel(&mut self, stream: yaac_protocol::StreamId) {
        self.tunnel_streams.insert(stream);
    }

    /// Stop tracking a tunnel (closed either way).
    pub fn forget_tunnel(&mut self, stream: yaac_protocol::StreamId) {
        self.tunnel_streams.remove(&stream);
    }

    /// Tunnel bytes that have arrived, in order.
    pub fn take_tunnel_data(&mut self) -> Vec<(yaac_protocol::StreamId, Vec<u8>)> {
        self.sort_data();
        std::mem::take(&mut self.tunnel_data)
    }

    /// `stream/credit` and `hub/preview/tunnelClose` for tunnels.
    pub fn take_tunnel_notes(&mut self) -> Vec<(String, Value)> {
        std::mem::take(&mut self.tunnel_notes)
    }

    /// Bytes for a tunnel (within the credit the hub granted). Never blocks.
    pub fn send_tunnel_data(&mut self, stream: yaac_protocol::StreamId, bytes: Vec<u8>) {
        self.link.send_data(stream, bytes);
    }

    /// Terminal notifications from the hub since the last call: `hub/terminal/title`,
    /// `hub/terminal/exited`, and `stream/credit` (input credit for a terminal stream).
    pub fn take_terminal_notes(&mut self) -> Vec<(String, Value)> {
        std::mem::take(&mut self.terminal_notes)
    }

    /// Collect completed preview attach requests without waiting for the hub.
    pub fn take_preview_replies(&mut self) -> Vec<(RpcId, Result<Value, String>)> {
        std::mem::take(&mut self.preview_replies)
    }

    /// Agent browser calls the hub offered this window (docs/11-preview.md#agents). Each
    /// must be answered once with [`Client::answer_browser_call`].
    pub fn take_browser_calls(&mut self) -> Vec<(RpcId, attach::BrowserCallParams)> {
        std::mem::take(&mut self.browser_calls)
    }

    /// Answer an agent browser call. `method_not_found` makes the hub offer the call to
    /// another window.
    pub fn answer_browser_call(&mut self, id: RpcId, outcome: Result<yaac_protocol::BrowserCallResult, ErrorObject>) {
        self.answer_window_call(id,outcome.map(|r| serde_json::json!(r)));
    }

    pub fn answer_window_call(&mut self,id: RpcId,outcome: Result<Value, ErrorObject>) {
        self.link.send(match outcome {
            Ok(result) => Message::Response { id, result },
            Err(error) => Message::Error { id: Some(id), error },
        });
    }

    fn routed(&mut self, target: &TargetId, method: &str, params: Value, call: Call) {
        let env = RoutedRequest { target: target.clone(), method: method.to_owned(), params };
        let value = serde_json::to_value(env).unwrap_or(Value::Null);
        self.request(attach::names::REQUEST, value, call);
    }

    /// Run an ephemeral support diagnosis on a connected target.
    pub fn troubleshoot(&mut self, target: &TargetId, params: yaac_protocol::TroubleshootParams) -> RpcId {
        let id = RpcId::Number(self.next_id);
        self.routed(target, m::DIAGNOSTICS_TROUBLESHOOT, json!(params), Call::Troubleshoot(id.clone()));
        id
    }

    /// Collect the last diagnosis response without waiting for the hub.
    pub fn take_troubleshoot_reply(&mut self) -> Option<(RpcId, Result<yaac_protocol::TroubleshootResult, String>)> {
        self.troubleshoot_reply.take()
    }

    /// Read the hub-owned skill index.
    pub fn catalog_list(&mut self) {
        if !self.pending.values().any(|c| *c == Call::CatalogList) {
            self.request(attach::names::CATALOG_SKILLS, json!({}), Call::CatalogList);
        }
    }

    /// Load one skill to edit on the window.
    pub fn catalog_edit(&mut self, id: &str) -> RpcId {
        self.catalog_request(attach::names::CATALOG_SKILLS, json!({ "id": id }))
    }

    /// Save the user's latest edit against the revision they opened.
    pub fn catalog_save(&mut self, skill: yaac_protocol::CatalogSkillSave) -> RpcId {
        self.catalog_request(attach::names::CATALOG_SKILL_SAVE, json!(skill))
    }

    /// Remove a user-owned entry.
    pub fn catalog_remove(&mut self, skill: yaac_protocol::CatalogSkillRemove) -> RpcId {
        self.catalog_request(attach::names::CATALOG_SKILL_REMOVE, json!(skill))
    }

    fn catalog_request(&mut self, method: &str, params: Value) -> RpcId {
        self.request(method, params, Call::CatalogEdit(RpcId::Number(self.next_id)))
    }

    /// Collect an edit reply without blocking the window.
    pub fn take_catalog_reply(&mut self) -> Option<(RpcId, Result<Value, String>)> {
        self.catalog_reply.take()
    }

    /// Fetch one bounded chunk of an image stored on a connected target.
    pub fn read_image_chunk(&mut self, token: u64, target: &TargetId, id: &yaac_protocol::UploadId, offset: u64) {
        self.read_upload_chunk(target, id, offset, Call::ImageChunk(token));
    }

    /// Read a published snapshot over its originating target connection.
    pub fn read_artifact_chunk(&mut self, token: u64, target: &TargetId, id: &yaac_protocol::UploadId, offset: u64) {
        self.read_upload_chunk(target, id, offset, Call::ArtifactChunk(token));
    }

    fn read_upload_chunk(&mut self, target: &TargetId, id: &yaac_protocol::UploadId, offset: u64, recipient: Call) {
        let params = yaac_protocol::UploadReadParams { upload_id: id.clone(), offset, max_bytes: 512 * 1024 };
        self.routed(target, m::UPLOAD_READ, json!(params), recipient);
    }

    /// Fetch one bounded chunk of a clipboard image stored with the local draft.
    pub fn read_draft_image_chunk(&mut self, token: u64, id: &str, offset: u64) {
        self.request("hub/drafts/blob/read", json!({ "blobId": id, "offset": offset }), Call::ImageChunk(token));
    }

    fn apply_session_discovery(&mut self, target: &TargetId, result: Value) {
        if let Ok(result) = serde_json::from_value::<yaac_protocol::DiscoveryCapsResult>(result)
            && let Some(record) = self.state.target_mut(target)
        {
            record.session_discovery = result.caps.auto_discover;
        }
    }

    /// Whether the target's discovery setting is being read or written.
    pub fn session_discovery_pending(&self, target: &TargetId) -> bool {
        self.pending.values().any(|call| matches!(call, Call::SessionDiscovery { target: t, .. } if t == target))
    }

    /// Pause or resume automatic session discovery on a connected target.
    pub fn set_session_discovery(&mut self, target: &TargetId, enabled: bool) {
        if self.session_discovery_pending(target) || !self.state.target(target)
            .is_some_and(|record| record.connected() && record.session_discovery.is_some_and(|on| on != enabled))
        {
            return;
        }
        let op = self.op_id();
        self.routed(target, m::DISCOVERY_CAPS, json!({ "opId": op, "update": { "autoDiscover": enabled } }),
            Call::SessionDiscovery { target: target.clone(), write: true });
    }

    /// A fresh operation id: this host, this window run, a counter (D159).
    pub fn op_id(&mut self) -> OpId {
        self.ops += 1;
        let me = self.state.client_id.clone().unwrap_or_default();
        if self.run.is_empty() {
            OpId::new(format!("op-{me}-{}", self.ops))
        } else {
            OpId::new(format!("op-{me}-{}-{}", self.run, self.ops))
        }
    }

    /// Sets this window run's unique part of operation ids (the window picks it at start from
    /// the clock and the process; `yaac-core` has no clock). Without it — tests, the demo — the
    /// ids are only unique within one run.
    pub fn set_run(&mut self, run: impl Into<String>) {
        self.run = run.into();
    }

    fn error(&mut self, key: Option<ThreadKey>, text: impl Into<String>) {
        let text = text.into();
        if self.errors.iter().any(|error| error.key == key && error.text == text) {
            return;
        }
        self.next_error += 1;
        self.errors.push(ClientError { id: self.next_error, key, text });
    }

    /// Dismiss one local notice. Stale clicks cannot remove another notice.
    pub fn dismiss_error(&mut self, id: u64, key: Option<&ThreadKey>) {
        self.errors.retain(|error| error.id != id || error.key.as_ref() != key);
    }

    // ── incoming ────────────────────────────────────────────────────────────────────────

    fn handle(&mut self, msg: Message) {
        match msg {
            Message::Response { id, result } => {
                if let Some(call) = self.pending.remove(&id) {
                    self.on_result(call, result);
                }
            }
            Message::Error { id, error } => {
                if let Some(call) = id.and_then(|id| self.pending.remove(&id)) {
                    // After a build mismatch the hub refuses every request with the same
                    // reason. Only the hello's answer says it (as the hello outcome, which each
                    // window words for its case); the others' would repeat it as errors.
                    if !matches!(call, Call::Hello)
                        && matches!(error.code.as_str(), hub_names::HUB_OUTDATED | hub_names::WINDOW_OUTDATED)
                    {
                        return;
                    }
                    self.on_error(call, &error);
                } else {
                    self.state.hub.last_error = Some(error.message);
                }
            }
            Message::Notification { method, params } => self.on_notification(&method, params),
            Message::Request { id, method, params } if method == attach::names::THEME_CALL => {
                match serde_json::from_value(params) {
                    Ok(call) => self.theme_calls.push((id,call)),
                    Err(e) => self.answer_window_call(id,Err(ErrorObject::new(ErrorCode::InvalidParams,e.to_string()))),
                }
            }
            Message::Request { id, method, params } if method == attach::names::BROWSER_CALL => {
                match serde_json::from_value::<attach::BrowserCallParams>(params) {
                    Ok(call) => self.browser_calls.push((id, call)),
                    Err(e) => {
                        let error = ErrorObject::new(ErrorCode::InvalidParams, format!("bad {method}: {e}"));
                        self.link.send(Message::Error { id: Some(id), error });
                    }
                }
            }
            Message::Request { id, method, .. } => {
                let err = ErrorObject::new(ErrorCode::MethodNotFound, format!("the window does not serve {method}"));
                self.link.send(Message::Error { id: Some(id), error: err });
            }
        }
    }

    fn on_result(&mut self, call: Call, result: Value) {
        match call {
            Call::Hello => {
                let Ok(hello) = serde_json::from_value::<HelloResult>(result) else { return };
                if hello.build != self.build {
                    // The hub accepted the hello but is another build: only `hub/shutdown` is
                    // allowed now. (An older window is refused with an error instead.)
                    self.state.hub.outcome = HelloOutcome::HubOutdated { hub_build: hello.build.clone() };
                    self.state.hub.build = Some(hello.build);
                    return;
                }
                self.state.hub.attached = true;
                self.state.hub.outcome = HelloOutcome::Attached;
                // The hub tells the window its own client id ("answered by you").
                if let Some(id) = hello.client_id {
                    self.state.client_id = Some(id);
                }
                self.state.hub.build = Some(hello.build);
                self.request(hub_names::SETTINGS_GET, json!({}), Call::HubSettings);
                self.request(hub_names::VIEW_GET, json!({}), Call::View);
                if self.request_feed {
                    self.request(attach::names::NOTIFICATIONS_SUBSCRIBE, json!({}), Call::RequestFeed);
                }
                // Tell the (new) hub what this window shows again.
                if let Some(cards) = self.visible_requests.take() { self.set_visible_requests(cards); }
                if let Some(keys) = self.showing.take() {
                    self.set_showing(keys);
                }
                self.request(crate::attachments::names::DRAFTS_LIST, json!({}), Call::DraftsList);
                self.catalog_list();
                self.load_rate_limits();
                self.load_ssh_records();
                let ids: Vec<(TargetId, bool)> =
                    hello.targets.iter().map(|t| (t.id.clone(), t.state.status == attach::ConnectionStatus::Connected)).collect();
                self.state.set_targets(hello.targets);
                self.state.hub.index_pending = ids.iter().map(|(id, _)| id.clone()).collect();
                for (id, _connected) in ids {
                    // Cached index and providers are served by the hub even while offline.
                    self.load_target(&id);
                }
            }
            Call::Targets => {
                if let Ok(r) = serde_json::from_value::<attach::TargetsListResult>(result) {
                    self.state.set_targets(r.targets);
                }
            }
            Call::Providers(t) => {
                if let Ok(r) = serde_json::from_value::<ProviderListResult>(result) {
                    self.state.apply_providers(&t, r.providers, true);
                    if let Some(rec) = self.state.targets.iter_mut().find(|x| x.entry.id == t) {
                        rec.providers_fresh = rec.connected();
                    }
                }
            }
            Call::SessionDiscovery { target, .. } => self.apply_session_discovery(&target, result),
            Call::InstanceList(t) => {
                if let Ok(r) = serde_json::from_value::<yaac_protocol::ProviderInstanceListResult>(result)
                    && let Some(rec) = self.state.targets.iter_mut().find(|x| x.entry.id == t)
                {
                    rec.instance_configs = Some(r.instances);
                }
            }
            Call::InstanceChanged(t) | Call::InstanceHome { target: t, .. } => {
                self.list_instances(&t);
                self.routed(&t, m::PROVIDER_LIST, json!({}), Call::Providers(t.clone()));
            }
            Call::ForkPreview { key, instance } => {
                let state = match serde_json::from_value::<yaac_protocol::ThreadForkPreviewResult>(result) {
                    Ok(r) => ForkPreviewState::Ready(r),
                    Err(e) => ForkPreviewState::Failed(format!("The preview could not be read: {e}")),
                };
                self.state.fork_previews.insert((key, instance), state);
            }
            Call::FsList { target, path } => {
                let state = match serde_json::from_value::<yaac_protocol::FsListResult>(result) {
                    Ok(r) => FsListingState::Ready(r),
                    Err(e) => FsListingState::Failed(e.to_string()),
                };
                self.state.fs_listings.insert((target, path), state);
            }
            Call::Skills(key) => {
                let state = match serde_json::from_value::<yaac_protocol::SkillsListResult>(result) {
                    Ok(r) => crate::skills::SkillsState::Ready(r.skills),
                    Err(e) => crate::skills::SkillsState::Failed(e.to_string()),
                };
                self.state.skills.insert(key, state);
            }
            Call::Explain(key, item, request) => self.finish_explanation(key, item, request, result.get("text").and_then(Value::as_str)),
            Call::View => {
                self.state.apply_view(&result);
                self.clear_shown_unseen();
            }
            Call::RequestFeed => {
                if let Ok(feed) = serde_json::from_value(result) {
                    self.state.hub.request_feed = Some(feed);
                }
            }
            Call::FeedRespond { .. } => {}
            Call::UiStateGet => {
                let values = result.get("values").and_then(Value::as_object).cloned().unwrap_or_default();
                self.ui_state = Some(values.into_iter().collect());
            }
            Call::HubSettings => {
                self.state.hub.explanations = result.pointer("/settings/explanations").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default();
                self.state.hub.notifications = result.pointer("/settings/notifications").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default();
                self.state.hub.always = result.get("settings").and_then(|s| s.get("always")).and_then(Value::as_bool);
                self.state.hub.tailscale_attach = result.pointer("/settings/tailscaleAttach").and_then(Value::as_bool);
                if let Some(status) = result.get("tailscale") {
                    self.state.hub.tailscale_status = crate::state::tailscale_status_line(status);
                }
            }
            Call::Projects(t) => {
                if let Ok(r) = serde_json::from_value::<yaac_protocol::ProjectListResult>(result) {
                    self.state.set_projects(&t, r.projects);
                }
            }
            Call::ProjectCreate { target, for_draft } => {
                let Ok(r) = serde_json::from_value::<yaac_protocol::ProjectResult>(result) else { return };
                let p = crate::projects::Project::from_wire(&target, r.project);
                let id = p.id.clone();
                if !self.state.projects.iter().any(|x| x.id == id) {
                    self.state.projects.push(p);
                }
                if let Some(d) = for_draft {
                    self.set_draft_project(d, &id);
                }
            }
            Call::Find { query } => self.on_find(&query, &result),
            Call::Shutdown => self.state.hub.shutdown_answered = true,
            Call::TargetChanged => {
                self.request(attach::names::TARGETS_LIST, json!({}), Call::Targets);
            }
            Call::Sync { target: t, connected } => {
                self.state.hub.index_pending.remove(&t);
                if !connected && self.state.target(&t).is_some_and(|target| target.connected()) {
                    return; // A fresh sync was requested on reconnect.
                }
                let Ok(r) = serde_json::from_value::<ThreadSyncResult>(result) else { return };
                let more = r.more;
                self.state.apply_sync(&t, r);
                if connected && !more
                    && let Some(target) = self.state.target_mut(&t).filter(|target| target.connected())
                {
                    target.commands_current = true;
                }
                if more {
                    self.sync(&t);
                } else {
                    let warm: Vec<ThreadKey> = self.warm.iter().filter(|k| k.target == t).cloned().collect();
                    for k in warm {
                        if self.state.threads.contains_key(&k) && !self.state.transcripts.contains_key(&k) {
                            self.state.transcripts.insert(k.clone(), crate::transcript::Transcript::new());
                            self.state.recent.push_back(k.clone());
                            self.subscribe(&k);
                        }
                    }
                }
            }
            Call::Subscribe(key) => {
                // The user may have left the ancestor before the worker subscription replied.
                if self.state.copies.detached.contains(&key) { return; }
                let Ok(r) = serde_json::from_value::<ThreadSubscribeResult>(result) else { return };
                let Some(t) = self.state.transcripts.get_mut(&key) else { return };
                t.control = Some(r.control);
                t.control_holder = r.control_holder;
                t.take_over_blocked = r.take_over_blocked;
                t.active_turn = r.active_turn;
                t.set_queue(r.queued, t.revision + 1);
                if r.effective.is_some() {
                    t.effective = r.effective;
                }
                if r.context.is_some() {
                    t.context = r.context;
                }
                let reset = match &r.mode {
                    SubscribeMode::Replay(rp) => {
                        let hints = self.state.hints.entry(key.clone()).or_default();
                        hints.replay_until = Some(rp.head_seq);
                        t.position.is_none()
                    }
                    _ => true,
                };
                if reset {
                    t.begin_reset();
                    self.read_page(&key, None);
                }
                self.refresh_budget_of(&key);
            }
            read @ (Call::Read(_) | Call::ReadAround(_, _) | Call::ReadOlder(_, _) | Call::ReadNewer(_, _)) => {
                self.on_history_page(read, None, result);
            }
            Call::HistoryTail { read, page } => self.on_history_page(*read, Some(*page), result),
            Call::Budget => {
                if let Ok(b) = serde_json::from_value::<MeshBudgetResult>(result) {
                    self.state.apply_budget(b);
                }
            }
            // The update's progress arrives with the target's state.
            Call::ModeChanged { key, root, batch } => {
                if let Some(s) = self.state.next_settings.get_mut(&key) { s.mode = None; }
                let offline = result.get("pending").and_then(Value::as_bool) == Some(true);
                self.mode_reply(batch, root, &key, offline, false);
            }
            Call::Respond { .. } | Call::Ignore | Call::UpdateDaemon(_) => {}
            Call::CatalogList => {
                if let Ok(r) = serde_json::from_value::<yaac_protocol::CatalogSkillsResult>(result) {
                    self.state.catalog_skills = r.skills;
                    self.state.catalog_error = None;
                }
            }
            Call::CatalogEdit(id) => self.catalog_reply = Some((id, Ok(result))),
            Call::Preview(id) => self.preview_replies.push((id, Ok(result))),
            Call::Troubleshoot(id) => {
                let report = serde_json::from_value(result).map_err(|_| "The target returned an invalid diagnostic report.".to_owned());
                self.troubleshoot_reply = Some((id, report));
            }
            Call::Ssh(id) => self.ssh_replies.push((id, Ok(result))),
            Call::SshRecords => self.on_ssh_records(&result),
            Call::WslDistros => self.on_wsl_distros(result),
            Call::TurnStart { key, draft, op } => {
                self.echo_answered(&key, &op, &result);
                self.sent(&DraftRef::Thread(key), &draft);
            }
            Call::CommandLogs(command) => {
                if let Some(log) = self.command_log.as_mut().filter(|l| l.command == command)
                    && let Ok(result) = serde_json::from_value::<yaac_protocol::CommandLogsResult>(result)
                {
                    if result.output.is_some() || log.result.is_none() {
                        log.result = Some(result);
                    }
                    log.error = None;
                }
            }
            Call::CommandStop { .. } => {}
            Call::Queue { key, queue_id, .. } => self.on_queue_result(&key, &queue_id),
            Call::UploadStart { draft, id, attempt } => self.on_upload_started(&draft, id, attempt, &result),
            Call::BlobBegin { draft, id, chunks } => self.on_blob_begun(&draft, id, chunks, &result),
            Call::BlobAppend { .. } => {}
            Call::BlobEnd { draft, id } => self.on_blob_ended(&draft, id, &result),
            Call::UploadList => self.on_upload_list(&result),
            Call::DraftsList => self.on_drafts_list(&result),
            Call::RateLimits => self.on_rate_limits(result),
            Call::ThreadStart { draft } => {
                let Some(thread_id) = result.get("threadId").and_then(Value::as_str).map(ThreadId::new) else { return };
                let Some(target) = draft.target else { return };
                let key = ThreadKey::new(target.clone(), thread_id.clone());
                if !self.state.threads.contains_key(&key) {
                    let title = draft.draft.text.lines().map(|l| l.trim_start_matches('#').trim()).find(|l| !l.is_empty()).unwrap_or("New thread").to_owned();
                    let now = Timestamp::new("");
                    let summary = ThreadSummary {
                        id: thread_id,
                        instance: draft.instance.clone().unwrap_or_default(),
                        title,
                        preview: String::new(),
                        cwd: draft.cwd.clone().unwrap_or_default(),
                        project: None,
                        worktree: None,
                        branch: None,
                        model: draft.model.clone(),
                        model_off: None,
                        state: ThreadState::Running,
                        control: Control::Mine,
                        control_holder: None,
                        forked_from: None,
                        linked_children: None,
                        artifacts: None,
                        spawned_by: None,
                        reports_to_parent: None,
                        root: None,
                        children: 0,
                        peers: 0,
                        origin_kind: yaac_protocol::OriginKind::Yaac,
                        created_at: now.clone(),
                        updated_at: now,
                        archived: false,
                        repo: None,
                        git: None,
                        agents: yaac_protocol::AgentsPolicy::default(),
                        native_id: None,
                        hold: None,
                        fast: draft.fast,
                        ultrafast: draft.ultrafast.then_some(true),
                        placement: None,
                        subagent: None,
                        unavailable: None,
                        permission_mode: None,
                        turn_active: None,
                        running_commands: None,
                    };
                    self.state.upsert_summary(&target, summary);
                }
                self.state.new_threads.retain(|d| d.id != draft.id);
                self.sent(&DraftRef::New(draft.id), &draft.draft);
                if self.state.selection == Selection::NewThread(draft.id) {
                    self.open(&key);
                }
            }
            Call::Delete(key) => self.deleted(&key),
            Call::Compact { key, command } => {
                if let Some(draft) = self.state.drafts.get_mut(&key)
                    && command.as_ref() == Some(&draft.text)
                {
                    draft.text.clear();
                    self.draft_changed(&DraftRef::Thread(key));
                }
            }
            Call::ImageChunk(id) => self.image_chunks.push((id, serde_json::from_value(result).map_err(|e| e.to_string()))),
            Call::ArtifactChunk(id) => self.artifact_chunks.push((id, serde_json::from_value(result).map_err(|e| e.to_string()))),
            Call::FsSearch(id) => self.finish_composer_search(id, serde_json::from_value(result).map_err(|e| e.to_string())),
            Call::TakeOver(key) => {
                if let Some(t) = self.state.transcripts.get_mut(&key) {
                    t.take_over = None;
                    // The control event may still be on its way: the thread is this window's now.
                    if t.control == Some(Control::External) {
                        t.control = Some(Control::Free);
                        t.control_holder = None;
                        t.take_over_blocked = None;
                    }
                    t.revision += 1;
                }
                if let Some(r) = self.state.threads.get_mut(&key)
                    && r.summary.control == Control::External
                {
                    r.summary.control = Control::Free;
                    r.summary.control_holder = None;
                }
            }
            Call::BulkArchive { batch, key } => self.archived_one(batch, &key, crate::bulk::Outcome::Ok),
            Call::Move { batch, key, .. } => {
                // The target's summary after the move (placement, project, and folder).
                if let Some(s) = result.get("thread").and_then(|t| serde_json::from_value::<ThreadSummary>(t.clone()).ok()) {
                    self.state.upsert_summary(&key.target, s);
                }
                self.moved_one(batch, &key, crate::bulk::Outcome::Ok);
            }
            c @ (Call::ForgePrs(_) | Call::ForgePrepare(_) | Call::ForgeCreate(_) | Call::ForgeAsk(_)) => self.on_forge_result(c, result),
            Call::Fork { target } => {
                if let Some(thread_id) = result.get("threadId").and_then(Value::as_str) {
                    let key = ThreadKey::new(target, thread_id);
                    self.open(&key);
                }
            }
        }
    }

    fn on_history_page(&mut self, read: Call, buffered: Option<ThreadReadResult>, result: Value) {
        let (key, forward, edge) = match &read {
            Call::Read(key) => (key, false, None),
            Call::ReadAround(key, _) => (key, true, None),
            Call::ReadOlder(key, cursor) => (key, false, Some(cursor)),
            Call::ReadNewer(key, cursor) => (key, true, Some(cursor)),
            _ => unreachable!("only interactive history can be buffered"),
        };
        let Some(t) = self.state.transcripts.get(key) else { return };
        let current = match edge {
            Some(cursor) if forward => t.loading_newer && t.newer_cursor.as_ref() == Some(cursor),
            Some(cursor) => t.loading_older && t.older_cursor.as_ref() == Some(cursor),
            None => t.loading.is_some(),
        };
        if !current { return; }

        let Ok(mut page) = serde_json::from_value::<ThreadReadResult>(result) else {
            self.on_error(read, &ErrorObject::new(ErrorCode::InternalError, "The target returned an invalid history page."));
            return;
        };
        let resumed = buffered.is_some();
        if let Some(mut first) = buffered {
            let next_item = if forward { page.items.last() } else { page.items.first() };
            let buffered_item = first.items.last();
            let repeated = if forward { page.newer_cursor == first.newer_cursor } else { page.cursor == first.cursor };
            if page.position.epoch != first.position.epoch
                || next_item.map(|item| &item.turn_id) != buffered_item.map(|item| &item.turn_id)
                || (page.continues_turn == Some(true) && repeated)
            {
                self.on_error(read, &ErrorObject::new(ErrorCode::InvalidParams, "The thread changed while loading history."));
                return;
            }
            if edge.is_none() && page.position.seq > first.position.seq
                && let Some(load) = self.state.transcripts.get_mut(key).and_then(|t| t.loading.as_mut())
            {
                load.covered_items.extend(page.items.iter().map(|item| (item.id.clone(), page.position.seq)));
            }
            if forward {
                // Buffer in traversal order, then reverse once when the entire turn is ready.
                first.items.extend(page.items.into_iter().rev());
                first.newer_cursor = page.newer_cursor;
            } else {
                first.items.append(&mut page.items);
                first.cursor = page.cursor;
            }
            first.uploads.append(&mut page.uploads);
            first.continues_turn = page.continues_turn;
            page = first;
        }

        if page.continues_turn == Some(true) {
            let cursor = if forward { &page.newer_cursor } else { &page.cursor };
            if let Some(cursor) = cursor {
                // One item, extended through the rest of its turn, so this read never pulls
                // another group into the batch.
                let params = json!({ "threadId": key.thread, "cursor": cursor, "maxItems": 1, "wholeTurns": true });
                let target = key.target.clone();
                if forward && !resumed { page.items.reverse(); }
                self.routed(&target, m::THREAD_READ, params, Call::HistoryTail { read: Box::new(read), page: Box::new(page) });
                return;
            }
            self.on_error(read, &ErrorObject::new(ErrorCode::InternalError, "The target did not return the rest of this turn."));
            return;
        }
        if forward && resumed { page.items.reverse(); }
        if let Some(required) = &mut page.required_items {
            let included: std::collections::HashSet<_> = page.items.iter().map(|item| &item.id).collect();
            required.retain(|item| !included.contains(&item.id));
        }
        self.apply_history_page(read, page);
    }

    fn apply_history_page(&mut self, read: Call, r: ThreadReadResult) {
        match read {
            read @ (Call::Read(_) | Call::ReadAround(_, _)) => {
                let (key, around) = match read {
                    Call::Read(key) => (key, None),
                    Call::ReadAround(key, id) => (key, Some(id)),
                    _ => unreachable!(),
                };
                let rev = self.state.next_stamp();
                let Some(t) = self.state.transcripts.get_mut(&key) else { return };
                if let Some(id) = &around
                    && !r.items.iter().any(|item| &item.id == id)
                {
                    t.cancel_reset(rev);
                    if self.state.focus_item.as_ref() == Some(&(key.clone(), id.clone())) {
                        self.state.focus_item = None;
                    }
                    self.error(Some(key), "The selected message is unavailable.".to_owned());
                    return;
                }
                if r.context.is_some() {
                    t.context = r.context;
                }
                let Some(load) = t.loading.as_mut() else { return };
                load.pages.extend(r.items);
                load.required.extend(r.required_items.into_iter().flatten());
                for u in r.uploads {
                    if !u.present {
                        load.missing.insert(u.upload_id);
                    }
                }
                t.older_cursor = r.cursor.map(|cursor| cursor.to_string());
                t.newer_cursor = r.newer_cursor.map(|cursor| cursor.to_string());
                t.history_away = t.newer_cursor.is_some();
                if around.is_none() || r.prompts.is_some() {
                    t.prompts = r.prompts;
                }
                let buffered = load.buffered.clone();
                t.finish_reset(r.position, rev);
                self.state.copies.dirty = true;
                let items = t.items().to_vec();
                let stamp = self.state.next_stamp();
                let hints = self.state.hints.entry(key.clone()).or_default();
                if around.is_none() && hints.position.is_none_or(|p| p.epoch != r.position.epoch || p.seq <= r.position.seq) {
                    hints.rebuild(&items, r.position, stamp);
                    for ev in &buffered {
                        hints.apply(ev, stamp);
                    }
                }
                self.load_history_focus(&key);
            }
            Call::ReadOlder(key, cursor) | Call::ReadNewer(key, cursor) => {
                let newer = cursor.strip_prefix("history-newer:");
                let rev = self.state.next_stamp();
                let Some(t) = self.state.transcripts.get_mut(&key) else { return };
                let (edge, loading) = if newer.is_some() { (&t.newer_cursor, &mut t.loading_newer) } else { (&t.older_cursor, &mut t.loading_older) };
                if edge.as_deref() != Some(cursor.as_str()) || !std::mem::take(loading) {
                    return;
                }
                if t.position.is_none_or(|position| position.epoch != r.position.epoch) {
                    self.latest_history(&key);
                    return;
                }
                t.missing_uploads.extend(r.uploads.into_iter().filter(|u| !u.present).map(|u| u.upload_id));
                if let Some(anchor) = newer {
                    t.newer_cursor = r.newer_cursor.map(|cursor| cursor.to_string());
                    t.history_away = t.newer_cursor.is_some();
                    t.append_history(&ItemId::new(anchor), r.items, rev);
                } else {
                    t.older_cursor = r.cursor.map(|cursor| cursor.to_string());
                    t.prepend_history(r.items, rev);
                }
                self.load_history_focus(&key);
            }
            _ => unreachable!("only interactive history can be applied here"),
        }
    }

    fn on_error(&mut self, call: Call, error: &ErrorObject) {
        if let Call::Sync { target, .. } = &call {
            // Answered, if only with an error: the index is as known as it will get.
            self.state.hub.index_pending.remove(target);
        }
        match call {
            Call::HistoryTail { read, .. } => self.on_error(*read, error),
            Call::SessionDiscovery { write: true, .. } => {
                if error.code == ErrorCode::OutcomeUnknown {
                    self.error(None, "The target disconnected while changing session discovery. Check the setting after it reconnects.");
                } else {
                    self.error(None, format!("Could not update session discovery: {}", error.message));
                }
            }
            Call::SessionDiscovery { write: false, .. } => {}
            Call::UpdateDaemon(target) => {
                let name = self.state.target_name(&target);
                // Never re-sent: the target's state shows how it went once it is back.
                let text = if error.code == ErrorCode::OutcomeUnknown {
                    format!("YAAC lost contact with {name} while asking yaacd to update. It may be updating; YAAC does not ask again.")
                } else {
                    format!("yaacd on {name} was not updated: {}", error.message)
                };
                self.error(None, text);
            }
            Call::ModeChanged { key, root, batch } => {
                let title = self.state.summary(&key).map_or(key.thread.as_str(), |s| &s.title);
                let message = format!("Mode change for {title}: {}", error.message);
                self.error(Some(root.clone()), message);
                self.mode_reply(batch, root, &key, false, true);
            }
            Call::Respond { key, request_id } => {
                let rev = self.state.next_stamp();
                if let Some(t) = self.state.transcripts.get_mut(&key) {
                    match error.code {
                        // Never re-sent: the card says so and offers "Check latest status".
                        ErrorCode::OutcomeUnknown => {
                            t.set_request_state(&request_id, RequestState::Unknown, rev);
                        }
                        // Someone else answered, or it expired: the harness's events will say.
                        ErrorCode::RequestNotPending => {}
                        _ => {
                            t.set_request_state(&request_id, RequestState::Pending, rev);
                            t.local_answers.remove(&request_id);
                            self.error(Some(key), format!("Your answer was not sent: {}", error.message));
                        }
                    }
                }
            }
            Call::CommandLogs(command) => {
                if let Some(log) = self.command_log.as_mut().filter(|l| l.command == command) {
                    log.error = Some(error.message.clone());
                }
            }
            Call::CommandStop { view, .. } => {
                let why = if error.code == ErrorCode::OutcomeUnknown {
                    "The harness did not confirm whether the command stopped.".to_owned()
                } else {
                    format!("Could not stop the command. {}", error.message)
                };
                self.error(Some(view), why);
            }
            Call::Queue { key, queue_id, op } => self.on_queue_error(key, &queue_id, op, error),
            Call::TurnStart { key, draft, op } => {
                // Said on the message's own bubble (D165), which offers Edit and, when nothing
                // was sent, Retry. Never re-sent on its own (invariant 10).
                let rev = self.state.next_stamp();
                let unknown = error.code == ErrorCode::OutcomeUnknown;
                let shown = self.state.transcripts.get_mut(&key).is_some_and(|t| {
                    t.update_pending(&op, rev, |p| {
                        p.state = if unknown { crate::sending::SendState::Unknown } else { crate::sending::SendState::Failed(error.message.clone()) };
                    })
                });
                if unknown {
                    self.sent(&DraftRef::Thread(key.clone()), &draft);
                    if !shown {
                        self.error(Some(key), "YAAC lost contact while sending your message. It may or may not have arrived; it will not be sent again.");
                    }
                } else if !shown {
                    // No bubble to say it on: give the text and the attachments back.
                    self.give_back(&key, draft);
                    self.error(Some(key), format!("Not sent: {}", error.message));
                }
            }
            Call::Compact { key, .. } => {
                let message = if error.code == ErrorCode::OutcomeUnknown {
                    "YAAC lost contact before the harness confirmed compaction. Check the transcript before trying again.".to_owned()
                } else {
                    format!("Could not compact context: {}", error.message)
                };
                self.error(Some(key), message);
            }
            Call::ImageChunk(id) => self.image_chunks.push((id, Err(error.message.clone()))),
            Call::ArtifactChunk(id) => self.artifact_chunks.push((id, Err(error.message.clone()))),
            Call::FsSearch(id) => self.finish_composer_search(id, Err(error.message.clone())),
            Call::TakeOver(key) => {
                // Shown on the banner, never retried on its own.
                let why = if error.code == ErrorCode::OutcomeUnknown {
                    "YAAC lost contact while taking the thread over. It may or may not be yours now; it will not be asked again.".to_owned()
                } else {
                    error.message.clone()
                };
                if let Some(t) = self.state.transcripts.get_mut(&key) {
                    t.take_over = Some(crate::transcript::TakeOverRequest::Failed(why));
                    t.revision += 1;
                }
            }
            Call::Delete(key) => match error.code {
                ErrorCode::OutcomeUnknown => {
                    self.error(Some(key), "YAAC lost contact while deleting the thread. It may or may not be deleted; it will not be asked again. Check the list once the target answers.");
                }
                ErrorCode::ThreadNotFound => self.deleted(&key),
                _ => self.error(Some(key), format!("Not deleted: {}", error.message)),
            },
            Call::BulkArchive { batch, key } => {
                let outcome = if error.code == ErrorCode::OutcomeUnknown { crate::bulk::Outcome::Uncertain } else { crate::bulk::Outcome::Failed(error.message.clone()) };
                self.archived_one(batch, &key, outcome);
            }
            Call::Move { batch, key, before } => {
                let outcome = if error.code == ErrorCode::OutcomeUnknown {
                    crate::bulk::Outcome::Uncertain
                } else {
                    // Refused: the row goes back where it was.
                    self.state.upsert_summary(&key.target, *before);
                    crate::bulk::Outcome::Failed(error.message.clone())
                };
                self.moved_one(batch, &key, outcome);
            }
            Call::UploadStart { draft, id, attempt } => self.upload_failed(&draft, id, attempt, error),
            Call::BlobBegin { draft, id, .. } | Call::BlobAppend { draft, id } | Call::BlobEnd { draft, id } => {
                self.upload_failed(&draft, id, u32::MAX, error);
            }
            Call::UploadList | Call::DraftsList => {}
            Call::ThreadStart { draft } => {
                if !self.state.new_threads.iter().any(|d| d.id == draft.id) {
                    self.state.new_threads.push(draft);
                }
                self.error(None, format!("The thread was not started: {}", error.message));
            }
            Call::ReadOlder(key, cursor) | Call::ReadNewer(key, cursor) => {
                let Some(t) = self.state.transcripts.get_mut(&key) else { return };
                let (edge, loading) = if cursor.starts_with("history-newer:") {
                    (&mut t.newer_cursor, &mut t.loading_newer)
                } else {
                    (&mut t.older_cursor, &mut t.loading_older)
                };
                if edge.as_deref() != Some(&cursor) || !std::mem::take(loading) { return; }
                *edge = None;
                let stale_anchor = matches!(
                    (&error.code, error.message.as_str()),
                    (ErrorCode::InvalidParams, "this message is no longer in the thread" | "The thread changed while loading history.")
                        | (ErrorCode::InternalError, "the message is not cached")
                );
                if stale_anchor {
                    // The cursor belongs to the previous view of the history. Refresh it once
                    // from the live end, without asking for the same page again.
                    self.latest_history(&key);
                } else {
                    self.error(Some(key), format!("Could not load this page of messages: {}", error.message));
                }
            }
            Call::Subscribe(key) | Call::Read(key) | Call::ReadAround(key, _) => {
                let rev = self.state.next_stamp();
                if let Some(t) = self.state.transcripts.get_mut(&key) {
                    t.cancel_reset(rev);
                }
                self.error(Some(key), format!("Could not load the thread: {}", error.message));
            }
            Call::Hello => {
                self.state.hub.outcome = match error.code.as_str() {
                    hub_names::WINDOW_OUTDATED => {
                        // The refusal names the hub's build; a phone uses it to offer the
                        // matching app (docs/14-android.md, Updates).
                        self.state.hub.build = refused_by_build(&error.message);
                        HelloOutcome::WindowOutdated { message: error.message.clone() }
                    }
                    hub_names::HUB_OUTDATED => HelloOutcome::HubOutdated { hub_build: String::new() },
                    _ => HelloOutcome::Pending,
                };
                self.state.hub.last_error = Some(error.message.clone());
            }
            Call::Shutdown => self.state.hub.shutdown_answered = true,
            Call::ForkPreview { key, instance } => {
                self.state.fork_previews.insert((key, instance), ForkPreviewState::Failed(error.message.clone()));
            }
            Call::FsList { target, path } => {
                self.state.fs_listings.insert((target, path), FsListingState::Failed(error.message.clone()));
            }
            Call::Skills(key) => {
                self.state.skills.insert(key, crate::skills::SkillsState::Failed(error.message.clone()));
            }
            Call::Explain(key, item, request) => self.finish_explanation(key, item, request, None),
            Call::HubSettings | Call::Projects(_) | Call::SshRecords | Call::RequestFeed => {}
            // An older hub has no such method: nothing is stored there, and nothing is said.
            Call::UiStateGet => self.ui_state = Some(Vec::new()),
            // An answer that did not go through opens the request with the reason, as a
            // failed toast answer does; nothing is re-sent.
            Call::FeedRespond { mut open, answered } => {
                if answered {
                    open.error = Some(error.message.clone());
                    self.error(None, error.message.clone());
                    self.notification_open.push_back(open);
                } else {
                    self.error(None, error.message.clone());
                }
            }
            Call::InstanceHome { target, instance, home } => {
                if error.code == ErrorCode::StoreChange {
                    // Another store: nothing changed; the user decides (P300).
                    let data = error.data.clone().and_then(|d| serde_json::from_value::<yaac_protocol::StoreChangeData>(d).ok());
                    self.state.home_confirm = Some(crate::settings::HomeConfirm {
                        target,
                        instance,
                        home,
                        current: data.as_ref().and_then(|d| d.current.as_ref()).map(ToString::to_string),
                        next: data.as_ref().and_then(|d| d.next.as_ref()).map(ToString::to_string),
                    });
                } else if error.code == ErrorCode::OutcomeUnknown {
                    self.error(None, "YAAC lost contact while changing the harness home. It may or may not have changed; it will not be asked again. Check Settings → Providers once the target answers.");
                } else {
                    self.error(None, format!("The harness home was not changed: {}", error.message));
                }
            }
            Call::CatalogList => self.state.catalog_error = Some(error.message.clone()),
            Call::CatalogEdit(id) => self.catalog_reply = Some((id, Err(error.message.clone()))),
            Call::Preview(id) => self.preview_replies.push((id, Err(error.message.clone()))),
            Call::Troubleshoot(id) => self.troubleshoot_reply = Some((id, Err(error.message.clone()))),
            Call::Ssh(id) => self.ssh_replies.push((id, Err(error.clone()))),
            Call::WslDistros => self.on_wsl_distros_error(error),
            c @ (Call::ForgePrs(_) | Call::ForgePrepare(_) | Call::ForgeCreate(_) | Call::ForgeAsk(_)) => self.on_forge_error(c, error),
            Call::Find { query } => {
                if let Some(f) = self.state.find.as_mut().filter(|f| f.query == query) {
                    f.loading = false;
                    f.error = Some(error.message.clone());
                }
            }
            _ => self.error(None, error.message.clone()),
        }
    }

    fn on_notification(&mut self, method: &str, params: Value) {
        if method == "hub/modeChange/result" {
            if let (Some(target), Some(thread), Some(error)) = (params["target"].as_str(), params["thread"].as_str(), params["error"].as_str()) {
                self.error(None, format!("Mode change for {target}/{thread}: {error}"));
            }
            return;
        }
        match method {
            attach::names::CATALOG_SKILLS_CHANGED => self.catalog_list(),
            "hub/approvals/settings" => {
                if let Ok(models) = serde_json::from_value(params) { self.state.hub.explanations = models; }
            }
            "hub/notifications/settings" => {
                if let Ok(settings) = serde_json::from_value(params) { self.state.hub.notifications = settings; }
            }
            "hub/tailscale/status" => self.state.hub.tailscale_status = crate::state::tailscale_status_line(&params),
            attach::names::NOTIFICATIONS_REQUESTS => {
                if let Ok(feed) = serde_json::from_value(params) {
                    self.state.hub.request_feed = Some(feed);
                }
            }
            attach::names::NOTIFICATIONS_OPEN => {
                if let Ok(open) = serde_json::from_value::<attach::NotificationOpen>(params) {
                    if let Some(text) = &open.error { self.error(None, text.clone()); }
                    self.notification_open.push_back(open);
                }
            }
            attach::names::TARGET_STATE => {
                let Ok(p) = serde_json::from_value::<TargetStateParams>(params) else { return };
                let was = self.state.target(&p.id).map(|t| t.connected());
                let id = p.id.clone();
                let now_connected = p.state.status == attach::ConnectionStatus::Connected;
                self.state.apply_target_state(p);
                if now_connected && was == Some(false) {
                    self.load_target(&id);
                    // Transcripts whose load failed while it was away (copies' workers too).
                    let missing: Vec<_> = self.state.transcripts.keys().filter(|k| k.target == id
                        && !self.state.copies.detached.contains(*k) && self.stalled(k)).cloned().collect();
                    for key in missing { self.subscribe(&key); }
                    self.resume_offline_uploads(&id);
                }
                // The entry (enabled, transport) may have changed with the state: refresh it.
                self.request(attach::names::TARGETS_LIST, json!({}), Call::Targets);
            }
            hub_names::SSH_CHANGED => self.load_ssh_records(),
            attach::names::RATE_LIMITS_CHANGED => self.on_rate_limits(params),
            hub_names::TARGETS_CHANGED => {
                if let Ok(r) = serde_json::from_value::<attach::TargetsListResult>(params) {
                    self.state.set_targets(r.targets);
                }
            }
            hub_names::PROJECTS_CHANGED => {
                if let Some(t) = params.get("target").and_then(Value::as_str).map(TargetId::new) {
                    self.routed(&t, m::PROJECT_LIST, json!({}), Call::Projects(t.clone()));
                }
            }
            hub_names::VIEW_CHANGED => {
                self.state.apply_view(&params);
                self.clear_shown_unseen();
            }
            hub_names::INDEX_CHANGED => {
                if let Some(t) = params.get("target").and_then(Value::as_str).map(TargetId::new) {
                    self.sync(&t);
                }
            }
            hub_names::PROVIDERS_CHANGED => {
                if let Some(t) = params.get("target").and_then(Value::as_str).map(TargetId::new) {
                    self.routed(&t, m::PROVIDER_LIST, json!({}), Call::Providers(t.clone()));
                }
            }
            attach::names::NOTIFICATION => {
                let Ok(n) = serde_json::from_value::<RoutedNotification>(params) else { return };
                self.on_routed(n);
            }
            crate::attachments::names::UPLOAD_PROGRESS | crate::attachments::names::UPLOAD_DONE => self.on_upload_note(method, &params),
            attach::names::PREVIEW_TUNNEL_CLOSE => self.tunnel_notes.push((method.to_owned(), params)),
            m::STREAM_CREDIT
                if params.get("stream").and_then(|s| serde_json::from_value(s.clone()).ok()).is_some_and(|s| self.tunnel_streams.contains(&s)) =>
            {
                self.tunnel_notes.push((method.to_owned(), params));
            }
            attach::names::TERMINAL_TITLE | attach::names::TERMINAL_EXITED | attach::names::TERMINAL_RENAMED | m::STREAM_CREDIT => {
                self.terminal_notes.push((method.to_owned(), params));
            }
            _ => {}
        }
    }

    fn on_routed(&mut self, n: RoutedNotification) {
        let target = n.target;
        match n.method.as_str() {
            m::DISCOVERY_UPDATED => self.apply_session_discovery(&target, n.params),
            m::THREAD_EVENT => {
                let Ok(ev) = serde_json::from_value::<ThreadEventParams>(n.params) else { return };
                let key = ThreadKey::new(target.clone(), ev.thread_id.clone());
                let stale_epoch = self
                    .state
                    .transcripts
                    .get(&key)
                    .and_then(|t| t.position)
                    .is_some_and(|p| p.epoch != ev.epoch && self.state.transcripts.get(&key).is_some_and(|t| t.loading.is_none()));
                let budget_notice = matches!(&ev.event,
                    ThreadEvent::ItemStarted(e) | ThreadEvent::ItemCompleted(e)
                        if matches!(&e.item.body, ItemBody::Notice(nn) if nn.notice_kind == NoticeKind::BudgetExhausted));
                self.state.apply_thread_event(&target, &ev);
                if stale_epoch {
                    // A new journal: rebuild from a snapshot.
                    if let Some(t) = self.state.transcripts.get_mut(&key) {
                        t.position = None;
                    }
                    self.subscribe(&key);
                }
                if budget_notice {
                    self.refresh_budget_of(&key);
                }
            }
            m::THREAD_UPDATED => {
                if let Ok(p) = serde_json::from_value::<ThreadUpdatedParams>(n.params) {
                    if let Some(t) = self.state.targets.iter_mut().find(|t| t.entry.id == target) {
                        t.sync_token = p.token;
                    }
                    self.state.upsert_summary(&target, p.thread);
                }
            }
            m::THREAD_REMOVED => {
                if let Ok(p) = serde_json::from_value::<ThreadRemovedParams>(n.params) {
                    self.state.remove_thread(&ThreadKey::new(target, p.thread_id));
                }
            }
            m::PROJECT_UPDATED => {
                if let Ok(r) = serde_json::from_value::<yaac_protocol::ProjectListResult>(n.params) {
                    self.state.set_projects(&target, r.projects);
                }
            }
            m::FORGE_PRS_CHANGED => self.on_forge_changed(&target, n.params),
            // Exhausted or freed (D164): what the window shows of a budget is always current.
            m::MESH_BUDGET_CHANGED => {
                if let Ok(b) = serde_json::from_value::<MeshBudgetResult>(n.params) {
                    self.state.apply_budget(b);
                }
            }
            m::PROVIDER_UPDATED => {
                if let Ok(p) = serde_json::from_value::<ProviderUpdatedParams>(n.params) {
                    self.state.apply_providers(&target, vec![p.provider], false);
                }
            }
            _ => {}
        }
    }

    // ── outgoing ────────────────────────────────────────────────────────────────────────

    fn load_target(&mut self, target: &TargetId) {
        if self.state.target(target).is_some_and(|record| record.connected()) {
            self.routed(target, m::DISCOVERY_CAPS, json!({}), Call::SessionDiscovery { target: target.clone(), write: false });
        }
        self.routed(target, m::PROVIDER_LIST, json!({}), Call::Providers(target.clone()));
        self.routed(target, m::PROJECT_LIST, json!({}), Call::Projects(target.clone()));
        self.sync(target);
    }

    fn sync(&mut self, target: &TargetId) {
        let token = self.state.target(target).map(|t| t.sync_token.clone()).unwrap_or_default();
        let connected = self.state.target(target).is_some_and(|t| t.connected());
        self.routed(target, m::THREAD_SYNC, json!({ "token": token }), Call::Sync { target: target.clone(), connected });
    }

    fn subscribe(&mut self, key: &ThreadKey) {
        let pos = self.state.transcripts.get(key).and_then(|t| t.position);
        let mut params = json!({ "threadId": key.thread });
        if let Some(p) = pos {
            params["epoch"] = json!(p.epoch);
            params["afterSeq"] = json!(p.seq);
        }
        self.routed(&key.target, m::THREAD_SUBSCRIBE, params, Call::Subscribe(key.clone()));
    }

    fn read_page(&mut self, key: &ThreadKey, cursor: Option<String>) {
        // A copy needs the outstanding requests and turn origin for the normal card.
        // thread/read includes older outstanding items alongside this newest whole turn.
        let max_items = if cursor.is_none() && self.state.copies.following.contains(key) { 1 } else { 120 };
        let mut params = json!({ "threadId": key.thread, "maxBytes": 262_144, "maxItems": max_items, "wholeTurns": true });
        let call = if let Some(cursor) = cursor {
            params["cursor"] = json!(cursor);
            if cursor.starts_with("history-newer:") { Call::ReadNewer(key.clone(), cursor) } else { Call::ReadOlder(key.clone(), cursor) }
        } else {
            Call::Read(key.clone())
        };
        self.routed(&key.target, m::THREAD_READ, params, call);
    }

    /// Move the transcript window toward older messages.
    pub fn older_history(&mut self, key: &ThreadKey) {
        let Some(t) = self.state.transcripts.get_mut(key) else { return };
        let Some(cursor) = t.older_cursor.clone().filter(|_| t.loading.is_none() && !t.loading_older && !t.loading_newer) else { return };
        t.loading_older = true;
        self.read_page(key, Some(cursor));
    }

    pub fn newer_history(&mut self, key: &ThreadKey) {
        let Some(t) = self.state.transcripts.get_mut(key) else { return };
        let Some(cursor) = t.newer_cursor.clone().filter(|_| t.loading.is_none() && !t.loading_older && !t.loading_newer) else { return };
        t.loading_newer = true;
        self.read_page(key, Some(cursor));
    }

    pub fn focus_history_item(&mut self, key: &ThreadKey, item: ItemId) {
        self.state.focus_item = Some((key.clone(), item));
        self.load_history_focus(key);
    }

    /// Seek directly to a prompt or linked message, keeping paging for older targets.
    pub fn load_history_focus(&mut self, key: &ThreadKey) {
        let Some((_, item)) = self.state.focus_item.as_ref().filter(|(k, _)| k == key) else { return };
        let Some(t) = self.state.transcripts.get_mut(key) else { return };
        if !t.is_loaded() || t.item(item).is_some() { return; }
        if t.prompts.is_some() {
            let item = item.clone();
            let params = json!({ "threadId": key.thread, "maxBytes": 262_144, "maxItems": 120, "wholeTurns": true, "around": item });
            t.begin_reset();
            self.routed(&key.target, m::THREAD_READ, params, Call::ReadAround(key.clone(), item));
        } else {
            self.older_history(key);
        }
    }

    /// Return to a fresh page at the live end.
    pub fn latest_history(&mut self, key: &ThreadKey) {
        if self.state.focus_item.as_ref().is_some_and(|(k, _)| k == key) {
            self.state.focus_item = None;
        }
        let Some(t) = self.state.transcripts.get_mut(key).filter(|t| t.loading.is_none()) else { return };
        t.begin_reset();
        self.read_page(key, None);
    }

    fn refresh_budget_of(&mut self, key: &ThreadKey) {
        if let Some(root) = root_of(&self.state, key) {
            self.refresh_budget(&root);
        }
    }

    /// Ask the root's target for an orchestration's budget.
    pub fn refresh_budget(&mut self, root: &Address) {
        let Some(target) = root.target() else { return };
        let op = self.op_id();
        let params = json!({ "opId": op, "root": root, "action": MeshBudgetAction::Status });
        self.routed(&target, m::MESH_BUDGET, params, Call::Budget);
    }

    /// "Continue": grant the orchestration another allowance.
    pub fn continue_budget(&mut self, root: &Address) {
        let Some(target) = root.target() else { return };
        let op = self.op_id();
        let params = json!({ "opId": op, "root": root, "action": MeshBudgetAction::Continue });
        self.routed(&target, m::MESH_BUDGET, params, Call::Budget);
    }

    /// Set, extend or remove the orchestration's time limit (P352): `None` removes it. Not a
    /// person acting: the clock runs on. Sent only where the root's target can do it.
    pub fn set_time_limit(&mut self, root: &Address, wall_clock_secs: Option<u64>) {
        let Some(target) = root.target() else { return };
        let op = self.op_id();
        let params = json!({ "opId": op, "root": root, "action": MeshBudgetAction::SetTimeLimit, "wallClockSecs": wall_clock_secs });
        self.routed(&target, m::MESH_BUDGET, params, Call::Budget);
    }

    /// Report pending requests whose cards are visible in the focused window.
    pub fn set_visible_requests(&mut self, mut cards: Vec<attach::RequestLocation>) {
        cards.sort();
        cards.dedup();
        if self.state.hub.explanations.main.is_some() {
            for location in &cards {
                let key = ThreadKey::new(location.target.clone(), location.thread_id.clone());
                let Some(transcript) = self.state.transcripts.get_mut(&key) else { continue };
                if transcript.explanations.contains_key(&location.request_id) { continue; }
                let Some((id, tool, input)) = transcript.items().iter().find_map(|item| {
                    let ItemBody::Request(r) = &item.body else { return None };
                    if r.request_id != location.request_id || r.state != RequestState::Pending { return None; }
                    let yaac_protocol::RequestBody::Approval(a) = &r.body else { return None };
                    if a.command_explanation.is_some() { return None; }
                    let (tool, input) = crate::requests::explanation_input(a)?;
                    Some((item.id.clone(), tool, input))
                }) else { continue };
                transcript.explanations.insert(location.request_id.clone(), true);
                self.routed(&location.target, m::APPROVAL_EXPLAIN,
                    json!({"tool":tool,"input":input,"models":self.state.hub.explanations}),
                    Call::Explain(key, id, location.request_id.clone()));
            }
        }
        if self.visible_requests.as_ref() != Some(&cards) {
            self.request(attach::names::NOTIFICATIONS_VISIBLE, json!(cards), Call::Ignore);
            self.visible_requests = Some(cards);
        }
    }

    /// Which threads this window shows now (the open thread, when the thread view is on
    /// screen). Reported to the hub when it changes (`hub/view/showing`), so a turn that ends
    /// in a thread no window shows is marked unseen (D114); a shown thread's mark is cleared.
    fn finish_explanation(&mut self, key: ThreadKey, id: ItemId, request: RequestId, text: Option<&str>) {
        let rev = self.state.next_stamp();
        let Some(t) = self.state.transcripts.get_mut(&key) else { return };
        let Some(mut item) = t.item(&id).cloned() else { return };
        let ItemBody::Request(r) = &mut item.body else { return };
        if r.request_id != request || r.state != RequestState::Pending { return; }
        let yaac_protocol::RequestBody::Approval(a) = &mut r.body else { return };
        a.command_explanation = text.map(str::to_owned);
        t.explanations.insert(request, false);
        t.upsert(item, rev);
    }

    pub fn set_showing(&mut self, keys: Vec<ThreadKey>) {
        if self.showing.as_ref() == Some(&keys) {
            return;
        }
        let threads: Vec<Value> = keys.iter().map(|k| json!({ "target": k.target, "threadId": k.thread })).collect();
        self.request(hub_names::VIEW_SHOWING, json!({ "threads": threads }), Call::Ignore);
        self.showing = Some(keys);
        self.follow_copies();
        self.clear_shown_unseen();
    }

    fn follow_copies(&mut self) {
        self.state.copies.dirty = false;
        let workers = self.showing.iter().flatten()
            .flat_map(|ancestor| crate::copies::sources(&self.state, ancestor))
            .map(|(key, _)| key).collect();
        let previous = std::mem::replace(&mut self.state.copies.sources, workers);
        let added: Vec<_> = self.state.copies.sources.difference(&previous).cloned().collect();
        for worker in previous.difference(&self.state.copies.sources).cloned().collect::<Vec<_>>() {
            if self.state.copies.following.remove(&worker) {
                self.routed(&worker.target, m::THREAD_UNSUBSCRIBE, json!({"threadId": worker.thread}), Call::Ignore);
                if self.state.transcripts.contains_key(&worker) {
                    self.state.copies.detached.insert(worker);
                }
            }
        }
        for worker in added {
            if !self.state.transcripts.contains_key(&worker) || self.state.copies.detached.contains(&worker) {
                self.watch_transcript(&worker);
                self.state.copies.following.insert(worker);
            }
        }
        for worker in &self.state.copies.sources {
            if let Some(hints) = self.state.hints.get(worker) {
                self.state.copies.shown.extend(hints.requests.iter().filter(|(_, r)| crate::hints::request_is_open(&r.state))
                    .map(|(id, _)| (worker.clone(), id.clone())));
            }
        }
    }

    /// A thread on screen is seen: its unseen mark goes (here and in the hub).
    fn clear_shown_unseen(&mut self) {
        let shown: Vec<ThreadKey> = self.showing.iter().flatten().filter(|k| self.state.is_unseen(k)).cloned().collect();
        for k in shown {
            self.set_view(&k, crate::state::ViewPatch { unseen: Some(false), ..Default::default() });
        }
    }

    /// Delete a thread (`thread/delete`, D110): YAAC's record always, the harness's session
    /// too when `also_in_harness`. One operation id; an uncertain outcome is shown, never
    /// re-sent. Once deleted, the thread leaves the list, the history, and the view
    /// preferences.
    pub fn delete(&mut self, key: &ThreadKey, also_in_harness: bool) {
        let op = self.op_id();
        let params = json!({ "opId": op, "threadId": key.thread, "alsoInHarness": also_in_harness });
        self.routed(&key.target, m::THREAD_DELETE, params, Call::Delete(key.clone()));
    }

    /// **Take over** a thread another program holds (`thread/takeOver`, D140): one click, no
    /// confirmation. The banner shows it in flight, then goes away, or shows the reason.
    pub fn take_over(&mut self, key: &ThreadKey) {
        if self.state.transcripts.get(key).is_some_and(|t| t.take_over == Some(crate::transcript::TakeOverRequest::InFlight)) {
            return;
        }
        let op = self.op_id();
        let params = json!({ "opId": op, "threadId": key.thread });
        self.routed(&key.target, m::THREAD_TAKE_OVER, params, Call::TakeOver(key.clone()));
        if let Some(t) = self.state.transcripts.get_mut(key) {
            t.take_over = Some(crate::transcript::TakeOverRequest::InFlight);
            t.revision += 1;
        }
    }

    fn deleted(&mut self, key: &ThreadKey) {
        if self.state.view.get(key).is_some_and(|v| *v != crate::state::ViewPref::default()) {
            let all_off = crate::state::ViewPatch { pinned: Some(false), muted: Some(false), muted_until: Some(None), dismissed: Some(false), unseen: Some(false) };
            self.set_view(key, all_off);
        }
        self.state.view.remove(key);
        if self.state.selection == Selection::Thread(key.clone()) {
            self.state.selection = Selection::None;
        }
        self.state.remove_thread(key);
    }

    /// Open a thread: select it, keep its transcript, subscribe if it is not loaded.
    pub fn open(&mut self, key: &ThreadKey) {
        self.state.selection = Selection::Thread(key.clone());
        self.state.clear_arrival(key);
        if self.state.is_unseen(key) {
            self.set_view(key, crate::state::ViewPatch { unseen: Some(false), ..Default::default() });
        }
        self.watch_transcript(key);
    }

    /// Load and follow a transcript while leaving selection and read state alone.
    pub fn watch_transcript(&mut self, key: &ThreadKey) {
        self.state.copies.following.remove(key);
        let detached = self.state.copies.detached.remove(key);
        let fresh = !self.state.transcripts.contains_key(key);
        if fresh {
            self.state.transcripts.insert(key.clone(), crate::transcript::Transcript::new());
        }
        self.state.touch(key);
        if fresh || detached || self.stalled(key) {
            self.subscribe(key);
        }
    }

    /// A transcript that never loaded and has nothing on its way: its subscription or first
    /// page failed (the target was offline, the connection dropped). Opening it, or its target
    /// reconnecting, subscribes again; otherwise it would say "Loading…" for good. Reads only,
    /// so this is not a retry of anything that changes state.
    fn stalled(&self, key: &ThreadKey) -> bool {
        self.state.transcripts.get(key).is_some_and(|t| t.position.is_none() && t.loading.is_none())
            && !self.pending.values().any(|call| matches!(call,
                Call::Subscribe(k) | Call::Read(k) | Call::ReadAround(k, _) if k == key))
    }

    /// Re-subscribe from the current position to learn the latest state of everything —
    /// "Check latest status" on an outcome-unknown card. Sends nothing else.
    pub fn check_latest(&mut self, key: &ThreadKey) {
        self.subscribe(key);
    }

    /// **Update now**: ask the hub to have the target's older `yaacd` hand over at once
    /// (running turns are interrupted, marked as interrupted by an update). Sent once; the
    /// window asks first when turns are running ([`crate::components::update_now_dialog`]).
    pub fn update_daemon(&mut self, target: &TargetId) {
        self.request(attach::names::TARGETS_UPDATE_DAEMON, json!({ "id": target }), Call::UpdateDaemon(target.clone()));
    }

    /// Ask the hub to reconnect a target now.
    pub fn retry_target(&mut self, target: &TargetId) {
        let params = json!({ "id": target, "enabled": true });
        self.request(attach::names::TARGETS_ENABLE, params, Call::Ignore);
    }

    /// Refresh the hub's component snapshot and skip the backoff for enabled,
    /// disconnected targets. The hub keeps ownership of their existing connections.
    pub fn refresh_components(&mut self) {
        use attach::ConnectionStatus;
        if !self.state.hub.attached {
            return;
        }
        let retry: Vec<_> = self.state.targets.iter()
            .filter(|t| t.entry.enabled && matches!(t.entry.state.status, ConnectionStatus::Offline | ConnectionStatus::Stopped))
            .map(|t| t.entry.id.clone())
            .collect();
        for id in retry {
            self.request(hub_names::TARGETS_RECONNECT, json!({ "id": id }), Call::Ignore);
        }
        self.request(attach::names::TARGETS_LIST, json!({}), Call::Targets);
    }

    /// Answer a request. The card shows "submitted" at once; a lost answer becomes "outcome
    /// unknown" and is never re-sent.
    pub fn respond(&mut self, key: &ThreadKey, request_id: &RequestId, response: &RequestResponse, summary: String) {
        self.respond_from(key, key, request_id, response, summary);
    }

    /// Answer a worker's request from the card in `view`.
    pub fn respond_from(&mut self, view: &ThreadKey, key: &ThreadKey, request_id: &RequestId, response: &RequestResponse, summary: String) {
        if !self.state.may_answer(key) { return; }
        // Two views can deliver actions for the same card before either view redraws.
        if self.state.transcripts.get(key).is_some_and(|t| t.items().iter().any(|item|
            matches!(&item.body, ItemBody::Request(r) if &r.request_id == request_id && r.state != RequestState::Pending))) {
            return;
        }
        self.state.copies.answered_in.insert((key.clone(), request_id.clone()), view.clone());
        let rev = self.state.next_stamp();
        if let Some(t) = self.state.transcripts.get_mut(key) {
            t.set_request_state(request_id, RequestState::Submitted, rev);
            t.local_answers.insert(request_id.clone(), crate::transcript::LocalAnswer { summary, verb: None });
        }
        let op = self.op_id();
        let params = json!({ "opId": op, "threadId": key.thread, "requestId": request_id, "response": response });
        self.routed(&key.target, m::REQUEST_RESPOND, params, Call::Respond { key: key.clone(), request_id: request_id.clone() });
    }

    /// Stop the running turn (by turn id, so a late Stop never hits the next turn).
    pub fn interrupt(&mut self, key: &ThreadKey) {
        let Some(turn) = self.state.transcripts.get(key).and_then(|t| t.active_turn.clone()) else { return };
        let params = json!({ "threadId": key.thread, "turnId": turn });
        self.routed(&key.target, m::TURN_INTERRUPT, params, Call::Ignore);
    }

    // ── The echo of a sent message (D165) ────────────────────────────────────────────────

    /// The message appears at once, at the end of the transcript, "Sending…".
    fn echo(&mut self, key: &ThreadKey, op: &OpId, input: Vec<yaac_protocol::InputPart>, draft: &Draft, method: &str, params: &Value) {
        let rev = self.state.next_stamp();
        let mut pending = crate::sending::Pending::new(op.clone(), input, draft.clone(), method, params.clone());
        // Steered: the turn it goes into is known now.
        pending.turn = params.get("turnId").and_then(Value::as_str).map(yaac_protocol::TurnId::new);
        self.state.transcripts.entry(key.clone()).or_default().add_pending(pending, rev);
    }

    /// `yaacd` answered the send: queued, it leaves the transcript (the queue shows it);
    /// otherwise it waits for its own item, which replaces it.
    fn echo_answered(&mut self, key: &ThreadKey, op: &OpId, result: &Value) {
        let rev = self.state.next_stamp();
        let Some(t) = self.state.transcripts.get_mut(key) else { return };
        if result.get("outcome").and_then(Value::as_str) == Some("queued") {
            t.remove_pending(op, rev);
            return;
        }
        let turn = result.get("turnId").and_then(Value::as_str).filter(|s| !s.is_empty()).map(yaac_protocol::TurnId::new);
        t.update_pending(op, rev, |p| {
            p.state = crate::sending::SendState::Sent;
            if turn.is_some() {
                p.turn = turn;
            }
        });
        t.settle_pending(op, rev);
    }

    /// Text and attachments back in the composer: in place of an empty draft, else after it.
    fn give_back(&mut self, key: &ThreadKey, draft: Draft) {
        let slot = self.state.drafts.entry(key.clone()).or_default();
        if slot.is_empty() {
            *slot = draft;
        } else {
            if !draft.text.is_empty() {
                slot.text = format!("{}\n\n{}", slot.text, draft.message_text());
            }
            slot.attachments.extend(draft.attachments);
        }
        self.dirty.insert(DraftRef::Thread(key.clone()));
    }

    /// **Edit** on a message that was not sent (or may not have been): it leaves the
    /// transcript and its text and attachments go back to the composer.
    pub fn edit_send(&mut self, key: &ThreadKey, item: &ItemId) -> bool {
        let rev = self.state.next_stamp();
        let Some(t) = self.state.transcripts.get_mut(key) else { return false };
        let Some(op) = t.pending_of(item).filter(|p| p.settled()).map(|p| p.op.clone()) else { return false };
        let Some(p) = t.remove_pending(&op, rev) else { return false };
        self.give_back(key, p.draft);
        true
    }

    /// **Dismiss** on a message whose outcome is unknown: the bubble goes; nothing is sent.
    pub fn dismiss_send(&mut self, key: &ThreadKey, item: &ItemId) -> bool {
        let rev = self.state.next_stamp();
        let Some(t) = self.state.transcripts.get_mut(key) else { return false };
        let Some(op) = t.pending_of(item).filter(|p| p.state == crate::sending::SendState::Unknown).map(|p| p.op.clone()) else { return false };
        t.remove_pending(&op, rev).is_some()
    }

    /// **Retry** a message that was not sent: the same request under a new operation id. Only
    /// ever on the user's click, and never for an outcome that is unknown (invariant 10).
    pub fn retry_send(&mut self, key: &ThreadKey, item: &ItemId) -> bool {
        if !self.state.may_act(key) {
            return false;
        }
        let Some(old) = self.state.transcripts.get(key).and_then(|t| t.pending_of(item)).filter(|p| p.can_retry()).cloned() else { return false };
        let op = self.op_id();
        let rev = self.state.next_stamp();
        let mut params = old.params.clone();
        params["opId"] = json!(op);
        if let Some(t) = self.state.transcripts.get_mut(key) {
            t.update_pending(&old.op, rev, |p| {
                p.op = op.clone();
                p.params = params.clone();
                p.state = crate::sending::SendState::Sending;
            });
        }
        let call = Call::TurnStart { key: key.clone(), draft: old.draft, op };
        self.routed(&key.target, &old.method, params, call);
        true
    }

    /// Start a new-thread draft on a target and select it.
    pub fn new_thread(&mut self, target: TargetId, cwd: Option<yaac_protocol::TargetPath>) -> u64 {
        self.new_draft(Some(target), cwd)
    }

    /// Open the new-thread composer before the user has picked a project.
    pub fn blank_thread(&mut self) -> u64 {
        self.new_draft(None, None)
    }

    fn new_draft(&mut self, target: Option<TargetId>, cwd: Option<yaac_protocol::TargetPath>) -> u64 {
        let id = self.state.new_threads.iter().map(|d| d.id).max().unwrap_or(0) + 1;
        self.state.new_threads.push(NewThreadDraft {
            id,
            target,
            project: None,
            cwd,
            instance: None,
            model: None,
            effort: None,
            mode: None,
            fast: false,
            ultrafast: false,
            fast_note: None,
            draft: Draft::default(),
        });
        self.state.selection = Selection::NewThread(id);
        id
    }

    /// Whether this thread advertises native context compaction.
    pub fn offers_compact(&self, key: &ThreadKey) -> bool {
        self.state.transcripts.get(key).and_then(|t| t.effective.as_ref())
            .is_some_and(|effective| effective.capabilities.contains(&yaac_protocol::Capability::Compact))
    }

    /// A manual compaction request is waiting for native confirmation.
    pub fn compacting(&self, key: &ThreadKey) -> bool {
        self.pending.values().any(|call| matches!(call, Call::Compact { key: active, .. } if active == key))
    }

    /// Run native compaction without replacing the thread or sending a user message.
    pub fn compact(&mut self, key: &ThreadKey) -> bool {
        if self.compacting(key) || !self.state.may_act(key) {
            return false;
        }
        let unavailable = if !self.offers_compact(key) {
            Some("This thread does not support context compaction.")
        } else if self.state.summary(key).is_some_and(|s| s.hold.is_some()) {
            Some("The thread is stopped and held.")
        } else if crate::queue::running(&self.state, key) {
            Some("Wait for the current turn to finish before compacting context.")
        } else if crate::queue::awaiting_user(&self.state, key) {
            Some("Answer the pending request before compacting context.")
        } else {
            None
        };
        if let Some(message) = unavailable {
            self.error(Some(key.clone()), message);
            return false;
        }
        let command = self.state.drafts.get(key).filter(|draft| crate::compose::is_compact_command(&draft.text))
            .map(|draft| draft.text.clone());
        let params = yaac_protocol::ThreadCompactParams { op_id: self.op_id(), thread_id: key.thread.clone() };
        self.routed(&key.target, m::THREAD_COMPACT, json!(params), Call::Compact { key: key.clone(), command });
        true
    }

    /// Resolve the working directory on the target serving this composer.
    pub fn composer_cwd(&self, draft: &DraftRef) -> Option<(TargetId, yaac_protocol::TargetPath)> {
        match draft {
            DraftRef::Thread(key) => Some((key.target.clone(), self.state.summary(key)?.cwd.clone())),
            DraftRef::New(id) => {
                let draft = self.state.new_threads.iter().find(|d| d.id == *id)?;
                Some((draft.target.clone()?, draft.cwd.clone()?))
            }
        }
    }

    /// Format a picked path for the provider that will receive the message.
    pub fn composer_file_reference(&self, draft: &DraftRef, path: &str) -> String {
        use yaac_protocol::{Capability, ProviderStatus, has_capability};

        let capabilities = match draft {
            DraftRef::Thread(key) => self.state.transcripts.get(key)
                .and_then(|transcript| transcript.effective.as_ref())
                .map(|effective| effective.capabilities.as_slice())
                .or_else(|| self.state.provider_of(key).map(|p| p.capabilities.as_slice())),
            DraftRef::New(id) => (|| {
                let draft = self.state.new_threads.iter().find(|draft| draft.id == *id)?;
                let target = self.state.target(draft.target.as_ref()?)?;
                let provider = match &draft.instance {
                    Some(instance) => target.provider(instance),
                    None => target.providers.iter().find(|p| p.status == ProviderStatus::Ready),
                }?;
                Some(provider.capabilities.as_slice())
            })(),
        };
        let native = capabilities.is_some_and(|caps| has_capability(caps, &Capability::NativeFileMentions));
        if native && !path.contains('"') && !path.chars().any(char::is_control) {
            return format!(r#"@"{path}""#);
        }
        crate::compose::file_reference(path)
    }

    /// Ask the existing target connection for files under this composer's working directory.
    pub fn search_composer_files(&mut self, draft: &DraftRef, query: &str) {
        let Some((target, cwd)) = self.composer_cwd(draft) else { return };
        let params = yaac_protocol::FsSearchParams { cwd, query: query.to_owned() };
        if self.composer_files.as_ref().is_some_and(|search| &search.draft == draft
            && search.target == target && search.params == params && search.result.is_none()) {
            return;
        }
        let id = self.state.next_stamp();
        self.composer_files = Some(ComposerFiles { draft: draft.clone(), target: target.clone(), params: params.clone(), result: None, id });
        self.routed(&target, m::FS_SEARCH, json!(params), Call::FsSearch(id));
    }

    /// Only show results for the current draft and its current working directory.
    pub fn composer_files(&self, draft: &DraftRef) -> Option<&ComposerFiles> {
        let search = self.composer_files.as_ref()?;
        let (target, cwd) = self.composer_cwd(draft)?;
        (&search.draft == draft && search.target == target && search.params.cwd == cwd).then_some(search)
    }

    fn finish_composer_search(&mut self, id: u64, result: Result<yaac_protocol::FsSearchResult, String>) {
        if let Some(search) = self.composer_files.as_mut().filter(|search| search.id == id) {
            search.result = Some(result);
        }
    }

    /// Send a draft's first message: `thread/start` creates the thread and sends it, as one
    /// operation.
    pub fn start_thread(&mut self, draft_id: u64) -> bool {
        let Some(draft) = self.state.new_threads.iter().find(|d| d.id == draft_id).cloned() else { return false };
        if crate::compose::is_compact_command(&draft.draft.text) {
            self.error(None, "Start the conversation before compacting context.");
            return false;
        }
        let Some(target_id) = draft.target.as_ref() else { return false };
        let Some(target) = self.state.target(target_id) else { return false };
        let instance = draft.instance.clone().or_else(|| {
            target.providers.iter().find(|p| p.status == yaac_protocol::ProviderStatus::Ready).map(|p| p.instance.clone())
        });
        let Some(instance) = instance else { return false };
        let mut chosen = draft.clone();
        chosen.instance = Some(instance.clone());
        let support = crate::attachments::draft_image_support(&self.state, &chosen);
        let Some(input) = crate::attachments::input_parts(&draft.draft.text, &draft.draft.attachments, &support, Some(target_id)) else { return false };
        let input = crate::skills::attach(&self.state, crate::skills::for_new_thread(&self.state, &chosen), input, &draft.draft.text);
        let Some(cwd) = draft.cwd.clone() else { return false };
        let op = self.op_id();
        let mut params = json!({ "opId": op, "instance": instance, "cwd": cwd, "input": input });
        // Linked checkouts are resolved from their working directory. The project shortcut
        // on thread/start requires the project's registered folder.
        if let Some(p) = draft.project.as_ref().and_then(|pid| self.state.projects.iter().find(|p| &p.id == pid))
            && cwd.as_str() == p.folder
        {
            params["project"] = json!(p.wire_id);
        }
        if let Some(mdl) = &draft.model {
            params["model"] = json!(mdl);
        }
        if let Some(e) = &draft.effort {
            params["effort"] = json!(e);
        }
        if let Some(md) = &draft.mode {
            params["mode"] = json!(md);
        }
        if draft.fast {
            params["fast"] = json!(true);
        }
        if draft.ultrafast {
            params["ultrafast"] = json!(true);
        }
        let mut sent = draft.clone();
        sent.instance = Some(instance);
        self.routed(target_id, m::THREAD_START, params, Call::ThreadStart { draft: sent });
        true
    }

    /// A `thread/update` patch, applied to the summary at once; the target's `thread/updated`
    /// confirms it.
    fn update_thread(&mut self, key: &ThreadKey, patch: Value, local: impl FnOnce(&mut ThreadSummary)) {
        if let Some(r) = self.state.threads.get_mut(key) {
            local(&mut r.summary);
        }
        let op = self.op_id();
        let mut params = json!({ "opId": op, "threadId": key.thread });
        if let (Some(p), Value::Object(fields)) = (params.as_object_mut(), patch) {
            p.extend(fields);
        }
        self.routed(&key.target, m::THREAD_UPDATE, params, Call::Ignore);
    }

    /// Change a thread's view preferences: applied at once, and stored by the hub
    /// (`hub/view/set`, see the SEAM on [`hub_names::VIEW_SET`]).
    pub fn set_view(&mut self, key: &ThreadKey, patch: crate::state::ViewPatch) {
        let params = self.view_entry(key, &patch);
        self.request(hub_names::VIEW_SET, params, Call::Ignore);
    }

    /// Change several threads' view preferences in one `hub/view/setMany` (D138): applied at
    /// once, stored by the hub as one change.
    pub fn set_view_many(&mut self, changes: &[(ThreadKey, crate::state::ViewPatch)]) {
        if changes.is_empty() {
            return;
        }
        let entries: Vec<Value> = changes.iter().map(|(k, p)| self.view_entry(k, p)).collect();
        self.request(hub_names::VIEW_SET_MANY, json!({ "entries": entries }), Call::Ignore);
    }

    /// Apply a patch locally; the wire entry for it.
    fn view_entry(&mut self, key: &ThreadKey, patch: &crate::state::ViewPatch) -> Value {
        let v = self.state.view.entry(key.clone()).or_default();
        let mut params = json!({ "target": key.target, "threadId": key.thread });
        if let Some(p) = patch.pinned {
            v.pinned = p;
            params["pinned"] = json!(p);
        }
        if let Some(m) = patch.muted {
            v.muted = m;
            params["muted"] = json!(m);
        }
        if let Some(u) = patch.muted_until {
            v.muted_until = u;
            params["mutedUntil"] = match u {
                Some(t) => json!(crate::time::format_rfc3339(t)),
                None => Value::Null,
            };
        }
        if let Some(d) = patch.dismissed {
            v.dismissed = d;
            params["dismissed"] = json!(d);
        }
        if let Some(u) = patch.unseen {
            v.unseen = u;
            params["unseen"] = json!(u);
        }
        params
    }

    /// Mute (with no end, or until a moment) or unmute a thread.
    pub fn set_muted(&mut self, key: &ThreadKey, muted: bool) {
        self.set_view(key, crate::state::ViewPatch { muted: Some(muted), muted_until: Some(None), ..Default::default() });
    }

    /// Mute until a moment (seconds since the epoch); `None` for no end.
    pub fn mute_until(&mut self, key: &ThreadKey, until: Option<i64>) {
        self.set_view(key, crate::state::ViewPatch { muted: Some(true), muted_until: Some(until), ..Default::default() });
    }

    /// Archive a thread (`thread/archive`, under `threadArchive`). An archived thread is
    /// listed only under Archived, so archiving also clears its pin and its mute (D93);
    /// unarchiving does not restore them.
    pub fn archive(&mut self, key: &ThreadKey) {
        let op = self.op_id();
        let params = json!({ "opId": op, "threadId": key.thread });
        self.routed(&key.target, m::THREAD_ARCHIVE, params, Call::Ignore);
        if let Some(r) = self.state.threads.get_mut(key) {
            r.summary.archived = true;
        }
        let v = self.state.view.get(key).cloned().unwrap_or_default();
        if v.pinned || v.muted {
            self.set_view(key, crate::state::ViewPatch { pinned: Some(false), muted: Some(false), muted_until: Some(None), ..Default::default() });
        }
    }

    /// Mute several threads (until a moment, or with no end) in one `hub/view/setMany` (D138).
    pub fn mute_many(&mut self, keys: &[ThreadKey], until: Option<i64>) {
        let patch = crate::state::ViewPatch { muted: Some(true), muted_until: Some(until), ..Default::default() };
        let changes: Vec<_> = keys.iter().map(|k| (k.clone(), patch)).collect();
        self.set_view_many(&changes);
    }

    /// Unmute several threads in one `hub/view/setMany` (D138).
    pub fn unmute_many(&mut self, keys: &[ThreadKey]) {
        let patch = crate::state::ViewPatch { muted: Some(false), muted_until: Some(None), ..Default::default() };
        let changes: Vec<_> = keys.iter().map(|k| (k.clone(), patch)).collect();
        self.set_view_many(&changes);
    }

    /// Archive several threads (D138): one `thread/archive` per thread, each with its own
    /// operation id and routed to its own target. Nothing is re-sent: once every answer is in,
    /// the pins and mutes of those archived go in one `hub/view/setMany`, and anything that
    /// failed or whose outcome is unknown is reported once ([`Client::bulk_report`]).
    pub fn archive_many(&mut self, keys: &[ThreadKey]) {
        if keys.is_empty() {
            return;
        }
        self.next_batch += 1;
        let batch = self.next_batch;
        self.batches.insert(batch, ArchiveBatch { outcomes: keys.iter().map(|k| (k.clone(), None)).collect() });
        self.bulk_report = None;
        for key in keys {
            let op = self.op_id();
            let params = json!({ "opId": op, "threadId": key.thread });
            self.routed(&key.target, m::THREAD_ARCHIVE, params, Call::BulkArchive { batch, key: key.clone() });
            if let Some(r) = self.state.threads.get_mut(key) {
                r.summary.archived = true;
            }
        }
    }

    /// One answer of a bulk Archive; the last one finishes it.
    fn archived_one(&mut self, batch: u64, key: &ThreadKey, outcome: crate::bulk::Outcome) {
        let Some(b) = self.batches.get_mut(&batch) else { return };
        let refused = matches!(outcome, crate::bulk::Outcome::Failed(_));
        if let Some(slot) = b.outcomes.iter_mut().find(|(k, o)| k == key && o.is_none()) {
            slot.1 = Some(outcome);
        }
        // A refusal leaves the thread where it was.
        if refused
            && let Some(r) = self.state.threads.get_mut(key)
        {
            r.summary.archived = false;
        }
        if b.outcomes.iter().any(|(_, o)| o.is_none()) {
            return;
        }
        let Some(b) = self.batches.remove(&batch) else { return };
        let off = crate::state::ViewPatch { pinned: Some(false), muted: Some(false), muted_until: Some(None), ..Default::default() };
        let clear: Vec<_> = b
            .outcomes
            .iter()
            .filter(|(k, o)| *o == Some(crate::bulk::Outcome::Ok) && self.state.view.get(k).is_some_and(|v| v.pinned || v.muted))
            .map(|(k, _)| (k.clone(), off))
            .collect();
        self.set_view_many(&clear);
        let titled: Vec<(String, crate::bulk::Outcome)> = b
            .outcomes
            .into_iter()
            .map(|(k, o)| (self.state.summary(&k).map_or_else(|| k.thread.to_string(), |s| s.title.clone()), o.unwrap_or(crate::bulk::Outcome::Uncertain)))
            .collect();
        self.bulk_report = crate::bulk::archive_report(&titled);
    }

    /// Bulk Archives still waiting for an answer.
    pub fn archives_in_flight(&self) -> usize {
        self.batches.len()
    }

    /// Move threads (D144): one `thread/move` per thread, each with its own operation id,
    /// never re-sent. Each row moves at once (the target's answer replaces it; a refusal puts
    /// it back), and anything refused or uncertain is reported once in the bar under the list.
    /// `None` for a destination forgets the placement (Undo of a thread that had none).
    /// `notice`: the bar's "Moved … · Undo" for a move that needed no confirmation.
    pub fn move_threads(&mut self, moves: &[(ThreadKey, Option<crate::moving::Destination>)], notice: Option<crate::moving::MoveNotice>) {
        if moves.is_empty() {
            return;
        }
        self.next_batch += 1;
        let batch = self.next_batch;
        self.moves.insert(batch, moves.iter().map(|(k, _)| (k.clone(), None)).collect());
        self.bulk_report = None;
        self.move_notice = notice;
        for (key, dest) in moves {
            let Some(before) = self.state.summary(key).cloned() else { continue };
            let op = self.op_id();
            let mut params = json!({ "opId": op, "threadId": key.thread });
            if let Some(d) = dest {
                params["project"] = json!(d.project);
                if let Some(w) = &d.worktree {
                    params["worktree"] = json!(w);
                }
            }
            self.routed(&key.target, m::THREAD_MOVE, params, Call::Move { batch, key: key.clone(), before: Box::new(before) });
            self.place(key, dest.as_ref());
        }
    }

    /// The row moves at once: its project (and checkout) as the target will report them.
    fn place(&mut self, key: &ThreadKey, dest: Option<&crate::moving::Destination>) {
        let project = dest.and_then(|d| self.state.projects.iter().find(|p| p.target == key.target && p.wire_id == d.project).cloned());
        let Some(r) = self.state.threads.get_mut(key) else { return };
        match (dest, project) {
            (Some(d), Some(p)) => {
                r.summary.project = Some(yaac_protocol::ProjectRef {
                    path: yaac_protocol::TargetPath::new(p.folder.clone()),
                    display_name: p.name.clone(),
                    cross_target_key: None,
                    kind: Some(yaac_protocol::ProjectKind::User),
                    id: Some(yaac_protocol::ProjectId::new(p.wire_id.clone())),
                    repo: None,
                });
                let root = d.worktree.clone().or_else(|| p.repo.as_ref().map(|x| x.root.clone()));
                let name = root.as_ref().and_then(|w| {
                    crate::moving::worktrees(&self.state, &p).into_iter().find(|c| &c.root == w).map(|c| c.label)
                });
                let r = self.state.threads.get_mut(key).expect("listed");
                r.summary.placement = Some(yaac_protocol::ThreadPlacement {
                    project: yaac_protocol::ProjectId::new(p.wire_id),
                    worktree: root.map(yaac_protocol::TargetPath::new),
                    worktree_name: name,
                });
            }
            // Forgotten: the folder decides again (the target says which project).
            _ => r.summary.placement = None,
        }
    }

    /// One answer of a move; the last one of its batch reports what did not succeed.
    fn moved_one(&mut self, batch: u64, key: &ThreadKey, outcome: crate::bulk::Outcome) {
        let Some(b) = self.moves.get_mut(&batch) else { return };
        if let Some(slot) = b.iter_mut().find(|(k, o)| k == key && o.is_none()) {
            slot.1 = Some(outcome);
        }
        if b.iter().any(|(_, o)| o.is_none()) {
            return;
        }
        let Some(b) = self.moves.remove(&batch) else { return };
        let titled: Vec<(String, crate::bulk::Outcome)> = b
            .into_iter()
            .map(|(k, o)| (self.state.summary(&k).map_or_else(|| k.thread.to_string(), |s| s.title.clone()), o.unwrap_or(crate::bulk::Outcome::Uncertain)))
            .collect();
        if let Some(r) = crate::bulk::report("moved", &titled) {
            // Something did not move: Undo would not undo what the user sees.
            self.move_notice = None;
            self.bulk_report = Some(r);
        }
    }

    /// Undo the last move (the bar's Undo): each thread back where it was.
    pub fn undo_move(&mut self) {
        let Some(n) = self.move_notice.take() else { return };
        self.move_threads(&n.undo, None);
    }

    /// Unarchive a thread (`thread/unarchive`).
    pub fn unarchive(&mut self, key: &ThreadKey) {
        let op = self.op_id();
        let params = json!({ "opId": op, "threadId": key.thread });
        self.routed(&key.target, hub_names::THREAD_UNARCHIVE, params, Call::Ignore);
        if let Some(r) = self.state.threads.get_mut(key) {
            r.summary.archived = false;
        }
    }

    /// Rename a thread (`thread/rename`, under `threadRename`).
    pub fn rename(&mut self, key: &ThreadKey, title: &str) {
        let op = self.op_id();
        let params = json!({ "opId": op, "threadId": key.thread, "title": title });
        self.routed(&key.target, m::THREAD_RENAME, params, Call::Ignore);
    }

    /// Pin or unpin a thread (a view preference kept by the hub).
    pub fn set_pinned(&mut self, key: &ThreadKey, pinned: bool) {
        self.set_view(key, crate::state::ViewPatch { pinned: Some(pinned), ..Default::default() });
    }

    /// Hide (or show again) a discovered thread in the window only.
    pub fn set_dismissed(&mut self, key: &ThreadKey, dismissed: bool) {
        self.set_view(key, crate::state::ViewPatch { dismissed: Some(dismissed), ..Default::default() });
    }

    /// Create a project from a folder on a target (`project/create`). Once created, the draft
    /// `for_draft` (if any) is put in it.
    pub fn create_project(&mut self, target: &TargetId, folder: &str, name: &str, for_draft: Option<u64>) {
        let op = self.op_id();
        let params = json!({ "opId": op, "folder": folder, "name": name });
        self.routed(target, m::PROJECT_CREATE, params, Call::ProjectCreate { target: target.clone(), for_draft });
    }

    /// Rename a project (`project/rename`); `project/updated` brings the new list.
    pub fn rename_project(&mut self, project_id: &str, name: &str) {
        let Some(p) = self.state.projects.iter().find(|p| p.id == project_id).cloned() else { return };
        let op = self.op_id();
        let params = json!({ "opId": op, "projectId": p.wire_id, "name": name });
        self.routed(&p.target, m::PROJECT_RENAME, params, Call::Ignore);
    }

    /// Delete a project (`project/delete`); its threads become temporary projects again.
    pub fn delete_project(&mut self, project_id: &str) {
        let Some(p) = self.state.projects.iter().find(|p| p.id == project_id).cloned() else { return };
        let op = self.op_id();
        self.routed(&p.target, m::PROJECT_DELETE, json!({ "opId": op, "projectId": p.wire_id }), Call::Ignore);
    }

    /// Search threads through the hub (`hub/threads/find`: by id or fuzzy title, paged), when
    /// attached to a real hub. An empty query clears the results.
    pub fn find(&mut self, query: &str) {
        let q = query.trim().to_owned();
        if q.is_empty() {
            self.state.find = None;
            return;
        }
        if self.state.find.as_ref().is_some_and(|f| f.query == q) {
            return;
        }
        self.state.find = Some(crate::state::FindState { query: q.clone(), loading: true, ..Default::default() });
        self.request(hub_names::THREADS_FIND, json!({ "query": q }), Call::Find { query: q });
    }

    /// The next page of the current search.
    pub fn find_more(&mut self) {
        let Some(f) = self.state.find.as_mut() else { return };
        let Some(cursor) = f.cursor.take() else { return };
        f.loading = true;
        let q = f.query.clone();
        self.request(hub_names::THREADS_FIND, json!({ "query": q, "cursor": cursor }), Call::Find { query: q });
    }

    fn on_find(&mut self, query: &str, result: &Value) {
        let Ok(page) = serde_json::from_value::<FindPage>(result.clone()) else { return };
        let mut keys = Vec::new();
        for h in page.results {
            let key = ThreadKey::new(h.target.clone(), h.thread.id.clone());
            // A match may be a thread this window has not indexed yet (another target's cache).
            if !self.state.threads.contains_key(&key) {
                self.state.upsert_summary(&h.target, h.thread);
            }
            keys.push(key);
        }
        let Some(f) = self.state.find.as_mut().filter(|f| f.query == query) else { return };
        f.results.extend(keys);
        f.total = page.total;
        f.cursor = page.cursor;
        f.loading = false;
    }

    /// Choose a draft's project, defaulting to the project's own checkout.
    pub fn set_draft_project(&mut self, draft: u64, project_id: &str) {
        let Some(p) = self.state.projects.iter().find(|p| p.id == project_id).cloned() else { return };
        let mut retarget = false;
        if let Some(d) = self.state.new_threads.iter_mut().find(|d| d.id == draft) {
            if d.target.as_ref() != Some(&p.target) {
                d.instance = None;
                d.model = None;
                d.effort = None;
                d.mode = None;
                d.ultrafast = false;
                retarget = true;
            }
            d.target = Some(p.target.clone());
            d.cwd = Some(p.folder.clone().into());
            d.project = Some(p.id);
        }
        // An upload id is only meaningful on the target that issued it: the attachments follow.
        if retarget {
            self.retarget_uploads(&DraftRef::New(draft));
        }
        self.dirty.insert(DraftRef::New(draft));
    }

    /// Choose a known checkout, keeping the project's relative folder within it.
    pub fn set_draft_worktree(&mut self, draft: u64, root: &str) {
        let Some(index) = self.state.new_threads.iter().position(|d| d.id == draft) else { return };
        let d = &self.state.new_threads[index];
        let Some(p) = self.state.projects.iter().find(|p| Some(&p.id) == d.project.as_ref()) else { return };
        let Some(worktree) = crate::moving::worktrees(&self.state, p).into_iter().find(|w| w.root == root) else { return };
        self.state.new_threads[index].cwd = Some(crate::moving::folder_for(p, (!worktree.own).then_some(root)).into());
        self.draft_changed(&DraftRef::New(draft));
    }

    /// Turn fast mode on or off for a thread's next turns (D125). Sent with the next
    /// `turn/start`; the target keeps it from then on.
    pub fn set_fast(&mut self, key: &ThreadKey, on: bool) {
        let n = self.state.next_settings.entry(key.clone()).or_default();
        n.fast = Some(on);
        n.ultrafast = Some(false);
        n.fast_note = None;
    }

    /// Apply a confirmed selection to subsequent messages in this session.
    pub fn set_ultrafast(&mut self, key: &ThreadKey) {
        let n = self.state.next_settings.entry(key.clone()).or_default();
        n.fast = Some(false);
        n.ultrafast = Some(true);
        n.fast_note = None;
    }

    /// Choose the model for a thread's next message. A model without fast mode turns fast
    /// mode off, with a quiet note.
    pub fn set_next_model(&mut self, key: &ThreadKey, model: yaac_protocol::ModelId) {
        let was = self.state.thread_fast(key);
        let was_ultrafast = self.state.thread_ultrafast(key);
        self.state.next_settings.entry(key.clone()).or_default().model = Some(model);
        let info = self.state.next_model_info(key).cloned();
        let (on, note) = crate::controls::reconcile_fast(info.as_ref(), was);
        let n = self.state.next_settings.entry(key.clone()).or_default();
        if on != was {
            n.fast = Some(on);
        }
        n.fast_note = note;
        if was_ultrafast && !info.as_ref().is_some_and(crate::controls::model_has_ultrafast) {
            n.ultrafast = Some(false);
            n.fast_note = Some("Ultrafast is off for this model.".to_owned());
        }
    }

    /// Turn fast mode on or off for a new-thread draft.
    pub fn set_draft_fast(&mut self, id: u64, on: bool) {
        if let Some(d) = self.state.new_threads.iter_mut().find(|d| d.id == id) {
            d.fast = on;
            d.ultrafast = false;
            d.fast_note = None;
        }
        self.draft_changed(&crate::attachments::DraftRef::New(id));
    }

    pub fn set_draft_ultrafast(&mut self, id: u64) {
        if let Some(d) = self.state.new_threads.iter_mut().find(|d| d.id == id) {
            d.fast = false;
            d.ultrafast = true;
            d.fast_note = None;
        }
        self.draft_changed(&crate::attachments::DraftRef::New(id));
    }

    /// Choose a draft's instance and model: effort is kept when the model offers it, the mode
    /// goes back to the default on another instance, and fast mode goes off (with a quiet note)
    /// when the model has none.
    pub fn set_draft_model(&mut self, id: u64, instance: yaac_protocol::InstanceId, model: yaac_protocol::ModelId) {
        let providers = self
            .state
            .new_threads
            .iter()
            .find(|d| d.id == id)
            .and_then(|d| self.state.target(d.target.as_ref()?))
            .map(|t| t.providers.clone())
            .unwrap_or_default();
        if let Some(d) = self.state.new_threads.iter_mut().find(|d| d.id == id) {
            let info = providers.iter().find(|p| p.instance == instance).and_then(|p| p.models.iter().find(|m| m.id == model));
            d.effort = info.and_then(|m| crate::controls::reconcile_effort(m, d.effort.as_ref()));
            let (on, note) = crate::controls::reconcile_fast(info, d.fast);
            d.fast = on;
            d.fast_note = note;
            if d.ultrafast && (d.instance.as_ref() != Some(&instance) || !info.is_some_and(crate::controls::model_has_ultrafast)) {
                d.ultrafast = false;
                d.fast_note = Some("Ultrafast is off for this model or provider.".to_owned());
            }
            if d.instance.as_ref() != Some(&instance) {
                d.mode = None;
            }
            d.instance = Some(instance);
            d.model = Some(model);
        }
        self.draft_changed(&crate::attachments::DraftRef::New(id));
    }

    /// Set a chat's agents policy (`thread/update`).
    pub fn set_agents_policy(&mut self, key: &ThreadKey, policy: crate::state::AgentsPolicy) {
        let wire = match policy {
            crate::state::AgentsPolicy::Ask => yaac_protocol::AgentsPolicy::Ask,
            crate::state::AgentsPolicy::Allow => yaac_protocol::AgentsPolicy::Allow,
        };
        let w = wire.clone();
        self.update_thread(key, json!({ "agents": wire }), move |s| s.agents = w);
    }

    /// Fork a thread into a destination on the same target, from its last completed turn.
    /// A native fork opens idle, with no first message yet (`thread/forkOnly`). A handoff
    /// creates the destination conversation with its first message (`thread/fork`): `message`
    /// is that message, which `yaacd` sends after the preamble pointing at the transcript.
    pub fn fork(&mut self, key: &ThreadKey, choice: &crate::fork::ForkChoice, message: Option<&str>, keep_children: bool) {
        let op = self.op_id();
        let mut params = json!({ "opId": op, "source": key.thread, "instance": choice.instance });
        if keep_children {
            if !crate::fork::can_keep_children(&self.state, key) {
                self.error(Some(key.clone()), "This target does not support keeping children on a fork.");
                return;
            }
            params["keepChildren"] = json!(true);
        }
        if let Some(mdl) = &choice.model {
            params["model"] = json!(mdl);
        }
        let method = match message.map(str::trim).filter(|t| !t.is_empty()) {
            Some(text) => {
                params["input"] = json!([{ "type": "text", "text": text }]);
                m::THREAD_FORK
            }
            None => m::THREAD_FORK_ONLY,
        };
        self.routed(&key.target, method, params, Call::Fork { target: key.target.clone() });
    }

    /// Ask what a fork into `instance` would send and where (`thread/forkPreview`). Asked once
    /// per destination; the answer is kept in [`AppState::fork_previews`].
    pub fn fork_preview(&mut self, key: &ThreadKey, instance: &InstanceId) {
        let slot = (key.clone(), instance.clone());
        if matches!(self.state.fork_previews.get(&slot), Some(ForkPreviewState::Loading | ForkPreviewState::Ready(_))) {
            return;
        }
        self.state.fork_previews.insert(slot, ForkPreviewState::Loading);
        let params = json!({ "source": key.thread, "instance": instance, "maxPreviewBytes": 4096 });
        self.routed(&key.target, m::THREAD_FORK_PREVIEW, params, Call::ForkPreview { key: key.clone(), instance: instance.clone() });
    }

    /// List a directory on a target (`fs/list`) for the folder browser; `path` empty is the
    /// home directory. The answer is kept in [`AppState::fs_listings`].
    pub fn fs_list(&mut self, target: &TargetId, path: &str) {
        let slot = (target.clone(), path.to_owned());
        if matches!(self.state.fs_listings.get(&slot), Some(FsListingState::Loading | FsListingState::Ready(_))) {
            return;
        }
        self.state.fs_listings.insert(slot, FsListingState::Loading);
        let params = if path.is_empty() { json!({}) } else { json!({ "path": path }) };
        self.routed(target, m::FS_LIST, params, Call::FsList { target: target.clone(), path: path.to_owned() });
    }

    /// Asks for a composer's skills (`skills/list`) unless they are loading or loaded. `force`
    /// asks again (after a failure, or to pick up a new `SKILL.md`).
    pub fn skills_list(&mut self, scope: &crate::skills::SkillScopeKey, force: bool) {
        use crate::skills::SkillsState;
        let known = self.state.skills.get(scope);
        if matches!(known, Some(SkillsState::Loading)) || (!force && matches!(known, Some(SkillsState::Ready(_)))) {
            return;
        }
        self.state.skills.insert(scope.clone(), SkillsState::Loading);
        let params = json!({ "instance": scope.instance, "cwd": scope.cwd, "forceReload": force });
        self.routed(&scope.target, m::SKILLS_LIST, params, Call::Skills(scope.clone()));
    }

    // ── hub, targets, providers (settings) ─────────────────────────────────────────────

    /// Ask an older hub to checkpoint and exit (the window is newer). Allowed right after a
    /// `hub/hello` that found another build.
    pub fn shutdown_hub(&mut self) {
        self.request(attach::names::SHUTDOWN, json!({}), Call::Shutdown);
    }

    pub fn set_explanations(&mut self, models: yaac_protocol::ExplanationModels) {
        self.request(hub_names::SETTINGS_UPDATE, json!({ "explanations": models }), Call::HubSettings);
        self.state.hub.explanations = models;
    }

    /// Receive the hub's request reminder as a list (`state.hub.request_feed`), now and on
    /// every later attach: for a client that draws its own notifications (Android).
    pub fn subscribe_request_feed(&mut self) {
        if self.request_feed {
            return;
        }
        self.request_feed = true;
        if self.state.hub.attached {
            self.request(attach::names::NOTIFICATIONS_SUBSCRIBE, json!({}), Call::RequestFeed);
        }
    }

    /// A feed entry's action, as the toast's: `decision` `allow` or `deny` answers it with
    /// the decision the hub offered (refused by the hub if `revision` is no longer current);
    /// `None` is Open — the request opens here at once and the hub marks it seen.
    pub fn respond_request_feed(&mut self, location: &attach::RequestLocation, revision: u64, decision: Option<&str>) -> Result<(), String> {
        let feed = self.state.hub.request_feed.as_ref().ok_or("No request list from the hub")?;
        let entry = feed.items.iter().find(|i| &i.location == location).ok_or("This request no longer needs you")?;
        if decision.is_some() && entry.decisions.is_none() {
            return Err("This request cannot be answered from a notification".into());
        }
        let open = attach::NotificationOpen { location: location.clone(), item_id: entry.item_id.clone(), error: None };
        let mut params = json!({ "revision": revision, "location": location });
        if let Some(d) = decision {
            params["decision"] = json!(d);
        } else {
            self.notification_open.push_back(open.clone());
        }
        self.request(attach::names::NOTIFICATIONS_RESPOND, params, Call::FeedRespond { open, answered: decision.is_some() });
        Ok(())
    }

    /// Ask the hub for stored window-side choices (`hub/uiState/get`); the answer arrives
    /// through [`Client::take_ui_state`]. For a client on another machine than the hub, which
    /// cannot read the hub's cache file.
    pub fn load_ui_state(&mut self, keys: &[&str]) {
        self.ui_state = None;
        self.request(hub_names::UI_STATE_GET, json!({ "keys": keys }), Call::UiStateGet);
    }

    /// The stored choices, once they have arrived (empty from a hub that has none to give).
    pub fn take_ui_state(&mut self) -> Option<Vec<(String, Value)>> {
        self.ui_state.take()
    }

    pub fn set_notifications(&mut self, settings: attach::NotificationSettings) {
        self.state.hub.notifications = settings;
        self.request(hub_names::SETTINGS_UPDATE, json!({ "notifications": settings }), Call::HubSettings);
    }

    /// Whether the hub keeps running and starts at login.
    pub fn set_always(&mut self, always: bool) {
        self.state.hub.always = Some(always);
        self.request(hub_names::SETTINGS_UPDATE, json!({ "always": always }), Call::HubSettings);
    }

    /// "Allow the Android client over Tailscale": the hub also accepts attaches on this host's
    /// Tailscale addresses, from the same Tailscale login only.
    pub fn set_tailscale_attach(&mut self, on: bool) {
        self.state.hub.tailscale_attach = Some(on);
        self.request(hub_names::SETTINGS_UPDATE, json!({ "tailscaleAttach": on }), Call::HubSettings);
    }

    /// "Clear cache": the hub drops its cache (drafts are never touched), and the window
    /// reloads every target.
    pub fn clear_cache(&mut self) {
        self.request(hub_names::CACHE_CLEAR, json!({}), Call::Ignore);
        let ids: Vec<TargetId> = self.state.targets.iter().map(|t| t.entry.id.clone()).collect();
        for t in &ids {
            if let Some(rec) = self.state.targets.iter_mut().find(|x| &x.entry.id == t) {
                rec.sync_token = yaac_protocol::SyncToken::default();
            }
            self.load_target(t);
        }
    }

    /// Store a window-side choice in the hub's `ui_state` (only when attached to a real hub).
    pub fn set_ui_state(&mut self, key: &str, value: &Value) {
        if self.state.hub.attached {
            self.request(hub_names::UI_STATE_SET, json!({ "key": key, "value": value }), Call::Ignore);
        }
    }

    /// Enable or disable a target (Disable keeps it but stops using it).
    pub fn set_target_enabled(&mut self, target: &TargetId, enabled: bool) {
        if let Some(t) = self.state.targets.iter_mut().find(|t| &t.entry.id == target) {
            t.entry.enabled = enabled;
        }
        let params = json!({ "id": target, "enabled": enabled });
        self.request(attach::names::TARGETS_ENABLE, params, Call::TargetChanged);
    }

    /// Remove (forget) a target. Its cache rows go with it.
    pub fn remove_target(&mut self, target: &TargetId) {
        self.state.targets.retain(|t| &t.entry.id != target);
        self.state.ssh_records.remove(target);
        let keys: Vec<ThreadKey> = self.state.threads.keys().filter(|k| &k.target == target).cloned().collect();
        for k in keys {
            self.state.remove_thread(&k);
        }
        self.request(attach::names::TARGETS_REMOVE, json!({ "id": target }), Call::TargetChanged);
    }

    /// Add a target (an SSH connection once set up, or a WSL distro and user).
    pub fn add_target(&mut self, name: &str, transport: &attach::TargetTransport) {
        let params = json!({ "name": name, "transport": transport, "enabled": true });
        self.request(attach::names::TARGETS_ADD, params, Call::TargetChanged);
    }

    /// Re-probe a target's instances (one, or all) and their model lists.
    pub fn refresh_providers(&mut self, target: &TargetId, instance: Option<&InstanceId>) {
        let params = match instance {
            Some(i) => json!({ "instance": i }),
            None => json!({}),
        };
        self.routed(target, m::PROVIDER_REFRESH, params, Call::InstanceChanged(target.clone()));
    }

    /// Read a target's stored instance configurations.
    pub fn list_instances(&mut self, target: &TargetId) {
        self.routed(target, m::PROVIDER_INSTANCE_LIST, json!({}), Call::InstanceList(target.clone()));
    }

    /// Add an instance: a kind the target reports, an optional name, an optional socket.
    pub fn add_instance(&mut self, target: &TargetId, kind: &yaac_protocol::ProviderKind, name: Option<&str>, socket: Option<&str>, home: Option<&str>) {
        let op = self.op_id();
        let mut params = json!({ "opId": op, "kind": kind, "enabled": true, "backend": "auto" });
        if let Some(h) = home.filter(|h| !h.trim().is_empty()) {
            params["home"] = json!(h.trim());
        }
        if let Some(n) = name.filter(|n| !n.trim().is_empty()) {
            params["name"] = json!(n.trim());
        }
        if let Some(s) = socket.filter(|s| !s.trim().is_empty()) {
            params["endpoint"] = json!({ "socket": s.trim() });
        }
        self.routed(target, m::PROVIDER_INSTANCE_ADD, params, Call::InstanceChanged(target.clone()));
    }

    /// Change an instance's name, socket, or enabled switch.
    pub fn update_instance(&mut self, target: &TargetId, id: &InstanceId, name: Option<Option<&str>>, socket: Option<Option<&str>>, enabled: Option<bool>) {
        let op = self.op_id();
        let mut params = json!({ "opId": op, "id": id });
        if let Some(n) = name {
            params["name"] = json!(n);
        }
        if let Some(s) = socket {
            params["endpoint"] = s.map_or(Value::Null, |s| json!({ "socket": s }));
        }
        if let Some(e) = enabled {
            params["enabled"] = json!(e);
        }
        self.routed(target, m::PROVIDER_INSTANCE_UPDATE, params, Call::InstanceChanged(target.clone()));
    }

    /// The per-instance switch "Trust folders YAAC works in" (D147), stored in the instance's
    /// configuration on the target.
    pub fn set_trust_folders(&mut self, target: &TargetId, id: &InstanceId, on: bool) {
        let op = self.op_id();
        if let Some(c) = self.state.targets.iter_mut().find(|t| &t.entry.id == target).and_then(|t| t.instance_configs.as_mut()).and_then(|cs| cs.iter_mut().find(|c| &c.id == id)) {
            // Shown at once; the reply replaces it with what the target stored.
            c.trust_folders = if on { None } else { Some(false) };
        }
        self.routed(target, m::PROVIDER_INSTANCE_UPDATE, json!({ "opId": op, "id": id, "trustFolders": on }), Call::InstanceChanged(target.clone()));
    }

    /// Set a model recommendation on this target, or restore the generated value.
    pub fn set_model_best_use(&mut self, target: &TargetId, instance: InstanceId, model: yaac_protocol::ModelId,
        expected_model: yaac_protocol::ModelId, summary: Option<String>) {
        let op_id = self.op_id();
        let params = yaac_protocol::ModelBestUseSetParams { op_id, instance, model, expected_model, summary };
        self.routed(target, m::PROVIDER_MODEL_BEST_USE_SET, json!(params), Call::Ignore);
    }

    /// Turn one model on or off for an instance on this target.
    pub fn set_model_enabled(&mut self, target: &TargetId, instance: InstanceId, model: yaac_protocol::ModelId, enabled: bool) {
        let op_id = self.op_id();
        let params = yaac_protocol::ProviderInstanceSetModelEnabledParams { op_id, instance, model, enabled };
        self.routed(target, m::PROVIDER_INSTANCE_SET_MODEL_ENABLED, json!(params), Call::Ignore);
    }

    pub fn set_approval_explanation_model(&mut self, target: &TargetId, instance: &InstanceId, model: Option<yaac_protocol::ModelId>) {
        let op = self.op_id();
        self.routed(target, m::PROVIDER_INSTANCE_UPDATE,
            json!({ "opId": op, "id": instance, "approvalExplanationModel": model }), Call::InstanceChanged(target.clone()));
    }

    /// Set an instance's harness home (P300); `None` or empty: back to the harness default.
    /// Editing an implicit default instance stores its configuration on the target. A home
    /// that leads to another store is refused until the user confirms it
    /// ([`AppState::home_confirm`]); `confirmed` is that confirmation, sent with a new
    /// operation id.
    pub fn set_instance_home(&mut self, target: &TargetId, id: &InstanceId, home: Option<&str>, confirmed: bool) {
        let op = self.op_id();
        let home = home.map(str::trim).filter(|h| !h.is_empty()).map(str::to_owned);
        let mut params = json!({ "opId": op, "id": id, "home": home });
        if confirmed {
            params["confirmStoreChange"] = json!(true);
        }
        self.state.home_confirm = None;
        self.routed(target, m::PROVIDER_INSTANCE_UPDATE, params, Call::InstanceHome { target: target.clone(), instance: id.clone(), home });
    }

    /// The user confirmed the waiting harness-home change (P300).
    pub fn confirm_instance_home(&mut self) {
        if let Some(c) = self.state.home_confirm.take() {
            self.set_instance_home(&c.target, &c.instance, c.home.as_deref(), true);
        }
    }

    /// The user kept the current harness home.
    pub fn cancel_instance_home(&mut self) {
        self.state.home_confirm = None;
    }

    /// Remove an instance.
    pub fn remove_instance(&mut self, target: &TargetId, id: &InstanceId) {
        let op = self.op_id();
        self.routed(target, m::PROVIDER_INSTANCE_REMOVE, json!({ "opId": op, "id": id }), Call::InstanceChanged(target.clone()));
    }

    /// The per-instance switch "Offer YAAC tools to sessions started outside YAAC". **SEAM:**
    /// no wire method edits a harness's own configuration yet; window-local.
    pub fn set_outside_tools(&mut self, target: &TargetId, instance: &InstanceId, on: bool) {
        let k = (target.clone(), instance.clone());
        if on {
            self.state.outside_tools.insert(k);
        } else {
            self.state.outside_tools.remove(&k);
        }
    }

    // ── modes that bypass approvals (screen 30) ────────────────────────────────────────

    /// The user chose a mode. One the provider marks `skipsApprovals` is not applied: it
    /// waits in [`AppState::mode_confirm`] for the user's confirmation.
    pub fn choose_mode(&mut self, target: ModeTarget, mode: yaac_protocol::ModeId) {
        let modes = crate::mode_confirm::modes_for(&self.state, &target);
        if let Some(tree) = crate::mode_confirm::tree(&self.state, &target, &mode, &modes) {
            self.state.mode_confirm = Some(tree);
            return;
        }
        match crate::mode_confirm::pick(&modes, target, mode) {
            ModePick::Apply(t, m) => {
                self.state.mode_confirm = None;
                self.apply_mode(t, m);
            }
            ModePick::Confirm(p) => self.state.mode_confirm = Some(p),
        }
    }

    /// "Bypass permissions": apply the pending mode. The confirmation is not remembered.
    pub fn confirm_mode(&mut self) {
        self.confirm_mode_scope(false);
    }

    /// Apply the snapshot the user confirmed, with one operation id per recipient.
    pub fn confirm_mode_scope(&mut self, all: bool) {
        let Some(p) = self.state.mode_confirm.take() else { return };
        let ModeTarget::Thread(root) = &p.target else { self.apply_mode(p.target, p.mode); return };
        if p.workers.is_empty() { self.apply_mode(p.target, p.mode); return }
        let Some(source) = self.state.summary(root).cloned() else { return };
        let Some(mode) = p.chosen else { return };
        let batch = self.op_id();
        self.mode_results.insert(batch.clone(), ModeChangeResults {
            remaining: if all { 1 + p.workers.len() } else { 1 },
            ..Default::default()
        });
        for (key, interrupt) in std::iter::once((root, p.interrupt_root))
            .chain(p.workers.iter().filter(|_| all).map(|r| (&r.key, r.interrupt))) {
            let params = yaac_protocol::ThreadSetModeParams { op_id: self.op_id(), thread_id: key.thread.clone(),
                source_target: root.target.clone(), source_instance: source.instance.clone(),
                source_title: source.title.clone(), mode: mode.clone(), allow_interrupt: interrupt };
            self.routed(&key.target, m::THREAD_SET_MODE, json!(params), Call::ModeChanged { key: key.clone(), root: root.clone(), batch: batch.clone() });
        }
    }

    fn mode_reply(&mut self, batch: OpId, root: ThreadKey, key: &ThreadKey, offline: bool, failed: bool) {
        let Some(result) = self.mode_results.get_mut(&batch) else { return };
        if offline {
            let title = self.state.summary(key).map_or(key.thread.as_str(), |s| &s.title);
            result.offline.push(format!("{title} on {}", self.state.target_name(&key.target)));
        } else if failed {
            result.failed += 1;
        } else {
            result.applied += 1;
        }
        result.remaining -= 1;
        if result.remaining == 0 {
            let mut result = self.mode_results.remove(&batch).unwrap();
            if !result.offline.is_empty() {
                result.offline.sort();
                self.error(Some(root), format!("Mode change: {} applied, {} pending offline ({}), {} failed.",
                    result.applied, result.offline.len(), result.offline.join(", "), result.failed));
            }
        }
    }

    /// Cancel (the default): nothing changes.
    pub fn cancel_mode(&mut self) {
        self.state.mode_confirm = None;
    }

    fn apply_mode(&mut self, target: ModeTarget, mode: yaac_protocol::ModeId) {
        match target {
            ModeTarget::Thread(k) => self.state.next_settings.entry(k).or_default().mode = Some(mode),
            ModeTarget::Draft(id) => {
                if let Some(d) = self.state.new_threads.iter_mut().find(|d| d.id == id) {
                    d.mode = Some(mode);
                }
            }
        }
    }
}

/// One page of `hub/threads/find` (the parts the window uses).
#[derive(serde::Deserialize)]
struct FindPage {
    #[serde(default)]
    results: Vec<FindHit>,
    #[serde(default)]
    total: u32,
    #[serde(default)]
    cursor: Option<String>,
}

/// The hub's build in its `window_outdated` refusal: "This window (<ours>) is older than the
/// running hub (<theirs>). Update YAAC." The text is the only place a refusal carries it.
fn refused_by_build(message: &str) -> Option<String> {
    let rest = &message[message.find("running hub (")? + "running hub (".len()..];
    let build = &rest[..rest.find(')')?];
    (!build.is_empty() && !build.contains(char::is_whitespace)).then(|| build.to_owned())
}

#[derive(serde::Deserialize)]
struct FindHit {
    target: TargetId,
    thread: ThreadSummary,
}

#[cfg(test)]
mod mode_results_tests {
    use super::*;

    #[test]
    fn offline_replies_are_summarized_once_at_the_root() {
        let mut client = Client::new(Box::new(crate::link::DetachedLink), "test", ClientId::new("window"));
        let root = ThreadKey::new("local", "root");
        let remote = ThreadKey::new("remote", "leaf");
        let batch = client.op_id();
        client.mode_results.insert(batch.clone(), ModeChangeResults { remaining: 2, ..Default::default() });
        let call = |key| Call::ModeChanged { key, root: root.clone(), batch: batch.clone() };
        client.on_result(call(remote), json!({ "pending": true }));
        assert!(client.errors.is_empty());
        client.on_result(call(root.clone()), json!({}));
        assert!(client.mode_results.is_empty());
        assert_eq!(client.errors[0].key.as_ref(), Some(&root));
        assert!(client.errors[0].text.contains("1 applied, 1 pending offline (leaf on remote), 0 failed"));
    }
}

#[cfg(test)]
mod history_tests {
    use super::*;
    use crate::transcript::Transcript;
    use yaac_protocol::{Epoch, JournalPosition, Seq};

    #[test]
    fn a_stale_page_cursor_refreshes_the_history_without_showing_an_error() {
        for cursor in ["history:missing", "history-newer:missing"] {
            let mut client = Client::new(Box::new(crate::link::DetachedLink), "test", ClientId::new("window"));
            client.pending.clear();
            let key = ThreadKey::new(TargetId::new("local"), ThreadId::new("chat"));
            let message = |id: &str| json!({
                "id": id, "turnId": "turn", "status": "completed", "kind": "agentMessage", "text": id
            });
            let mut transcript = Transcript::new();
            transcript.replace_all(vec![serde_json::from_value(message("missing")).unwrap()], 1);
            let newer = cursor.starts_with("history-newer:");
            if newer {
                transcript.newer_cursor = Some(cursor.into());
            } else {
                transcript.older_cursor = Some(cursor.into());
            }
            client.state.transcripts.insert(key.clone(), transcript);
            if newer { client.newer_history(&key); } else { client.older_history(&key); }
            let read = client.pending.drain().next().unwrap().1;
            client.on_error(read, &ErrorObject::new(ErrorCode::InvalidParams, "this message is no longer in the thread"));
            assert!(client.errors.is_empty());
            assert!(client.state.transcripts[&key].item(&ItemId::new("missing")).is_some());
            let refresh = client.pending.drain().next().unwrap().1;
            assert!(matches!(&refresh, Call::Read(k) if k == &key));
            client.on_result(refresh, json!({
                "items": [message("latest")], "position": { "epoch": 3, "seq": 1 }, "prompts": [],
            }));
            assert!(client.state.transcripts[&key].item(&ItemId::new("latest")).is_some());
            assert!(client.errors.is_empty());
            assert!(client.pending.is_empty());
        }
    }

    #[test]
    fn an_older_turn_stays_out_of_the_visible_window_until_its_last_chunk_arrives() {
        let mut client = Client::new(Box::new(crate::link::DetachedLink), "test", ClientId::new("window"));
        client.pending.clear();
        let key = ThreadKey::new(TargetId::new("local"), ThreadId::new("chat"));
        let position = JournalPosition { epoch: Epoch(3), seq: Seq(1) };
        let message = |id: &str, turn: &str| json!({
            "id": id, "turnId": turn, "status": "completed", "kind": "agentMessage", "text": id
        });
        let mut transcript = Transcript::new();
        transcript.position = Some(position);
        transcript.older_cursor = Some("history:visible".into());
        transcript.replace_all(vec![serde_json::from_value(message("visible", "live")).unwrap()], 1);
        client.state.transcripts.insert(key.clone(), transcript);

        client.older_history(&key);
        let read = client.pending.drain().next().unwrap().1;
        client.on_result(read, json!({
            "items": [message("old-end", "older")], "position": position,
            "cursor": "history:old-end", "continuesTurn": true,
        }));
        let t = &client.state.transcripts[&key];
        assert!(t.loading_older);
        assert_eq!(t.older_cursor.as_deref(), Some("history:visible"));
        assert_eq!(t.items().len(), 1);

        let tail = client.pending.drain().next().unwrap().1;
        client.on_result(tail, json!({
            "items": [message("old-start", "older")], "position": position, "cursor": "history:old-start",
        }));
        let t = &client.state.transcripts[&key];
        assert!(!t.loading_older);
        assert_eq!(t.older_cursor.as_deref(), Some("history:old-start"));
        assert!(t.items().iter().map(|item| item.id.as_str()).eq(["old-start", "old-end", "visible"]));
    }

    #[test]
    fn a_later_chunk_does_not_replay_deltas_already_in_its_items() {
        use yaac_protocol::{DeltaField, ItemDeltaEvent};

        let mut client = Client::new(Box::new(crate::link::DetachedLink), "test", ClientId::new("window"));
        client.pending.clear();
        let key = ThreadKey::new(TargetId::new("local"), ThreadId::new("chat"));
        let mut t = Transcript::new();
        t.begin_reset();
        client.state.transcripts.insert(key.clone(), t);
        client.read_page(&key, None);
        let first = client.pending.drain().next().unwrap().1;
        client.on_result(first, json!({
            "items": [{ "id": "command", "turnId": "turn", "status": "completed", "kind": "agentMessage", "text": "End" }],
            "position": { "epoch": 3, "seq": 1 }, "cursor": "history:command", "continuesTurn": true,
        }));
        let delta = |seq, text: &str| ThreadEventParams {
            thread_id: key.thread.clone(),
            epoch: Epoch(3),
            seq: Seq(seq),
            event: ThreadEvent::ItemDelta(ItemDeltaEvent {
                item_id: ItemId::new("reply"), field: DeltaField::Text, delta: text.into(),
            }),
        };
        client.state.transcripts.get_mut(&key).unwrap().apply(&delta(2, "A"), 2);
        let tail = client.pending.drain().next().unwrap().1;
        client.on_result(tail, json!({
            "items": [{ "id": "reply", "turnId": "turn", "status": "inProgress", "kind": "agentMessage", "text": "AB" }],
            "position": { "epoch": 3, "seq": 3 },
        }));

        let t = client.state.transcripts.get_mut(&key).unwrap();
        assert!(!t.apply(&delta(3, "B"), 3), "the second page already contained this delta");
        assert!(t.apply(&delta(4, "C"), 4));
        assert!(matches!(&t.items()[0].body, ItemBody::AgentMessage(message) if message.text == "ABC"));
    }

    fn subscribes(client: &mut Client, key: &ThreadKey) -> usize {
        client.pending.drain().filter(|(_, call)| matches!(call, Call::Subscribe(k) if k == key)).count()
    }

    #[test]
    fn opening_a_thread_whose_load_failed_subscribes_again() {
        let mut client = Client::new(Box::new(crate::link::DetachedLink), "test", ClientId::new("window"));
        client.pending.clear();
        let key = ThreadKey::new(TargetId::new("wsl"), ThreadId::new("worker"));
        client.open(&key);
        let subscribe = client.pending.drain().find(|(_, call)| matches!(call, Call::Subscribe(_))).unwrap().1;
        // The target was briefly offline and the hub had not cached the thread yet.
        client.on_error(subscribe, &ErrorObject::new(ErrorCode::TargetOffline, "the target is not connected and the thread is not cached"));
        assert!(!client.state.transcripts[&key].is_loaded(), "nothing loaded, nothing in flight");

        client.open(&key);
        let again: Vec<_> = client.pending.drain().filter(|(_, call)| matches!(call, Call::Subscribe(k) if k == &key)).collect();
        assert_eq!(again.len(), 1, "opening it again loads it instead of showing Loading… for good");
        client.on_error(again.into_iter().next().unwrap().1, &ErrorObject::new(ErrorCode::TargetOffline, "the connection to the target dropped"));

        // A subscription already on its way is not sent twice.
        client.open(&key);
        client.open(&key);
        assert_eq!(subscribes(&mut client, &key), 1);

        // A loaded transcript is left alone.
        let mut client2 = Client::new(Box::new(crate::link::DetachedLink), "test", ClientId::new("window"));
        client2.pending.clear();
        client2.open(&key);
        let subscribe = client2.pending.drain().find(|(_, call)| matches!(call, Call::Subscribe(_))).unwrap().1;
        client2.on_result(subscribe, json!({ "mode": "reset", "epoch": 1, "headSeq": 1, "control": "free" }));
        let read = client2.pending.drain().find(|(_, call)| matches!(call, Call::Read(_))).unwrap().1;
        client2.on_result(read, json!({ "items": [], "position": { "epoch": 1, "seq": 1 }, "prompts": [] }));
        assert!(client2.state.transcripts[&key].is_loaded());
        client2.open(&key);
        assert_eq!(subscribes(&mut client2, &key), 0);
    }

    #[test]
    fn a_reconnected_target_reloads_transcripts_that_never_loaded() {
        let mut client = Client::new(Box::new(crate::link::DetachedLink), "test", ClientId::new("window"));
        client.pending.clear();
        client.state.set_targets(serde_json::from_value(json!([
            {"id": "wsl", "name": "wsl: Ubuntu", "enabled": true, "transport": {"kind":"local"}, "state": {"status":"connecting"}}
        ])).unwrap());
        let key = ThreadKey::new(TargetId::new("wsl"), ThreadId::new("worker"));
        client.open(&key);
        let subscribe = client.pending.drain().find(|(_, call)| matches!(call, Call::Subscribe(_))).unwrap().1;
        client.on_error(subscribe, &ErrorObject::new(ErrorCode::TargetOffline, "the target is not connected and the thread is not cached"));

        client.on_notification(attach::names::TARGET_STATE, json!({ "id": "wsl", "state": { "status": "connected" }, "enabled": true }));
        assert_eq!(subscribes(&mut client, &key), 1);
    }

    #[test]
    fn a_thread_that_is_one_long_turn_loads_across_wire_pages() {
        // The shape of a finished worker: its report (outside any turn) newest, then a single
        // turn too large for one wire page, and nothing older.
        let mut client = Client::new(Box::new(crate::link::DetachedLink), "test", ClientId::new("window"));
        client.pending.clear();
        let key = ThreadKey::new(TargetId::new("wsl"), ThreadId::new("worker"));
        let mut t = Transcript::new();
        t.begin_reset();
        client.state.transcripts.insert(key.clone(), t);
        client.read_page(&key, None);
        let item = |id: &str, turn: &str| json!({ "id": id, "turnId": turn, "status": "completed", "kind": "agentMessage", "text": id });
        let position = json!({ "epoch": 1, "seq": 157 });
        let pages = [
            json!({ "items": [item("report", ""), item("c", "turn")], "position": position, "cursor": "history:c", "continuesTurn": true, "prompts": [] }),
            json!({ "items": [item("b", "turn")], "position": position, "cursor": "history:b", "newerCursor": "history-newer:b", "continuesTurn": true }),
            json!({ "items": [item("a", "turn")], "position": position, "newerCursor": "history-newer:a" }),
        ];
        for page in pages {
            assert!(!client.state.transcripts[&key].is_loaded());
            let call = client.pending.drain().next().unwrap().1;
            client.on_result(call, page);
        }
        let t = &client.state.transcripts[&key];
        assert!(t.is_loaded());
        assert!(t.items().iter().map(|item| item.id.as_str()).eq(["a", "b", "c", "report"]));
        assert!(client.pending.is_empty() && client.errors.is_empty());
    }
}
