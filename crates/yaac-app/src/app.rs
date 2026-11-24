//! The window: shell layout, input, and the loop that applies hub messages.
//!
//! Rendering reads the state and records [`Action`]s; they are applied after the frame's UI
//! is built, so drawing never mutates what it is drawing from. The UI thread never blocks:
//! the hub link is polled (non-blocking), and a repaint is requested only when something
//! arrived (the link's waker), the demo is streaming, or an arrow is fading.

use std::collections::{BTreeMap, BTreeSet, HashMap};
#[cfg(feature = "hub")]
use std::sync::{Arc, Mutex};

use eframe::egui::{self, Key};
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
use yaac_core::controls::{self, ControlRowView, FastToggle, Speed};
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
    /// The sidebar's ordering (D116).
    Ordering,
    Context(ThreadKey),
    /// The menu of the sidebar's selection (D138), opened on this selected row.
    Selection(ThreadKey),
    /// A section header's menu, by section id: Archive all, Mute all (D138); a temporary
    /// project's also has Create project… and Dismiss all.
    Header(String),
    Project,
    DraftWorktree,
    Model,
    Effort,
    Speed,
    Mode,
    Agents,
    ForkDest,
    SettingsTarget,
    /// The WSL distro dropdown of Settings → Targets.
    WslDistro,
    InstanceKind,
    /// The renderer dropdown (D128).
    Renderer,
    Demo,
    /// A row's PR badge menu (D119): Create a new PR, Copy link.
    PrBadge(ThreadKey),
    /// The Move dialog's Project dropdown (D144).
    MoveProject,
    /// The Move dialog's Worktree dropdown (D144).
    MoveWorktree,
}

/// Sidebar rows being dragged to another group (D144).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DragRows {
    /// The threads: the selection when the dragged row is in one of two or more, else the row.
    pub keys: Vec<ThreadKey>,
    /// Screenshots only: draw the drag as if the pointer were over this section.
    pub force_over: Option<String>,
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
    pub keep_children: bool,
    /// A handoff's first message, typed in the dialog.
    pub message: String,
}

/// View state that is not application state.
#[derive(Debug)]
pub(crate) struct UiState {
    pub grouping: Grouping,
    pub collapsed_groups: BTreeSet<String>,
    pub search: String,
    pub muted_open: bool,
    pub commands_collapsed: BTreeSet<ThreadKey>,
    pub chat_width: f32,
    pub agents_offset: f32,
    pub ultrafast_confirm: Option<DraftRef>,
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
    pub dock_preview: crate::agents_ui::DockPreview,
    /// What the floating pill drew last frame: its icons and its "more" item (D168).
    pub pill_icons: usize,
    pub pill_more: Option<egui::Rect>,
    /// The card opens scrolled to the agents the pill hides (the pointer came from "more").
    pub card_to_hidden: bool,
    /// The docked panel's "N finished" folds that are open, by relation (D168).
    pub dock_folds_open: [bool; 3],
    /// Scroll the docked panel to its first fold once (screenshots).
    pub dock_to_fold: bool,
    pub menu: Option<Menu>,
    /// The sidebar's ordering (D116).
    pub ordering: yaac_core::sidebar::Ordering,
    /// Holds a reorder back while the pointer is over the list (D116).
    pub steady: yaac_core::sidebar::Steady,
    /// The sidebar list's rectangle last frame (for the hold).
    pub list_rect: Option<egui::Rect>,
    /// The Needs-you button's cycle (D117).
    pub needs_cycle: yaac_core::sidebar::NeedsCycle,
    /// The Delete confirmation (D110).
    pub delete: Option<yaac_core::thread_menu::DeleteDialog>,
    /// The Archive confirmation (D111).
    pub archive: Option<yaac_core::thread_menu::ArchiveDialog>,
    /// **Update now**'s confirmation, while turns would be interrupted.
    pub update_now: Option<yaac_core::components::UpdateNowDialog>,
    /// The bulk Archive confirmation (D138).
    pub bulk_archive: Option<yaac_core::bulk::BulkArchiveDialog>,
    /// The Move dialog (D144).
    pub move_dialog: Option<yaac_core::moving::MoveDialog>,
    /// Rows being dragged to another group (D144).
    pub drag: Option<DragRows>,
    /// Each section of the sidebar drawn last frame and the area it covers (header and rows),
    /// for dropping dragged rows on it.
    pub drop_zones: Vec<(String, egui::Rect)>,
    /// Section headers drawn last frame (tests find a group's header by it).
    pub list_heads: Vec<(String, egui::Rect)>,
    /// The sidebar's multi-selection (D138), window-local.
    pub list_sel: yaac_core::bulk::ListSelection,
    /// Every listed sidebar row in display order, as last drawn.
    pub list_order: Vec<ThreadKey>,
    /// The sidebar rows drawn last frame and where (tests find rows by it).
    pub list_rows: Vec<(ThreadKey, egui::Rect)>,
    /// A header's menu was drawn this frame (a header can be drawn twice: in place and sticky).
    pub header_menu_drawn: bool,
    /// Keyboard focus is in the sidebar list (a row was clicked last): Esc, Ctrl+A and
    /// Shift+↑/↓ act on the selection.
    pub list_focus: bool,
    /// The `+N` list of a PR badge while it is open, and where it was drawn (D119).
    pub pr_more: Option<((ThreadKey, String), egui::Rect)>,
    /// The threads whose rows were drawn this frame (their pull requests are kept fresh).
    pub pr_visible: Vec<ThreadKey>,
    /// The last WPF pull request text edit acknowledged by the controller.
    pub wpf_pr_edit: u64,
    /// Screenshots only: show this sidebar row's `+N` list as if hovered.
    pub pr_more_force: Option<ThreadKey>,
    /// Screenshots only: show this account's rate-limit tooltip as if hovered (D161).
    pub limits_tip_force: Option<String>,
    /// Focus this request card's first button on the next frame.
    pub focus_card: Option<ItemId>,
    /// Scroll the transcript to this item on the next frame.
    pub scroll_to: Option<ItemId>,
    /// Scroll the transcript to the end on the next frame.
    pub scroll_end: bool,
    /// The open thread is scrolled up from its newest message (D142): which thread, and how
    /// many messages it had when the view left the bottom.
    pub away_from_end: Option<(ThreadKey, usize)>,
    /// The open thread's transcript ended its last pass scrolled to the very end: which
    /// thread, and the height of its view. The next pass starts at the end of its own layout
    /// (D152) instead of where egui's stick-to-bottom left it one pass ago.
    pub at_end: Option<(ThreadKey, f32)>,
    /// The open thread's transcript ended its last pass scrolled up: which thread, the block
    /// at the top of its view (by identity) and the view's top relative to that block's. The
    /// next pass keeps that block where it was on screen, whatever was measured above it
    /// (D163, P250).
    pub scroll_anchor: Option<(ThreadKey, u64, f32)>,
    /// An input method is composing in the composer.
    pub ime_composing: bool,
    /// The skill picker over the composer (`/` and `$`).
    pub(crate) skills: crate::skills_ui::Picker,
    /// The same picker over the WPF composer.
    pub(crate) wpf_skills: crate::skills_ui::WpfSkills,
    pub settings: bool,
    pub settings_page: Page,
    pub theme: ThemeChoice,
    pub font_size: f32,
    pub send_key: SendKey,
    /// D127: closing the window hides it to the tray.
    pub tray_on_close: bool,
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
    /// Screenshots only: the pointer rests here (a hovered menu row).
    pub shot_pointer: Option<egui::Pos2>,
    /// Screenshots only: lay the shell out in this size instead of the window's.
    pub layout_size: Option<egui::Vec2>,
    /// The settings pages' own state.
    pub set: crate::settings::SettingsUi,
    /// Transcript block heights as last drawn, keyed by block identity, not content (D152, P200).
    pub heights: HashMap<u64, f32>,
    /// Guessed heights of transcript blocks not drawn yet, by block identity: the item's
    /// revision and the width each was guessed for (D163, P251).
    pub guesses: HashMap<u64, (u64, f32, f32)>,
    /// Tests only: the transcript's geometry, one entry per pass.
    #[cfg(test)]
    pub geom: Vec<crate::thread::Geom>,
    /// Tests only: guess every unmeasured message block at a flat 60 points, as before D163.
    #[cfg(test)]
    pub flat_guesses: bool,
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
    /// The Commander, while open (D130).
    pub commander: Option<crate::commander::CommanderUi>,
    /// The key bindings in effect (D131).
    pub bindings: yaac_core::commands::Bindings,
    /// The Commander's recently used commands, most recent first.
    pub recent: Vec<String>,
    /// The request card whose button has keyboard focus (set while drawing it).
    pub card_focus: Option<ItemId>,
    /// "Quit YAAC?" is asked (Quit with the tray off).
    pub quit_confirm: bool,
    /// Finished turns whose work the user opened again (D156).
    pub worked: yaac_core::turns::Opened,
    /// The timeline beside the transcript (D153).
    pub timeline: crate::timeline::TimelineUi,
}

impl UiState {
    fn new(theme: ThemeChoice) -> Self {
        Self {
            grouping: Grouping::default(),
            collapsed_groups: BTreeSet::new(),
            search: String::new(),
            muted_open: false,
            commands_collapsed: BTreeSet::new(),
            chat_width: 860.0,
            agents_offset: 0.0,
            ultrafast_confirm: None,
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
            dock_preview: crate::agents_ui::DockPreview::default(),
            pill_icons: 0,
            pill_more: None,
            card_to_hidden: false,
            dock_folds_open: [false; 3],
            dock_to_fold: false,
            menu: None,
            ordering: yaac_core::sidebar::Ordering::default(),
            steady: yaac_core::sidebar::Steady::default(),
            list_rect: None,
            needs_cycle: yaac_core::sidebar::NeedsCycle::default(),
            delete: None,
            archive: None,
            update_now: None,
            bulk_archive: None,
            move_dialog: None,
            drag: None,
            drop_zones: Vec::new(),
            list_heads: Vec::new(),
            list_sel: yaac_core::bulk::ListSelection::default(),
            list_order: Vec::new(),
            list_rows: Vec::new(),
            header_menu_drawn: false,
            list_focus: false,
            pr_more: None,
            pr_visible: Vec::new(),
            wpf_pr_edit: 0,
            pr_more_force: None,
            limits_tip_force: None,
            focus_card: None,
            scroll_to: None,
            scroll_end: false,
            away_from_end: None,
            at_end: None,
            scroll_anchor: None,
            ime_composing: false,
            skills: Default::default(),
            wpf_skills: Default::default(),
            settings: false,
            settings_page: Page::Targets,
            theme,
            font_size: theme::size::UI,
            send_key: SendKey::Enter,
            tray_on_close: true,
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
            shot_pointer: None,
            layout_size: None,
            set: crate::settings::SettingsUi::default(),
            heights: HashMap::new(),
            guesses: HashMap::new(),
            #[cfg(test)]
            geom: Vec::new(),
            #[cfg(test)]
            flat_guesses: false,
            textsel: crate::seltext::TextSel::default(),
            diffs: crate::thread::DiffCache::default(),
            history: yaac_core::history::History::default(),
            restore: None,
            nav_menu: None,
            nav_sel: 0,
            nav_held: false,
            commander: None,
            bindings: yaac_core::commands::Bindings::default(),
            recent: Vec::new(),
            card_focus: None,
            quit_confirm: false,
            worked: yaac_core::turns::Opened::default(),
            timeline: crate::timeline::TimelineUi::default(),
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
            collapsed_groups: self.collapsed_groups.clone(),
            ordering: self.ordering,
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
            tray_on_close: self.tray_on_close,
            bindings: self.bindings.overrides.clone(),
            recent: self.recent.clone(),
        }
    }

    #[cfg_attr(not(feature = "hub"), allow(dead_code))]
    fn load_prefs(&mut self, p: &UiPrefs) {
        self.grouping = p.grouping;
        self.collapsed_groups = p.collapsed_groups.clone();
        self.ordering = p.ordering;
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
        self.tray_on_close = p.tray_on_close;
        self.bindings.overrides = p.bindings.clone();
        self.recent = p.recent.clone();
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
    /// Changes only when a folder-picker action replaces text in the WPF fields.
    pub field_revision: u32,
}

/// What the user did this frame.
#[derive(Clone, Debug)]
pub(crate) enum Action {
    OpenImage(crate::images::ImageSource, String),
    OlderHistory(ThreadKey),
    NewerHistory(ThreadKey),
    FocusHistory(ThreadKey, ItemId),
    LatestHistory(ThreadKey),
    Open(ThreadKey),
    /// After the Commander or a key opened a thread: the caret goes to its composer when the
    /// user can type there (D149).
    TypeHere,
    OpenAt(ThreadKey, Option<ItemId>),
    OpenRequest(ThreadKey),
    Respond { key: ThreadKey, request: RequestId, response: RequestResponse, summary: String },
    Send(ThreadKey),
    /// Send now while a turn runs: steered or interrupting (D166).
    SendNow(ThreadKey),
    /// Open a queued message in the composer for editing.
    QueueEdit(ThreadKey, String),
    /// Leave the edit without saving.
    QueueEditEnd(ThreadKey),
    /// Cancel a queued message.
    ToggleCommands(ThreadKey),
    CommandLogs(ThreadKey, String),
    CommandStop(ThreadKey, String),
    QueueCancel(ThreadKey, String),
    /// Send a queued message now.
    QueuePromote(ThreadKey, String),
    /// A message that was not sent (D165): its text back in the composer, or sent again under
    /// a new op id, or (an unknown outcome) its bubble dismissed.
    EditSend(ThreadKey, ItemId),
    RetrySend(ThreadKey, ItemId),
    DismissSend(ThreadKey, ItemId),
    DismissError(u64, Option<ThreadKey>),
    Stop(ThreadKey),
    StartDraft(u64),
    Compact(ThreadKey),
    NewThread,
    StartFirstThread,
    NewAt {
        target: TargetId,
        project: Option<String>,
        worktree: Option<String>,
        folder: Option<String>,
    },
    /// A draft in this thread's project and target, with its harness and model.
    NewInProject(ThreadKey),
    CheckLatest(ThreadKey),
    /// **Take over** a thread another program holds (D140).
    TakeOver(ThreadKey),
    ContinueBudget(Address),
    OpenFork(ThreadKey),
    /// Fork; a handoff carries its first message.
    Fork(ThreadKey, ForkChoice, Option<String>, bool),
    SetMuted(ThreadKey, bool),
    /// Mute until a moment (`None`: no end), from the Mute dialog.
    MuteUntil(ThreadKey, Option<i64>),
    /// Open the Mute dialog (`true`: "Change…", prefilled with the current mute).
    OpenMute(ThreadKey, bool),
    Unarchive(ThreadKey),
    /// Ask before archiving (D111).
    OpenArchive(ThreadKey),
    /// Ask before deleting (D110).
    OpenDelete(ThreadKey),
    /// Delete; `true`: the harness's session too.
    Delete(ThreadKey, bool),
    /// The Needs-you button or Ctrl+J (D117): open the next thread that needs the user.
    NeedsNext,
    Dismiss(ThreadKey, bool),
    SetPinned(ThreadKey, bool),
    CreateProject(CreateProject),
    DraftProject(u64, String),
    DraftWorktree(u64, String),
    Archive(ThreadKey),
    /// Ask once before archiving several threads (D138); they are in display order.
    OpenBulkArchive(Vec<ThreadKey>),
    /// Archive these (the confirmation's eligible threads).
    BulkArchive(Vec<ThreadKey>),
    /// Open the Mute dialog once for several threads (D138).
    OpenBulkMute(Vec<ThreadKey>),
    /// Mute several threads until a moment (`None`: no end), in one `hub/view/setMany`.
    MuteMany(Vec<ThreadKey>, Option<i64>),
    /// Unmute several threads, in one `hub/view/setMany`.
    UnmuteMany(Vec<ThreadKey>),
    /// Clear the related agents' unseen marks in one hub update.
    MarkRead(Vec<ThreadKey>),
    /// Unselect every sidebar row.
    ClearSelection,
    /// Open the Move dialog for these threads (D144), with a destination already chosen (a
    /// Commander choice, or a drop that changes a working directory).
    OpenMove(Vec<ThreadKey>, Option<yaac_core::moving::Destination>),
    /// Move these threads; with a notice, the bar shows it with Undo.
    Move(Vec<(ThreadKey, yaac_core::moving::Destination)>, Option<yaac_core::moving::MoveNotice>),
    /// The bar's Undo after a move.
    UndoMove,
    /// Close the bar's move notice.
    DismissMoveNotice,
    Rename(ThreadKey, String),
    SetModel(ThreadKey, yaac_protocol::ModelId),
    SetEffort(ThreadKey, yaac_protocol::EffortId),
    /// Fast mode on or off for a thread's next turns (D125).
    SetFast(ThreadKey, bool),
    ChooseSpeed(DraftRef, Speed),
    ConfirmUltrafast,
    ChooseMode(ModeTarget, ModeId),
    ConfirmMode,
    CancelMode,
    SetAgents(ThreadKey, AgentsPolicy),
    DraftModel(u64, InstanceId, yaac_protocol::ModelId),
    DraftEffort(u64, yaac_protocol::EffortId),
    /// Fast mode on or off for a new-thread draft (D125).
    DraftFast(u64, bool),
    Retry(TargetId),
    /// **Update now** on a target's older `yaacd`: asks first when turns would be interrupted.
    UpdateDaemon(TargetId),
    /// The confirmation's Update now.
    ConfirmUpdateDaemon(TargetId),
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
    /// Ask the hub for the installed WSL distros again (`hub/wsl/distros`).
    LoadDistros,
    RefreshProviders(TargetId, Option<InstanceId>),
    /// Target, kind, name, socket, harness home (P300; empty: the default).
    AddInstance(TargetId, ProviderKind, String, String, String),
    /// Set an instance's harness home (P300); empty: back to the default.
    InstanceHome(TargetId, InstanceId, String),
    /// Confirm the waiting harness-home change to another store (P300).
    ConfirmInstanceHome,
    /// Keep the current harness home.
    CancelInstanceHome,
    UpdateInstance(TargetId, InstanceId, String, String),
    SessionDiscovery(TargetId, bool),
    InstanceEnabled(TargetId, InstanceId, bool),
    RemoveInstance(TargetId, InstanceId),
    OutsideTools(TargetId, InstanceId, bool),
    TrustFolders(TargetId, InstanceId, bool),
    ModelBestUse(TargetId, InstanceId, yaac_protocol::ModelId, yaac_protocol::ModelId, Option<String>),
    SetAlways(bool),
    ClearCache,
    FsList(TargetId, String),
    /// `skills/list` for a composer's picker; `true`: ask again (Retry).
    Skills(yaac_core::skills::SkillScopeKey, bool),
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
    /// Open a URL in the host's default browser (a pull request, D119).
    OpenUrl(String),
    /// Open the Create PR modal for a thread (D120).
    OpenCreatePr(ThreadKey),
    /// The modal's Create.
    CreatePr,
    /// The modal's "Ask the agent to write it".
    AskAgentPr,
    /// The modal's target branch.
    PrBase(String),
    /// A PR badge's menu was opened: ask the target again.
    PrRefresh(ThreadKey),
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
    /// The SSH setup backend's requests and answers (`hub/ssh/*`).
    ssh: std::rc::Rc<std::cell::RefCell<crate::ssh_hub::SshBridge>>,
}

/// The SSH setup backend of a window attached to no hub (or built without one): SSH setup is
/// the hub's (it owns every connection to a target), so every step fails with this message
/// rather than opening an SSH session from the window.
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
        yaac_core::ssh_setup::StepError::new("SSH setup needs the hub, and this window is not attached to one.")
    }
}

/// What construction needs from eframe's creation context.
struct Cc {
    egui_ctx: egui::Context,
}

/// The window.
pub(crate) struct YaacApp {
    pub client: Client,
    pub browsers: crate::preview_controller::Browsers,
    pub preview_ui: crate::preview_ui::Renderer,
    /// Agent browser calls being carried out on this window's tabs.
    pub browser_agent: crate::preview_agent::Agent,
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
    pub images: crate::images::Images,
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
    /// The tray and what closing does (D127).
    pub shell: crate::tray::Shell,
    /// The renderer in use and the stored choice (D128).
    pub display: crate::display::DisplayState,
    /// The in-app updater (D136): the title row's update item and Components' release row.
    pub updater: crate::updater::Updater,
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

/// Points one mouse-wheel line scrolls (egui's default is 40): about five lines of transcript
/// text per notch (D139).
pub(crate) const LINE_SCROLL_SPEED: f32 = 100.0;

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
    let mut c = Client::new(Box::new(SharedAttach(attach.clone())), crate::BUILD, ClientId::new("window"));
    c.set_run(window_run());
    c
}

/// A part of operation ids unique to this window run (D159): the start time in microseconds
/// and the process id, in base 36. `yaacd` remembers operation ids, so a window that restarted
/// its count at 1 had its new messages refused ("was used for thread/archive") or, worse,
/// answered with an earlier request's outcome.
pub(crate) fn window_run() -> String {
    let micros = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_micros());
    let mut n = micros ^ (u128::from(std::process::id()) << 64);
    let mut s = Vec::new();
    while n > 0 {
        let d = u8::try_from(n % 36).unwrap_or(0);
        s.push(if d < 10 { b'0' + d } else { b'a' + d - 10 });
        n /= 36;
    }
    s.reverse();
    String::from_utf8(s).unwrap_or_default()
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
        // One wheel notch moves 40 points by egui's default, too little for a transcript or a
        // long thread list (owner, D139). Touchpads report pixels and are not affected.
        cc.egui_ctx.options_mut(|o| o.input_options.line_scroll_speed = LINE_SCROLL_SPEED);
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
                    let ssh = std::rc::Rc::new(std::cell::RefCell::new(crate::ssh_hub::SshBridge::new(cc.egui_ctx.clone())));
                    let backend = crate::ssh_hub::HubSsh(ssh.clone());
                    hub = Some(HubConn { attach, config, shutdown_sent: false, restarted: false, prefs_loaded: false, ssh });
                    (c, None, Scenario::Fleet, Clock::System, Box::new(backend) as Box<dyn SshSetupBackend>)
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
        let mut shell = launch.shell;
        shell.bind(&cc.egui_ctx);
        if launch.tray {
            shell.install_tray(&cc.egui_ctx);
        }
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
            images: crate::images::Images::new(&cc.egui_ctx),
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
            shell,
            display: launch.display,
            browsers: Default::default(),
            preview_ui: Default::default(),
            browser_agent: Default::default(),
            updater: launch.updater,
            ctx: cc.egui_ctx,
        };
        // The release check, on a worker (nothing when the updater is off).
        app.updater.start(&app.ctx);
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
        self.browsers = Default::default();
        self.preview_ui = Default::default();
        self.browser_agent = Default::default();
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

    /// Settings → Targets with its WSL section came into view: ask the hub for the installed
    /// distros (the window never runs `wsl.exe` itself).
    fn wsl_section_opened(&mut self) {
        if self.ui.settings && self.ui.settings_page == Page::Targets && self.host_windows && self.client.state.wsl_on {
            self.client.load_wsl_distros();
        }
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
        self.client.load_history_focus(key);
        self.ui.restore = None;
        self.ui.scroll_to = item;
        if self.ui.scroll_to.is_none() {
            self.ui.scroll_end = true;
        }
        self.ui.focus_card = self.pending_card(key);
    }

    /// The caret goes to the open thread's composer (D149, P171) — only when it can be typed in:
    /// shown (not read-only, not on a stopped distro), not dimmed by a request waiting for an
    /// answer, and no card has the keyboard.
    fn type_here(&mut self) {
        if self.ui.settings || self.ui.focus_card.is_some() {
            return;
        }
        let Some(key) = self.client.state.selected_thread().cloned() else { return };
        let map = yaac_core::attention::AttentionMap::compute(&self.client.state);
        if let yaac_core::header::ComposerView::Shown { dimmed_hint: None, .. } = yaac_core::header::composer(&self.client.state, &map, &key) {
            self.ui.focus_composer = true;
        }
    }

    fn pending_card(&self, key: &ThreadKey) -> Option<ItemId> {
        if !self.client.state.may_answer(key) {
            return None;
        }
        let t = self.client.state.transcripts.get(key)?;
        t.items().iter().rev().find_map(|i| match &i.body {
            yaac_protocol::ItemBody::Request(r) if r.state == yaac_protocol::RequestState::Pending => Some(i.id.clone()),
            yaac_protocol::ItemBody::Notice(n)
                if n.notice_kind == yaac_protocol::NoticeKind::BudgetExhausted
                    && yaac_core::agents::root_of(&self.client.state, key)
                        .and_then(|r| self.client.state.key_of(&r))
                        // Only while it holds the thread back (D164): a running thread keeps Enter.
                        .is_some_and(|k| yaac_core::attention::budget_mark(&self.client.state, &k) == Some(yaac_core::attention::BudgetMark::Paused)) =>
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

    pub(crate) fn new_thread(&mut self) {
        let id = self.client.blank_thread();
        self.show_new_draft(id);
    }

    /// The welcome screen starts on the recommended ready target.
    pub(crate) fn start_first_thread(&mut self) {
        let st = &self.client.state;
        if let Some(target) = yaac_core::first_run::preferred_start_target(st).cloned() {
            self.new_thread_at(&target, None, None, None);
        }
    }

    fn new_thread_at(&mut self, target: &TargetId, project: Option<&str>, worktree: Option<&str>, folder: Option<&str>) {
        let project = project.or_else(|| {
            if folder.is_some() {
                return None;
            }
            self.client.state.projects.iter().find(|p| &p.target == target).map(|p| p.id.as_str())
        }).map(str::to_owned);
        let id = self.client.new_thread(target.clone(), None);
        match project {
            Some(p) => {
                self.client.set_draft_project(id, &p);
                if let Some(root) = worktree {
                    self.client.set_draft_worktree(id, root);
                }
            }
            None => {
                let mut cp = crate::sidebar::create_project_for(self, target, folder.unwrap_or("~"));
                cp.for_draft = Some(id);
                self.ui.create_project = Some(cp);
            }
        }
        self.show_new_draft(id);
    }

    fn show_new_draft(&mut self, id: u64) {
        self.ui.settings = false;
        self.ui.threads_overlay = false;
        self.ui.focus_composer = true;
        self.ui.history.visit(Place::Draft(id), None);
    }

    pub(crate) fn create_project_for_draft(&mut self, id: u64, target: &TargetId) {
        let state = &self.client.state;
        if state.target(target).is_none() {
            return;
        }
        let Some(d) = state.new_threads.iter().find(|d| d.id == id) else { return };
        let folder = d.cwd.as_ref().filter(|_| d.target.as_ref() == Some(target)).map_or("~", |p| p.as_str());
        let mut project = crate::sidebar::create_project_for(self, target, folder);
        project.for_draft = Some(id);
        self.ui.create_project = Some(project);
    }

    /// "New thread in this project": a draft beside the open thread, with the same harness and
    /// model (the draft's model picker can still change both).
    pub(crate) fn new_in_project(&mut self, key: &ThreadKey) {
        let instance_model = self.client.state.summary(key).map(|s| (s.instance.clone(), s.model.clone()));
        let project = self.client.state.project_of(key).map(|p| p.id.clone());
        self.new_thread_at(&key.target, project.as_deref(), None, None);
        let yaac_core::state::Selection::NewThread(id) = self.client.state.selection else { return };
        if let Some((inst, Some(model))) = instance_model {
            self.client.set_draft_model(id, inst, model);
        }
    }

    pub(crate) fn install_update(&mut self) -> Result<(), String> {
        self.updater.install()?;
        self.shell.quit(&self.ctx);
        Ok(())
    }

    pub(crate) fn quit_window(&mut self) {
        self.shell.quit(&self.ctx);
    }

    pub(crate) fn apply(&mut self, actions: Vec<Action>) {
        for a in actions {
            match a {
                Action::OpenImage(source, name) => self.images.open(source, name),
                Action::OlderHistory(k) => self.client.older_history(&k),
                Action::NewerHistory(k) => self.client.newer_history(&k),
                Action::FocusHistory(k, item) => self.client.focus_history_item(&k, item),
                Action::LatestHistory(k) => {
                    self.client.latest_history(&k);
                    self.ui.scroll_end = true;
                    self.ui.scroll_anchor = None;
                    self.ui.scroll_to = None;
                    self.ui.restore = None;
                }
                Action::Open(k) => self.open(&k, None),
                Action::TypeHere => self.type_here(),
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
                Action::SendNow(k) => {
                    if self.client.send_now(&k) {
                        self.ui.scroll_end = true;
                    }
                }
                Action::QueueEdit(k, q) => {
                    self.client.begin_queue_edit(&k, &q);
                    self.ui.focus_composer = true;
                }
                Action::QueueEditEnd(k) => {
                    self.client.end_queue_edit(&k);
                    self.ui.focus_composer = true;
                }
                Action::ToggleCommands(key) => {
                    if !self.ui.commands_collapsed.remove(&key) {
                        self.ui.commands_collapsed.insert(key);
                    }
                }
                Action::CommandLogs(key, id) => self.client.toggle_command_log(&key, &id),
                Action::CommandStop(key, id) => self.client.stop_command(&key, &id),
                Action::QueueCancel(k, q) => self.client.queue_cancel(&k, &q),
                Action::QueuePromote(k, q) => {
                    self.client.queue_promote(&k, &q);
                    self.ui.scroll_end = true;
                }
                Action::EditSend(k, item) => {
                    if self.client.edit_send(&k, &item) {
                        self.ui.focus_composer = true;
                    }
                }
                Action::RetrySend(k, item) => {
                    self.client.retry_send(&k, &item);
                }
                Action::DismissSend(k, item) => {
                    self.client.dismiss_send(&k, &item);
                }
                Action::DismissError(id, key) => self.client.dismiss_error(id, key.as_ref()),
                Action::Stop(k) => self.client.interrupt(&k),
                Action::Compact(k) => {
                    if self.client.compact(&k) {
                        self.ui.scroll_end = true;
                    }
                }
                Action::StartDraft(id) => {
                    self.client.start_thread(id);
                }
                Action::NewThread => self.new_thread(),
                Action::StartFirstThread => self.start_first_thread(),
                Action::NewAt { target, project, worktree, folder } => {
                    self.new_thread_at(&target, project.as_deref(), worktree.as_deref(), folder.as_deref());
                }
                Action::NewInProject(k) => self.new_in_project(&k),
                Action::CheckLatest(k) => self.client.check_latest(&k),
                Action::TakeOver(k) => self.client.take_over(&k),
                Action::ContinueBudget(r) => self.client.continue_budget(&r),
                Action::OpenFork(k) => {
                    if let Some(choice) = yaac_core::fork::default_choice(&self.client.state, &k) {
                        if self.client.state.selected_thread() != Some(&k) {
                            self.open(&k, None);
                        }
                        self.client.fork_preview(&k, &choice.instance);
                        self.ui.fork = Some(ForkDialog { key: k, choice, preview_open: true, message: String::new(), keep_children: false });
                        self.ui.menu = None;
                    }
                }
                Action::Fork(k, c, message, keep_children) => {
                    self.client.fork(&k, &c, message.as_deref(), keep_children);
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
                Action::OpenArchive(k) => {
                    self.ui.archive = yaac_core::thread_menu::ArchiveDialog::new(&self.client.state, &k);
                    self.ui.menu = None;
                }
                Action::OpenDelete(k) => {
                    self.ui.delete = yaac_core::thread_menu::DeleteDialog::new(&self.client.state, &k);
                    self.ui.menu = None;
                }
                Action::Delete(k, also) => {
                    self.client.delete(&k, also);
                    self.ui.delete = None;
                }
                Action::NeedsNext => {
                    let map = yaac_core::attention::AttentionMap::compute(&self.client.state);
                    let list = yaac_core::sidebar::needs_you(&self.client.state, &map, self.now());
                    if let Some(k) = self.ui.needs_cycle.next(&list) {
                        self.apply(vec![Action::OpenRequest(k)]);
                    }
                }
                Action::Dismiss(k, d) => self.client.set_dismissed(&k, d),
                Action::SetPinned(k, v) => self.client.set_pinned(&k, v),
                Action::CreateProject(cp) => {
                    // The draft (if any) is put in the project once the target created it.
                    self.client.create_project(&cp.target, &cp.folder, &cp.name, cp.for_draft);
                }
                Action::DraftProject(d, p) => self.client.set_draft_project(d, &p),
                Action::DraftWorktree(d, root) => self.client.set_draft_worktree(d, &root),
                Action::Archive(k) => {
                    self.client.archive(&k);
                    self.ui.archive = None;
                }
                Action::OpenBulkArchive(keys) => {
                    let now = self.now();
                    self.ui.bulk_archive = Some(yaac_core::bulk::BulkArchiveDialog::new(&self.client.state, &keys, now));
                    self.ui.menu = None;
                }
                Action::BulkArchive(keys) => {
                    self.client.archive_many(&keys);
                    self.ui.bulk_archive = None;
                    self.ui.list_sel.clear();
                }
                Action::OpenBulkMute(keys) => {
                    let now = self.now();
                    let plan = yaac_core::bulk::plan(&self.client.state, &keys, yaac_core::bulk::BulkKind::Mute, now);
                    self.ui.mute = yaac_core::mute::MuteDialog::many(plan.eligible, now, self.zone);
                    self.ui.menu = None;
                }
                Action::MuteMany(keys, until) => {
                    self.client.mute_many(&keys, until);
                    self.ui.mute = None;
                    self.ui.list_sel.clear();
                }
                Action::UnmuteMany(keys) => {
                    let now = self.now();
                    let plan = yaac_core::bulk::plan(&self.client.state, &keys, yaac_core::bulk::BulkKind::Unmute, now);
                    self.client.unmute_many(&plan.eligible);
                    self.ui.menu = None;
                    self.ui.list_sel.clear();
                }
                Action::MarkRead(keys) => {
                    let changes: Vec<_> = keys.into_iter().collect::<BTreeSet<_>>().into_iter()
                        .map(|k| (k, yaac_core::state::ViewPatch { unseen: Some(false), ..Default::default() })).collect();
                    self.client.set_view_many(&changes);
                }
                Action::ClearSelection => {
                    self.ui.list_sel.clear();
                    self.ui.menu = None;
                }
                Action::OpenMove(keys, dest) => {
                    let mut d = yaac_core::moving::MoveDialog::new(&self.client.state, &keys);
                    if let Some(to) = dest
                        && d.blocked.is_none()
                    {
                        d.set_project(&to.project);
                        d.worktree = to.worktree;
                    }
                    self.ui.move_dialog = Some(d);
                    self.ui.menu = None;
                }
                Action::Move(moves, notice) => {
                    let moves: Vec<_> = moves.into_iter().map(|(k, d)| (k, Some(d))).collect();
                    self.client.move_threads(&moves, notice);
                    self.ui.move_dialog = None;
                    self.ui.list_sel.clear();
                }
                Action::UndoMove => self.client.undo_move(),
                Action::DismissMoveNotice => self.client.move_notice = None,
                Action::Rename(k, t) => self.client.rename(&k, &t),
                Action::SetModel(k, m) => self.client.set_next_model(&k, m),
                Action::SetFast(k, on) => self.client.set_fast(&k, on),
                Action::DraftFast(id, on) => self.client.set_draft_fast(id, on),
                Action::ChooseSpeed(draft, speed) => {
                    if self.composer_speed(&draft).is_some_and(|control| control.options().any(|s| s == speed) && control.current() != speed) {
                        let on = speed == Speed::Fast;
                        match (speed, draft) {
                            (Speed::Ultrafast, draft) => self.ui.ultrafast_confirm = Some(draft),
                            (_, DraftRef::Thread(key)) => self.client.set_fast(&key, on),
                            (_, DraftRef::New(id)) => self.client.set_draft_fast(id, on),
                        }
                    }
                }
                Action::ConfirmUltrafast => {
                    if let Some(draft) = self.ui.ultrafast_confirm.take()
                        && self.composer_speed(&draft).is_some_and(|control| control.offers_ultrafast)
                    {
                        match draft {
                            DraftRef::Thread(key) => self.client.set_ultrafast(&key),
                            DraftRef::New(id) => self.client.set_draft_ultrafast(id),
                        }
                    }
                }
                Action::SetEffort(k, e) => {
                    self.client.state.next_settings.entry(k).or_default().effort = Some(e);
                }
                Action::ChooseMode(t, m) => self.client.choose_mode(t, m),
                Action::ConfirmMode => self.client.confirm_mode(),
                Action::CancelMode => self.client.cancel_mode(),
                Action::SetAgents(k, pol) => self.client.set_agents_policy(&k, pol),
                Action::DraftModel(id, inst, model) => self.client.set_draft_model(id, inst, model),
                Action::DraftEffort(id, e) => {
                    if let Some(d) = self.client.state.new_threads.iter_mut().find(|d| d.id == id) {
                        d.effort = Some(e);
                    }
                    self.client.draft_changed(&DraftRef::New(id));
                }
                Action::Retry(t) => self.client.retry_target(&t),
                Action::UpdateDaemon(t) => match yaac_core::components::update_now_dialog(&self.client.state, &t) {
                    Some(d) => self.ui.update_now = Some(d),
                    None => self.client.update_daemon(&t),
                },
                Action::ConfirmUpdateDaemon(t) => {
                    self.ui.update_now = None;
                    self.client.update_daemon(&t);
                }
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
                        self.wsl_section_opened();
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
                    self.wsl_section_opened();
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
                    // With a hub, its records win; it only needs to hear what the user did by hand.
                    #[cfg(feature = "hub")]
                    if self.hub.is_some() && matches!(r, SetupRecord::RemovedByHand { .. }) {
                        self.client.ssh_removed_by_hand(&t);
                    }
                    self.client.state.ssh_records.insert(t, r);
                }
                Action::WslOn(on) => {
                    self.client.state.wsl_on = on;
                    self.wsl_section_opened();
                }
                Action::LoadDistros => self.client.load_wsl_distros(),
                Action::RefreshProviders(t, i) => self.client.refresh_providers(&t, i.as_ref()),
                Action::AddInstance(t, kind, name, socket, home) => self.client.add_instance(&t, &kind, Some(&name), Some(&socket), Some(&home)),
                Action::InstanceHome(t, i, home) => self.client.set_instance_home(&t, &i, Some(&home), false),
                Action::ConfirmInstanceHome => self.client.confirm_instance_home(),
                Action::CancelInstanceHome => self.client.cancel_instance_home(),
                Action::UpdateInstance(t, i, name, socket) => {
                    let name = name.trim().to_owned();
                    let socket = socket.trim().to_owned();
                    self.client.update_instance(&t, &i, Some((!name.is_empty()).then_some(name.as_str())), Some((!socket.is_empty()).then_some(socket.as_str())), None);
                }
                Action::SessionDiscovery(t, enabled) => self.client.set_session_discovery(&t, enabled),
                Action::InstanceEnabled(t, i, e) => self.client.update_instance(&t, &i, None, None, Some(e)),
                Action::RemoveInstance(t, i) => self.client.remove_instance(&t, &i),
                Action::OutsideTools(t, i, on) => self.client.set_outside_tools(&t, &i, on),
                Action::TrustFolders(t, i, on) => self.client.set_trust_folders(&t, &i, on),
                Action::ModelBestUse(target, instance, model, expected, summary) => {
                    self.client.set_model_best_use(&target, instance, model, expected, summary);
                }
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
                Action::Skills(scope, force) => self.client.skills_list(&scope, force),
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
                Action::OpenUrl(url) => {
                    // In the demo and while taking screenshots nothing leaves the window.
                    if self.demo.is_none() {
                        crate::browser::open(&url);
                    }
                }
                Action::OpenCreatePr(k) => {
                    self.ui.menu = None;
                    if self.client.state.create_pr.is_none() {
                        self.ui.wpf_pr_edit = 0;
                        self.client.open_create_pr(&k);
                    }
                }
                Action::CreatePr => {
                    self.client.submit_create_pr();
                }
                Action::AskAgentPr => {
                    self.client.ask_agent_for_pr();
                }
                Action::PrBase(b) => self.client.set_create_pr_base(&b),
                Action::PrRefresh(k) => {
                    let now = self.now();
                    self.client.forge_refresh(&k, now);
                }
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
                    self.browsers = Default::default();
                    self.preview_ui = Default::default();
                    self.browser_agent = Default::default();
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
                self.browsers = Default::default();
                self.preview_ui = Default::default();
                self.browser_agent = Default::default();
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
    pub(crate) fn persist_prefs(&mut self) {
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
    pub(crate) fn composer_speed(&self, draft: &DraftRef) -> Option<FastToggle> {
        if self.open_composer().as_ref() != Some(draft) {
            return None;
        }
        match draft {
            DraftRef::Thread(key) => match controls::thread_row(&self.client.state, key) {
                ControlRowView::Row(row) => row.fast,
                ControlRowView::Hidden => None,
            },
            DraftRef::New(id) => controls::draft_row_of(&self.client.state, *id).and_then(|row| row.fast),
        }
    }

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
    pub(crate) fn paste(&mut self, ctx: &egui::Context) {
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

    /// The keyboard: one dispatcher, through the command table (D131).
    pub(crate) fn keyboard(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        crate::commander::dispatch(self, ctx, actions);
    }
}

impl YaacApp {
    pub(crate) fn tick_running_commands(&mut self, ctx: &egui::Context, key: &ThreadKey) {
        if self.ui.commands_collapsed.contains(key) {
            return;
        }
        let now = self.now();
        self.client.refresh_command_log(key, now);
        if !yaac_core::running_commands::rows(&self.client.state, key).is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_secs(1));
        }
    }

    pub(crate) fn upkeep(&mut self, ctx: &egui::Context) {
        // Apply everything that arrived since the last frame: a burst of events becomes one
        // repaint. Arrivals from a real hub wake the window through the link's waker.
        self.updater.poll();
        let arrived = self.pump_demo();
        #[cfg(feature = "hub")]
        self.hub_upkeep();
        #[cfg(feature = "hub")]
        if let Some(h) = &self.hub {
            self.client.ssh_zone = self.zone;
            crate::ssh_hub::SshBridge::exchange(&h.ssh, &mut self.client);
        }
        let ssh_changed = self.ui.set.pump_ssh(self.ssh.as_mut());
        let t = ctx.input(|i| i.time);
        let files_changed = self.pump_files();
        self.images.pump(&mut self.client);
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
        // The tray (D127): show and quit requests, close-to-tray, the needs-you badge. This
        // runs while the window is hidden too (eframe calls only `logic` then).
        let state = &self.client.state;
        self.shell.upkeep(ctx, self.ui.tray_on_close, || {
            let map = AttentionMap::compute(state);
            yaac_core::sidebar::needs_you(state, &map, now).len()
        });
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
        if let Some(at) = self.ui.shot_pointer.filter(|_| self.shots.is_some()) {
            raw.events.push(egui::Event::PointerMoved(at));
        }
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.upkeep(ctx);
        let repaint = ctx.clone();
        self.browsers.check_runtime(move || repaint.request_repaint());
        let visible = self.client.state.selected_thread().filter(|_| !self.ui.settings && !self.shell.hidden && ctx.input(|i| i.viewport().visible() != Some(false))).cloned();
        if let Some(delay) = self.browsers.tick(&mut self.client, visible.as_ref(), std::time::Instant::now()) {
            ctx.request_repaint_after(delay);
        }
        self.preview_ui.sync(&self.browsers, visible.as_ref());
        // Agent browser calls (docs/11-preview.md#agents), on the calling thread's tabs.
        #[cfg(feature = "preview")]
        self.preview_ui.pump(&mut self.browsers, visible.as_ref(), ctx);
        #[cfg(feature = "preview")]
        let next = self.browser_agent.step(&mut self.client, &mut self.browsers, &mut self.preview_ui, std::time::Instant::now());
        #[cfg(not(feature = "preview"))]
        let next = self.browser_agent.step(&mut self.client, &mut self.browsers, std::time::Instant::now());
        if let Some(delay) = next {
            ctx.request_repaint_after(delay);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        crate::icons::set_spinner_window_hidden(&ctx, self.shell.hidden);
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
        // The renderer reached a frame: it did not crash the start (D128).
        self.display.first_frame();
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
        if self.ui.ultrafast_confirm.is_some() {
            let escape = ctx.input_mut(|i| {
                i.consume_key(egui::Modifiers::NONE, Key::Enter);
                i.consume_key(egui::Modifiers::CTRL, Key::Enter);
                i.consume_key(egui::Modifiers::NONE, Key::Escape)
            });
            if escape {
                self.ui.ultrafast_confirm = None;
            }
        } else if !self.images.is_open() && !self.preview_ui.page_focus.is_some_and(|id| ctx.memory(|m| m.has_focus(id))) {
            self.keyboard(&ctx, &mut actions);
        }
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
        crate::shell::window_error_notice(&ctx, self, window.width(), &mut actions);
        crate::titlebar::panes(ui, window, self);
        crate::titlebar::buttons(ui, window, &p);
        match crate::titlebar::update(ui, window, &p, &self.updater) {
            Some(crate::updater::CaptionAction::Download) => self.updater.download(),
            // The installer closes the window and the hub itself (`yaac quit`), so quitting
            // here only saves it the wait; it starts YAAC again when done (`/RELAUNCH`).
            Some(crate::updater::CaptionAction::Install) => {
                if let Err(e) = self.install_update() {
                    self.updater.notice = Some(e);
                }
            }
            None => {}
        }
        crate::dialogs::update_notice(ui.ctx(), self);
        crate::dialogs::mode_confirm(ui.ctx(), self, &mut actions);
        crate::dialogs::ultrafast_confirm(ui.ctx(), self, &mut actions);
        crate::dialogs::mute(ui.ctx(), self, &mut actions);
        crate::pr_ui::create_dialog(ui.ctx(), self, &mut actions);
        crate::dialogs::quit(ui.ctx(), self);
        crate::commander::show(ui.ctx(), self, &mut actions);
        self.images.show(ui.ctx());
        // Pull requests of the rows on screen (D119): asked when due, never polled; one calm
        // repaint when the next refresh is due.
        let visible = std::mem::take(&mut self.ui.pr_visible);
        if !visible.is_empty() && self.shots.is_none() {
            self.client.forge_visible(&visible, now);
            ctx.request_repaint_after(std::time::Duration::from_secs(u64::try_from(yaac_core::pr::VISIBLE_REFRESH_SECS).unwrap_or(300)));
        }
        if self.client.poll_create_pr() {
            ctx.request_repaint();
        }
        self.apply(actions);
        self.persist_prefs();
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use yaac_core::demo::threads;
    use yaac_core::history::{Anchor, Place};

    use super::*;
    use eframe::egui::Modifiers;

    pub(crate) fn demo_app(ctx: &egui::Context) -> YaacApp {
        let launch = crate::Launch {
            demo: Some(Scenario::Fleet),
            theme: ThemeChoice::Dark,
            shots: None,
            host_windows: false,
            #[cfg(feature = "hub")]
            attach: None,
            display: crate::display::DisplayState::default(),
            shell: crate::tray::Shell::new(),
            tray: false,
            updater: crate::updater::Updater::off("tests"),
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

    /// D140: the banner's **Take over** sends `thread/takeOver` with an operation id, once;
    /// when it is answered the banner goes and the composer unlocks.
    #[test]
    fn clicking_take_over_on_the_banner_sends_thread_take_over() {
        let ctx = egui::Context::default();
        let mut app = demo_app(&ctx);
        let hs = threads::handshake();
        app.apply(vec![Action::Open(hs.clone())]);
        app.pump_demo();
        assert!(!app.client.state.may_act(&hs), "held by a terminal session");
        let screen = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0)));
        for _ in 0..3 {
            window_frame(&ctx, &mut app, egui::RawInput { screen_rect: screen, ..Default::default() });
        }
        let button = ctx.read_response(egui::Id::unique(crate::thread::TAKE_OVER_BUTTON)).expect("Take over is on the banner");
        let at = button.rect.center();
        let sent = |app: &YaacApp| app.demo.as_ref().unwrap().requests().into_iter().filter(|(_, m, _)| m == "thread/takeOver").collect::<Vec<_>>();
        assert!(sent(&app).is_empty());
        for events in [
            vec![egui::Event::PointerMoved(at)],
            vec![egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }],
            vec![egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }],
        ] {
            window_frame(&ctx, &mut app, egui::RawInput { screen_rect: screen, events, ..Default::default() });
        }
        let reqs = sent(&app);
        assert_eq!(reqs.len(), 1, "one click, no confirmation");
        assert_eq!(reqs[0].2["threadId"], hs.thread.as_str());
        assert!(reqs[0].2["opId"].as_str().is_some_and(|o| !o.is_empty()), "an operation id");
        app.pump_demo();
        window_frame(&ctx, &mut app, egui::RawInput { screen_rect: screen, ..Default::default() });
        assert!(app.client.state.may_act(&hs), "the composer unlocks");
        let map = yaac_core::attention::AttentionMap::compute(&app.client.state);
        let h = yaac_core::header::header(&app.client.state, &map, &hs).unwrap();
        assert!(!h.banners.iter().any(|b| matches!(b, yaac_core::header::Banner::InUseElsewhere { .. })), "the banner goes away");
    }

    /// One frame as eframe runs it while the window is shown: `logic` (the hub's messages,
    /// the demo's streaming ticks), then `ui`, in every pass.
    fn live_frame(ctx: &egui::Context, app: &mut YaacApp, input: egui::RawInput) {
        let mut frame = eframe::Frame::_new_kittest();
        let mut out = ctx.run_ui(input, |ui| {
            eframe::App::logic(app, ui.ctx(), &mut frame);
            eframe::App::ui(app, ui, &mut frame);
        });
        out.textures_delta.clear();
    }

    /// D152: while a reply streams into a thread read at its bottom, the view only ever moves
    /// forward: one frame per delta, the content never shrinks, the offset never goes back,
    /// every frame shows the end of its own layout (the newest line stays put on screen), and
    /// a block once measured is never drawn at a guess.
    #[test]
    fn a_streaming_reply_never_moves_the_view_backwards() {
        let ctx = egui::Context::default();
        let mut app = demo_app(&ctx);
        let billing = threads::billing();
        app.apply(vec![Action::Open(billing.clone())]);
        let clock = std::cell::Cell::new(0.0_f64);
        // 40 ms apart: the demo hub releases one scheduled event per frame.
        let screen = || {
            clock.set(clock.get() + 0.04);
            egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 420.0))), time: Some(clock.get()), ..Default::default() }
        };
        for _ in 0..6 {
            live_frame(&ctx, &mut app, screen());
        }
        assert!(app.ui.away_from_end.is_none(), "opened at its bottom");
        app.apply(vec![Action::StreamReply]);
        // The pass each frame showed (an earlier one may have been discarded).
        let mut shown: Vec<crate::thread::Geom> = Vec::new();
        // Whether the turn was still running when each frame was drawn.
        let mut running = Vec::new();
        let mut passes = 0;
        for _ in 0..400 {
            app.ui.geom.clear();
            live_frame(&ctx, &mut app, screen());
            passes += app.ui.geom.len();
            shown.push(app.ui.geom.last().cloned().expect("the transcript was drawn"));
            running.push(app.client.state.transcripts[&billing].active_turn.is_some());
            if !app.demo.as_ref().unwrap().busy() && shown.len() > 3 {
                break;
            }
        }
        assert!(running.iter().filter(|r| **r).count() > 100, "the reply streamed over many frames");
        // Settled: nothing left to measure, so more frames lay out once and change nothing.
        let mut settled = Vec::new();
        for _ in 0..6 {
            app.ui.geom.clear();
            live_frame(&ctx, &mut app, screen());
            settled.push(app.ui.geom.len());
            shown.push(app.ui.geom.last().cloned().expect("drawn"));
            running.push(false);
        }
        eprintln!("passes per settled frame {settled:?}");
        assert!(settled[2..].iter().all(|n| *n == 1), "no pass discarded once settled: {settled:?}");
        let text: Vec<String> = shown
            .iter()
            .enumerate()
            .map(|(n, g)| format!("{n:3} drawn@{:7.1} end@{:7.1} content {:7.1} view {:5.1} last {:?} est {:?}", g.drawn_offset, g.offset, g.content, g.view, g.last_bottom, g.estimated))
            .collect();
        eprintln!("{} frames, {passes} passes\n{}", shown.len(), text.join("\n"));
        let mut problems = Vec::new();
        let mut measured: std::collections::BTreeSet<usize> = std::collections::BTreeSet::new();
        for (n, w) in shown.windows(2).enumerate() {
            let (a, b) = (&w[0], &w[1]);
            // While it streams. (When the turn ends its activity may fold up, once.)
            if !running[n + 1] {
                continue;
            }
            if let (Some(x), Some(y)) = (a.last_bottom, b.last_bottom)
                && (x - y).abs() > 0.5
            {
                problems.push(format!("frame {}: the newest line moved on screen {x} → {y}", n + 1));
            }
            if b.content < a.content - 0.5 {
                problems.push(format!("frame {}: content shrank {} → {}", n + 1, a.content, b.content));
            }
            if b.drawn_offset < a.drawn_offset - 0.5 {
                problems.push(format!("frame {}: offset went back {} → {}", n + 1, a.drawn_offset, b.drawn_offset));
            }
        }
        for (n, g) in shown.iter().enumerate() {
            // Stuck to the bottom: the frame shows the end of what it laid out.
            if (g.drawn_offset - (g.content - g.view).max(0.0)).abs() > 1.0 {
                problems.push(format!("frame {n}: drawn at {} but the end is at {}", g.drawn_offset, g.content - g.view));
            }
            if let Some(again) = g.estimated.iter().find(|i| measured.contains(*i)) {
                problems.push(format!("frame {n}: item {again} drawn at a guess after it was measured"));
            }
            measured.extend(g.drawn.iter().filter(|i| !g.estimated.contains(i)));
        }
        assert!(problems.is_empty(), "{} problems:\n{}", problems.len(), problems.join("\n"));
        assert!(app.ui.away_from_end.is_none(), "still at the bottom");
        let last = shown.last().unwrap();
        assert!(last.estimated.is_empty());
    }

    #[test]
    fn scrolled_up_offers_the_way_back_and_ctrl_end_takes_it() {
        let ctx = egui::Context::default();
        let mut app = demo_app(&ctx);
        let orch = threads::orchestrator();
        app.apply(vec![Action::Open(orch.clone())]);
        // Frames 100 ms apart, as a real window draws them (smooth scrolling settles).
        let clock = std::cell::Cell::new(0.0_f64);
        let screen = || {
            clock.set(clock.get() + 0.1);
            egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 300.0))), time: Some(clock.get()), ..Default::default() }
        };
        // Settle the layout at the bottom.
        for _ in 0..4 {
            window_frame(&ctx, &mut app, screen());
        }
        assert!(app.ui.away_from_end.is_none(), "at the newest message: no button");
        // The mouse wheel over the transcript scrolls it up, away from the end.
        let mut wheel = screen();
        wheel.events.push(egui::Event::PointerMoved(egui::pos2(800.0, 120.0)));
        wheel.events.push(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: egui::vec2(0.0, 3.0),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Wheel,
            modifiers: Modifiers::NONE,
        });
        window_frame(&ctx, &mut app, wheel);
        for _ in 0..20 {
            window_frame(&ctx, &mut app, screen());
        }
        assert!(matches!(&app.ui.away_from_end, Some((k, _)) if k == &orch), "scrolled up: the button is offered");
        // Ctrl+End jumps back, even with the caret in the composer.
        app.ui.focus_composer = true;
        window_frame(&ctx, &mut app, screen());
        let mut input = screen();
        input.events.push(egui::Event::Key { key: Key::End, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::CTRL });
        window_frame(&ctx, &mut app, input);
        for _ in 0..3 {
            window_frame(&ctx, &mut app, screen());
        }
        assert!(app.ui.away_from_end.is_none(), "back at the newest message");
    }

    /// The long conversation (D153) with answers of very different lengths: one line, a few
    /// paragraphs, a list, a code block.
    fn long_varied_app(ctx: &egui::Context) -> (YaacApp, ThreadKey) {
        let mut app = demo_app(ctx);
        if let Some(h) = &app.demo {
            h.play(yaac_core::demo::Scene::LongConversation);
            h.drain();
        }
        app.pump_demo();
        let key = threads::parser();
        app.open(&key, None);
        for _ in 0..20 {
            app.pump_demo();
        }
        let t = app.client.state.transcripts.get_mut(&key).expect("the parser thread");
        let rev = t.revision + 1;
        let answers: Vec<yaac_protocol::Item> = t.items().iter().filter(|i| i.id.as_str().ends_with("-a")).cloned().collect();
        let para = "The lexer now reads the input through a small buffer and hands the parser one token at a time, so a large file never has to be held in memory at once. ";
        for (n, mut item) in answers.into_iter().enumerate() {
            let text = match n % 4 {
                0 => continue,
                1 => format!("{}\n\n{}\n\n{para}", para.repeat(3), para.repeat(2)),
                2 => format!("Here is the change:\n\n```rust\n{}```\n\nAll tests pass.", "fn lex(input: &mut impl BufRead) -> Result<Token> {\n    next(input)\n}\n".repeat(6)),
                _ => format!("- {para}\n- {para}\n- {para}\n\n{}", para.repeat(4)),
            };
            item.body = yaac_protocol::ItemBody::AgentMessage(yaac_protocol::AgentMessageItem { text });
            t.upsert(item, rev);
        }
        (app, key)
    }

    /// Wheel notches up the long conversation from its bottom, then down from its top: what
    /// went wrong, and each block's guessed height against the one drawn.
    fn wheel_notches(flat_guesses: bool) -> (Vec<String>, Vec<(f32, f32)>) {
        let ctx = egui::Context::default();
        let (mut app, _key) = long_varied_app(&ctx);
        app.ui.flat_guesses = flat_guesses;
        let clock = std::cell::Cell::new(0.0_f64);
        let screen = || {
            clock.set(clock.get() + 1.0 / 60.0);
            egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 800.0))), time: Some(clock.get()), ..Default::default() }
        };
        let frame = |app: &mut YaacApp, input: egui::RawInput| {
            app.ui.geom.clear();
            window_frame(&ctx, app, input);
            app.ui.geom.last().cloned().expect("the transcript was drawn")
        };
        let mut g = frame(&mut app, screen());
        for _ in 0..10 {
            g = frame(&mut app, screen());
        }
        assert!(app.ui.away_from_end.is_none(), "opened at its bottom");
        // Where each block on screen is, by identity.
        let visible = |g: &crate::thread::Geom| -> std::collections::HashMap<u64, f32> { g.placed.iter().filter(|(_, y)| *y >= 0.0 && *y < g.view).copied().collect() };
        // How far the content moved on screen from `a` to `b` (positive: up, scrolling down).
        #[allow(clippy::cast_precision_loss, reason = "a few blocks")]
        let moved = |a: &crate::thread::Geom, b: &crate::thread::Geom| -> Option<f32> {
            let (va, vb) = (visible(a), visible(b));
            let d: Vec<f32> = va.iter().filter_map(|(id, ya)| vb.get(id).map(|yb| ya - yb)).collect();
            (!d.is_empty()).then(|| d.iter().sum::<f32>() / d.len() as f32)
        };
        // Per notch: its direction (+1 up), the offset's change, the content's movement on
        // screen, and whether it ended at either end.
        let mut notches: Vec<(f32, f32, f32, bool)> = Vec::new();
        let mut problems = Vec::new();
        let mut guessed: Vec<(f32, f32)> = Vec::new();
        for dir in [1.0_f32, -1.0] {
            for _ in 0..200 {
                let before = g.clone();
                let max_offset = (before.content - before.view).max(0.0);
                if (dir > 0.0 && before.offset <= 0.5) || (dir < 0.0 && before.offset >= max_offset - 0.5) {
                    break;
                }
                let mut input = screen();
                input.events.push(egui::Event::PointerMoved(egui::pos2(900.0, 300.0)));
                input.events.push(egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Line,
                    delta: egui::vec2(0.0, dir),
                    phase: egui::TouchPhase::Move,
                    source: egui::MouseWheelSource::Wheel,
                    modifiers: Modifiers::NONE,
                });
                let mut prev = before.clone();
                g = frame(&mut app, input);
                for n in 0..20 {
                    if let Some(m) = moved(&prev, &g)
                        && m * dir > 0.5
                    {
                        problems.push(format!("notch {} frame {n}: the content moved {m:.1} against the wheel", notches.len()));
                    }
                    prev = g.clone();
                    g = frame(&mut app, screen());
                    guessed.extend(app.ui.geom.iter().flat_map(|x| x.guessed.iter().copied()));
                }
                let clamped = g.offset <= 0.5 || g.offset >= (g.content - g.view).max(0.0) - 0.5;
                let m = moved(&before, &g).unwrap_or(f32::NAN);
                notches.push((dir, g.offset - before.offset, m, clamped));
            }
        }
        let text: Vec<String> = notches
            .iter()
            .enumerate()
            .map(|(n, (d, off, m, c))| format!("{n:3} {} offset {off:+7.1} content {m:+7.1}{}", if *d > 0.0 { "up  " } else { "down" }, if *c { " (at an end)" } else { "" }))
            .collect();
        eprintln!("{}", text.join("\n"));
        eprintln!("guessed → drawn ({}): {:?}", if flat_guesses { "flat" } else { "from the content" }, guessed.iter().map(|(a, b)| format!("{a:.0}→{b:.0}")).collect::<Vec<_>>());
        let ups = notches.iter().filter(|n| n.0 > 0.0).count();
        assert!(ups > 20 && notches.len() - ups > 20, "a long way both ways: {ups} up, {} down", notches.len() - ups);
        for (n, (d, _, m, at_end)) in notches.iter().enumerate() {
            if !at_end && (m.is_nan() || (m + d * LINE_SCROLL_SPEED).abs() > 2.0) {
                problems.push(format!("notch {n}: the content moved {m:.1}, not {:.1}", -d * LINE_SCROLL_SPEED));
            }
        }
        (problems, guessed)
    }

    /// D163: one wheel notch moves the transcript's content on screen by the same amount
    /// wherever the view is, scrolling up from the bottom or down from the top, however wrong
    /// the guess at a block's height was before it was measured; and no frame moves it the
    /// other way. Guesses from the content are near the drawn heights.
    #[test]
    fn identical_wheel_notches_move_the_transcript_by_the_same_amount() {
        let (problems, _) = wheel_notches(true);
        assert!(problems.is_empty(), "flat guesses: {} problems:\n{}", problems.len(), problems.join("\n"));
        let (problems, guessed) = wheel_notches(false);
        assert!(problems.is_empty(), "guesses from the content: {} problems:\n{}", problems.len(), problems.join("\n"));
        let off: f32 = guessed.iter().map(|(g, d)| (g - d).abs()).sum();
        let drawn: f32 = guessed.iter().map(|(_, d)| d).sum();
        assert!(guessed.len() > 30 && off < drawn * 0.1, "guesses {off} off over {drawn} drawn");
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

    /// Drags the pointer from `from` 30 points to the right over a few frames; returns
    /// whether the window was asked to start a move.
    fn drag_starts_window_move(ctx: &egui::Context, app: &mut YaacApp, from: egui::Pos2) -> bool {
        let screen = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0)));
        let mut moved = false;
        let steps: Vec<Vec<egui::Event>> = vec![
            vec![egui::Event::PointerMoved(from)],
            vec![egui::Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }],
            vec![egui::Event::PointerMoved(from + egui::vec2(15.0, 0.0))],
            vec![egui::Event::PointerMoved(from + egui::vec2(30.0, 0.0))],
            vec![egui::Event::PointerButton { pos: from + egui::vec2(30.0, 0.0), button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }],
        ];
        for events in steps {
            let mut frame = eframe::Frame::_new_kittest();
            let mut out = ctx.run_ui(egui::RawInput { screen_rect: screen, events, ..Default::default() }, |ui| eframe::App::ui(app, ui, &mut frame));
            out.textures_delta.clear();
            moved |= out.viewport_output.values().any(|v| v.commands.contains(&egui::ViewportCommand::StartDrag));
        }
        moved
    }

    #[test]
    fn the_title_row_drags_the_window_but_not_from_its_controls() {
        let ctx = egui::Context::default();
        let mut app = demo_app(&ctx);
        app.apply(vec![Action::Open(threads::billing())]);
        for _ in 0..3 {
            window_frame(&ctx, &mut app, egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0))), ..Default::default() });
        }
        // An empty part of the row, right of the thread's title: the window moves.
        assert!(drag_starts_window_move(&ctx, &mut app, egui::pos2(1000.0, 20.0)));
        // The sidebar's collapse button sits in the row: pressing it never moves the window.
        let collapsed = app.ui.sidebar.collapsed;
        assert!(!drag_starts_window_move(&ctx, &mut app, egui::pos2(255.0, 21.0)));
        assert_eq!(app.ui.sidebar.collapsed, collapsed, "a drag is not a click");
    }

    /// One whole pass as eframe runs it (`logic`, then `ui`); returns the root's commands.
    fn full_pass(ctx: &egui::Context, app: &mut YaacApp, input: egui::RawInput) -> Vec<egui::ViewportCommand> {
        let mut frame = eframe::Frame::_new_kittest();
        let mut out = ctx.run_ui(input, |ui| {
            eframe::App::logic(app, ui.ctx(), &mut frame);
            eframe::App::ui(app, ui, &mut frame);
        });
        out.textures_delta.clear();
        out.viewport_output.remove(&egui::ViewportId::ROOT).map(|v| v.commands).unwrap_or_default()
    }

    fn close_request() -> egui::RawInput {
        let mut input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0))), ..Default::default() };
        input.viewports.entry(egui::ViewportId::ROOT).or_default().events.push(egui::ViewportEvent::Close);
        input
    }

    #[test]
    fn closing_with_the_tray_option_on_hides_the_window_instead_of_quitting() {
        let ctx = egui::Context::default();
        let mut app = demo_app(&ctx);
        app.shell.fake_tray = true;
        assert!(app.ui.tray_on_close, "on by default");
        // The title bar's close button and Alt+F4 both arrive as a close request.
        let cmds = full_pass(&ctx, &mut app, close_request());
        assert!(cmds.contains(&egui::ViewportCommand::CancelClose), "{cmds:?}");
        assert!(cmds.contains(&egui::ViewportCommand::Visible(false)));
        assert!(app.shell.hidden);
        // The tray's left click shows it again and brings it to the front.
        app.shell.inject(crate::tray::ShellCmd::Show);
        let cmds = full_pass(&ctx, &mut app, egui::RawInput::default());
        assert!(cmds.contains(&egui::ViewportCommand::Visible(true)) && cmds.contains(&egui::ViewportCommand::Focus), "{cmds:?}");
        assert!(!app.shell.hidden);
        // Quit in the tray menu: the close goes through.
        app.shell.inject(crate::tray::ShellCmd::Quit);
        let cmds = full_pass(&ctx, &mut app, egui::RawInput::default());
        assert!(cmds.contains(&egui::ViewportCommand::Close));
        let cmds = full_pass(&ctx, &mut app, close_request());
        assert!(!cmds.contains(&egui::ViewportCommand::CancelClose), "{cmds:?}");
    }

    #[test]
    fn closing_quits_with_the_option_off_or_without_a_tray() {
        let ctx = egui::Context::default();
        let mut app = demo_app(&ctx);
        // No tray icon (Linux, or the icon could not be added): hiding would lose the window.
        let cmds = full_pass(&ctx, &mut app, close_request());
        assert!(!cmds.contains(&egui::ViewportCommand::CancelClose));
        app.shell.fake_tray = true;
        app.ui.tray_on_close = false;
        let cmds = full_pass(&ctx, &mut app, close_request());
        assert!(!cmds.contains(&egui::ViewportCommand::CancelClose) && !cmds.contains(&egui::ViewportCommand::Visible(false)));
        assert!(!app.shell.hidden);
    }

    fn screen() -> Option<egui::Rect> {
        Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0)))
    }

    /// One frame with these events; every key pressed is released at its end (else the next
    /// press of it would be a repeat).
    fn keys(ctx: &egui::Context, app: &mut YaacApp, mut events: Vec<egui::Event>) {
        let ups: Vec<egui::Event> = events
            .iter()
            .filter_map(|e| match e {
                egui::Event::Key { key, modifiers, .. } => Some(egui::Event::Key { key: *key, physical_key: None, pressed: false, repeat: false, modifiers: *modifiers }),
                _ => None,
            })
            .collect();
        events.extend(ups);
        window_frame(ctx, app, egui::RawInput { screen_rect: screen(), events, ..Default::default() });
    }

    fn press(k: Key, m: Modifiers) -> egui::Event {
        egui::Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers: m }
    }

    const CTRL: Modifiers = Modifiers { alt: false, ctrl: true, shift: false, mac_cmd: false, command: true };
    const CTRL_SHIFT: Modifiers = Modifiers { alt: false, ctrl: true, shift: true, mac_cmd: false, command: true };

    #[test]
    fn ctrl_space_opens_the_commander_typing_filters_and_enter_runs() {
        let ctx = egui::Context::default();
        let mut app = demo_app(&ctx);
        app.apply(vec![Action::Open(threads::fix())]);
        keys(&ctx, &mut app, vec![]);
        assert!(!app.ui.ordering.running_only);
        keys(&ctx, &mut app, vec![press(Key::Space, CTRL)]);
        assert!(app.ui.commander.is_some(), "Ctrl+Space opens it");
        // The field has the caret: typing filters.
        keys(&ctx, &mut app, vec![egui::Event::Text("only running".into())]);
        assert_eq!(app.ui.commander.as_ref().map(|c| c.query.as_str()), Some("only running"));
        keys(&ctx, &mut app, vec![press(Key::Enter, Modifiers::NONE)]);
        assert!(app.ui.ordering.running_only, "Enter ran Show only running (D129)");
        assert!(app.ui.commander.is_none());
        assert_eq!(app.ui.recent.first().map(String::as_str), Some("running-only"), "remembered as recent");
        // Esc closes it without running anything.
        keys(&ctx, &mut app, vec![press(Key::Space, CTRL)]);
        keys(&ctx, &mut app, vec![press(Key::Escape, Modifiers::NONE)]);
        assert!(app.ui.commander.is_none());
        assert!(app.ui.ordering.running_only);
    }

    /// Ctrl+Space → "Find a thread…" → the thread whose title is `title`.
    fn find_and_open(ctx: &egui::Context, app: &mut YaacApp, title: &str) {
        keys(ctx, app, vec![press(Key::Space, CTRL)]);
        keys(ctx, app, vec![egui::Event::Text("Find a thread".into())]);
        keys(ctx, app, vec![press(Key::Enter, Modifiers::NONE)]);
        assert_eq!(app.ui.commander.as_ref().and_then(|c| c.step).map(|s| s.0), Some(yaac_core::commands::Cmd::FindThread));
        keys(ctx, app, vec![egui::Event::Text(title.into())]);
        keys(ctx, app, vec![press(Key::Enter, Modifiers::NONE)]);
        assert!(app.ui.commander.is_none());
        for _ in 0..3 {
            keys(ctx, app, vec![]);
        }
    }

    /// D165: Enter shows the message at once as the user's bubble, "Sending…", before the hub
    /// answers; the hub's later answer replaces it in the same place, measured as it was.
    #[test]
    fn a_sent_message_is_on_screen_before_the_hub_answers_and_its_item_replaces_it() {
        let ctx = egui::Context::default();
        let mut app = demo_app(&ctx);
        let billing = threads::billing();
        app.apply(vec![Action::Open(billing.clone()), Action::TypeHere]);
        for _ in 0..3 {
            keys(&ctx, &mut app, vec![]);
        }
        keys(&ctx, &mut app, vec![egui::Event::Text("Add a test for the refund race.".into())]);
        keys(&ctx, &mut app, vec![press(Key::Enter, Modifiers::NONE)]);
        assert!(app.client.state.drafts.get(&billing).is_none_or(|d| d.text.is_empty()), "the composer cleared");
        let bubble = |app: &YaacApp| {
            let t = &app.client.state.transcripts[&billing];
            let shown: Vec<(usize, ItemId)> = t
                .items()
                .iter()
                .enumerate()
                .filter(|(_, i)| matches!(&i.body, yaac_protocol::ItemBody::UserMessage(u) if yaac_core::message::copy_text(u) == "Add a test for the refund race."))
                .map(|(n, i)| (n, i.id.clone()))
                .collect();
            assert_eq!(shown.len(), 1, "one bubble, never two");
            shown[0].clone()
        };
        app.ui.geom.clear();
        keys(&ctx, &mut app, vec![]);
        let (at, echo) = bubble(&app);
        assert!(yaac_core::sending::is_echo(&echo), "nothing has come back from the hub yet");
        assert!(app.ui.geom.iter().any(|g| g.drawn.contains(&at)), "the bubble is drawn");
        assert_eq!(app.client.state.transcripts[&billing].pending_of(&echo).and_then(|p| p.note()).as_deref(), Some("Sending…"));
        // The hub answers: the message's own item takes the echo's place.
        // (The demo hub releases it after what other threads already scheduled.)
        for _ in 0..10_000 {
            assert!(app.demo.as_ref().is_some_and(|h| h.tick()), "the answer is scheduled");
            app.pump_demo();
            if !yaac_core::sending::is_echo(&bubble(&app).1) {
                break;
            }
        }
        app.ui.geom.clear();
        keys(&ctx, &mut app, vec![]);
        let (now_at, real) = bubble(&app);
        assert_eq!(now_at, at, "in place");
        assert!(!yaac_core::sending::is_echo(&real));
        assert!(app.ui.geom.iter().any(|g| g.drawn.contains(&at)));
        assert!(app.ui.geom.iter().all(|g| !g.estimated.contains(&at)), "it keeps the echo's measured height: nothing jumps");
    }

    #[test]
    fn a_thread_found_in_the_commander_opens_with_the_caret_in_its_composer() {
        let ctx = egui::Context::default();
        let mut app = demo_app(&ctx);
        app.apply(vec![Action::Open(threads::fix())]);
        keys(&ctx, &mut app, vec![]);
        find_and_open(&ctx, &mut app, "billing-service");
        let billing = threads::billing();
        assert_eq!(app.client.state.selected_thread(), Some(&billing));
        let id = crate::composer::composer_id(&yaac_core::attachments::DraftRef::Thread(billing.clone()));
        assert_eq!(ctx.memory(|m| m.focused()), Some(id), "the caret is in billing-service's composer (D149)");
        // Typing goes straight into the draft.
        keys(&ctx, &mut app, vec![egui::Event::Text("hi".into())]);
        assert_eq!(app.client.state.drafts.get(&billing).map(|d| d.text.as_str()), Some("hi"));
        // A thread another program holds has no composer to type in: nothing takes the caret.
        find_and_open(&ctx, &mut app, "Explain the handshake");
        assert_eq!(app.client.state.selected_thread(), Some(&threads::handshake()));
        assert!(!ctx.text_edit_focused(), "read-only: no caret");
        // A thread waiting on an approval keeps the keyboard on its card.
        find_and_open(&ctx, &mut app, "Fix frame decoder");
        assert_eq!(app.client.state.selected_thread(), Some(&threads::fix()));
        assert!(!ctx.text_edit_focused(), "the approval card has the keyboard");
    }

    #[test]
    fn a_command_with_a_choice_continues_inside_the_commander() {
        let ctx = egui::Context::default();
        let mut app = demo_app(&ctx);
        app.apply(vec![Action::Open(threads::fix())]);
        keys(&ctx, &mut app, vec![]);
        // Ctrl+M: the model list, in the Commander.
        keys(&ctx, &mut app, vec![press(Key::M, CTRL)]);
        let c = app.ui.commander.as_ref().expect("open at the step");
        assert_eq!(c.step.map(|s| s.0), Some(yaac_core::commands::Cmd::Model));
        // Esc goes back to the whole list, a second Esc closes it.
        keys(&ctx, &mut app, vec![press(Key::Escape, Modifiers::NONE)]);
        assert!(app.ui.commander.as_ref().is_some_and(|c| c.step.is_none()));
        keys(&ctx, &mut app, vec![press(Key::Escape, Modifiers::NONE)]);
        assert!(app.ui.commander.is_none());
    }

    #[test]
    fn a_rebound_key_works_and_the_old_one_no_longer_does() {
        let ctx = egui::Context::default();
        let mut app = demo_app(&ctx);
        app.apply(vec![Action::Open(threads::fix())]);
        keys(&ctx, &mut app, vec![]);
        let y = yaac_core::keys::Chord::parse("Ctrl+Shift+Y").unwrap();
        assert!(app.ui.bindings.assign(yaac_core::commands::Cmd::RunningOnly, y, false));
        keys(&ctx, &mut app, vec![press(Key::R, CTRL_SHIFT)]);
        assert!(!app.ui.ordering.running_only, "the old key does nothing");
        keys(&ctx, &mut app, vec![press(Key::Y, CTRL_SHIFT)]);
        assert!(app.ui.ordering.running_only, "the new key runs it");
        // The change is a window preference, stored as a difference.
        assert_eq!(app.ui.prefs().bindings.0.len(), 1);
    }

    #[test]
    fn plain_keys_do_not_fire_inside_the_composer() {
        let ctx = egui::Context::default();
        let mut app = demo_app(&ctx);
        app.apply(vec![Action::Open(threads::billing())]);
        app.ui.focus_card = None;
        app.ui.focus_composer = true;
        keys(&ctx, &mut app, vec![]);
        keys(&ctx, &mut app, vec![]);
        assert!(ctx.text_edit_focused(), "the composer has the caret");
        // F2 (Rename) is a plain key: typing in the composer keeps it.
        keys(&ctx, &mut app, vec![press(Key::F2, Modifiers::NONE)]);
        assert!(app.ui.rename.is_none());
        // A chord with Ctrl still works there (Ctrl+Shift+P pins).
        let pinned = app.client.state.is_pinned(&threads::billing());
        keys(&ctx, &mut app, vec![press(Key::P, CTRL_SHIFT)]);
        assert_ne!(app.client.state.is_pinned(&threads::billing()), pinned);
        // Out of the field, F2 renames.
        ctx.memory_mut(|m| {
            if let Some(id) = m.focused() {
                m.surrender_focus(id);
            }
        });
        keys(&ctx, &mut app, vec![]);
        assert!(!ctx.text_edit_focused());
        keys(&ctx, &mut app, vec![press(Key::F2, Modifiers::NONE)]);
        assert!(app.ui.rename.is_some());
    }

    /// A click on a sidebar row (press and release on separate frames) with these modifiers.
    fn click_row(ctx: &egui::Context, app: &mut YaacApp, key: &ThreadKey, button: egui::PointerButton, m: Modifiers) {
        let rect = app.ui.list_rows.iter().find(|(k, _)| k == key).map(|(_, r)| *r).unwrap_or_else(|| panic!("{key:?} is drawn"));
        click_at(ctx, app, rect.center(), button, m);
    }

    fn click_at(ctx: &egui::Context, app: &mut YaacApp, at: egui::Pos2, button: egui::PointerButton, m: Modifiers) {
        let frame = |mut events: Vec<egui::Event>| {
            events.insert(0, egui::Event::ModifiersChanged(m));
            egui::RawInput { screen_rect: screen(), events, ..Default::default() }
        };
        window_frame(ctx, app, frame(vec![egui::Event::PointerMoved(at)]));
        window_frame(ctx, app, frame(vec![egui::Event::PointerButton { pos: at, button, pressed: true, modifiers: m }]));
        window_frame(ctx, app, frame(vec![egui::Event::PointerButton { pos: at, button, pressed: false, modifiers: m }]));
    }

    #[test]
    fn ctrl_click_two_rows_and_archive_selected_sends_one_archive_each_with_its_own_op_id() {
        let ctx = egui::Context::default();
        let mut app = demo_app(&ctx);
        for key in [threads::users(), threads::billing()] {
            app.client.state.threads.get_mut(&key).unwrap().summary.spawned_by = None;
        }
        app.apply(vec![Action::Open(threads::fix())]);
        for _ in 0..3 {
            keys(&ctx, &mut app, vec![]);
        }
        let (users, billing) = (threads::users(), threads::billing());
        // Ctrl+click two rows: both selected, neither opened.
        click_row(&ctx, &mut app, &users, egui::PointerButton::Primary, CTRL);
        click_row(&ctx, &mut app, &billing, egui::PointerButton::Primary, CTRL);
        assert_eq!(app.ui.list_sel.len(), 2);
        assert_eq!(app.client.state.selected_thread(), Some(&threads::fix()), "Ctrl+click opens nothing");
        // Right-click one of them: the selection's menu.
        click_row(&ctx, &mut app, &billing, egui::PointerButton::Secondary, Modifiers::NONE);
        assert_eq!(app.ui.menu, Some(Menu::Selection(billing.clone())));
        // Its first entry, "Archive selected (2)…", opens one confirmation for both.
        let item = app.ui.list_rows.iter().find(|(k, _)| *k == billing).map(|(_, r)| egui::pos2(r.right() + 60.0, r.top() + 16.0)).unwrap();
        click_at(&ctx, &mut app, item, egui::PointerButton::Primary, Modifiers::NONE);
        let d = app.ui.bulk_archive.clone().expect("the bulk confirmation");
        assert_eq!(d.title, "Archive 2 threads?");
        assert_eq!(d.skipped, None);
        // Enter confirms: one `thread/archive` per thread, each with its own operation id.
        let hub = app.demo.clone().unwrap();
        let before = hub.requests().len();
        keys(&ctx, &mut app, vec![press(Key::Enter, Modifiers::NONE)]);
        assert!(app.ui.bulk_archive.is_none());
        let archives: Vec<_> = hub.requests().into_iter().skip(before).filter(|(_, m, _)| m == "thread/archive").collect();
        assert_eq!(archives.len(), 2, "{archives:?}");
        let ops: BTreeSet<String> = archives.iter().map(|(_, _, p)| p["opId"].as_str().unwrap().to_owned()).collect();
        assert_eq!(ops.len(), 2, "distinct operation ids");
        let ids: BTreeSet<String> = archives.iter().map(|(_, _, p)| p["threadId"].as_str().unwrap().to_owned()).collect();
        assert_eq!(ids, BTreeSet::from([users.thread.to_string(), billing.thread.to_string()]));
        // Both moved to Archived; the selection is gone; nothing is sent again.
        app.pump_demo();
        assert!(app.ui.list_sel.is_empty());
        assert!(app.client.state.summary(&users).unwrap().archived && app.client.state.summary(&billing).unwrap().archived);
        assert!(app.client.bulk_report.is_none(), "all succeeded: no report");
        let n = hub.requests().len();
        keys(&ctx, &mut app, vec![]);
        assert!(hub.requests()[n..].iter().all(|(_, m, _)| m != "thread/archive"));
    }

    /// Press on `from`, move in steps to `to`, release there: one frame per event.
    fn drag(ctx: &egui::Context, app: &mut YaacApp, from: egui::Pos2, to: egui::Pos2) {
        let frame = |events: Vec<egui::Event>| egui::RawInput { screen_rect: screen(), events, ..Default::default() };
        window_frame(ctx, app, frame(vec![egui::Event::PointerMoved(from)]));
        window_frame(ctx, app, frame(vec![egui::Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }]));
        for i in 1..=6 {
            let at = from + (to - from) * (i as f32 / 6.0);
            window_frame(ctx, app, frame(vec![egui::Event::PointerMoved(at)]));
        }
        window_frame(ctx, app, frame(vec![egui::Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }]));
        window_frame(ctx, app, frame(vec![]));
    }

    fn head_of(app: &YaacApp, of: &ThreadKey) -> egui::Pos2 {
        let id = yaac_core::sidebar::section_of(&app.client.state, app.ui.grouping, of).expect("its section");
        app.ui.list_heads.iter().find(|(s, _)| *s == id).map(|(_, r)| r.center()).unwrap_or_else(|| panic!("the header of {id} is drawn"))
    }

    fn row_of(app: &YaacApp, key: &ThreadKey) -> egui::Pos2 {
        app.ui.list_rows.iter().find(|(k, _)| k == key).map(|(_, r)| r.center()).unwrap_or_else(|| panic!("{key:?} is drawn"))
    }

    #[test]
    fn clicking_a_group_header_hides_its_rows_and_keeps_the_chat_open() {
        let ctx = egui::Context::default();
        let mut app = demo_app(&ctx);
        let key = threads::fix();
        app.apply(vec![Action::Open(key.clone())]);
        keys(&ctx, &mut app, vec![]);
        let header = head_of(&app, &key);
        click_at(&ctx, &mut app, header, egui::PointerButton::Primary, Modifiers::NONE);
        keys(&ctx, &mut app, vec![]);
        assert!(!app.ui.list_order.contains(&key));
        assert_eq!(app.client.state.selected_thread(), Some(&key));

        let header = head_of(&app, &key);
        click_at(&ctx, &mut app, header, egui::PointerButton::Primary, Modifiers::NONE);
        keys(&ctx, &mut app, vec![]);
        assert!(app.ui.list_order.contains(&key));
    }

    #[test]
    fn dragging_a_row_onto_another_projects_header_sends_thread_move_with_an_undo() {
        let ctx = egui::Context::default();
        let mut app = demo_app(&ctx);
        for key in [threads::billing(), threads::search()] {
            app.client.state.threads.get_mut(&key).unwrap().summary.spawned_by = None;
        }
        app.apply(vec![Action::Open(threads::fix())]);
        for _ in 0..3 {
            keys(&ctx, &mut app, vec![]);
        }
        let hub = app.demo.clone().unwrap();
        let billing = threads::billing();
        let yaac = app.client.state.project_of(&threads::fix()).map(|p| p.wire_id.clone()).expect("yaac on devbox");
        // billing-service (Claude: no folder change) onto "yaac" on the same machine: moved at
        // once, one `thread/move` with its own operation id, and an Undo in the bar.
        let before = hub.requests().len();
        let (from, to) = (row_of(&app, &billing), head_of(&app, &threads::fix()));
        drag(&ctx, &mut app, from, to);
        let moves: Vec<_> = hub.requests().into_iter().skip(before).filter(|(_, m, _)| m == "thread/move").collect();
        assert_eq!(moves.len(), 1, "{moves:?}");
        let (_, _, params) = &moves[0];
        assert_eq!(params["threadId"], billing.thread.as_str());
        assert_eq!(params["project"], yaac.as_str());
        assert!(params["opId"].as_str().is_some_and(|o| !o.is_empty()));
        assert!(app.ui.drag.is_none() && app.ui.move_dialog.is_none(), "no confirmation: the folder does not change");
        app.pump_demo();
        assert_eq!(app.client.state.project_of(&billing).map(|p| p.wire_id.clone()), Some(yaac));
        let notice = app.client.move_notice.clone().expect("the bar's notice");
        assert_eq!(notice.text, "Moved “billing-service” to yaac");
        // Undo puts it back: the placement is forgotten (it had none).
        app.apply(vec![Action::UndoMove]);
        let undo: Vec<_> = hub.requests().into_iter().filter(|(_, m, _)| m == "thread/move").collect();
        assert_eq!(undo.len(), 2);
        assert!(undo[1].2.get("project").is_none(), "{:?}", undo[1].2);
        app.pump_demo();
        for _ in 0..2 {
            keys(&ctx, &mut app, vec![]);
        }

        // search-service is on gpu-box: dropping it on devbox's group is refused, nothing sent.
        let n = hub.requests().len();
        let (from, to) = (row_of(&app, &threads::search()), head_of(&app, &threads::fix()));
        drag(&ctx, &mut app, from, to);
        assert!(hub.requests()[n..].iter().all(|(_, m, _)| m != "thread/move"), "another machine: refused");

        // Fix frame decoder (Codex: its folder follows) onto "api" on devbox: asks first.
        let (from, to) = (row_of(&app, &threads::fix()), head_of(&app, &billing));
        drag(&ctx, &mut app, from, to);
        let d = app.ui.move_dialog.clone().expect("the confirmation");
        assert!(d.sentence(&app.client.state).starts_with("The agent will work in /home/dev/api from its next turn"), "{}", d.sentence(&app.client.state));
        assert!(hub.requests()[n..].iter().all(|(_, m, _)| m != "thread/move"), "nothing sent before Move");
        keys(&ctx, &mut app, vec![press(Key::Enter, Modifiers::NONE)]);
        assert!(app.ui.move_dialog.is_none());
        assert_eq!(hub.requests()[n..].iter().filter(|(_, m, _)| m == "thread/move").count(), 1);
        assert!(app.client.move_notice.is_none(), "no Undo when a folder changed");
    }

    #[test]
    fn shift_click_ranges_esc_clears_and_a_plain_click_opens() {
        let ctx = egui::Context::default();
        let mut app = demo_app(&ctx);
        for key in [threads::search(), threads::billing(), threads::users(), threads::index()] {
            app.client.state.threads.get_mut(&key).unwrap().summary.spawned_by = None;
        }
        app.apply(vec![Action::Open(threads::search())]);
        for _ in 0..3 {
            keys(&ctx, &mut app, vec![]);
        }
        // From the open thread down to billing-service, across two groups.
        click_row(&ctx, &mut app, &threads::billing(), egui::PointerButton::Primary, Modifiers::SHIFT);
        assert_eq!(app.ui.list_sel.in_order(&app.ui.list_order), [threads::search(), threads::ci(), threads::billing()]);
        // The bar under the list says so.
        assert!(app.ui.list_focus);
        keys(&ctx, &mut app, vec![press(Key::Escape, Modifiers::NONE)]);
        assert!(app.ui.list_sel.is_empty(), "Esc clears it");
        // Ctrl+A with focus in the list: every listed row.
        keys(&ctx, &mut app, vec![press(Key::A, CTRL)]);
        assert_eq!(app.ui.list_sel.len(), app.ui.list_order.len());
        // A plain click opens the row and clears the selection.
        click_row(&ctx, &mut app, &threads::users(), egui::PointerButton::Primary, Modifiers::NONE);
        assert!(app.ui.list_sel.is_empty());
        assert_eq!(app.client.state.selected_thread(), Some(&threads::users()));
    }

    #[test]
    fn settings_keyboard_records_flags_a_conflict_and_replaces() {
        use yaac_core::commands::Cmd;
        let ctx = egui::Context::default();
        let mut app = demo_app(&ctx);
        app.apply(vec![Action::SettingsPage(Page::Keyboard)]);
        keys(&ctx, &mut app, vec![]);
        // Recording the Commander's key: Ctrl+K belongs to Search threads.
        app.ui.set.keys.recording = Some(Cmd::Commander);
        keys(&ctx, &mut app, vec![press(Key::K, CTRL)]);
        assert_eq!(app.ui.set.keys.conflict.as_ref().map(|c| c.2), Some(Cmd::SearchThreads));
        assert_eq!(app.ui.bindings.label(Cmd::Commander), "Ctrl+Space", "nothing changes until resolved");
        assert!(app.ui.settings, "the page stays open");
        // Replace.
        let (cmd, chord, _) = app.ui.set.keys.conflict.take().unwrap();
        app.ui.bindings.assign(cmd, chord, true);
        assert_eq!(app.ui.bindings.label(Cmd::Commander), "Ctrl+K");
        assert_eq!(app.ui.bindings.label(Cmd::SearchThreads), "");
        // Esc cancels a recording without leaving Settings.
        app.ui.set.keys.recording = Some(Cmd::Rename);
        keys(&ctx, &mut app, vec![press(Key::Escape, Modifiers::NONE)]);
        assert!(app.ui.set.keys.recording.is_none() && app.ui.settings);
        assert_eq!(app.ui.bindings.label(Cmd::Rename), "F2");
    }

    /// D166 (the owner's report: "the Send button turns into STOP; I can only send again once
    /// I hit Stop"): while a turn runs, Enter queues, Ctrl+Enter sends now, and the strip's ✕
    /// cancels a queued message — each a request to `yaacd`, never a send that is dropped.
    #[test]
    fn while_a_turn_runs_enter_queues_ctrl_enter_sends_now_and_the_strip_cancels() {
        let ctx = egui::Context::default();
        let mut app = demo_app(&ctx);
        let billing = threads::billing();
        app.apply(vec![Action::Open(billing.clone())]);
        app.pump_demo();
        let hub = app.demo.clone().unwrap();
        hub.play(yaac_core::demo::Scene::QueueWhileRunning);
        app.pump_demo();
        assert!(yaac_core::queue::running(&app.client.state, &billing));
        app.ui.focus_composer = true;
        for _ in 0..3 {
            keys(&ctx, &mut app, vec![]);
        }
        let starts = |hub: &DemoHub| hub.requests().into_iter().filter(|(_, m, _)| m == "turn/start").map(|(_, _, p)| p).collect::<Vec<_>>();
        // Type and press Enter: queued, the composer clears, the strip shows it third.
        keys(&ctx, &mut app, vec![egui::Event::Text("Bump the crate version too.".into())]);
        keys(&ctx, &mut app, vec![press(Key::Enter, Modifiers::NONE)]);
        let sent = starts(&hub);
        assert_eq!(sent.len(), 1, "{sent:?}");
        assert_eq!(sent[0]["delivery"], "queue");
        app.pump_demo();
        keys(&ctx, &mut app, vec![]);
        assert!(app.client.state.drafts.get(&billing).is_none_or(|d| d.text.is_empty()), "the composer clears");
        let strip: Vec<String> = yaac_core::queue::strip(&app.client.state, &billing).into_iter().map(|r| r.preview).collect();
        assert_eq!(strip.last().map(String::as_str), Some("Bump the crate version too."), "{strip:?}");
        assert_eq!(strip.len(), 3);
        // Ctrl+Enter sends now (this thread cannot steer: it interrupts).
        keys(&ctx, &mut app, vec![egui::Event::Text("Use the shared retry helper.".into())]);
        keys(&ctx, &mut app, vec![press(Key::Enter, CTRL)]);
        let sent = starts(&hub);
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[1]["delivery"], "now");
        app.pump_demo();
        for _ in 0..2 {
            keys(&ctx, &mut app, vec![]);
        }
        // The strip's ✕ on the first queued message: turn/queueCancel for it.
        let first = app.client.state.transcripts[&billing].queued[0].queue_id.clone();
        let cancel = ctx.read_response(crate::composer::strip_button_id("cancel", &first)).expect("the strip shows its ✕");
        click_at(&ctx, &mut app, cancel.rect.center(), egui::PointerButton::Primary, Modifiers::NONE);
        let cancels: Vec<_> = hub.requests().into_iter().filter(|(_, m, _)| m == "turn/queueCancel").map(|(_, _, p)| p).collect();
        assert_eq!(cancels.len(), 1, "{cancels:?}");
        assert_eq!(cancels[0]["queueId"], first.as_str());
        app.pump_demo();
        assert!(app.client.state.transcripts[&billing].queued.iter().all(|q| q.queue_id != first));
    }
}
