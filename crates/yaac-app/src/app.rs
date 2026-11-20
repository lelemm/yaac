//! The window: shell layout, input, and the loop that applies hub messages.
//!
//! Rendering reads the state and records [`Action`]s; they are applied after the frame's UI
//! is built, so drawing never mutates what it is drawing from. The UI thread never blocks:
//! the hub link is polled (non-blocking), and a repaint is requested only when something
//! arrived, the demo is streaming, or an arrow is fading.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use eframe::egui::{self, Key, Modifiers};
use yaac_core::activity::Expansion;
use yaac_core::agents::{PanelInput, PanelMode};
use yaac_core::attention::AttentionMap;
use yaac_core::client::Client;
use yaac_core::demo::{DEMO_CLIENT, DemoHub, Scenario, Scene};
use yaac_core::permission::BudgetFields;
use yaac_core::sidebar::Grouping;
use yaac_core::state::AgentsPolicy;
use yaac_core::ThreadKey;
use yaac_protocol::{Address, ClientId, InstanceId, ItemId, QuestionId, RequestId, RequestResponse, TargetId};

use crate::markdown;
use crate::screenshot::Shots;
use crate::theme::{self, Palette, ThemeChoice};

/// Which menu is open (one at a time).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Menu {
    Grouping,
    Context(ThreadKey),
    TempProject(String),
    Project,
    Model,
    Effort,
    Mode,
    Agents,
    Fork,
    Demo,
}

/// The user's in-progress answers to a question card. UI memory only: never written to the
/// state or to disk, so a secret is never cached.
#[derive(Clone, Debug, Default)]
pub(crate) struct QuestionDraft {
    pub selected: BTreeMap<QuestionId, BTreeSet<String>>,
    pub other: BTreeMap<QuestionId, String>,
}

/// View state that is not application state.
#[derive(Debug)]
pub(crate) struct UiState {
    pub grouping: Grouping,
    pub search: String,
    pub muted_open: bool,
    pub expansion: Expansion,
    pub panel: PanelMode,
    pub menu: Option<Menu>,
    pub needs_popup: bool,
    pub needs_sel: usize,
    /// Focus this request card's first button on the next frame.
    pub focus_card: Option<ItemId>,
    /// Scroll the transcript to this item on the next frame.
    pub scroll_to: Option<ItemId>,
    /// Scroll the transcript to the end on the next frame.
    pub scroll_end: bool,
    /// An input method is composing in the composer.
    pub ime_composing: bool,
    pub settings: bool,
    pub theme: ThemeChoice,
    pub questions: HashMap<RequestId, QuestionDraft>,
    pub budgets: HashMap<RequestId, BudgetFields>,
    pub dismissed: BTreeSet<TargetId>,
    pub focus_search: bool,
    pub focus_composer: bool,
    pub rename: Option<(ThreadKey, String)>,
    pub create_project: Option<CreateProject>,
    /// Item heights, keyed by content hash and width.
    pub heights: HashMap<u64, f32>,
}

impl UiState {
    fn new(theme: ThemeChoice) -> Self {
        Self {
            grouping: Grouping::default(),
            search: String::new(),
            muted_open: false,
            expansion: Expansion::default(),
            panel: PanelMode::Pill,
            menu: None,
            needs_popup: false,
            needs_sel: 0,
            focus_card: None,
            scroll_to: None,
            scroll_end: false,
            ime_composing: false,
            settings: false,
            theme,
            questions: HashMap::new(),
            budgets: HashMap::new(),
            dismissed: BTreeSet::new(),
            focus_search: false,
            focus_composer: false,
            rename: None,
            create_project: None,
            heights: HashMap::new(),
        }
    }

    pub(crate) fn toggle_menu(&mut self, m: Menu) {
        if self.menu.as_ref() == Some(&m) {
            self.menu = None;
        } else {
            self.menu = Some(m);
        }
    }
}

/// The Create project dialog (screen 29).
#[derive(Clone, Debug)]
pub(crate) struct CreateProject {
    pub target: TargetId,
    pub folder: String,
    pub name: String,
    /// "Use the repository root: …", when the folder is inside one.
    pub repo_root: Option<String>,
    /// The draft to put in the project once created ("New project…" from a draft).
    pub for_draft: Option<u64>,
}

/// What the user did this frame.
#[derive(Clone, Debug)]
pub(crate) enum Action {
    Open(ThreadKey),
    OpenAt(ThreadKey, Option<ItemId>),
    OpenRequest(ThreadKey),
    Respond { key: ThreadKey, request: RequestId, response: RequestResponse, summary: String },
    Send(ThreadKey),
    Stop(ThreadKey),
    StartDraft(u64),
    NewThread,
    CheckLatest(ThreadKey),
    ContinueBudget(Address),
    Fork(ThreadKey, InstanceId),
    SetMuted(ThreadKey, bool),
    SetPinned(ThreadKey, bool),
    CreateProject(CreateProject),
    DraftProject(u64, String),
    Archive(ThreadKey),
    Rename(ThreadKey, String),
    SetModel(ThreadKey, yaac_protocol::ModelId),
    SetEffort(ThreadKey, yaac_protocol::EffortId),
    SetMode(ThreadKey, yaac_protocol::ModeId),
    SetAgents(ThreadKey, AgentsPolicy),
    DraftModel(u64, InstanceId, yaac_protocol::ModelId),
    DraftEffort(u64, yaac_protocol::EffortId),
    DraftMode(u64, yaac_protocol::ModeId),
    Retry(TargetId),
    Panel(PanelInput),
    Settings(bool),
    Theme(ThemeChoice),
    Scene(Scene),
    DemoReset,
    StreamReply,
}

/// The time the view models are computed at.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Clock {
    /// A fixed story time (demo, screenshots): the story's "now" plus seconds since start.
    Story { base: i64, frozen: bool },
    /// Real time.
    System,
}

/// The window.
pub(crate) struct YaacApp {
    pub client: Client,
    pub demo: Option<DemoHub>,
    pub scenario: Scenario,
    pub ui: UiState,
    pub p: Palette,
    pub md: markdown::Cache,
    pub clock: Clock,
    pub started: std::time::Instant,
    pub shots: Option<Shots>,
    last_tick: f64,
    pub applied: Option<bool>,
}

impl std::fmt::Debug for YaacApp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("YaacApp")
    }
}

/// The story's present for the demo: 2026-11-20 14:00 UTC.
pub(crate) const STORY_NOW: i64 = 1_795_197_600;

pub(crate) fn demo_client(scenario: Scenario) -> (Client, DemoHub) {
    let hub = DemoHub::new(scenario, STORY_NOW);
    let mut client = Client::new(Box::new(hub.clone()), env!("CARGO_PKG_VERSION"), ClientId::new(DEMO_CLIENT));
    client.set_warm(hub.all_threads());
    match scenario {
        Scenario::Fleet => DemoHub::apply_seams(&mut client.state),
        Scenario::Fresh => client.state.projects.push(yaac_core::projects::Project {
            id: "local:api".into(),
            name: "api".into(),
            target: TargetId::new("local"),
            folder: "~/code/api".into(),
            repo: None,
        }),
    }
    for _ in 0..1000 {
        if !client.pump() {
            break;
        }
    }
    (client, hub)
}

impl YaacApp {
    pub(crate) fn new(cc: &eframe::CreationContext<'_>, demo: Option<Scenario>, theme: ThemeChoice, shots: Option<Shots>) -> Self {
        theme::install_fonts(&cc.egui_ctx);
        theme::apply(&cc.egui_ctx, &Palette::DARK);
        theme::apply(&cc.egui_ctx, &Palette::LIGHT);
        let (client, hub, scenario, clock) = match demo {
            Some(s) => {
                let (c, h) = demo_client(s);
                (c, Some(h), s, Clock::Story { base: STORY_NOW, frozen: shots.is_some() })
            }
            None => {
                // Real hub attachment is a later package: the seam is a `HubLink`.
                let c = Client::new(Box::new(yaac_core::link::DetachedLink), env!("CARGO_PKG_VERSION"), ClientId::new("window"));
                (c, None, Scenario::Fleet, Clock::System)
            }
        };
        let mut app = Self {
            client,
            demo: hub,
            scenario,
            ui: UiState::new(theme),
            p: Palette::DARK,
            md: markdown::Cache::default(),
            clock,
            started: std::time::Instant::now(),
            shots,
            last_tick: 0.0,
            applied: None,
        };
        if app.demo.is_some() {
            app.land();
        }
        app
    }

    /// Where the demo starts: the orchestrator (or a fresh draft on a fresh install).
    pub(crate) fn land(&mut self) {
        match self.scenario {
            Scenario::Fleet => self.open(&yaac_core::demo::threads::orchestrator(), None),
            Scenario::Fresh => {
                let id = self.client.new_thread(TargetId::new("local"), Some("~/code/api".into()));
                self.client.set_draft_project(id, "local:api");
            }
        }
    }

    pub(crate) fn reset_demo(&mut self, scenario: Scenario) {
        let (c, h) = demo_client(scenario);
        self.client = c;
        self.demo = Some(h);
        self.scenario = scenario;
        self.md.clear();
        let theme = self.ui.theme;
        let panel = self.ui.panel;
        self.ui = UiState::new(theme);
        self.ui.panel = panel;
        self.land();
    }

    pub(crate) fn now(&self) -> i64 {
        match self.clock {
            Clock::Story { base, frozen: true } => base,
            Clock::Story { base, frozen: false } => base + i64::try_from(self.started.elapsed().as_secs()).unwrap_or(0),
            Clock::System => std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0)),
        }
    }

    /// Open a thread, optionally at an item; focus its pending card when it has one.
    pub(crate) fn open(&mut self, key: &ThreadKey, item: Option<ItemId>) {
        self.client.open(key);
        self.pump_demo();
        self.ui.settings = false;
        self.client.state.focus_item = item.clone().map(|i| (key.clone(), i));
        self.ui.scroll_to = item;
        if self.ui.scroll_to.is_none() {
            self.ui.scroll_end = true;
        }
        self.ui.focus_card = self.pending_card(key);
    }

    fn pending_card(&self, key: &ThreadKey) -> Option<ItemId> {
        if !self.client.state.may_act(key) {
            return None;
        }
        let t = self.client.state.transcripts.get(key)?;
        t.items().iter().rev().find_map(|i| match &i.body {
            yaac_protocol::ItemBody::Request(r) if r.state == yaac_protocol::RequestState::Pending => Some(i.id.clone()),
            yaac_protocol::ItemBody::Notice(n)
                if n.notice_kind == yaac_protocol::NoticeKind::BudgetExhausted
                    && yaac_core::agents::root_of(&self.client.state, key)
                        .and_then(|r| self.client.state.budgets.get(&r))
                        .is_some_and(|b| b.exhausted) =>
            {
                Some(i.id.clone())
            }
            _ => None,
        })
    }

    pub(crate) fn pump_demo(&mut self) -> bool {
        let mut any = false;
        for _ in 0..1000 {
            if !self.client.pump() {
                break;
            }
            any = true;
        }
        any
    }

    fn apply(&mut self, actions: Vec<Action>) {
        for a in actions {
            match a {
                Action::Open(k) => self.open(&k, None),
                Action::OpenAt(k, i) => self.open(&k, i),
                Action::OpenRequest(k) => {
                    self.open(&k, None);
                    let item = self.pending_card(&k);
                    self.ui.scroll_to = item.clone();
                    self.ui.focus_card = item;
                }
                Action::Respond { key, request, response, summary } => {
                    self.client.respond(&key, &request, &response, summary);
                    self.ui.questions.remove(&request);
                }
                Action::Send(k) => {
                    if self.client.send(&k) {
                        self.ui.scroll_end = true;
                    }
                }
                Action::Stop(k) => self.client.interrupt(&k),
                Action::StartDraft(id) => {
                    self.client.start_thread(id);
                }
                Action::NewThread => {
                    let target = self
                        .client
                        .state
                        .selected_thread()
                        .map(|k| k.target.clone())
                        .or_else(|| self.client.state.targets.iter().find(|t| t.connected()).map(|t| t.entry.id.clone()));
                    if let Some(t) = target {
                        let cwd = self
                            .client
                            .state
                            .threads
                            .iter()
                            .filter(|(k, _)| k.target == t)
                            .max_by_key(|(_, r)| r.summary.updated_at.clone())
                            .map(|(_, r)| r.summary.cwd.clone());
                        let selected_project = self.client.state.selected_thread().and_then(|k| self.client.state.project_of(k)).map(|p| p.id.clone());
                        let id = self.client.new_thread(t, cwd);
                        if let Some(p) = selected_project.or_else(|| self.client.state.projects.first().map(|p| p.id.clone())) {
                            self.client.set_draft_project(id, &p);
                        }
                        self.ui.settings = false;
                        self.ui.focus_composer = true;
                    }
                }
                Action::CheckLatest(k) => self.client.check_latest(&k),
                Action::ContinueBudget(r) => self.client.continue_budget(&r),
                Action::Fork(k, i) => self.client.fork(&k, &i),
                Action::SetMuted(k, m) => self.client.set_muted(&k, m),
                Action::SetPinned(k, v) => self.client.set_pinned(&k, v),
                Action::CreateProject(cp) => {
                    self.client.create_project(&cp.target, &cp.folder, &cp.name);
                    if let (Some(d), Some(p)) = (cp.for_draft, self.client.state.projects.last().map(|p| p.id.clone())) {
                        self.client.set_draft_project(d, &p);
                    }
                }
                Action::DraftProject(d, p) => self.client.set_draft_project(d, &p),
                Action::Archive(k) => self.client.archive(&k),
                Action::Rename(k, t) => self.client.rename(&k, &t),
                Action::SetModel(k, m) => {
                    self.client.state.next_settings.entry(k).or_default().model = Some(m);
                }
                Action::SetEffort(k, e) => {
                    self.client.state.next_settings.entry(k).or_default().effort = Some(e);
                }
                Action::SetMode(k, m) => {
                    self.client.state.next_settings.entry(k).or_default().mode = Some(m);
                }
                Action::SetAgents(k, pol) => self.client.set_agents_policy(&k, pol),
                Action::DraftModel(id, inst, model) => {
                    let providers = self
                        .client
                        .state
                        .new_threads
                        .iter()
                        .find(|d| d.id == id)
                        .and_then(|d| self.client.state.target(&d.target))
                        .map(|t| t.providers.clone())
                        .unwrap_or_default();
                    if let Some(d) = self.client.state.new_threads.iter_mut().find(|d| d.id == id) {
                        let info = providers.iter().find(|p| p.instance == inst).and_then(|p| p.models.iter().find(|m| m.id == model));
                        d.effort = info.and_then(|m| yaac_core::controls::reconcile_effort(m, d.effort.as_ref()));
                        if d.instance.as_ref() != Some(&inst) {
                            d.mode = None;
                        }
                        d.instance = Some(inst);
                        d.model = Some(model);
                    }
                }
                Action::DraftEffort(id, e) => {
                    if let Some(d) = self.client.state.new_threads.iter_mut().find(|d| d.id == id) {
                        d.effort = Some(e);
                    }
                }
                Action::DraftMode(id, m) => {
                    if let Some(d) = self.client.state.new_threads.iter_mut().find(|d| d.id == id) {
                        d.mode = Some(m);
                    }
                }
                Action::Retry(t) => self.client.retry_target(&t),
                Action::Panel(i) => self.ui.panel = self.ui.panel.next(i),
                Action::Settings(open) => {
                    self.ui.settings = open;
                }
                Action::Theme(t) => {
                    self.ui.theme = t;
                    self.applied = None;
                }
                Action::Scene(s) => {
                    if let Some(h) = &self.demo {
                        h.play(s);
                    }
                    self.pump_demo();
                    if let Some(k) = self.client.state.selected_thread().cloned() {
                        self.ui.scroll_end = true;
                        self.ui.focus_card = self.pending_card(&k);
                    }
                }
                Action::DemoReset => self.reset_demo(self.scenario),
                Action::StreamReply => {
                    if let Some(k) = self.client.state.selected_thread().cloned() {
                        self.client.state.drafts.entry(k.clone()).or_default().text =
                            "Add a regression test for the last-frame drop.".into();
                        self.client.send(&k);
                        self.ui.scroll_end = true;
                    }
                }
            }
        }
    }

    fn palette_for(&self, ctx: &egui::Context) -> Palette {
        let dark = match self.ui.theme {
            ThemeChoice::Dark => true,
            ThemeChoice::Light => false,
            ThemeChoice::System => ctx.system_theme().is_none_or(|t| t == egui::Theme::Dark),
        };
        if dark { Palette::DARK } else { Palette::LIGHT }
    }

    fn keyboard(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let ctrl = |k: Key| ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, k));
        if ctrl(Key::J) {
            self.ui.needs_popup = !self.ui.needs_popup;
            self.ui.needs_sel = 0;
        }
        if ctrl(Key::N) {
            actions.push(Action::NewThread);
        }
        if ctrl(Key::K) {
            self.ui.focus_search = true;
        }
        if ctrl(Key::Comma) {
            actions.push(Action::Settings(true));
        }
        if self.ui.needs_popup && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            self.ui.needs_popup = false;
        }
        if self.ui.menu.is_some() && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            self.ui.menu = None;
        }
    }
}

impl eframe::App for YaacApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Apply everything that arrived since the last frame: a burst of events becomes one
        // repaint.
        let arrived = self.pump_demo();
        let t = ctx.input(|i| i.time);
        if let Some(h) = &self.demo
            && h.busy()
            && self.shots.is_none()
        {
            if t - self.last_tick >= 0.035 {
                self.last_tick = t;
                h.tick();
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(35));
        }
        if arrived {
            ctx.request_repaint();
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let p = self.palette_for(&ctx);
        if self.applied != Some(p.dark) {
            ctx.set_theme(if p.dark { egui::Theme::Dark } else { egui::Theme::Light });
            self.applied = Some(p.dark);
        }
        self.p = p;
        if let Some(mut shots) = self.shots.take() {
            let done = shots.step(self, &ctx);
            if done {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            } else {
                self.shots = Some(shots);
            }
        }
        let mut actions = Vec::new();
        self.keyboard(&ctx, &mut actions);
        let map = AttentionMap::compute(&self.client.state);
        let now = self.now();
        if self.ui.settings {
            crate::settings::show(ui, self, &mut actions);
        } else {
            crate::shell::show(ui, self, &map, now, &mut actions, frame);
        }
        self.apply(actions);
    }
}
