//! The window: shell layout, input, and the loop that applies hub messages.
//!
//! Rendering reads the state and records [`Action`]s; they are applied after the frame's UI
//! is built, so drawing never mutates what it is drawing from. The UI thread never blocks:
//! the hub link is polled (non-blocking), and a repaint is requested only when something
//! arrived (the link's waker), the demo is streaming, or an arrow is fading.

use std::collections::{BTreeMap, BTreeSet, HashMap};
#[cfg(feature = "hub")]
use std::sync::{Arc, Mutex};

use eframe::egui::{self, Key, Modifiers};
use yaac_core::activity::Expansion;
use yaac_core::agents::{PanelInput, PanelMode};
use yaac_core::attention::AttentionMap;
use yaac_core::client::Client;
use yaac_core::demo::ssh::{ScriptedSsh, SshScript};
use yaac_core::demo::{DEMO_CLIENT, DemoHub, Scenario, Scene};
use yaac_core::fork::ForkChoice;
use yaac_core::mode_confirm::ModeTarget;
use yaac_core::permission::BudgetFields;
use yaac_core::prefs::{SendKey, ThemePref, UiPrefs};
use yaac_core::settings::Page;
use yaac_core::sidebar::Grouping;
use yaac_core::ssh_setup::{SetupRecord, SshSetupBackend};
use yaac_core::state::{AgentsPolicy, HelloOutcome};
use yaac_core::ThreadKey;
use yaac_protocol::attach::TargetTransport;
use yaac_protocol::{Address, ClientId, InstanceId, ItemId, ModeId, ProviderKind, QuestionId, RequestId, RequestResponse, TargetId};

#[cfg(feature = "hub")]
use crate::attach::{AttachConfig, HubAttach, LinkStatus, SharedAttach};
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
    ForkDest,
    SettingsTarget,
    InstanceKind,
    Demo,
}

/// The user's in-progress answers to a question card. UI memory only: never written to the
/// state or to disk, so a secret is never cached.
#[derive(Clone, Debug, Default)]
pub(crate) struct QuestionDraft {
    pub selected: BTreeMap<QuestionId, BTreeSet<String>>,
    pub other: BTreeMap<QuestionId, String>,
}

/// The fork dialog (screen 11).
#[derive(Clone, Debug)]
pub(crate) struct ForkDialog {
    pub key: ThreadKey,
    pub choice: ForkChoice,
    pub preview_open: bool,
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
    pub settings_page: Page,
    pub theme: ThemeChoice,
    pub font_size: f32,
    pub send_key: SendKey,
    pub questions: HashMap<RequestId, QuestionDraft>,
    pub budgets: HashMap<RequestId, BudgetFields>,
    pub dismissed: BTreeSet<TargetId>,
    pub focus_search: bool,
    pub focus_composer: bool,
    pub rename: Option<(ThreadKey, String)>,
    pub create_project: Option<CreateProject>,
    pub fork: Option<ForkDialog>,
    /// Narrow layout: the thread list is open as an overlay.
    pub threads_overlay: bool,
    /// Screenshots only: lay the shell out in this size instead of the window's.
    pub layout_size: Option<egui::Vec2>,
    /// The settings pages' own state.
    pub set: crate::settings::SettingsUi,
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
            settings_page: Page::Targets,
            theme,
            font_size: theme::size::UI,
            send_key: SendKey::Enter,
            questions: HashMap::new(),
            budgets: HashMap::new(),
            dismissed: BTreeSet::new(),
            focus_search: false,
            focus_composer: false,
            rename: None,
            create_project: None,
            fork: None,
            threads_overlay: false,
            layout_size: None,
            set: crate::settings::SettingsUi::default(),
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

    /// The persisted choices as they are now.
    pub(crate) fn prefs(&self) -> UiPrefs {
        UiPrefs {
            grouping: self.grouping,
            docked: self.panel == PanelMode::Docked,
            theme: match self.theme {
                ThemeChoice::System => ThemePref::System,
                ThemeChoice::Dark => ThemePref::Dark,
                ThemeChoice::Light => ThemePref::Light,
            },
            muted_open: self.muted_open,
            font_size: self.font_size,
            send_key: self.send_key,
        }
    }

    #[cfg_attr(not(feature = "hub"), allow(dead_code))]
    fn load_prefs(&mut self, p: &UiPrefs) {
        self.grouping = p.grouping;
        self.panel = if p.docked { PanelMode::Docked } else { PanelMode::Pill };
        self.theme = match p.theme {
            ThemePref::System => ThemeChoice::System,
            ThemePref::Dark => ThemeChoice::Dark,
            ThemePref::Light => ThemeChoice::Light,
        };
        self.muted_open = p.muted_open;
        self.font_size = p.font_size;
        self.send_key = p.send_key;
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
    /// The folder browser is open at this directory ("" for the home directory).
    pub browse: Option<String>,
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
    OpenFork(ThreadKey),
    Fork(ThreadKey, ForkChoice),
    SetMuted(ThreadKey, bool),
    SetPinned(ThreadKey, bool),
    CreateProject(CreateProject),
    DraftProject(u64, String),
    Archive(ThreadKey),
    Rename(ThreadKey, String),
    SetModel(ThreadKey, yaac_protocol::ModelId),
    SetEffort(ThreadKey, yaac_protocol::EffortId),
    ChooseMode(ModeTarget, ModeId),
    ConfirmMode,
    CancelMode,
    SetAgents(ThreadKey, AgentsPolicy),
    DraftModel(u64, InstanceId, yaac_protocol::ModelId),
    DraftEffort(u64, yaac_protocol::EffortId),
    Retry(TargetId),
    Panel(PanelInput),
    Settings(bool),
    SettingsPage(Page),
    Theme(ThemeChoice),
    TargetEnabled(TargetId, bool),
    RemoveTarget(TargetId),
    AddTarget(String, TargetTransport),
    SetRecord(TargetId, SetupRecord),
    WslOn(bool),
    RefreshProviders(TargetId, Option<InstanceId>),
    AddInstance(TargetId, ProviderKind, String, String),
    UpdateInstance(TargetId, InstanceId, String, String),
    InstanceEnabled(TargetId, InstanceId, bool),
    RemoveInstance(TargetId, InstanceId),
    OutsideTools(TargetId, InstanceId, bool),
    SetAlways(bool),
    ClearCache,
    FsList(TargetId, String),
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

/// The attachment to a real hub.
#[cfg(feature = "hub")]
pub(crate) struct HubConn {
    pub attach: Arc<Mutex<HubAttach>>,
    pub config: AttachConfig,
    shutdown_sent: bool,
    restarted: bool,
    prefs_loaded: bool,
}

/// SEAM: the real SSH setup backend belongs to the hub (it owns every connection to a target),
/// and the attach protocol has no `hub/ssh/*` methods yet. Attached to a real hub, every step
/// fails with this message rather than opening an SSH session from the window.
#[derive(Debug, Default)]
pub(crate) struct HubSshPending {
    queue: std::collections::VecDeque<yaac_core::ssh_setup::StepReport>,
}

impl SshSetupBackend for HubSshPending {
    fn reach(&mut self, _c: &yaac_core::ssh_setup::SshConn) {
        self.queue.push_back(yaac_core::ssh_setup::StepReport::Reached(Err(Self::err())));
    }
    fn probe(&mut self, _c: &yaac_core::ssh_setup::SshConn, _k: &yaac_core::ssh_setup::HostKey) {
        self.queue.push_back(yaac_core::ssh_setup::StepReport::Probed(Err(Self::err())));
    }
    fn sign_in(&mut self, _c: &yaac_core::ssh_setup::SshConn, credential: yaac_core::ssh_setup::Credential) {
        drop(credential);
        self.queue.push_back(yaac_core::ssh_setup::StepReport::SignedIn(Err(Self::err())));
    }
    fn install_key(&mut self, _c: &yaac_core::ssh_setup::SshConn) {
        self.queue.push_back(yaac_core::ssh_setup::StepReport::KeyInstalled(Err(Self::err())));
    }
    fn verify(&mut self, _c: &yaac_core::ssh_setup::SshConn, _k: &yaac_core::ssh_setup::InstalledKey) {
        self.queue.push_back(yaac_core::ssh_setup::StepReport::Verified(Err(Self::err())));
    }
    fn remove_key(&mut self, _c: &yaac_core::ssh_setup::SshConn, _k: &yaac_core::ssh_setup::InstalledKey) {
        self.queue.push_back(yaac_core::ssh_setup::StepReport::KeyRemoved(Err(Self::err())));
    }
    fn revoke(&mut self, _c: &yaac_core::ssh_setup::SshConn, _k: &yaac_core::ssh_setup::InstalledKey) {
        self.queue.push_back(yaac_core::ssh_setup::StepReport::Revoked(Err(Self::err())));
    }
    fn poll(&mut self) -> Option<yaac_core::ssh_setup::StepReport> {
        self.queue.pop_front()
    }
}

impl HubSshPending {
    fn err() -> yaac_core::ssh_setup::StepError {
        yaac_core::ssh_setup::StepError::new("SSH setup is not available in this build yet: the hub does not offer it.")
    }
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
    #[cfg(feature = "hub")]
    pub hub: Option<HubConn>,
    /// The host is Windows (or pretends to be, `--host windows`): WSL settings are shown.
    pub host_windows: bool,
    pub ssh: Box<dyn SshSetupBackend>,
    saved_prefs: UiPrefs,
    last_tick: f64,
    pub applied: Option<bool>,
    #[cfg_attr(not(feature = "hub"), allow(dead_code))]
    ctx: egui::Context,
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
    let mut client = Client::new(Box::new(hub.clone()), crate::BUILD, ClientId::new(DEMO_CLIENT));
    client.set_warm(hub.all_threads());
    if scenario == Scenario::Fleet {
        DemoHub::apply_seams(&mut client.state);
    }
    for _ in 0..1000 {
        if !client.pump() {
            break;
        }
    }
    (client, hub)
}

#[cfg(feature = "hub")]
fn waker(ctx: &egui::Context) -> crate::attach::Waker {
    let ctx = ctx.clone();
    Arc::new(move || ctx.request_repaint())
}

#[cfg(feature = "hub")]
fn hub_client(attach: &Arc<Mutex<HubAttach>>) -> Client {
    // The client id is a placeholder until `hub/hello` returns this host's own.
    Client::new(Box::new(SharedAttach(attach.clone())), crate::BUILD, ClientId::new("window"))
}

impl YaacApp {
    pub(crate) fn new(cc: &eframe::CreationContext<'_>, launch: crate::Launch) -> Self {
        theme::install_fonts(&cc.egui_ctx);
        theme::apply(&cc.egui_ctx, &Palette::DARK);
        theme::apply(&cc.egui_ctx, &Palette::LIGHT);
        #[cfg(feature = "hub")]
        let mut hub = None;
        let (client, demo, scenario, clock, ssh): (Client, Option<DemoHub>, Scenario, Clock, Box<dyn SshSetupBackend>) = match launch.demo {
            Some(s) => {
                let (c, h) = demo_client(s);
                (c, Some(h), s, Clock::Story { base: STORY_NOW, frozen: launch.shots.is_some() }, Box::new(ScriptedSsh::new(SshScript::Happy)))
            }
            None => {
                #[cfg(feature = "hub")]
                if let Some(config) = launch.attach {
                    let attach = Arc::new(Mutex::new(HubAttach::start(config.clone(), waker(&cc.egui_ctx))));
                    let c = hub_client(&attach);
                    hub = Some(HubConn { attach, config, shutdown_sent: false, restarted: false, prefs_loaded: false });
                    (c, None, Scenario::Fleet, Clock::System, Box::new(HubSshPending::default()) as Box<dyn SshSetupBackend>)
                } else {
                    let c = Client::new(Box::new(yaac_core::link::DetachedLink), crate::BUILD, ClientId::new("window"));
                    (c, None, Scenario::Fleet, Clock::System, Box::new(HubSshPending::default()))
                }
                #[cfg(not(feature = "hub"))]
                {
                    let c = Client::new(Box::new(yaac_core::link::DetachedLink), crate::BUILD, ClientId::new("window"));
                    (c, None, Scenario::Fleet, Clock::System, Box::new(HubSshPending::default()))
                }
            }
        };
        let ui = UiState::new(launch.theme);
        let saved_prefs = ui.prefs();
        let mut app = Self {
            client,
            demo,
            scenario,
            ui,
            p: Palette::DARK,
            md: markdown::Cache::default(),
            clock,
            started: std::time::Instant::now(),
            shots: launch.shots,
            #[cfg(feature = "hub")]
            hub,
            host_windows: launch.host_windows,
            ssh,
            saved_prefs,
            last_tick: 0.0,
            applied: None,
            ctx: cc.egui_ctx.clone(),
        };
        if app.demo.is_some() {
            app.land();
        }
        app
    }

    /// Where the demo starts: the orchestrator, or the first-run page on a fresh install.
    pub(crate) fn land(&mut self) {
        if self.scenario == Scenario::Fleet {
            self.open(&yaac_core::demo::threads::orchestrator(), None);
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
        self.ssh = Box::new(ScriptedSsh::new(SshScript::Happy));
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
        self.ui.threads_overlay = false;
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

    /// Start a new-thread draft on the open thread's machine (or the first connected one), in
    /// its project (or the first project there).
    pub(crate) fn new_thread(&mut self) {
        let st = &self.client.state;
        let target = st
            .selected_thread()
            .map(|k| k.target.clone())
            .or_else(|| st.targets.iter().find(|t| t.connected() && matches!(t.entry.transport, TargetTransport::Local(_))).map(|t| t.entry.id.clone()))
            .or_else(|| st.targets.iter().find(|t| t.connected()).map(|t| t.entry.id.clone()));
        let Some(t) = target else { return };
        let selected_project = st.selected_thread().and_then(|k| st.project_of(k)).map(|p| p.id.clone());
        let first_here = st.projects.iter().find(|p| p.target == t).map(|p| p.id.clone());
        let id = self.client.new_thread(t, None);
        if let Some(p) = selected_project.or(first_here) {
            self.client.set_draft_project(id, &p);
        }
        self.ui.settings = false;
        self.ui.threads_overlay = false;
        self.ui.focus_composer = true;
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
                Action::NewThread => self.new_thread(),
                Action::CheckLatest(k) => self.client.check_latest(&k),
                Action::ContinueBudget(r) => self.client.continue_budget(&r),
                Action::OpenFork(k) => {
                    if let Some(choice) = yaac_core::fork::default_choice(&self.client.state, &k) {
                        if self.client.state.selected_thread() != Some(&k) {
                            self.open(&k, None);
                        }
                        self.client.fork_preview(&k, &choice.instance);
                        self.ui.fork = Some(ForkDialog { key: k, choice, preview_open: false });
                        self.ui.menu = None;
                    }
                }
                Action::Fork(k, c) => {
                    self.client.fork(&k, &c);
                    self.ui.fork = None;
                }
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
                Action::ChooseMode(t, m) => self.client.choose_mode(t, m),
                Action::ConfirmMode => self.client.confirm_mode(),
                Action::CancelMode => self.client.cancel_mode(),
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
                Action::Retry(t) => self.client.retry_target(&t),
                Action::Panel(i) => self.ui.panel = self.ui.panel.next(i),
                Action::Settings(open) => {
                    self.ui.settings = open;
                    self.ui.threads_overlay = false;
                }
                Action::SettingsPage(p) => {
                    self.ui.settings = true;
                    self.ui.settings_page = p;
                    self.ui.menu = None;
                }
                Action::Theme(t) => {
                    self.ui.theme = t;
                    self.applied = None;
                }
                Action::TargetEnabled(t, e) => self.client.set_target_enabled(&t, e),
                Action::RemoveTarget(t) => self.client.remove_target(&t),
                Action::AddTarget(name, transport) => self.client.add_target(&name, &transport),
                Action::SetRecord(t, r) => {
                    self.client.state.ssh_records.insert(t, r);
                }
                Action::WslOn(on) => self.client.state.wsl_on = on,
                Action::RefreshProviders(t, i) => self.client.refresh_providers(&t, i.as_ref()),
                Action::AddInstance(t, kind, name, socket) => self.client.add_instance(&t, &kind, Some(&name), Some(&socket)),
                Action::UpdateInstance(t, i, name, socket) => {
                    let name = name.trim().to_owned();
                    let socket = socket.trim().to_owned();
                    self.client.update_instance(&t, &i, Some((!name.is_empty()).then_some(name.as_str())), Some((!socket.is_empty()).then_some(socket.as_str())), None);
                }
                Action::InstanceEnabled(t, i, e) => self.client.update_instance(&t, &i, None, None, Some(e)),
                Action::RemoveInstance(t, i) => self.client.remove_instance(&t, &i),
                Action::OutsideTools(t, i, on) => self.client.set_outside_tools(&t, &i, on),
                Action::SetAlways(on) => {
                    self.client.set_always(on);
                    #[cfg(feature = "hub")]
                    if let Some(h) = &self.hub
                        && let Some(a) = &h.config.autostart
                    {
                        // The login entry follows the setting (the hub keeps running either way
                        // while a window is attached).
                        let _ = if on { yaac_hub::autostart::install_autostart(a) } else { yaac_hub::autostart::remove_autostart(a) };
                    }
                }
                Action::ClearCache => self.client.clear_cache(),
                Action::FsList(t, path) => self.client.fs_list(&t, &path),
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

    /// The hello outcomes, the old hub's replacement, and the persisted choices.
    #[cfg(feature = "hub")]
    fn hub_upkeep(&mut self) {
        let Some(h) = &mut self.hub else { return };
        let status = h.attach.lock().map_or(LinkStatus::Closed, |a| a.status());
        if let LinkStatus::Failed(e) = &status {
            self.client.state.hub.last_error = Some(format!("Could not reach the hub: {e}"));
        }
        match self.client.state.hub.outcome.clone() {
            HelloOutcome::HubOutdated { .. } if !h.restarted => {
                if !h.shutdown_sent {
                    self.client.shutdown_hub();
                    h.shutdown_sent = true;
                }
                if self.client.state.hub.shutdown_answered || status == LinkStatus::Closed {
                    // The old hub is exiting: start this build's hub and attach again.
                    let attach = Arc::new(Mutex::new(HubAttach::restart(h.config.clone(), waker(&self.ctx))));
                    h.attach = attach.clone();
                    h.restarted = true;
                    self.client = hub_client(&attach);
                }
            }
            HelloOutcome::HubOutdated { hub_build } => {
                self.client.state.hub.last_error = Some(format!("The running hub ({hub_build}) could not be replaced by this build."));
            }
            HelloOutcome::WindowOutdated { message } => {
                // Attach to nothing: this window is older than the hub.
                let mut c = Client::new(Box::new(yaac_core::link::DetachedLink), crate::BUILD, ClientId::new("window"));
                c.state.hub.outcome = HelloOutcome::WindowOutdated { message };
                self.client = c;
                self.hub = None;
            }
            HelloOutcome::Attached if !h.prefs_loaded => {
                let stored = h.attach.lock().ok().and_then(|mut a| a.take_prefs());
                if let Some(stored) = stored {
                    let mut p = self.ui.prefs();
                    for (k, v) in &stored {
                        p.load(k, v);
                    }
                    self.ui.load_prefs(&p);
                    self.saved_prefs = p;
                    self.applied = None;
                    h.prefs_loaded = true;
                }
            }
            _ => {}
        }
    }

    /// Send changed window-side choices to the hub (attached only; the demo keeps them in
    /// memory).
    fn persist_prefs(&mut self) {
        let now = self.ui.prefs();
        if now == self.saved_prefs {
            return;
        }
        #[cfg(feature = "hub")]
        if self.hub.as_ref().is_some_and(|h| h.prefs_loaded) {
            for k in now.changed(&self.saved_prefs) {
                self.client.set_ui_state(k, &now.value(k));
            }
        }
        self.saved_prefs = now;
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
        // repaint. Arrivals from a real hub wake the window through the link's waker.
        let arrived = self.pump_demo();
        #[cfg(feature = "hub")]
        self.hub_upkeep();
        let ssh_changed = self.ui.set.pump_ssh(self.ssh.as_mut());
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
        if arrived || ssh_changed {
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
        if self.shots.is_none() {
            let zoom = self.ui.font_size / theme::size::UI;
            if (ctx.zoom_factor() - zoom).abs() > 0.001 {
                ctx.set_zoom_factor(zoom);
            }
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
        if let HelloOutcome::WindowOutdated { message } = &self.client.state.hub.outcome {
            crate::first_run::update_needed(ui, &p, message);
        } else if self.ui.settings {
            crate::settings::show(ui, self, &mut actions);
        } else {
            crate::shell::show(ui, self, &map, now, &mut actions, frame);
        }
        crate::dialogs::mode_confirm(ui.ctx(), self, &mut actions);
        self.apply(actions);
        self.persist_prefs();
    }
}
