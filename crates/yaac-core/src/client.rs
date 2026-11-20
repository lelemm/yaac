//! The window's side of the attach protocol: turns user actions into hub messages, applies the
//! hub's answers and notifications to [`AppState`], and remembers what each outstanding
//! request was for. No I/O of its own: everything goes through a [`HubLink`].

use std::collections::HashMap;

use serde_json::{Value, json};
use yaac_protocol::attach::{self, HelloResult, RoutedNotification, RoutedRequest, TargetStateParams};
use yaac_protocol::methods::names as m;
use yaac_protocol::rpc::{ErrorCode, ErrorObject, Message, RpcId};
use yaac_protocol::{
    Address, ClientId, Control, InputPart, InstanceId, ItemBody, MeshBudgetAction, MeshBudgetResult, NoticeKind,
    OpId, ProviderListResult, ProviderUpdatedParams, RequestId, RequestResponse, RequestState, SubscribeMode,
    TargetId, ThreadEvent, ThreadEventParams, ThreadId, ThreadReadResult, ThreadRemovedParams, ThreadState,
    ThreadSubscribeResult, ThreadSummary, ThreadSyncResult, ThreadUpdatedParams, Timestamp, TurnId,
};

use crate::agents::root_of;
use crate::images::{ImageRoute, route};
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
    // SEAM (D92): the hub's view preferences — pin, mute (with `mutedUntil`), dismiss — kept
    // on the user's computer, not on the thread. The attach methods are being added to
    // `yaac-protocol` by the daemon/hub package; until they land, their names live here and
    // the demo hub answers them. Connect at merge: move these to `attach::names` and decode
    // with the protocol's own types.
    /// Every thread's view preferences.
    pub const VIEW_GET: &str = "hub/view/get";
    /// Change one thread's view preferences.
    pub const VIEW_SET: &str = "hub/view/set";
    /// View preferences changed (another window, or a timed mute the hub cleared).
    pub const VIEW_CHANGED: &str = "hub/view/changed";
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
    Hello,
    View,
    Targets,
    Providers(TargetId),
    Sync(TargetId),
    Subscribe(ThreadKey),
    Read(ThreadKey),
    Budget,
    Respond { key: ThreadKey, request_id: RequestId },
    TurnStart { key: ThreadKey, draft: Draft },
    ThreadStart { draft: NewThreadDraft },
    Fork { target: TargetId },
    ForkPreview { key: ThreadKey, instance: InstanceId },
    InstanceList(TargetId),
    InstanceChanged(TargetId),
    FsList { target: TargetId, path: String },
    HubSettings,
    Projects(TargetId),
    ProjectCreate { target: TargetId, for_draft: Option<u64> },
    Find { query: String },
    Shutdown,
    TargetChanged,
    Ignore,
}

/// A problem to show the user (as a notice, never a dialog).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientError {
    /// What it concerns.
    pub key: Option<ThreadKey>,
    /// Text.
    pub text: String,
}

/// The window's attach-protocol client.
pub struct Client {
    /// The state everything is drawn from.
    pub state: AppState,
    link: Box<dyn HubLink>,
    next_id: i64,
    pending: HashMap<RpcId, Call>,
    ops: u64,
    build: String,
    warm: Vec<ThreadKey>,
    /// Errors not yet shown.
    pub errors: Vec<ClientError>,
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
            build: build.into(),
            warm: Vec::new(),
            errors: Vec::new(),
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
        any
    }

    fn request(&mut self, method: &str, params: Value, call: Call) -> RpcId {
        let id = RpcId::Number(self.next_id);
        self.next_id += 1;
        self.pending.insert(id.clone(), call);
        self.link.send(Message::request(id.clone(), method, params));
        id
    }

    fn routed(&mut self, target: &TargetId, method: &str, params: Value, call: Call) {
        let env = RoutedRequest { target: target.clone(), method: method.to_owned(), params };
        let value = serde_json::to_value(env).unwrap_or(Value::Null);
        self.request(attach::names::REQUEST, value, call);
    }

    /// A fresh operation id.
    pub fn op_id(&mut self) -> OpId {
        self.ops += 1;
        let me = self.state.client_id.clone().unwrap_or_default();
        OpId::new(format!("op-{me}-{}", self.ops))
    }

    fn error(&mut self, key: Option<ThreadKey>, text: impl Into<String>) {
        self.errors.push(ClientError { key, text: text.into() });
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
                    self.on_error(call, &error);
                } else {
                    self.state.hub.last_error = Some(error.message);
                }
            }
            Message::Notification { method, params } => self.on_notification(&method, params),
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
                let ids: Vec<(TargetId, bool)> =
                    hello.targets.iter().map(|t| (t.id.clone(), t.state.status == attach::ConnectionStatus::Connected)).collect();
                self.state.set_targets(hello.targets);
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
            Call::InstanceList(t) => {
                if let Ok(r) = serde_json::from_value::<yaac_protocol::ProviderInstanceListResult>(result)
                    && let Some(rec) = self.state.targets.iter_mut().find(|x| x.entry.id == t)
                {
                    rec.instance_configs = Some(r.instances);
                }
            }
            Call::InstanceChanged(t) => {
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
            Call::View => self.state.apply_view(&result),
            Call::HubSettings => {
                self.state.hub.always = result.get("settings").and_then(|s| s.get("always")).and_then(Value::as_bool);
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
            Call::Sync(t) => {
                let Ok(r) = serde_json::from_value::<ThreadSyncResult>(result) else { return };
                let more = r.more;
                self.state.apply_sync(&t, r);
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
                let Ok(r) = serde_json::from_value::<ThreadSubscribeResult>(result) else { return };
                let Some(t) = self.state.transcripts.get_mut(&key) else { return };
                t.control = Some(r.control);
                t.control_holder = r.control_holder;
                t.active_turn = r.active_turn;
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
            Call::Read(key) => {
                let Ok(r) = serde_json::from_value::<ThreadReadResult>(result) else { return };
                let rev = self.state.next_stamp();
                let Some(t) = self.state.transcripts.get_mut(&key) else { return };
                if r.context.is_some() {
                    t.context = r.context;
                }
                let Some(load) = t.loading.as_mut() else { return };
                load.pages.extend(r.items);
                for u in r.uploads {
                    if !u.present {
                        load.missing.insert(u.upload_id);
                    }
                }
                if let Some(cursor) = r.cursor {
                    self.read_page(&key, Some(cursor.to_string()));
                    return;
                }
                let buffered = load.buffered.clone();
                t.finish_reset(r.position, rev);
                let items = t.items().to_vec();
                let stamp = self.state.next_stamp();
                let hints = self.state.hints.entry(key.clone()).or_default();
                if hints.position.is_none_or(|p| p.epoch != r.position.epoch || p.seq <= r.position.seq) {
                    hints.rebuild(&items, r.position, stamp);
                    for ev in &buffered {
                        hints.apply(ev, stamp);
                    }
                }
            }
            Call::Budget => {
                if let Ok(b) = serde_json::from_value::<MeshBudgetResult>(result) {
                    self.state.apply_budget(b);
                }
            }
            Call::Respond { .. } | Call::TurnStart { .. } | Call::Ignore => {}
            Call::ThreadStart { draft } => {
                let Some(thread_id) = result.get("threadId").and_then(Value::as_str).map(ThreadId::new) else { return };
                let key = ThreadKey::new(draft.target.clone(), thread_id.clone());
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
                        state: ThreadState::Running,
                        control: Control::Mine,
                        control_holder: None,
                        forked_from: None,
                        spawned_by: None,
                        root: None,
                        children: 0,
                        peers: 0,
                        origin_kind: yaac_protocol::OriginKind::Yaac,
                        created_at: now.clone(),
                        updated_at: now,
                        archived: false,
                        repo: None,
                        git: None,
                        muted: false,
                        pinned: false,
                        agents: yaac_protocol::AgentsPolicy::default(),
                        native_id: None,
                    };
                    self.state.upsert_summary(&draft.target, summary);
                }
                self.state.new_threads.retain(|d| d.id != draft.id);
                if self.state.selection == Selection::NewThread(draft.id) {
                    self.open(&key);
                }
            }
            Call::Fork { target } => {
                if let Some(thread_id) = result.get("threadId").and_then(Value::as_str) {
                    let key = ThreadKey::new(target, thread_id);
                    self.open(&key);
                }
            }
        }
    }

    fn on_error(&mut self, call: Call, error: &ErrorObject) {
        match call {
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
            Call::TurnStart { key, draft } => {
                if error.code == ErrorCode::OutcomeUnknown {
                    self.error(Some(key), "YAAC lost contact while sending your message. It may or may not have arrived; it will not be sent again.");
                } else {
                    // Nothing was sent: give the text back.
                    let slot = self.state.drafts.entry(key.clone()).or_default();
                    if slot.text.is_empty() {
                        *slot = draft;
                    }
                    self.error(Some(key), format!("Not sent: {}", error.message));
                }
            }
            Call::ThreadStart { draft } => {
                if !self.state.new_threads.iter().any(|d| d.id == draft.id) {
                    self.state.new_threads.push(draft);
                }
                self.error(None, format!("The thread was not started: {}", error.message));
            }
            Call::Subscribe(key) | Call::Read(key) => {
                if let Some(t) = self.state.transcripts.get_mut(&key) {
                    t.loading = None;
                }
                self.error(Some(key), format!("Could not load the thread: {}", error.message));
            }
            Call::Hello => {
                self.state.hub.outcome = match error.code.as_str() {
                    hub_names::WINDOW_OUTDATED => HelloOutcome::WindowOutdated { message: error.message.clone() },
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
            Call::HubSettings | Call::Projects(_) => {}
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
        match method {
            attach::names::TARGET_STATE => {
                let Ok(p) = serde_json::from_value::<TargetStateParams>(params) else { return };
                let was = self.state.target(&p.id).map(|t| t.connected());
                let id = p.id.clone();
                let now_connected = p.state.status == attach::ConnectionStatus::Connected;
                self.state.apply_target_state(p);
                if now_connected && was == Some(false) {
                    self.load_target(&id);
                }
                // The entry (enabled, transport) may have changed with the state: refresh it.
                self.request(attach::names::TARGETS_LIST, json!({}), Call::Targets);
            }
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
            hub_names::VIEW_CHANGED => self.state.apply_view(&params),
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
            _ => {}
        }
    }

    fn on_routed(&mut self, n: RoutedNotification) {
        let target = n.target;
        match n.method.as_str() {
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
        self.routed(target, m::PROVIDER_LIST, json!({}), Call::Providers(target.clone()));
        self.routed(target, m::PROJECT_LIST, json!({}), Call::Projects(target.clone()));
        self.sync(target);
    }

    fn sync(&mut self, target: &TargetId) {
        let token = self.state.target(target).map(|t| t.sync_token.clone()).unwrap_or_default();
        self.routed(target, m::THREAD_SYNC, json!({ "token": token }), Call::Sync(target.clone()));
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
        let mut params = json!({ "threadId": key.thread, "maxBytes": 262_144 });
        if let Some(c) = cursor {
            params["cursor"] = json!(c);
        }
        self.routed(&key.target, m::THREAD_READ, params, Call::Read(key.clone()));
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

    /// Open a thread: select it, keep its transcript, subscribe if it is not loaded.
    pub fn open(&mut self, key: &ThreadKey) {
        self.state.selection = Selection::Thread(key.clone());
        self.state.clear_arrival(key);
        let fresh = !self.state.transcripts.contains_key(key);
        if fresh {
            self.state.transcripts.insert(key.clone(), crate::transcript::Transcript::new());
        }
        self.state.touch(key);
        if fresh {
            self.subscribe(key);
        }
    }

    /// Re-subscribe from the current position to learn the latest state of everything —
    /// "Check latest status" on an outcome-unknown card. Sends nothing else.
    pub fn check_latest(&mut self, key: &ThreadKey) {
        self.subscribe(key);
    }

    /// Ask the hub to reconnect a target now.
    pub fn retry_target(&mut self, target: &TargetId) {
        let params = json!({ "id": target, "enabled": true });
        self.request(attach::names::TARGETS_ENABLE, params, Call::Ignore);
    }

    /// Answer a request. The card shows "submitted" at once; a lost answer becomes "outcome
    /// unknown" and is never re-sent.
    pub fn respond(&mut self, key: &ThreadKey, request_id: &RequestId, response: &RequestResponse, summary: String) {
        if !self.state.may_act(key) {
            return;
        }
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

    fn input_parts(draft: &Draft, support: &yaac_protocol::Support) -> Option<Vec<InputPart>> {
        let mut parts = Vec::new();
        if !draft.text.trim().is_empty() {
            parts.push(InputPart::text(draft.text.clone()));
        }
        for a in &draft.attachments {
            let upload = a.upload.clone()?;
            if a.progress.is_some() {
                return None;
            }
            match route(a.is_image, support, a.choice) {
                ImageRoute::AsImage => parts.push(InputPart::Image(yaac_protocol::ImagePart { upload_id: upload, name: Some(a.name.clone()), media_type: None })),
                ImageRoute::AsFile | ImageRoute::AsFileCannotSee => {
                    parts.push(InputPart::File(yaac_protocol::FilePart { upload_id: upload, name: Some(a.name.clone()), media_type: None }));
                }
                ImageRoute::Ask => return None,
            }
        }
        (!parts.is_empty()).then_some(parts)
    }

    /// Send the composer's content: a new turn, or input added to the running turn under
    /// `steer`. Returns whether anything was sent.
    pub fn send(&mut self, key: &ThreadKey) -> bool {
        if !self.state.may_act(key) {
            return false;
        }
        let Some(draft) = self.state.drafts.get(key).cloned() else { return false };
        let t = self.state.transcripts.get(key);
        let effective = t.and_then(|t| t.effective.as_ref());
        let support = effective.map_or(yaac_protocol::Support::Unknown, |e| e.image_input.clone());
        let Some(input) = Self::input_parts(&draft, &support) else { return false };
        let active: Option<TurnId> = t.and_then(|t| t.active_turn.clone());
        let can_steer = effective.is_some_and(|e| yaac_protocol::has_capability(&e.capabilities, &yaac_protocol::Capability::Steer));
        let op = self.op_id();
        let next = self.state.next_settings.get(key).cloned().unwrap_or_default();
        self.state.drafts.remove(key);
        match active {
            Some(turn) if can_steer => {
                let params = json!({ "opId": op, "threadId": key.thread, "turnId": turn, "input": input });
                self.routed(&key.target, m::TURN_STEER, params, Call::TurnStart { key: key.clone(), draft });
            }
            Some(_) => {
                self.state.drafts.insert(key.clone(), draft);
                return false;
            }
            None => {
                let mut params = json!({ "opId": op, "threadId": key.thread, "input": input });
                if let Some(mdl) = next.model {
                    params["model"] = json!(mdl);
                }
                if let Some(e) = next.effort {
                    params["effort"] = json!(e);
                }
                if let Some(md) = next.mode {
                    params["mode"] = json!(md);
                }
                self.routed(&key.target, m::TURN_START, params, Call::TurnStart { key: key.clone(), draft });
            }
        }
        true
    }

    /// Start a new-thread draft on a target and select it.
    pub fn new_thread(&mut self, target: TargetId, cwd: Option<yaac_protocol::TargetPath>) -> u64 {
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
            draft: Draft::default(),
        });
        self.state.selection = Selection::NewThread(id);
        id
    }

    /// Send a draft's first message: `thread/start` creates the thread and sends it, as one
    /// operation.
    pub fn start_thread(&mut self, draft_id: u64) -> bool {
        let Some(draft) = self.state.new_threads.iter().find(|d| d.id == draft_id).cloned() else { return false };
        let Some(target) = self.state.target(&draft.target) else { return false };
        let instance = draft.instance.clone().or_else(|| {
            target.providers.iter().find(|p| p.status == yaac_protocol::ProviderStatus::Ready).map(|p| p.instance.clone())
        });
        let Some(instance) = instance else { return false };
        let support = target
            .provider(&instance)
            .and_then(|p| {
                let model = draft.model.as_ref();
                p.models.iter().find(|mm| Some(&mm.id) == model).or_else(|| p.models.iter().find(|mm| mm.is_default))
            })
            .map_or(yaac_protocol::Support::Unknown, crate::images::model_image_support);
        let Some(input) = Self::input_parts(&draft.draft, &support) else { return false };
        let Some(cwd) = draft.cwd.clone() else { return false };
        let op = self.op_id();
        let mut params = json!({ "opId": op, "instance": instance, "cwd": cwd, "input": input });
        if let Some(p) = draft.project.as_ref().and_then(|pid| self.state.projects.iter().find(|p| &p.id == pid)) {
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
        let mut sent = draft.clone();
        sent.instance = Some(instance);
        self.routed(&draft.target, m::THREAD_START, params, Call::ThreadStart { draft: sent });
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
        self.request(hub_names::VIEW_SET, params, Call::Ignore);
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

    /// Choose a draft's project: it starts in the project folder's root.
    pub fn set_draft_project(&mut self, draft: u64, project_id: &str) {
        let Some(p) = self.state.projects.iter().find(|p| p.id == project_id).cloned() else { return };
        if let Some(d) = self.state.new_threads.iter_mut().find(|d| d.id == draft) {
            if d.target != p.target {
                d.instance = None;
                d.model = None;
                d.effort = None;
                d.mode = None;
            }
            d.target = p.target.clone();
            d.cwd = Some(p.folder.clone().into());
            d.project = Some(p.id);
        }
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

    /// Fork a thread into a destination on the same target, from its last completed turn,
    /// with no first message yet (`thread/forkOnly`).
    pub fn fork(&mut self, key: &ThreadKey, choice: &crate::fork::ForkChoice) {
        let op = self.op_id();
        let mut params = json!({ "opId": op, "source": key.thread, "instance": choice.instance });
        if let Some(mdl) = &choice.model {
            params["model"] = json!(mdl);
        }
        self.routed(&key.target, m::THREAD_FORK_ONLY, params, Call::Fork { target: key.target.clone() });
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

    // ── hub, targets, providers (settings) ─────────────────────────────────────────────

    /// Ask an older hub to checkpoint and exit (the window is newer). Allowed right after a
    /// `hub/hello` that found another build.
    pub fn shutdown_hub(&mut self) {
        self.request(attach::names::SHUTDOWN, json!({}), Call::Shutdown);
    }

    /// Whether the hub keeps running and starts at login.
    pub fn set_always(&mut self, always: bool) {
        self.state.hub.always = Some(always);
        self.request(hub_names::SETTINGS_UPDATE, json!({ "always": always }), Call::HubSettings);
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
    pub fn add_instance(&mut self, target: &TargetId, kind: &yaac_protocol::ProviderKind, name: Option<&str>, socket: Option<&str>) {
        let op = self.op_id();
        let mut params = json!({ "opId": op, "kind": kind, "enabled": true, "backend": "auto" });
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
        if let Some(p) = self.state.mode_confirm.take() {
            self.apply_mode(p.target, p.mode);
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

#[derive(serde::Deserialize)]
struct FindHit {
    target: TargetId,
    thread: ThreadSummary,
}
