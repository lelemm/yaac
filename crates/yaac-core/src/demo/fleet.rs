//! The fleet of the screen brief (docs/design/screens.md, "The fleet used in every mockup").
//! Demo only: the fictional provider and model names live here and nowhere else.

use std::collections::BTreeMap;

use yaac_protocol::attach::{ConnectionState, ConnectionStatus, LocalTransport, SshTransport, TargetEntry, TargetTransport, WslTransport};
use yaac_protocol::{
    Address, AppliedSettings, BackendKind, Capability, ContextUsage, Control, DecisionKind, EffortId, EffortInfo, Epoch,
    InstanceId, Item, ItemStatus, MeshBudget, MeshBudgetResult, MeshBudgetUsage, MeshMessageKind, MeshPeer, ModeId,
    ModeInfo, ModelCapability, ModelId, ModelInfo, OriginKind, ProjectRef, ProviderDescriptor, ProviderKind,
    ProviderStatus, ReceiptReason, Seq, StoreIdentity, Support, TargetId, TargetName, TargetPath, ThreadEffective,
    ThreadEvent, ThreadEventParams, ThreadId, ThreadState, ThreadSummary, Timestamp, ToolKind, TurnId, UploadId,
    WorktreeRef, ItemEvent, GitStatus, Project, ProjectId, ProjectKind,
};

use super::build::*;
use crate::key::ThreadKey;
use crate::time::timestamp;

/// One demo thread with its journal.
#[derive(Clone, Debug)]
pub(crate) struct DemoThread {
    pub summary: ThreadSummary,
    pub items: Vec<Item>,
    pub journal: Vec<ThreadEventParams>,
    pub epoch: Epoch,
    pub control: Control,
    pub holder: Option<String>,
    /// Why the program holding it will not hand it over (D140).
    pub take_over_blocked: Option<String>,
    pub active_turn: Option<TurnId>,
    pub effective: ThreadEffective,
    pub context: Option<ContextUsage>,
    pub missing: Vec<UploadId>,
    pub subscribed: bool,
}

impl DemoThread {
    pub(super) fn head(&self) -> Seq {
        Seq(self.journal.len() as u64)
    }
}

/// The whole demo world.
#[derive(Clone, Debug)]
pub(crate) struct Fleet {
    pub base: i64,
    pub targets: Vec<TargetEntry>,
    pub providers: BTreeMap<TargetId, Vec<ProviderDescriptor>>,
    pub threads: BTreeMap<ThreadKey, DemoThread>,
    pub budgets: BTreeMap<Address, MeshBudgetResult>,
    /// Each target's projects.
    pub projects: BTreeMap<TargetId, Vec<Project>>,
    /// The hub's view preferences (pin, mute, dismiss), by thread (D92).
    pub view: BTreeMap<ThreadKey, crate::state::ViewPref>,
}

pub(crate) const ORCH: (&str, &str) = ("local", "t-orch");
pub(crate) const USERS: (&str, &str) = ("local", "t-users");
pub(crate) const BILLING: (&str, &str) = ("devbox", "t-billing");
pub(crate) const SEARCH: (&str, &str) = ("gpu-box", "t-search");
pub(crate) const INDEX: (&str, &str) = ("gpu-box", "t-index");
pub(crate) const CI: (&str, &str) = ("devbox", "t-ci");
pub(crate) const NOTES: (&str, &str) = ("staging", "t-notes");
pub(crate) const FIX: (&str, &str) = ("devbox", "t-fix");
pub(crate) const HANDSHAKE: (&str, &str) = ("devbox", "t-handshake");
pub(crate) const OLD_SPIKE: (&str, &str) = ("local", "t-spike");
pub(crate) const LINT: (&str, &str) = ("devbox", "t-lint");
pub(crate) const BENCH: (&str, &str) = ("gpu-box", "t-bench");
pub(crate) const V2PLAN: (&str, &str) = ("local", "t-v2plan");
pub(crate) const PARSER: (&str, &str) = ("devbox", "t-parser");

/// What the demo's signed-out instance reports as its status message.
pub(crate) const SIGN_IN_HINT: &str = "Run `claude` in a terminal to sign in";

pub(crate) fn wsl_target(id: &str, distro: &str, user: Option<&str>, enabled: bool) -> TargetEntry {
    target(
        id,
        &match user {
            Some(u) => format!("wsl: {distro} ({u})"),
            None => format!("wsl: {distro}"),
        },
        TargetTransport::Wsl(WslTransport { distro: distro.into(), user: user.map(str::to_owned) }),
        if enabled { ConnectionStatus::Connected } else { ConnectionStatus::Disabled },
        None,
        enabled,
    )
}

pub(crate) fn k(pair: (&str, &str)) -> ThreadKey {
    ThreadKey::new(pair.0, pair.1)
}

fn effort(id: &str) -> EffortInfo {
    EffortInfo { id: EffortId::new(id), display_name: Some(id.to_owned()), description: None }
}

fn model(id: &str, default: bool, images: Support) -> ModelInfo {
    ModelInfo {
        id: ModelId::new(id),
        display_name: id.to_owned(),
        description: None,
        is_default: default,
        hidden: false,
        capabilities: vec![ModelCapability { capability: Capability::ImageInput, support: images }],
        efforts: vec![effort("low"), effort("medium"), effort("high")],
        default_effort: Some(EffortId::new("high")),
        context_window: Some(200_000),
    }
}

/// The model, offering fast mode (D125). One demo model does.
fn with_fast(mut m: ModelInfo) -> ModelInfo {
    m.capabilities.push(ModelCapability { capability: Capability::FastMode, support: Support::Supported });
    m
}

/// Whether a demo model offers fast mode.
pub(super) fn has_fast(m: &ModelInfo) -> bool {
    crate::controls::model_has_fast(m)
}

fn modes() -> Vec<ModeInfo> {
    vec![
        ModeInfo { id: ModeId::new("ask"), display_name: "ask".into(), description: None, is_default: true, skips_approvals: false },
        ModeInfo { id: ModeId::new("auto"), display_name: "auto".into(), description: None, is_default: false, skips_approvals: false },
        ModeInfo { id: ModeId::new("bypass"), display_name: "bypass permissions".into(), description: None, is_default: false, skips_approvals: true },
    ]
}

fn caps() -> Vec<Capability> {
    vec![
        Capability::ThreadList,
        Capability::ThreadRead,
        Capability::ThreadFork,
        Capability::ThreadRename,
        Capability::ThreadArchive,
        Capability::ThreadDelete,
        Capability::ThreadTakeOver,
        Capability::StreamText,
        Capability::StreamReasoning,
        Capability::Interrupt,
        Capability::Approvals,
        Capability::UserQuestions,
        Capability::Modes,
        Capability::ModelSwitch,
        Capability::EffortSwitch,
        Capability::ContextUsage,
        Capability::ModelList,
        Capability::MeshTools,
        // yaacd's own (D144): any thread can be moved to another project.
        Capability::ThreadMove,
    ]
}

pub(crate) fn codex(instance: &str, name: Option<&str>, status: ProviderStatus) -> ProviderDescriptor {
    let display = match name {
        Some(n) => format!("Codex · {n}"),
        None => "Codex".to_owned(),
    };
    ProviderDescriptor {
        instance: InstanceId::new(instance),
        kind: ProviderKind::new("codex"),
        name: name.map(str::to_owned),
        display_name: display,
        backend: BackendKind::Daemon,
        harness_version: Some("0.171".into()),
        store: StoreIdentity { key: Some(format!("store-{instance}")), home: None, account: None },
        status,
        status_message: None,
        // The demo's Codex can change a conversation's folder (D144); its Claude cannot.
        capabilities: [caps(), vec![Capability::ThreadMoveWorkingDirectory]].concat(),
        models: vec![
            model("gpt-6.2-sol", true, Support::Supported),
            model("gpt-6.2-astra", false, Support::Supported),
            model("gpt-6.1-terra", false, Support::Unknown),
        ],
        models_complete: true,
        modes: modes(),
        revision: 1,
        backend_origin: None,
        models_read_at: None,
    }
}

pub(crate) fn claude(status: ProviderStatus) -> ProviderDescriptor {
    ProviderDescriptor {
        instance: InstanceId::new("claude"),
        kind: ProviderKind::new("claude"),
        name: None,
        display_name: "Claude".into(),
        backend: BackendKind::Sdk,
        harness_version: Some("2.1.329".into()),
        store: StoreIdentity { key: Some("store-claude".into()), home: None, account: Some("work@example.com".into()) },
        status,
        status_message: None,
        capabilities: caps(),
        models: vec![with_fast(model("opus-5.5", true, Support::Supported)), model("haiku-5", false, Support::Unsupported)],
        models_complete: true,
        modes: modes(),
        revision: 1,
        backend_origin: None,
        models_read_at: None,
    }
}

fn target(id: &str, name: &str, transport: TargetTransport, status: ConnectionStatus, detail: Option<&str>, enabled: bool) -> TargetEntry {
    TargetEntry {
        id: TargetId::new(id),
        name: TargetName::new(name),
        transport,
        enabled,
        state: ConnectionState { status, detail: detail.map(str::to_owned), daemon: None, update: None },
    }
}

fn ssh(host: &str) -> TargetTransport {
    TargetTransport::Ssh(SshTransport { host: host.to_owned(), port: None, user: None })
}

impl Fleet {
    pub(super) fn ts(&self, offset: i64) -> Timestamp {
        timestamp(self.base + offset)
    }

    pub(super) fn summary(&self, key: &ThreadKey) -> &ThreadSummary {
        &self.threads[key].summary
    }

    pub(super) fn target_name(&self, id: &TargetId) -> String {
        self.targets.iter().find(|t| &t.id == id).map_or_else(|| id.to_string(), |t| t.name.to_string())
    }

    /// "title · provider · target" for a thread.
    pub(super) fn peer(&self, key: &ThreadKey) -> MeshPeer {
        let s = self.summary(key);
        let provider = self
            .providers
            .get(&key.target)
            .and_then(|ps| ps.iter().find(|p| p.instance == s.instance))
            .map(|p| p.display_name.clone());
        let reachable = self.targets.iter().find(|t| t.id == key.target).map(|t| t.state.status == ConnectionStatus::Connected);
        MeshPeer {
            address: key.address(),
            title: Some(s.title.clone()),
            provider,
            model: s.model.clone(),
            target_name: Some(self.target_name(&key.target)),
            reachable,
        }
    }

    fn effective_for(&self, key: &ThreadKey, instance: &str, model_id: &str) -> ThreadEffective {
        let p = self.providers.get(&key.target).and_then(|ps| ps.iter().find(|p| p.instance.as_str() == instance));
        let m = p.and_then(|p| p.models.iter().find(|m| m.id.as_str() == model_id));
        let mut capabilities = p.map(|p| p.capabilities.clone()).unwrap_or_default();
        if m.is_some_and(has_fast) {
            capabilities.push(Capability::FastMode);
        }
        ThreadEffective {
            capabilities,
            image_input: m.map_or(Support::Unknown, crate::images::model_image_support),
            image_formats: vec!["image/png".into(), "image/jpeg".into()],
            efforts: m.map(|m| m.efforts.clone()).unwrap_or_default(),
            modes: p.map(|p| p.modes.clone()).unwrap_or_default(),
            applied: AppliedSettings {
                model: Some(ModelId::new(model_id)),
                effort: Some(EffortId::new("high")),
                mode: Some(ModeId::new("ask")),
            },
        }
    }

    /// Add a thread with a summary and items (journaled as completed items).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn add(
        &mut self,
        key: &ThreadKey,
        title: &str,
        instance: &str,
        model_id: &str,
        project: (&str, &str),
        state: ThreadState,
        updated: i64,
    ) {
        let (proj, branch) = project;
        let cwd = format!("/home/dev/{proj}");
        let summary = ThreadSummary {
            id: key.thread.clone(),
            instance: InstanceId::new(instance),
            title: title.to_owned(),
            preview: String::new(),
            cwd: TargetPath::new(cwd.clone()),
            project: Some(ProjectRef {
                path: TargetPath::new(format!("{cwd}/.git")),
                display_name: proj.to_owned(),
                cross_target_key: Some(format!("example.com/{proj}")),
                kind: None,
                id: None,
                repo: None,
            }),
            worktree: Some(WorktreeRef { path: TargetPath::new(cwd), branch: Some(branch.to_owned()) }),
            branch: Some(branch.to_owned()),
            model: Some(ModelId::new(model_id)),
            state,
            control: Control::Free,
            control_holder: None,
            forked_from: None,
            spawned_by: None,
            root: None,
            children: 0,
            peers: 0,
            origin_kind: OriginKind::Yaac,
            created_at: self.ts(updated - 600),
            updated_at: self.ts(updated),
            archived: false,
            repo: None,
            git: None,
            agents: yaac_protocol::AgentsPolicy::default(),
            native_id: None,
            hold: None,
            fast: false,
            placement: None,
        };
        let effective = self.effective_for(key, instance, model_id);
        self.threads.insert(
            key.clone(),
            DemoThread {
                summary,
                items: Vec::new(),
                journal: Vec::new(),
                epoch: Epoch(1),
                control: Control::Free,
                holder: None,
                take_over_blocked: None,
                active_turn: None,
                effective,
                context: Some(ContextUsage { used: Some(76_000), window: Some(200_000) }),
                missing: Vec::new(),
                subscribed: false,
            },
        );
    }

    /// Journal an event and return it (the hub decides whether to forward it).
    pub(super) fn journal(&mut self, key: &ThreadKey, event: ThreadEvent) -> ThreadEventParams {
        let t = self.threads.get_mut(key).expect("demo thread");
        match &event {
            ThreadEvent::ItemStarted(e) | ThreadEvent::ItemCompleted(e) => upsert(&mut t.items, e.item.clone()),
            ThreadEvent::ItemUpdated(e) => upsert(&mut t.items, e.item.clone()),
            ThreadEvent::ItemDelta(e) => {
                if let Some(i) = t.items.iter_mut().find(|i| i.id == e.item_id) {
                    let mut tr = crate::transcript::Transcript::new();
                    tr.upsert(i.clone(), 0);
                    tr.apply(
                        &ThreadEventParams { thread_id: key.thread.clone(), epoch: t.epoch, seq: Seq(1), event: event.clone() },
                        0,
                    );
                    *i = tr.items()[0].clone();
                }
            }
            ThreadEvent::RequestChanged(e) => {
                if let Some(i) = t.items.iter_mut().find(|i| i.id == e.item_id)
                    && let yaac_protocol::ItemBody::Request(r) = &mut i.body
                {
                    r.state = e.state.clone();
                    r.resolution = e.resolution.clone();
                }
            }
            ThreadEvent::TurnStarted(e) => t.active_turn = Some(e.turn_id.clone()),
            ThreadEvent::TurnCompleted(_) => t.active_turn = None,
            ThreadEvent::ControlChanged(e) => {
                t.control = e.control.clone();
                t.holder = e.holder.clone();
                t.take_over_blocked = e.take_over_blocked.clone();
            }
            ThreadEvent::SettingsChanged(e) => t.effective = e.effective.clone(),
            ThreadEvent::UsageUpdated(e) => {
                if e.context.is_some() {
                    t.context = e.context.clone();
                }
            }
            ThreadEvent::QueueChanged(_) | ThreadEvent::HoldChanged(_) | ThreadEvent::Unknown(_) => {}
        }
        let params = ThreadEventParams { thread_id: key.thread.clone(), epoch: t.epoch, seq: Seq(t.journal.len() as u64 + 1), event };
        t.journal.push(params.clone());
        params
    }

    /// Journal a completed item.
    pub(super) fn push(&mut self, key: &ThreadKey, item: Item) -> ThreadEventParams {
        let event = if item.status == ItemStatus::InProgress {
            ThreadEvent::ItemStarted(ItemEvent { item })
        } else {
            ThreadEvent::ItemCompleted(ItemEvent { item })
        };
        self.journal(key, event)
    }

    fn set(&mut self, key: &ThreadKey, f: impl FnOnce(&mut ThreadSummary)) {
        if let Some(t) = self.threads.get_mut(key) {
            f(&mut t.summary);
        }
    }

    /// The standard fleet.
    pub(super) fn standard(base: i64) -> Self {
        let mut f = Fleet {
            base,
            targets: vec![
                target("local", "local", TargetTransport::Local(LocalTransport {}), ConnectionStatus::Connected, None, true),
                target("devbox", "devbox", ssh("devbox.example.com"), ConnectionStatus::Connected, None, true),
                target("gpu-box", "gpu-box", ssh("gpu-box.example.com"), ConnectionStatus::Connected, None, true),
                target("staging", "staging", ssh("staging.example.com"), ConnectionStatus::Offline, Some("last seen 3h ago"), true),
                target("legacy", "legacy", TargetTransport::Ssh(SshTransport { host: "legacy.example.com".into(), port: None, user: Some("deploy".into()) }), ConnectionStatus::Disabled, None, false),
                target(
                    "wsl-ubuntu",
                    "wsl: Ubuntu",
                    TargetTransport::Wsl(WslTransport { distro: "Ubuntu".into(), user: None }),
                    ConnectionStatus::Disabled,
                    None,
                    false,
                ),
            ],
            providers: BTreeMap::new(),
            threads: BTreeMap::new(),
            budgets: BTreeMap::new(),
            projects: BTreeMap::new(),
            view: BTreeMap::new(),
        };
        f.providers.insert(TargetId::new("local"), vec![codex("codex", None, ProviderStatus::Ready), claude(ProviderStatus::Ready)]);
        f.providers.insert(
            TargetId::new("devbox"),
            vec![
                codex("codex", None, ProviderStatus::Ready),
                codex("codex-work", Some("work"), ProviderStatus::Ready),
                claude(ProviderStatus::Ready),
            ],
        );
        f.providers.insert(TargetId::new("gpu-box"), vec![claude(ProviderStatus::Ready), codex("codex", None, ProviderStatus::Ready)]);
        f.providers.insert(TargetId::new("staging"), vec![codex("codex", None, ProviderStatus::Ready)]);

        let (orch, users, billing, search, index, ci, notes, fix, hs) =
            (k(ORCH), k(USERS), k(BILLING), k(SEARCH), k(INDEX), k(CI), k(NOTES), k(FIX), k(HANDSHAKE));
        let root = orch.address();
        f.add(&orch, "Migrate auth module · orchestrator", "codex", "gpt-6.2-sol", ("api", "main"), ThreadState::Running, -30);
        f.add(&users, "users-service", "claude", "opus-5.5", ("api", "main"), ThreadState::Idle, -12 * 60);
        f.add(&billing, "billing-service", "claude", "opus-5.5", ("api", "main"), ThreadState::Idle, -9 * 60);
        f.add(&search, "search-service", "claude", "opus-5.5", ("search", "main"), ThreadState::Running, -20);
        // A worker running in fast mode (D125): its spinner is yellow.
        f.threads.get_mut(&search).expect("search").summary.fast = true;
        f.add(&index, "index-rebuild", "claude", "opus-5.5", ("search", "main"), ThreadState::NeedsAttention, -41 * 60);
        f.add(&ci, "CI integration", "codex", "gpt-6.2-sol", ("api", "main"), ThreadState::Running, -60);
        f.add(&notes, "Release notes", "codex", "gpt-6.2-sol", ("docs", "main"), ThreadState::Idle, -3 * 3600 - 900);
        f.add(&fix, "Fix frame decoder", "codex-work", "gpt-6.2-sol", ("yaac", "feature/protocol"), ThreadState::NeedsAttention, -2 * 60);
        f.add(&hs, "Explain the handshake", "codex", "gpt-6.2-sol", ("yaac", "feature/protocol"), ThreadState::Idle, -2 * 3600 - 120);

        f.set(&orch, |s| {
            s.children = 3;
            s.peers = 2;
            s.origin_kind = OriginKind::Yaac;
            s.preview = "Waiting for search-service".into();
        });
        for (w, preview) in [(&users, "merged, 14 tests pass"), (&billing, "merged, 14 tests pass"), (&search, "rebuilding the index")] {
            let addr = orch.address();
            f.set(w, |s| {
                s.spawned_by = Some(addr.clone());
                s.root = Some(addr);
                s.preview = preview.into();
            });
        }
        f.set(&search, |s| s.children = 1);
        let saddr = search.address();
        f.set(&index, |s| {
            s.spawned_by = Some(saddr);
            s.root = Some(root.clone());
            s.preview = "approval: ./scripts/reindex.sh --drop-old".into();
        });
        f.set(&ci, |s| {
            s.root = Some(root.clone());
            s.peers = 1;
            s.preview = "waiting for billing to merge".into();
        });
        f.set(&notes, |s| {
            s.root = Some(root.clone());
            s.peers = 1;
            s.preview = "no messages yet".into();
        });
        f.set(&fix, |s| s.preview = "approval: cargo fix --allow-dirty".into());
        f.set(&hs, |s| {
            s.control = Control::External;
            s.control_holder = Some("a terminal session".into());
            s.origin_kind = OriginKind::Interactive;
            s.preview = "The handshake has four main steps".into();
        });
        if let Some(t) = f.threads.get_mut(&hs) {
            t.control = Control::External;
            t.holder = Some("a terminal session".into());
        }
        f.budgets.insert(
            root.clone(),
            MeshBudgetResult {
                root: root.clone(),
                allowance: MeshBudget { agent_turns: 100, threads_spawned: 20, wall_clock_secs: 12 * 3600 },
                used: MeshBudgetUsage { agent_turns: 14, threads_spawned: 4, wall_clock_secs: 3600 },
                exhausted: false,
            },
        );

        // ── transcripts ────────────────────────────────────────────────────────────────
        let (p_orch, p_users, p_billing, p_search, p_index, p_ci, p_notes) =
            (f.peer(&orch), f.peer(&users), f.peer(&billing), f.peer(&search), f.peer(&index), f.peer(&ci), f.peer(&notes));

        // Orchestrator (02).
        f.push(&orch, done("o1", "o-t1", user(
            "Split this migration across three Opus workers, one per service. Tell the Codex thread on devbox when billing is merged, and send release notes to staging.",
            &[], &[],
        )));
        f.journal(&orch, ThreadEvent::TurnStarted(yaac_protocol::TurnStartedEvent { turn_id: TurnId::new("o-t1"), origin: yaac_protocol::TurnOrigin::User, at: None }));
        f.push(&orch, done("o1b", "o-t1", agent("I'll run one worker per service and keep CI and the release notes thread informed.")));
        f.push(&orch, item("o2", "o-t1", ItemStatus::InProgress, spawn(vec![
            (p_users, ThreadState::Idle),
            (p_billing.clone(), ThreadState::Idle),
            (p_search.clone(), ThreadState::Running),
        ])));
        f.push(&orch, done("o4", "o-t1", received(
            message("m-bill-result", &billing.address(), &orch.address(), MeshMessageKind::Result,
                "Migration merged. 14 tests pass. One flaky test skipped: `test_refund_race`.", f.ts(-1300)),
            p_billing,
        )));
        f.push(&orch, done("o5", "o-t1", sent(
            message("m-to-ci", &orch.address(), &ci.address(), MeshMessageKind::Message,
                "Billing is merged. You can resume the integration tests.", f.ts(-60)),
            p_ci,
            delivered(None),
        )));
        f.push(&orch, done("o6", "o-t1", sent(
            message("m-to-notes", &orch.address(), &notes.address(), MeshMessageKind::Message,
                "Draft the notes for the auth migration.", f.ts(-3 * 3600 - 840)),
            p_notes,
            queued(ReceiptReason::TargetOffline),
        )));
        f.push(&orch, item("o5b", "o-t1", ItemStatus::InProgress, sent(
            message("m-to-search", &orch.address(), &search.address(), MeshMessageKind::Message,
                "How far along is the index rebuild?", f.ts(-5)),
            p_search.clone(),
            delivered(None),
        )));
        f.push(&orch, item("o3", "o-t1", ItemStatus::InProgress, wait(vec![(p_search.clone(), ThreadState::Running)], false)));
        f.push(&orch, item("o6b", "o-t1", ItemStatus::Completed, agent(
            "users-service and billing-service are merged. One flaky test was skipped in billing: `test_refund_race`. I'm waiting on search-service.",
        )));
        if let Some(t) = f.threads.get_mut(&orch) {
            t.context = Some(ContextUsage { used: Some(76_000), window: Some(200_000) });
        }

        // users-service.
        let task = |svc: &str| format!("Migrate the {svc} service to the new auth module. Report back when it is merged.");
        f.push(&users, done("u1", "u-t1", received(
            message("m-users-task", &orch.address(), &users.address(), MeshMessageKind::Message, &task("users"), f.ts(-3000)),
            p_orch.clone(),
        )));
        f.push(&users, done("u2", "u-t1", reasoning("Plan the users-service migration.")));
        f.push(&users, done("u3", "u-t1", tool(ToolKind::FileChange, "Edited 3 files", None, super::diffs::USERS)));
        f.push(&users, done("u4", "u-t1", tool(ToolKind::Command, "Ran `cargo test -p users` · 14 passed", Some(0), "test result: ok. 14 passed")));
        f.push(&users, done("u5", "u-t1", agent("Merged to main. All 14 tests pass.")));
        f.push(&users, done("u6", "u-t1", sent(
            message("m-users-result", &users.address(), &orch.address(), MeshMessageKind::Result, "Migration merged. 14 tests pass.", f.ts(-2400)),
            p_orch.clone(),
            delivered(None),
        )));

        // billing-service (03).
        f.push(&billing, done("b1", "b-t1", received(
            message("m-bill-task", &orch.address(), &billing.address(), MeshMessageKind::Message, &task("billing"), f.ts(-3000)),
            p_orch.clone(),
        )));
        f.push(&billing, done("b2", "b-t1", reasoning("Find every call into the old auth module in the billing service, then swap them for the new client.")));
        f.push(&billing, done("b3", "b-t1", tool(ToolKind::FileChange, "Edited 4 files", None, super::diffs::BILLING)));
        f.push(&billing, done("b4", "b-t1", tool(ToolKind::Command, "Ran `cargo test -p billing` · 14 passed", Some(0), "test result: ok. 14 passed; 1 ignored")));
        f.push(&billing, done("b5", "b-t1", agent("Merged to main. One flaky test was skipped.")));
        f.push(&billing, done("b6", "b-t1", sent(
            message("m-bill-result", &billing.address(), &orch.address(), MeshMessageKind::Result,
                "Migration merged. 14 tests pass. One flaky test skipped: `test_refund_race`.", f.ts(-1300)),
            p_orch.clone(),
            delivered(None),
        )));

        // search-service.
        f.push(&search, done("s1", "s-t1", received(
            message("m-search-task", &orch.address(), &search.address(), MeshMessageKind::Message, &task("search"), f.ts(-3000)),
            p_orch.clone(),
        )));
        f.push(&search, done("s2", "s-t1", tool(ToolKind::FileChange, "Edited 2 files", None, super::diffs::SEARCH)));
        f.push(&search, done("s3", "s-t1", agent("The index must be rebuilt against the new auth tables. Starting a worker for it.")));
        f.push(&search, item("s4", "s-t1", ItemStatus::InProgress, spawn(vec![(p_index.clone(), ThreadState::NeedsAttention)])));
        f.push(&search, item("s5", "s-t1", ItemStatus::InProgress, wait(vec![(p_index, ThreadState::NeedsAttention)], false)));
        f.journal(&search, ThreadEvent::TurnStarted(yaac_protocol::TurnStartedEvent { turn_id: TurnId::new("s-t1"), origin: yaac_protocol::TurnOrigin::Mesh, at: None }));

        // index-rebuild (04).
        f.push(&index, done("i1", "i-t1", received(
            message("m-index-task", &search.address(), &index.address(), MeshMessageKind::Message,
                "Rebuild the search index against the new auth tables.", f.ts(-2700)),
            p_search,
        )));
        f.push(&index, done("i2", "i-t1", tool(ToolKind::Command, "Ran `./scripts/reindex.sh --dry-run` · 3 shards would be dropped", Some(0), "would drop: shard-03, shard-07, shard-11")));
        f.push(&index, item("i3", "i-t1", ItemStatus::InProgress, approval("r-index", "./scripts/reindex.sh --drop-old", "/srv/search", vec![
            decision("allow", "Allow once", DecisionKind::Allow),
            decision("deny", "Deny", DecisionKind::Deny),
        ])));
        f.journal(&index, ThreadEvent::TurnStarted(yaac_protocol::TurnStartedEvent { turn_id: TurnId::new("i-t1"), origin: yaac_protocol::TurnOrigin::Mesh, at: None }));
        if let Some(t) = f.threads.get_mut(&index) {
            t.context = Some(ContextUsage { used: Some(30_000), window: Some(200_000) });
        }

        // CI integration: a contact waiting for messages.
        f.push(&ci, done("c1", "c-t1", received(
            message("m-ci-hello", &orch.address(), &ci.address(), MeshMessageKind::Message,
                "Hold the integration tests until billing is merged; I will tell you.", f.ts(-2000)),
            p_orch,
        )));
        f.push(&ci, done("c2", "c-t1", agent("Understood. Waiting for word that billing is merged.")));
        f.push(&ci, item("c3", "c-t1", ItemStatus::InProgress, wait(vec![], true)));
        f.journal(&ci, ThreadEvent::TurnStarted(yaac_protocol::TurnStartedEvent { turn_id: TurnId::new("c-t1"), origin: yaac_protocol::TurnOrigin::Mesh, at: None }));

        // Release notes: started by a person; the orchestrator's message is still queued.
        f.push(&notes, done("n0", "n-t0", user("Keep the release notes current.", &[], &[])));
        f.push(&notes, done("n0b", "n-t0", agent("Will do. Send me what changes and I will draft the notes.")));

        // Four muted threads (the Muted section).
        for (key, title, proj, upd) in [
            (k(OLD_SPIKE), "Old spike", ("spike", "main"), -6 * 86_400),
            (k(LINT), "Lint pass", ("api", "main"), -2 * 86_400),
            (k(BENCH), "Benchmark the codec", ("search", "main"), -3 * 86_400),
            (k(V2PLAN), "Draft the v2 plan", ("api", "main"), -4 * 86_400),
        ] {
            f.add(&key, title, if key.target.as_str() == "gpu-box" { "claude" } else { "codex" }, if key.target.as_str() == "gpu-box" { "opus-5.5" } else { "gpt-6.2-sol" }, proj, ThreadState::Idle, upd);
            f.push(&key, done(&format!("{}-1", key.thread), "m-t1", user(title, &[], &[])));
        }
        // A thread in a folder that is not a project yet, and not in git.
        let parser = k(PARSER);
        f.add(&parser, "Try the new parser", "codex", "gpt-6.2-sol", ("scratch", "main"), ThreadState::Idle, -86_400);
        f.set(&parser, |s| {
            s.cwd = TargetPath::new("~/scratch/spike-7");
            s.project = None;
            s.worktree = None;
            s.branch = None;
            s.origin_kind = OriginKind::Interactive;
        });
        f.push(&parser, done("parser-1", "p-t1", user("Try the new parser on the sample files.", &[], &[])));
        let lint = k(LINT);
        f.set(&lint, |s| s.root = Some(root.clone()));

        // Fix frame decoder (01).
        f.push(&fix, done("f1", "f-t1", user("Why does decode() drop the last frame?", &["u1/decoder-log.txt"], &[])));
        f.push(&fix, done("f2", "f-t1", reasoning("The reader computes the payload end from the length prefix; check whether the prefix includes the kind byte.")));
        f.push(&fix, done("f3", "f-t1", tool(ToolKind::Command, "Ran `cargo test -p yaac-protocol` · passed", Some(0), "test result: ok. 73 passed")));
        f.push(&fix, done("f4", "f-t1", agent(
            "The length prefix counts the kind byte, so the reader stops one byte early.\n\n```rust\nlet kind = buf[0];\nlet len = u32::from_be_bytes(buf[1..5].try_into()?);\nlet total = 1 + len as usize; // include the kind byte\nif buf.len() < total {\n    return Err(Error::Incomplete);\n}\n```",
        )));
        f.push(&fix, item("f5", "f-t1", ItemStatus::InProgress, approval("r-fix", "cargo fix --allow-dirty", "/home/dev/yaac", vec![
            decision("allow", "Allow once", DecisionKind::Allow),
            decision("allow-thread", "Allow for this thread", DecisionKind::AllowSession),
            decision("deny", "Deny", DecisionKind::Deny),
        ])));
        f.journal(&fix, ThreadEvent::TurnStarted(yaac_protocol::TurnStartedEvent { turn_id: TurnId::new("f-t1"), origin: yaac_protocol::TurnOrigin::User, at: None }));
        // "Thought for 4s".
        f.time_item(&fix, "f2", -184, -180);

        // Explain the handshake (10): held by a terminal session.
        f.push(&hs, done("h1", "h-t1", user("Can you explain the handshake between the client and server, step by step? Please include the key messages and what each one does.", &[], &[])));
        f.push(&hs, done("h2", "h-t1", agent(
            "The handshake has four main steps:\n\n1. **ClientHello** · the client proposes protocol versions and cipher suites.\n2. **ServerHello** · the server selects the version and cipher suite.\n3. **Key exchange** · both sides establish a shared session key.\n4. **Finished** · each side verifies the handshake and transitions to an encrypted session.\n\nHere is a more detailed breakdown with the exact messages and fields …",
        )));
        f.push(&hs, item("h3", "h-t2", ItemStatus::InProgress, approval("r-hs", "openssl s_client -connect localhost:8443", "/home/dev/yaac", vec![
            decision("allow", "Allow once", DecisionKind::Allow),
            decision("deny", "Deny", DecisionKind::Deny),
        ])));
        f.push(&hs, item("h4", "h-t2", ItemStatus::InProgress, agent("I'll pause here until the approval is handled in the other program.")));

        // What the targets report beyond the brief's table: muted and pinned threads, git
        // status, and the projects the user created (the targets resolve which one each
        // thread belongs to).
        // Pin and mute are the hub's view preferences, not thread fields (D92). Lint pass is
        // muted until later today (a timed mute).
        for key in [k(OLD_SPIKE), k(LINT), k(BENCH), k(V2PLAN)] {
            f.view.entry(key).or_default().muted = true;
        }
        f.view.entry(k(LINT)).or_default().muted_until = Some(f.base + 2 * 3600 + 30 * 60);
        f.view.entry(k(ORCH)).or_default().pinned = true;
        // Archived threads: listed only in the Archived section (D93).
        for (n, (id, title)) in [("t-arch-1", "Spike: protobuf codec"), ("t-arch-2", "Old CI flake hunt"), ("t-arch-3", "Rename the crates")].into_iter().enumerate() {
            let key = ThreadKey::new(TargetId::new("local"), yaac_protocol::ThreadId::new(id));
            let ago = -(i64::try_from(n).unwrap_or(0) + 8) * 86_400;
            f.add(&key, title, "codex", "gpt-6.2-sol", ("api", "main"), ThreadState::Idle, ago);
            f.push(&key, done(&format!("{id}-1"), "a-t1", user(title, &[], &[])));
            f.set(&key, |s| s.archived = true);
        }
        // The targets report a finished worker as done and a thread blocked on another agent as
        // waiting.
        for key in [k(USERS), k(BILLING), k(NOTES), k(LINT)] {
            f.set(&key, |s| s.state = ThreadState::Done);
        }
        f.set(&k(CI), |s| s.state = ThreadState::Waiting);
        f.time_item(&k(BILLING), "b2", -9 * 60 - 40, -9 * 60 - 31);
        f.time_item(&k(USERS), "u2", -12 * 60 - 30, -12 * 60 - 25);
        for (key, branch, [added, modified, deleted, ahead, behind]) in [
            (k(ORCH), "main", [0, 4, 0, 0, 0]),
            (k(USERS), "auth/users", [3, 8, 1, 3, 0]),
            (k(FIX), "feature/protocol", [1, 6, 0, 2, 0]),
            (k(HANDSHAKE), "feature/protocol", [0, 0, 0, 0, 0]),
            (k(BILLING), "auth/billing", [2, 6, 1, 1, 0]),
            (k(CI), "main", [0, 0, 0, 0, 2]),
            (k(SEARCH), "auth/search", [0, 3, 0, 0, 0]),
            (k(INDEX), "auth/search", [0, 3, 0, 0, 0]),
            (k(NOTES), "main", [0, 0, 0, 0, 0]),
            (k(LINT), "main", [0, 0, 0, 0, 0]),
            (k(BENCH), "main", [0, 1, 0, 0, 0]),
            (k(V2PLAN), "main", [0, 0, 0, 0, 0]),
            (k(OLD_SPIKE), "main", [0, 0, 0, 0, 0]),
        ] {
            f.set(&key, |s| {
                s.git = Some(GitStatus { branch: Some(branch.into()), base: None, added, modified, deleted, ahead, behind, removed: false, at: None });
                s.branch = Some(branch.into());
            });
        }
        for (target, name, folder) in [
            ("local", "api", "/home/dev/api"),
            ("local", "spike", "/home/dev/spike"),
            ("devbox", "yaac", "/home/dev/yaac"),
            ("devbox", "api", "/home/dev/api"),
            ("gpu-box", "search", "/home/dev/search"),
            ("staging", "docs", "/home/dev/docs"),
        ] {
            f.create_project(&TargetId::new(target), name, folder);
        }
        f
    }

    /// Give an item start and end times (seconds from now), for "Thought for 4s".
    pub(super) fn time_item(&mut self, key: &ThreadKey, id: &str, start: i64, end: i64) {
        let (s, e) = (self.ts(start), self.ts(end));
        if let Some(t) = self.threads.get_mut(key)
            && let Some(i) = t.items.iter_mut().find(|i| i.id.as_str() == id)
        {
            i.started_at = Some(s);
            i.completed_at = Some(e);
        }
    }

    /// Add a project on a target, and resolve every thread there again.
    pub(super) fn create_project(&mut self, target: &TargetId, name: &str, folder: &str) -> Project {
        let created_at = self.ts(0);
        let list = self.projects.entry(target.clone()).or_default();
        let p = Project {
            id: ProjectId::new(format!("p{}", list.len() + 1)),
            name: name.to_owned(),
            folder: TargetPath::new(folder.trim_end_matches('/')),
            repo: None,
            cross_target_key: None,
            created_at,
        };
        list.push(p.clone());
        self.resolve_projects(target);
        p
    }

    /// The target's matching rule: the deepest project folder containing the thread's folder;
    /// otherwise a temporary project named after the folder.
    pub(super) fn resolve_projects(&mut self, target: &TargetId) {
        let projects: Vec<crate::projects::Project> =
            self.projects.get(target).cloned().unwrap_or_default().into_iter().map(|p| crate::projects::Project::from_wire(target, p)).collect();
        // A thread the user moved (D144) stays where it was put.
        for (key, t) in self.threads.iter_mut().filter(|(k, t)| &k.target == target && t.summary.placement.is_none()) {
            let loc = crate::projects::Location { target: key.target.clone(), folder: t.summary.cwd.to_string(), repo: None };
            let found = crate::projects::project_for(&projects, &loc);
            let cross = t.summary.project.as_ref().and_then(|p| p.cross_target_key.clone());
            t.summary.project = Some(match found {
                Some(p) => ProjectRef {
                    path: TargetPath::new(p.folder.clone()),
                    display_name: p.name.clone(),
                    cross_target_key: cross,
                    kind: Some(ProjectKind::User),
                    id: Some(ProjectId::new(p.wire_id.clone())),
                    repo: None,
                },
                None => ProjectRef {
                    path: t.summary.cwd.clone(),
                    display_name: t.summary.cwd.to_string(),
                    cross_target_key: None,
                    kind: Some(ProjectKind::Temporary),
                    id: None,
                    repo: None,
                },
            });
        }
    }

    /// A fresh install: only this computer, no threads yet (screens 07–09).
    pub(super) fn fresh(base: i64) -> Self {
        let mut f = Fleet {
            base,
            targets: vec![target("local", "local", TargetTransport::Local(LocalTransport {}), ConnectionStatus::Connected, None, true)],
            providers: BTreeMap::new(),
            threads: BTreeMap::new(),
            budgets: BTreeMap::new(),
            projects: BTreeMap::new(),
            view: BTreeMap::new(),
        };
        let mut c = claude(ProviderStatus::NeedsAuth);
        c.status_message = Some(SIGN_IN_HINT.into());
        f.providers.insert(TargetId::new("local"), vec![codex("codex", None, ProviderStatus::Ready), c]);
        f
    }

    /// Start a fresh journal for a thread (the items are kept, re-journaled from seq 1).
    pub(super) fn new_epoch(&mut self, key: &ThreadKey) {
        let Some(t) = self.threads.get_mut(key) else { return };
        t.epoch = Epoch(t.epoch.0 + 1);
        let epoch = t.epoch;
        t.journal = t
            .items
            .iter()
            .enumerate()
            .map(|(i, item)| ThreadEventParams {
                thread_id: key.thread.clone(),
                epoch,
                seq: Seq(i as u64 + 1),
                event: ThreadEvent::ItemCompleted(ItemEvent { item: item.clone() }),
            })
            .collect();
    }

    pub(super) fn new_thread_id(&self, target: &TargetId) -> ThreadId {
        let n = self.threads.keys().filter(|k| &k.target == target).count() + 1;
        ThreadId::new(format!("t-new-{n}"))
    }
}

fn upsert(items: &mut Vec<Item>, item: Item) {
    match items.iter_mut().find(|i| i.id == item.id) {
        Some(slot) => *slot = item,
        None => items.push(item),
    }
}
