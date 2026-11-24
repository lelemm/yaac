//! A fake `yaacd` in Rust, speaking the YAAC protocol well enough to exercise the hub without
//! the TypeScript daemon: `initialize`, the thread index, a journal per thread with scripted
//! events, replay from a position, `reset` with a paged, gap-free snapshot, acks, and the mesh
//! calls (`mesh/deliver`, `mesh/receipt`, `mesh/roster`, `mesh/query`, and `mesh/relay`
//! originated by the daemon).
//!
//! Its state outlives connections, as a real daemon's does: kill the connection, keep
//! emitting, and the journal holds what the hub missed.

#![allow(dead_code, unreachable_pub)]

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use yaac_protocol::dialect::{self, CURRENT, negotiate};
use yaac_protocol::frame::{Frame, FrameDecoder, encode_message_frame};
use yaac_protocol::methods::names;
use yaac_protocol::rpc::{ErrorCode, ErrorObject, Message, RpcId};
use yaac_protocol::{
    AgentMessageItem, Control, DaemonInfo, DeltaField, Epoch, InitializeParams, InitializeResult, InstanceId, Item,
    ItemBody, ItemDeltaEvent, ItemEvent, ItemStatus, ItemUpdatedEvent, JournalPosition, ListThreadsReply,
    MeshDeliverParams, MeshMessage, MeshQueryResult, MeshReceiptParams, MeshRosterParams, OriginKind, Receipt,
    ReceiptStatus, Seq, SubscribeMode, SubscribeReplay, SubscribeReset, TargetFacts, TargetPath, ThreadAckParams,
    ThreadEvent, ThreadEventParams, ThreadId, ThreadReadParams, ThreadReadResult, ThreadState, ThreadSubscribeParams,
    ThreadSubscribeResult, ThreadSummary, ThreadSyncResult, Timestamp, TurnCompletedEvent, TurnId, TurnOrigin,
    TurnStartedEvent, TurnStatus,
};
use yaac_transport::{BoxFuture, ChildGuard, ConnectCtx, ConnectError, Connector, Stream};

/// How a `thread/subscribe` was answered (for assertions).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Subscribed {
    /// Replay from `from` (the client's `afterSeq + 1`).
    Replay { epoch: u64, from: u64 },
    /// Reset (no position, another epoch, or before the journal's start).
    Reset { epoch: u64 },
}

#[derive(Debug)]
struct FakeThread {
    summary: ThreadSummary,
    epoch: Epoch,
    /// Events of the current epoch; `journal[i]` has seq `i + 1`.
    journal: Vec<ThreadEventParams>,
    /// The thread's items as the harness would report them (oldest first).
    items: Vec<Item>,
}

#[derive(Debug)]
struct Conn {
    tx: mpsc::UnboundedSender<Vec<u8>>,
    subs: HashSet<ThreadId>,
    abort: Option<tokio::task::AbortHandle>,
    snapshots: HashMap<u64, (Vec<Item>, JournalPosition)>,
}

#[derive(Debug, Default)]
struct State {
    threads: BTreeMap<ThreadId, FakeThread>,
    index_changes: u64,
    conns: BTreeMap<u64, Conn>,
    next_conn: u64,
    next_snapshot: u64,
    next_peer_id: i64,
    offline: bool,
    connects: u64,
    subscribes: Vec<(ThreadId, Subscribed)>,
    acks: Vec<ThreadAckParams>,
    deliver_calls: Vec<MeshMessage>,
    mailbox: Vec<MeshMessage>,
    receipts: Vec<MeshReceiptParams>,
    rosters: Vec<MeshRosterParams>,
    queries: u64,
    peer_answers: HashMap<i64, Result<Value, ErrorObject>>,
    page_items: usize,
    deliver_fails: bool,
    projects: Vec<yaac_protocol::Project>,
    usage_reports: Vec<yaac_protocol::MeshUsageParams>,
    usage_exhausted: bool,
    /// Conversations beyond the discovery caps: not in the index, found by `thread/lookup`.
    hidden: Vec<ThreadSummary>,
    lookups: Vec<String>,
    caps: Option<yaac_protocol::DiscoveryCaps>,
    keeps: Vec<Vec<ThreadId>>,
    /// `provider/list`'s providers (default: none).
    providers: Vec<Value>,
    /// `provider/rateLimits`'s instances, and the `refresh` of each ask.
    rate_limits: Vec<Value>,
    rate_limit_asks: Vec<bool>,
}

/// A fake daemon. Cheap to clone; clones share state.
#[derive(Clone, Debug)]
pub struct FakeDaemon {
    name: String,
    state: Arc<Mutex<State>>,
}

fn frame(message: &Message) -> Vec<u8> {
    let mut out = Vec::new();
    encode_message_frame(&message.to_json(), &mut out).expect("frame");
    out
}

pub fn summary(id: &str) -> ThreadSummary {
    ThreadSummary {
        id: ThreadId::new(id),
        instance: InstanceId::new("fake"),
        native_id: Some(format!("native-{id}")),
        title: format!("thread {id}"),
        preview: String::new(),
        cwd: TargetPath::new("/work"),
        project: None,
        worktree: None,
        repo: None,
        git: None,
        branch: None,
        model: None,
        state: ThreadState::Idle,
        control: Control::Free,
        control_holder: None,
        forked_from: None,
        spawned_by: None,
        root: None,
        children: 0,
        peers: 0,
        origin_kind: OriginKind::Yaac,
        created_at: Timestamp::new("2026-01-01T00:00:00Z"),
        updated_at: Timestamp::new("2026-01-01T00:00:00Z"),
        archived: false,
        agents: yaac_protocol::AgentsPolicy::Ask,
        hold: None,
        fast: false,
        ultrafast: None,
        placement: None,
        subagent: None,
        unavailable: None,
        running_commands: None,
    }
}

pub fn agent_item(id: &str, turn: &str, text: &str, status: ItemStatus) -> Item {
    Item {
        id: yaac_protocol::ItemId::new(id),
        turn_id: TurnId::new(turn),
        status,
        started_at: None,
        completed_at: None,
        body: ItemBody::AgentMessage(AgentMessageItem { text: text.to_owned() }),
    }
}

impl FakeDaemon {
    pub fn new(name: &str) -> Self {
        let state = State { page_items: 2, ..State::default() };
        Self { name: name.to_owned(), state: Arc::new(Mutex::new(state)) }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().expect("fake state")
    }

    // ── Scripting ──────────────────────────────────────────────────────────────────────

    /// Adds a thread (idle, empty).
    pub fn add_thread(&self, id: &str) {
        let mut s = self.lock();
        s.index_changes += 1;
        s.threads.insert(
            ThreadId::new(id),
            FakeThread { summary: summary(id), epoch: Epoch(1), journal: Vec::new(), items: Vec::new() },
        );
    }

    /// Appends one event to the thread's journal and sends it to subscribed connections.
    /// Returns its seq.
    pub fn emit(&self, thread: &str, event: ThreadEvent) -> u64 {
        let mut s = self.lock();
        let id = ThreadId::new(thread);
        let t = s.threads.get_mut(&id).expect("thread");
        let seq = t.journal.len() as u64 + 1;
        let params = ThreadEventParams { thread_id: id.clone(), epoch: t.epoch, seq: Seq(seq), event };
        apply(&mut t.items, &params.event);
        t.journal.push(params.clone());
        let message = Message::notification(names::THREAD_EVENT, serde_json::to_value(&params).unwrap());
        let bytes = frame(&message);
        for conn in s.conns.values() {
            if conn.subs.contains(&id) {
                let _ = conn.tx.send(bytes.clone());
            }
        }
        seq
    }

    /// A whole turn: started, an agent message streamed in `deltas` pieces, completed.
    /// Returns the message text.
    pub fn turn(&self, thread: &str, turn: &str, deltas: usize) -> String {
        self.emit(thread, ThreadEvent::TurnStarted(TurnStartedEvent { turn_id: TurnId::new(turn), origin: TurnOrigin::User, at: None }));
        let item = format!("{turn}-msg");
        self.emit(thread, ThreadEvent::ItemStarted(ItemEvent { item: agent_item(&item, turn, "", ItemStatus::InProgress) }));
        let mut text = String::new();
        for i in 0..deltas {
            let delta = format!("{turn}.{i};");
            text.push_str(&delta);
            self.emit(
                thread,
                ThreadEvent::ItemDelta(ItemDeltaEvent { item_id: yaac_protocol::ItemId::new(&item), field: DeltaField::Text, delta }),
            );
        }
        self.emit(thread, ThreadEvent::ItemCompleted(ItemEvent { item: agent_item(&item, turn, &text, ItemStatus::Completed) }));
        self.emit(
            thread,
            ThreadEvent::TurnCompleted(TurnCompletedEvent {
                turn_id: TurnId::new(turn),
                status: TurnStatus::Completed,
                usage: None,
                error: None,
                at: None,
            }),
        );
        text
    }

    /// Starts a new journal for the thread (as after corruption or an unmigratable state
    /// change): old positions get a `reset`.
    pub fn new_epoch(&self, thread: &str) {
        let mut s = self.lock();
        let t = s.threads.get_mut(&ThreadId::new(thread)).expect("thread");
        t.epoch = Epoch(t.epoch.0 + 1);
        t.journal.clear();
    }

    /// The thread's head position.
    pub fn head(&self, thread: &str) -> JournalPosition {
        let s = self.lock();
        let t = &s.threads[&ThreadId::new(thread)];
        JournalPosition { epoch: t.epoch, seq: Seq(t.journal.len() as u64) }
    }

    /// The thread's items as the harness has them.
    pub fn items(&self, thread: &str) -> Vec<Item> {
        self.lock().threads[&ThreadId::new(thread)].items.clone()
    }

    /// Drops every live connection, mid-stream (as a dying link would).
    pub fn kill_connections(&self) {
        let mut s = self.lock();
        for (_, conn) in std::mem::take(&mut s.conns) {
            if let Some(abort) = conn.abort {
                abort.abort();
            }
        }
    }

    /// While offline, every connect fails and live connections are dropped.
    pub fn set_offline(&self, offline: bool) {
        self.lock().offline = offline;
        if offline {
            self.kill_connections();
        }
    }

    pub fn set_deliver_fails(&self, fails: bool) {
        self.lock().deliver_fails = fails;
    }

    pub fn connected(&self) -> bool {
        !self.lock().conns.is_empty()
    }

    pub fn connects(&self) -> u64 {
        self.lock().connects
    }

    pub fn subscribes(&self) -> Vec<(ThreadId, Subscribed)> {
        self.lock().subscribes.clone()
    }

    pub fn acks(&self) -> Vec<ThreadAckParams> {
        self.lock().acks.clone()
    }

    pub fn deliver_calls(&self) -> Vec<MeshMessage> {
        self.lock().deliver_calls.clone()
    }

    pub fn mailbox(&self) -> Vec<MeshMessage> {
        self.lock().mailbox.clone()
    }

    pub fn receipts(&self) -> Vec<MeshReceiptParams> {
        self.lock().receipts.clone()
    }

    pub fn rosters(&self) -> Vec<MeshRosterParams> {
        self.lock().rosters.clone()
    }

    pub fn set_projects(&self, projects: Vec<yaac_protocol::Project>) {
        self.lock().projects = projects;
    }

    /// What `provider/list` answers.
    pub fn set_providers(&self, providers: Vec<Value>) {
        self.lock().providers = providers;
    }

    /// What `provider/rateLimits` answers.
    pub fn set_rate_limits(&self, instances: Vec<Value>) {
        self.lock().rate_limits = instances;
    }

    /// The `refresh` flag of each `provider/rateLimits` asked.
    pub fn rate_limit_asks(&self) -> Vec<bool> {
        self.lock().rate_limit_asks.clone()
    }

    /// Sends `project/updated` on every connection.
    pub fn originate_projects_updated(&self) {
        let s = self.lock();
        let params = json!({ "projects": s.projects });
        for c in s.conns.values() {
            let _ = c.tx.send(frame(&Message::notification(names::PROJECT_UPDATED, params.clone())));
        }
    }

    pub fn usage_reports(&self) -> Vec<yaac_protocol::MeshUsageParams> {
        self.lock().usage_reports.clone()
    }

    pub fn set_usage_exhausted(&self, exhausted: bool) {
        self.lock().usage_exhausted = exhausted;
    }

    pub fn queries(&self) -> u64 {
        self.lock().queries
    }

    /// A conversation beyond the discovery caps: not listed, found only by `thread/lookup`.
    pub fn hide(&self, summary: ThreadSummary) {
        self.lock().hidden.push(summary);
    }

    /// The ids `thread/lookup` was asked for.
    pub fn lookups(&self) -> Vec<String> {
        self.lock().lookups.clone()
    }

    /// Every `thread/keep` received, in order.
    pub fn keeps(&self) -> Vec<Vec<ThreadId>> {
        self.lock().keeps.clone()
    }

    /// Removes a thread and pushes `thread/removed` to live connections.
    pub fn remove_thread(&self, id: &str) {
        let mut s = self.lock();
        s.threads.remove(&ThreadId::new(id));
        s.index_changes += 1;
        let token = yaac_protocol::SyncToken::new(format!("fake:{}", s.index_changes));
        let bytes = frame(&Message::notification(names::THREAD_REMOVED, json!({ "threadId": id, "token": token })));
        for conn in s.conns.values() {
            let _ = conn.tx.send(bytes.clone());
        }
    }

    /// The caps `discovery/caps` answers with.
    pub fn set_caps(&self, caps: yaac_protocol::DiscoveryCaps) {
        self.lock().caps = Some(caps);
    }

    /// Pushes a notification to live connections.
    pub fn originate_notification(&self, method: &str, params: Value) {
        let s = self.lock();
        let bytes = frame(&Message::notification(method, params));
        for conn in s.conns.values() {
            let _ = conn.tx.send(bytes.clone());
        }
    }

    /// Changes a thread's summary and pushes `thread/updated` to live connections.
    pub fn originate_notification_thread_updated(&self, summary: &ThreadSummary) {
        let mut s = self.lock();
        s.index_changes += 1;
        let token = yaac_protocol::SyncToken::new(format!("fake:{}", s.index_changes));
        if let Some(t) = s.threads.get_mut(&summary.id) {
            t.summary = summary.clone();
        }
        let bytes = frame(&Message::notification(names::THREAD_UPDATED, json!({ "thread": summary, "token": token })));
        for conn in s.conns.values() {
            let _ = conn.tx.send(bytes.clone());
        }
    }

    /// Sends a daemon-originated request on the live connection; returns its id.
    pub fn originate(&self, method: &str, params: Value) -> Option<i64> {
        let mut s = self.lock();
        s.next_peer_id += 1;
        let id = s.next_peer_id;
        let bytes = frame(&Message::request(RpcId::Number(id), method, params));
        let conn = s.conns.values().next()?;
        conn.tx.send(bytes).ok()?;
        Some(id)
    }

    /// The hub's answer to a request this daemon originated.
    pub fn peer_answer(&self, id: i64) -> Option<Result<Value, ErrorObject>> {
        self.lock().peer_answers.get(&id).cloned()
    }

    /// `mesh/relay` as the daemon's outbox would send it.
    pub fn relay(&self, message: &MeshMessage) -> Option<i64> {
        self.originate(names::MESH_RELAY, json!({ "message": message }))
    }

    // ── Serving ────────────────────────────────────────────────────────────────────────

    fn open(&self) -> Result<Stream, ConnectError> {
        let (hub_side, daemon_side) = tokio::io::duplex(16 * 1024);
        let (tx, rx) = mpsc::unbounded_channel();
        let conn_id = {
            let mut s = self.lock();
            if s.offline {
                return Err(ConnectError::Other(format!("{} is offline", self.name)));
            }
            s.connects += 1;
            s.next_conn += 1;
            let id = s.next_conn;
            s.conns.insert(id, Conn { tx: tx.clone(), subs: HashSet::new(), abort: None, snapshots: HashMap::new() });
            id
        };
        let me = self.clone();
        let task = tokio::spawn(async move { me.serve(conn_id, daemon_side, tx, rx).await });
        if let Some(conn) = self.lock().conns.get_mut(&conn_id) {
            conn.abort = Some(task.abort_handle());
        }
        let (reader, writer) = tokio::io::split(hub_side);
        Ok(Stream { reader: Box::new(reader), writer: Box::new(writer), child: ChildGuard::none() })
    }

    async fn serve(
        self,
        conn_id: u64,
        stream: tokio::io::DuplexStream,
        tx: mpsc::UnboundedSender<Vec<u8>>,
        mut rx: mpsc::UnboundedReceiver<Vec<u8>>,
    ) {
        let (mut reader, mut writer) = tokio::io::split(stream);
        let mut decoder = FrameDecoder::new();
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            tokio::select! {
                read = reader.read(&mut buf) => {
                    let n = match read { Ok(0) | Err(_) => break, Ok(n) => n };
                    decoder.push(&buf[..n]);
                    while let Ok(Some(f)) = decoder.next_frame() {
                        if let Frame::Message(payload) = f
                            && let Ok(message) = Message::from_json(&payload)
                        {
                            self.on_message(conn_id, message, &tx);
                        }
                    }
                }
                out = rx.recv() => {
                    let Some(bytes) = out else { break };
                    if writer.write_all(&bytes).await.is_err() { break; }
                }
            }
        }
        self.lock().conns.remove(&conn_id);
    }

    fn on_message(&self, conn_id: u64, message: Message, tx: &mpsc::UnboundedSender<Vec<u8>>) {
        match message {
            Message::Request { id, method, params } => {
                let mut out = Vec::new();
                let outcome = self.on_request(conn_id, &method, params, &mut out);
                let reply = match outcome {
                    Ok(result) => Message::Response { id, result },
                    Err(error) => Message::Error { id: Some(id), error },
                };
                let _ = tx.send(frame(&reply));
                for m in out {
                    let _ = tx.send(frame(&m));
                }
            }
            Message::Response { id: RpcId::Number(n), result } => {
                self.lock().peer_answers.insert(n, Ok(result));
            }
            Message::Error { id: Some(RpcId::Number(n)), error } => {
                self.lock().peer_answers.insert(n, Err(error));
            }
            _ => {}
        }
    }

    /// Answers a request; `out` gets messages to send right after the answer (replayed
    /// events).
    fn on_request(&self, conn_id: u64, method: &str, params: Value, out: &mut Vec<Message>) -> Result<Value, ErrorObject> {
        let bad = |e: serde_json::Error| ErrorObject::new(ErrorCode::InvalidParams, e.to_string());
        let mut s = self.lock();
        match method {
            names::INITIALIZE => {
                let p: InitializeParams = serde_json::from_value(params).map_err(bad)?;
                let negotiated = negotiate(dialect::SUPPORTED, p.protocol).map_err(|e| e.to_error_object())?;
                let result = InitializeResult {
                    protocol: negotiated.answer(),
                    schema_hash: CURRENT.schema_hash().to_owned(),
                    daemon: DaemonInfo { build: format!("fake-{}", self.name), state_format: 1 },
                    target: TargetFacts {
                        os: "linux".into(),
                        arch: "x86_64".into(),
                        hostname: self.name.clone(),
                        user: "tester".into(),
                        home: TargetPath::new("/home/tester"),
                    },
                };
                Ok(serde_json::to_value(result).unwrap())
            }
            names::PING => Ok(json!({})),
            names::PROVIDER_LIST => Ok(json!({ "providers": s.providers })),
            names::PROVIDER_RATE_LIMITS => {
                s.rate_limit_asks.push(params["refresh"].as_bool().unwrap_or(false));
                Ok(json!({ "instances": s.rate_limits }))
            }
            names::THREAD_SYNC => {
                let changed: Vec<ThreadSummary> = s.threads.values().map(|t| t.summary.clone()).collect();
                let result = ThreadSyncResult {
                    changed,
                    removed: vec![],
                    token: yaac_protocol::SyncToken::new(format!("fake:{}", s.index_changes)),
                    more: false,
                    full: true,
                    stale_instances: vec![],
                };
                Ok(serde_json::to_value(result).unwrap())
            }
            names::THREAD_SUBSCRIBE => {
                let p: ThreadSubscribeParams = serde_json::from_value(params).map_err(bad)?;
                let t = s.threads.get(&p.thread_id).ok_or_else(|| ErrorObject::new(ErrorCode::ThreadNotFound, "no such thread"))?;
                let head = t.journal.len() as u64;
                let replay = p.epoch == Some(t.epoch) && p.after_seq.is_some_and(|a| a.0 <= head);
                let (mode, logged) = if replay {
                    let after = p.after_seq.map_or(0, |a| a.0);
                    for e in &t.journal[usize::try_from(after).unwrap()..] {
                        out.push(Message::notification(names::THREAD_EVENT, serde_json::to_value(e).unwrap()));
                    }
                    (
                        SubscribeMode::Replay(SubscribeReplay { epoch: t.epoch, from_seq: Seq(after + 1), head_seq: Seq(head) }),
                        Subscribed::Replay { epoch: t.epoch.0, from: after + 1 },
                    )
                } else {
                    (SubscribeMode::Reset(SubscribeReset { epoch: t.epoch, head_seq: Seq(head) }), Subscribed::Reset { epoch: t.epoch.0 })
                };
                let result = ThreadSubscribeResult {
                    mode,
                    control: Control::Free,
                    control_holder: None,
                    take_over_blocked: None,
                    active_turn: None,
                    effective: None,
                    context: None,
                    tokens: None,
                    queued: None,
                    hold: None,
                };
                s.subscribes.push((p.thread_id.clone(), logged));
                if let Some(conn) = s.conns.get_mut(&conn_id) {
                    conn.subs.insert(p.thread_id);
                }
                Ok(serde_json::to_value(result).unwrap())
            }
            names::THREAD_UNSUBSCRIBE => {
                let id = params.get("threadId").and_then(Value::as_str).map(ThreadId::new);
                if let (Some(id), Some(conn)) = (id, s.conns.get_mut(&conn_id)) {
                    conn.subs.remove(&id);
                }
                Ok(json!({}))
            }
            names::THREAD_READ => {
                let p: ThreadReadParams = serde_json::from_value(params).map_err(bad)?;
                let page = s.page_items;
                let (snap_id, offset) = match &p.cursor {
                    Some(c) => {
                        let (a, b) = c.as_str().split_once(':').unwrap();
                        (a.parse::<u64>().unwrap(), b.parse::<usize>().unwrap())
                    }
                    None => {
                        let t = s.threads.get(&p.thread_id).ok_or_else(|| ErrorObject::new(ErrorCode::ThreadNotFound, "no such thread"))?;
                        let snapshot = (t.items.iter().rev().cloned().collect::<Vec<_>>(), JournalPosition { epoch: t.epoch, seq: Seq(t.journal.len() as u64) });
                        s.next_snapshot += 1;
                        let id = s.next_snapshot;
                        s.conns.get_mut(&conn_id).unwrap().snapshots.insert(id, snapshot);
                        (id, 0)
                    }
                };
                let (items, position) = s.conns[&conn_id].snapshots[&snap_id].clone();
                let end = (offset + page).min(items.len());
                let cursor = (end < items.len()).then(|| yaac_protocol::PageCursor::new(format!("{snap_id}:{end}")));
                let result = ThreadReadResult { items: items[offset..end].to_vec(), position, cursor, newer_cursor: None, continues_turn: None, prompts: None, required_items: None, uploads: vec![], context: None };
                Ok(serde_json::to_value(result).unwrap())
            }
            names::THREAD_ACK => {
                let p: ThreadAckParams = serde_json::from_value(params).map_err(bad)?;
                s.acks.push(p);
                Ok(json!({}))
            }
            names::MESH_ROSTER => {
                s.rosters.push(serde_json::from_value(params).map_err(bad)?);
                Ok(json!({}))
            }
            names::MESH_RECEIPT => {
                s.receipts.push(serde_json::from_value(params).map_err(bad)?);
                Ok(json!({}))
            }
            names::MESH_DELIVER => {
                let p: MeshDeliverParams = serde_json::from_value(params).map_err(bad)?;
                if s.deliver_fails {
                    return Err(ErrorObject::new(ErrorCode::InternalError, "disk full"));
                }
                s.deliver_calls.push(p.message.clone());
                if !s.mailbox.iter().any(|m| m.id == p.message.id) {
                    s.mailbox.push(p.message);
                }
                let receipt = Receipt { status: ReceiptStatus::Delivered, reason: None, detail: None, handling: None };
                Ok(json!({ "receipt": receipt }))
            }
            names::PROJECT_LIST => Ok(json!({ "projects": s.projects })),
            names::THREAD_KEEP => {
                let p: yaac_protocol::ThreadKeepParams = serde_json::from_value(params).map_err(bad)?;
                s.keeps.push(p.thread_ids);
                Ok(json!({}))
            }
            names::DISCOVERY_CAPS => Ok(json!({ "caps": s.caps.clone().unwrap_or_default() })),
            names::THREAD_LOOKUP => {
                let p: yaac_protocol::ThreadLookupParams = serde_json::from_value(params).map_err(bad)?;
                s.lookups.push(p.id.clone());
                let found: Vec<ThreadSummary> =
                    s.hidden.iter().filter(|t| t.id.as_str() == p.id || t.native_id.as_deref() == Some(p.id.as_str())).cloned().collect();
                Ok(json!({ "threads": found }))
            }
            names::MESH_USAGE => {
                let p: yaac_protocol::MeshUsageParams = serde_json::from_value(params).map_err(bad)?;
                s.usage_reports.push(p);
                Ok(json!({ "exhausted": s.usage_exhausted }))
            }
            names::MESH_QUERY => {
                s.queries += 1;
                Ok(serde_json::to_value(MeshQueryResult::ListThreads(ListThreadsReply { threads: vec![], ..Default::default() })).unwrap())
            }
            other => Err(ErrorObject::new(ErrorCode::MethodNotFound, format!("fake daemon: unknown method {other}"))),
        }
    }
}

fn apply(items: &mut Vec<Item>, event: &ThreadEvent) {
    let upsert = |items: &mut Vec<Item>, item: &Item| match items.iter_mut().find(|i| i.id == item.id) {
        Some(existing) => *existing = item.clone(),
        None => items.push(item.clone()),
    };
    match event {
        ThreadEvent::ItemStarted(e) | ThreadEvent::ItemCompleted(e) => upsert(items, &e.item),
        ThreadEvent::ItemUpdated(ItemUpdatedEvent { item, .. }) => upsert(items, item),
        ThreadEvent::ItemDelta(d) => {
            if let Some(Item { body: ItemBody::AgentMessage(m), .. }) = items.iter_mut().find(|i| i.id == d.item_id) {
                m.text.push_str(&d.delta);
            }
        }
        _ => {}
    }
}

impl Connector for FakeDaemon {
    fn connect<'a>(&'a self, _ctx: &'a ConnectCtx) -> BoxFuture<'a, Result<Stream, ConnectError>> {
        Box::pin(async move { self.open() })
    }

    fn describe(&self) -> String {
        format!("fake daemon {}", self.name)
    }
}
