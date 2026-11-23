//! **Demo only.** A scripted, in-memory hub that plays the fleet story of the screen brief, so
//! the window runs with no daemon and no hub process (`yaac --demo`), and tests can drive the
//! whole client. The fictional provider and model names of the brief live in this module and
//! nowhere else.
//!
//! It answers the attach protocol the way a hub would (`hub/hello`, `hub/request` routed to a
//! target, `hub/notification` for events), keeps a journal per thread so subscribe / replay /
//! reset / read all behave, and can play *scenes* — the later moments of the story (screens 05,
//! 06, 19, 20, 23) — and stream replies tick by tick.

pub(crate) mod build;
mod diffs;
mod fleet;
mod forge;
mod long;
mod limits;
pub mod ssh;

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use serde_json::{Value, json};
use yaac_protocol::attach::{self, ConnectionState, ConnectionStatus, DaemonSummary, RoutedNotification, RoutedRequest, TargetStateParams};
use yaac_protocol::methods::names as m;
use yaac_protocol::rpc::{ErrorCode, ErrorObject, Message, RpcId};
use yaac_protocol::{
    AnsweredBy, ClientId, ContextUsage, Control, ForkKind, HandoffItem, InputPart, InstanceId, Item, ItemBody, ItemEvent,
    ItemStatus, ItemUpdatedEvent, MeshMessageKind, QueueChange, QueueChangedEvent, MeshPeer, NoticeKind, ReceiptReason, RequestChangedEvent,
    RequestResolution, RequestResponse, RequestState, Seq, SettingsChangedEvent, Support, TargetId, TargetPath,
    ThreadEvent, ThreadEventParams, ThreadId, ThreadState, ThreadSummary, ToolKind, TurnCompletedEvent, TurnId,
    TurnOrigin, TurnStartedEvent, TurnStatus, UsageUpdatedEvent,
};

use self::build::*;
use self::fleet::{PARSER, BENCH, BILLING, CI, FIX, Fleet, HANDSHAKE, INDEX, LINT, NOTES, ORCH, SEARCH, USERS, V2PLAN, k};
use crate::key::ThreadKey;
use crate::link::HubLink;
use crate::attachments::{DraftAttachment, Phase, Source, Upload};
use crate::state::AppState;

/// The client id the demo window uses, so "Answered by you" works.
pub const DEMO_CLIENT: &str = "demo-window";

/// Which world to start in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scenario {
    /// The fleet of the brief.
    Fleet,
    /// A fresh install: only this computer, no threads (screens 07–09).
    Fresh,
}

/// A later moment of the story.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scene {
    /// 05: every state a request can be in, on Fix frame decoder.
    RequestStates,
    /// 25: the orchestrator asks for permission to work with other agents.
    Permission,
    /// 19: an older daemon, a retrying target, a stopped distro, a cleaned attachment.
    Notices,
    /// 20: the orchestration ran out of budget.
    BudgetReached,
    /// 23/24: staging reconnects; the queued message is delivered and answered.
    StagingReconnects,
    /// 13: WSL connections on this PC (the Targets page).
    WslConnections,
    /// 17: Claude on this computer is signed out.
    LocalSignedOut,
    /// 74/75: Claude Code markup made readable: a paste, a command, notices (D141).
    HarnessMarkup,
    /// 100–103: three weeks of prompts on Try the new parser (D153, D154, D156).
    LongConversation,
    /// 90–93: agents talking to agents (D150): the orchestrator spawns, messages and waits
    /// through YAAC; users-service (Claude Code) runs its own subagents.
    AgentCalls,
    /// 95/96: Claude Code's own subagents (D151), listed under search-service beside its YAAC worker.
    HarnessSubagents,
    /// 120: billing-service (Claude Code) is working, and two messages wait in its queue (D166).
    /// The turn runs until it is stopped, sent to now, or [`DemoHub::finish_turn`].
    QueueWhileRunning,
    /// 130–133: the orchestrator with 30 related agents, most of them finished (D168).
    ManyAgents,
}

impl Scene {
    /// Every scene, for the demo menu.
    pub const ALL: [Scene; 13] = [
        Scene::RequestStates,
        Scene::Permission,
        Scene::Notices,
        Scene::BudgetReached,
        Scene::StagingReconnects,
        Scene::WslConnections,
        Scene::LocalSignedOut,
        Scene::HarnessMarkup,
        Scene::LongConversation,
        Scene::AgentCalls,
        Scene::HarnessSubagents,
        Scene::QueueWhileRunning,
        Scene::ManyAgents,
    ];

    /// Menu label.
    pub fn label(self) -> &'static str {
        match self {
            Scene::RequestStates => "Every request state (Fix frame decoder)",
            Scene::Permission => "The orchestrator asks to work with other agents",
            Scene::Notices => "Notices: old daemon, retrying, stopped distro",
            Scene::BudgetReached => "The orchestration runs out of budget",
            Scene::StagingReconnects => "staging reconnects; the queued message arrives",
            Scene::WslConnections => "WSL connections on this PC",
            Scene::LocalSignedOut => "Claude on this computer is signed out",
            Scene::HarnessMarkup => "A paste, a command, and notices from Claude Code (users-service)",
            Scene::LongConversation => "Three weeks of prompts (Try the new parser)",
            Scene::AgentCalls => "Agents talking to agents: spawn, message, wait, Claude Code subagents",
            Scene::HarnessSubagents => "Claude Code's own subagents under search-service",
            Scene::QueueWhileRunning => "billing-service is working; two messages wait in its queue",
            Scene::ManyAgents => "An orchestrator with 30 agents, most of them finished",
        }
    }
}

/// The thread keys of the fleet, for tests and the demo window.
pub mod threads {
    use super::{ThreadKey, fleet};
    /// Migrate auth module — orchestrator.
    pub fn orchestrator() -> ThreadKey { fleet::k(fleet::ORCH) }
    /// users-service.
    pub fn users() -> ThreadKey { fleet::k(fleet::USERS) }
    /// billing-service.
    pub fn billing() -> ThreadKey { fleet::k(fleet::BILLING) }
    /// search-service.
    pub fn search() -> ThreadKey { fleet::k(fleet::SEARCH) }
    /// index-rebuild.
    pub fn index() -> ThreadKey { fleet::k(fleet::INDEX) }
    /// CI integration.
    pub fn ci() -> ThreadKey { fleet::k(fleet::CI) }
    /// Release notes.
    pub fn notes() -> ThreadKey { fleet::k(fleet::NOTES) }
    /// Fix frame decoder.
    pub fn fix() -> ThreadKey { fleet::k(fleet::FIX) }
    /// Explain the handshake.
    pub fn handshake() -> ThreadKey { fleet::k(fleet::HANDSHAKE) }
    /// Try the new parser (a temporary project).
    pub fn parser() -> ThreadKey { fleet::k(fleet::PARSER) }
    /// Claude Code's Explore subagent of search-service, running (after `Scene::HarnessSubagents`, D151).
    pub fn subagent_explore() -> ThreadKey { fleet::k(fleet::SUB_EXPLORE) }
    /// Its general-purpose subagent, done.
    pub fn subagent_schema() -> ThreadKey { fleet::k(fleet::SUB_SCHEMA) }
}

#[allow(clippy::large_enum_variant, reason = "demo only; a handful are queued at a time")]
enum Pending {
    Event(ThreadKey, ThreadEvent),
    State(ThreadKey, ThreadState),
    /// A hub notification (upload progress), with the upload's handle.
    Note(String, Message),
}

struct Inner {
    always: bool,
    fleet: Fleet,
    ready: VecDeque<Message>,
    scheduled: VecDeque<(u32, Pending)>,
    counter: u64,
    /// The draft store: drafts by key, and blobs (`DraftBlob`) by id.
    drafts: std::collections::BTreeMap<String, Value>,
    blobs: std::collections::BTreeMap<String, Value>,
    /// Pull requests created in the demo (D120), by thread.
    created: Vec<(ThreadKey, yaac_protocol::PrSummary)>,
    /// The requests received, newest last (bounded): (target for a routed one, method, params).
    log: VecDeque<(Option<TargetId>, String, Value)>,
    /// What `hub/wsl/distros` answers: the distros, or an error message.
    distros: Result<Value, String>,
    /// The next routed request is lost on the way: answered `outcome_unknown`, not acted on.
    lose_next: bool,
}

/// The demo hub. Clone it to keep a handle for [`DemoHub::play`] and [`DemoHub::tick`] after
/// giving one to the client as its [`HubLink`].
#[derive(Clone)]
pub struct DemoHub {
    inner: Rc<RefCell<Inner>>,
}

impl std::fmt::Debug for DemoHub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DemoHub")
    }
}

impl HubLink for DemoHub {
    fn send(&mut self, message: Message) {
        self.inner.borrow_mut().handle(message);
    }

    fn poll(&mut self) -> Option<Message> {
        self.inner.borrow_mut().ready.pop_front()
    }
}

impl DemoHub {
    /// A hub for a scenario; `now` is the story's present (seconds since the epoch).
    pub fn new(scenario: Scenario, now: i64) -> Self {
        let fleet = match scenario {
            Scenario::Fleet => Fleet::standard(now),
            Scenario::Fresh => Fleet::fresh(now),
        };
        Self { inner: Rc::new(RefCell::new(Inner { always: true, fleet, ready: VecDeque::new(), scheduled: VecDeque::new(), counter: 0, drafts: Default::default(), blobs: Default::default(), created: Vec::new(), distros: Ok(demo_distros()), log: VecDeque::new(), lose_next: false })) }
    }

    /// Every thread of the scenario — the window's "recently used" set, so their hints are
    /// warm as if it had been open before.
    pub fn all_threads(&self) -> Vec<ThreadKey> {
        self.inner.borrow().fleet.threads.keys().cloned().collect()
    }

    /// The story's present.
    pub fn now(&self) -> i64 {
        self.inner.borrow().fleet.base
    }

    /// Play a scene: its events are queued as a hub would send them.
    pub fn play(&self, scene: Scene) {
        self.inner.borrow_mut().play(scene);
    }

    /// Release the next scheduled message (streaming). Returns whether one was released.
    pub fn tick(&self) -> bool {
        self.inner.borrow_mut().tick()
    }

    /// Something is scheduled.
    pub fn busy(&self) -> bool {
        !self.inner.borrow().scheduled.is_empty()
    }

    /// The running turn of `key` finishes (its answer, then the turn's end); as `yaacd` does,
    /// the first queued message is then delivered and starts the next turn (D166).
    pub fn finish_turn(&self, key: &ThreadKey) {
        let mut inner = self.inner.borrow_mut();
        let Some(turn) = inner.fleet.threads.get(key).and_then(|t| t.active_turn.clone()) else { return };
        inner.scheduled.retain(|(_, pend)| !matches!(pend, Pending::Event(k, _) | Pending::State(k, _) if k == key));
        let answer = format!("{turn}-done");
        inner.emit_now(key, ThreadEvent::ItemCompleted(ItemEvent { item: done(&answer, turn.as_str(), agent("Done: the retry is in and the tests pass.")) }));
        inner.emit_now(key, ThreadEvent::TurnCompleted(TurnCompletedEvent { turn_id: turn, status: TurnStatus::Completed, usage: None, error: None, at: None }));
        inner.turn_settled(key, ThreadState::Idle);
    }

    /// The next routed request is lost on the way (`outcome_unknown`), for tests of
    /// invariant 10.
    pub fn lose_next_answer(&self) {
        self.inner.borrow_mut().lose_next = true;
    }

    /// The running turn of `key` asks to run a command: an approval waits for the user.
    pub fn ask_approval(&self, key: &ThreadKey) {
        let mut inner = self.inner.borrow_mut();
        let Some(turn) = inner.fleet.threads.get(key).and_then(|t| t.active_turn.clone()) else { return };
        let body = approval(&format!("r-{turn}"), "cargo publish --dry-run", "/home/dev/billing", vec![]);
        inner.emit_now(key, ThreadEvent::ItemStarted(ItemEvent { item: item(&format!("{turn}-ask"), turn.as_str(), ItemStatus::InProgress, body) }));
        inner.set_state_now(key, ThreadState::NeedsAttention);
    }

    /// Release everything scheduled.
    pub fn drain(&self) {
        while self.tick() {}
    }

    /// The requests received so far, oldest first (at most the last 512): the target of a
    /// routed request (else `None`), the method, and its params. Tests read it.
    pub fn requests(&self) -> Vec<(Option<TargetId>, String, Value)> {
        self.inner.borrow().log.iter().cloned().collect()
    }

    /// The program holding `key` will not hand it over (`Some(reason)`), or will again
    /// (`None`) — D140, for the screens and tests.
    pub fn block_take_over(&self, key: &ThreadKey, reason: Option<&str>) {
        let mut inner = self.inner.borrow_mut();
        let Some(t) = inner.fleet.threads.get(key) else { return };
        let ev = yaac_protocol::ControlChangedEvent { control: t.control.clone(), holder: t.holder.clone(), take_over_blocked: reason.map(str::to_owned), active_turn: t.active_turn.clone() };
        inner.emit_now(key, ThreadEvent::ControlChanged(ev));
    }

    /// The root's target frees its budget on its own — a person acted on it, or another
    /// window chose Continue — and says so (`mesh/budget/changed`, D164). Tests read it.
    pub fn free_budget(&self, root: &yaac_protocol::Address) {
        let mut inner = self.inner.borrow_mut();
        let Some(b) = inner.fleet.budgets.get_mut(root) else { return };
        b.exhausted = false;
        b.held = Some(0);
        b.used.wall_clock_secs = 0;
        let b = b.clone();
        let Some(target) = root.target() else { return };
        inner.ready.push_back(routed(&target, m::MESH_BUDGET_CHANGED, json!(b)));
    }

    /// What `hub/wsl/distros` answers from now on: `{ distros }`, or a failure's message.
    pub fn set_wsl_distros(&self, answer: Result<Value, String>) {
        self.inner.borrow_mut().distros = answer;
    }

    /// Window-side data the protocol does not carry yet (the remaining seam): what YAAC did
    /// on each SSH server. Demo only.
    pub fn apply_seams(state: &mut AppState) {
        use crate::ssh_setup::SetupRecord;
        state.ssh_records.insert("devbox".into(), SetupRecord::InstalledByYaac { key: ssh::demo_line("devbox"), date: "2026-11-20".into() });
        state.ssh_records.insert("gpu-box".into(), SetupRecord::AlreadyInPlace { date: "2026-10-02".into() });
        state.ssh_records.insert("staging".into(), SetupRecord::InstalledByYaac { key: ssh::demo_line("staging"), date: "2026-11-02".into() });
        state.ssh_records.insert("legacy".into(), SetupRecord::NotSetUp);
    }

    /// Screen 26: mute search-service, users-service, and index-rebuild; unmute two others so
    /// the Muted section holds four.
    pub fn mute_scene() -> (Vec<ThreadKey>, Vec<ThreadKey>) {
        (vec![k(SEARCH), k(USERS), k(INDEX)], vec![k(BENCH), k(V2PLAN)])
    }

    /// Composer attachments for screen 19: an image the model cannot see (uploaded) and a
    /// file uploading at 62%.
    pub fn notice_attachments() -> Vec<DraftAttachment> {
        vec![Self::chip(1, "diagram.png", Some("image/png"), true), Self::uploading(2, "trace.log", 0.62)]
    }

    /// An uploaded chip (on devbox, the Fix frame decoder's target). `image` names a media
    /// type the hub would have sniffed.
    pub fn chip(id: u64, name: &str, media_type: Option<&str>, done: bool) -> DraftAttachment {
        let mut a = DraftAttachment::new(id, name, Some(Source::Path { path: format!("/home/dev/Pictures/{name}") }), media_type.map(str::to_owned));
        a.size = Some(48 * 1024);
        if done {
            a.upload = Upload::Done { target: TargetId::new("devbox"), upload_id: format!("u{id}/{name}").as_str().into() };
        }
        a
    }

    /// A chip whose upload is sending, `fraction` of the way.
    pub fn uploading(id: u64, name: &str, fraction: f64) -> DraftAttachment {
        let mut a = Self::chip(id, name, None, false);
        let total = 3_400_000u64;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss, reason = "demo numbers")]
        let sent = (total as f64 * fraction) as u64;
        a.upload = Upload::Running { handle: format!("hu_demo_{id}"), phase: Phase::Sending, sent, total };
        a
    }

    /// A chip whose image support is unknown, for the "send as image or file?" state.
    pub fn unknown_image() -> DraftAttachment {
        Self::chip(3, "sketch.png", Some("image/png"), true)
    }

    /// A chip whose upload failed, with the hub's message.
    pub fn failed(id: u64, name: &str, message: &str) -> DraftAttachment {
        let mut a = Self::chip(id, name, None, false);
        a.upload = Upload::Failed { message: message.into(), offline: false };
        a
    }
}

/// The demo PC's distros: Ubuntu is Windows' default and running; two more are stopped.
fn demo_distros() -> Value {
    json!({ "distros": [
        { "name": "Ubuntu", "default": true, "running": true, "version": 2 },
        { "name": "Debian", "default": false, "running": false, "version": 2 },
        { "name": "Ubuntu-24.04", "default": false, "running": false, "version": 2 },
    ] })
}

impl Inner {
    /// `thread/move` (D144) as a target answers it: the placement always; the folder too where
    /// the thread's provider has `threadMoveWorkingDirectory` (the demo's Codex instances).
    fn move_thread(&mut self, id: RpcId, key: &ThreadKey, p: &Value) {
        let project = p.get("project").and_then(Value::as_str).map(str::to_owned);
        let worktree = p.get("worktree").and_then(Value::as_str).map(str::to_owned);
        let found = project.as_ref().and_then(|pid| self.fleet.projects.get(&key.target).and_then(|ps| ps.iter().find(|x| x.id.as_str() == pid)).cloned());
        let caps = self.fleet.threads.get(key).map(|t| t.effective.capabilities.clone()).unwrap_or_default();
        let Some(t) = self.fleet.threads.get_mut(key) else {
            self.ready.push_back(error(id, ErrorCode::ThreadNotFound, "no such thread"));
            return;
        };
        if t.summary.archived {
            self.ready.push_back(error(id, ErrorCode::InvalidParams, "an archived thread cannot be moved; unarchive it first"));
            return;
        }
        if !t.control.may_act() {
            self.ready.push_back(error(id, ErrorCode::ThreadInUse, "this thread is in use elsewhere; it can only be watched"));
            return;
        }
        let mut working = None;
        match (project, found) {
            (None, _) => {
                // Forgotten: its folder decides again.
                t.summary.placement = None;
                self.fleet.resolve_projects(&key.target);
            }
            (Some(_), None) => {
                self.ready.push_back(error(id, ErrorCode::InvalidParams, "no such project on this target"));
                return;
            }
            (Some(_), Some(pr)) => {
                t.summary.project = Some(yaac_protocol::ProjectRef {
                    path: pr.folder.clone(),
                    display_name: pr.name.clone(),
                    cross_target_key: None,
                    kind: Some(yaac_protocol::ProjectKind::User),
                    id: Some(pr.id.clone()),
                    repo: pr.repo.clone(),
                });
                t.summary.placement = Some(yaac_protocol::ThreadPlacement {
                    project: pr.id.clone(),
                    worktree: worktree.clone().map(yaac_protocol::TargetPath::new),
                    worktree_name: worktree.as_deref().map(|w| w.rsplit('/').next().unwrap_or(w).to_owned()),
                });
                if caps.contains(&yaac_protocol::Capability::ThreadMoveWorkingDirectory) {
                    let folder = worktree.unwrap_or_else(|| pr.folder.to_string());
                    if t.summary.cwd.as_str() != folder {
                        t.summary.cwd = yaac_protocol::TargetPath::new(folder.clone());
                        working = Some(folder);
                    }
                }
            }
        }
        let Some(summary) = self.fleet.threads.get(key).map(|t| t.summary.clone()) else { return };
        let mut out = json!({ "thread": summary });
        if let Some(w) = working {
            out["workingDirectory"] = json!(w);
        }
        self.ready.push_back(result(id, out));
        self.ready.push_back(routed(&key.target, m::THREAD_UPDATED, json!({ "thread": summary, "token": "tok-2" })));
    }
}

fn result(id: RpcId, value: Value) -> Message {
    Message::Response { id, result: value }
}

fn error(id: RpcId, code: ErrorCode, text: &str) -> Message {
    Message::Error { id: Some(id), error: ErrorObject::new(code, text) }
}

fn routed(target: &TargetId, method: &str, params: Value) -> Message {
    let n = RoutedNotification { target: target.clone(), method: method.to_owned(), params };
    Message::notification(attach::names::NOTIFICATION, serde_json::to_value(n).unwrap_or(Value::Null))
}

impl Inner {
    fn next(&mut self, prefix: &str) -> String {
        self.counter += 1;
        format!("{prefix}-{}", self.counter)
    }

    fn handle(&mut self, msg: Message) {
        let Message::Request { id, method, params } = msg else { return };
        let logged = match serde_json::from_value::<RoutedRequest>(params.clone()) {
            Ok(r) if method == attach::names::REQUEST => (Some(r.target), r.method, r.params),
            _ => (None, method.clone(), params.clone()),
        };
        if self.log.len() >= 512 {
            self.log.pop_front();
        }
        self.log.push_back(logged);
        if self.lose_next && method == attach::names::REQUEST {
            self.lose_next = false;
            self.ready.push_back(error(id, ErrorCode::OutcomeUnknown, "the connection to the target was lost before it answered"));
            return;
        }
        match method.as_str() {
            attach::names::HELLO => {
                // The demo is always the window's own build.
                let build = params.get("build").cloned().unwrap_or_else(|| json!("demo"));
                let targets = self.fleet.targets.clone();
                self.ready.push_back(result(id, json!({ "build": build, "targets": targets })));
            }
            attach::names::SHUTDOWN | crate::client::hub_names::UI_STATE_SET => {
                self.ready.push_back(result(id, json!({})));
            }
            crate::client::hub_names::TARGETS_RECONNECT => {
                self.ready.push_back(result(id, json!({})));
                // Connecting to a stopped WSL distro boots it: the connection comes up.
                let tid = params.get("id").and_then(Value::as_str).unwrap_or("").to_owned();
                if self.fleet.targets.iter().any(|t| t.id.as_str() == tid && matches!(t.transport, attach::TargetTransport::Wsl(_))) {
                    self.target_state(&tid, ConnectionStatus::Connected, None, Some(true), None);
                }
            }
            crate::client::hub_names::CACHE_CLEAR => self.ready.push_back(result(id, json!({}))),
            attach::names::RATE_LIMITS_GET => self.ready.push_back(result(id, limits::accounts(&self.fleet))),
            crate::client::hub_names::WSL_DISTROS => match self.distros.clone() {
                Ok(v) => self.ready.push_back(result(id, v)),
                Err(m) => self.ready.push_back(error(id, ErrorCode::from_wire("wsl_failed"), &m)),
            },
            // SEAM (D92): the hub's view preferences; see `client::hub_names::VIEW_GET`.
            crate::client::hub_names::VIEW_GET => {
                let threads: Vec<Value> = self
                    .fleet
                    .view
                    .iter()
                    .map(|(k, v)| {
                        json!({
                            "target": k.target, "threadId": k.thread, "pinned": v.pinned, "muted": v.muted,
                            "mutedUntil": v.muted_until.map(crate::time::format_rfc3339), "dismissed": v.dismissed,
                            "unseen": v.unseen,
                        })
                    })
                    .collect();
                self.ready.push_back(result(id, json!({ "threads": threads })));
            }
            crate::client::hub_names::VIEW_SET => {
                self.set_view(&params);
                self.ready.push_back(result(id, json!({})));
            }
            crate::client::hub_names::VIEW_SET_MANY => {
                for e in params.get("entries").and_then(Value::as_array).cloned().unwrap_or_default() {
                    self.set_view(&e);
                }
                self.ready.push_back(result(id, json!({ "views": [] })));
            }
            crate::client::hub_names::VIEW_SHOWING => self.ready.push_back(result(id, json!({}))),
            crate::client::hub_names::THREADS_FIND => self.find(id, &params),
            crate::client::hub_names::SETTINGS_GET | crate::client::hub_names::SETTINGS_UPDATE => {
                if let Some(a) = params.get("always").and_then(Value::as_bool) {
                    self.always = a;
                }
                self.ready.push_back(result(id, json!({ "settings": { "always": self.always, "idleExitSecs": 300 } })));
            }
            attach::names::TARGETS_ADD => {
                let name = params.get("name").and_then(Value::as_str).unwrap_or("new").to_owned();
                let Ok(transport) = serde_json::from_value::<attach::TargetTransport>(params.get("transport").cloned().unwrap_or(Value::Null)) else {
                    self.ready.push_back(error(id, ErrorCode::InvalidParams, "bad transport"));
                    return;
                };
                if self.fleet.targets.iter().any(|t| t.name.as_str() == name) {
                    self.ready.push_back(error(id, ErrorCode::from_wire("name_taken"), "That name is already used."));
                    return;
                }
                let entry = attach::TargetEntry {
                    id: TargetId::new(name.clone()),
                    name: yaac_protocol::TargetName::new(name),
                    transport,
                    enabled: true,
                    state: ConnectionState { status: ConnectionStatus::Connected, detail: None, daemon: None, update: None },
                };
                self.fleet.targets.push(entry.clone());
                self.fleet.providers.entry(entry.id.clone()).or_insert_with(|| vec![fleet::codex("codex", None, yaac_protocol::ProviderStatus::Ready)]);
                self.ready.push_back(result(id, json!({ "target": entry })));
                self.targets_changed();
            }
            attach::names::TARGETS_REMOVE => {
                let target = params.get("id").and_then(Value::as_str).map(TargetId::new);
                self.fleet.targets.retain(|t| Some(&t.id) != target.as_ref());
                self.ready.push_back(result(id, json!({})));
                self.targets_changed();
            }
            attach::names::TARGETS_LIST => {
                let targets = self.fleet.targets.clone();
                self.ready.push_back(result(id, json!({ "targets": targets })));
            }
            attach::names::TARGETS_ENABLE => {
                let target = params.get("id").and_then(Value::as_str).map(TargetId::new);
                let enabled = params.get("enabled").and_then(Value::as_bool);
                if let (Some(e), Some(t)) = (enabled, self.fleet.targets.iter_mut().find(|t| Some(&t.id) == target.as_ref()))
                    && t.enabled != e
                {
                    t.enabled = e;
                    t.state.status = if e { ConnectionStatus::Connected } else { ConnectionStatus::Disabled };
                }
                // "Start it": connecting to a stopped WSL distro boots it.
                let boot = target.as_ref().and_then(|tid| self.fleet.targets.iter().find(|t| &t.id == tid)).is_some_and(|t| {
                    enabled == Some(true) && matches!(t.transport, attach::TargetTransport::Wsl(_)) && t.state.status != ConnectionStatus::Connected
                });
                if boot && let Some(t) = &target {
                    self.target_state(t.as_str(), ConnectionStatus::Connected, None, Some(true), None);
                }
                let entry = self.fleet.targets.iter().find(|t| Some(&t.id) == target.as_ref()).cloned();
                match entry {
                    Some(t) => self.ready.push_back(result(id, json!({ "target": t }))),
                    None => self.ready.push_back(error(id, ErrorCode::TargetUnknown, "no such target")),
                }
            }
            attach::names::REQUEST => {
                let Ok(req) = serde_json::from_value::<RoutedRequest>(params) else {
                    self.ready.push_back(error(id, ErrorCode::InvalidParams, "bad envelope"));
                    return;
                };
                self.routed_request(id, &req.target, &req.method, &req.params);
            }
            _ if method.starts_with("hub/upload/") || method.starts_with("hub/drafts/") => self.uploads(id, &method, &params),
            _ => self.ready.push_back(error(id, ErrorCode::MethodNotFound, &method)),
        }
    }

    /// One `hub/view/set` patch (also each entry of `hub/view/setMany`).
    fn set_view(&mut self, params: &Value) {
        if let (Some(t), Some(th)) = (params.get("target").and_then(Value::as_str), params.get("threadId").and_then(Value::as_str)) {
            let key = ThreadKey::new(TargetId::new(t), yaac_protocol::ThreadId::new(th));
            let v = self.fleet.view.entry(key).or_default();
            if let Some(x) = params.get("pinned").and_then(Value::as_bool) {
                v.pinned = x;
            }
            if let Some(x) = params.get("muted").and_then(Value::as_bool) {
                v.muted = x;
            }
            if let Some(x) = params.get("dismissed").and_then(Value::as_bool) {
                v.dismissed = x;
            }
            if let Some(x) = params.get("unseen").and_then(Value::as_bool) {
                v.unseen = x;
            }
            if let Some(u) = params.get("mutedUntil") {
                v.muted_until = u.as_str().and_then(crate::time::parse_rfc3339);
            }
        }
    }

    /// The hub's uploads and draft store, played: an upload hashes, waits, sends in four
    /// steps, and is verified, a tick at a time. The media type comes from the name (the real
    /// hub sniffs the content).
    fn uploads(&mut self, id: RpcId, method: &str, p: &Value) {
        use crate::attachments::names as n;
        let s = |k: &str| p.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
        match method {
            n::DRAFTS_LIST => {
                let drafts: Vec<Value> = self.drafts.values().cloned().collect();
                self.ready.push_back(result(id, json!({ "drafts": drafts })));
            }
            n::DRAFTS_PUT => {
                self.drafts.insert(s("key"), p.clone());
                self.ready.push_back(result(id, p.clone()));
            }
            n::DRAFTS_DELETE => {
                let key = s("key");
                self.drafts.remove(&key);
                self.blobs.retain(|_, b| b["draft"] != json!(key));
                self.ready.push_back(result(id, json!({})));
            }
            n::BLOB_BEGIN => {
                let blob = self.next("b");
                let mut b = p.clone();
                b["blobId"] = json!(blob);
                b["sha256"] = json!("0".repeat(64));
                self.blobs.insert(blob.clone(), b);
                self.ready.push_back(result(id, json!({ "blobId": blob })));
            }
            n::BLOB_APPEND => self.ready.push_back(result(id, json!({ "received": p.get("offset").and_then(Value::as_u64).unwrap_or(0) }))),
            n::BLOB_END => match self.blobs.get(&s("blobId")).cloned() {
                Some(b) => self.ready.push_back(result(id, b)),
                None => self.ready.push_back(error(id, ErrorCode::from_wire("blob_not_found"), "no stored attachment; attach it again")),
            },
            n::BLOB_DELETE => {
                self.blobs.remove(&s("blobId"));
                self.ready.push_back(result(id, json!({})));
            }
            n::UPLOAD_LIST => self.ready.push_back(result(id, json!({ "uploads": [] }))),
            n::UPLOAD_CANCEL => {
                let handle = s("handle");
                let before = self.scheduled.len();
                self.scheduled.retain(|(_, x)| !matches!(x, Pending::Note(h, _) if h == &handle));
                self.ready.push_back(result(id, json!({})));
                if self.scheduled.len() != before {
                    let done = json!({ "handle": handle, "target": s("target"), "name": "", "size": 0,
                        "error": { "code": n::UPLOAD_CANCELLED, "message": "the upload was cancelled" } });
                    self.ready.push_back(Message::notification(n::UPLOAD_DONE, done));
                }
            }
            n::UPLOAD_START => {
                let handle = self.next("hu");
                let name = Some(s("name")).filter(|x| !x.is_empty()).unwrap_or_else(|| s("path").rsplit('/').next().unwrap_or("file").to_owned());
                let blob = self.blobs.get(&s("blobId")).cloned();
                let size = blob.as_ref().and_then(|b| b.get("size").and_then(Value::as_u64)).unwrap_or(480 * 1024);
                let media = p.get("mediaType").and_then(Value::as_str).map(str::to_owned).or_else(|| {
                    let ext = name.rsplit('.').next().unwrap_or_default().to_ascii_lowercase();
                    match ext.as_str() {
                        "png" => Some("image/png"),
                        "jpg" | "jpeg" => Some("image/jpeg"),
                        "gif" => Some("image/gif"),
                        "webp" => Some("image/webp"),
                        _ => None,
                    }
                    .map(str::to_owned)
                });
                self.ready.push_back(result(id, json!({ "handle": handle })));
                let (target, draft) = (s("target"), p.get("draft").cloned().unwrap_or(Value::Null));
                let note = |phase: &str, sent: u64| {
                    Message::notification(n::UPLOAD_PROGRESS, json!({ "handle": handle, "target": target, "draft": draft, "phase": phase, "sent": sent, "total": size }))
                };
                let mut steps = vec![(2, note("hashing", 0)), (3, note("queued", 0))];
                for q in 1..=4u64 {
                    steps.push((4, note("sending", size * q / 4)));
                }
                steps.push((3, note("verifying", size)));
                let upload_id = self.next("u");
                steps.push((2, Message::notification(n::UPLOAD_DONE, json!({
                    "handle": handle, "target": target, "draft": draft, "uploadId": upload_id, "name": name, "size": size,
                    "mediaType": media, "skipped": false,
                }))));
                for (delay, m) in steps {
                    self.schedule(delay, Pending::Note(handle.clone(), m));
                }
            }
            _ => self.ready.push_back(error(id, ErrorCode::MethodNotFound, method)),
        }
    }

    fn routed_request(&mut self, id: RpcId, target: &TargetId, method: &str, p: &Value) {
        let thread = |p: &Value, field: &str| p.get(field).and_then(Value::as_str).map(|t| ThreadKey::new(target.clone(), t));
        match method {
            m::PROVIDER_LIST => {
                let providers = self.fleet.providers.get(target).cloned().unwrap_or_default();
                self.ready.push_back(result(id, json!({ "providers": providers })));
            }
            m::THREAD_SYNC => {
                let token = p.get("token").and_then(Value::as_str).unwrap_or("");
                let changed: Vec<ThreadSummary> = if token.is_empty() {
                    self.fleet.threads.iter().filter(|(k, _)| &k.target == target).map(|(_, t)| t.summary.clone()).collect()
                } else {
                    Vec::new()
                };
                self.ready.push_back(result(
                    id,
                    json!({ "changed": changed, "removed": [], "token": "tok-1", "more": false, "full": token.is_empty(), "staleInstances": [] }),
                ));
            }
            m::THREAD_SUBSCRIBE => {
                let Some(key) = thread(p, "threadId") else { return };
                let Some(t) = self.fleet.threads.get_mut(&key) else {
                    self.ready.push_back(error(id, ErrorCode::ThreadNotFound, "no such thread"));
                    return;
                };
                t.subscribed = true;
                let epoch = p.get("epoch").and_then(Value::as_u64);
                let after = p.get("afterSeq").and_then(Value::as_u64);
                let head = t.head();
                let mode = match (epoch, after) {
                    (Some(e), Some(a)) if e == t.epoch.0 && a <= head.0 => {
                        json!({ "mode": "replay", "epoch": t.epoch, "fromSeq": a + 1, "headSeq": head })
                    }
                    _ => json!({ "mode": "reset", "epoch": t.epoch, "headSeq": head }),
                };
                let replay: Vec<ThreadEventParams> = match (epoch, after) {
                    (Some(e), Some(a)) if e == t.epoch.0 => t.journal.iter().filter(|ev| ev.seq.0 > a).cloned().collect(),
                    _ => Vec::new(),
                };
                let mut res = mode;
                res["control"] = json!(t.control);
                if let Some(h) = &t.holder {
                    res["controlHolder"] = json!(h);
                }
                if let Some(b) = &t.take_over_blocked {
                    res["takeOverBlocked"] = json!(b);
                }
                if let Some(turn) = &t.active_turn {
                    res["activeTurn"] = json!(turn);
                }
                res["effective"] = json!(t.effective);
                if !t.queue.is_empty() {
                    res["queued"] = json!(t.queue);
                }
                let context = t.context.clone();
                let replaying = !replay.is_empty() || matches!((epoch, after), (Some(_), Some(_)));
                self.ready.push_back(result(id, res));
                for ev in replay {
                    self.ready.push_back(routed(target, m::THREAD_EVENT, json!(ev)));
                }
                if replaying && let Some(c) = context {
                    self.report_usage(&key, c);
                }
            }
            m::THREAD_READ => {
                let Some(key) = thread(p, "threadId") else { return };
                let Some(t) = self.fleet.threads.get(&key) else {
                    self.ready.push_back(error(id, ErrorCode::ThreadNotFound, "no such thread"));
                    return;
                };
                const PAGE: usize = 6;
                let start: usize = p.get("cursor").and_then(Value::as_str).and_then(|c| c.parse().ok()).unwrap_or(0);
                let newest_first: Vec<Item> = t.items.iter().rev().cloned().collect();
                let page: Vec<Item> = newest_first.iter().skip(start).take(PAGE).cloned().collect();
                let cursor = (start + PAGE < newest_first.len()).then(|| (start + PAGE).to_string());
                let mut uploads = Vec::new();
                for item in &page {
                    if let ItemBody::UserMessage(u) = &item.body {
                        for part in &u.parts {
                            let up = match part {
                                InputPart::Image(i) => &i.upload_id,
                                InputPart::File(f) => &f.upload_id,
                                _ => continue,
                            };
                            uploads.push(json!({ "uploadId": up, "present": !t.missing.contains(up) }));
                        }
                    }
                }
                let mut res = json!({ "items": page, "position": { "epoch": t.epoch, "seq": t.head() }, "uploads": uploads });
                let last = cursor.is_none();
                let context = t.context.clone();
                if let Some(c) = cursor {
                    res["cursor"] = json!(c);
                }
                self.ready.push_back(result(id, res));
                // Context usage is a live report, not part of a snapshot: report it after one.
                if last && let Some(c) = context {
                    self.report_usage(&key, c);
                }
            }
            m::THREAD_UNSUBSCRIBE | m::THREAD_ACK => self.ready.push_back(result(id, json!({}))),
            m::MESH_BUDGET => {
                let root = p.get("root").and_then(Value::as_str).map(|r| yaac_protocol::Address(r.to_owned()));
                let action = p.get("action").and_then(Value::as_str).unwrap_or("status");
                let Some(root) = root else { return };
                let Some(b) = self.fleet.budgets.get_mut(&root) else {
                    self.ready.push_back(error(id, ErrorCode::ThreadNotFound, "no orchestration with that root"));
                    return;
                };
                if action == "continue" {
                    b.allowance.agent_turns += 100;
                    b.exhausted = false;
                    b.held = Some(0);
                }
                let b = b.clone();
                self.ready.push_back(result(id, json!(b)));
            }
            m::REQUEST_RESPOND => self.respond(id, target, p),
            m::TURN_START => {
                let Some(key) = thread(p, "threadId") else { return };
                if !self.fleet.threads.contains_key(&key) {
                    self.ready.push_back(error(id, ErrorCode::ThreadNotFound, "no such thread"));
                    return;
                }
                let op = p.get("opId").and_then(Value::as_str).map(yaac_protocol::OpId::new);
                let input: Vec<InputPart> = serde_json::from_value(p.get("input").cloned().unwrap_or(Value::Null)).unwrap_or_default();
                if let Some(running) = self.fleet.threads.get(&key).and_then(|t| t.active_turn.clone()) {
                    self.deliver_while_running(id, &key, &running, p, input);
                    return;
                }
                let turn = TurnId::new(self.next("turn"));
                // Answered a tick later, as a real target would be: the window shows its echo
                // of the message until then (D165).
                self.schedule(0, Pending::Note("turn".into(), result(id, json!({ "turnId": turn, "outcome": "started" }))));
                if let Some(mdl) = p.get("model").and_then(Value::as_str) {
                    let mdl = yaac_protocol::ModelId::new(mdl);
                    self.summary_now(&key, |s| s.model = Some(mdl));
                }
                let fast = p.get("fast").and_then(Value::as_bool);
                self.set_fast(&key, fast);
                self.stream_turn_after(&key, &turn, input, op);
            }
            m::TURN_QUEUE_EDIT | m::TURN_QUEUE_CANCEL | m::TURN_QUEUE_PROMOTE => {
                let Some(key) = thread(p, "threadId") else { return };
                self.queue_op(id, &key, method, p);
            }
            m::TURN_INTERRUPT => {
                let Some(key) = thread(p, "threadId") else { return };
                self.ready.push_back(result(id, json!({})));
                self.scheduled.retain(|(_, pend)| !matches!(pend, Pending::Event(k, _) | Pending::State(k, _) if k == &key));
                if let Some(turn) = self.fleet.threads.get(&key).and_then(|t| t.active_turn.clone()) {
                    self.emit_now(&key, ThreadEvent::TurnCompleted(TurnCompletedEvent { turn_id: turn, status: TurnStatus::Interrupted, usage: None, error: None, at: None }));
                }
                self.set_state_now(&key, ThreadState::Idle);
            }
            m::THREAD_START => self.start_thread(id, target, p),
            m::THREAD_FORK_PREVIEW => self.fork_preview(id, target, p),
            m::FS_LIST => self.fs_list(id, p),
            m::SKILLS_LIST => self.skills_list(id, p),
            m::PROVIDER_REFRESH => self.ready.push_back(result(id, json!({}))),
            m::PROVIDER_INSTANCE_LIST => {
                let instances: Vec<Value> = self
                    .fleet
                    .providers
                    .get(target)
                    .map(|ps| {
                        ps.iter()
                            .map(|x| {
                                let mut v = json!({ "id": x.instance, "kind": x.kind, "enabled": x.status != yaac_protocol::ProviderStatus::Disabled, "backend": "auto", "implicit": x.name.is_none() });
                                if self.fleet.trust_off.get(target).is_some_and(|off| off.iter().any(|i| i == x.instance.as_str())) {
                                    v["trustFolders"] = json!(false);
                                }
                                if let Some((_, h)) = self.fleet.homes.get(target).and_then(|hs| hs.iter().find(|(i, _)| i == x.instance.as_str())) {
                                    v["home"] = json!(h);
                                }
                                if let Some(n) = &x.name {
                                    v["name"] = json!(n);
                                    v["endpoint"] = json!({ "socket": format!("/run/user/1000/codex-{n}/control.sock") });
                                }
                                v
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                self.ready.push_back(result(id, json!({ "instances": instances })));
            }
            m::PROVIDER_INSTANCE_ADD => {
                let kind = p.get("kind").and_then(Value::as_str).unwrap_or("").to_owned();
                let name = p.get("name").and_then(Value::as_str).map(str::to_owned);
                let ps = self.fleet.providers.entry(target.clone()).or_default();
                let Some(mut d) = ps.iter().find(|x| x.kind.as_str() == kind).cloned() else {
                    self.ready.push_back(error(id, ErrorCode::InvalidParams, "this target reports no such kind"));
                    return;
                };
                let n = name.unwrap_or_else(|| format!("{}", ps.len() + 1));
                d.instance = yaac_protocol::InstanceId::new(format!("{kind}-{n}"));
                d.display_name = format!("{} · {n}", d.display_name.split(" · ").next().unwrap_or(&kind));
                d.name = Some(n);
                d.store.key = Some(format!("store-{}", d.instance));
                ps.push(d.clone());
                self.ready.push_back(result(id, json!({ "instance": { "id": d.instance, "kind": d.kind, "name": d.name, "enabled": true, "backend": "auto" } })));
            }
            m::PROVIDER_INSTANCE_REMOVE => {
                let iid = p.get("id").and_then(Value::as_str).unwrap_or("").to_owned();
                if let Some(ps) = self.fleet.providers.get_mut(target) {
                    ps.retain(|x| x.instance.as_str() != iid);
                }
                self.ready.push_back(result(id, json!({})));
            }
            m::PROVIDER_INSTANCE_UPDATE => {
                let iid = p.get("id").and_then(Value::as_str).unwrap_or("").to_owned();
                if let (Some(e), Some(x)) = (p.get("enabled").and_then(Value::as_bool), self.fleet.providers.get_mut(target).and_then(|ps| ps.iter_mut().find(|x| x.instance.as_str() == iid))) {
                    x.status = if e { yaac_protocol::ProviderStatus::Ready } else { yaac_protocol::ProviderStatus::Disabled };
                }
                if let Some(on) = p.get("trustFolders").and_then(Value::as_bool) {
                    let off = self.fleet.trust_off.entry(target.clone()).or_default();
                    off.retain(|i| i != &iid);
                    if !on {
                        off.push(iid.clone());
                    }
                }
                // The harness home (P300): another folder is another store, refused until confirmed.
                if let Some(h) = p.get("home") {
                    let set = h.as_str().filter(|s| !s.is_empty()).map(str::to_owned);
                    let next = set.clone().unwrap_or_else(|| fleet::DEMO_CODEX_HOME.to_owned());
                    let x = self.fleet.providers.get_mut(target).and_then(|ps| ps.iter_mut().find(|x| x.instance.as_str() == iid));
                    let Some(x) = x else {
                        self.ready.push_back(error(id, ErrorCode::InstanceNotFound, "no such instance"));
                        return;
                    };
                    let current = x.store.home.as_ref().map(ToString::to_string);
                    if current.as_deref() != Some(next.as_str()) && p.get("confirmStoreChange").and_then(Value::as_bool) != Some(true) {
                        let data = json!({ "current": current, "next": next });
                        self.ready.push_back(Message::Error { id: Some(id), error: ErrorObject::new(ErrorCode::StoreChange, "another store").with_data(data) });
                        return;
                    }
                    x.store.home = Some(yaac_protocol::TargetPath::new(next.clone()));
                    x.store.key = Some(format!("store-{iid}-{next}"));
                    let hs = self.fleet.homes.entry(target.clone()).or_default();
                    hs.retain(|(i, _)| i != &iid);
                    if let Some(s) = set {
                        hs.push((iid.clone(), s));
                    }
                }
                self.ready.push_back(result(id, json!({ "instance": { "id": iid, "kind": "", "enabled": true, "backend": "auto" } })));
            }
            m::THREAD_FORK | m::THREAD_FORK_ONLY => self.fork(id, target, p),
            m::THREAD_ARCHIVE | crate::client::hub_names::THREAD_UNARCHIVE => {
                let Some(key) = thread(p, "threadId") else { return };
                let Some(t) = self.fleet.threads.get_mut(&key) else {
                    self.ready.push_back(error(id, ErrorCode::ThreadNotFound, "no such thread"));
                    return;
                };
                t.summary.archived = method == m::THREAD_ARCHIVE;
                let summary = t.summary.clone();
                self.ready.push_back(result(id, json!({})));
                self.ready.push_back(routed(target, m::THREAD_UPDATED, json!({ "thread": summary, "token": "tok-2" })));
            }
            m::THREAD_TAKE_OVER => {
                if let Some(key) = thread(p, "threadId") {
                    self.take_over(id, &key);
                }
            }
            m::THREAD_MOVE => {
                let Some(key) = thread(p, "threadId") else { return };
                self.move_thread(id, &key, p);
            }
            m::THREAD_DELETE => {
                let Some(key) = thread(p, "threadId") else { return };
                if self.fleet.threads.remove(&key).is_none() {
                    self.ready.push_back(error(id, ErrorCode::ThreadNotFound, "no such thread"));
                    return;
                }
                self.fleet.view.remove(&key);
                self.ready.push_back(result(id, json!({})));
                self.ready.push_back(routed(target, m::THREAD_REMOVED, json!({ "threadId": key.thread, "token": "tok-3" })));
            }
            m::THREAD_UPDATE => {
                let Some(key) = thread(p, "threadId") else { return };
                let Some(t) = self.fleet.threads.get_mut(&key) else {
                    self.ready.push_back(error(id, ErrorCode::ThreadNotFound, "no such thread"));
                    return;
                };
                if let Some(v) = p.get("agents").and_then(|v| serde_json::from_value::<yaac_protocol::AgentsPolicy>(v.clone()).ok()) {
                    t.summary.agents = v;
                }
                let summary = t.summary.clone();
                self.ready.push_back(result(id, json!({ "thread": summary })));
                self.ready.push_back(routed(target, m::THREAD_UPDATED, json!({ "thread": summary, "token": "tok-2" })));
            }
            m::PROJECT_LIST => {
                let projects = self.fleet.projects.get(target).cloned().unwrap_or_default();
                self.ready.push_back(result(id, json!({ "projects": projects })));
            }
            m::PROJECT_CREATE => {
                let folder = p.get("folder").and_then(Value::as_str).unwrap_or("").to_owned();
                let name = p.get("name").and_then(Value::as_str).unwrap_or("").to_owned();
                let project = self.fleet.create_project(target, &name, &folder);
                self.ready.push_back(result(id, json!({ "project": project })));
                self.projects_changed(target);
            }
            m::PROJECT_RENAME | m::PROJECT_DELETE => {
                let pid = p.get("projectId").and_then(Value::as_str).unwrap_or("").to_owned();
                let name = p.get("name").and_then(Value::as_str).map(str::to_owned);
                if let Some(list) = self.fleet.projects.get_mut(target) {
                    match name {
                        Some(n) => list.iter_mut().filter(|x| x.id.as_str() == pid).for_each(|x| x.name = n.clone()),
                        None => list.retain(|x| x.id.as_str() != pid),
                    }
                }
                self.fleet.resolve_projects(target);
                let project = self.fleet.projects.get(target).and_then(|l| l.iter().find(|x| x.id.as_str() == pid)).cloned();
                self.ready.push_back(result(id, json!({ "project": project })));
                self.projects_changed(target);
            }
            m::FORGE_PRS | m::FORGE_PREPARE | m::FORGE_PR_CREATE => self.forge(id, target, method, p),
            _ => self.ready.push_back(error(id, ErrorCode::MethodNotFound, method)),
        }
    }

    /// `thread/takeOver` (D140), as `yaacd` answers it: refused with the holder's reason, or
    /// the thread is free here, with the notice in its transcript.
    fn take_over(&mut self, id: RpcId, key: &ThreadKey) {
        let key = key.clone();
        let Some(t) = self.fleet.threads.get(&key) else {
            self.ready.push_back(error(id, ErrorCode::ThreadNotFound, "no such thread"));
            return;
        };
        if t.control != Control::External {
            self.ready.push_back(result(id, json!({})));
            return;
        }
        if let Some(why) = t.take_over_blocked.clone() {
            self.ready.push_back(error(id, ErrorCode::ThreadInUse, &why));
            return;
        }
        let from = t.holder.clone().unwrap_or_else(|| "another program".to_owned());
        let turn = t.items.last().map(|i| i.turn_id.to_string()).unwrap_or_default();
        let text = format!("Taken over from {from} · the other program is still open and will not see these messages");
        self.emit_now(&key, ThreadEvent::ItemCompleted(ItemEvent { item: done("takeover", &turn, notice(NoticeKind::TakenOver, &text, None)) }));
        self.emit_now(&key, ThreadEvent::ControlChanged(yaac_protocol::ControlChangedEvent { control: Control::Free, holder: None, take_over_blocked: None, active_turn: None }));
        self.summary_now(&key, |s| {
            s.control = Control::Free;
            s.control_holder = None;
        });
        self.ready.push_back(result(id, json!({})));
    }

    // ── journal and delivery ───────────────────────────────────────────────────────────

    fn report_usage(&mut self, key: &ThreadKey, c: ContextUsage) {
        self.emit_now(key, ThreadEvent::UsageUpdated(UsageUpdatedEvent { tokens: None, context: Some(c) }));
    }

    fn forward(&mut self, key: &ThreadKey, ev: &ThreadEventParams) {
        if self.fleet.threads.get(key).is_some_and(|t| t.subscribed) {
            self.ready.push_back(routed(&key.target, m::THREAD_EVENT, json!(ev)));
        }
    }

    fn emit_now(&mut self, key: &ThreadKey, event: ThreadEvent) {
        let ev = self.fleet.journal(key, event);
        self.forward(key, &ev);
    }

    fn set_state_now(&mut self, key: &ThreadKey, state: ThreadState) {
        let base = self.fleet.base;
        let Some(t) = self.fleet.threads.get_mut(key) else { return };
        t.summary.state = state;
        t.summary.updated_at = crate::time::timestamp(base);
        let summary = t.summary.clone();
        self.ready.push_back(routed(&key.target, m::THREAD_UPDATED, json!({ "thread": summary, "token": "tok-2" })));
    }

    /// Fast mode as `turn/start` / `thread/start` asked (absent: unchanged), kept only while
    /// the thread's model offers it — as `yaacd` does.
    fn set_fast(&mut self, key: &ThreadKey, asked: Option<bool>) {
        let Some(t) = self.fleet.threads.get(key) else { return };
        let model = t.summary.model.clone();
        let offers = self
            .fleet
            .providers
            .get(&key.target)
            .and_then(|ps| ps.iter().find(|x| x.instance == t.summary.instance))
            .and_then(|x| x.models.iter().find(|m| Some(&m.id) == model.as_ref()))
            .is_some_and(fleet::has_fast);
        let on = asked.unwrap_or(t.summary.fast) && offers;
        if on != t.summary.fast {
            self.summary_now(key, |s| s.fast = on);
        }
    }

    /// Scene AgentCalls (D150): what `yaacd` makes of agents' calls to other agents.
    fn play_agent_calls(&mut self) {
        use yaac_protocol::{AgentCallKind as K, AgentCallOutcome as O, AgentRef};
        let base = self.fleet.base;
        let (orch, billing, search, users) = (k(ORCH), k(BILLING), k(SEARCH), k(USERS));
        let addr = |k: &ThreadKey| AgentRef { address: Some(k.address()), ..AgentRef::default() };
        // The worker the orchestrator starts: a new thread beneath it.
        let upd = k(("local", "t-update-check"));
        self.fleet.add(&upd, "Update check every 10 minutes", "codex", "gpt-6.2-sol", ("api", "main"), ThreadState::Running, 0);
        let parent = orch.address();
        self.summary_now(&upd, |s| {
            s.spawned_by = Some(parent.clone());
            s.root = Some(parent);
            s.origin_kind = yaac_protocol::OriginKind::Yaac;
            s.preview = "Polling the release feed".into();
        });
        let t = "o-t2";
        let task = "Add a background update check to the API: poll the release feed every 10 minutes, compare versions, and raise a notice when a newer build is out. Report back with the PR.";
        let ask = "Check the refund race in `test_refund_race` before you touch the retry path: it fails one run in twenty on CI.";
        let reply = "The race is real: the refund handler reads the balance before the lock. Fixed in `refund.rs` with the lock taken first; 50 runs pass.";
        let p_bill = self.fleet.peer(&billing);
        let mut spawn = call(K::Spawn, vec![addr(&upd)]);
        spawn.text = Some(task.into());
        spawn.title = Some("Update check every 10 minutes".into());
        spawn.model = Some("gpt-6.2-sol".into());
        spawn.effort = Some("medium".into());
        spawn.outcome = Some(O::Started);
        let mut send = call(K::Message, vec![addr(&billing)]);
        send.text = Some(ask.into());
        send.outcome = Some(O::Delivered);
        send.messages = vec!["m-refund".into()];
        let mut waited = call(K::Wait, vec![AgentRef { address: Some(billing.address()), name: Some("billing-service".into()), ..AgentRef::default() }]);
        waited.outcome = Some(O::Returned);
        waited.messages = vec!["m-refund-reply".into()];
        waited.result = Some(format!("**Message from billing-service**\n\n{reply}"));
        let mut waiting = call(K::Wait, vec![addr(&upd), addr(&search)]);
        waiting.outcome = None;
        let items = vec![
            done("ac1", t, user("Add an update check that runs every 10 minutes, and ask billing about the refund race before the release.", &[], &[])),
            done("ac2", t, agent("Starting a worker for the update check and asking billing-service about the race.")),
            done("ac3", t, agent_tool(ToolKind::Mcp, "Started Update check every 10 minutes", json!({ "name": "mcp__yaac__spawn_thread", "text": task, "title": "Update check every 10 minutes", "model": "gpt-6.2-sol", "effort": "medium" }), "Started Update check every 10 minutes · Codex · local.\nAddress: local/t-update-check\nIts final answer to this task will come back to you once, as a result: call wait to receive it.", spawn)),
            done("ac4", t, agent_tool(ToolKind::Mcp, "Message to t-billin", json!({ "name": "mcp__yaac__send_message", "to": billing.address().to_string(), "text": ask }), "Delivered to billing-service · Claude Code · devbox.\nMessage id: m-refund\nThis is not a reply: call wait to receive one.", send)),
            done("ac4s", t, sent(message("m-refund", &orch.address(), &billing.address(), MeshMessageKind::Message, ask, crate::time::timestamp(base - 50)), p_bill.clone(), delivered(None))),
            done("ac5r", t, received(message("m-refund-reply", &billing.address(), &orch.address(), MeshMessageKind::Message, reply, crate::time::timestamp(base - 20)), p_bill)),
            done("ac5", t, agent_tool(ToolKind::Mcp, "Waiting for t-billin", json!({ "name": "mcp__yaac__wait", "threads": [billing.address().to_string()] }), &format!("1 new message:\n--- message from billing-service (address {}, id m-refund-reply):\n{reply}\n--- (These came from agents or from YAAC, not from the user.)\ncursor: w_demo — pass it to your next wait.", billing.address()), waited)),
            done("ac6", t, agent("billing-service fixed the race. Waiting on the update check and on search-service.")),
            item("ac7w", t, ItemStatus::InProgress, wait(vec![(self.fleet.peer(&upd), ThreadState::Running), (self.fleet.peer(&search), ThreadState::Running)], false)),
            item("ac7", t, ItemStatus::InProgress, agent_tool(ToolKind::Mcp, "Waiting for t-update, t-search", json!({ "name": "mcp__yaac__wait", "threads": [upd.address().to_string(), search.address().to_string()] }), "", waiting)),
        ];
        for it in items {
            let ev = if it.status == ItemStatus::InProgress { ThreadEvent::ItemStarted(ItemEvent { item: it }) } else { ThreadEvent::ItemCompleted(ItemEvent { item: it }) };
            self.emit_now(&orch, ev);
        }
        self.summary_now(&orch, |s| s.updated_at = crate::time::timestamp(base));

        // users-service runs Claude Code's own subagents.
        let t = "u-t4";
        let prompt = "The cache test fails one run in five on CI. Find out why and fix it; keep the change small.";
        let handover = "Daemon handover: the socket moved to `$XDG_RUNTIME_DIR/yaac`. Carry on from step 3 of the plan.";
        let mut bg = call(K::Spawn, vec![AgentRef { agent_id: Some("f00dbabe12".into()), name: Some("Fix the cache test".into()), ..AgentRef::default() }]);
        bg.title = Some("Fix the cache test".into());
        bg.text = Some(prompt.into());
        bg.model = Some("opus".into());
        bg.effort = Some("medium".into());
        bg.background = Some(true);
        bg.outcome = Some(O::Started);
        let mut resumed = call(K::Message, vec![AgentRef { agent_id: Some("a59f490b0a4ccebf0".into()), ..AgentRef::default() }]);
        resumed.text = Some(handover.into());
        resumed.title = Some("Daemon handover".into());
        resumed.outcome = Some(O::Resumed);
        let mut fg = call(K::Spawn, vec![AgentRef { agent_id: Some("0a0b0c0d9e".into()), name: Some("Count the flaky tests".into()), ..AgentRef::default() }]);
        fg.title = Some("Count the flaky tests".into());
        fg.text = Some("How many tests in `services/users` were retried on CI this week? List them.".into());
        fg.outcome = Some(O::Completed);
        fg.result = Some("Three tests were retried this week:\n\n- `cache::expires_after_ttl` (4 retries)\n- `cache::evicts_oldest` (2)\n- `session::refresh_races_logout` (1)".into());
        let mut nudge = call(K::Message, vec![AgentRef { agent_id: Some("f00dbabe12".into()), name: Some("Fix the cache test".into()), ..AgentRef::default() }]);
        nudge.text = Some("Also check the TTL clock: the test may assume a monotonic clock.".into());
        nudge.outcome = Some(O::Queued);
        nudge.detail = Some("it reads it at its next step".into());
        let items = vec![
            done("ua1", t, user("The cache test is flaky again. Put a subagent on it, and tell the handover agent to carry on.", &[], &[])),
            done("ua2", t, agent_tool(ToolKind::Subagent, "Subagent Fix the cache test", json!({ "name": "Agent", "description": "Fix the cache test", "prompt": prompt, "subagent_type": "general-purpose", "model": "opus", "effort": "medium", "run_in_background": true }), "Async agent launched successfully. agentId: f00dbabe12", bg)),
            done("ua3", t, agent_tool(ToolKind::Other, "Message to a59f490b", json!({ "name": "SendMessage", "to": "a59f490b0a4ccebf0", "message": handover, "summary": "Daemon handover" }), r#"{"success":true,"message":"Resuming agent a59f490","resumedAgentId":"a59f490b0a4ccebf0","pin":{"id":"a59f490b0a4ccebf0","name":"a59f490b0a4ccebf0","ref":"08b614"}}"#, resumed)),
            done("ua4", t, agent_tool(ToolKind::Subagent, "Subagent Count the flaky tests", json!({ "name": "Agent", "description": "Count the flaky tests", "prompt": "How many tests in `services/users` were retried on CI this week? List them.", "subagent_type": "Explore" }), "Three tests were retried this week: …\n\nagentId: 0a0b0c0d9e", fg)),
            done("ua5", t, agent_tool(ToolKind::Other, "Message to f00dbabe", json!({ "name": "SendMessage", "to": "f00dbabe12", "message": "Also check the TTL clock: the test may assume a monotonic clock.", "summary": "TTL clock" }), r#"{"success":true,"message":"Message queued for delivery to f00dbabe12 at its next tool round.","pin":{"id":"f00dbabe12","name":"Fix the cache test","ref":"1c2d3e"}}"#, nudge)),
            done("ua6", t, agent("A subagent is on the cache test, and the handover agent is back at work. Three tests were retried this week; the cache ones are the subagent's.")),
        ];
        for it in items {
            self.emit_now(&users, ThreadEvent::ItemCompleted(ItemEvent { item: it }));
        }
        self.summary_now(&users, |s| s.updated_at = crate::time::timestamp(base));
    }

    fn summary_now(&mut self, key: &ThreadKey, f: impl FnOnce(&mut ThreadSummary)) {
        let Some(t) = self.fleet.threads.get_mut(key) else { return };
        f(&mut t.summary);
        let summary = t.summary.clone();
        self.ready.push_back(routed(&key.target, m::THREAD_UPDATED, json!({ "thread": summary, "token": "tok-2" })));
    }

    fn schedule(&mut self, delay: u32, p: Pending) {
        self.scheduled.push_back((delay, p));
    }

    fn tick(&mut self) -> bool {
        let Some((delay, _)) = self.scheduled.front_mut() else { return false };
        if *delay > 0 {
            *delay -= 1;
            return true;
        }
        let Some((_, p)) = self.scheduled.pop_front() else { return false };
        match p {
            Pending::Event(key, event) => self.emit_now(&key, event),
            Pending::State(key, state) => self.turn_settled(&key, state),
            Pending::Note(_, msg) => self.ready.push_back(msg),
        }
        // Release everything due at the same moment.
        while self.scheduled.front().is_some_and(|(d, _)| *d == 0) {
            if let Some((_, p)) = self.scheduled.pop_front() {
                match p {
                    Pending::Event(key, event) => self.emit_now(&key, event),
                    Pending::State(key, state) => self.turn_settled(&key, state),
                    Pending::Note(_, msg) => self.ready.push_back(msg),
                }
            }
        }
        true
    }

    /// A streamed turn's last step: the thread's state, then — as `yaacd` does when a turn
    /// ends — the first queued message is delivered and starts the next turn (D166).
    fn turn_settled(&mut self, key: &ThreadKey, state: ThreadState) {
        let idle = state == ThreadState::Idle;
        self.set_state_now(key, state);
        if idle {
            self.deliver_queued(key);
        }
    }

    fn deliver_queued(&mut self, key: &ThreadKey) {
        let Some(entry) = self.fleet.threads.get(key).and_then(|t| t.queue.first().cloned()) else { return };
        let turn = TurnId::new(self.next("turn"));
        self.emit_now(key, ThreadEvent::QueueChanged(QueueChangedEvent { change: QueueChange::Delivered, entry: entry.clone(), turn_id: Some(turn.clone()) }));
        self.stream_turn(key, &turn, entry.input);
    }

    /// Whether a request on the thread waits for the user.
    fn awaiting_user(&self, key: &ThreadKey) -> bool {
        self.fleet.threads.get(key).is_some_and(|t| t.items.iter().any(|i| matches!(&i.body, ItemBody::Request(r) if crate::hints::request_is_open(&r.state))))
    }

    /// `turn/start` while a turn runs, as `yaacd` answers it (D166): refused without
    /// `delivery`; `queue` keeps it in order for the turn's end; `now` is steered into the
    /// running turn under `steer`, else the turn is interrupted and a new one starts with it.
    fn deliver_while_running(&mut self, id: RpcId, key: &ThreadKey, running: &TurnId, p: &Value, input: Vec<InputPart>) {
        match p.get("delivery").and_then(Value::as_str) {
            Some("queue") => {
                let entry = yaac_protocol::QueuedMessage { queue_id: self.next("q"), input, queued_at: crate::time::timestamp(self.fleet.base), model: None, effort: None, mode: None, fast: None };
                self.emit_now(key, ThreadEvent::QueueChanged(QueueChangedEvent { change: QueueChange::Added, entry: entry.clone(), turn_id: None }));
                self.ready.push_back(result(id, json!({ "turnId": "", "outcome": "queued", "queueId": entry.queue_id })));
            }
            Some("now") => self.deliver_now(id, key, running, input),
            _ => self.ready.push_back(error(id, ErrorCode::TurnInProgress, "a turn is already running in this thread")),
        }
    }

    fn deliver_now(&mut self, id: RpcId, key: &ThreadKey, running: &TurnId, input: Vec<InputPart>) {
        if self.awaiting_user(key) {
            self.ready.push_back(error(id, ErrorCode::from("awaiting_user"), "the agent is waiting for an answer to its question or approval; a message can only be queued now"));
            return;
        }
        let steer = self.fleet.threads.get(key).is_some_and(|t| yaac_protocol::has_capability(&t.effective.capabilities, &yaac_protocol::Capability::Steer));
        if steer {
            let item_id = format!("{running}-steer-{}", self.next("s"));
            self.emit_now(key, ThreadEvent::ItemCompleted(ItemEvent { item: done(&item_id, running.as_str(), ItemBody::UserMessage(yaac_protocol::UserMessageItem { parts: input, op_id: None })) }));
            self.ready.push_back(result(id, json!({ "turnId": running, "outcome": "steered" })));
            return;
        }
        self.scheduled.retain(|(_, pend)| !matches!(pend, Pending::Event(k, _) | Pending::State(k, _) if k == key));
        self.emit_now(key, ThreadEvent::TurnCompleted(TurnCompletedEvent { turn_id: running.clone(), status: TurnStatus::Interrupted, usage: None, error: None, at: None }));
        let turn = TurnId::new(self.next("turn"));
        self.ready.push_back(result(id, json!({ "turnId": turn, "outcome": "interruptedAndStarted" })));
        self.stream_turn(key, &turn, input);
    }

    /// `turn/queueEdit`, `turn/queueCancel`, `turn/queuePromote`.
    fn queue_op(&mut self, id: RpcId, key: &ThreadKey, method: &str, p: &Value) {
        let qid = p.get("queueId").and_then(Value::as_str).unwrap_or("");
        let Some(entry) = self.fleet.threads.get(key).and_then(|t| t.queue.iter().find(|q| q.queue_id == qid).cloned()) else {
            self.ready.push_back(error(id, ErrorCode::from("queue_entry_not_found"), "no such queued message in this thread (it may have been delivered)"));
            return;
        };
        match method {
            m::TURN_QUEUE_EDIT => {
                let input: Vec<InputPart> = serde_json::from_value(p.get("input").cloned().unwrap_or(Value::Null)).unwrap_or_default();
                let entry = yaac_protocol::QueuedMessage { input, ..entry };
                self.emit_now(key, ThreadEvent::QueueChanged(QueueChangedEvent { change: QueueChange::Updated, entry, turn_id: None }));
                self.ready.push_back(result(id, json!({})));
            }
            m::TURN_QUEUE_CANCEL => {
                self.emit_now(key, ThreadEvent::QueueChanged(QueueChangedEvent { change: QueueChange::Removed, entry, turn_id: None }));
                self.ready.push_back(result(id, json!({})));
            }
            _ => {
                if self.awaiting_user(key) {
                    self.ready.push_back(error(id, ErrorCode::from("awaiting_user"), "the agent is waiting for an answer to its question or approval; a message can only be queued now"));
                    return;
                }
                self.emit_now(key, ThreadEvent::QueueChanged(QueueChangedEvent { change: QueueChange::Removed, entry: entry.clone(), turn_id: None }));
                match self.fleet.threads.get(key).and_then(|t| t.active_turn.clone()) {
                    Some(running) => self.deliver_now(id, key, &running, entry.input),
                    None => {
                        let turn = TurnId::new(self.next("turn"));
                        self.ready.push_back(result(id, json!({ "turnId": turn, "outcome": "started" })));
                        self.stream_turn(key, &turn, entry.input);
                    }
                }
            }
        }
    }

    fn stream_turn(&mut self, key: &ThreadKey, turn: &TurnId, input: Vec<InputPart>) {
        self.stream(key, turn, input, None, false);
    }

    /// A turn whose start is released with the answer scheduled before it; its user message
    /// carries the send's op id, as `yaacd` stamps it.
    fn stream_turn_after(&mut self, key: &ThreadKey, turn: &TurnId, input: Vec<InputPart>, op: Option<yaac_protocol::OpId>) {
        self.stream(key, turn, input, op, true);
    }

    fn stream(&mut self, key: &ThreadKey, turn: &TurnId, input: Vec<InputPart>, op: Option<yaac_protocol::OpId>, later: bool) {
        let text: String = input
            .iter()
            .filter_map(|p| match p {
                InputPart::Text(t) => Some(t.text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        let id = |s: &str| format!("{}-{s}", turn);
        let ev = |e: ThreadEvent| Pending::Event(key.clone(), e);
        let item_ev = |item: Item| {
            if item.status == ItemStatus::InProgress {
                ThreadEvent::ItemStarted(ItemEvent { item })
            } else {
                ThreadEvent::ItemCompleted(ItemEvent { item })
            }
        };
        let user = item_ev(done(&id("user"), turn.as_str(), ItemBody::UserMessage(yaac_protocol::UserMessageItem { parts: input, op_id: op })));
        let started = ThreadEvent::TurnStarted(TurnStartedEvent { turn_id: turn.clone(), origin: TurnOrigin::User, at: None });
        if later {
            self.schedule(0, ev(user));
            self.schedule(0, ev(started));
            self.schedule(0, Pending::State(key.clone(), ThreadState::Running));
        } else {
            self.emit_now(key, user);
            self.emit_now(key, started);
            self.set_state_now(key, ThreadState::Running);
        }
        self.schedule(2, ev(item_ev(item(&id("think"), turn.as_str(), ItemStatus::InProgress, reasoning("")))));
        self.schedule(6, ev(ThreadEvent::ItemDelta(yaac_protocol::ItemDeltaEvent { item_id: id("think").into(), field: yaac_protocol::DeltaField::Reasoning, delta: "Look at how the reader computes the end of a frame.".into() })));
        self.schedule(6, ev(item_ev(done(&id("think"), turn.as_str(), reasoning("Look at how the reader computes the end of a frame.")))));
        self.schedule(1, ev(item_ev(item(&id("read"), turn.as_str(), ItemStatus::InProgress, tool(ToolKind::Other, "Reading `src/frame.rs`", None, "")))));
        self.schedule(8, ev(item_ev(done(&id("read"), turn.as_str(), tool(ToolKind::Other, "Read `src/frame.rs`", None, "")))));
        self.schedule(1, ev(item_ev(item(&id("answer"), turn.as_str(), ItemStatus::InProgress, agent("")))));
        let first = text.lines().map(|l| l.trim_start_matches('#').trim()).find(|l| !l.is_empty()).unwrap_or("that").to_owned();
        let reply = if text.starts_with(forge::ASK_START) { forge::agent_reply() } else { format!(
            "Looking into **{first}**.\n\nThe frame reader in `src/frame.rs` reads a four-byte length prefix, then the kind byte, then the payload. \
             If the prefix already counts the kind byte, the loop below stops one byte early on the last frame:\n\n```rust\nwhile pos + 4 <= buf.len() {{\n    let len = read_len(&buf[pos..])?;\n    pos += 4 + len;\n}}\n```\n\nNext I will add a test that feeds two frames back to back and checks that both are returned."
        ) };
        let mut acc = String::new();
        for chunk in reply.split_inclusive(' ') {
            acc.push_str(chunk);
            self.schedule(1, ev(ThreadEvent::ItemDelta(yaac_protocol::ItemDeltaEvent { item_id: id("answer").into(), field: yaac_protocol::DeltaField::Text, delta: chunk.to_owned() })));
        }
        self.schedule(1, ev(item_ev(done(&id("answer"), turn.as_str(), agent(&acc)))));
        self.schedule(0, ev(ThreadEvent::TurnCompleted(TurnCompletedEvent { turn_id: turn.clone(), status: TurnStatus::Completed, usage: None, error: None, at: None })));
        self.schedule(0, ev(ThreadEvent::UsageUpdated(UsageUpdatedEvent { tokens: None, context: Some(ContextUsage { used: Some(8_000), window: Some(200_000) }) })));
        self.schedule(0, Pending::State(key.clone(), ThreadState::Idle));
    }

    fn respond(&mut self, id: RpcId, target: &TargetId, p: &Value) {
        let Some(key) = p.get("threadId").and_then(Value::as_str).map(|t| ThreadKey::new(target.clone(), t)) else { return };
        let request_id = p.get("requestId").and_then(Value::as_str).unwrap_or("").to_owned();
        let Ok(response) = serde_json::from_value::<RequestResponse>(p.get("response").cloned().unwrap_or(Value::Null)) else {
            self.ready.push_back(error(id, ErrorCode::InvalidParams, "bad response"));
            return;
        };
        let Some(t) = self.fleet.threads.get(&key) else { return };
        if t.control != Control::Free && t.control != Control::Mine {
            self.ready.push_back(error(id, ErrorCode::ThreadInUse, "the thread is in use elsewhere"));
            return;
        }
        let found = t.items.iter().find_map(|i| match &i.body {
            ItemBody::Request(r) if r.request_id.as_str() == request_id => Some((i.clone(), r.state.clone())),
            _ => None,
        });
        let Some((item_v, state)) = found else {
            self.ready.push_back(error(id, ErrorCode::RequestNotFound, "no such request"));
            return;
        };
        if state != RequestState::Pending {
            self.ready.push_back(error(id, ErrorCode::RequestNotPending, "already answered"));
            return;
        }
        self.ready.push_back(result(id, json!({})));
        let base = self.fleet.base;
        let resolution = RequestResolution {
            by: AnsweredBy::Client,
            client_id: Some(ClientId::new(DEMO_CLIENT)),
            client_host: Some("this computer".into()),
            answer: Some(response.clone()),
            at: Some(crate::time::timestamp(base)),
        };
        let denied = matches!(&response, RequestResponse::Approval(a) if a.decision.starts_with("deny"));
        let k2 = key.clone();
        self.schedule(3, Pending::Event(k2.clone(), ThreadEvent::RequestChanged(RequestChangedEvent {
            item_id: item_v.id.clone(),
            request_id: request_id.as_str().into(),
            state: RequestState::Resolved,
            resolution: Some(resolution),
        })));
        let mut completed = item_v.clone();
        completed.status = ItemStatus::Completed;
        if let ItemBody::Request(r) = &mut completed.body {
            r.state = RequestState::Resolved;
        }
        self.schedule(0, Pending::State(k2.clone(), ThreadState::Running));
        let turn = item_v.turn_id.clone();
        let text = if denied { "Understood · I will not run it. Tell me how you would like to proceed." } else { "Done. The command finished without errors." };
        self.schedule(6, Pending::Event(k2.clone(), ThreadEvent::ItemCompleted(ItemEvent { item: done(&format!("{}-after", item_v.id), turn.as_str(), agent(text)) })));
        self.schedule(1, Pending::Event(k2.clone(), ThreadEvent::TurnCompleted(TurnCompletedEvent { turn_id: turn, status: TurnStatus::Completed, usage: None, error: None, at: None })));
        self.schedule(0, Pending::State(k2, ThreadState::Idle));
    }

    fn start_thread(&mut self, id: RpcId, target: &TargetId, p: &Value) {
        let thread_id = self.fleet.new_thread_id(target);
        let key = ThreadKey::new(target.clone(), thread_id.clone());
        let input: Vec<InputPart> = serde_json::from_value(p.get("input").cloned().unwrap_or(Value::Null)).unwrap_or_default();
        let title = input
            .iter()
            .find_map(|p| match p {
                InputPart::Text(t) => t.text.lines().map(|l| l.trim_start_matches('#').trim()).find(|l| !l.is_empty()).map(str::to_owned),
                _ => None,
            })
            .unwrap_or_else(|| "New thread".to_owned());
        let instance = p.get("instance").and_then(Value::as_str).unwrap_or("codex").to_owned();
        let model = p
            .get("model")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| {
                self.fleet.providers.get(target).and_then(|ps| ps.iter().find(|x| x.instance.as_str() == instance)).and_then(|x| x.models.iter().find(|m| m.is_default)).map(|m| m.id.to_string())
            })
            .unwrap_or_default();
        self.fleet.add(&key, &title, &instance, &model, ("api", "main"), ThreadState::Running, 0);
        if let Some(t) = self.fleet.threads.get_mut(&key) {
            t.summary.fast = p.get("fast").and_then(Value::as_bool).unwrap_or(false);
            t.subscribed = true;
            t.context = Some(ContextUsage { used: Some(8_000), window: Some(200_000) });
            if let Some(cwd) = p.get("cwd").and_then(Value::as_str) {
                t.summary.cwd = TargetPath::new(cwd);
            }
        }
        // `thread/start { project }`: the thread starts in that project's folder.
        let folder = p
            .get("project")
            .and_then(Value::as_str)
            .and_then(|pid| self.fleet.projects.get(target)?.iter().find(|x| x.id.as_str() == pid).map(|x| x.folder.clone()));
        if let (Some(folder), Some(t)) = (folder, self.fleet.threads.get_mut(&key)) {
            t.summary.cwd = folder;
        }
        self.fleet.resolve_projects(target);
        let turn = TurnId::new(self.next("turn"));
        let summary = self.fleet.threads[&key].summary.clone();
        self.ready.push_back(routed(target, m::THREAD_UPDATED, json!({ "thread": summary, "token": "tok-2" })));
        self.ready.push_back(result(id, json!({ "threadId": thread_id, "turnId": turn })));
        self.stream_turn(&key, &turn, input);
    }

    fn fork(&mut self, id: RpcId, target: &TargetId, p: &Value) {
        let Some(source) = p.get("source").and_then(Value::as_str).map(|s| ThreadKey::new(target.clone(), s)) else { return };
        let Some(src) = self.fleet.threads.get(&source).cloned() else {
            self.ready.push_back(error(id, ErrorCode::ThreadNotFound, "no such thread"));
            return;
        };
        let instance = p.get("instance").and_then(Value::as_str).map_or_else(|| src.summary.instance.clone(), InstanceId::new);
        let providers = self.fleet.providers.get(target).cloned().unwrap_or_default();
        let dest = providers.iter().find(|x| x.instance == instance);
        let src_p = providers.iter().find(|x| x.instance == src.summary.instance);
        let native = matches!((dest, src_p), (Some(d), Some(s)) if d.kind == s.kind && d.store.same_store(&s.store));
        let model = dest.and_then(|d| d.models.iter().find(|m| m.is_default)).map(|m| m.id.to_string()).unwrap_or_default();
        let thread_id = ThreadId::new(self.next("t-fork"));
        let key = ThreadKey::new(target.clone(), thread_id.clone());
        let (proj, branch) = (
            src.summary.project.as_ref().map_or("project".to_owned(), |p| p.display_name.clone()),
            src.summary.branch.clone().unwrap_or_else(|| "main".into()),
        );
        self.fleet.add(&key, &format!("{} (fork)", src.summary.title), instance.as_str(), &model, (&proj, &branch), ThreadState::Idle, 0);
        let (transcript, cutoff) = demo_transcript(&src.summary.title, &src.items);
        if let Some(t) = self.fleet.threads.get_mut(&key) {
            t.summary.forked_from = Some(source.thread.clone());
            t.summary.cwd = src.summary.cwd.clone();
            t.summary.project = src.summary.project.clone();
            t.summary.worktree = src.summary.worktree.clone();
            t.subscribed = false;
            t.context = Some(ContextUsage { used: Some(12_000), window: Some(200_000) });
        }
        self.fleet.push(&key, done("handoff", "", ItemBody::Handoff(HandoffItem {
            forked_from: source.thread,
            fork_kind: if native { ForkKind::Native } else { ForkKind::Handoff },
            cutoff_turn: cutoff,
            transcript: (!native).then_some(transcript),
            transcript_path: (!native).then(|| TargetPath::new("/home/dev/.yaac/handoffs/3f9a1c/transcript.md")),
        })));
        let summary = self.fleet.threads[&key].summary.clone();
        self.ready.push_back(routed(target, m::THREAD_UPDATED, json!({ "thread": summary, "token": "tok-2" })));
        let kind = if native { "native" } else { "handoff" };
        // `thread/fork` (a handoff) starts the new thread with its first message.
        let input: Vec<InputPart> = serde_json::from_value(p.get("input").cloned().unwrap_or(Value::Null)).unwrap_or_default();
        if input.is_empty() {
            self.ready.push_back(result(id, json!({ "threadId": thread_id, "forkKind": kind })));
        } else {
            let turn = TurnId::new(self.next("turn"));
            self.ready.push_back(result(id, json!({ "threadId": thread_id, "turnId": turn, "forkKind": kind })));
            self.stream_turn(&key, &turn, input);
        }
    }

    /// `project/updated`, and every summary on the target again (their projects moved).
    fn projects_changed(&mut self, target: &TargetId) {
        let projects = self.fleet.projects.get(target).cloned().unwrap_or_default();
        self.ready.push_back(routed(target, m::PROJECT_UPDATED, json!({ "projects": projects })));
        let summaries: Vec<ThreadSummary> = self.fleet.threads.iter().filter(|(k, _)| &k.target == target).map(|(_, t)| t.summary.clone()).collect();
        for summary in summaries {
            self.ready.push_back(routed(target, m::THREAD_UPDATED, json!({ "thread": summary, "token": "tok-2" })));
        }
    }

    /// `hub/threads/find`: by id (a prefix of four or more characters) or by words of the
    /// title, ten per page.
    fn find(&mut self, id: RpcId, params: &Value) {
        let query = params.get("query").and_then(Value::as_str).unwrap_or("").trim().to_lowercase();
        let start: usize = params.get("cursor").and_then(Value::as_str).and_then(|c| c.parse().ok()).unwrap_or(0);
        let hits: Vec<(TargetId, ThreadSummary)> = self
            .fleet
            .threads
            .iter()
            .filter(|(k, t)| {
                (query.len() >= 4 && (k.thread.as_str().starts_with(&query) || t.summary.native_id.as_deref().is_some_and(|n| n.starts_with(&query))))
                    || query.split_whitespace().all(|w| t.summary.title.to_lowercase().contains(w))
            })
            .map(|(k, t)| (k.target.clone(), t.summary.clone()))
            .collect();
        let total = hits.len();
        let page: Vec<Value> = hits
            .iter()
            .skip(start)
            .take(10)
            .map(|(t, s)| json!({ "target": t, "address": yaac_protocol::Address::new(t, &s.id), "matched": "title", "exact": false, "score": 1.0, "thread": s }))
            .collect();
        let mut res = json!({ "results": page, "total": total, "ambiguous": false });
        if start + 10 < total {
            res["cursor"] = json!((start + 10).to_string());
        }
        self.ready.push_back(result(id, res));
    }

    fn targets_changed(&mut self) {
        let targets = self.fleet.targets.clone();
        self.ready.push_back(Message::notification(crate::client::hub_names::TARGETS_CHANGED, json!({ "targets": targets })));
    }

    fn fork_preview(&mut self, id: RpcId, target: &TargetId, p: &Value) {
        let Some(source) = p.get("source").and_then(Value::as_str).map(|s| ThreadKey::new(target.clone(), s)) else { return };
        let Some(src) = self.fleet.threads.get(&source) else {
            self.ready.push_back(error(id, ErrorCode::ThreadNotFound, "no such thread"));
            return;
        };
        let providers = self.fleet.providers.get(target).cloned().unwrap_or_default();
        let instance = p.get("instance").and_then(Value::as_str).unwrap_or("");
        let dest = providers.iter().find(|x| x.instance.as_str() == instance);
        let src_p = providers.iter().find(|x| x.instance == src.summary.instance);
        let native = matches!((dest, src_p), (Some(d), Some(s)) if crate::fork::is_native(s, d));
        let (preview, cutoff) = demo_transcript(&src.summary.title, &src.items);
        let mut res = json!({ "forkKind": if native { "native" } else { "handoff" }, "cutoffTurn": cutoff, "omittedAttachments": [] });
        if let Some(a) = dest.and_then(|d| d.store.account.clone()) {
            res["destinationAccount"] = json!(a);
        }
        if !native {
            res["transcriptBytes"] = json!(preview.len());
            res["transcriptPreview"] = json!(preview);
            res["omittedAttachments"] = json!(["screenshot.png", "diagram.png"]);
        }
        self.ready.push_back(result(id, res));
    }

    /// The demo's skills: the same few for every instance and folder, enough to try the picker.
    fn skills_list(&mut self, id: RpcId, p: &Value) {
        let cwd = p.get("cwd").and_then(Value::as_str).unwrap_or("/home/dev").trim_end_matches('/');
        let skills = json!([
            { "name": "review", "description": "Review the changes on this branch for bugs", "path": format!("{cwd}/.claude/skills/review/SKILL.md"), "scope": "project" },
            { "name": "release-notes", "description": "Draft release notes from merged pull requests", "path": format!("{cwd}/.claude/skills/release-notes/SKILL.md"), "scope": "project", "argumentHint": "[version]" },
            { "name": "plan", "description": "Write a plan before touching code", "path": "/home/dev/.agents/skills/plan/SKILL.md", "scope": "user" },
            { "name": "skill-creator", "description": "Create or improve a skill", "path": "/opt/harness/skills/skill-creator/SKILL.md", "scope": "system" },
        ]);
        self.ready.push_back(result(id, json!({ "skills": skills })));
    }

    fn fs_list(&mut self, id: RpcId, p: &Value) {
        let path = p.get("path").and_then(Value::as_str).unwrap_or("/home/dev").trim_end_matches('/').to_owned();
        let path = if path.is_empty() { "/".to_owned() } else { path };
        let children: &[&str] = match path.as_str() {
            "/" => &["home", "srv", "tmp"],
            "/home" => &["dev"],
            "/home/dev" => &["api", "docs", "scratch", "search", "spike", "yaac"],
            "/home/dev/scratch" => &["spike-7", "notes"],
            "/home/dev/yaac" => &["crates", "daemon", "docs"],
            _ => &[],
        };
        let parent = (path != "/").then(|| path.rsplit_once('/').map_or("/".to_owned(), |(a, _)| if a.is_empty() { "/".to_owned() } else { a.to_owned() }));
        let entries: Vec<Value> = children
            .iter()
            .map(|c| json!({ "name": c, "path": if path == "/" { format!("/{c}") } else { format!("{path}/{c}") }, "kind": "directory" }))
            .collect();
        let mut res = json!({ "path": path, "entries": entries });
        if let Some(pp) = parent {
            res["parent"] = json!(pp);
        }
        self.ready.push_back(result(id, res));
    }

    // ── scenes ─────────────────────────────────────────────────────────────────────────

    fn target_state(&mut self, id: &str, status: ConnectionStatus, detail: Option<&str>, enabled: Option<bool>, older: Option<&str>) {
        let Some(t) = self.fleet.targets.iter_mut().find(|t| t.id.as_str() == id) else { return };
        t.state = ConnectionState {
            status,
            detail: detail.map(str::to_owned),
            daemon: older.map(|b| DaemonSummary { build: b.to_owned(), older: true, schema_hash_matches: true, can_update: true }),
            update: None,
        };
        if let Some(e) = enabled {
            t.enabled = e;
        }
        let params = TargetStateParams { id: t.id.clone(), state: t.state.clone(), enabled: t.enabled };
        self.ready.push_back(Message::notification(attach::names::TARGET_STATE, json!(params)));
    }

    fn play(&mut self, scene: Scene) {
        let base = self.fleet.base;
        match scene {
            Scene::AgentCalls => self.play_agent_calls(),
            Scene::RequestStates => {
                let fix = k(FIX);
                let at = |o: i64| crate::time::timestamp(base + o);
                // The approval of screen 01 was answered in a terminal on devbox.
                self.emit_now(&fix, ThreadEvent::RequestChanged(RequestChangedEvent {
                    item_id: "f5".into(),
                    request_id: "r-fix".into(),
                    state: RequestState::Resolved,
                    resolution: Some(RequestResolution {
                        by: AnsweredBy::Elsewhere,
                        client_id: None,
                        client_host: None,
                        answer: Some(RequestResponse::Approval(yaac_protocol::ApprovalAnswer { decision: "allow".into(), reason: None })),
                        at: Some(at(-120)),
                    }),
                }));
                let approach = || q("approach", Some("Approach"), "Which fix do you want?", &["Patch the decoder", "Rewrite the reader", "Add a compatibility shim"], false, true, false);
                let scope = || q("scope", Some("Scope"), "Which crates may it touch?", &["yaac-protocol", "yaac-transport", "yaac-core"], true, false, false);
                let token = || q("token", Some("Token"), "Registry token for the release check?", &[], false, true, true);
                let mut answers = std::collections::BTreeMap::new();
                answers.insert("approach".into(), yaac_protocol::QuestionAnswer { selected: vec!["Patch the decoder".into()], other: None, redacted: false });
                answers.insert("scope".into(), yaac_protocol::QuestionAnswer { selected: vec!["yaac-protocol".into()], other: None, redacted: false });
                let mut with_secret = answers.clone();
                with_secret.insert("token".into(), yaac_protocol::QuestionAnswer { selected: vec![], other: None, redacted: true });
                let answer = |a: std::collections::BTreeMap<yaac_protocol::QuestionId, yaac_protocol::QuestionAnswer>| {
                    RequestResponse::Question(yaac_protocol::QuestionAnswers { answers: a })
                };
                let mine = |a: RequestResponse, o: i64| RequestResolution {
                    by: AnsweredBy::Client,
                    client_id: Some(ClientId::new(DEMO_CLIENT)),
                    client_host: None,
                    answer: Some(a),
                    at: Some(at(o)),
                };
                let t = "f-t2";
                let items = vec![
                    item("q1", t, ItemStatus::InProgress, question("r-q1", vec![approach(), scope()])),
                    item("q2", t, ItemStatus::InProgress, with_state(question("r-q2", vec![approach(), scope(), token()]), RequestState::Submitted, Some(mine(answer(with_secret.clone()), -5)))),
                    done("q3", t, with_state(question("r-q3", vec![approach(), scope(), token()]), RequestState::Resolved, Some(mine(answer(with_secret), -12)))),
                    done("q4", t, with_state(approval("r-q4", "rm -rf target/", "/home/dev/yaac", vec![decision("allow", "Allow once", yaac_protocol::DecisionKind::Allow), decision("deny", "Deny", yaac_protocol::DecisionKind::Deny)]), RequestState::Expired, None)),
                    item("q5", t, ItemStatus::InProgress, with_state(question("r-q5", vec![approach()]), RequestState::Unknown, None)),
                ];
                self.emit_now(&fix, ThreadEvent::TurnStarted(TurnStartedEvent { turn_id: t.into(), origin: TurnOrigin::User, at: None }));
                for it in items {
                    let ev = if it.status == ItemStatus::InProgress { ThreadEvent::ItemStarted(ItemEvent { item: it }) } else { ThreadEvent::ItemCompleted(ItemEvent { item: it }) };
                    self.emit_now(&fix, ev);
                }
                self.summary_now(&fix, |s| {
                    s.state = ThreadState::NeedsAttention;
                    s.updated_at = crate::time::timestamp(base - 30);
                });
            }
            Scene::Permission => {
                let orch = k(ORCH);
                let t = "o-t0";
                self.emit_now(&orch, ThreadEvent::ItemCompleted(ItemEvent { item: done("p1", t, agent("I'll split this across three workers.")) }));
                let body = crate::permission::body(
                    &["Start 3 threads · Claude · opus-5.5 on local, devbox, and gpu-box", "Message CI integration · Codex · devbox"],
                    crate::permission::BudgetFields { agent_turns: 100, threads: 20, hours: 12 },
                );
                let req = ItemBody::Request(yaac_protocol::RequestItem {
                    request_id: "r-perm".into(),
                    blocking: true,
                    state: RequestState::Pending,
                    resolution: None,
                    body,
                });
                self.emit_now(&orch, ThreadEvent::ItemStarted(ItemEvent { item: item("p2", t, ItemStatus::InProgress, req) }));
                self.summary_now(&orch, |s| {
                    s.state = ThreadState::NeedsAttention;
                    s.updated_at = crate::time::timestamp(base);
                });
            }
            Scene::Notices => {
                self.target_state("devbox", ConnectionStatus::Connected, None, None, Some("0.7"));
                self.target_state("staging", ConnectionStatus::Offline, Some("retrying in 20s"), None, None);
                self.target_state("wsl-ubuntu", ConnectionStatus::Disabled, Some("not running"), Some(true), None);
                let fix = k(FIX);
                let mut first = self.fleet.threads[&fix].items[0].clone();
                if let ItemBody::UserMessage(u) = &mut first.body {
                    u.parts.push(InputPart::Image(yaac_protocol::ImagePart { upload_id: "u2/screenshot.png".into(), name: None, media_type: None }));
                }
                if let Some(t) = self.fleet.threads.get_mut(&fix) {
                    t.missing.push("u2/screenshot.png".into());
                }
                self.emit_now(&fix, ThreadEvent::ItemUpdated(ItemUpdatedEvent { item: first, through_seq: None }));
                let mut eff = self.fleet.threads[&fix].effective.clone();
                eff.image_input = Support::Unsupported;
                self.emit_now(&fix, ThreadEvent::SettingsChanged(SettingsChangedEvent { effective: eff.clone() }));
                // The daemon was replaced by an older one: a new journal. The window sees the
                // new epoch, resets, and learns from `thread/read` which uploads are gone.
                self.fleet.new_epoch(&fix);
                self.emit_now(&fix, ThreadEvent::SettingsChanged(SettingsChangedEvent { effective: eff }));
            }
            Scene::BudgetReached => {
                let orch = k(ORCH);
                let root = orch.address();
                if let Some(b) = self.fleet.budgets.get_mut(&root) {
                    b.used.agent_turns = b.allowance.agent_turns;
                    b.used.threads_spawned = 7;
                    b.exhausted = true;
                    b.held = Some(2);
                }
                self.emit_now(&orch, ThreadEvent::ItemCompleted(ItemEvent {
                    item: done("o-budget", "o-t1", notice(
                        NoticeKind::BudgetExhausted,
                        "This orchestration reached its budget: 100 of 100 agent-started turns. Nothing was cancelled. 2 messages are waiting to be delivered.",
                        Some(budget_details(2)),
                    )),
                }));
                // Its own turn ended and the workers' replies wait: paused, not running (D164).
                self.summary_now(&orch, |s| {
                    s.updated_at = crate::time::timestamp(base);
                    s.state = ThreadState::Idle;
                });
            }
            Scene::WslConnections => {
                if let Some(t) = self.fleet.targets.iter_mut().find(|t| t.id.as_str() == "wsl-ubuntu") {
                    t.enabled = true;
                    t.state.status = ConnectionStatus::Connected;
                }
                self.fleet.targets.push(fleet::wsl_target("wsl-ubuntu-deploy", "Ubuntu", Some("deploy"), true));
                self.fleet.targets.push(fleet::wsl_target("wsl-debian", "Debian", None, false));
                self.targets_changed();
            }
            Scene::HarnessMarkup => {
                // What yaacd makes of Claude Code's markup (D141): a pasted log and a slash
                // command in the user's messages, a reminder kept as context, and notices for a
                // subagent's report and background tasks.
                use yaac_protocol::TextRole;
                let users = k(USERS);
                let log = "INFO  migrate: batch 1/4 committed (2500 rows)\nINFO  migrate: batch 2/4 committed (2500 rows)\nINFO  migrate: batch 3/4 committed (2500 rows)\nWARN  migrate: batch 4/4 lock wait 30s on users_email_idx\nERROR migrate: batch 4/4 aborted: deadlock detected\nINFO  retry: attempt 1 from cursor 7500\nINFO  retry: batch 4/4 skipped (cursor already at 10000)\nINFO  migrate: done · 7500 of 10000 rows\nWARN  verify: 2500 rows missing in users_v2\nERROR verify: count mismatch (7500 != 10000)\nINFO  cleanup: dropped temp table users_tmp\nINFO  cleanup: released advisory lock 42";
                let parts = vec![
                    InputPart::text("The migration drops the last batch. Here is the log:"),
                    InputPart::text_as(log, TextRole::Pasted, None),
                    InputPart::text("Why does the retry skip it?"),
                    InputPart::text_as("The user is reviewing on a phone; keep answers short.", TextRole::Context, Some("System reminder")),
                ];
                let t2 = "u-t2";
                let t3 = "u-t3";
                let items = vec![
                    done("um1", t2, ItemBody::UserMessage(yaac_protocol::UserMessageItem { parts, op_id: None })),
                    done("um2", t2, agent("The retry reads the cursor **before** batch 4 rolls back, so it starts at 10000 and finds nothing left. Read the cursor after the rollback.")),
                    done("um3", t3, ItemBody::UserMessage(yaac_protocol::UserMessageItem {
                        parts: vec![
                            InputPart::text_as("/model opus-5.5", TextRole::Command, None),
                            InputPart::text_as("Set model to opus-5.5", TextRole::Context, Some("Command output")),
                        ],
                        op_id: None,
                    })),
                    done("um4", t3, notice_body(NoticeKind::Info, "Subagent schema-check reported back", "Checked all 41 tables against users_v2. Only users_email_idx differs: it is unique in v2 and was not in v1, which is what deadlocks batch 4.")),
                    done("um5", t3, notice_body(NoticeKind::Info, "Background command \"cargo test -p users\" completed (exit code 0)", "Background command \"cargo test -p users\" completed (exit code 0)\n\nOutput: /tmp/tasks/b7k2.output")),
                    done("um6", t3, notice_body(NoticeKind::Info, "2 background shell commands didn't finish before the previous session ended", "2 background shell commands didn't finish before the previous session ended.\n\nThey were stopped. Run them again if you still need them.")),
                ];
                for it in items {
                    self.emit_now(&users, ThreadEvent::ItemCompleted(ItemEvent { item: it }));
                }
                self.summary_now(&users, |s| s.updated_at = crate::time::timestamp(base));
            }
            Scene::QueueWhileRunning => {
                let billing = k(BILLING);
                let turn = TurnId::new("b-run");
                let t = turn.as_str();
                let msg = "Add a retry with backoff around the invoice webhook, and cover it with a test.";
                self.emit_now(&billing, ThreadEvent::ItemCompleted(ItemEvent { item: done("bq-user", t, ItemBody::UserMessage(yaac_protocol::UserMessageItem { parts: vec![InputPart::text(msg)], op_id: None })) }));
                self.emit_now(&billing, ThreadEvent::TurnStarted(TurnStartedEvent { turn_id: turn.clone(), origin: TurnOrigin::User, at: Some(crate::time::timestamp(base - 50)) }));
                self.emit_now(&billing, ThreadEvent::ItemCompleted(ItemEvent { item: done("bq-read", t, tool(ToolKind::Other, "Read `src/webhooks/invoice.rs`", None, "")) }));
                self.emit_now(&billing, ThreadEvent::ItemCompleted(ItemEvent { item: done("bq-say", t, agent("The webhook handler posts once and gives up on any error. I will wrap the call in a retry with exponential backoff, capped at five attempts, and keep the idempotency key the same across attempts.")) }));
                self.emit_now(&billing, ThreadEvent::ItemStarted(ItemEvent { item: item("bq-test", t, ItemStatus::InProgress, tool(ToolKind::Command, "cargo test -p billing webhook", None, "running 12 tests\n")) }));
                self.set_state_now(&billing, ThreadState::Running);
                for (i, text) in ["Also log each retry at debug level, with the attempt number.", "When the tests pass, open a draft PR against main."].iter().enumerate() {
                    let entry = yaac_protocol::QueuedMessage { queue_id: format!("bq-{}", i + 1), input: vec![InputPart::text(*text)], queued_at: crate::time::timestamp(base - 20 + i as i64 * 10), model: None, effort: None, mode: None, fast: None };
                    self.emit_now(&billing, ThreadEvent::QueueChanged(QueueChangedEvent { change: QueueChange::Added, entry, turn_id: None }));
                }
            }
            Scene::HarnessSubagents => {
                // Claude Code's own subagents (D151) appear under their parent as the harness
                // reports them: listed, linked, and the parent counting them.
                let kids = self.fleet.add_harness_subagents();
                for key in kids.iter().chain(std::iter::once(&k(SEARCH))) {
                    self.summary_now(key, |_| {});
                }
            }
            Scene::ManyAgents => {
                // Thirty related agents on the orchestrator (D168): its own Codex subagents and
                // users-service's Claude Code subagents join the fleet's six.
                let kids = self.fleet.add_many_agents();
                for key in kids.iter().chain([&k(ORCH), &k(USERS)]) {
                    self.summary_now(key, |_| {});
                }
            }
            Scene::LongConversation => {
                let parser = k(PARSER);
                for ev in long::events(base) {
                    self.emit_now(&parser, ev);
                }
                self.summary_now(&parser, |s| s.updated_at = crate::time::timestamp(base - 60));
            }
            Scene::LocalSignedOut => {
                let local = TargetId::new("local");
                if let Some(c) = self.fleet.providers.get_mut(&local).and_then(|ps| ps.iter_mut().find(|x| x.instance.as_str() == "claude")) {
                    c.status = yaac_protocol::ProviderStatus::NeedsAuth;
                    c.status_message = Some(fleet::SIGN_IN_HINT.into());
                    c.revision += 1;
                    let d = c.clone();
                    self.ready.push_back(routed(&local, m::PROVIDER_UPDATED, json!({ "provider": d })));
                }
            }
            Scene::StagingReconnects => {
                self.target_state("staging", ConnectionStatus::Connected, None, None, None);
                let (orch, notes) = (k(ORCH), k(NOTES));
                self.emit_now(&orch, ThreadEvent::ItemCompleted(ItemEvent {
                    item: done("o7", "o-t1", notice(NoticeKind::Info, "staging reconnected · 3h 12m after it went offline", None)),
                }));
                let mut o6 = self.fleet.threads[&orch].items.iter().find(|i| i.id.as_str() == "o6").cloned();
                if let Some(o6) = &mut o6
                    && let ItemBody::MeshSent(s) = &mut o6.body
                {
                    s.receipt = delivered(Some("after 3h 12m queued"));
                    s.to.reachable = Some(true);
                }
                if let Some(o6) = o6 {
                    self.emit_now(&orch, ThreadEvent::ItemUpdated(ItemUpdatedEvent { item: o6, through_seq: None }));
                }
                let p_notes = self.fleet.peer(&notes);
                let p_orch = self.fleet.peer(&orch);
                self.emit_now(&orch, ThreadEvent::ItemCompleted(ItemEvent {
                    item: done("o8", "o-t1", received(
                        message("m-notes-reply", &notes.address(), &orch.address(), MeshMessageKind::Message, "Draft is in `docs/release/auth-migration.md`.", crate::time::timestamp(base - 30)),
                        p_notes,
                    )),
                }));
                let old = yaac_protocol::Address::new(&TargetId::new("staging"), &ThreadId::new("t-old-changelog"));
                self.emit_now(&orch, ThreadEvent::ItemCompleted(ItemEvent {
                    item: done("o9", "o-t1", sent(
                        message("m-old", &orch.address(), &old, MeshMessageKind::Message, "Archive the 1.x notes.", crate::time::timestamp(base - 20)),
                        MeshPeer { address: old.clone(), title: Some("Old changelog bot".into()), provider: Some("Codex".into()), model: None, target_name: Some("staging".into()), reachable: Some(true) },
                        failed(ReceiptReason::ThreadDeleted),
                    )),
                }));
                self.emit_now(&orch, ThreadEvent::ItemCompleted(ItemEvent {
                    item: done("o10", "o-t1", agent("The release notes draft is ready. I could not reach the old changelog bot · that thread no longer exists.")),
                }));
                // The other end (24).
                let sent_at = crate::time::timestamp(base - 3 * 3600 - 840);
                self.fleet.push(&notes, done("n1", "n-t1", received(
                    message("m-to-notes", &orch.address(), &notes.address(), MeshMessageKind::Message, "Draft the notes for the auth migration.", sent_at),
                    p_orch.clone(),
                )));
                self.fleet.push(&notes, done("n2", "n-t1", reasoning("Collect the merged changes of the three workers.")));
                self.fleet.time_item(&notes, "n2", -3 * 3600 - 800, -3 * 3600 - 794);
                self.fleet.push(&notes, done("n3", "n-t1", tool(ToolKind::FileChange, "Wrote `docs/release/auth-migration.md`", None, "")));
                self.fleet.push(&notes, done("n4", "n-t1", agent("Drafted the release notes.")));
                self.fleet.push(&notes, done("n5", "n-t1", sent(
                    message("m-notes-reply", &notes.address(), &orch.address(), MeshMessageKind::Message, "Draft is in `docs/release/auth-migration.md`.", crate::time::timestamp(base - 30)),
                    p_orch,
                    delivered(None),
                )));
                if let Some(t) = self.fleet.threads.get(&notes)
                    && t.subscribed
                {
                    let evs: Vec<ThreadEventParams> = t.journal.iter().rev().take(5).rev().cloned().collect();
                    for ev in evs {
                        self.ready.push_back(routed(&notes.target, m::THREAD_EVENT, json!(ev)));
                    }
                }
                self.summary_now(&notes, |s| {
                    s.updated_at = crate::time::timestamp(base - 30);
                    s.preview = "Drafted the release notes.".into();
                });
            }
        }
    }
}

// Silence an unused-constant lint for keys used only by some scenes.
const _: [(&str, &str); 5] = [BILLING, CI, HANDSHAKE, LINT, PARSER];
const _: Seq = Seq(0);

/// The transcript a handoff fork would carry, in `yaacd`'s format (docs/05-threads.md, Fork):
/// messages in full, one line per tool call with its outcome, no reasoning. With the turn it
/// cuts off at: the last one.
fn demo_transcript(title: &str, items: &[Item]) -> (String, Option<TurnId>) {
    let mut turns: Vec<&TurnId> = Vec::new();
    for it in items {
        if !it.turn_id.as_str().is_empty() && !turns.contains(&&it.turn_id) {
            turns.push(&it.turn_id);
        }
    }
    let mut blocks = vec![
        "# Prior conversation".to_owned(),
        format!("Forked from \"{title}\" (Codex on devbox), through turn {}.\nHidden reasoning and full tool output are not included.", turns.len()),
    ];
    let mut current: Option<&TurnId> = None;
    for it in items {
        if it.turn_id.as_str().is_empty() {
            continue;
        }
        if current != Some(&it.turn_id) {
            current = Some(&it.turn_id);
            let n = turns.iter().position(|t| *t == &it.turn_id).unwrap_or(0) + 1;
            blocks.push(format!("## Turn {n}"));
        }
        match &it.body {
            ItemBody::UserMessage(u) => {
                let text: Vec<&str> = u.parts.iter().filter_map(|p| if let InputPart::Text(t) = p { Some(t.text.as_str()) } else { None }).collect();
                blocks.push("**User:**".to_owned());
                blocks.push(text.join("\n\n"));
            }
            ItemBody::AgentMessage(a) => {
                blocks.push("**Agent:**".to_owned());
                blocks.push(a.text.clone());
            }
            ItemBody::ToolCall(t) => {
                let outcome = match it.status {
                    yaac_protocol::ItemStatus::Failed => "failed",
                    yaac_protocol::ItemStatus::Declined => "declined",
                    _ => "succeeded",
                };
                blocks.push(format!("- Tool ({}): `{}` · {outcome}", t.tool.as_str(), t.title.replace('`', "'")));
            }
            _ => {}
        }
    }
    let mut out = String::new();
    for (i, b) in blocks.iter().enumerate() {
        if i > 0 {
            out.push_str(if b.starts_with("- ") && blocks[i - 1].starts_with("- ") { "\n" } else { "\n\n" });
        }
        out.push_str(b);
    }
    out.push('\n');
    (out, turns.last().map(|t| (*t).clone()))
}
