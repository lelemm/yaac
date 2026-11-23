//! The window's application state. Pure data plus the reducers that apply hub messages to it.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use yaac_protocol::attach::{ConnectionStatus, TargetEntry, TargetStateParams, TargetTransport};
use yaac_protocol::{
    Address, ClientId, Control, EffortId, InstanceId, ItemId, MeshBudgetResult, ModeId, ModelId,
    FsListResult, ProviderDescriptor, ProviderInstanceConfig, SyncToken,
    ThreadForkPreviewResult, TargetId, TargetPath, ThreadEventParams, ThreadSummary,
    ThreadSyncResult, ProjectKind,
};

use crate::hints::{HintChange, Hints, RequestKindWord};
use crate::key::ThreadKey;
use crate::transcript::Transcript;

/// A target as the window knows it.
#[derive(Clone, Debug)]
pub struct TargetRecord {
    /// The hub's entry, including connection state.
    pub entry: TargetEntry,
    /// Provider instances reported by the target (or the cache).
    pub providers: Vec<ProviderDescriptor>,
    /// The thread index token.
    pub sync_token: SyncToken,
    /// Whether a full index has been received at least once.
    pub synced: bool,
    /// Instances whose last scan was incomplete.
    pub stale_instances: Vec<InstanceId>,
    /// The providers were read from the connected target in this session (not only from the
    /// cache): the model picker says "updated just now".
    pub providers_fresh: bool,
    /// The stored instance configurations (`providerInstance/list`), once asked for.
    pub instance_configs: Option<Vec<ProviderInstanceConfig>>,
    pending_full: Option<Vec<ThreadSummary>>,
}

impl TargetRecord {
    /// A record for a hub entry.
    pub fn new(entry: TargetEntry) -> Self {
        Self {
            entry,
            providers: Vec::new(),
            sync_token: SyncToken::default(),
            synced: false,
            stale_instances: Vec::new(),
            providers_fresh: false,
            instance_configs: None,
            pending_full: None,
        }
    }

    /// Whether the target is connected now.
    pub fn connected(&self) -> bool {
        self.entry.state.status == ConnectionStatus::Connected
    }

    /// "SSH", "WSL", or nothing for the local machine — for the origin row.
    pub fn transport_word(&self) -> Option<&'static str> {
        match &self.entry.transport {
            TargetTransport::Ssh(_) => Some("SSH"),
            TargetTransport::Wsl(_) => Some("WSL"),
            TargetTransport::Local(_) | TargetTransport::Unknown(_) => None,
        }
    }

    /// The instance descriptor.
    pub fn provider(&self, instance: &InstanceId) -> Option<&ProviderDescriptor> {
        self.providers.iter().find(|p| &p.instance == instance)
    }
}

/// A thread in the index.
#[derive(Clone, Debug)]
pub struct ThreadRecord {
    /// The summary as last reported.
    pub summary: ThreadSummary,
    /// Ordering stamp, compared with the hints' stamp to pick the fresher source.
    pub stamp: u64,
}

/// How an image chip will be sent, when the user chose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendAs {
    /// As image input.
    Image,
    /// As a file path.
    File,
}

pub use crate::attachments::DraftAttachment;

/// Unsent composer content.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Draft {
    /// The text.
    pub text: String,
    /// Attachments.
    pub attachments: Vec<DraftAttachment>,
}

impl Draft {
    /// No text and no attachments: nothing to keep.
    pub fn is_empty(&self) -> bool {
        self.text.is_empty() && self.attachments.is_empty()
    }
}

/// A thread that has not sent its first message: host-only.
#[derive(Clone, Debug, PartialEq)]
pub struct NewThreadDraft {
    /// Local id.
    pub id: u64,
    /// The chosen target.
    pub target: TargetId,
    /// The chosen project (the thread starts in its folder's root).
    pub project: Option<String>,
    /// The working directory (the project's folder).
    pub cwd: Option<TargetPath>,
    /// The chosen instance.
    pub instance: Option<InstanceId>,
    /// The chosen model.
    pub model: Option<ModelId>,
    /// The chosen effort.
    pub effort: Option<EffortId>,
    /// The chosen mode; `None` means the harness's own default.
    pub mode: Option<ModeId>,
    /// Fast mode (D125), offered only when the chosen model has it.
    pub fast: bool,
    /// "Fast mode is off …": a quiet note after a model change turned fast mode off.
    pub fast_note: Option<String>,
    /// Composer content.
    pub draft: Draft,
}

/// Selections for the next message of a started thread (sent as overrides on `turn/start`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NextSettings {
    /// Model override.
    pub model: Option<ModelId>,
    /// Effort override.
    pub effort: Option<EffortId>,
    /// Mode override.
    pub mode: Option<ModeId>,
    /// Fast mode on or off (D125); `None` keeps the thread's own setting.
    pub fast: Option<bool>,
    /// "Fast mode is off …": a quiet note after a model change turned fast mode off.
    pub fast_note: Option<String>,
}

/// What fills the centre of the window.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Selection {
    /// Nothing yet.
    #[default]
    None,
    /// A thread.
    Thread(ThreadKey),
    /// A new-thread draft.
    NewThread(u64),
    /// Settings (a full-window page; the app keeps which page is open).
    Settings,
}

/// "index-rebuild needs an approval · Ctrl+J to go there".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Arrival {
    /// The thread.
    pub key: ThreadKey,
    /// What arrived.
    pub kind: RequestKindWord,
}

/// The sidebar search, answered by the hub across every cached target.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FindState {
    /// The query these results are for.
    pub query: String,
    /// Still asking.
    pub loading: bool,
    /// Matches so far (pages appended), as thread keys.
    pub results: Vec<ThreadKey>,
    /// More pages: the cursor to ask with.
    pub cursor: Option<String>,
    /// Total matches.
    pub total: u32,
    /// The failure, shown as-is.
    pub error: Option<String>,
}

/// Whether a chat's agent may start and message other threads without asking.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AgentsPolicy {
    /// Ask for permission (the default for a new chat).
    #[default]
    Ask,
    /// Allow without asking.
    Allow,
}

/// How `hub/hello` ended.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum HelloOutcome {
    /// Not answered yet.
    #[default]
    Pending,
    /// The same build: attached.
    Attached,
    /// The window is newer than the running hub: ask it to shut down (`hub/shutdown`) and
    /// start this build's hub.
    HubOutdated {
        /// The running hub's build.
        hub_build: String,
    },
    /// The window is older than the running hub: tell the user to update YAAC, and attach
    /// to nothing.
    WindowOutdated {
        /// The hub's message, shown as-is.
        message: String,
    },
}

/// The hub attachment.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HubState {
    /// `hub/hello` completed with the same build.
    pub attached: bool,
    /// How `hub/hello` ended.
    pub outcome: HelloOutcome,
    /// The hub's build.
    pub build: Option<String>,
    /// The last error the hub reported, for the footer.
    pub last_error: Option<String>,
    /// The old hub answered `hub/shutdown` (it is exiting).
    pub shutdown_answered: bool,
    /// Whether the hub keeps running and starts at login (`hub/settings/get`), once known.
    pub always: Option<bool>,
}

/// A `thread/forkPreview` in flight or answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ForkPreviewState {
    /// Asked; no answer yet.
    Loading,
    /// The target's answer.
    Ready(ThreadForkPreviewResult),
    /// The request failed; the message is shown as-is.
    Failed(String),
}

/// An `fs/list` in flight or answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FsListingState {
    /// Asked; no answer yet.
    Loading,
    /// The listing.
    Ready(FsListResult),
    /// The request failed; the message is shown as-is.
    Failed(String),
}

/// Everything the window shows, as data.
#[derive(Clone, Debug, Default)]
pub struct AppState {
    /// The hub.
    pub hub: HubState,
    /// Targets, in the hub's order.
    pub targets: Vec<TargetRecord>,
    /// The thread index across targets.
    pub threads: BTreeMap<ThreadKey, ThreadRecord>,
    /// Hints per thread.
    pub hints: BTreeMap<ThreadKey, Hints>,
    /// Transcripts held in memory: the open thread plus a small least-recently-used set.
    pub transcripts: BTreeMap<ThreadKey, Transcript>,
    /// Most recent first; bounds `transcripts`.
    pub recent: VecDeque<ThreadKey>,
    /// Orchestration budgets, by root.
    pub budgets: BTreeMap<Address, MeshBudgetResult>,
    /// Composer drafts of started threads.
    pub drafts: BTreeMap<ThreadKey, Draft>,
    /// New-thread drafts.
    pub new_threads: Vec<NewThreadDraft>,
    /// Next-message settings per thread.
    pub next_settings: BTreeMap<ThreadKey, NextSettings>,
    /// A queued message being edited in a thread's composer (D166), by thread.
    pub queue_edits: BTreeMap<ThreadKey, crate::queue::QueueEdit>,
    /// The centre.
    pub selection: Selection,
    /// This window's client id, to recognise "answered by you".
    pub client_id: Option<ClientId>,
    /// Requests that arrived while the user was elsewhere.
    pub arrivals: Vec<Arrival>,
    /// The projects of every target (`project/list`, `project/updated`), with window-unique
    /// ids (see [`crate::projects::Project`]).
    pub projects: Vec<crate::projects::Project>,
    /// `hub/threads/find` results for the sidebar search, when attached to a real hub.
    pub find: Option<FindState>,
    /// What YAAC did on each SSH target during setup (when, the line it installed): from the
    /// hub's `hub/ssh/records`, per server, shown on every target of that server (window-local
    /// only with the demo hub).
    pub ssh_records: BTreeMap<TargetId, crate::ssh_setup::SetupRecord>,
    /// SEAM (wire field missing): the per-instance switch "Offer YAAC tools to sessions
    /// started outside YAAC". No wire field or method exists; window-local.
    pub outside_tools: BTreeSet<(TargetId, InstanceId)>,
    /// A mode that bypasses approvals, chosen and waiting for confirmation (screen 30). Never
    /// remembered beyond the one choice.
    pub mode_confirm: Option<crate::mode_confirm::PendingModeConfirm>,
    /// A harness-home change that leads to another store, waiting for the user's
    /// confirmation (P300). Never sent again on its own.
    pub home_confirm: Option<crate::settings::HomeConfirm>,
    /// SEAM (wire field missing): the WSL master switch (off by default). The hub settings
    /// carry no such field; window-local.
    pub wsl_on: bool,
    /// The installed WSL distros (`hub/wsl/distros`) and the one chosen to add.
    pub wsl_distros: crate::settings::WslDistros,
    /// `thread/forkPreview` answers, by source thread and destination instance.
    pub fork_previews: BTreeMap<(ThreadKey, InstanceId), ForkPreviewState>,
    /// `fs/list` answers, by target and directory asked for ("" for the home directory).
    pub fs_listings: BTreeMap<(TargetId, String), FsListingState>,
    /// An item to scroll to and highlight (navigation from a To/From link or Needs you).
    pub focus_item: Option<(ThreadKey, ItemId)>,
    /// Bounded transcript cache size (open thread excluded).
    pub transcript_capacity: usize,
    /// View preferences per thread — pin, mute, dismiss — kept by the hub on this computer
    /// (D92; filled from `hub/view/get`, see the SEAM in `client::hub_names`). Never read from
    /// the thread summary.
    pub view: BTreeMap<ThreadKey, ViewPref>,
    /// Pull requests of the threads' branches (D119), from `forge/prs` and `forge/prs/changed`.
    pub prs: crate::pr::PrStore,
    /// The Create PR modal (D120), while open.
    pub create_pr: Option<crate::pr::CreatePr>,
    /// Plan rate limits of every account on any target (D161), from the hub.
    pub rate_limits: crate::limits::Accounts,
    stamp: u64,
}

/// One thread's view preferences.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ViewPref {
    /// Pinned to the top of the list.
    pub pinned: bool,
    /// Muted.
    pub muted: bool,
    /// Muted until this moment (seconds since the epoch); `None` with `muted`: no end.
    pub muted_until: Option<i64>,
    /// A discovered thread hidden in the window.
    pub dismissed: bool,
    /// A turn ended while no window showed the thread (D114); opening it clears this.
    pub unseen: bool,
}

/// A change to view preferences: only the fields given change.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ViewPatch {
    /// Pin or unpin.
    pub pinned: Option<bool>,
    /// Mute or unmute.
    pub muted: Option<bool>,
    /// Set (or clear, with `Some(None)`) the end of a mute.
    pub muted_until: Option<Option<i64>>,
    /// Dismiss or bring back.
    pub dismissed: Option<bool>,
    /// Mark or clear "finished while nobody looked" (D114).
    pub unseen: Option<bool>,
}

impl AppState {
    /// Empty state.
    pub fn new() -> Self {
        Self { transcript_capacity: 8, ..Self::default() }
    }

    /// A fresh ordering stamp (also a revision number for UI caches).
    pub fn next_stamp(&mut self) -> u64 {
        self.stamp += 1;
        self.stamp
    }

    /// The target record.
    pub fn target(&self, id: &TargetId) -> Option<&TargetRecord> {
        self.targets.iter().find(|t| &t.entry.id == id)
    }

    fn target_mut(&mut self, id: &TargetId) -> Option<&mut TargetRecord> {
        self.targets.iter_mut().find(|t| &t.entry.id == id)
    }

    /// The target's display name.
    pub fn target_name(&self, id: &TargetId) -> String {
        self.target(id).map_or_else(|| id.to_string(), |t| t.entry.name.to_string())
    }

    /// A thread's summary.
    pub fn summary(&self, key: &ThreadKey) -> Option<&ThreadSummary> {
        self.threads.get(key).map(|r| &r.summary)
    }

    /// The model a thread's next message goes to: the one chosen for it, else the one the
    /// harness reports applied, else the summary's — as listed by its instance.
    pub fn next_model_info(&self, key: &ThreadKey) -> Option<&yaac_protocol::ModelInfo> {
        let p = self.provider_of(key)?;
        let id = self
            .next_settings
            .get(key)
            .and_then(|n| n.model.as_ref())
            .or_else(|| self.transcripts.get(key).and_then(|t| t.effective.as_ref()).and_then(|e| e.applied.model.as_ref()))
            .or_else(|| self.summary(key).and_then(|s| s.model.as_ref()))?;
        p.models.iter().find(|m| &m.id == id)
    }

    /// Whether a thread's next turns run in fast mode (D125): the choice made here, else what
    /// the target reports.
    pub fn thread_fast(&self, key: &ThreadKey) -> bool {
        self.next_settings
            .get(key)
            .and_then(|n| n.fast)
            .unwrap_or_else(|| self.summary(key).is_some_and(|s| s.fast))
    }

    /// The provider descriptor of a thread.
    pub fn provider_of(&self, key: &ThreadKey) -> Option<&ProviderDescriptor> {
        let summary = self.summary(key)?;
        self.target(&key.target)?.provider(&summary.instance)
    }

    /// Who may act on a thread: the subscription's report when held, else the summary's.
    /// Unknown is read-only.
    pub fn control(&self, key: &ThreadKey) -> Control {
        if let Some(c) = self.transcripts.get(key).and_then(|t| t.control.clone()) {
            return c;
        }
        self.summary(key).map_or(Control::Unknown("unknown".into()), |s| s.control.clone())
    }

    /// Where a read-only thread is in use.
    pub fn control_holder(&self, key: &ThreadKey) -> Option<String> {
        if let Some(t) = self.transcripts.get(key)
            && t.control.is_some()
        {
            return t.control_holder.clone();
        }
        self.summary(key).and_then(|s| s.control_holder.clone())
    }

    /// Whether an edit, cancel or Send now of this window is in flight for a queued message.
    pub fn queue_busy(&self, key: &ThreadKey, queue_id: &str) -> bool {
        self.transcripts.get(key).is_some_and(|t| t.queue_busy.contains(queue_id))
    }

    /// Whether this window may send, steer, and answer on a thread.
    pub fn may_act(&self, key: &ThreadKey) -> bool {
        self.control(key).may_act() && self.stopped_distro(&key.target).is_none() && self.store_away(key).is_none()
    }

    /// Why the thread cannot be used while its instance leads to another store (P10, P300),
    /// as the target said; `None` when it can.
    pub fn store_away(&self, key: &ThreadKey) -> Option<String> {
        self.summary(key).and_then(|s| s.unavailable.clone())
    }

    /// Whether this window may answer a thread's approvals and questions. As `may_act`, except
    /// for a read-only harness sub-agent (D151): it is held by the agent that started it, but
    /// what it asks is the user's (D162), answered by whoever may act on its parent.
    pub fn may_answer(&self, key: &ThreadKey) -> bool {
        let Some(s) = self.summary(key) else { return self.may_act(key) };
        match (&s.subagent, &s.spawned_by) {
            (Some(_), Some(parent)) => match self.key_of(parent) {
                Some(p) => self.may_act(&p),
                // Its parent is not listed here: nothing says another writer holds it.
                None => self.stopped_distro(&key.target).is_none(),
            },
            _ => self.may_act(key),
        }
    }

    /// A WSL distro that is down (D96): its name, while the target is a WSL connection that is
    /// turned on but not connected. Its threads stay listed and readable from the cache, but
    /// every state-changing action on them is unavailable until it is started again — which
    /// asks the hub to connect (booting the distro); when the connection comes up the threads
    /// are responsive again by themselves.
    pub fn stopped_distro(&self, target: &TargetId) -> Option<String> {
        let t = self.target(target)?;
        match &t.entry.transport {
            TargetTransport::Wsl(w) if t.entry.enabled && t.entry.state.status != ConnectionStatus::Connected => Some(w.distro.clone()),
            _ => None,
        }
    }

    /// The selected thread, when a thread is selected.
    pub fn selected_thread(&self) -> Option<&ThreadKey> {
        match &self.selection {
            Selection::Thread(k) => Some(k),
            _ => None,
        }
    }

    /// The key of the thread at a mesh address, when it is in the index.
    pub fn key_of(&self, address: &Address) -> Option<ThreadKey> {
        let key = ThreadKey::from_address(address)?;
        self.threads.contains_key(&key).then_some(key)
    }

    /// Children of each parent address, from every summary's `spawned_by`.
    pub fn children_index(&self) -> BTreeMap<Address, Vec<ThreadKey>> {
        let mut map: BTreeMap<Address, Vec<ThreadKey>> = BTreeMap::new();
        for (key, rec) in &self.threads {
            if let Some(parent) = &rec.summary.spawned_by {
                map.entry(parent.clone()).or_default().push(key.clone());
            }
        }
        map
    }

    // ── reducers ────────────────────────────────────────────────────────────────────────

    /// Replace the target list (from `hub/hello` or `hub/targets/list`).
    pub fn set_targets(&mut self, entries: Vec<TargetEntry>) {
        let mut old: Vec<TargetRecord> = std::mem::take(&mut self.targets);
        for entry in entries {
            match old.iter().position(|t| t.entry.id == entry.id) {
                Some(i) => {
                    let mut rec = old.swap_remove(i);
                    rec.entry = entry;
                    self.targets.push(rec);
                }
                None => self.targets.push(TargetRecord::new(entry)),
            }
        }
    }

    /// `hub/target/state`.
    pub fn apply_target_state(&mut self, params: TargetStateParams) {
        if let Some(t) = self.target_mut(&params.id) {
            t.entry.state = params.state;
            t.entry.enabled = params.enabled;
        }
    }

    /// `provider/list` result, or `provider/updated`.
    pub fn apply_providers(&mut self, target: &TargetId, providers: Vec<ProviderDescriptor>, replace: bool) {
        let Some(t) = self.target_mut(target) else { return };
        if replace {
            t.providers = providers;
            return;
        }
        for p in providers {
            match t.providers.iter_mut().find(|x| x.instance == p.instance) {
                Some(slot) if slot.revision <= p.revision => *slot = p,
                Some(_) => {}
                None => t.providers.push(p),
            }
        }
    }

    /// A summary changed or appeared.
    pub fn upsert_summary(&mut self, target: &TargetId, summary: ThreadSummary) {
        let stamp = self.next_stamp();
        let key = ThreadKey::new(target.clone(), summary.id.clone());
        self.threads.insert(key, ThreadRecord { summary, stamp });
    }

    /// A thread is gone.
    pub fn remove_thread(&mut self, key: &ThreadKey) {
        self.threads.remove(key);
        self.hints.remove(key);
        self.transcripts.remove(key);
        self.recent.retain(|k| k != key);
    }

    /// One page of `thread/sync`. A full listing replaces this target's index once its last
    /// page arrives; an incremental one merges.
    pub fn apply_sync(&mut self, target: &TargetId, result: ThreadSyncResult) {
        let Some(t) = self.target_mut(target) else { return };
        t.sync_token = result.token;
        t.stale_instances = result.stale_instances;
        if result.full || t.pending_full.is_some() {
            let pending = t.pending_full.get_or_insert_with(Vec::new);
            pending.extend(result.changed);
            if result.more {
                return;
            }
            let all = t.pending_full.take().unwrap_or_default();
            t.synced = true;
            let keep: Vec<ThreadKey> =
                self.threads.keys().filter(|k| &k.target == target).cloned().collect();
            for key in keep {
                if !all.iter().any(|s| s.id == key.thread) {
                    self.remove_thread(&key);
                }
            }
            for s in all {
                self.upsert_summary(target, s);
            }
        } else {
            t.synced = true;
            for s in result.changed {
                self.upsert_summary(target, s);
            }
        }
        for id in result.removed {
            self.remove_thread(&ThreadKey::new(target.clone(), id));
        }
    }

    /// A `thread/event` for any thread: updates its hints always, and its transcript when held.
    pub fn apply_thread_event(&mut self, target: &TargetId, event: &ThreadEventParams) {
        let key = ThreadKey::new(target.clone(), event.thread_id.clone());
        let stamp = self.next_stamp();
        let change: HintChange = self.hints.entry(key.clone()).or_default().apply(event, stamp);
        if let Some(kind) = change.arrived
            && self.selected_thread() != Some(&key)
            && self.may_answer(&key)
        {
            self.arrivals.retain(|a| a.key != key);
            self.arrivals.push(Arrival { key: key.clone(), kind });
        }
        if let Some(t) = self.transcripts.get_mut(&key) {
            t.apply(event, stamp);
        }
    }

    /// The budget of an orchestration.
    pub fn apply_budget(&mut self, result: MeshBudgetResult) {
        self.budgets.insert(result.root.clone(), result);
    }

    /// Mark a thread as recently opened, evicting old transcripts beyond the capacity.
    pub fn touch(&mut self, key: &ThreadKey) {
        self.recent.retain(|k| k != key);
        self.recent.push_front(key.clone());
        while self.recent.len() > self.transcript_capacity + 1 {
            if let Some(old) = self.recent.pop_back() {
                if self.selected_thread() == Some(&old) {
                    self.recent.push_front(old);
                    break;
                }
                self.transcripts.remove(&old);
            }
        }
    }

    /// Whether a thread is muted at `now`: muted with no end, or until a moment still ahead.
    /// (A timed mute needs no timer: it simply stops applying.)
    pub fn is_muted(&self, key: &ThreadKey, now: i64) -> bool {
        self.view.get(key).is_some_and(|v| v.muted && v.muted_until.is_none_or(|u| now < u))
    }

    /// When a thread's timed mute ends, if it is muted until a moment.
    pub fn muted_until(&self, key: &ThreadKey, now: i64) -> Option<i64> {
        self.view.get(key).filter(|_| self.is_muted(key, now)).and_then(|v| v.muted_until)
    }

    /// The nearest moment after `now` at which a timed mute ends: the window schedules one
    /// repaint for it, so the row comes back on time without repainting meanwhile.
    pub fn next_unmute(&self, now: i64) -> Option<i64> {
        self.view.values().filter(|v| v.muted).filter_map(|v| v.muted_until).filter(|u| *u > now).min()
    }

    /// Whether a thread is pinned (a view preference kept by the hub).
    pub fn is_pinned(&self, key: &ThreadKey) -> bool {
        self.view.get(key).is_some_and(|v| v.pinned)
    }

    /// Whether a thread's last turn ended while no window showed it (D114).
    pub fn is_unseen(&self, key: &ThreadKey) -> bool {
        self.view.get(key).is_some_and(|v| v.unseen)
    }

    /// Whether a discovered thread is hidden in the window.
    pub fn is_dismissed(&self, key: &ThreadKey) -> bool {
        self.view.get(key).is_some_and(|v| v.dismissed)
    }

    /// Merge `hub/view/get`'s result or a `hub/view/changed` notification:
    /// `{ "threads": [ { target, threadId, pinned?, muted?, mutedUntil?, dismissed? } ] }`.
    pub fn apply_view(&mut self, v: &serde_json::Value) {
        let Some(list) = v.get("views").or_else(|| v.get("threads")).and_then(serde_json::Value::as_array) else { return };
        for t in list {
            let (Some(target), Some(thread)) = (t.get("target").and_then(serde_json::Value::as_str), t.get("threadId").and_then(serde_json::Value::as_str)) else {
                continue;
            };
            let key = ThreadKey::new(yaac_protocol::TargetId::new(target), yaac_protocol::ThreadId::new(thread));
            let e = self.view.entry(key).or_default();
            let b = |k: &str| t.get(k).and_then(serde_json::Value::as_bool);
            if let Some(x) = b("pinned") {
                e.pinned = x;
            }
            if let Some(x) = b("muted") {
                e.muted = x;
            }
            if let Some(x) = b("dismissed") {
                e.dismissed = x;
            }
            if let Some(x) = b("unseen") {
                e.unseen = x;
            }
            if let Some(u) = t.get("mutedUntil") {
                e.muted_until = u.as_str().and_then(crate::time::parse_rfc3339);
            }
        }
    }

    /// The project a thread belongs to, if any: the user project its target resolved
    /// (`ProjectRef.kind = user`); a temporary project (or none known) is `None`. A target that
    /// does not resolve projects yet falls back to the folder rule over the known projects.
    pub fn project_of(&self, key: &ThreadKey) -> Option<&crate::projects::Project> {
        let s = self.summary(key)?;
        match s.project.as_ref().and_then(|p| p.kind.as_ref()) {
            Some(ProjectKind::User) => {
                let id = s.project.as_ref().and_then(|p| p.id.as_ref())?;
                self.projects.iter().find(|p| p.target == key.target && p.wire_id == id.as_str())
            }
            Some(_) => None,
            None => {
                let loc = crate::projects::Location {
                    target: key.target.clone(),
                    folder: s.cwd.to_string(),
                    repo: s.repo.as_ref().map(crate::projects::RepoLocation::from_wire),
                };
                crate::projects::project_for(&self.projects, &loc)
            }
        }
    }

    /// The chat's agents policy (default: ask).
    pub fn agents_policy(&self, key: &ThreadKey) -> AgentsPolicy {
        match self.summary(key).map(|s| &s.agents) {
            Some(yaac_protocol::AgentsPolicy::Allow) => AgentsPolicy::Allow,
            _ => AgentsPolicy::Ask,
        }
    }

    /// Replace one target's projects (from `project/list` or `project/updated`).
    pub fn set_projects(&mut self, target: &TargetId, projects: Vec<yaac_protocol::Project>) {
        self.projects.retain(|p| &p.target != target);
        self.projects.extend(projects.into_iter().map(|p| crate::projects::Project::from_wire(target, p)));
    }

    /// Forget arrivals for a thread (it was opened).
    pub fn clear_arrival(&mut self, key: &ThreadKey) {
        self.arrivals.retain(|a| &a.key != key);
    }
}
