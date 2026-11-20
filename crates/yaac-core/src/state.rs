//! The window's application state. Pure data plus the reducers that apply hub messages to it.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use yaac_protocol::attach::{ConnectionStatus, TargetEntry, TargetStateParams, TargetTransport};
use yaac_protocol::{
    Address, ClientId, Control, EffortId, InstanceId, ItemId, MeshBudgetResult, ModeId, ModelId,
    ProviderDescriptor, SyncToken, TargetId, TargetPath, ThreadEventParams, ThreadSummary,
    ThreadSyncResult, UploadId,
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

/// One attachment in a composer.
#[derive(Clone, Debug, PartialEq)]
pub struct DraftAttachment {
    /// Local id.
    pub id: u64,
    /// File name.
    pub name: String,
    /// Detected as an image (by content).
    pub is_image: bool,
    /// Upload progress 0..=1 while uploading; `None` when done.
    pub progress: Option<f32>,
    /// The user's choice when the model's image support is unknown.
    pub choice: Option<SendAs>,
    /// The upload, once begun.
    pub upload: Option<UploadId>,
}

/// Unsent composer content.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Draft {
    /// The text.
    pub text: String,
    /// Attachments.
    pub attachments: Vec<DraftAttachment>,
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
    /// Settings (a later package fills the pages).
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

/// A thread's git status, as compactly as the sidebar shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitStatus {
    /// The branch.
    pub branch: String,
    /// Files that differ from origin ("Δ7").
    pub changed_files: u32,
    /// Commits not pushed ("↑2").
    pub ahead: u32,
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

/// The hub attachment.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HubState {
    /// `hub/hello` completed.
    pub attached: bool,
    /// The hub's build.
    pub build: Option<String>,
    /// The last error the hub reported, for the footer.
    pub last_error: Option<String>,
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
    /// The centre.
    pub selection: Selection,
    /// This window's client id, to recognise "answered by you".
    pub client_id: Option<ClientId>,
    /// Requests that arrived while the user was elsewhere.
    pub arrivals: Vec<Arrival>,
    /// SEAM (wire field missing): muted threads. `ThreadSummary` has `archived` but no
    /// `muted`; until the protocol carries it, mute is window-local state.
    pub muted: BTreeSet<ThreadKey>,
    /// SEAM (wire field missing): pinned threads, window-local like `muted`.
    pub pinned: BTreeSet<ThreadKey>,
    /// SEAM (wire field missing): each thread's git status (branch, files differing from
    /// origin, commits not pushed). Absent: the folder is not in git (or not known).
    pub git: BTreeMap<ThreadKey, GitStatus>,
    /// SEAM (wire field missing): the projects the user created (see [`crate::projects`]).
    pub projects: Vec<crate::projects::Project>,
    /// SEAM (wire field missing): where each thread's folder sits in a repository, for the
    /// repository rule of [`crate::projects::project_for`].
    pub repo_locations: BTreeMap<ThreadKey, crate::projects::RepoLocation>,
    /// SEAM (wire field missing): the per-chat "agents: ask / allow" choice of threads a
    /// person started. No per-thread mesh policy exists on the wire yet.
    pub agents_policy: BTreeMap<ThreadKey, AgentsPolicy>,
    /// An item to scroll to and highlight (navigation from a To/From link or Needs you).
    pub focus_item: Option<(ThreadKey, ItemId)>,
    /// Bounded transcript cache size (open thread excluded).
    pub transcript_capacity: usize,
    stamp: u64,
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

    /// Whether this window may send, steer, and answer on a thread.
    pub fn may_act(&self, key: &ThreadKey) -> bool {
        self.control(key).may_act()
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
            && self.may_act(&key)
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

    /// Whether a thread is muted (see the seam on [`AppState::muted`]).
    pub fn is_muted(&self, key: &ThreadKey) -> bool {
        self.muted.contains(key)
    }

    /// Whether a thread is pinned (see the seam on [`AppState::pinned`]).
    pub fn is_pinned(&self, key: &ThreadKey) -> bool {
        self.pinned.contains(key)
    }

    /// The project a thread belongs to, if any.
    pub fn project_of(&self, key: &ThreadKey) -> Option<&crate::projects::Project> {
        let s = self.summary(key)?;
        let loc = crate::projects::Location {
            target: key.target.clone(),
            folder: s.cwd.to_string(),
            repo: self.repo_locations.get(key).cloned(),
        };
        crate::projects::project_for(&self.projects, &loc)
    }

    /// The chat's agents policy (default: ask).
    pub fn agents_policy(&self, key: &ThreadKey) -> AgentsPolicy {
        self.agents_policy.get(key).copied().unwrap_or_default()
    }

    /// Forget arrivals for a thread (it was opened).
    pub fn clear_arrival(&mut self, key: &ThreadKey) {
        self.arrivals.retain(|a| &a.key != key);
    }
}
