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
use yaac_core::history::{Anchor, Place};
use yaac_core::mode_confirm::ModeTarget;
use yaac_core::permission::BudgetFields;
use yaac_core::prefs::{SendKey, ThemePref, UiPrefs};
use yaac_core::settings::Page;
use yaac_core::sidebar::Grouping;
use yaac_core::ssh_setup::{SetupRecord, SshSetupBackend};
use yaac_core::attachments::DraftRef;
use yaac_core::state::{AgentsPolicy, HelloOutcome, SendAs};
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
    /// The Archived section is open.
    pub archived_open: bool,
    /// The Mute dialog.
    pub mute: Option<yaac_core::mute::MuteDialog>,
    /// Scroll the thread list to its end on the next frame (screenshots).
    pub sidebar_to_end: bool,
    /// The sidebar's width, or the strip (D94).
    pub sidebar: yaac_core::strip::SidebarSize,
    /// The docked agents panel's width (D105), and its width when a drag of its edge began.
    pub agents_width: f32,
    pub agents_drag: Option<f32>,
    /// The edge's width when a drag of it began.
    pub sidebar_drag: Option<f32>,
    /// The strip's hover card: its open/close rule and the thread it shows.
    pub strip_hover: yaac_core::hover::HoverCard,
    pub strip_hover_key: Option<(ThreadKey, egui::Rect)>,
    pub expansion: Expansion,
    pub panel: PanelMode,
    /// The agents card's open/close rule.
    pub hover: yaac_core::hover::HoverCard,
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
    /// Screenshots only: show the "Drop to attach" target as if files were dragged over.
    pub drop_preview: bool,
    /// Screenshots only: lay the shell out in this size instead of the window's.
    pub layout_size: Option<egui::Vec2>,
    /// The settings pages' own state.
    pub set: crate::settings::SettingsUi,
    /// Item heights, keyed by content hash and width.
    pub heights: HashMap<u64, f32>,
    /// The transcript's text selection.
    pub textsel: crate::seltext::TextSel,
    /// Parsed diffs of file changes.
    pub diffs: crate::thread::DiffCache,
    /// Back and forward.
    pub history: yaac_core::history::History,
    /// Restore this reading position in the transcript on the next frame.
    pub restore: Option<yaac_core::history::Anchor>,
    /// A history button's list is open (true: forward).
    pub nav_menu: Option<bool>,
    /// The highlighted row of that list (keyboard).
    pub nav_sel: usize,
    /// A press on a history button opened its list by holding: its release is not a click.
    pub nav_held: bool,
}

impl UiState {
    fn new(theme: ThemeChoice) -> Self {
        Self {
            grouping: Grouping::default(),
            search: String::new(),
            muted_open: false,
            archived_open: false,
            mute: None,
            sidebar_to_end: false,
            sidebar: yaac_core::strip::SidebarSize::default(),
            agents_width: yaac_core::prefs::AGENTS_DEFAULT_WIDTH,
            agents_drag: None,
            sidebar_drag: None,
            strip_hover: yaac_core::hover::HoverCard::default(),
            strip_hover_key: None,
            expansion: Expansion::default(),
            panel: PanelMode::Pill,
            hover: yaac_core::hover::HoverCard::default(),
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
            drop_preview: false,
            layout_size: None,
            set: crate::settings::SettingsUi::default(),
            heights: HashMap::new(),
            textsel: crate::seltext::TextSel::default(),
            diffs: crate::thread::DiffCache::default(),
            history: yaac_core::history::History::default(),
            restore: None,
            nav_menu: None,
            nav_sel: 0,
            nav_held: false,
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
            sidebar: self.sidebar,
            agents_width: self.agents_width,
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
        self.sidebar = p.sidebar;
        self.agents_width = p.agents_width;
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
    /// Mute until a moment (`None`: no end), from the Mute dialog.
    MuteUntil(ThreadKey, Option<i64>),
    /// Open the Mute dialog (`true`: "Change…", prefilled with the current mute).
    OpenMute(ThreadKey, bool),
    Unarchive(ThreadKey),
    Dismiss(ThreadKey, bool),
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
    /// Back (negative) or forward through the history, that many places.
    Nav(isize),
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
    /// The attachment button: open the file picker for this composer.
    AttachPick(DraftRef),
    /// Files to attach (dropped, picked, or pasted as a file list).
    AttachPaths(DraftRef, Vec<String>),
    /// The × on a chip.
    RemoveAttachment(DraftRef, u64),
    /// "Send as image" / "as file" on a chip whose model's image support is unknown.
    SendAs(DraftRef, u64, SendAs),
    /// "Retry" on a failed chip.
    RetryAttachment(DraftRef, u64),
    /// A long pasted text: attach it as a file (`true`) or paste it as text.
    LargePaste(bool),
    /// The composer's text changed: store the draft soon.
    DraftEdited(DraftRef),
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

/// What construction needs from eframe's creation context.
struct Cc {
    egui_ctx: egui::Context,
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
    /// The file picker, the clipboard, and thumbnails, on worker threads.
    pub files: crate::files::Files,
    /// When drafts were last stored in the hub (seconds of egui time).
    drafts_flushed: f64,
    /// The host's time zone (D91): every time shown or asked for is local to this computer.
    pub zone: yaac_core::time::Zone,
    zone_checked: f64,
    saved_prefs: UiPrefs,
    last_tick: f64,
    pub applied: Option<bool>,
    /// Diagnostics: `YAAC_DEBUG_COPY=<text>` copies the text on the first frame through the
    /// same path as Copy (egui's copy command, handed to the system clipboard by egui-winit),
    /// so the clipboard can be checked from outside the window.
    debug_copy: Option<String>,
    #[cfg_attr(not(feature = "hub"), allow(dead_code))]
    ctx: egui::Context,
}

impl std::fmt::Debug for YaacApp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("YaacApp")
    }
}

/// The host's UTC offset now. Read in the window, never in core (D91).
pub(crate) fn host_zone() -> yaac_core::time::Zone {
    yaac_core::time::Zone::new(chrono::Local::now().offset().local_minus_utc())
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
        Self::from_ctx(&cc.egui_ctx, launch)
    }

    /// The window on an egui context (the real one, or a headless one in tests).
    pub(crate) fn from_ctx(egui_ctx: &egui::Context, launch: crate::Launch) -> Self {
        let cc = Cc { egui_ctx: egui_ctx.clone() };
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
            files: crate::files::Files::new(&cc.egui_ctx),
            drafts_flushed: 0.0,
            #[cfg(feature = "hub")]
            hub,
            host_windows: launch.host_windows,
            ssh,
            zone: host_zone(),
            zone_checked: 0.0,
            saved_prefs,
            last_tick: 0.0,
            applied: None,
            debug_copy: std::env::var("YAAC_DEBUG_COPY").ok().filter(|t| !t.is_empty()),
            ctx: cc.egui_ctx,
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

    /// Open a thread, optionally at an item; focus its pending card when it has one. A new
    /// place in the history.
    pub(crate) fn open(&mut self, key: &ThreadKey, item: Option<ItemId>) {
        let anchor = item.clone().map(|item| Anchor { item, offset: 0.0 });
        self.open_here(key, item);
        self.ui.history.visit(Place::Thread(key.clone()), anchor);
    }

    /// Whether a place in the history still exists (a deleted thread is skipped).
    pub(crate) fn place_exists(&self, p: &Place) -> bool {
        match p {
            Place::Thread(k) => self.client.state.summary(k).is_some(),
            Place::Draft(id) => self.client.state.new_threads.iter().any(|d| d.id == *id),
            Place::Settings(_) => true,
        }
    }

    /// Move `n` places in the history (negative: back) and show the place arrived at, where
    /// the user was reading.
    pub(crate) fn nav(&mut self, n: isize) {
        let mut h = std::mem::take(&mut self.ui.history);
        let exists = |p: &Place| self.place_exists(p);
        let entry = h.jump(n, &exists).cloned();
        self.ui.history = h;
        let Some(e) = entry else { return };
        self.ui.nav_menu = None;
        match e.place {
            Place::Thread(k) => {
                self.open_here(&k, None);
                self.ui.focus_card = None;
                self.ui.scroll_end = e.anchor.is_none();
                self.ui.restore = e.anchor;
            }
            Place::Settings(page) => {
                self.ui.settings = true;
                self.ui.settings_page = page;
            }
            Place::Draft(id) => {
                self.client.state.selection = yaac_core::state::Selection::NewThread(id);
                self.ui.settings = false;
            }
        }
    }

    /// Open a thread without touching the history (Back and Forward use this).
    fn open_here(&mut self, key: &ThreadKey, item: Option<ItemId>) {
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

    /// The sidebar search goes to the hub (`hub/threads/find`) when attached to a real one.
    pub(crate) fn uses_hub_search(&self) -> bool {
        #[cfg(feature = "hub")]
        {
            self.hub.is_some() && self.client.state.hub.attached
        }
        #[cfg(not(feature = "hub"))]
        {
            false
        }
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
        let id = self.client.new_thread(t.clone(), None);
        match selected_project.or(first_here) {
            Some(p) => self.client.set_draft_project(id, &p),
            None => {
                // No project on this machine yet: a thread starts in a project's folder, so ask
                // for one right away (the draft goes into it once created).
                let mut cp = crate::sidebar::create_project_for(self, &t, "~");
                cp.for_draft = Some(id);
                self.ui.create_project = Some(cp);
            }
        }
        self.ui.settings = false;
        self.ui.threads_overlay = false;
        self.ui.focus_composer = true;
        self.ui.history.visit(Place::Draft(id), None);
    }

    pub(crate) fn apply(&mut self, actions: Vec<Action>) {
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
                Action::MuteUntil(k, u) => {
                    self.client.mute_until(&k, u);
                    self.ui.mute = None;
                }
                Action::OpenMute(k, change) => {
                    let now = self.now();
                    let title = self.client.state.summary(&k).map(|s| s.title.clone()).unwrap_or_default();
                    let until = self.client.state.muted_until(&k, now);
                    self.ui.mute = Some(if change {
                        yaac_core::mute::MuteDialog::prefill(k, &title, until, now, self.zone)
                    } else {
                        yaac_core::mute::MuteDialog::new(k, &title, now, self.zone)
                    });
                    self.ui.menu = None;
                }
                Action::Unarchive(k) => self.client.unarchive(&k),
                Action::Dismiss(k, d) => self.client.set_dismissed(&k, d),
                Action::SetPinned(k, v) => self.client.set_pinned(&k, v),
                Action::CreateProject(cp) => {
                    // The draft (if any) is put in the project once the target created it.
                    self.client.create_project(&cp.target, &cp.folder, &cp.name, cp.for_draft);
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
                    self.client.draft_changed(&DraftRef::New(id));
                }
                Action::DraftEffort(id, e) => {
                    if let Some(d) = self.client.state.new_threads.iter_mut().find(|d| d.id == id) {
                        d.effort = Some(e);
                    }
                    self.client.draft_changed(&DraftRef::New(id));
                }
                Action::Retry(t) => self.client.retry_target(&t),
                Action::Panel(i) => {
                    self.ui.panel = self.ui.panel.next(i);
                    if self.ui.panel != PanelMode::Card {
                        self.ui.hover.close();
                    }
                }
                Action::Settings(open) => {
                    self.ui.settings = open;
                    self.ui.threads_overlay = false;
                    if open {
                        self.ui.history.visit(Place::Settings(self.ui.settings_page), None);
                    } else {
                        // Leaving Settings goes back to where the user was.
                        let exists = |p: &Place| self.place_exists(p) && !matches!(p, Place::Settings(_));
                        if self.ui.history.can_back(&exists) && matches!(self.ui.history.current().map(|e| &e.place), Some(Place::Settings(_))) {
                            self.nav(-1);
                        }
                        self.ui.settings = false;
                    }
                }
                Action::SettingsPage(p) => {
                    self.ui.settings = true;
                    self.ui.settings_page = p;
                    self.ui.menu = None;
                    self.ui.history.visit(Place::Settings(p), None);
                }
                Action::Nav(n) => self.nav(n),
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
                Action::AttachPick(d) => self.files.pick(d),
                Action::AttachPaths(d, paths) => {
                    self.client.attach_files(&d, &paths);
                }
                Action::RemoveAttachment(d, id) => self.client.remove_attachment(&d, id),
                Action::SendAs(d, id, c) => self.client.choose_send_as(&d, id, c),
                Action::RetryAttachment(d, id) => self.client.retry_attachment(&d, id),
                Action::LargePaste(as_file) => {
                    if let Some(lp) = self.files.large_paste.take() {
                        if as_file {
                            self.files.text_as_file(lp.draft, lp.text);
                        } else {
                            self.files.paste_events.push(egui::Event::Paste(lp.text));
                            self.ctx.request_repaint();
                        }
                    }
                }
                Action::DraftEdited(d) => self.client.draft_changed(&d),
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

    /// The composer of what is open, when it takes attachments: a thread this window may act
    /// on, or a new-thread draft.
    pub(crate) fn open_composer(&self) -> Option<DraftRef> {
        match &self.client.state.selection {
            yaac_core::state::Selection::Thread(k) if self.client.state.may_act(k) && !self.ui.settings => Some(DraftRef::Thread(k.clone())),
            yaac_core::state::Selection::NewThread(id) if !self.ui.settings => Some(DraftRef::New(*id)),
            _ => None,
        }
    }

    /// Ctrl+V: the clipboard is read off the UI thread. Into the open composer (focused, or
    /// nothing focused) a copied file list or a bitmap becomes chips; into any other field only
    /// text is pasted.
    fn paste(&mut self, ctx: &egui::Context) {
        let focused = ctx.memory(|m| m.focused());
        let into = self.open_composer().filter(|d| focused.is_none() || focused == Some(crate::composer::composer_id(d)));
        use chrono::{Datelike as _, Timelike as _};
        let now = chrono::Local::now();
        self.files.paste(into, (now.year(), now.month(), now.day(), now.hour(), now.minute()));
    }

    /// Apply what the file workers finished. Returns whether anything arrived.
    fn pump_files(&mut self) -> bool {
        use crate::files::{Clip, Outcome};
        let outcomes = self.files.poll();
        let any = !outcomes.is_empty();
        for o in outcomes {
            match o {
                Outcome::Picked(d, paths) => {
                    self.files.picking = false;
                    if let Some(paths) = paths {
                        let paths: Vec<String> = paths.iter().map(|p| p.to_string_lossy().into_owned()).collect();
                        self.client.attach_files(&d, &paths);
                    }
                }
                Outcome::Pasted(into, clip) => match (into, clip) {
                    (Some(d), Clip::Files(paths)) => {
                        self.client.attach_files(&d, &paths);
                    }
                    (Some(_), Clip::Image(enc)) => self.attach_encoded(enc),
                    (Some(d), Clip::Text(text)) if yaac_core::attachments::is_large_paste(&text) => {
                        self.files.large_paste = Some(crate::files::LargePaste { draft: d, text });
                    }
                    (_, Clip::Text(text)) => self.files.paste_events.push(egui::Event::Paste(text)),
                    (_, Clip::Refused(why)) => self.files.notice = Some(why),
                    // The worker could not read the clipboard (a Wayland compositor without
                    // the data-control protocol and no XWayland): egui's own paste, which
                    // reads text through the window's connection, still works.
                    (_, Clip::Nothing) => self.ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaste),
                    // Files and bitmaps are read only for a composer.
                    (None, Clip::Files(_) | Clip::Image(_)) => {}
                },
                Outcome::Encoded(enc) => self.attach_encoded(enc),
                Outcome::Thumb(k, img) => self.files.set_thumb(k, img),
                Outcome::Notice(n) => self.files.notice = Some(n),
            }
        }
        any
    }

    fn attach_encoded(&mut self, enc: crate::files::Encoded) {
        let id = self.client.attach_bytes(&enc.draft, &enc.name, Some(&enc.media_type), enc.size, enc.chunks);
        if let (Some(id), Some(t)) = (id, enc.thumb) {
            self.files.set_thumb(crate::files::ThumbKey::Chip(enc.draft, id), Some(t));
        }
    }

    pub(crate) fn keyboard(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let ctrl = |k: Key| ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, k));
        if ctrl(Key::J) {
            self.ui.needs_popup = !self.ui.needs_popup;
            self.ui.needs_sel = 0;
        }
        if ctrl(Key::N) {
            actions.push(Action::NewThread);
        }
        if ctrl(Key::B) {
            self.ui.sidebar.toggle();
        }
        if ctrl(Key::K) {
            self.ui.focus_search = true;
        }
        if ctrl(Key::Comma) {
            actions.push(Action::Settings(true));
        }
        // Ctrl+V arrives as a key (`clipboard_shortcuts` is off): see `paste`.
        let paste = ctx.input_mut(|i| {
            i.consume_key(Modifiers::COMMAND, Key::V) || i.consume_key(Modifiers::NONE, Key::Paste) || (cfg!(windows) && i.consume_key(Modifiers::SHIFT, Key::Insert))
        });
        if paste {
            self.paste(ctx);
        }
        // Back and forward: Alt+Left / Alt+Right, and the mouse's side buttons.
        let (alt_left, alt_right, extra1, extra2) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::ALT, Key::ArrowLeft),
                i.consume_key(Modifiers::ALT, Key::ArrowRight),
                i.pointer.button_pressed(egui::PointerButton::Extra1),
                i.pointer.button_pressed(egui::PointerButton::Extra2),
            )
        });
        if alt_left || extra1 {
            actions.push(Action::Nav(-1));
        }
        if alt_right || extra2 {
            actions.push(Action::Nav(1));
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
    /// Ctrl+C and Ctrl+X become egui's copy and cut again (the clipboard shortcuts are off so
    /// that Ctrl+V reaches the window as a key), and text read from the clipboard by a worker
    /// is pasted into the focused field.
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw: &mut egui::RawInput) {
        for e in &mut raw.events {
            if let egui::Event::Key { key, pressed: true, modifiers, .. } = e {
                let copy = (modifiers.command_only() && *key == Key::C) || *key == Key::Copy || (cfg!(windows) && modifiers.ctrl && *key == Key::Insert);
                let cut = (modifiers.command_only() && *key == Key::X) || *key == Key::Cut || (cfg!(windows) && modifiers.shift && *key == Key::Delete);
                if copy {
                    *e = egui::Event::Copy;
                } else if cut {
                    *e = egui::Event::Cut;
                }
            }
        }
        raw.events.append(&mut self.files.paste_events);
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Apply everything that arrived since the last frame: a burst of events becomes one
        // repaint. Arrivals from a real hub wake the window through the link's waker.
        let arrived = self.pump_demo();
        #[cfg(feature = "hub")]
        self.hub_upkeep();
        let ssh_changed = self.ui.set.pump_ssh(self.ssh.as_mut());
        let t = ctx.input(|i| i.time);
        let files_changed = self.pump_files();
        // Drafts go to the hub's store at a calm pace, not on every key.
        if self.client.drafts_dirty() {
            if t - self.drafts_flushed >= 1.0 {
                self.client.flush_drafts();
                self.drafts_flushed = t;
            } else {
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(1.0 - (t - self.drafts_flushed)));
            }
        }
        // The offset changes with daylight saving: look again once a minute (only when a
        // frame happens anyway).
        if t - self.zone_checked > 60.0 || self.zone_checked == 0.0 {
            self.zone = host_zone();
            self.zone_checked = t.max(f64::MIN_POSITIVE);
        }
        // A timed mute ends by itself: one repaint when the nearest one is due, so the row
        // comes back on time without repainting meanwhile.
        let now = self.now();
        if let Some(at) = self.client.state.next_unmute(now) {
            ctx.request_repaint_after(std::time::Duration::from_secs(u64::try_from(at - now).unwrap_or(0)));
        }
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
        if arrived || ssh_changed || files_changed {
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
        // A few frames in, once the window and its clipboard are up.
        if ctx.cumulative_frame_nr() > 5
            && let Some(text) = self.debug_copy.take()
        {
            eprintln!("YAAC_DEBUG_COPY: copying {} characters", text.chars().count());
            ctx.copy_text(text);
        }
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
        // The window's own title strip (D104): the strip's empty part drags the window; the
        // content's widgets in it come later and win; the buttons are drawn last, on top.
        let window = match self.ui.layout_size {
            Some(sz) => egui::Rect::from_min_size(ui.max_rect().min, sz),
            None => ui.max_rect(),
        };
        crate::titlebar::drag_area(ui, window);
        if let HelloOutcome::WindowOutdated { message } = &self.client.state.hub.outcome {
            let rest = egui::Rect::from_min_max(egui::pos2(window.left(), window.top() + crate::titlebar::HEIGHT), window.max);
            ui.painter().rect_filled(window, 0.0, p.bg);
            ui.scope_builder(egui::UiBuilder::new().max_rect(rest), |ui| crate::first_run::update_needed(ui, &p, message));
        } else if self.ui.settings {
            crate::settings::show(ui, self, &mut actions);
        } else {
            crate::shell::show(ui, self, &map, now, &mut actions, frame);
        }
        crate::titlebar::buttons(ui, window, &p);
        crate::dialogs::mode_confirm(ui.ctx(), self, &mut actions);
        crate::dialogs::mute(ui.ctx(), self, &mut actions);
        self.apply(actions);
        self.persist_prefs();
    }
}

#[cfg(test)]
mod tests {
    use yaac_core::demo::threads;
    use yaac_core::history::{Anchor, Place};

    use super::*;

    pub(crate) fn demo_app(ctx: &egui::Context) -> YaacApp {
        let launch = crate::Launch {
            demo: Some(Scenario::Fleet),
            theme: ThemeChoice::Dark,
            shots: None,
            host_windows: false,
            #[cfg(feature = "hub")]
            attach: None,
        };
        YaacApp::from_ctx(ctx, launch)
    }

    #[test]
    fn back_and_forward_through_threads_settings_and_drafts() {
        let ctx = egui::Context::default();
        let mut app = demo_app(&ctx);
        let (orch, fix, billing) = (threads::orchestrator(), threads::fix(), threads::billing());
        app.apply(vec![Action::Open(fix.clone())]);
        // Reading somewhere in the frame decoder.
        app.ui.history.set_anchor(Some(Anchor { item: "f3".into(), offset: 7.0 }));
        app.apply(vec![Action::Open(billing.clone())]);
        app.apply(vec![Action::Settings(true)]);
        assert!(app.ui.settings);
        // Back from Settings returns to the thread that was open.
        app.apply(vec![Action::Settings(false)]);
        assert!(!app.ui.settings);
        assert_eq!(app.client.state.selected_thread(), Some(&billing));
        app.apply(vec![Action::Nav(-1)]);
        assert_eq!(app.client.state.selected_thread(), Some(&fix));
        assert_eq!(app.ui.restore, Some(Anchor { item: "f3".into(), offset: 7.0 }), "lands where the user was reading");
        // Forward is available after going back; a new place drops it.
        let exists = |p: &Place| app.place_exists(p);
        assert!(app.ui.history.can_forward(&exists));
        app.apply(vec![Action::Open(orch.clone())]);
        let exists = |p: &Place| app.place_exists(p);
        assert!(!app.ui.history.can_forward(&exists));
        // The list jumps several steps at once: back two from the orchestrator.
        app.apply(vec![Action::Nav(-2)]);
        assert_eq!(app.client.state.selected_thread(), Some(&orch));
        let exists = |p: &Place| app.place_exists(p);
        let fwd: Vec<Place> = app.ui.history.menu(true, &exists).into_iter().map(|(_, e)| e.place.clone()).collect();
        assert_eq!(fwd, [Place::Thread(fix), Place::Thread(orch)], "the skipped entries stay forward");
    }

    #[test]
    fn the_mouse_side_buttons_and_alt_arrows_go_back_and_forward() {
        let ctx = egui::Context::default();
        let mut app = demo_app(&ctx);
        let (orch, fix) = (threads::orchestrator(), threads::fix());
        app.apply(vec![Action::Open(fix.clone())]);
        assert_eq!(app.client.state.selected_thread(), Some(&fix));
        let press = |b: egui::PointerButton| egui::Event::PointerButton { pos: egui::pos2(10.0, 10.0), button: b, pressed: true, modifiers: Default::default() };
        let key = |k: Key| egui::Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::ALT };
        for (events, want) in [
            (vec![press(egui::PointerButton::Extra1)], &orch),
            (vec![press(egui::PointerButton::Extra2)], &fix),
            (vec![key(Key::ArrowLeft)], &orch),
            (vec![key(Key::ArrowRight)], &fix),
        ] {
            let input = egui::RawInput { events, ..Default::default() };
            let mut actions = Vec::new();
            let mut out = ctx.run_ui(input, |ui| app.keyboard(ui.ctx(), &mut actions));
            out.textures_delta.clear();
            app.apply(actions);
            assert_eq!(app.client.state.selected_thread(), Some(want));
        }
    }

    #[derive(Debug)]
    struct Dropped(std::path::PathBuf);

    impl egui::DroppedFile for Dropped {
        fn path(&self) -> &std::path::Path {
            &self.0
        }
        fn bytes(&self) -> Result<Vec<u8>, String> {
            std::fs::read(&self.0).map_err(|e| e.to_string())
        }
    }

    /// One frame of the whole window (`App::ui`), headless.
    fn window_frame(ctx: &egui::Context, app: &mut YaacApp, input: egui::RawInput) {
        let mut frame = eframe::Frame::_new_kittest();
        let mut out = ctx.run_ui(input, |ui| eframe::App::ui(app, ui, &mut frame));
        out.textures_delta.clear();
    }

    #[test]
    fn dropping_a_file_on_the_thread_view_starts_its_upload() {
        let ctx = egui::Context::default();
        let mut app = demo_app(&ctx);
        let billing = threads::billing();
        app.apply(vec![Action::Open(billing.clone())]);
        let screen = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0)));
        window_frame(&ctx, &mut app, egui::RawInput { screen_rect: screen, ..Default::default() });
        // Hovering shows the drop target; nothing is attached yet.
        // An absolute path on this system (the window ignores anything else).
        let trace = std::env::temp_dir().join("trace.log");
        let hover = egui::HoveredFile { path: Some(trace.clone()), mime: String::new() };
        window_frame(&ctx, &mut app, egui::RawInput { screen_rect: screen, hovered_files: vec![hover], ..Default::default() });
        let d = DraftRef::Thread(billing);
        assert!(app.client.draft(&d).is_none_or(|x| x.attachments.is_empty()));
        // Dropped (over the transcript): a chip, and its upload asked of the hub at once.
        let file: egui::DroppedFileHandle = std::sync::Arc::new(Dropped(trace.clone()));
        window_frame(&ctx, &mut app, egui::RawInput { screen_rect: screen, dropped_files: vec![file], ..Default::default() });
        let draft = app.client.draft(&d).expect("a draft");
        assert_eq!(draft.attachments.len(), 1);
        assert_eq!(draft.attachments[0].name, "trace.log");
        assert!(matches!(draft.attachments[0].source, Some(yaac_core::attachments::Source::Path { ref path }) if *path == trace.to_string_lossy()));
        // The demo hub answered `hub/upload/start` with a handle: it is running.
        app.pump_demo();
        assert!(matches!(app.client.draft(&d).unwrap().attachments[0].upload, yaac_core::attachments::Upload::Running { .. }));
    }
}
