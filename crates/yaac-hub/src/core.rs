//! The hub core: one task on the I/O thread that owns every target session, every attached
//! window, and the routing between them. Everything that can block runs on the store worker;
//! the core never waits for it — finished jobs come back as [`CoreEvent`]s.

mod attach_tunnel;
mod browser;
mod host_update;
mod notifications;
mod limits;
mod preview;
mod terminal;
mod ssh;
mod upload_preview;
mod uploads;
mod voice;
mod voice_call;

pub use uploads::UploadLimits;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use yaac_protocol::attach::{
    self, ConnectionState, ConnectionStatus, DaemonSummary, DaemonUpdate, DaemonUpdatePhase, HelloParams, HelloResult, RoutedNotification, RoutedRequest,
    TargetEntry, TargetResult, TargetStateParams, TargetsAddParams, TargetsEnableParams, TargetsListResult,
    TargetsRemoveParams, TargetsUpdateDaemonParams, TargetsUpdateDaemonResult, TargetsUpdateParams,
};
use yaac_protocol::dialect::{Inbound, PeerRequest};
use yaac_protocol::flows::{FlowError, FlowStep, OpenThread, OpenThreadFlow, Opened, finish_page, read_older_page};
use yaac_protocol::methods::{
    self, DaemonHandover, DiscoveryCapsMethod, MeshDeliver, MeshQuery, MeshReceipt, MeshRoster, MeshUsage, Ping, ProjectList, ProviderList, ThreadAck, ThreadKeep, ThreadLookup, ThreadSync, names,
};
use yaac_protocol::rpc::{ErrorCode, ErrorObject, Message, RpcId};
use yaac_protocol::session::{ClientSession, ReadyInfo, SessionEvent};
use yaac_protocol::{
    ClientInfo, Control, DaemonHandoverParams, Epoch, HandoverState, HandoverWhen, Item, ItemBody, JournalPosition, MeshDeliverParams, MeshUsageParams, MeshMessage, MeshQueryParams, MeshReceiptParams,
    MeshRosterParams, MessageId, OpId, PageCursor, Receipt, ReceiptReason, ReceiptStatus, RequestState, RosterEntry, Seq,
    StreamCreditParams, StreamId, SubscribeMode, SubscribeReplay, SubscribeReset, SyncToken, TargetId, TargetName, ThreadAckParams,
    ThreadEvent, ThreadEventParams, ThreadId, ThreadReadParams, ThreadReadResult, ThreadState, ThreadSubscribeParams,
    ThreadSubscribeResult, ThreadSyncParams, ThreadSyncResult, ThreadUnsubscribeParams, ThreadUpdatedParams,
};

use crate::cache::LiveState;
use crate::config::HubConfig;
use crate::conn::{ConnEvent, Outgoing, supervise};
use crate::protocol::{
    self as ext, BusyReasons, ConnectionStep, Draft, DraftKey, DraftsListResult, LookupHit, LookupParams, LookupResult, OperationRef,
    OperationsListResult, SettingsResult, SettingsUpdateParams, StatusResult, TargetLogResult, TargetRef, ThreadResetParams, UiStateSetParams,
};
use crate::find::{FindParams, Pager};
use crate::relay::{Accepted, receipt};
use crate::settings::{Settings, TargetDef, new_id};
use crate::pool::Pool;
use crate::worker::Worker;

/// One attached window.
pub(crate) type ClientKey = u64;

type Key = (TargetId, ThreadId);

/// Internal holder of a blocked thread subscription. Attach clients are numbered from zero.
const NOTIFICATION_CLIENT: ClientKey = u64::MAX;

/// Everything the core reacts to. Moved once through a channel; boxing the large variants
/// would only make every match clumsier.
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub(crate) enum CoreEvent {
    NotificationAction(u64, String),
    NotificationDestination(attach::NotificationOpen, Vec<Key>),
    PendingModes(TargetId, u64, Vec<yaac_protocol::ThreadSetModeParams>),
    /// A window connected; `remote`: over Tailscale, admitted as this host's own login.
    Attached { client: ClientKey, tx: crate::attach_writer::Sender, remote: bool },
    /// Raw interactive input from a window.
    ClientData { client: ClientKey, stream: StreamId, bytes: Vec<u8> },
    /// A message from a window.
    FromClient { client: ClientKey, message: Message },
    /// A window went away.
    Detached { client: ClientKey },
    /// Something happened on a target connection (supervisor id, generation).
    Conn { target: TargetId, supervisor: u64, generation: u64, event: ConnEvent },
    /// A batch of a thread's events is committed.
    Committed { target: TargetId, thread: ThreadId, events: Vec<ThreadEventParams>, position: JournalPosition },
    /// A thread's transcript was rebuilt from a snapshot.
    Rebuilt { target: TargetId, thread: ThreadId, position: JournalPosition },
    /// Answer a window's request.
    Reply { client: ClientKey, id: RpcId, outcome: Result<Value, ErrorObject> },
    /// A catalog reply, with an optional notice for the other windows.
    CatalogWindow { client: ClientKey, id: RpcId, outcome: Result<Value, ErrorObject>, changed: bool },
    /// A catalog response to the originating target.
    CatalogTarget { source: TargetId, generation: u64, id: RpcId, outcome: Result<Value, ErrorObject> },
    /// Send to every attached window.
    Broadcast(Message),
    /// A `mesh/relay` is stored (or recognised as a duplicate).
    RelayAccepted { source: TargetId, generation: u64, peer_id: RpcId, message: MeshMessage, accepted: Result<Accepted, String>, in_flight: u64 },
    /// Messages waiting for a destination that is now connected.
    Deliver { dest: TargetId, messages: Vec<MeshMessage>, in_flight: u64 },
    /// The recipient reported the final harness outcome for a relayed message.
    DeliveryReceipt {
        dest: TargetId,
        generation: u64,
        peer_id: RpcId,
        outcome: Result<Option<TargetId>, String>,
        params: MeshReceiptParams,
        in_flight: u64,
    },
    /// Receipts owed to sources (one that just connected, or that just became due).
    Receipts { receipts: Vec<(TargetId, MessageId, Receipt)>, in_flight: u64 },
    /// Undelivered-message notices to hand to their senders' targets now.
    Notices { deliveries: Vec<(TargetId, MeshMessage)>, in_flight: u64 },
    /// The cached index of every target, for `hub/threads/find`.
    FindReady { client: ClientKey, id: RpcId, params: FindParams, threads: Vec<(TargetId, yaac_protocol::ThreadSummary)> },
    /// The cached index of every target, for an agent's `findThreads` / `listTargets`.
    MeshIndexReady { source: TargetId, generation: u64, peer_id: RpcId, query: yaac_protocol::MeshQuery, threads: Vec<(TargetId, yaac_protocol::ThreadSummary)>, labels: Vec<(TargetId, crate::mesh::TargetLabels)> },
    /// The relay changed; how much is still in flight.
    InFlight(u64),
    /// View preferences changed in the durable store (D92): tell the windows, answer the
    /// window that asked (`one`: `hub/view/set`, else `hub/view/setMany`), and tell the
    /// targets which threads are kept.
    ViewsChanged { reply: Option<(ClientKey, RpcId, bool)>, views: Vec<crate::protocol::ThreadView> },
    /// Checkpoint done; answer `hub/shutdown` and stop.
    ShutdownReady { client: ClientKey, id: RpcId },
    /// A store job failed (logged; the cache is disposable).
    StoreError(String),
    /// SIGINT / SIGTERM.
    Signal,
    /// An upload's draft blob was looked up: its file, name, and media type.
    UploadSource { handle: String, result: Result<(std::path::PathBuf, String, Option<String>), ErrorObject> },
    /// A pool worker hashed an upload's file.
    UploadHashed { handle: String, result: Result<uploads::Hashed, ErrorObject> },
    /// An SSH setup task reported something (`hub/ssh/*`).
    Ssh(ssh::SshEvent),
    /// A pool worker read the next chunk of an upload's file.
    UploadChunk { handle: String, offset: u64, result: Result<Vec<u8>, ErrorObject> },
    /// A pool worker resized a target image for inline display.
    UploadPreviewReady { client: ClientKey, id: RpcId, outcome: Result<Value, ErrorObject> },
    /// A loopback listener or browser socket reported progress.
    Preview(preview::Event),
    /// Attaching over Tailscale changed state.
    Tailscale(crate::protocol::TailscaleAttach),
    /// The update of this computer moved on (`hub/update/request`).
    HostUpdate(host_update::Event),
}

/// Why the hub stopped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExitReason {
    /// Idle with no window for the idle period.
    Idle,
    /// A window asked (`hub/shutdown`).
    Shutdown,
    /// A signal.
    Signal,
}

pub(crate) fn log(message: impl AsRef<str>) {
    if std::env::var_os("YAAC_HUB_LOG").is_some() {
        eprintln!("yaac-hub: {}", message.as_ref());
    }
}

// ── Per-window state ──────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Hello {
    /// No `hub/hello` yet.
    None,
    /// Same build: fully attached.
    Attached,
    /// The window is newer: only `hub/shutdown` is allowed.
    HubOutdated,
    /// The window is older: nothing is allowed.
    WindowOutdated,
}

#[derive(Debug)]
struct Client {
    tx: crate::attach_writer::Sender,
    hello: Hello,
    /// Attached over Tailscale (admitted as this host's login), not the local socket.
    remote: bool,
    subs: HashSet<Key>,
    /// What the window shows (`hub/view/showing`, D114).
    showing: HashSet<Key>,
    visible_requests: Option<HashSet<attach::RequestLocation>>,
    /// Receives the request reminder's list (`hub/notifications/subscribe`).
    feed: bool,
}

// ── Per-target state ──────────────────────────────────────────────────────────────────────

#[derive(Debug)]
enum Pending {
    Mode { op: OpId, thread: ThreadId },
    Routed { client: ClientKey, id: RpcId, method: &'static str, op: Option<OpId> },
    Open { thread: ThreadId, serial: u64, flow: Box<OpenThreadFlow> },
    Page { thread: ThreadId, serial: u64 },
    RequiredRequests { thread: ThreadId, serial: u64 },
    ProviderList,
    Sync,
    MeshQuery { source: TargetId, generation: u64, peer_id: RpcId },
    /// A peer request forwarded to another target; its answer goes back to the source.
    PeerForward { source: TargetId, generation: u64, peer_id: RpcId, method: &'static str },
    ProjectList,
    Discovery,
    /// One target's answer to a `thread/lookup` the hub fans out.
    Lookup { lookup: u64 },
    Deliver { message_id: MessageId },
    Receipt { message_id: MessageId, receipt: Receipt },
    UploadBegin { handle: String },
    UploadEnd { handle: String },
    UploadCancel { handle: String },
    UploadPreview(upload_preview::PreviewRead),
    Terminal { client: ClientKey, id: RpcId, terminal: Option<String> },
    /// A `voice/*` request from a window (or the hub's own cancel).
    Voice { client: ClientKey, id: RpcId, ask: voice::VoiceAsk },
    /// A `voiceCall/*` request from a window (or the hub's own hang-up).
    VoiceCall { client: ClientKey, id: RpcId, ask: voice_call::CallAsk },
    PreviewOpen { socket: u64 },
    /// `tunnel/open` for a window's tunnel over its attach connection (P380).
    AttachTunnel { client: ClientKey, id: RpcId },
    /// `daemon/handover`; **Update now** answers the window that asked.
    Handover { reply: Option<(ClientKey, RpcId)> },
    /// `provider/rateLimits` (D161).
    RateLimits,
    Ignore,
}

#[derive(Debug, PartialEq, Eq)]
enum Phase {
    /// `thread/subscribe` sent. Events now are from an earlier subscription: dropped.
    Subscribing,
    /// Reset: reading the snapshot. Events now are buffered and applied after it.
    Reading,
    /// Events are applied as they arrive.
    Live,
}

#[derive(Debug)]
struct Sub {
    /// Tells this subscription's responses from those of one it replaced.
    serial: u64,
    phase: Phase,
    command: OpenThread,
    epoch: Option<Epoch>,
    snapshot: Vec<Item>,
    required_requests: Vec<Item>,
    snapshot_bytes: u64,
    snapshot_position: Option<JournalPosition>,
    buffered: Vec<ThreadEventParams>,
    subscribed: Option<ThreadSubscribeResult>,
    /// Windows' `thread/subscribe` requests waiting for the thread to be in the cache.
    waiters: Vec<(ClientKey, RpcId, ThreadSubscribeParams)>,
    /// The cache holds the thread at a position consistent with this subscription.
    ready: bool,
    acked: Option<JournalPosition>,
}

#[derive(Debug)]
struct Conn {
    generation: u64,
    session: ClientSession,
    writer: mpsc::UnboundedSender<Outgoing>,
    /// The writer holds one frame. The scheduler selects again on completion.
    writer_busy: bool,
    kill: Option<oneshot::Sender<()>>,
    ready: bool,
    pending: HashMap<RpcId, Pending>,
    /// Newer choices received while the store reads the previous connection's offline choices.
    modes_during_reconnect: Option<HashSet<ThreadId>>,
    terminal_streams: HashMap<StreamId, StreamId>,
    /// Target stream → hub-local stream, for windows' dictations.
    voice_streams: HashMap<StreamId, StreamId>,
    /// This connection's instances as the transcriber choice needs them (capabilities folded,
    /// models dropped): docs/plan/voice.md, P392.
    transcribers: Vec<yaac_protocol::ProviderDescriptor>,
    preview_streams: HashMap<StreamId, u64>,
    /// Target stream → attach stream, for windows' tunnels (P380).
    attach_tunnels: HashMap<StreamId, StreamId>,
    subs: HashMap<ThreadId, Sub>,
    delivering: HashSet<MessageId>,
    last_rx: Instant,
    last_ping: Instant,
    sync_full: bool,
}

#[derive(Debug)]
struct Target {
    def: TargetDef,
    state: ConnectionState,
    supervisor: Option<(u64, JoinHandle<()>, Arc<crate::conn::Wake>)>,
    conn: Option<Conn>,
    /// The `yaacd` build this target's connector installs and starts when nothing serves
    /// ([`yaac_transport::Connector::daemon_build`]); `None` when it does not know.
    daemon_build: Option<String>,
}

impl Target {
    fn entry(&self) -> TargetEntry {
        TargetEntry {
            id: self.def.id.clone(),
            name: self.def.name.clone(),
            transport: self.def.transport.clone(),
            enabled: self.def.enabled,
            state: self.state.clone(),
        }
    }

    fn ready_conn(&mut self) -> Option<&mut Conn> {
        self.conn.as_mut().filter(|c| c.ready)
    }
}

fn state(status: ConnectionStatus, detail: Option<String>) -> ConnectionState {
    ConnectionState { status, detail, daemon: None, update: None }
}

fn err(code: ErrorCode, message: impl Into<String>) -> ErrorObject {
    ErrorObject::new(code, message)
}

fn file_view_missing() -> ErrorObject {
    err(ErrorCode::CapabilityMissing, "the target daemon does not support the worktree browser (fileView)")
}

fn hub_err(code: &str, message: impl Into<String>) -> ErrorObject {
    ErrorObject::new(ErrorCode::from_wire(code), message)
}

fn to_value<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

fn parse<T: DeserializeOwned>(params: Value) -> Result<T, ErrorObject> {
    serde_json::from_value(params).map_err(|e| err(ErrorCode::InvalidParams, e.to_string()))
}

// A build's numbered id: the `<id>` of `<version>+<id>` when it is a number (a build made by
// the build scripts, `scripts/build-id.sh`); `None` for a development build (`+dev…`) or none.
use yaac_transport::bootstrap::build_number;

/// Compares builds, window against hub: by their leading `major.minor.patch`, then by their
/// build numbers when both have one. Otherwise, equal numbers with a different suffix count as
/// the window being newer (the window that was just built replaces the hub).
fn compare_builds(window: &str, hub: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering as O;
    if window == hub {
        return O::Equal;
    }
    let numbers = |b: &str| -> Vec<u64> {
        b.split(['+', '-', ' '])
            .next()
            .unwrap_or_default()
            .split('.')
            .map(|p| p.parse().unwrap_or(0))
            .collect()
    };
    match numbers(window).cmp(&numbers(hub)) {
        O::Equal => match (build_number(window), build_number(hub)) {
            (Some(w), Some(h)) if w != h => w.cmp(&h),
            _ => O::Greater,
        },
        other => other,
    }
}

/// Whether a target's `yaacd` comes from an older build than this hub: both carry a build
/// number (every artefact of one build shares it, whatever each one's own version) and the
/// daemon's is lower. A development daemon counts as older than a numbered hub (a release
/// replaces it); a development hub never calls anything older (P94).
fn daemon_build_older(daemon: &str, hub: &str) -> bool {
    match (build_number(daemon), build_number(hub)) {
        (Some(d), Some(h)) => d < h,
        (None, Some(_)) => true,
        _ => false,
    }
}

/// Whether a serving daemon may be handed over to the build `to` (the build this target's
/// connector installs and starts): only to a numbered build, strictly newer than the serving
/// one — a serving development build counts as older than any numbered one (P94). A hub
/// without a bundle, a development bundle, or the same or an older build never hands over
/// (docs/02-architecture.md#one-serving-daemon-per-user).
fn hands_over_to(serving: &str, to: Option<&str>) -> bool {
    match (build_number(serving), to.and_then(build_number)) {
        (Some(s), Some(t)) => t > s,
        (None, Some(_)) => true,
        _ => false,
    }
}

/// A hand-over the hub asked of a target's daemon, kept per target across the reconnect: the
/// loop guard. A pair of builds is asked at most once; if the same older build serves again
/// after it, the hub stops and says so ([`DaemonUpdatePhase::Stuck`]) instead of asking again
/// (CLAUDE.md, invariant 10).
#[derive(Debug)]
struct Handover {
    /// The build that was serving.
    from: String,
    /// The build that should start.
    to: String,
    /// **Update now** (`when: now`).
    forced: bool,
    /// The daemon's answer, once it came.
    answer: Option<HandoverState>,
    /// A connection closed since it was asked.
    closed: bool,
    /// The same older build served again after the hand-over: not asked again.
    stuck: Option<String>,
}

impl Handover {
    fn view(&self) -> DaemonUpdate {
        let phase = if self.stuck.is_some() {
            DaemonUpdatePhase::Stuck
        } else if self.forced || self.closed || self.answer == Some(HandoverState::Exiting) {
            DaemonUpdatePhase::Updating
        } else {
            DaemonUpdatePhase::Waiting
        };
        DaemonUpdate { from: self.from.clone(), to: self.to.clone(), phase, forced: self.forced, detail: self.stuck.clone() }
    }
}

// ── The core ──────────────────────────────────────────────────────────────────────────────

pub(crate) struct Core {
    cfg: HubConfig,
    settings: Settings,
    worker: Worker,
    tx: mpsc::UnboundedSender<CoreEvent>,
    clients: HashMap<ClientKey, Client>,
    targets: BTreeMap<TargetId, Target>,
    /// Threads some window has open, and which windows.
    open: HashMap<Key, BTreeSet<ClientKey>>,
    /// The position the cache will hold once queued jobs finish.
    applied: HashMap<Key, JournalPosition>,
    /// The position the cache holds now.
    committed: HashMap<Key, JournalPosition>,
    tokens: HashMap<TargetId, SyncToken>,
    notifications: notifications::Notifications,
    thread_states: HashMap<Key, ThreadState>,
    batch: BTreeMap<Key, Vec<ThreadEventParams>>,
    in_flight: u64,
    supervisors: Arc<AtomicU64>,
    serials: u64,
    idle_since: Option<Instant>,
    last_ack: Instant,
    last_evict: Instant,
    last_notice_sweep: Instant,
    finder: Pager,
    /// `thread/lookup`s fanned out to targets, waiting for their answers.
    lookups: HashMap<u64, Lookup>,
    next_lookup: u64,
    /// Each target's recent connection steps (diagnostics), newest last.
    steps: HashMap<TargetId, std::collections::VecDeque<ConnectionStep>>,
    /// Threads the user keeps (pinned or muted), per target: sent as `thread/keep`.
    kept: HashMap<TargetId, BTreeSet<ThreadId>>,
    /// Threads marked unseen (D114), mirrored from the view store.
    unseen: HashSet<Key>,
    /// Uploads in progress, by handle ([`uploads`]).
    uploads: HashMap<String, uploads::Upload>,
    upload_seq: u64,
    upload_preview_slots: Arc<tokio::sync::Semaphore>,
    terminals: terminal::Relays,
    voice: voice::Relays,
    voice_calls: voice_call::Calls,
    attach_tunnels: attach_tunnel::AttachTunnels,
    previews: preview::Previews,
    browser: browser::BrowserCalls,
    /// File reads and hashing.
    pool: Pool,
    /// `hub/targets/start` requests waiting for their target to end connected or not.
    starts: HashMap<TargetId, Vec<(ClientKey, RpcId)>>,
    /// SSH setup state: host keys to confirm, open install sessions.
    ssh: ssh::SshState,
    /// Hand-overs asked of older daemons, per target ([`Handover`]).
    handovers: HashMap<TargetId, Handover>,
    /// Rate limits of every account on every target (D161).
    limits: limits::LimitsState,
    pub(crate) exit: Option<ExitReason>,
    /// The hub setting `tailscaleAttach`, followed by the Tailscale listener.
    tailscale_on: tokio::sync::watch::Sender<bool>,
    /// Attaching over Tailscale now.
    tailscale: crate::protocol::TailscaleAttach,
    /// An update of this computer a phone asked for (`hub/update/request`).
    host_update: Option<host_update::Running>,
}

/// What a fanned-out lookup answers once every target has.
#[derive(Debug)]
enum LookupFor {
    /// `hub/threads/lookup`.
    Window { client: ClientKey, id: RpcId },
    /// `hub/threads/find`, whose index search found nothing for an id.
    Find { client: ClientKey, id: RpcId, params: FindParams },
    /// An agent's `findThreads`, likewise.
    Mesh { source: TargetId, generation: u64, peer_id: RpcId, query: Box<yaac_protocol::MeshQuery>, labels: Vec<(TargetId, crate::mesh::TargetLabels)> },
}

#[derive(Debug)]
struct Lookup {
    waiting: usize,
    found: Vec<(TargetId, yaac_protocol::ThreadSummary)>,
    unreachable: Vec<TargetId>,
    answer: LookupFor,
}

/// At most this many connection steps are kept per target.
const STEPS_KEPT: usize = 50;

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

impl std::fmt::Debug for Core {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Core").field("targets", &self.targets.len()).field("clients", &self.clients.len()).finish_non_exhaustive()
    }
}

/// What the core is started with (loaded from the stores before the runtime starts).
#[derive(Debug, Default)]
pub(crate) struct Loaded {
    pub(crate) positions: Vec<(TargetId, ThreadId, JournalPosition)>,
    pub(crate) tokens: Vec<(TargetId, SyncToken)>,
    pub(crate) states: Vec<(TargetId, ThreadId, ThreadState)>,
    pub(crate) in_flight: u64,
    /// Threads the user keeps (pinned or muted in the view store), per target.
    pub(crate) kept: Vec<(TargetId, ThreadId)>,
    /// Threads marked unseen in the view store.
    pub(crate) unseen: Vec<(TargetId, ThreadId)>,
    /// The last rate limits of every account (D161).
    pub(crate) rate_limits: crate::limits::RateLimitBook,
}

impl Core {
    pub(crate) fn new(
        cfg: HubConfig,
        settings: Settings,
        worker: Worker,
        pool: Pool,
        tx: mpsc::UnboundedSender<CoreEvent>,
        loaded: Loaded,
    ) -> Self {
        let mut core = Self {
            cfg,
            settings,
            worker,
            tx,
            clients: HashMap::new(),
            targets: BTreeMap::new(),
            open: HashMap::new(),
            applied: HashMap::new(),
            committed: HashMap::new(),
            tokens: loaded.tokens.into_iter().collect(),
            notifications: Default::default(),
            thread_states: loaded.states.into_iter().map(|(t, th, s)| ((t, th), s)).collect(),
            batch: BTreeMap::new(),
            in_flight: loaded.in_flight,
            supervisors: Arc::new(AtomicU64::new(0)),
            serials: 0,
            idle_since: None,
            last_ack: Instant::now(),
            last_evict: Instant::now(),
            last_notice_sweep: Instant::now(),
            finder: Pager::default(),
            lookups: HashMap::new(),
            next_lookup: 0,
            steps: HashMap::new(),
            kept: HashMap::new(),
            unseen: loaded.unseen.into_iter().collect(),
            uploads: HashMap::new(),
            upload_seq: 0,
            upload_preview_slots: Arc::new(tokio::sync::Semaphore::new(upload_preview::CONCURRENT_PREVIEWS)),
            terminals: Default::default(),
            voice: Default::default(),
            voice_calls: Default::default(),
            attach_tunnels: Default::default(),
            previews: preview::Previews::default(),
            browser: browser::BrowserCalls::default(),
            pool,
            starts: HashMap::new(),
            ssh: ssh::SshState::default(),
            handovers: HashMap::new(),
            limits: limits::LimitsState::new(loaded.rate_limits),
            exit: None,
            tailscale_on: tokio::sync::watch::channel(false).0,
            tailscale: Default::default(),
            host_update: None,
        };
        core.tailscale_on.send_replace(core.settings.hub.tailscale_attach);
        for (target, thread) in loaded.kept {
            core.kept.entry(target).or_default().insert(thread);
        }
        for (target, thread, position) in loaded.positions {
            core.applied.insert((target.clone(), thread.clone()), position);
            core.committed.insert((target, thread), position);
        }
        let defs = core.settings.targets.clone();
        for def in defs {
            let id = def.id.clone();
            let initial = if def.enabled { state(ConnectionStatus::Connecting, None) } else { state(ConnectionStatus::Disabled, None) };
            core.targets.insert(id.clone(), Target { def, state: initial, supervisor: None, conn: None, daemon_build: None });
            core.start_target(&id);
        }
        core
    }

    /// The setting the Tailscale listener follows.
    pub(crate) fn tailscale_switch(&self) -> tokio::sync::watch::Receiver<bool> {
        self.tailscale_on.subscribe()
    }

    /// Stops every connection and hands back the worker so its queue can drain.
    pub(crate) fn finish(mut self) -> (Worker, Vec<crate::attach_writer::Sender>) {
        for target in self.targets.values_mut() {
            if let Some((_, handle, _)) = target.supervisor.take() {
                handle.abort();
            }
            target.conn = None;
        }
        let clients = self.clients.drain().map(|(_, c)| c.tx).collect();
        (self.worker, clients)
    }

    pub(crate) fn handle(&mut self, event: CoreEvent) {
        match event {
            CoreEvent::NotificationAction(group, action) => self.notification_action(group, &action),
            CoreEvent::NotificationDestination(open, lineage) => self.show_notification_destination(open, &lineage),
            CoreEvent::Attached { client, tx, remote } => {
                self.clients.insert(client, Client { tx, hello: Hello::None, remote, subs: HashSet::new(), showing: HashSet::new(), visible_requests: None, feed: false });
            }
            CoreEvent::ClientData { client, stream, bytes } => {
                if !self.attach_tunnel_input(client, stream, bytes.clone()) && !self.voice_input(client, stream, bytes.clone()) {
                    self.terminal_input(client, stream, bytes);
                }
            }
            CoreEvent::FromClient { client, message } => self.on_client(client, message),
            CoreEvent::Detached { client } => self.on_detached(client),
            CoreEvent::Conn { target, supervisor, generation, event } => self.on_conn(&target, supervisor, generation, event),
            CoreEvent::Committed { target, thread, events, position } => self.on_committed(&target, thread, events, position),
            CoreEvent::Rebuilt { target, thread, position } => self.on_rebuilt(&target, &thread, position),
            CoreEvent::Reply { client, id, outcome } => self.reply(client, id, outcome),
            CoreEvent::CatalogWindow { client, id, outcome, changed } => {
                if changed && outcome.is_ok() {
                    self.broadcast(&Message::notification(attach::names::CATALOG_SKILLS_CHANGED, json!({})));
                }
                self.reply(client, id, outcome);
            }
            CoreEvent::CatalogTarget { source, generation, id, outcome } => {
                self.respond_peer(&source, generation, id, names::CATALOG_SKILLS, outcome);
            }
            CoreEvent::Broadcast(message) => self.broadcast(&message),
            CoreEvent::RelayAccepted { source, generation, peer_id, message, accepted, in_flight } => {
                self.in_flight = in_flight;
                self.on_relay_accepted(&source, generation, peer_id, message, accepted);
            }
            CoreEvent::Deliver { dest, messages, in_flight } => {
                self.in_flight = in_flight;
                for message in messages {
                    self.deliver(&dest, message);
                }
            }
            CoreEvent::DeliveryReceipt { dest, generation, peer_id, outcome, params, in_flight } => {
                self.in_flight = in_flight;
                let result = match outcome {
                    Ok(source) => {
                        if let Some(source) = source {
                            self.send_receipt(&source, params.message_id, params.receipt);
                        }
                        Ok(json!({}))
                    }
                    Err(e) => Err(err(ErrorCode::InternalError, e)),
                };
                self.respond_peer(&dest, generation, peer_id, names::MESH_DELIVERY_RECEIPT, result);
            }
            CoreEvent::Receipts { receipts, in_flight } => {
                self.in_flight = in_flight;
                for (source, message_id, receipt) in receipts {
                    self.send_receipt(&source, message_id, receipt);
                }
            }
            CoreEvent::Notices { deliveries, in_flight } => {
                self.in_flight = in_flight;
                for (dest, message) in deliveries {
                    self.deliver(&dest, message);
                }
            }
            CoreEvent::FindReady { client, id, params, threads } => self.on_find_ready(client, id, &params, &threads),
            CoreEvent::MeshIndexReady { source, generation, peer_id, query, threads, labels } => {
                self.on_mesh_index(&source, generation, peer_id, &query, &threads, &labels);
            }
            CoreEvent::PendingModes(target, generation, changes) => {
                let conn = self.targets.get_mut(&target).and_then(|t| t.conn.as_mut());
                let superseded = conn.filter(|c| c.generation == generation)
                    .and_then(|c| c.modes_during_reconnect.take()).unwrap_or_default();
                for change in changes {
                    if superseded.contains(&change.thread_id) {
                        self.worker.submit(move |s| s.drafts.op_done(&change.op_id).err().map(|e| CoreEvent::StoreError(e.to_string())));
                        continue;
                    }
                    let receipt = Pending::Mode { op: change.op_id.clone(), thread: change.thread_id.clone() };
                    if !self.send::<methods::ThreadSetMode>(&target, &change, receipt) {
                        let t = target.clone();
                        let keep = self.targets.contains_key(&target);
                        self.worker.submit(move |s| {
                            let result = if keep { s.drafts.restore_unsent_mode(&t, &change) }
                                else { s.drafts.op_done(&change.op_id) };
                            result.err().map(|e| CoreEvent::StoreError(e.to_string()))
                        });
                    }
                }
                self.flush(&target);
            }
            CoreEvent::InFlight(n) => self.in_flight = n,
            CoreEvent::ViewsChanged { reply, views } => self.on_views_changed(reply, views),
            CoreEvent::ShutdownReady { client, id } => {
                self.reply(client, id, Ok(json!({})));
                self.exit = Some(ExitReason::Shutdown);
            }
            CoreEvent::StoreError(e) => log(format!("store: {e}")),
            CoreEvent::Signal => self.exit = Some(ExitReason::Signal),
            CoreEvent::HostUpdate(event) => self.host_update_event(event),
            CoreEvent::Tailscale(status) => {
                if status != self.tailscale {
                    self.tailscale = status;
                    self.broadcast(&Message::notification(ext::names::TAILSCALE_STATUS, to_value(&self.tailscale)));
                }
            }
            CoreEvent::UploadSource { handle, result } => self.on_upload_source(&handle, result),
            CoreEvent::UploadHashed { handle, result } => self.on_upload_hashed(&handle, result),
            CoreEvent::UploadChunk { handle, offset, result } => self.on_upload_chunk(&handle, offset, result),
            CoreEvent::UploadPreviewReady { client, id, outcome } => match outcome {
                Ok(result) => {
                    if let Some(c) = self.clients.get(&client) {
                        let _ = c.tx.bulk_reply(Message::Response { id, result });
                    }
                }
                Err(error) => self.reply(client, id, Err(error)),
            },
            CoreEvent::Preview(event) => self.preview_event(event),
            CoreEvent::Ssh(event) => self.on_ssh(event),
        }
        self.flush_batches();
    }

    // ── Windows ───────────────────────────────────────────────────────────────────────

    fn send_to(&self, client: ClientKey, message: Message) {
        if let Some(c) = self.clients.get(&client) {
            let _ = c.tx.send(message);
        }
    }

    fn reply(&self, client: ClientKey, id: RpcId, outcome: Result<Value, ErrorObject>) {
        let message = match outcome {
            Ok(result) => Message::Response { id, result },
            Err(error) => Message::Error { id: Some(id), error },
        };
        self.send_to(client, message);
    }

    fn broadcast(&self, message: &Message) {
        for c in self.clients.values().filter(|c| c.hello == Hello::Attached) {
            let _ = c.tx.send(message.clone());
        }
    }

    fn notify_all(&self, method: &str, params: Value) {
        self.broadcast(&Message::notification(method, params));
    }

    fn routed_notification(target: &TargetId, method: &str, params: Value) -> Message {
        let n = RoutedNotification { target: target.clone(), method: method.to_owned(), params };
        Message::notification(attach::names::NOTIFICATION, to_value(&n))
    }

    fn on_detached(&mut self, client: ClientKey) {
        let Some(c) = self.clients.remove(&client) else { return };
        if c.visible_requests.is_some_and(|cards| !cards.is_empty()) { self.notifications.reconsider(Instant::now()); }
        self.terminal_window_closed(client);
        self.voice_window_closed(client);
        self.voice_call_window_closed(client);
        self.attach_tunnels_client_gone(client);
        self.browser_window_closed(client);
        self.cancel_preview_reads(client);
        for key in c.subs {
            self.release(client, &key);
        }
    }

    fn on_client(&mut self, client: ClientKey, message: Message) {
        match message {
            Message::Request { id, method, params } => {
                let outcome = self.on_client_request(client, &id, &method, params);
                if let Some(outcome) = outcome {
                    self.reply(client, id, outcome);
                }
            }
            Message::Notification { method, params } if method == names::STREAM_CREDIT => {
                if let Ok(credit) = parse::<StreamCreditParams>(params)
                    && !self.attach_tunnel_client_credit(client, &credit)
                {
                    self.terminal_output_credit(client, &credit);
                }
            }
            Message::Notification { method, params }
                if method == attach::names::NOTIFICATION && self.clients.get(&client).is_some_and(|c| c.hello == Hello::Attached) =>
            {
                self.on_client_notification(params);
            }
            Message::Response { id, result } => self.browser_answer(client, &id, Ok(result)),
            Message::Error { id: Some(id), error } => self.browser_answer(client, &id, Err(error)),
            _ => {}
        }
    }

    fn on_client_notification(&mut self, params: Value) {
        let Ok(n) = serde_json::from_value::<RoutedNotification>(params) else { return };
        if n.method != names::STREAM_CREDIT {
            return; // the only window → target notification there is so far
        }
        let Ok(credit) = serde_json::from_value::<StreamCreditParams>(n.params) else { return };
        if let Some(conn) = self.targets.get_mut(&n.target).and_then(Target::ready_conn) {
            if conn.preview_streams.contains_key(&credit.stream) || conn.terminal_streams.contains_key(&credit.stream) || conn.voice_streams.contains_key(&credit.stream) || conn.attach_tunnels.contains_key(&credit.stream) {
                return; // The hub is the receiver on this stream.
            }
            let _ = conn.session.notify::<methods::StreamCredit>(&credit);
        }
        self.flush(&n.target);
    }

    /// Handles a window request. `None`: the answer comes later.
    fn on_client_request(&mut self, client: ClientKey, id: &RpcId, method: &str, params: Value) -> Option<Result<Value, ErrorObject>> {
        let hello = self.clients.get(&client).map_or(Hello::None, |c| c.hello);
        if method == attach::names::HELLO {
            return Some(self.on_hello(client, params));
        }
        if method == attach::names::UPDATE_REQUEST {
            // Frozen core: served whatever the client's build (docs/09-compatibility.md).
            return self.host_update_request(client, id, params);
        }
        let remote = self.clients.get(&client).is_some_and(|c| c.remote);
        if remote && method == attach::names::SHUTDOWN {
            // Whatever the builds: a phone never stops or replaces the PC's hub.
            return Some(Err(hub_err(ext::codes::REMOTE_NOT_ALLOWED, "The hub is stopped only from its own computer, never from a client over Tailscale.")));
        }
        match (hello, method) {
            (Hello::Attached | Hello::HubOutdated, attach::names::SHUTDOWN) => {
                let id = id.clone();
                self.worker.submit(move |stores| {
                    let _ = stores.relay.checkpoint();
                    let _ = stores.drafts.checkpoint();
                    let _ = stores.cache.checkpoint();
                    Some(CoreEvent::ShutdownReady { client, id })
                });
                return None;
            }
            (Hello::Attached, _) => {}
            (Hello::None, _) => return Some(Err(hub_err(ext::codes::HELLO_REQUIRED, "send hub/hello first"))),
            (Hello::HubOutdated, _) => {
                return Some(Err(hub_err(ext::codes::HUB_OUTDATED, "the running hub is older than this window; send hub/shutdown")));
            }
            (Hello::WindowOutdated, _) => {
                return Some(Err(hub_err(ext::codes::WINDOW_OUTDATED, "this window is older than the running hub; update YAAC")));
            }
        }
        match method {
            attach::names::TARGETS_LIST => Some(Ok(to_value(&TargetsListResult { targets: self.entries() }))),
            attach::names::RATE_LIMITS_GET => Some(parse(params).and_then(|p| self.limits_get(&p))),
            attach::names::TARGETS_ADD => Some(parse(params).and_then(|p| self.targets_add(p))),
            attach::names::TARGETS_UPDATE => Some(parse(params).and_then(|p| self.targets_update(p))),
            attach::names::TARGETS_REMOVE => Some(parse(params).and_then(|p| self.targets_remove(&p))),
            attach::names::TARGETS_ENABLE => Some(parse(params).and_then(|p| self.targets_enable(&p))),
            attach::names::REQUEST => match parse::<RoutedRequest>(params) {
                Ok(request) => self.on_routed(client, id, request),
                Err(e) => Some(Err(e)),
            },
            ext::names::TARGETS_RECONNECT => Some(parse::<TargetRef>(params).and_then(|p| {
                let t = self.targets.get(&p.id).ok_or_else(|| err(ErrorCode::TargetUnknown, "no such target"))?;
                if let Some((_, _, wake)) = &t.supervisor {
                    wake.retry();
                }
                Ok(json!({}))
            })),
            ext::names::TARGETS_START => match parse::<TargetRef>(params) {
                Ok(p) => self.targets_start(client, id, &p.id),
                Err(e) => Some(Err(e)),
            },
            attach::names::TARGETS_UPDATE_DAEMON => match parse::<TargetsUpdateDaemonParams>(params) {
                Ok(p) => self.targets_update_daemon(client, id, &p.id),
                Err(e) => Some(Err(e)),
            },
            ext::names::WSL_DISTROS => {
                self.wsl_distros(client, id);
                None
            }
            ext::names::STATUS => Some(Ok(to_value(&self.status()))),
            ext::names::THREADS_FIND => match parse::<FindParams>(params) {
                Ok(p) => {
                    self.find_threads(client, id, p);
                    None
                }
                Err(e) => Some(Err(e)),
            },
            ext::names::THREADS_LOOKUP => match parse::<LookupParams>(params) {
                Ok(p) => {
                    let targets = self.lookup_targets(p.target.as_ref());
                    self.start_lookup(&p.id, &targets, LookupFor::Window { client, id: id.clone() });
                    None
                }
                Err(e) => Some(Err(e)),
            },
            ext::names::VIEW_GET => {
                self.store_reply(client, id, |s| s.drafts.views().map(|views| to_value(&ext::ViewGetResult { views })).map_err(|e| e.to_string()));
                None
            }
            ext::names::VIEW_SET => match parse::<ext::ViewSetParams>(params) {
                Ok(p) => {
                    self.set_views(vec![p], Some((client, id.clone(), true)));
                    None
                }
                Err(e) => Some(Err(e)),
            },
            attach::names::NOTIFICATIONS_SUBSCRIBE => {
                if let Some(c) = self.clients.get_mut(&client) {
                    c.feed = true;
                }
                Some(Ok(to_value(&self.notifications.feed)))
            }
            attach::names::NOTIFICATIONS_RESPOND => match parse::<attach::FeedRespondParams>(params) {
                Ok(p) => self.feed_respond(client, id, p),
                Err(e) => Some(Err(e)),
            },
            attach::names::NOTIFICATIONS_VISIBLE => Some(parse::<HashSet<attach::RequestLocation>>(params).map(|visible| {
                self.clients.get_mut(&client).unwrap().visible_requests = Some(visible);
                self.notifications.reconsider(Instant::now());
                while let Some(open) = self.notifications.open.pop_front() {
                    self.send_to(client, Message::notification(attach::names::NOTIFICATIONS_OPEN, to_value(&open)));
                }
                json!({})
            })),
            ext::names::VIEW_SHOWING => match parse::<ext::ViewShowingParams>(params) {
                Ok(p) => {
                    if let Some(c) = self.clients.get_mut(&client) {
                        c.showing = p.threads.into_iter().map(|t| (t.target, t.thread_id)).collect();
                    }
                    Some(Ok(json!({})))
                }
                Err(e) => Some(Err(e)),
            },
            ext::names::VIEW_SET_MANY => match parse::<ext::ViewSetManyParams>(params) {
                Ok(p) => {
                    self.set_views(p.entries, Some((client, id.clone(), false)));
                    None
                }
                Err(e) => Some(Err(e)),
            },
            ext::names::TARGETS_LOG => Some(parse::<TargetRef>(params).and_then(|p| {
                if !self.targets.contains_key(&p.id) {
                    return Err(err(ErrorCode::TargetUnknown, "no such target"));
                }
                let steps = self.steps.get(&p.id).map(|s| s.iter().cloned().collect()).unwrap_or_default();
                Ok(to_value(&TargetLogResult { steps }))
            })),
            ext::names::PROJECTS_LIST => match parse::<TargetRef>(params) {
                Ok(p) => {
                    self.store_reply(client, id, move |s| {
                        s.cache.projects(&p.id).map(|projects| json!({ "projects": projects })).map_err(|e| e.to_string())
                    });
                    None
                }
                Err(e) => Some(Err(e)),
            },
            attach::names::CATALOG_SKILLS => match parse::<yaac_protocol::CatalogSkillsParams>(params) {
                Ok(p) => {
                    self.store_reply(client, id, move |s| s.drafts.catalog(&p).map(|r| to_value(&r)));
                    None
                }
                Err(e) => Some(Err(e)),
            },
            attach::names::CATALOG_SKILL_SAVE => match parse::<yaac_protocol::CatalogSkillSave>(params) {
                Ok(p) => {
                    let id = id.clone();
                    self.worker.submit(move |s| {
                        let outcome = s.drafts.save_catalog_skill(p).map(|r| to_value(&r))
                            .map_err(|e| err(ErrorCode::InvalidParams, e));
                        Some(CoreEvent::CatalogWindow { client, id, outcome, changed: true })
                    });
                    None
                }
                Err(e) => Some(Err(e)),
            },
            attach::names::CATALOG_SKILL_REMOVE => match parse::<yaac_protocol::CatalogSkillRemove>(params) {
                Ok(p) => {
                    let id = id.clone();
                    self.worker.submit(move |s| {
                        let outcome = s.drafts.remove_catalog_skill(&p).map(|()| json!({}))
                            .map_err(|e| err(ErrorCode::InvalidParams, e));
                        Some(CoreEvent::CatalogWindow { client, id, outcome, changed: true })
                    });
                    None
                }
                Err(e) => Some(Err(e)),
            },
            ext::names::SETTINGS_GET => Some(Ok(to_value(&SettingsResult { settings: self.settings.hub.clone(), tailscale: self.tailscale.clone() }))),
            ext::names::SETTINGS_UPDATE => Some(parse::<SettingsUpdateParams>(params).and_then(|p| {
                if remote && (p.always.is_some() || p.idle_exit_secs.is_some()) {
                    // When the hub runs and exits is decided on its computer. (A phone may
                    // still turn `tailscaleAttach` off, which only ends its own access.)
                    return Err(hub_err(ext::codes::REMOTE_NOT_ALLOWED, "Whether the hub starts at login and keeps running is set on its own computer."));
                }
                let previous = self.settings.hub.clone();
                if let Some(settings) = &p.explanations { self.settings.hub.explanations = settings.clone(); }
                if let Some(settings) = p.notifications { self.settings.hub.notifications = settings; }
                if let Some(always) = p.always {
                    self.settings.hub.always = always;
                }
                if let Some(secs) = p.idle_exit_secs {
                    self.settings.hub.idle_exit_secs = secs;
                }
                if let Some(on) = p.tailscale_attach {
                    self.settings.hub.tailscale_attach = on;
                }
                if let Err(e) = self.save_settings() {
                    self.settings.hub = previous;
                    return Err(e);
                }
                let on = self.settings.hub.tailscale_attach;
                self.tailscale_on.send_if_modified(|current| std::mem::replace(current, on) != on);
                if p.explanations.is_some() {
                    self.broadcast(&Message::notification("hub/approvals/settings", to_value(&self.settings.hub.explanations)));
                }
                if p.notifications.is_some() {
                    self.broadcast(&Message::notification("hub/notifications/settings", to_value(&self.settings.hub.notifications)));
                    self.notifications.reconsider(Instant::now());
                }
                Ok(to_value(&SettingsResult { settings: self.settings.hub.clone(), tailscale: self.tailscale.clone() }))
            })),
            ext::names::CACHE_CLEAR => {
                self.clear_cache(client, id.clone());
                None
            }
            ext::names::UI_STATE_GET => match parse::<ext::UiStateGetParams>(params) {
                Ok(p) => {
                    self.store_reply(client, id, move |s| {
                        let mut values = serde_json::Map::new();
                        for key in p.keys {
                            if let Some(v) = s.cache.ui_state(&key).map_err(|e| e.to_string())? {
                                let v = if key == attach::COMPOSER_HISTORY {
                                    json!(attach::history_entries(&v).collect::<Vec<_>>())
                                } else {
                                    v
                                };
                                values.insert(key, v);
                            }
                        }
                        Ok(to_value(&ext::UiStateGetResult { values }))
                    });
                    None
                }
                Err(e) => Some(Err(e)),
            },
            ext::names::UI_STATE_SET => match parse::<UiStateSetParams>(params) {
                Ok(p) => {
                    self.store_reply(client, id, move |s| s.cache.set_ui_state(&p.key, &p.value).map(|()| json!({})).map_err(|e| e.to_string()));
                    None
                }
                Err(e) => Some(Err(e)),
            },
            attach::names::COMPOSER_HISTORY_PUSH => match parse::<attach::ComposerHistoryPush>(params) {
                Ok(p) if attach::history_entry(&p.text) => {
                    self.store_reply(client, id, move |s| {
                        let key = attach::COMPOSER_HISTORY;
                        let stored = s.cache.ui_state(key).map_err(|e| e.to_string())?.unwrap_or_default();
                        let history = std::iter::once(p.text.as_str()).chain(attach::history_entries(&stored))
                            .take(attach::COMPOSER_HISTORY_LIMIT).collect::<Vec<_>>();
                        s.cache.set_ui_state(key, &json!(history)).map_err(|e| e.to_string())?;
                        Ok(json!({}))
                    });
                    None
                }
                Ok(_) => Some(Err(err(ErrorCode::InvalidParams, "message is empty or too large for history"))),
                Err(e) => Some(Err(e)),
            },
            ext::names::DRAFTS_LIST => {
                self.store_reply(client, id, |s| {
                    s.drafts.list().map(|drafts| to_value(&DraftsListResult { drafts })).map_err(|e| e.to_string())
                });
                None
            }
            ext::names::DRAFTS_PUT => match parse::<Draft>(params) {
                Ok(d) => {
                    self.store_reply(client, id, move |s| s.drafts.put(&d).map(|d| to_value(&d)).map_err(|e| e.to_string()));
                    None
                }
                Err(e) => Some(Err(e)),
            },
            ext::names::DRAFTS_DELETE => match parse::<DraftKey>(params) {
                Ok(k) => {
                    self.store_reply(client, id, move |s| s.drafts.delete(&k.key).map(|()| json!({})).map_err(|e| e.to_string()));
                    None
                }
                Err(e) => Some(Err(e)),
            },
            ext::names::OPERATIONS_LIST => {
                self.store_reply(client, id, |s| {
                    s.drafts.operations().map(|operations| to_value(&OperationsListResult { operations })).map_err(|e| e.to_string())
                });
                None
            }
            ext::names::OPERATIONS_DISMISS => match parse::<OperationRef>(params) {
                Ok(o) => {
                    self.store_reply(client, id, move |s| s.drafts.op_done(&o.op_id).map(|()| json!({})).map_err(|e| e.to_string()));
                    None
                }
                Err(e) => Some(Err(e)),
            },
            ext::names::UPLOAD_START => Some(parse::<ext::UploadStartParams>(params).and_then(|p| self.upload_start(p))),
            ext::names::UPLOAD_CANCEL => Some(parse::<ext::UploadHandleParams>(params).map(|p| self.upload_cancel(&p.handle))),
            ext::names::UPLOAD_LIST => Some(Ok(self.upload_list())),
            attach::names::UPLOAD_PREVIEW => match parse(params).and_then(|p| self.upload_preview(client, id, p)) {
                Ok(()) => None,
                Err(error) => Some(Err(error)),
            },
            name if name.starts_with("hub/terminal/") => self.terminal_request(client, id, name, params),
            name if name.starts_with("hub/voice/") => self.voice_request(client, id, name, params),
            name if name.starts_with("hub/voiceCall/") => self.voice_call_request(client, id, name, params),
            attach::names::PREVIEW_TUNNEL => match parse(params) {
                Ok(p) => self.attach_tunnel_open(client, id, p),
                Err(e) => Some(Err(e)),
            },
            attach::names::PREVIEW_TUNNEL_CLOSE => Some(parse(params).map(|p| {
                self.attach_tunnel_client_close(client, &p);
                json!({})
            })),
            attach::names::PREVIEW_PROXY => Some(parse(params).and_then(|p| self.preview_proxy(p))),
            attach::names::PREVIEW_PROXY_CLOSE => Some(parse(params).and_then(|p| self.preview_proxy_close(p))),
            attach::names::PREVIEW_OPEN => Some(parse(params).and_then(|p| self.preview_open(p))),
            attach::names::PREVIEW_CLOSE => Some(parse(params).and_then(|p| self.preview_close(p))),
            attach::names::PREVIEW_LIST => Some(Ok(self.preview_list())),
            ext::names::BLOB_BEGIN => match parse::<ext::BlobBeginParams>(params) {
                Ok(p) => {
                    let max = self.cfg.uploads.max_blob;
                    self.blob_reply(client, id, move |s| {
                        let blob_id = s.drafts.blob_begin(&p.draft, &p.name, p.media_type.as_deref(), p.size, max)?;
                        Ok(to_value(&ext::BlobBeginResult { blob_id }))
                    });
                    None
                }
                Err(e) => Some(Err(e)),
            },
            ext::names::BLOB_APPEND => match parse::<ext::BlobAppendParams>(params) {
                Ok(p) => {
                    // Decoded on the store worker, not here.
                    self.blob_reply(client, id, move |s| {
                        use base64::Engine as _;
                        let bytes = base64::engine::general_purpose::STANDARD
                            .decode(p.data_base64.as_bytes())
                            .map_err(|e| crate::drafts::DraftsError::Blob { code: "invalid_params", message: format!("dataBase64: {e}") })?;
                        if bytes.len() > ext::BLOB_CHUNK_MAX {
                            return Err(crate::drafts::DraftsError::Blob {
                                code: "invalid_params",
                                message: format!("a chunk is at most {} bytes", ext::BLOB_CHUNK_MAX),
                            });
                        }
                        let received = s.drafts.blob_append(&p.blob_id, p.offset, &bytes)?;
                        Ok(to_value(&ext::BlobAppendResult { received }))
                    });
                    None
                }
                Err(e) => Some(Err(e)),
            },
            ext::names::BLOB_END => match parse::<ext::BlobRef>(params) {
                Ok(p) => {
                    self.blob_reply(client, id, move |s| Ok(to_value(&s.drafts.blob_end(&p.blob_id)?)));
                    None
                }
                Err(e) => Some(Err(e)),
            },
            ext::names::BLOB_READ => match parse::<ext::BlobReadParams>(params) {
                Ok(p) => {
                    self.blob_reply(client, id, move |stores| {
                        use base64::Engine as _;
                        use std::io::{Read, Seek, SeekFrom};
                        let Some((blob, path)) = stores.drafts.blob(&p.blob_id)? else {
                            return Ok(json!({ "present": false }));
                        };
                        if !blob.media_type.as_deref().is_some_and(|mime| mime.starts_with("image/")) {
                            return Ok(json!({ "present": false }));
                        }
                        let mut file = std::fs::File::open(path)?;
                        let offset = p.offset.min(blob.size);
                        file.seek(SeekFrom::Start(offset))?;
                        let mut bytes = vec![0; usize::try_from(blob.size - offset).map_or(ext::BLOB_CHUNK_MAX, |n| n.min(ext::BLOB_CHUNK_MAX))];
                        file.read_exact(&mut bytes)?;
                        Ok(json!({ "present": true, "size": blob.size, "mediaType": blob.media_type,
                            "more": offset + (bytes.len() as u64) < blob.size,
                            "dataBase64": base64::engine::general_purpose::STANDARD.encode(bytes) }))
                    });
                    None
                }
                Err(e) => Some(Err(e)),
            },
            ext::names::BLOB_LIST => match parse::<ext::BlobListParams>(params) {
                Ok(p) => {
                    self.blob_reply(client, id, move |s| Ok(to_value(&ext::BlobListResult { blobs: s.drafts.blobs(p.draft.as_deref())? })));
                    None
                }
                Err(e) => Some(Err(e)),
            },
            ext::names::BLOB_DELETE => match parse::<ext::BlobRef>(params) {
                Ok(p) => {
                    self.blob_reply(client, id, move |s| s.drafts.blob_delete(&p.blob_id).map(|()| json!({})));
                    None
                }
                Err(e) => Some(Err(e)),
            },
            ext::names::DEBUG_DROP_CONNECTION if self.cfg.test_methods => Some(parse::<TargetRef>(params).map(|p| {
                if let Some(kill) = self.targets.get_mut(&p.id).and_then(|t| t.conn.as_mut()).and_then(|c| c.kill.take()) {
                    let _ = kill.send(());
                }
                json!({})
            })),
            m if m.starts_with("hub/ssh/") => self.ssh_request(client, id, m, params),
            _ => Some(Err(err(ErrorCode::MethodNotFound, format!("unknown hub method {method}")))),
        }
    }

    fn store_reply(
        &self,
        client: ClientKey,
        id: &RpcId,
        job: impl FnOnce(&mut crate::worker::Stores) -> Result<Value, String> + Send + 'static,
    ) {
        let id = id.clone();
        self.worker.submit(move |stores| {
            let outcome = job(stores).map_err(|e| err(ErrorCode::InternalError, e));
            Some(CoreEvent::Reply { client, id, outcome })
        });
    }

    /// A draft-store job whose refusals keep their own error code.
    fn blob_reply(
        &self,
        client: ClientKey,
        id: &RpcId,
        job: impl FnOnce(&mut crate::worker::Stores) -> Result<Value, crate::drafts::DraftsError> + Send + 'static,
    ) {
        let id = id.clone();
        self.worker.submit(move |stores| {
            let outcome = job(stores).map_err(|e| match e {
                crate::drafts::DraftsError::Blob { code, message } => hub_err(code, message),
                other => err(ErrorCode::InternalError, other.to_string()),
            });
            Some(CoreEvent::Reply { client, id, outcome })
        });
    }

    fn on_hello(&mut self, client: ClientKey, params: Value) -> Result<Value, ErrorObject> {
        let hello: HelloParams = parse(params)?;
        let order = compare_builds(&hello.build, &self.cfg.build);
        let state = match order {
            std::cmp::Ordering::Equal => Hello::Attached,
            std::cmp::Ordering::Greater => Hello::HubOutdated,
            std::cmp::Ordering::Less => Hello::WindowOutdated,
        };
        if let Some(c) = self.clients.get_mut(&client) {
            c.hello = state;
        }
        if state == Hello::WindowOutdated {
            return Err(hub_err(
                ext::codes::WINDOW_OUTDATED,
                format!("This window ({}) is older than the running hub ({}). Update YAAC.", hello.build, self.cfg.build),
            ));
        }
        self.idle_since = None;
        Ok(to_value(&HelloResult {
            build: self.cfg.build.clone(),
            targets: self.entries(),
            client_id: self.settings.client_id.clone(),
            hub_id: self.settings.hub_id.clone(),
            host_name: Some(self.cfg.host_name.clone()).filter(|n| !n.is_empty()),
        }))
    }

    fn entries(&self) -> Vec<TargetEntry> {
        self.settings.targets.iter().filter_map(|d| self.targets.get(&d.id)).map(Target::entry).collect()
    }

    fn status(&self) -> StatusResult {
        StatusResult {
            build: self.cfg.build.clone(),
            pid: std::process::id(),
            busy: self.busy(),
            settings: self.settings.hub.clone(),
            cache_file: self.cfg.paths.cache_file().display().to_string(),
            client_id: self.settings.client_id.clone(),
            tailscale: self.tailscale.clone(),
        }
    }

    fn save_settings(&self) -> Result<(), ErrorObject> {
        self.settings.save(&self.cfg.paths.settings_file()).map_err(|e| err(ErrorCode::InternalError, e.to_string()))
    }

    fn clear_cache(&mut self, client: ClientKey, id: RpcId) {
        self.applied.clear();
        self.committed.clear();
        self.tokens.clear();
        // Every open subscription is now ahead of an empty cache: each is re-opened with no
        // position (a reset), once the cache is empty (the worker runs jobs in order).
        self.worker.submit(move |stores| {
            let outcome = stores.cache.clear().map(|()| json!({})).map_err(|e| err(ErrorCode::InternalError, e.to_string()));
            Some(CoreEvent::Reply { client, id, outcome })
        });
        let ready: Vec<TargetId> = self.targets.iter().filter(|(_, t)| t.conn.as_ref().is_some_and(|c| c.ready)).map(|(id, _)| id.clone()).collect();
        for target in ready {
            let subscribed: Vec<ThreadId> = self.targets[&target].conn.as_ref().map(|c| c.subs.keys().cloned().collect()).unwrap_or_default();
            for thread in &subscribed {
                let params = ThreadUnsubscribeParams { thread_id: thread.clone() };
                self.send::<methods::ThreadUnsubscribe>(&target, &params, Pending::Ignore);
            }
            self.refresh_target_with(&target, true);
            self.flush(&target);
        }
    }

    // ── Target management ────────────────────────────────────────────────────────────────

    fn targets_changed(&mut self) {
        let list = to_value(&TargetsListResult { targets: self.entries() });
        self.notify_all(ext::names::TARGETS_CHANGED, list);
        self.send_rosters();
    }

    fn set_state(&mut self, id: &TargetId, mut new: ConnectionState) {
        // An update under way stays visible across the reconnect it causes.
        new.update = self.handovers.get(id).map(Handover::view);
        self.settle_starts(id, &new);
        let Some(t) = self.targets.get_mut(id) else { return };
        if t.state == new {
            return;
        }
        t.state = new.clone();
        let step = match new.status {
            ConnectionStatus::Connecting => "connecting",
            ConnectionStatus::Connected => "ready",
            ConnectionStatus::Offline => "offline",
            ConnectionStatus::Disabled => "disabled",
            ConnectionStatus::Incompatible => "incompatible",
            _ => "state",
        };
        let detail = new.detail.clone().or_else(|| new.daemon.as_ref().map(|d| format!("yaacd {}", d.build)));
        self.step(id, step, detail);
        let Some(t) = self.targets.get(id) else { return };
        let params = to_value(&TargetStateParams { id: id.clone(), state: new.clone(), enabled: t.def.enabled });
        self.notify_all(attach::names::TARGET_STATE, params);
        self.limits_presence(id);
        let target = id.clone();
        self.worker.submit(move |stores| stores.cache.put_target(&target, None, &new).err().map(|e| CoreEvent::StoreError(e.to_string())));
    }

    // ── View preferences (D92) ───────────────────────────────────────────────────────────

    fn set_views(&mut self, patches: Vec<ext::ViewSetParams>, reply: Option<(ClientKey, RpcId, bool)>) {
        if let Some(bad) = patches.iter().find(|p| !self.targets.contains_key(&p.target)) {
            if let Some((client, id, _)) = reply {
                self.reply(client, id, Err(err(ErrorCode::TargetUnknown, format!("no target {}", bad.target))));
            }
            return;
        }
        self.worker.submit(move |s| match s.drafts.set_views(&patches) {
            Ok(views) => Some(CoreEvent::ViewsChanged { reply, views }),
            Err(e) => match reply {
                Some((client, id, _)) => Some(CoreEvent::Reply { client, id, outcome: Err(err(ErrorCode::InternalError, e.to_string())) }),
                None => Some(CoreEvent::StoreError(e.to_string())),
            },
        });
    }

    fn on_views_changed(&mut self, reply: Option<(ClientKey, RpcId, bool)>, views: Vec<ext::ThreadView>) {
        if let Some((client, id, one)) = reply {
            let result = if one {
                views.first().map(|v| to_value(&ext::ViewSetResult { view: v.clone() })).unwrap_or(Value::Null)
            } else {
                to_value(&ext::ViewGetResult { views: views.clone() })
            };
            self.reply(client, id, Ok(result));
        }
        let mut touched: BTreeSet<TargetId> = BTreeSet::new();
        for v in &views {
            let key = (v.target.clone(), v.thread_id.clone());
            if v.unseen {
                self.unseen.insert(key);
            } else {
                self.unseen.remove(&key);
            }
            let set = self.kept.entry(v.target.clone()).or_default();
            let changed = if v.keeps() { set.insert(v.thread_id.clone()) } else { set.remove(&v.thread_id) };
            if changed {
                touched.insert(v.target.clone());
            }
        }
        for t in touched {
            self.send_kept(&t);
        }
        if !views.is_empty() {
            self.notify_all(ext::names::VIEW_CHANGED, to_value(&ext::ViewChangedParams { views }));
        }
    }

    /// Tells a target which of its threads the user keeps (`thread/keep`).
    fn send_kept(&mut self, target: &TargetId) {
        let thread_ids: Vec<ThreadId> = self.kept.get(target).map(|s| s.iter().cloned().collect()).unwrap_or_default();
        self.send::<ThreadKeep>(target, &yaac_protocol::ThreadKeepParams { thread_ids }, Pending::Ignore);
        self.flush(target);
    }

    /// A thread is gone from its target: its pin and mute go with it.
    fn view_thread_removed(&mut self, target: &TargetId, thread: &ThreadId) {
        let key = (target.clone(), thread.clone());
        self.notifications.labels.remove(&key);
        self.notifications.snapshot(&key, &[]);
        self.release(NOTIFICATION_CLIENT, &key);
        if !self.kept.get(target).is_some_and(|s| s.contains(thread)) && !self.unseen.contains(&(target.clone(), thread.clone())) {
            return;
        }
        let (target, thread) = (target.clone(), thread.clone());
        self.worker.submit(move |s| match s.drafts.view_thread_removed(&target, &thread) {
            Ok(v) => Some(CoreEvent::ViewsChanged { reply: None, views: v.into_iter().collect() }),
            Err(e) => Some(CoreEvent::StoreError(e.to_string())),
        });
    }

    /// Records one connection step (diagnostics). Never message content.
    fn step(&mut self, id: &TargetId, step: &str, detail: Option<String>) {
        let log = self.steps.entry(id.clone()).or_default();
        log.push_back(ConnectionStep { at_ms: now_ms(), step: step.to_owned(), detail });
        while log.len() > STEPS_KEPT {
            log.pop_front();
        }
    }

    fn start_target(&mut self, id: &TargetId) {
        let Some(t) = self.targets.get_mut(id) else { return };
        if !t.def.enabled || t.supervisor.is_some() {
            return;
        }
        // SSH targets: the hub's own connector, with YAAC's key when YAAC set the server up.
        let made = match (&t.def.transport, &self.cfg.ssh) {
            (attach::TargetTransport::Ssh(s), Some(ssh)) => {
                let ident = crate::ssh::Ident::from_transport(s);
                let record = self.settings.ssh.iter().find(|e| crate::ssh::Ident::from_params(&e.conn) == ident).map(|e| &e.record);
                Ok(ssh.connector(s, record))
            }
            _ => self.cfg.connectors.connector(&t.def),
        };
        match made {
            Err(message) => {
                let new = state(ConnectionStatus::Offline, Some(message));
                self.set_state(id, new);
            }
            Ok(connector) => {
                t.daemon_build = connector.daemon_build();
                let sup = self.supervisors.fetch_add(1, Ordering::Relaxed) + 1;
                let notify = Arc::new(crate::conn::Wake::default());
                let core = self.tx.clone();
                let (target, name) = (id.clone(), t.def.name.as_str().to_owned());
                let backoff = self.cfg.backoff;
                let retry = notify.clone();
                // Events carry the supervisor's id, so a stopped supervisor's last events can
                // never be mistaken for the current connection's.
                let handle = tokio::spawn(supervise(sup, target, name, connector, backoff, retry, core));
                t.supervisor = Some((sup, handle, notify));
            }
        }
    }

    /// **Start it** (D96): connect a target whose machine is not running, booting it. Answered
    /// when the target ends connected or the attempt fails ([`Core::settle_starts`]).
    fn targets_start(&mut self, client: ClientKey, rid: &RpcId, id: &TargetId) -> Option<Result<Value, ErrorObject>> {
        let Some(t) = self.targets.get(id) else { return Some(Err(err(ErrorCode::TargetUnknown, "no such target"))) };
        if !t.def.enabled {
            return Some(Err(hub_err(ext::codes::START_FAILED, "the target is disabled; enable it first")));
        }
        if t.state.status == ConnectionStatus::Connected {
            return Some(Ok(json!({})));
        }
        let Some((_, _, wake)) = &t.supervisor else {
            let why = t.state.detail.clone().unwrap_or_else(|| "the target cannot be connected".into());
            return Some(Err(hub_err(ext::codes::START_FAILED, why)));
        };
        wake.start();
        self.starts.entry(id.clone()).or_default().push((client, rid.clone()));
        self.set_state(id, state(ConnectionStatus::Connecting, None));
        None
    }

    /// `hub/wsl/distros`: lists the installed distros on a task (it runs `wsl.exe`, which
    /// starts nothing) and answers when it is done.
    fn wsl_distros(&self, client: ClientKey, id: &RpcId) {
        let lister = self.cfg.wsl_distros.clone();
        let core = self.tx.clone();
        let id = id.clone();
        tokio::spawn(async move {
            let outcome = match lister.list().await {
                Ok(list) => {
                    let distros = list
                        .into_iter()
                        .map(|d| ext::WslDistroEntry { name: d.name, default: d.default, running: d.running, version: d.version })
                        .collect();
                    Ok(to_value(&ext::WslDistrosResult { distros }))
                }
                Err(yaac_transport::wsl::ListError::Unavailable(_)) => {
                    Err(hub_err(ext::codes::WSL_UNAVAILABLE, "WSL is not installed on this PC"))
                }
                Err(yaac_transport::wsl::ListError::Failed(m)) => Err(hub_err(ext::codes::WSL_FAILED, m)),
            };
            let _ = core.send(CoreEvent::Reply { client, id, outcome });
        });
    }

    /// Answers waiting `hub/targets/start` requests once the target's state says how the
    /// attempt ended. `stopped` does not end one: a check that was already under way when the
    /// user chose Start it may still report the machine down; the starting attempt follows.
    fn settle_starts(&mut self, id: &TargetId, new: &ConnectionState) {
        let outcome = match new.status {
            ConnectionStatus::Connected => Ok(json!({})),
            ConnectionStatus::Offline | ConnectionStatus::Incompatible | ConnectionStatus::Disabled => {
                Err(hub_err(ext::codes::START_FAILED, new.detail.clone().unwrap_or_else(|| "the target did not connect".into())))
            }
            _ => return,
        };
        for (client, rid) in self.starts.remove(id).unwrap_or_default() {
            self.reply(client, rid, outcome.clone());
        }
    }

    fn stop_target(&mut self, id: &TargetId, new: ConnectionState) {
        self.handovers.remove(id);
        let Some(t) = self.targets.get_mut(id) else { return };
        if let Some((_, handle, _)) = t.supervisor.take() {
            handle.abort();
        }
        if t.conn.is_some() {
            self.on_closed(id, "the target was disabled".into());
        }
        self.set_state(id, new);
    }

    fn targets_add(&mut self, p: TargetsAddParams) -> Result<Value, ErrorObject> {
        let name = p.name.as_str().trim();
        if name.is_empty() || !self.settings.name_is_free(name, None) {
            return Err(hub_err(ext::codes::NAME_TAKEN, format!("a target named {name:?} already exists, or the name is empty")));
        }
        let def = TargetDef {
            id: TargetId::new(new_id("t")),
            name: TargetName::new(name),
            transport: p.transport,
            enabled: p.enabled,
            extra: serde_json::Map::new(),
        };
        self.settings.targets.push(def.clone());
        if let Err(e) = self.save_settings() {
            self.settings.targets.pop();
            return Err(e);
        }
        let id = def.id.clone();
        let initial = if def.enabled { state(ConnectionStatus::Connecting, None) } else { state(ConnectionStatus::Disabled, None) };
        self.targets.insert(id.clone(), Target { def, state: initial, supervisor: None, conn: None, daemon_build: None });
        self.start_target(&id);
        self.targets_changed();
        Ok(to_value(&TargetResult { target: self.targets[&id].entry() }))
    }

    fn targets_update(&mut self, p: TargetsUpdateParams) -> Result<Value, ErrorObject> {
        let index = self.settings.targets.iter().position(|t| t.id == p.id).ok_or_else(|| err(ErrorCode::TargetUnknown, "no such target"))?;
        if let Some(name) = &p.name {
            let name = name.as_str().trim();
            if name.is_empty() || !self.settings.name_is_free(name, Some(&p.id)) {
                return Err(hub_err(ext::codes::NAME_TAKEN, format!("a target named {name:?} already exists, or the name is empty")));
            }
        }
        let before = self.settings.targets[index].clone();
        if let Some(name) = &p.name {
            self.settings.targets[index].name = TargetName::new(name.as_str().trim());
        }
        let transport_changed = p.transport.as_ref().is_some_and(|t| *t != before.transport);
        if let Some(transport) = p.transport {
            self.settings.targets[index].transport = transport;
        }
        if let Err(e) = self.save_settings() {
            self.settings.targets[index] = before;
            return Err(e);
        }
        let def = self.settings.targets[index].clone();
        if let Some(t) = self.targets.get_mut(&p.id) {
            t.def = def.clone();
        }
        if transport_changed && def.enabled {
            self.stop_target(&p.id, state(ConnectionStatus::Connecting, None));
            self.start_target(&p.id);
        }
        self.targets_changed();
        Ok(to_value(&TargetResult { target: self.targets[&p.id].entry() }))
    }

    fn targets_enable(&mut self, p: &TargetsEnableParams) -> Result<Value, ErrorObject> {
        let index = self.settings.targets.iter().position(|t| t.id == p.id).ok_or_else(|| err(ErrorCode::TargetUnknown, "no such target"))?;
        if self.settings.targets[index].enabled != p.enabled {
            self.settings.targets[index].enabled = p.enabled;
            if let Err(e) = self.save_settings() {
                self.settings.targets[index].enabled = !p.enabled;
                return Err(e);
            }
            if let Some(t) = self.targets.get_mut(&p.id) {
                t.def.enabled = p.enabled;
            }
            if p.enabled {
                self.set_state(&p.id, state(ConnectionStatus::Connecting, None));
                self.start_target(&p.id);
            } else {
                self.stop_target(&p.id, state(ConnectionStatus::Disabled, None));
            }
            self.targets_changed();
        }
        Ok(to_value(&TargetResult { target: self.targets[&p.id].entry() }))
    }

    fn targets_remove(&mut self, p: &TargetsRemoveParams) -> Result<Value, ErrorObject> {
        let index = self.settings.targets.iter().position(|t| t.id == p.id).ok_or_else(|| err(ErrorCode::TargetUnknown, "no such target"))?;
        let removed = self.settings.targets.remove(index);
        if let Err(e) = self.save_settings() {
            self.settings.targets.insert(index, removed);
            return Err(e);
        }
        self.stop_target(&p.id, state(ConnectionStatus::Disabled, None));
        self.preview_removed(&p.id);
        self.targets.remove(&p.id);
        let id = p.id.clone();
        self.notifications.remove_target(&id);
        self.open.retain(|(t, _), _| *t != id);
        for c in self.clients.values_mut() {
            c.subs.retain(|(t, _)| *t != id);
        }
        self.applied.retain(|(t, _), _| *t != id);
        self.committed.retain(|(t, _), _| *t != id);
        self.thread_states.retain(|(t, _), _| *t != id);
        self.tokens.remove(&id);
        self.steps.remove(&id);
        self.kept.remove(&id);
        self.limits_forget(&id);
        self.worker.submit(move |stores| {
            if let Err(e) = stores.cache.remove_target(&id) {
                return Some(CoreEvent::StoreError(e.to_string()));
            }
            if let Err(e) = stores.drafts.view_remove_target(&id) {
                return Some(CoreEvent::StoreError(e.to_string()));
            }
            // Messages that were waiting for it fail; their senders are told.
            let receipts = stores.relay.fail_for_removed(&id).unwrap_or_default();
            let in_flight = stores.relay.in_flight().unwrap_or(0);
            Some(CoreEvent::Receipts { receipts, in_flight })
        });
        self.targets_changed();
        Ok(json!({}))
    }

    // ── Connections ──────────────────────────────────────────────────────────────────────

    fn on_conn(&mut self, id: &TargetId, supervisor: u64, generation: u64, event: ConnEvent) {
        let Some(t) = self.targets.get_mut(id) else { return };
        if t.supervisor.as_ref().map(|(s, _, _)| *s) != Some(supervisor) {
            return; // a stopped supervisor's last words
        }
        match event {
            ConnEvent::Connecting => {
                let quiet = t.conn.is_some();
                if !quiet {
                    self.step(id, "connecting", None);
                }
                let Some(t) = self.targets.get_mut(id) else { return };
                // A stopped target stays shown as stopped while it is checked again.
                if t.conn.is_none() && !matches!(t.state.status, ConnectionStatus::Offline | ConnectionStatus::Stopped) {
                    self.set_state(id, state(ConnectionStatus::Connecting, None));
                }
            }
            ConnEvent::Failed(reason) => {
                log(format!("{id}: connect failed: {reason}"));
                self.step(id, "failed", Some(reason.clone()));
                self.set_state(id, state(ConnectionStatus::Offline, Some(reason)));
            }
            ConnEvent::Down(reason) => {
                // D35, D96: "WSL distro Ubuntu is not running". The target stays enabled and
                // its threads unusable; it connects as soon as the distro runs, or when the
                // user chooses Start it (`hub/targets/start`). The hub never starts it.
                self.set_state(id, state(ConnectionStatus::Stopped, Some(reason)));
            }
            ConnEvent::Opened { writer, kill } => {
                let client = ClientInfo {
                    name: "yaac".into(),
                    version: self.cfg.build.clone(),
                    host: Some(self.cfg.host_name.clone()),
                    id: self.settings.client_id.clone(),
                };
                let now = Instant::now();
                t.conn = Some(Conn {
                    generation,
                    session: ClientSession::new(client),
                    writer,
                    writer_busy: false,
                    kill: Some(kill),
                    ready: false,
                    modes_during_reconnect: Some(HashSet::new()),
                    pending: HashMap::new(),
                    terminal_streams: HashMap::new(),
                    voice_streams: HashMap::new(),
                    transcribers: Vec::new(),
                    preview_streams: HashMap::new(),
                    attach_tunnels: HashMap::new(),
                    subs: HashMap::new(),
                    delivering: HashSet::new(),
                    last_rx: now,
                    last_ping: now,
                    sync_full: false,
                });
                self.step(id, "opened", None);
                self.flush(id);
            }
            ConnEvent::Bytes(bytes) => {
                let Some(conn) = t.conn.as_mut().filter(|c| c.generation == generation) else { return };
                conn.last_rx = Instant::now();
                match conn.session.receive(&bytes) {
                    Ok(events) => {
                        for event in events {
                            self.on_session(id, event);
                        }
                    }
                    Err(e) => {
                        log(format!("{id}: protocol error, dropping the connection: {e}"));
                        if let Some(kill) = self.targets.get_mut(id).and_then(|t| t.conn.as_mut()).and_then(|c| c.kill.take()) {
                            let _ = kill.send(());
                        }
                    }
                }
                self.flush(id);
            }
            ConnEvent::Written { bytes, elapsed, bulk } => {
                let Some(conn) = t.conn.as_mut().filter(|c| c.generation == generation) else { return };
                conn.writer_busy = false;
                if bulk {
                    conn.session.on_written(bytes, elapsed);
                }
                self.flush(id);
            }
            ConnEvent::Closed(reason) => {
                if t.conn.as_ref().is_some_and(|c| c.generation == generation) {
                    log(format!("{id}: connection closed: {reason}"));
                    if let Some(h) = self.handovers.get_mut(id) {
                        h.closed = true;
                    }
                    self.step(id, "closed", Some(reason.clone()));
                    self.on_closed(id, reason.clone());
                    if self.targets.get(id).is_some_and(|t| t.def.enabled && t.supervisor.is_some()) {
                        self.set_state(id, state(ConnectionStatus::Offline, Some(reason)));
                    }
                }
            }
        }
    }

    /// Drops the connection's state: pending requests fail, subscriptions end (the threads
    /// stay open and are resubscribed on reconnect).
    fn on_closed(&mut self, id: &TargetId, _reason: String) {
        let Some(conn) = self.targets.get_mut(id).and_then(|t| t.conn.take()) else { return };
        for stream in conn.terminal_streams.values() {
            self.terminals.streams.remove(stream);
        }
        self.preview_disconnected(id);
        self.attach_tunnels_target_gone(id);
        self.voice_target_closed(id);
        self.voice_call_target_closed(id);
        self.browser_target_closed(id);
        let was_ready = conn.ready;
        // Batched events of this connection are still valid: commit them.
        self.flush_batches();
        for (_, pending) in conn.pending {
            match pending {
                Pending::Routed { client, id: rid, method, op } => {
                    let error = if let Some(op) = op {
                        self.worker.submit(move |s| s.drafts.op_uncertain(&op).err().map(|e| CoreEvent::StoreError(e.to_string())));
                        err(ErrorCode::OutcomeUnknown, format!("the connection dropped before {method} was answered; it may or may not have taken effect"))
                    } else {
                        err(ErrorCode::TargetOffline, "the connection to the target dropped")
                    };
                    if client == NOTIFICATION_CLIENT { self.notification_answer(&rid, Some(error)); }
                    else { self.reply(client, rid, Err(error)); }
                }
                Pending::Mode { op, thread } => {
                    self.broadcast(&Message::notification("hub/modeChange/result", json!({ "target": id, "thread": thread, "error": "Connection lost. The mode change outcome is unknown." })));
                    self.worker.submit(move |s| s.drafts.op_uncertain(&op).err().map(|e| CoreEvent::StoreError(e.to_string())));
                }
                Pending::UploadPreview(read) => self.close_preview_read(read),
                Pending::Terminal { client, id, .. } => self.reply(client, id, Err(err(ErrorCode::OutcomeUnknown, "target disconnected before answering terminal request"))),
                Pending::Voice { ask: voice::VoiceAsk::Quiet, .. } => {}
                Pending::Voice { client, id, ask: voice::VoiceAsk::Start(_) } => {
                    self.reply(client, id, Err(err(ErrorCode::OutcomeUnknown, "the connection dropped before the dictation started; it may have started and ended")));
                }
                Pending::Voice { client, id, ask: voice::VoiceAsk::Stream } => self.reply(client, id, Err(err(ErrorCode::TargetOffline, "the connection to the target dropped"))),
                Pending::VoiceCall { ask: voice_call::CallAsk::Quiet, .. } => {}
                Pending::VoiceCall { client, id, ask: voice_call::CallAsk::Start } => {
                    self.reply(client, id, Err(err(ErrorCode::OutcomeUnknown, "the connection dropped before the call started; it may have started and ended")));
                }
                Pending::VoiceCall { client, id, ask: voice_call::CallAsk::End } => self.reply(client, id, Err(err(ErrorCode::TargetOffline, "the connection to the target dropped"))),
                Pending::MeshQuery { source, generation, peer_id } => {
                    self.respond_peer(&source, generation, peer_id, names::MESH_QUERY, Err(err(ErrorCode::TargetOffline, "the target went offline")));
                }
                Pending::Lookup { lookup } => self.lookup_answered(lookup, id, None),
                Pending::Handover { reply: Some((client, rid)) } => {
                    // Never asked again: the update shows how it went once the target is back.
                    let e = err(ErrorCode::OutcomeUnknown, "the connection dropped before yaacd answered; it may be handing over");
                    self.reply(client, rid, Err(e));
                }
                _ => {}
            }
        }
        for (_, sub) in conn.subs {
            for (client, rid, _) in sub.waiters {
                self.reply(client, rid, Err(err(ErrorCode::TargetOffline, "the connection to the target dropped")));
            }
        }
        self.uploads_on_closed(id);
        if was_ready {
            self.send_rosters();
        }
    }

    /// Select one frame when the writer becomes available. An interactive frame arriving
    /// mid-write can take the next slot even when there is a backlog of bulk frames.
    fn flush(&mut self, id: &TargetId) {
        if !self.uploads.is_empty() {
            self.upload_pump(id);
        }
        let Some(conn) = self.targets.get_mut(id).and_then(|t| t.conn.as_mut()) else { return };
        self.previews.grant_reads(conn);
        loop {
            let scheduler = conn.session.scheduler();
            // Preserve the immediate control path on targets without active terminals.
            // Once an interactive stream exists, pick again at every frame boundary.
            let paced = !conn.terminal_streams.is_empty() || scheduler.pending_control() == 0;
            if paced && conn.writer_busy { break; }
            let bulk = scheduler.pending_control() == 0 && !scheduler.interactive_ready();
            let Some(bytes) = conn.session.poll_transmit() else { break };
            if conn.writer.send(Outgoing { bytes, bulk, paced }).is_err() { break; }
            if paced {
                conn.writer_busy = true;
                self.previews.grant_reads(conn);
            }
        }
    }

    fn on_session(&mut self, id: &TargetId, event: SessionEvent) {
        match event {
            SessionEvent::Ready(info) => {
                self.step(id, "negotiated", Some(format!("yaacd {}{}", info.daemon.build, if info.daemon_is_older { " (older)" } else { "" })));
                self.on_ready(id, &info);
            }
            SessionEvent::NegotiationFailed { error, .. } => {
                self.set_state(id, state(ConnectionStatus::Incompatible, Some(error.message)));
            }
            SessionEvent::Response { id: rid, method, outcome } => {
                let pending = self.targets.get_mut(id).and_then(|t| t.conn.as_mut()).and_then(|c| c.pending.remove(&rid));
                if let Some(pending) = pending {
                    self.on_response(id, pending, method, outcome);
                }
            }
            SessionEvent::Notification(inbound) => self.on_inbound(id, inbound),
            SessionEvent::PeerRequest { id: peer_id, request, .. } => self.on_peer_request(id, peer_id, request),
            SessionEvent::Data { stream, bytes } => self.target_data(id, stream, bytes),
            SessionEvent::OrphanError(e) => log(format!("{id}: daemon error without id: {}", e.code)),
        }
    }

    fn send<M: methods::Method>(&mut self, id: &TargetId, params: &M::Params, pending: Pending) -> bool {
        let Some(conn) = self.targets.get_mut(id).and_then(Target::ready_conn) else { return false };
        match conn.session.request::<M>(params) {
            Ok(rid) => {
                conn.pending.insert(rid, pending);
                true
            }
            Err(e) => {
                log(format!("{id}: cannot send {}: {e}", M::NAME));
                false
            }
        }
    }

    fn on_ready(&mut self, id: &TargetId, info: &ReadyInfo) {
        let Some(t) = self.targets.get_mut(id) else { return };
        let Some(conn) = t.conn.as_mut() else { return };
        conn.ready = true;
        let generation = conn.generation;
        let terminal = conn.session.features().is_some_and(|f| f.allows(&yaac_protocol::Capability::Terminal));
        let file_view = conn.session.features().is_some_and(|f| f.allows(&yaac_protocol::Capability::FileView));
        let budget_time_limit = conn.session.features().is_some_and(|f| f.allows(&yaac_protocol::Capability::BudgetTimeLimit));
        let port_forward = self.preview_check(id).is_ok();
        let can_update = self.handover_on_ready(id, &info.daemon.build);
        let new = ConnectionState {
            status: ConnectionStatus::Connected,
            detail: None,
            daemon: Some(DaemonSummary {
                build: info.daemon.build.clone(),
                // Older by protocol, or by build: either way it needs updating.
                older: can_update || info.daemon_is_older || daemon_build_older(&info.daemon.build, &self.cfg.build),
                schema_hash_matches: info.schema_hash_matches,
                can_update,
                port_forward,
                file_view,
                terminal,
                budget_time_limit,
                terminal_unavailable_reason: info.terminal_unavailable_reason.clone(),
            }),
            update: None,
        };
        let facts = info.target.clone();
        let target = id.clone();
        let stored = new.clone();
        self.worker.submit(move |s| s.cache.put_target(&target, Some(&facts), &stored).err().map(|e| CoreEvent::StoreError(e.to_string())));
        self.set_state(id, new);
        self.send_kept(id);
        let dest = id.clone();
        self.worker.submit(move |s| Some(match s.drafts.claim_modes(&dest) {
            Ok(changes) => CoreEvent::PendingModes(dest, generation, changes),
            Err(e) => CoreEvent::StoreError(e.to_string()),
        }));
        if self.cfg.notifier.is_some() {
            // An incremental index sync need not repeat threads still blocked since last exit.
            for (key, state) in &self.thread_states {
                if &key.0 == id && *state == ThreadState::NeedsAttention {
                    self.open.entry(key.clone()).or_default().insert(NOTIFICATION_CLIENT);
                }
            }
        }
        self.refresh_target(id);
        self.send_rosters();
        // The relay: deliver what waited for this target, and hand back receipts owed to it.
        let target = id.clone();
        self.worker.submit(move |s| {
            let messages = s.relay.queued_for(&target).unwrap_or_default();
            let in_flight = s.relay.in_flight().unwrap_or(0);
            Some(CoreEvent::Deliver { dest: target, messages, in_flight })
        });
        let target = id.clone();
        self.worker.submit(move |s| {
            let receipts = s.relay.pending_receipts(&target).unwrap_or_default();
            let in_flight = s.relay.in_flight().unwrap_or(0);
            let receipts = receipts.into_iter().map(|(m, r)| (target.clone(), m, r)).collect();
            Some(CoreEvent::Receipts { receipts, in_flight })
        });
    }

    // ── Updating an older daemon (docs/02-architecture.md#one-serving-daemon-per-user) ──────

    /// A connection is ready with `serving` serving: settles a hand-over asked earlier, and
    /// asks a newer-able daemon to hand over when idle — once per pair of builds. Returns
    /// whether this hub can update the daemon ([`DaemonSummary::can_update`]).
    fn handover_on_ready(&mut self, id: &TargetId, serving: &str) -> bool {
        let to = self.targets.get(id).and_then(|t| t.daemon_build.clone());
        let newer = hands_over_to(serving, to.as_deref());
        if let Some(h) = self.handovers.get_mut(id) {
            if h.from == serving {
                // The same older build serves again after a hand-over was asked: stop here.
                if h.closed && h.stuck.is_none() {
                    let why = match h.answer {
                        Some(HandoverState::Scheduled) => format!(
                            "yaacd {} still serves after it was asked to hand over to {} when idle. If the connection only dropped, it still hands over once idle; YAAC does not ask again.",
                            h.from, h.to
                        ),
                        _ => format!("yaacd {} started again after handing over to {}. YAAC does not ask again; see the target's log (diagnostics).", h.from, h.to),
                    };
                    log(format!("{id}: {why}"));
                    h.stuck = Some(why.clone());
                    self.step(id, "update stuck", Some(why));
                }
                return newer;
            }
            // Another build serves now: the update went in (or something else replaced it).
            let done = self.handovers.remove(id).map(|h| h.from).unwrap_or_default();
            self.step(id, "updated", Some(format!("yaacd {done} -> {serving}")));
        }
        if newer && let Some(to) = to {
            self.ask_handover(id, serving.to_owned(), to, false, None);
        }
        newer
    }

    /// Sends `daemon/handover` (`now` when `forced`) and remembers it. False when it could not
    /// be sent.
    fn ask_handover(&mut self, id: &TargetId, from: String, to: String, forced: bool, reply: Option<(ClientKey, RpcId)>) -> bool {
        let when = if forced { HandoverWhen::Now } else { HandoverWhen::Idle };
        if !self.send::<DaemonHandover>(id, &DaemonHandoverParams { when }, Pending::Handover { reply }) {
            return false;
        }
        let how = if forced { "now" } else { "when idle" };
        self.step(id, "handover", Some(format!("asked yaacd {from} to hand over {how}, to {to}")));
        self.handovers.insert(id.clone(), Handover { from, to, forced, answer: None, closed: false, stuck: None });
        self.flush(id);
        true
    }

    /// The daemon answered `daemon/handover`.
    fn on_handover_answer(&mut self, id: &TargetId, reply: Option<(ClientKey, RpcId)>, outcome: Result<Value, ErrorObject>) {
        let outcome = outcome.and_then(|w| self.decode_typed::<DaemonHandover>(id, w));
        let answered = match &outcome {
            Ok(r) => {
                if let Some(h) = self.handovers.get_mut(id) {
                    h.answer = Some(r.state.clone());
                }
                self.step(id, "handover", Some(format!("yaacd answered {}", r.state.as_str())));
                Ok(to_value(&TargetsUpdateDaemonResult { state: r.state.clone() }))
            }
            Err(e) => {
                // Refused: not asked again for this pair; the window shows why.
                if let Some(h) = self.handovers.get_mut(id) {
                    h.stuck = Some(format!("yaacd {} refused to hand over: {}", h.from, e.message));
                }
                self.step(id, "handover", Some(format!("refused: {}", e.message)));
                Err(e.clone())
            }
        };
        if let Some((client, rid)) = reply {
            self.reply(client, rid, answered);
        }
        self.refresh_state(id);
    }

    /// **Update now** (`hub/targets/updateDaemon`): a forced hand-over, the user's explicit
    /// action — allowed after a stuck one too, since nothing is retried behind their back.
    fn targets_update_daemon(&mut self, client: ClientKey, rid: &RpcId, id: &TargetId) -> Option<Result<Value, ErrorObject>> {
        let Some(t) = self.targets.get(id) else { return Some(Err(err(ErrorCode::TargetUnknown, "no such target"))) };
        let name = t.def.name.as_str().to_owned();
        let daemon = t.state.daemon.as_ref().filter(|_| t.state.status == ConnectionStatus::Connected && t.conn.as_ref().is_some_and(|c| c.ready));
        let Some(daemon) = daemon else { return Some(Err(err(ErrorCode::TargetOffline, format!("{name} is not connected")))) };
        let (from, to) = (daemon.build.clone(), t.daemon_build.clone());
        let Some(to) = to.filter(|to| hands_over_to(&from, Some(to))) else {
            return Some(Err(hub_err(ext::codes::UPDATE_UNAVAILABLE, format!("this YAAC has no newer yaacd for {name} than {from}"))));
        };
        if self.handovers.get(id).is_some_and(|h| h.forced && h.stuck.is_none() && h.from == from) {
            // Already asked to hand over now: the same answer, nothing sent twice.
            return Some(Ok(to_value(&TargetsUpdateDaemonResult { state: HandoverState::Exiting })));
        }
        self.handovers.remove(id);
        if !self.ask_handover(id, from, to, true, Some((client, rid.clone()))) {
            return Some(Err(err(ErrorCode::TargetOffline, format!("{name} is not connected"))));
        }
        self.refresh_state(id);
        None
    }

    /// Broadcasts a target's state again (its update changed).
    fn refresh_state(&mut self, id: &TargetId) {
        if let Some(s) = self.targets.get(id).map(|t| t.state.clone()) {
            self.set_state(id, s);
        }
    }

    /// Providers, the thread index, and every open thread, from the stored positions.
    fn refresh_target(&mut self, id: &TargetId) {
        self.refresh_target_with(id, false);
    }

    fn refresh_target_with(&mut self, id: &TargetId, fresh: bool) {
        self.send::<ProviderList>(id, &yaac_protocol::Empty {}, Pending::ProviderList);
        self.send::<ProjectList>(id, &yaac_protocol::ProjectListParams {}, Pending::ProjectList);
        self.send::<DiscoveryCapsMethod>(id, &yaac_protocol::DiscoveryCapsParams::default(), Pending::Discovery);
        let token = self.tokens.get(id).cloned().unwrap_or_default();
        self.send::<ThreadSync>(id, &ThreadSyncParams { token, max_bytes: Some(256 * 1024) }, Pending::Sync);
        let open: Vec<ThreadId> = self.open.keys().filter(|(t, _)| t == id).map(|(_, th)| th.clone()).collect();
        for thread in open {
            let watched = self.open.get(&(id.clone(), thread.clone())).is_some_and(|h| h.contains(&NOTIFICATION_CLIENT));
            self.start_open(id, &thread, fresh || watched);
        }
    }

    fn send_rosters(&mut self) {
        let entries: Vec<RosterEntry> = self
            .settings
            .targets
            .iter()
            .map(|d| RosterEntry {
                id: d.id.clone(),
                name: d.name.clone(),
                reachable: self.targets.get(&d.id).is_some_and(|t| t.conn.as_ref().is_some_and(|c| c.ready)),
            })
            .collect();
        let ready: Vec<(TargetId, TargetName)> =
            self.targets.values().filter(|t| t.conn.as_ref().is_some_and(|c| c.ready)).map(|t| (t.def.id.clone(), t.def.name.clone())).collect();
        for (id, name) in ready {
            let params = MeshRosterParams { self_id: id.clone(), self_name: name, targets: entries.clone() };
            self.send::<MeshRoster>(&id, &params, Pending::Ignore);
            self.flush(&id);
        }
    }

    fn on_response(&mut self, id: &TargetId, pending: Pending, method: &'static str, outcome: Result<Value, ErrorObject>) {
        let outcome = if matches!(method, names::TUNNEL_OPEN | names::PREVIEW_PORTS)
            && outcome.as_ref().is_err_and(|e| e.code == ErrorCode::MethodNotFound)
        {
            if let Some(t) = self.targets.get(id) {
                let mut state = t.state.clone();
                if let Some(daemon) = state.daemon.as_mut() {
                    daemon.port_forward = false;
                    self.set_state(id, state);
                }
            }
            self.preview_check(id).and(outcome)
        } else if method == names::TERMINAL_RENAME && outcome.as_ref().is_err_and(|e| e.code == ErrorCode::MethodNotFound) {
            // An older yaacd without names (P343): terminals still work; the window keeps the
            // name itself.
            Err(err(ErrorCode::CapabilityMissing, "this machine's YAAC cannot name terminals yet"))
        } else if method.starts_with("terminal/") && outcome.as_ref().is_err_and(|e| e.code == ErrorCode::MethodNotFound) {
            if let Some(t) = self.targets.get(id) {
                let mut state = t.state.clone();
                if let Some(daemon) = state.daemon.as_mut() {
                    daemon.terminal = false;
                    daemon.terminal_unavailable_reason = Some(format!("the target does not implement {method}"));
                    self.set_state(id, state);
                }
            }
            Err(err(ErrorCode::CapabilityMissing, "terminal unavailable on this target"))
        } else if matches!(method, names::FS_TREE | names::FS_READ | names::GIT_FILE_DIFF)
            && outcome.as_ref().is_err_and(|e| e.code == ErrorCode::MethodNotFound)
        {
            if let Some(t) = self.targets.get(id) {
                let mut state = t.state.clone();
                if let Some(daemon) = state.daemon.as_mut() {
                    daemon.file_view = false;
                    self.set_state(id, state);
                }
            }
            Err(file_view_missing())
        } else {
            outcome
        };
        match pending {
            Pending::Routed { client, id: rid, method: m, op } => {
                let outcome = outcome.and_then(|wire| self.decode_current(id, m, wire));
                if let Some(op) = op {
                    self.worker.submit(move |s| s.drafts.op_done(&op).err().map(|e| CoreEvent::StoreError(e.to_string())));
                }
                if client == NOTIFICATION_CLIENT {
                    self.notification_answer(&rid, outcome.err());
                    return;
                }
                if methods::request_info(m).is_some_and(|info| info.response_class == yaac_protocol::schedule::Class::Bulk)
                    && let Ok(result) = outcome
                {
                    if let Some(c) = self.clients.get(&client) {
                        let _ = c.tx.bulk_reply(Message::Response { id: rid, result });
                    }
                } else {
                    self.reply(client, rid, outcome);
                }
            }
            Pending::Open { thread, serial, flow } => self.on_open_response(id, thread, serial, *flow, outcome),
            Pending::RequiredRequests { thread, serial } => {
                let page = outcome.and_then(|wire| self.decode_typed::<methods::ThreadRead>(id, wire)).ok();
                if let Some(sub) = self.sub_mut(id, &thread).filter(|s| s.serial == serial) {
                    if let Some(page) = page {
                        sub.required_requests = page.required_items.unwrap_or_default();
                        sub.required_requests.extend(page.items.into_iter().filter(|i| matches!(i.body, ItemBody::Request(_))));
                    }
                    self.finish_rebuild(id, &thread);
                }
            }
            Pending::Page { thread, serial } => {
                if self.sub_mut(id, &thread).is_some_and(|s| s.serial == serial) {
                    self.on_page(id, &thread, outcome);
                }
            }
            Pending::ProviderList => {
                let Ok(result) = outcome.and_then(|w| self.decode_typed::<ProviderList>(id, w)) else { return };
                let mut providers = result.providers;
                if let Some(conn) = self.targets.get(id).and_then(|t| t.conn.as_ref()) {
                    for p in &mut providers {
                        conn.session.fold_capabilities(&mut p.capabilities);
                    }
                }
                self.limits_providers(id, &providers);
                self.voice_providers(id, &providers, true);
                let target = id.clone();
                self.worker.submit(move |s| match s.cache.replace_providers(&target, &providers) {
                    Ok(()) => Some(CoreEvent::Broadcast(Message::notification(ext::names::PROVIDERS_CHANGED, json!({ "target": target })))),
                    Err(e) => Some(CoreEvent::StoreError(e.to_string())),
                });
            }
            Pending::Sync => {
                let Ok(result) = outcome.and_then(|w| self.decode_typed::<ThreadSync>(id, w)) else { return };
                self.on_sync(id, result);
            }
            Pending::MeshQuery { source, generation, peer_id } => {
                let outcome = outcome.and_then(|wire| self.decode_current(id, method, wire));
                self.respond_peer(&source, generation, peer_id, names::MESH_QUERY, outcome);
            }
            Pending::PeerForward { source, generation, peer_id, method: m } => {
                let outcome = outcome.and_then(|wire| self.decode_current(id, m, wire));
                self.respond_peer(&source, generation, peer_id, m, outcome);
            }
            Pending::ProjectList => {
                let Ok(result) = outcome.and_then(|w| self.decode_typed::<ProjectList>(id, w)) else { return };
                let target = id.clone();
                self.worker.submit(move |s| match s.cache.replace_projects(&target, &result.projects) {
                    Ok(()) => Some(CoreEvent::Broadcast(Message::notification(ext::names::PROJECTS_CHANGED, json!({ "target": target })))),
                    Err(e) => Some(CoreEvent::StoreError(e.to_string())),
                });
            }
            Pending::Discovery => {
                let Ok(result) = outcome.and_then(|w| self.decode_typed::<DiscoveryCapsMethod>(id, w)) else { return };
                self.store_discovery(id, result.caps);
            }
            Pending::Lookup { lookup } => {
                let found = outcome.and_then(|w| self.decode_typed::<ThreadLookup>(id, w)).ok().map(|r| r.threads);
                self.lookup_answered(lookup, id, found);
            }
            Pending::Handover { reply } => self.on_handover_answer(id, reply, outcome),
            Pending::Mode { op, thread } => {
                if let Err(e) = outcome {
                    self.broadcast(&Message::notification("hub/modeChange/result", json!({ "target": id, "thread": thread, "error": e.message })));
                }
                self.worker.submit(move |s| s.drafts.op_done(&op).err().map(|e| CoreEvent::StoreError(e.to_string())));
            }
            Pending::RateLimits => self.limits_answered(id, outcome),
            Pending::Deliver { message_id } => self.on_delivered(id, message_id, outcome),
            Pending::UploadBegin { handle } => self.on_upload_begun(id, &handle, outcome),
            Pending::UploadEnd { handle } => self.on_upload_ended(id, &handle, outcome),
            Pending::UploadCancel { handle } => self.on_upload_cancelled(&handle),
            Pending::UploadPreview(read) => self.on_preview_chunk(id, read, outcome),
            Pending::Terminal { client, id: rid, terminal } => self.terminal_response(id, client, rid, method, terminal, outcome),
            Pending::Voice { client, id: rid, ask } => self.voice_response(id, client, rid, method, &ask, outcome),
            Pending::VoiceCall { client, id: rid, ask } => self.voice_call_response(id, client, rid, method, ask, outcome),
            Pending::PreviewOpen { socket } => self.preview_opened(id, socket, outcome),
            Pending::AttachTunnel { client, id: rid } => self.attach_tunnel_opened(id, client, rid, outcome),
            Pending::Receipt { message_id, receipt } => {
                if outcome.is_ok() {
                    self.worker.submit(move |s| {
                        let _ = s.relay.receipt_sent(&message_id, &receipt);
                        Some(CoreEvent::InFlight(s.relay.in_flight().unwrap_or(0)))
                    });
                }
            }
            Pending::Ignore => {}
        }
    }

    fn decode_current(&self, id: &TargetId, method: &str, wire: Value) -> Result<Value, ErrorObject> {
        if methods::allowed_before_initialize(method) {
            return Ok(wire);
        }
        let dialect = self.targets.get(id).and_then(|t| t.conn.as_ref()).and_then(|c| c.session.dialect());
        let dialect = dialect.ok_or_else(|| err(ErrorCode::TargetOffline, "the target is not connected"))?;
        dialect.decode_result(method, wire).map_err(|e| err(ErrorCode::InternalError, e.to_string()))
    }

    fn decode_typed<M: methods::Method>(&self, id: &TargetId, wire: Value) -> Result<M::Result, ErrorObject> {
        let conn = self.targets.get(id).and_then(|t| t.conn.as_ref()).ok_or_else(|| err(ErrorCode::TargetOffline, "offline"))?;
        conn.session.decode::<M>(wire).map_err(|e| err(ErrorCode::InternalError, e.to_string()))
    }

    fn on_sync(&mut self, id: &TargetId, result: ThreadSyncResult) {
        let Some(conn) = self.targets.get_mut(id).and_then(|t| t.conn.as_mut()) else { return };
        let first_full = result.full && !conn.sync_full;
        conn.sync_full = result.full && result.more;
        if first_full {
            self.thread_states.retain(|(t, _), _| t != id);
        }
        for summary in &result.changed {
            self.note_state(id, summary);
        }
        for removed in &result.removed {
            self.thread_states.remove(&(id.clone(), removed.clone()));
            self.view_thread_removed(id, removed);
        }
        self.tokens.insert(id.clone(), result.token.clone());
        if result.more {
            let params = ThreadSyncParams { token: result.token.clone(), max_bytes: Some(256 * 1024) };
            self.send::<ThreadSync>(id, &params, Pending::Sync);
        }
        let target = id.clone();
        self.worker.submit(move |s| match s.cache.apply_index(&target, &result.changed, &result.removed, &result.token, first_full) {
            Ok(()) => Some(CoreEvent::Broadcast(Message::notification(ext::names::INDEX_CHANGED, json!({ "target": target })))),
            Err(e) => Some(CoreEvent::StoreError(e.to_string())),
        });
    }

    fn on_inbound(&mut self, id: &TargetId, inbound: Inbound) {
        match inbound {
            Inbound::ThreadEvent(event) => self.on_thread_event(id, event),
            Inbound::ThreadUpdated { thread, token } => {
                self.note_state(id, &thread);
                self.tokens.insert(id.clone(), token.clone());
                let target = id.clone();
                self.worker.submit(move |s| match s.cache.apply_index(&target, std::slice::from_ref(&thread), &[], &token, false) {
                    Ok(()) => Some(CoreEvent::Broadcast(Self::routed_notification(
                        &target,
                        names::THREAD_UPDATED,
                        to_value(&ThreadUpdatedParams { thread, token }),
                    ))),
                    Err(e) => Some(CoreEvent::StoreError(e.to_string())),
                });
            }
            Inbound::ThreadRemoved(p) => {
                self.thread_states.remove(&(id.clone(), p.thread_id.clone()));
                self.view_thread_removed(id, &p.thread_id);
                self.tokens.insert(id.clone(), p.token.clone());
                let target = id.clone();
                self.worker.submit(move |s| match s.cache.apply_index(&target, &[], std::slice::from_ref(&p.thread_id), &p.token, false) {
                    Ok(()) => Some(CoreEvent::Broadcast(Self::routed_notification(&target, names::THREAD_REMOVED, to_value(&p)))),
                    Err(e) => Some(CoreEvent::StoreError(e.to_string())),
                });
            }
            Inbound::ProviderUpdated(mut p) => {
                if let Some(conn) = self.targets.get(id).and_then(|t| t.conn.as_ref()) {
                    conn.session.fold_capabilities(&mut p.provider.capabilities);
                }
                self.voice_providers(id, std::slice::from_ref(&p.provider), false);
                let target = id.clone();
                self.worker.submit(move |s| match s.cache.put_provider(&target, &p.provider) {
                    Ok(()) => Some(CoreEvent::Broadcast(Self::routed_notification(&target, names::PROVIDER_UPDATED, to_value(&p)))),
                    Err(e) => Some(CoreEvent::StoreError(e.to_string())),
                });
            }
            Inbound::ProjectUpdated(p) => {
                let target = id.clone();
                self.worker.submit(move |s| match s.cache.replace_projects(&target, &p.projects) {
                    Ok(()) => Some(CoreEvent::Broadcast(Self::routed_notification(&target, names::PROJECT_UPDATED, to_value(&p)))),
                    Err(e) => Some(CoreEvent::StoreError(e.to_string())),
                });
            }
            Inbound::StreamCredit(p) => {
                if !self.attach_tunnel_target_credit(id, &p) && !self.voice_input_credit(id, &p) {
                    self.terminal_input_credit(id, &p);
                }
            }
            Inbound::Other { method, params } if matches!(method.as_str(), names::VOICE_TRANSCRIPT | names::VOICE_ENDED) => {
                self.voice_event(id, &method, params);
            }
            Inbound::Other { method, params } if matches!(method.as_str(), names::VOICE_CALL_ANSWER | names::VOICE_CALL_STATE | names::VOICE_CALL_TRANSCRIPT) => {
                self.voice_call_event(id, &method, params);
            }
            Inbound::Other { method, params } if matches!(method.as_str(), names::TERMINAL_TITLE | names::TERMINAL_EXITED | names::TERMINAL_RENAMED) => {
                self.terminal_event(id, &method, params);
            }
            Inbound::TunnelClose(p) => {
                if !self.attach_tunnel_remote_closed(id, p.stream) {
                    self.preview_remote_close(id, p.stream);
                }
            }
            Inbound::Other { method, params } if method == names::PROVIDER_RATE_LIMITS_UPDATED => self.limits_updated(id, params),
            Inbound::Other { method, params } if method == names::DISCOVERY_UPDATED => {
                if let Ok(p) = serde_json::from_value::<yaac_protocol::DiscoveryCapsResult>(params.clone()) {
                    self.store_discovery(id, p.caps);
                }
                self.broadcast(&Self::routed_notification(id, &method, params));
            }
            Inbound::Other { method, params } => self.broadcast(&Self::routed_notification(id, &method, params)),
        }
    }

    // ── Threads: subscription, replay, reset ─────────────────────────────────────────────

    /// Subscribes upstream from the stored position (or none, forcing a reset).
    fn start_open(&mut self, id: &TargetId, thread: &ThreadId, fresh: bool) {
        let key = (id.clone(), thread.clone());
        let after = if fresh { None } else { self.applied.get(&key).copied() };
        let cap = self.cfg.rebuild_cap_bytes;
        self.serials += 1;
        let serial = self.serials;
        let Some(conn) = self.targets.get_mut(id).and_then(Target::ready_conn) else { return };
        let waiters = conn.subs.remove(thread).map(|s| s.waiters).unwrap_or_default();
        let command = OpenThread { thread_id: thread.clone(), after, page_bytes: Some(cap.min(256 * 1024)) };
        match OpenThreadFlow::start(&mut conn.session, command.clone()) {
            Ok(flow) => {
                if let Some(rid) = flow.waiting().cloned() {
                    conn.pending.insert(rid, Pending::Open { thread: thread.clone(), serial, flow: Box::new(flow) });
                }
                conn.subs.insert(
                    thread.clone(),
                    Sub {
                        serial,
                        phase: Phase::Subscribing,
                        command,
                        epoch: None,
                        snapshot: Vec::new(),
                        required_requests: Vec::new(),
                        snapshot_bytes: 0,
                        snapshot_position: None,
                        buffered: Vec::new(),
                        subscribed: None,
                        waiters,
                        ready: false,
                        acked: None,
                    },
                );
            }
            Err(e) => log(format!("{id}: cannot subscribe: {e}")),
        }
    }

    fn sub_mut(&mut self, id: &TargetId, thread: &ThreadId) -> Option<&mut Sub> {
        self.targets.get_mut(id)?.conn.as_mut()?.subs.get_mut(thread)
    }

    fn on_open_response(
        &mut self,
        id: &TargetId,
        thread: ThreadId,
        serial: u64,
        mut flow: OpenThreadFlow,
        outcome: Result<Value, ErrorObject>,
    ) {
        let Some(conn) = self.targets.get_mut(id).and_then(|t| t.conn.as_mut()) else { return };
        let current = conn.subs.get(&thread).map(|s| s.serial);
        if current.is_some_and(|s| s != serial) {
            return; // replaced by a newer subscription, which will be answered on its own
        }
        let step = flow.on_response(&mut conn.session, outcome);
        if current.is_none() {
            // Released while subscribing: let go upstream too.
            if matches!(step, Ok(FlowStep::Done(_) | FlowStep::Waiting(_))) {
                let params = ThreadUnsubscribeParams { thread_id: thread };
                self.send::<methods::ThreadUnsubscribe>(id, &params, Pending::Ignore);
            }
            return;
        }
        match step {
            Err(FlowError::Remote(e)) => self.fail_sub(id, &thread, &e),
            Err(other) => self.fail_sub(id, &thread, &err(ErrorCode::InternalError, other.to_string())),
            Ok(FlowStep::Waiting(rid)) => {
                // Subscribed with a reset; the first snapshot page is on its way.
                conn.pending.insert(rid, Pending::Open { thread: thread.clone(), serial, flow: Box::new(flow) });
                if let Some(sub) = conn.subs.get_mut(&thread) {
                    sub.phase = Phase::Reading;
                }
            }
            Ok(FlowStep::Done(Opened { subscribed, snapshot: None })) => self.on_replay(id, &thread, subscribed),
            Ok(FlowStep::Done(Opened { subscribed, snapshot: Some(snapshot) })) => {
                let Some(sub) = self.sub_mut(id, &thread) else { return };
                sub.phase = Phase::Reading;
                sub.epoch = Some(snapshot.position.epoch);
                sub.subscribed = Some(subscribed);
                sub.snapshot_position = Some(snapshot.position);
                sub.snapshot_bytes = serde_json::to_vec(&snapshot.items).map_or(0, |v| v.len() as u64);
                sub.snapshot = snapshot.items;
                self.next_page_or_finish(id, &thread, snapshot.cursor);
            }
        }
    }

    fn on_replay(&mut self, id: &TargetId, thread: &ThreadId, subscribed: ThreadSubscribeResult) {
        let key = (id.clone(), thread.clone());
        let stored = self.applied.get(&key).copied();
        let ok = match &subscribed.mode {
            SubscribeMode::Replay(r) => stored.is_some_and(|p| p.epoch == r.epoch && r.from_seq.0 <= p.seq.0 + 1),
            _ => false,
        };
        if !ok {
            // The daemon offered a replay this cache cannot continue (a gap, another epoch):
            // rebuild from a snapshot instead.
            log(format!("{id}: replay does not continue the cache; rebuilding"));
            let params = ThreadUnsubscribeParams { thread_id: thread.clone() };
            self.send::<methods::ThreadUnsubscribe>(id, &params, Pending::Ignore);
            self.start_open(id, thread, true);
            return;
        }
        let SubscribeMode::Replay(SubscribeReplay { epoch, .. }) = subscribed.mode else { return };
        let live = LiveState::from_subscribed(&subscribed);
        let Some(sub) = self.sub_mut(id, thread) else { return };
        sub.phase = Phase::Live;
        sub.epoch = Some(epoch);
        sub.buffered.clear();
        sub.subscribed = Some(subscribed);
        sub.ready = true;
        let (target, th) = key;
        self.worker.submit(move |s| s.cache.set_live(&target, &th, &live).err().map(|e| CoreEvent::StoreError(e.to_string())));
        self.answer_waiters(id, thread);
    }

    fn next_page_or_finish(&mut self, id: &TargetId, thread: &ThreadId, cursor: Option<PageCursor>) {
        let cap = self.cfg.rebuild_cap_bytes;
        let watching = self.open.get(&(id.clone(), thread.clone())).is_some_and(|h| h.contains(&NOTIFICATION_CLIENT));
        let Some(conn) = self.targets.get_mut(id).and_then(Target::ready_conn) else { return };
        let Some(sub) = conn.subs.get_mut(thread) else { return };
        // A bounded cache rebuild can stop before reaching an old, still-open card.
        // An interactive read returns requiredItems independently of the history page size.
        if watching && cursor.is_some() && sub.snapshot_bytes >= cap {
            let serial = sub.serial;
            let p = ThreadReadParams { thread_id: thread.clone(), cursor: None, max_bytes: Some(4096), max_items: Some(1), around: None, whole_turns: None };
            if self.send::<methods::ThreadRead>(id, &p, Pending::RequiredRequests { thread: thread.clone(), serial }) { return; }
            self.finish_rebuild(id, thread);
            return;
        }
        if let Some(cursor) = cursor
            && sub.snapshot_bytes < cap
        {
            match read_older_page(&mut conn.session, &sub.command, &cursor) {
                Ok(rid) => {
                    conn.pending.insert(rid, Pending::Page { thread: thread.clone(), serial: sub.serial });
                    return;
                }
                Err(e) => log(format!("{id}: cannot read the next page: {e}")),
            }
        }
        self.finish_rebuild(id, thread);
    }

    fn on_page(&mut self, id: &TargetId, thread: &ThreadId, outcome: Result<Value, ErrorObject>) {
        let page = match outcome {
            Ok(wire) => {
                let Some(conn) = self.targets.get(id).and_then(|t| t.conn.as_ref()) else { return };
                finish_page(&conn.session, wire).ok()
            }
            Err(_) => None,
        };
        let Some(sub) = self.sub_mut(id, thread) else { return };
        match page {
            // A page from another snapshot cannot be mixed in: keep what is consistent.
            Some(page) if Some(page.position) == sub.snapshot_position => {
                sub.snapshot_bytes += serde_json::to_vec(&page.items).map_or(0, |v| v.len() as u64);
                sub.snapshot.extend(page.items);
                self.next_page_or_finish(id, thread, page.cursor);
            }
            _ => self.finish_rebuild(id, thread),
        }
    }

    /// Commits the snapshot (items and position in one transaction), then applies what was
    /// buffered while reading it.
    fn finish_rebuild(&mut self, id: &TargetId, thread: &ThreadId) {
        let key = (id.clone(), thread.clone());
        self.flush_batch(&key);
        let Some(sub) = self.sub_mut(id, thread) else { return };
        let Some(position) = sub.snapshot_position else { return };
        let required_requests = std::mem::take(&mut sub.required_requests);
        let mut items = std::mem::take(&mut sub.snapshot);
        items.reverse(); // pages are newest first
        let live = sub.subscribed.as_ref().map(LiveState::from_subscribed).unwrap_or_default();
        sub.phase = Phase::Live;
        let buffered = std::mem::take(&mut sub.buffered);
        let (target, th) = (key.0.clone(), key.1.clone());
        if self.cfg.notifier.is_some() {
            self.notifications.snapshot(&key, items.iter().chain(&required_requests));
        }
        self.worker.submit(move |s| match s.cache.replace_thread(&target, &th, &items, position, &live) {
            Ok(()) => Some(CoreEvent::Rebuilt { target, thread: th, position }),
            Err(e) => Some(CoreEvent::StoreError(e.to_string())),
        });
        self.applied.insert(key, position);
        for event in buffered {
            self.on_thread_event(id, event);
        }
    }

    fn on_rebuilt(&mut self, target: &TargetId, thread: &ThreadId, position: JournalPosition) {
        let key = (target.clone(), thread.clone());
        let newer = self.committed.get(&key).is_none_or(|c| c.epoch != position.epoch || c.seq <= position.seq);
        if newer {
            self.committed.insert(key.clone(), position);
        }
        let params = to_value(&ThreadResetParams { target: target.clone(), thread_id: thread.clone(), position });
        let message = Message::notification(ext::names::THREAD_RESET, params);
        for client in self.open.get(&key).into_iter().flatten() {
            self.send_to(*client, message.clone());
        }
        if let Some(sub) = self.sub_mut(target, thread) {
            sub.ready = true;
        }
        self.answer_waiters(target, thread);
    }

    fn on_thread_event(&mut self, id: &TargetId, event: ThreadEventParams) {
        let key = (id.clone(), event.thread_id.clone());
        let applied = self.applied.get(&key).copied();
        let Some(sub) = self.sub_mut(id, &key.1) else { return };
        match sub.phase {
            Phase::Subscribing => return,
            Phase::Reading => {
                sub.buffered.push(event);
                return;
            }
            Phase::Live => {}
        }
        if sub.epoch != Some(event.epoch) {
            return; // another journal; the subscription result decides
        }
        // Keep control and effective capabilities current for later window answers.
        if let Some(subscribed) = sub.subscribed.as_mut() {
            match &event.event {
                ThreadEvent::ControlChanged(c) => {
                    subscribed.control = c.control.clone();
                    subscribed.control_holder = c.holder.clone();
                    subscribed.take_over_blocked = c.take_over_blocked.clone();
                    subscribed.active_turn = c.active_turn.clone();
                }
                ThreadEvent::SettingsChanged(s) => subscribed.effective = Some(s.effective.clone()),
                ThreadEvent::QueueChanged(q) => {
                    let mut queue = subscribed.queued.take().unwrap_or_default();
                    crate::cache::apply::apply_queue(&mut queue, q);
                    subscribed.queued = (!queue.is_empty()).then_some(queue);
                }
                ThreadEvent::HoldChanged(h) => subscribed.hold = h.hold.clone(),
                _ => {}
            }
        }
        let after = event.position_after();
        if let Some(p) = applied
            && p.epoch == event.epoch
        {
            if after.seq <= p.seq {
                return; // already applied: a replay overlap or a duplicate
            }
            if event.seq.0 > p.seq.0 + 1 {
                // A gap: never skip events. Resubscribe from what is stored.
                log(format!("{id}: gap in the journal (have {}, got {}); resubscribing", p.seq.0, event.seq.0));
                self.flush_batch(&key);
                let params = ThreadUnsubscribeParams { thread_id: key.1.clone() };
                self.send::<methods::ThreadUnsubscribe>(id, &params, Pending::Ignore);
                self.start_open(id, &key.1, false);
                return;
            }
        }
        if self.cfg.notifier.is_some() {
            match &event.event {
                ThreadEvent::ItemStarted(e) | ThreadEvent::ItemCompleted(e) => self.notifications.observe(&key, &e.item, Instant::now()),
                ThreadEvent::ItemUpdated(e) => self.notifications.observe(&key, &e.item, Instant::now()),
                ThreadEvent::RequestChanged(e) if e.state != RequestState::Pending => self.notifications.resolve(&attach::RequestLocation {
                    target: key.0.clone(), thread_id: key.1.clone(), request_id: e.request_id.clone()
                }),
                _ => {}
            }
        }
        self.applied.insert(key.clone(), after);
        self.batch.entry(key).or_default().push(event);
    }

    fn flush_batch(&mut self, key: &Key) {
        let Some(events) = self.batch.remove(key) else { return };
        self.submit_commit(key.clone(), events);
    }

    fn flush_batches(&mut self) {
        let batch = std::mem::take(&mut self.batch);
        for (key, events) in batch {
            self.submit_commit(key, events);
        }
    }

    fn submit_commit(&self, (target, thread): Key, events: Vec<ThreadEventParams>) {
        let Some(position) = events.last().map(ThreadEventParams::position_after) else { return };
        self.worker.submit(move |s| match s.cache.commit_events(&target, &thread, &events, position) {
            Ok(()) => Some(CoreEvent::Committed { target, thread, events, position }),
            Err(e) => Some(CoreEvent::StoreError(e.to_string())),
        });
    }

    fn on_committed(&mut self, target: &TargetId, thread: ThreadId, events: Vec<ThreadEventParams>, position: JournalPosition) {
        let key = (target.clone(), thread);
        self.committed.insert(key.clone(), position);
        let Some(clients) = self.open.get(&key) else { return };
        for event in events {
            let message = Self::routed_notification(target, names::THREAD_EVENT, to_value(&event));
            for client in clients {
                self.send_to(*client, message.clone());
            }
        }
    }

    fn fail_sub(&mut self, id: &TargetId, thread: &ThreadId, error: &ErrorObject) {
        let Some(sub) = self.targets.get_mut(id).and_then(|t| t.conn.as_mut()).and_then(|c| c.subs.remove(thread)) else { return };
        let key = (id.clone(), thread.clone());
        for (client, rid, _) in sub.waiters {
            self.reply(client, rid, Err(error.clone()));
        }
        if let Some(clients) = self.open.remove(&key) {
            for client in clients {
                if let Some(c) = self.clients.get_mut(&client) {
                    c.subs.remove(&key);
                }
            }
        }
    }

    fn answer_waiters(&mut self, id: &TargetId, thread: &ThreadId) {
        let key = (id.clone(), thread.clone());
        let position = self.committed.get(&key).or_else(|| self.applied.get(&key)).copied();
        let Some(sub) = self.sub_mut(id, thread) else { return };
        if !sub.ready {
            return;
        }
        let waiters = std::mem::take(&mut sub.waiters);
        let subscribed = sub.subscribed.clone();
        let epoch = sub.epoch;
        for (client, rid, request) in waiters {
            let result = window_answer(subscribed.as_ref(), position, epoch, &request);
            self.reply(client, rid, Ok(to_value(&result)));
        }
    }

    /// A window opens a thread.
    fn window_subscribe(&mut self, client: ClientKey, rid: &RpcId, target: &TargetId, params: Value) -> Option<Result<Value, ErrorObject>> {
        let request: ThreadSubscribeParams = match parse(params) {
            Ok(r) => r,
            Err(e) => return Some(Err(e)),
        };
        let key = (target.clone(), request.thread_id.clone());
        let connected = self.targets.get(target).is_some_and(|t| t.conn.as_ref().is_some_and(|c| c.ready));
        if !connected {
            // Offline: answer from the cache if the thread is there; it is resubscribed
            // upstream when the target connects.
            let Some(position) = self.committed.get(&key).copied() else {
                return Some(Err(err(ErrorCode::TargetOffline, "the target is not connected and the thread is not cached")));
            };
            self.hold(client, &key);
            let rid = rid.clone();
            self.worker.submit(move |s| {
                let live = s.cache.thread(&key.0, &key.1).ok().flatten().map(|t| t.live).unwrap_or_default();
                let result = ThreadSubscribeResult {
                    mode: window_mode(position, &request),
                    // Last known; the target is offline, so the window cannot act on it anyway.
                    control: live.control.unwrap_or(Control::Free),
                    control_holder: live.control_holder,
                    take_over_blocked: None,
                    active_turn: live.active_turn,
                    effective: live.effective,
                    context: live.context,
                    tokens: live.tokens,
                    queued: (!live.queued.is_empty()).then_some(live.queued),
                    hold: live.hold,
                };
                Some(CoreEvent::Reply { client, id: rid, outcome: Ok(to_value(&result)) })
            });
            return None;
        }
        self.hold(client, &key);
        let touch = key.clone();
        self.worker.submit(move |s| s.cache.touch(&touch.0, &touch.1).err().map(|e| CoreEvent::StoreError(e.to_string())));
        let has_sub = self.sub_mut(target, &request.thread_id).is_some();
        if !has_sub {
            self.start_open(target, &request.thread_id, false);
        }
        let Some(sub) = self.sub_mut(target, &request.thread_id) else {
            return Some(Err(err(ErrorCode::InternalError, "could not subscribe")));
        };
        sub.waiters.push((client, rid.clone(), request));
        let thread = key.1.clone();
        self.answer_waiters(target, &thread);
        None
    }

    fn hold(&mut self, client: ClientKey, key: &Key) {
        self.open.entry(key.clone()).or_default().insert(client);
        if let Some(c) = self.clients.get_mut(&client) {
            c.subs.insert(key.clone());
        }
    }

    /// A window lets go of a thread; the last one to let go ends the upstream subscription.
    fn release(&mut self, client: ClientKey, key: &Key) {
        if let Some(c) = self.clients.get_mut(&client) {
            c.subs.remove(key);
        }
        let Some(holders) = self.open.get_mut(key) else { return };
        holders.remove(&client);
        if !holders.is_empty() {
            return;
        }
        self.open.remove(key);
        self.flush_batch(key);
        let (target, thread) = key;
        let committed = self.committed.get(key).copied();
        let Some(sub) = self.targets.get_mut(target).and_then(|t| t.conn.as_mut()).and_then(|c| c.subs.remove(thread)) else { return };
        if let Some(p) = committed
            && sub.phase == Phase::Live
            && sub.acked != Some(p)
            && sub.epoch == Some(p.epoch)
        {
            self.send::<ThreadAck>(target, &ThreadAckParams { thread_id: thread.clone(), epoch: p.epoch, seq: p.seq }, Pending::Ignore);
        }
        self.send::<methods::ThreadUnsubscribe>(target, &ThreadUnsubscribeParams { thread_id: thread.clone() }, Pending::Ignore);
        self.flush(target);
    }

    // ── The routing envelope ─────────────────────────────────────────────────────────────

    fn on_routed(&mut self, client: ClientKey, rid: &RpcId, request: RoutedRequest) -> Option<Result<Value, ErrorObject>> {
        let target = request.target.clone();
        if !self.targets.contains_key(&target) {
            return Some(Err(err(ErrorCode::TargetUnknown, format!("no target {target}"))));
        }
        let method = request.method.as_str();
        match method {
            name if name.starts_with("terminal/") => return Some(Err(hub_err(ext::codes::NOT_ROUTABLE, "use the hub terminal methods"))),
            name if name.starts_with("voice/") => return Some(Err(hub_err(ext::codes::NOT_ROUTABLE, "use the hub voice methods"))),
            name if name.starts_with("voiceCall/") => return Some(Err(hub_err(ext::codes::NOT_ROUTABLE, "use the hub voice call methods"))),
            names::INITIALIZE | names::MESH_RELAY | names::MESH_DELIVER | names::MESH_RECEIPT | names::MESH_DELIVERY_RECEIPT | names::MESH_ROSTER | names::TUNNEL_OPEN => {
                return Some(Err(hub_err(ext::codes::NOT_ROUTABLE, format!("{method} is handled by the hub itself"))));
            }
            names::PREVIEW_PORTS => {
                if let Err(error) = self.preview_check(&target) {
                    return Some(Err(error));
                }
            }
            names::THREAD_SUBSCRIBE => {
                let outcome = self.window_subscribe(client, rid, &target, request.params);
                self.flush(&target);
                return outcome;
            }
            names::THREAD_UNSUBSCRIBE => {
                let Ok(p) = parse::<ThreadUnsubscribeParams>(request.params) else {
                    return Some(Err(err(ErrorCode::InvalidParams, "threadId missing")));
                };
                self.release(client, &(target, p.thread_id));
                return Some(Ok(json!({})));
            }
            names::THREAD_ACK => return Some(Ok(json!({}))), // the hub acknowledges what it committed
            _ => {}
        }
        let Some(info) = methods::request_info(method) else {
            return Some(Err(err(ErrorCode::MethodNotFound, format!("unknown method {method}"))));
        };
        let connected = self.targets.get(&target).is_some_and(|t| t.conn.as_ref().is_some_and(|c| c.ready));
        if !connected {
            if method == names::THREAD_SET_MODE {
                let change = match parse::<yaac_protocol::ThreadSetModeParams>(request.params) {
                    Ok(p) => p, Err(e) => return Some(Err(e)),
                };
                self.store_reply(client, rid, move |s| s.drafts.queue_mode(&target, &change)
                    .map(|()| json!({ "pending": true })).map_err(|e| e.to_string()));
                return None;
            }
            if let Some(outcome) = self.answer_offline(client, rid, &target, info.name, request.params.clone()) {
                return outcome;
            }
            let frozen = methods::allowed_before_initialize(method);
            if !(frozen && self.targets.get(&target).is_some_and(|t| t.conn.is_some())) {
                return Some(Err(err(ErrorCode::TargetOffline, "the target is not connected")));
            }
        }
        let mutation = info.state_changing
            || info.name == names::DISCOVERY_CAPS && request.params.get("update").is_some_and(|update| !update.is_null());
        let op = mutation.then(|| request.params.get("opId").and_then(Value::as_str).map(OpId::new)).flatten();
        let thread = request.params.get("threadId").and_then(Value::as_str).map(ThreadId::new);
        let Some(conn) = self.targets.get_mut(&target).and_then(|t| t.conn.as_mut()) else {
            return Some(Err(err(ErrorCode::TargetOffline, "the target is not connected")));
        };
        if matches!(method, names::FS_TREE | names::FS_READ | names::GIT_FILE_DIFF)
            && !conn.session.features().is_some_and(|f| f.allows(&yaac_protocol::Capability::FileView))
        {
            return Some(Err(file_view_missing()));
        }
        if method == names::THREAD_SET_MODE
            && let (Some(newer), Some(thread)) = (conn.modes_during_reconnect.as_mut(), thread.as_ref()) {
            newer.insert(thread.clone());
        }
        match conn.session.request_value(info.name, request.params) {
            Ok(id) => {
                conn.pending.insert(id, Pending::Routed { client, id: rid.clone(), method: info.name, op: op.clone() });
                if let Some(op) = op {
                    let (t, m) = (target.clone(), info.name);
                    self.worker.submit(move |s| s.drafts.op_sent(&op, &t, m, thread.as_ref()).err().map(|e| CoreEvent::StoreError(e.to_string())));
                }
                self.flush(&target);
                None
            }
            Err(e) => Some(Err(err(ErrorCode::InvalidParams, e.to_string()))),
        }
    }

    /// While a target is offline, the index, providers, and cached transcripts are served from
    /// the cache. `None`: not answerable from the cache.
    fn answer_offline(&self, client: ClientKey, rid: &RpcId, target: &TargetId, method: &str, params: Value) -> Option<Option<Result<Value, ErrorObject>>> {
        let target = target.clone();
        let id = rid.clone();
        let reply = move |outcome: Result<Value, String>| Some(CoreEvent::Reply { client, id, outcome: outcome.map_err(|e| err(ErrorCode::InternalError, e)) });
        match method {
            names::PROVIDER_LIST => {
                self.worker.submit(move |s| reply(s.cache.providers(&target).map(|providers| json!({ "providers": providers })).map_err(|e| e.to_string())));
            }
            names::THREAD_SYNC => {
                self.worker.submit(move |s| {
                    reply((|| {
                        let token = s.cache.sync_token(&target)?.unwrap_or_default();
                        let current = !token.as_str().is_empty() && params.get("token").and_then(Value::as_str) == Some(token.as_str());
                        let changed = if current { vec![] } else { s.cache.threads(&target)? };
                        Ok(to_value(&ThreadSyncResult { changed, removed: vec![], token, more: false, full: !current, stale_instances: vec![] }))
                    })().map_err(|e: crate::cache::CacheError| e.to_string()))
                });
            }
            names::THREAD_READ => {
                let Ok(p) = serde_json::from_value::<ThreadReadParams>(params) else { return None };
                self.worker.submit(move |s| reply(read_from_cache(&mut s.cache, &target, &p)));
            }
            _ => return None,
        }
        Some(None)
    }

    // ── The mesh relay ───────────────────────────────────────────────────────────────────

    fn on_peer_request(&mut self, source: &TargetId, peer_id: RpcId, request: PeerRequest) {
        let generation = self.targets.get(source).and_then(|t| t.conn.as_ref()).map_or(0, |c| c.generation);
        match request {
            PeerRequest::MeshRelay(params) => {
                let message = params.message;
                if message.from.target().as_ref() != Some(source) {
                    let e = err(ErrorCode::PolicyDenied, "the sender's address does not belong to this target");
                    self.respond_peer(source, generation, peer_id, names::MESH_RELAY, Err(e));
                    return;
                }
                let dest = message.to.target().filter(|d| self.targets.contains_key(d));
                let online = dest.as_ref().is_some_and(|d| self.targets.get(d).is_some_and(|t| t.conn.as_ref().is_some_and(|c| c.ready)));
                let source = source.clone();
                self.worker.submit(move |s| {
                    let accepted = s.relay.accept(&source, dest.as_ref(), &message, online).map_err(|e| e.to_string());
                    let in_flight = s.relay.in_flight().unwrap_or(0);
                    Some(CoreEvent::RelayAccepted { source, generation, peer_id, message, accepted, in_flight })
                });
            }
            PeerRequest::MeshDeliveryReceipt(params) => {
                if !matches!(params.receipt.status, ReceiptStatus::Delivered | ReceiptStatus::Failed | ReceiptStatus::OutcomeUnknown) {
                    let e = err(ErrorCode::InvalidParams, "a final delivery receipt is required");
                    self.respond_peer(source, generation, peer_id, names::MESH_DELIVERY_RECEIPT, Err(e));
                    return;
                }
                let dest = source.clone();
                self.worker.submit(move |s| {
                    let outcome = s.relay.record_outcome(&dest, &params.message_id, &params.receipt).map_err(|e| e.to_string());
                    let in_flight = s.relay.in_flight().unwrap_or(0);
                    Some(CoreEvent::DeliveryReceipt { dest, generation, peer_id, outcome, params, in_flight })
                });
            }
            PeerRequest::MeshQuery(params) => self.route_query(source, generation, peer_id, &params),
            PeerRequest::MeshUsage(params) => self.route_usage(source, generation, peer_id, &params),
            PeerRequest::CatalogSkills(params) => {
                let source = source.clone();
                self.worker.submit(move |s| {
                    let outcome = s.drafts.catalog(&params).map(|r| to_value(&r))
                        .map_err(|e| err(ErrorCode::InvalidParams, e));
                    Some(CoreEvent::CatalogTarget { source, generation, id: peer_id, outcome })
                });
            }
            PeerRequest::ThemeCall(params) => self.route_theme(source, generation, peer_id, params),
            PeerRequest::BrowserCall(params) => self.route_browser(source, generation, peer_id, params),
            PeerRequest::Other { .. } => {} // the session already answered method_not_found
        }
    }

    fn respond_peer(&mut self, target: &TargetId, generation: u64, peer_id: RpcId, method: &str, outcome: Result<Value, ErrorObject>) {
        let Some(conn) = self.targets.get_mut(target).and_then(|t| t.conn.as_mut()).filter(|c| c.generation == generation) else { return };
        if let Err(e) = conn.session.respond(peer_id, method, outcome) {
            log(format!("{target}: cannot answer {method}: {e}"));
        }
        self.flush(target);
    }

    fn on_relay_accepted(&mut self, source: &TargetId, generation: u64, peer_id: RpcId, message: MeshMessage, accepted: Result<Accepted, String>) {
        match accepted {
            Err(e) => {
                // Not stored: do not acknowledge. The source retries until it is.
                self.respond_peer(source, generation, peer_id, names::MESH_RELAY, Err(err(ErrorCode::InternalError, e)));
            }
            Ok(accepted) => {
                let result = to_value(&yaac_protocol::MeshReceiptResult { receipt: accepted.receipt });
                self.respond_peer(source, generation, peer_id, names::MESH_RELAY, Ok(result));
                if accepted.fresh
                    && let Some(dest) = message.to.target()
                {
                    self.deliver(&dest, message);
                }
            }
        }
    }

    fn deliver(&mut self, dest: &TargetId, message: MeshMessage) {
        let Some(conn) = self.targets.get_mut(dest).and_then(Target::ready_conn) else { return };
        if !conn.delivering.insert(message.id.clone()) {
            return; // already on its way on this connection
        }
        let message_id = message.id.clone();
        if !self.send::<MeshDeliver>(dest, &MeshDeliverParams { message }, Pending::Deliver { message_id: message_id.clone() })
            && let Some(conn) = self.targets.get_mut(dest).and_then(|t| t.conn.as_mut())
        {
            conn.delivering.remove(&message_id);
        }
        self.flush(dest);
    }

    fn on_delivered(&mut self, dest: &TargetId, message_id: MessageId, outcome: Result<Value, ErrorObject>) {
        if let Some(conn) = self.targets.get_mut(dest).and_then(|t| t.conn.as_mut()) {
            conn.delivering.remove(&message_id);
        }
        let receipt = match outcome {
            Ok(wire) => match self.decode_typed::<MeshDeliver>(dest, wire) {
                Ok(result) => result.receipt,
                Err(_) => return, // undecodable: stays queued, retried on reconnect
            },
            Err(e) => {
                let permanent = matches!(
                    e.code,
                    ErrorCode::ThreadNotFound | ErrorCode::InvalidParams | ErrorCode::MethodNotFound | ErrorCode::PolicyDenied
                );
                if !permanent {
                    return; // stays queued; delivered again when the destination reconnects
                }
                let reason = match e.code {
                    ErrorCode::ThreadNotFound => ReceiptReason::UnknownAddress,
                    ErrorCode::PolicyDenied => ReceiptReason::PolicyDenied,
                    _ => ReceiptReason::RemoteDisallowed,
                };
                receipt(ReceiptStatus::Failed, Some(reason), Some(e.message))
            }
        };
        self.worker.submit(move |s| {
            let source = match s.relay.finish(&message_id, &receipt) {
                Ok(source) => source,
                Err(e) => return Some(CoreEvent::StoreError(e.to_string())),
            };
            let in_flight = s.relay.in_flight().unwrap_or(0);
            match source {
                Some(source) => Some(CoreEvent::Receipts { receipts: vec![(source, message_id, receipt)], in_flight }),
                None => Some(CoreEvent::InFlight(in_flight)),
            }
        });
    }

    fn send_receipt(&mut self, source: &TargetId, message_id: MessageId, receipt: Receipt) {
        let params = MeshReceiptParams { message_id: message_id.clone(), receipt: receipt.clone() };
        self.send::<MeshReceipt>(source, &params, Pending::Receipt { message_id, receipt });
        self.flush(source);
    }

    fn route_query(&mut self, source: &TargetId, generation: u64, peer_id: RpcId, params: &MeshQueryParams) {
        // The caller is stamped by its own yaacd: it must be a thread of the connection's target.
        if params.caller.target().as_ref() != Some(source) {
            let e = err(ErrorCode::PolicyDenied, "the calling thread does not belong to this target");
            self.respond_peer(source, generation, peer_id, names::MESH_QUERY, Err(e));
            return;
        }
        // Questions about the whole fleet are answered here, from the hub's own index.
        if matches!(params.query, yaac_protocol::MeshQuery::FindThreads(_) | yaac_protocol::MeshQuery::ListTargets(_)) {
            self.mesh_index_query(source, generation, peer_id, params.query.clone());
            return;
        }
        let dest = params.target.clone();
        if !self.targets.contains_key(&dest) {
            self.respond_peer(source, generation, peer_id, names::MESH_QUERY, Err(err(ErrorCode::TargetUnknown, format!("no target {dest}"))));
            return;
        }
        let pending = Pending::MeshQuery { source: source.clone(), generation, peer_id: peer_id.clone() };
        if !self.send::<MeshQuery>(&dest, params, pending) {
            self.respond_peer(source, generation, peer_id, names::MESH_QUERY, Err(err(ErrorCode::TargetOffline, format!("{dest} is not connected"))));
            return;
        }
        self.flush(&dest);
    }

    /// `mesh/usage`: a target's spending against a root on another target, forwarded to the
    /// root's target, whose answer (exhausted or not) goes back to the reporter. Counts are
    /// cumulative, so a report that fails is simply sent again later by the reporter.
    fn route_usage(&mut self, source: &TargetId, generation: u64, peer_id: RpcId, params: &MeshUsageParams) {
        if &params.source != source {
            let e = err(ErrorCode::PolicyDenied, "the reporting target does not match the connection");
            self.respond_peer(source, generation, peer_id, names::MESH_USAGE, Err(e));
            return;
        }
        let Some(dest) = params.root.target().filter(|d| self.targets.contains_key(d)) else {
            let e = err(ErrorCode::TargetUnknown, format!("the root {} is on no known target", params.root));
            self.respond_peer(source, generation, peer_id, names::MESH_USAGE, Err(e));
            return;
        };
        if &dest == source {
            let e = err(ErrorCode::InvalidParams, "the root is on the reporting target itself; it counts its own usage");
            self.respond_peer(source, generation, peer_id, names::MESH_USAGE, Err(e));
            return;
        }
        let pending = Pending::PeerForward { source: source.clone(), generation, peer_id: peer_id.clone(), method: names::MESH_USAGE };
        if !self.send::<MeshUsage>(&dest, params, pending) {
            let e = err(ErrorCode::TargetOffline, format!("{dest} is not connected"));
            self.respond_peer(source, generation, peer_id, names::MESH_USAGE, Err(e));
            return;
        }
        self.flush(&dest);
    }

    /// Messages that have waited undelivered past the limit: a passive notice to each sender,
    /// once. It is a `notice` message (it never starts a turn), carried by the relay like any
    /// other, so it reaches a sender whose target is offline when it comes back.
    fn notice_undelivered(&mut self) {
        let limit = self.cfg.undelivered_notice;
        let ready: Vec<TargetId> = self.targets.iter().filter(|(_, t)| t.conn.as_ref().is_some_and(|c| c.ready)).map(|(id, _)| id.clone()).collect();
        self.worker.submit(move |s| {
            let overdue = match s.relay.overdue(limit) {
                Ok(o) => o,
                Err(e) => return Some(CoreEvent::StoreError(e.to_string())),
            };
            let mut deliver: Vec<(TargetId, MeshMessage)> = Vec::new();
            for (source, message) in overdue {
                let notice = crate::relay::undelivered_notice(&message, limit);
                let online = ready.contains(&source);
                match s.relay.accept(&crate::relay::system_source(), Some(&source), &notice, online) {
                    Ok(a) if a.fresh && online => deliver.push((source, notice)),
                    Ok(_) => {}
                    Err(e) => return Some(CoreEvent::StoreError(e.to_string())),
                }
                let _ = s.relay.mark_noticed(&message.id);
            }
            let in_flight = s.relay.in_flight().unwrap_or(0);
            Some(CoreEvent::Notices { deliveries: deliver, in_flight })
        });
    }

    /// An agent's `findThreads` or `listTargets`: read the cached index off the core thread.
    fn mesh_index_query(&mut self, source: &TargetId, generation: u64, peer_id: RpcId, query: yaac_protocol::MeshQuery) {
        let targets: Vec<(TargetId, Option<TargetName>, bool)> = self
            .settings
            .targets
            .iter()
            .map(|d| (d.id.clone(), Some(d.name.clone()), self.targets.get(&d.id).is_some_and(|t| t.conn.as_ref().is_some_and(|c| c.ready))))
            .collect();
        let source = source.clone();
        self.worker.submit(move |s| {
            let mut threads = Vec::new();
            let mut labels = Vec::new();
            for (t, name, reachable) in &targets {
                match s.cache.threads(t) {
                    Ok(list) => threads.extend(list.into_iter().map(|th| (t.clone(), th))),
                    Err(e) => return Some(CoreEvent::StoreError(e.to_string())),
                }
                let providers = s.cache.providers(t).unwrap_or_default();
                labels.push((t.clone(), crate::mesh::TargetLabels { name: name.clone(), reachable: *reachable, providers }));
            }
            Some(CoreEvent::MeshIndexReady { source, generation, peer_id, query, threads, labels })
        });
    }

    fn on_mesh_index(
        &mut self,
        source: &TargetId,
        generation: u64,
        peer_id: RpcId,
        query: &yaac_protocol::MeshQuery,
        threads: &[(TargetId, yaac_protocol::ThreadSummary)],
        labels: &[(TargetId, crate::mesh::TargetLabels)],
    ) {
        if let yaac_protocol::MeshQuery::FindThreads(q) = query {
            let params = crate::mesh::find_params(q);
            if params.cursor.is_none()
                && crate::find::looks_like_exact_id(&params.query)
                && self.finder.page(threads, &params, Instant::now()).is_ok_and(|p| p.results.is_empty())
            {
                let targets = self.lookup_targets(params.target.as_ref());
                if !targets.is_empty() {
                    let answer = LookupFor::Mesh { source: source.clone(), generation, peer_id, query: Box::new(query.clone()), labels: labels.to_vec() };
                    self.start_lookup(params.query.trim(), &targets, answer);
                    return;
                }
            }
        }
        let outcome = match query {
            yaac_protocol::MeshQuery::FindThreads(q) => self
                .finder
                .page(threads, &crate::mesh::find_params(q), Instant::now())
                .map(|page| {
                    let label = |t: &TargetId| labels.iter().find(|(id, _)| id == t).map(|(_, l)| l.clone());
                    to_value(&yaac_protocol::MeshQueryResult::FindThreads(crate::mesh::find_reply(&page, &label)))
                })
                .map_err(|e| err(ErrorCode::InvalidParams, e.to_string())),
            _ => {
                let targets: Vec<(TargetId, TargetName, bool)> =
                    labels.iter().map(|(id, l)| (id.clone(), l.name.clone().unwrap_or_else(|| TargetName::new(id.as_str())), l.reachable)).collect();
                let states: Vec<(TargetId, ThreadState)> = threads.iter().map(|(t, s)| (t.clone(), s.state.clone())).collect();
                Ok(to_value(&yaac_protocol::MeshQueryResult::ListTargets(crate::mesh::targets_reply(&targets, &states))))
            }
        };
        self.respond_peer(source, generation, peer_id, names::MESH_QUERY, outcome);
    }

    /// `hub/threads/find`: the cached index of every target, searched and paged.
    fn find_threads(&mut self, client: ClientKey, id: &RpcId, params: FindParams) {
        let id = id.clone();
        let targets: Vec<TargetId> = self.targets.keys().cloned().collect();
        self.worker.submit(move |s| {
            let mut threads = Vec::new();
            for t in &targets {
                match s.cache.threads(t) {
                    Ok(list) => threads.extend(list.into_iter().map(|th| (t.clone(), th))),
                    Err(e) => return Some(CoreEvent::Reply { client, id, outcome: Err(err(ErrorCode::InternalError, e.to_string())) }),
                }
            }
            Some(CoreEvent::FindReady { client, id, params, threads })
        });
    }

    fn on_find_ready(&mut self, client: ClientKey, id: RpcId, params: &FindParams, threads: &[(TargetId, yaac_protocol::ThreadSummary)]) {
        let outcome = self.finder.page(threads, params, Instant::now());
        // An exact-looking id no index has: ask the targets (and their harnesses) directly.
        if let Ok(page) = &outcome
            && page.results.is_empty()
            && params.cursor.is_none()
            && crate::find::looks_like_exact_id(&params.query)
        {
            let targets = self.lookup_targets(params.target.as_ref());
            if !targets.is_empty() {
                let query = params.query.trim().to_owned();
                self.start_lookup(&query, &targets, LookupFor::Find { client, id, params: params.clone() });
                return;
            }
        }
        self.reply(client, id, outcome.map(|r| to_value(&r)).map_err(|e| err(ErrorCode::InvalidParams, e.to_string())));
    }

    /// The connected targets a lookup goes to (one, or all).
    fn lookup_targets(&self, only: Option<&TargetId>) -> Vec<TargetId> {
        self.targets
            .iter()
            .filter(|(id, t)| only.is_none_or(|o| o == *id) && t.conn.as_ref().is_some_and(|c| c.ready))
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Sends `thread/lookup` to each target; `answer` is given what they found.
    fn start_lookup(&mut self, id: &str, targets: &[TargetId], answer: LookupFor) {
        self.next_lookup += 1;
        let lookup = self.next_lookup;
        let params = yaac_protocol::ThreadLookupParams { id: id.to_owned(), instance: None };
        let mut state = Lookup { waiting: 0, found: Vec::new(), unreachable: Vec::new(), answer };
        for t in targets {
            if self.send::<ThreadLookup>(t, &params, Pending::Lookup { lookup }) {
                state.waiting += 1;
                self.flush(t);
            } else {
                state.unreachable.push(t.clone());
            }
        }
        self.lookups.insert(lookup, state);
        if self.lookups.get(&lookup).is_some_and(|l| l.waiting == 0) {
            self.finish_lookup(lookup);
        }
    }

    /// One target answered a lookup (`None`: it could not).
    fn lookup_answered(&mut self, lookup: u64, target: &TargetId, found: Option<Vec<yaac_protocol::ThreadSummary>>) {
        let Some(state) = self.lookups.get_mut(&lookup) else { return };
        state.waiting = state.waiting.saturating_sub(1);
        match found {
            Some(threads) => state.found.extend(threads.into_iter().map(|t| (target.clone(), t))),
            None => state.unreachable.push(target.clone()),
        }
        if state.waiting == 0 {
            self.finish_lookup(lookup);
        }
    }

    fn finish_lookup(&mut self, lookup: u64) {
        let Some(state) = self.lookups.remove(&lookup) else { return };
        // What was brought into a target's index reaches the cache through its `thread/updated`.
        match state.answer {
            LookupFor::Window { client, id } => {
                let threads = state.found.into_iter().map(|(target, thread)| LookupHit { target, thread }).collect();
                self.reply(client, id, Ok(to_value(&LookupResult { threads, unreachable: state.unreachable })));
            }
            LookupFor::Find { client, id, params } => {
                let outcome = self
                    .finder
                    .page(&state.found, &params, Instant::now())
                    .map(|r| to_value(&r))
                    .map_err(|e| err(ErrorCode::InvalidParams, e.to_string()));
                self.reply(client, id, outcome);
            }
            LookupFor::Mesh { source, generation, peer_id, query, labels } => {
                let yaac_protocol::MeshQuery::FindThreads(q) = query.as_ref() else { return };
                let outcome = self
                    .finder
                    .page(&state.found, &crate::mesh::find_params(q), Instant::now())
                    .map(|page| {
                        let label = |t: &TargetId| labels.iter().find(|(id, _)| id == t).map(|(_, l)| l.clone());
                        to_value(&yaac_protocol::MeshQueryResult::FindThreads(crate::mesh::find_reply(&page, &label)))
                    })
                    .map_err(|e| err(ErrorCode::InvalidParams, e.to_string()));
                self.respond_peer(&source, generation, peer_id, names::MESH_QUERY, outcome);
            }
        }
    }

    /// Keeps a target's discovery caps in the cache.
    fn store_discovery(&mut self, id: &TargetId, caps: yaac_protocol::DiscoveryCaps) {
        let target = id.clone();
        self.worker.submit(move |s| match s.cache.put_discovery(&target, &caps) {
            Ok(()) => Some(CoreEvent::Broadcast(Message::notification(ext::names::DISCOVERY_CHANGED, json!({ "target": target })))),
            Err(e) => Some(CoreEvent::StoreError(e.to_string())),
        });
    }

    /// Watch the journal of every blocked thread, including while its window is closed.
    fn note_state(&mut self, target: &TargetId, summary: &yaac_protocol::ThreadSummary) {
        let key = (target.clone(), summary.id.clone());
        let before = self.thread_states.insert(key.clone(), summary.state.clone());
        if before.as_ref().is_some_and(turn_going) && !turn_going(&summary.state) {
            self.turn_ended(key.clone());
        }
        if self.cfg.notifier.is_none() { return; }
        if summary.state == ThreadState::NeedsAttention {
            let target_name = self.targets.get(target).map_or(target.as_str(), |t| t.def.name.as_str());
            self.notifications.labels.insert(key.clone(), format!("{} on {target_name}", summary.title));
            self.open.entry(key).or_default().insert(NOTIFICATION_CLIENT);
            if self.sub_mut(target, &summary.id).is_none() { self.start_open(target, &summary.id, true); }
        } else if self.open.get(&key).is_some_and(|holders| holders.contains(&NOTIFICATION_CLIENT)) {
            self.notifications.labels.remove(&key);
            self.notifications.snapshot(&key, &[]);
            self.release(NOTIFICATION_CLIENT, &key);
        }
    }

    /// A thread's turn ended (D114): with no attached window showing it, it is marked unseen,
    /// kept like pin and mute so every window and the next start agree.
    fn turn_ended(&mut self, key: Key) {
        if self.unseen.contains(&key) {
            return;
        }
        let shown = self.clients.values().any(|c| c.hello == Hello::Attached && c.showing.contains(&key));
        if shown {
            return;
        }
        self.unseen.insert(key.clone());
        let (target, thread_id) = key;
        self.set_views(vec![ext::ViewSetParams { target, thread_id, unseen: Some(true), ..Default::default() }], None);
    }

    // ── Timers and lifecycle ─────────────────────────────────────────────────────────────

    fn busy(&self) -> BusyReasons {
        let enabled: HashSet<&TargetId> = self.targets.values().filter(|t| t.def.enabled).map(|t| &t.def.id).collect();
        let active = self
            .thread_states
            .iter()
            .filter(|((t, _), s)| enabled.contains(t) && matches!(s, ThreadState::Running | ThreadState::NeedsAttention))
            .count();
        let pending = self
            .targets
            .values()
            .filter_map(|t| t.conn.as_ref())
            .flat_map(|c| c.pending.values())
            .filter(|p| matches!(p, Pending::Routed { .. } | Pending::MeshQuery { .. }))
            .count();
        BusyReasons {
            windows: u32::try_from(self.clients.len()).unwrap_or(u32::MAX),
            active_threads: u32::try_from(active).unwrap_or(u32::MAX),
            pending_requests: u32::try_from(pending).unwrap_or(u32::MAX),
            messages_in_flight: u32::try_from(self.in_flight).unwrap_or(u32::MAX),
            uploads: u32::try_from(self.uploads_active()).unwrap_or(u32::MAX),
        }
    }

    pub(crate) fn tick(&mut self) {
        let now = Instant::now();
        self.notifications.flush(now, self.settings.hub.notifications,
            |loc| self.clients.values().any(|c| c.visible_requests.as_ref().is_some_and(|v| v.contains(loc))),
            self.cfg.notifier.as_deref(), &self.tx);
        if std::mem::take(&mut self.notifications.feed_changed) {
            let feed = to_value(&self.notifications.feed);
            let to: Vec<_> = self.clients.iter().filter(|(_, c)| c.feed).map(|(k, _)| *k).collect();
            for client in to {
                self.send_to(client, Message::notification(attach::names::NOTIFICATIONS_REQUESTS, feed.clone()));
            }
        }
        self.expire_browser_calls(now);
        self.expire_preview_reads(now);
        // Acknowledge what is committed.
        if now.duration_since(self.last_ack) >= self.cfg.ack_interval {
            self.last_ack = now;
            self.send_acks();
        }
        // Liveness: ping quiet connections; drop dead ones.
        let ids: Vec<TargetId> = self.targets.keys().cloned().collect();
        for id in &ids {
            let Some(conn) = self.targets.get_mut(id).and_then(|t| t.conn.as_mut()) else { continue };
            if now.duration_since(conn.last_rx) >= self.cfg.dead_after {
                log(format!("{id}: no traffic for {:?}; reconnecting", self.cfg.dead_after));
                if let Some(kill) = conn.kill.take() {
                    let _ = kill.send(());
                }
                continue;
            }
            if conn.ready && now.duration_since(conn.last_ping) >= self.cfg.ping_interval {
                conn.last_ping = now;
                self.send::<Ping>(id, &yaac_protocol::Empty {}, Pending::Ignore);
                self.flush(id);
            }
        }
        // Senders of messages that have waited too long are told so, once.
        if now.duration_since(self.last_notice_sweep) >= self.cfg.notice_sweep {
            self.last_notice_sweep = now;
            self.notice_undelivered();
        }
        // Bound the cache and expire device draft blobs.
        if now.duration_since(self.last_evict) >= Duration::from_secs(60) {
            self.last_evict = now;
            let protected: Vec<Key> = self.open.keys().cloned().collect();
            let uploading = self.uploads.values().filter_map(|u| u.blob_id.clone()).collect();
            let cap = self.cfg.cache_cap_bytes;
            self.worker.submit(move |s| {
                let _ = s.relay.purge_finished();
                let blobs = s.drafts.expire_device_blobs(&uploading);
                let cache = s.cache.evict(cap, &protected);
                blobs.err().map(|e| e.to_string()).or_else(|| cache.err().map(|e| e.to_string())).map(CoreEvent::StoreError)
            });
        }
        // Lifecycle.
        let busy = self.busy();
        let idle = busy == BusyReasons::default();
        if self.settings.hub.always || !idle {
            self.idle_since = None;
            return;
        }
        let since = *self.idle_since.get_or_insert(now);
        let limit = self.cfg.idle_exit.unwrap_or(Duration::from_secs(self.settings.hub.idle_exit_secs));
        if now.duration_since(since) >= limit {
            log("idle: exiting");
            self.exit = Some(ExitReason::Idle);
        }
    }

    fn send_acks(&mut self) {
        let mut acks: Vec<(TargetId, ThreadAckParams)> = Vec::new();
        for (id, t) in &mut self.targets {
            let Some(conn) = t.conn.as_mut().filter(|c| c.ready) else { continue };
            for (thread, sub) in &mut conn.subs {
                let Some(p) = self.committed.get(&(id.clone(), thread.clone())).copied() else { continue };
                if sub.phase == Phase::Live && sub.epoch == Some(p.epoch) && sub.acked != Some(p) {
                    sub.acked = Some(p);
                    acks.push((id.clone(), ThreadAckParams { thread_id: thread.clone(), epoch: p.epoch, seq: p.seq }));
                }
            }
        }
        for (id, params) in acks {
            self.send::<ThreadAck>(&id, &params, Pending::Ignore);
            self.flush(&id);
        }
    }
}

/// Answers a window's `thread/subscribe` relative to what the cache holds.
/// A turn is under way in this state (running, blocked on the user, or waiting on another
/// agent); its end is the move to any other state.
fn turn_going(state: &ThreadState) -> bool {
    matches!(state, ThreadState::Running | ThreadState::NeedsAttention | ThreadState::Waiting)
}

fn window_answer(
    subscribed: Option<&ThreadSubscribeResult>,
    position: Option<JournalPosition>,
    epoch: Option<Epoch>,
    request: &ThreadSubscribeParams,
) -> ThreadSubscribeResult {
    let position = position.unwrap_or(JournalPosition { epoch: epoch.unwrap_or_default(), seq: Seq(0) });
    ThreadSubscribeResult {
        mode: window_mode(position, request),
        control: subscribed.map_or(Control::Free, |s| s.control.clone()),
        control_holder: subscribed.and_then(|s| s.control_holder.clone()),
        take_over_blocked: subscribed.and_then(|s| s.take_over_blocked.clone()),
        active_turn: subscribed.and_then(|s| s.active_turn.clone()),
        effective: subscribed.and_then(|s| s.effective.clone()),
        context: subscribed.and_then(|s| s.context.clone()),
        tokens: subscribed.and_then(|s| s.tokens.clone()),
        queued: subscribed.and_then(|s| s.queued.clone()),
        hold: subscribed.and_then(|s| s.hold.clone()),
    }
}

fn window_mode(position: JournalPosition, request: &ThreadSubscribeParams) -> SubscribeMode {
    if request.epoch == Some(position.epoch) && request.after_seq == Some(position.seq) {
        SubscribeMode::Replay(SubscribeReplay { epoch: position.epoch, from_seq: Seq(position.seq.0 + 1), head_seq: position.seq })
    } else {
        SubscribeMode::Reset(SubscribeReset { epoch: position.epoch, head_seq: position.seq })
    }
}

/// `thread/read` served from the cache while the target is offline: pages newest first,
/// bounded by bytes, all valid at the cached position.
fn read_from_cache(cache: &mut crate::cache::Cache, target: &TargetId, p: &ThreadReadParams) -> Result<Value, String> {
    let cached = cache.thread(target, &p.thread_id).map_err(|e| e.to_string())?;
    let Some(cached) = cached else { return Err("the thread is not cached".into()) };
    let cursor = p.cursor.as_ref().map(PageCursor::as_str);
    let interactive = p.around.is_some() || cursor.is_some_and(|c| c.starts_with("history:") || c.starts_with("history-newer:"))
        || (cursor.is_none() && p.max_items.is_some());
    let first_page = cursor.is_none() && p.around.is_none();
    let position = cached.position;
    let context = cursor.is_none().then(|| cached.live.context.clone()).flatten();
    if interactive {
        let yaac_protocol::CachedHistoryPage { items, older: cursor, newer: newer_cursor, continues_turn } =
            yaac_protocol::cached_history_page(&cached.items, p)?;
        let prompts = if first_page {
            let spawned = cache.thread_summary(target, &p.thread_id).map_err(|e| e.to_string())?.is_some_and(|s| s.spawned_by.is_some());
            Some(yaac_protocol::TimelinePrompt::index(&cached.items, spawned))
        } else {
            None
        };
        let required_items = p.cursor.is_none().then(|| {
            cached.items.iter().filter(|item|
                matches!(&item.body, ItemBody::Request(request) if matches!(request.state, RequestState::Pending | RequestState::Submitted))
                    && !items.iter().any(|present| present.id == item.id)
            ).cloned().collect::<Vec<_>>()
        }).filter(|items| !items.is_empty());
        return Ok(to_value(&ThreadReadResult { items, position, cursor, newer_cursor, continues_turn: continues_turn.then_some(true), prompts, required_items, uploads: vec![], context }));
    }

    let skip = cursor.and_then(|c| c.strip_prefix("hubcache:")).and_then(|n| n.parse().ok()).unwrap_or(0);
    let max = p.max_bytes.unwrap_or(64 * 1024).max(1);
    let mut items = Vec::new();
    let mut bytes = 0u64;
    for item in cached.items.iter().rev().skip(skip) {
        let len = serde_json::to_vec(item).map_or(0, |v| v.len() as u64);
        if !items.is_empty() && bytes + len > max { break; }
        bytes += len;
        items.push(item.clone());
    }
    let next = skip + items.len();
    let cursor = (next < cached.items.len()).then(|| PageCursor::new(format!("hubcache:{next}")));
    Ok(to_value(&ThreadReadResult { items, position, cursor, newer_cursor: None, continues_turn: None, prompts: None, required_items: None, uploads: vec![], context }))
}

#[cfg(test)]
mod build_tests {
    use std::cmp::Ordering as O;

    use super::{compare_builds, daemon_build_older, hands_over_to};

    #[test]
    fn numbered_builds_order_by_their_number() {
        assert_eq!(compare_builds("0.1.0+42", "0.1.0+42"), O::Equal);
        assert_eq!(compare_builds("0.1.0+43", "0.1.0+42"), O::Greater, "a newer window replaces the hub");
        assert_eq!(compare_builds("0.1.0+41", "0.1.0+42"), O::Less, "an older window is refused");
        // The version still comes first.
        assert_eq!(compare_builds("0.2.0+1", "0.1.0+99"), O::Greater);
        assert_eq!(compare_builds("0.1.0+99", "0.2.0+1"), O::Less);
        // Development builds (and builds from before ids) have no order: the window wins.
        assert_eq!(compare_builds("0.1.0+dev.ab12", "0.1.0+dev.cd34"), O::Greater);
        assert_eq!(compare_builds("0.1.0+dev.ab12", "0.1.0+42"), O::Greater);
        assert_eq!(compare_builds("0.1.0+42", "0.1.0"), O::Greater);
    }

    #[test]
    fn a_daemon_is_older_only_by_a_lower_build_number() {
        // The daemon's own version may differ from the app's; the build number is shared.
        assert!(daemon_build_older("0.1.0+41", "0.1.0+42"));
        assert!(daemon_build_older("0.3.0+41", "0.1.0+42"));
        assert!(!daemon_build_older("0.1.0+42", "0.1.0+42"));
        assert!(!daemon_build_older("0.1.0+43", "0.1.0+42"));
        assert!(daemon_build_older("0.1.0+dev", "0.1.0+42"), "a release replaces a development daemon (P94)");
        assert!(!daemon_build_older("0.1.0+41", "0.1.0+dev.ab12"));
    }

    #[test]
    fn a_daemon_is_handed_over_only_to_a_newer_numbered_build() {
        // The connector names its install folder, `<version>+<id>-<sha16>`.
        assert!(hands_over_to("0.1.0+41", Some("0.1.0+42-0123456789abcdef")));
        assert!(hands_over_to("0.1.0+9", Some("0.1.0+10-0123456789abcdef")), "numbers, not text");
        assert!(!hands_over_to("0.1.0+42", Some("0.1.0+42-0123456789abcdef")), "the same build");
        assert!(!hands_over_to("0.1.0+43", Some("0.1.0+42-0123456789abcdef")), "an older bundle");
        assert!(!hands_over_to("0.1.0+41", None), "no bundle");
        assert!(!hands_over_to("0.1.0+41", Some("0.1.0-0123456789abcdef")), "a development bundle");
        assert!(hands_over_to("0.1.0+dev", Some("0.1.0+42-0123456789abcdef")), "a development daemon gives way to a release (P94)");
    }
}
