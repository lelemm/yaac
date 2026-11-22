//! The command table (D130, D131; `docs/plan/commands.md`): everything the window can do, in
//! one place. The Commander lists it, Settings → Keyboard shows its keys, and the window's one
//! keyboard dispatcher resolves chords through it — so the three never drift.
//!
//! Each command has a stable id (what a stored binding names), a name, a one-line explanation,
//! search aliases, a group, default keys, and a rule that says, from the current state, whether
//! it is enabled, disabled with a reason, or hidden. Commands that take an argument name a
//! [`StepKind`]: the Commander continues with a sub-list built by [`step_items`]. What running
//! a command *does* is the window's (it maps [`Cmd`] and [`StepValue`] to its actions).
//!
//! Availability that depends on the harness reads capabilities, never the provider's kind.

use std::collections::BTreeSet;

use yaac_protocol::{EffortId, InstanceId, ItemBody, ItemId, ModeId, ModelId, RequestState, TargetId, ThreadState};

use crate::agents::{SpawnedBy, agents_panel};
use crate::attention::AttentionMap;
use crate::controls::{ControlRow, ControlRowView, draft_row_of, thread_row};
use crate::key::ThreadKey;
use crate::keys::{Chord, Overrides};
use crate::prefs::ThemePref;
use crate::settings::Page;
use crate::sidebar::{Grouping, Order, Ordering};
use crate::state::{AgentsPolicy, AppState};
use crate::thread_menu::{MenuAction, thread_menu};

/// A command.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[allow(missing_docs, reason = "each variant is described by its row in COMMANDS")]
pub enum Cmd {
    // Go to
    Commander,
    SearchThreads,
    FindThread,
    NeedsNext,
    NextUnseen,
    NextThread,
    PrevThread,
    Back,
    Forward,
    History,
    Parent,
    Related,
    Settings,
    SettingsPage,
    // Threads
    NewThread,
    NewInProject,
    Rename,
    CopyId,
    Pin,
    Mute,
    Archive,
    Delete,
    Dismiss,
    DismissAllTemp,
    Fork,
    MoveThread,
    MoveSelected,
    CheckLatest,
    ContinueBudget,
    ArchiveSelected,
    MuteSelected,
    ClearSelection,
    // Composer and turns
    FocusComposer,
    Send,
    SendNow,
    NewLine,
    Stop,
    Attach,
    Paste,
    Model,
    Effort,
    Mode,
    Agents,
    Fast,
    // Requests
    AllowOnce,
    AllowSession,
    Deny,
    AnswerQuestion,
    // Pull requests
    OpenPr,
    CreatePr,
    CopyPrLink,
    // View
    ToggleSidebar,
    Dock,
    Grouping,
    Order,
    UnseenFirst,
    NeedsFirst,
    RunningOnly,
    ShowDismissed,
    ShowArchived,
    Theme,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    ExpandAll,
    CollapseAll,
    CopySelection,
    CopyMessage,
    // Targets and providers
    Retry,
    StartWsl,
    TargetEnable,
    AddSsh,
    AddWsl,
    RefreshProviders,
    // Window
    HideToTray,
    Quit,
    Renderer,
}

/// The catalogue's groups, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Group {
    /// Go to.
    GoTo,
    /// Threads.
    Threads,
    /// Composer and turns.
    Composer,
    /// Requests (act only on the focused card).
    Requests,
    /// Pull requests.
    PullRequests,
    /// View.
    View,
    /// Targets and providers.
    Targets,
    /// Window.
    Window,
}

impl Group {
    /// Every group, in order.
    pub const ALL: [Group; 8] = [Group::GoTo, Group::Threads, Group::Composer, Group::Requests, Group::PullRequests, Group::View, Group::Targets, Group::Window];

    /// The heading.
    pub fn label(self) -> &'static str {
        match self {
            Group::GoTo => "Go to",
            Group::Threads => "Threads",
            Group::Composer => "Composer and turns",
            Group::Requests => "Requests",
            Group::PullRequests => "Pull requests",
            Group::View => "View",
            Group::Targets => "Targets and providers",
            Group::Window => "Window",
        }
    }
}

/// Where a command's keys work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// Anywhere in the window, through the one dispatcher; rebindable.
    Global,
    /// Only where it is ("in the message box", "on the focused card"): handled there, shown
    /// in Settings → Keyboard but not rebindable. Its keys may repeat another local command's.
    Local(&'static str),
}

/// A command that continues inside the Commander with a choice.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StepKind {
    /// Any thread by title or id.
    Thread,
    /// The back/forward list.
    History,
    /// This thread's related agents.
    Related,
    /// A settings page.
    Page,
    /// A model of this harness.
    Model,
    /// An effort level.
    Effort,
    /// An approval mode.
    Mode,
    /// Agents: ask / allow.
    Agents,
    /// A mute duration.
    Mute,
    /// The sidebar's order.
    Order,
    /// The grouping checkboxes.
    Grouping,
    /// The theme.
    Theme,
    /// An offline target to retry.
    Retry,
    /// A stopped WSL distribution.
    StartWsl,
    /// A target to enable or disable.
    TargetEnable,
    /// A target whose providers to read again.
    RefreshProviders,
    /// A project (or a worktree of one) of the open thread's machine, to move it to (D139).
    MoveTo,
}

/// One row of the table.
#[derive(Clone, Copy, Debug)]
pub struct CommandDef {
    /// The command.
    pub cmd: Cmd,
    /// Stable id (stored bindings and the recent list name it).
    pub id: &'static str,
    /// The name.
    pub name: &'static str,
    /// What selecting it does, in one line.
    pub explain: &'static str,
    /// More words it is found by.
    pub aliases: &'static [&'static str],
    /// Its group.
    pub group: Group,
    /// Default keys (parsed by [`Chord::parse`]).
    pub keys: &'static [&'static str],
    /// Where the keys work.
    pub scope: Scope,
    /// The choice it continues with in the Commander.
    pub step: Option<StepKind>,
}

const fn def(cmd: Cmd, id: &'static str, name: &'static str, explain: &'static str, aliases: &'static [&'static str], group: Group, keys: &'static [&'static str]) -> CommandDef {
    CommandDef { cmd, id, name, explain, aliases, group, keys, scope: Scope::Global, step: None }
}

const fn step(d: CommandDef, s: StepKind) -> CommandDef {
    CommandDef { step: Some(s), ..d }
}

const fn local(d: CommandDef, place: &'static str) -> CommandDef {
    CommandDef { scope: Scope::Local(place), ..d }
}

use Group as G;

/// Every command, in catalogue order.
pub static COMMANDS: &[CommandDef] = &[
    // ── Go to
    def(Cmd::Commander, "commander", "Commander", "Open this list.", &["palette", "commands", "actions"], G::GoTo, &["Ctrl+Space"]),
    def(Cmd::SearchThreads, "search-threads", "Search threads", "Focus the sidebar's search field.", &["filter"], G::GoTo, &["Ctrl+K"]),
    step(def(Cmd::FindThread, "find-thread", "Find a thread…", "Find any thread by title or id across every target, and open it.", &["go to", "open", "jump", "switch"], G::GoTo, &["Ctrl+P"]), StepKind::Thread),
    def(Cmd::NeedsNext, "needs-next", "Next thread that needs you", "Open the next thread waiting on you, at its request.", &["approval", "question", "attention", "waiting"], G::GoTo, &["Ctrl+J"]),
    def(Cmd::NextUnseen, "next-unseen", "Next unseen finished thread", "Open the next thread whose finished turn you have not seen.", &["unread", "done"], G::GoTo, &["Ctrl+U"]),
    def(Cmd::NextThread, "next-thread", "Next thread in the list", "Move the selection down the sidebar.", &["down"], G::GoTo, &["Ctrl+Down"]),
    def(Cmd::PrevThread, "prev-thread", "Previous thread in the list", "Move the selection up the sidebar.", &["up"], G::GoTo, &["Ctrl+Up"]),
    def(Cmd::Back, "back", "Back", "Go back through visited threads.", &["history", "previous"], G::GoTo, &["Alt+Left", "Mouse back"]),
    def(Cmd::Forward, "forward", "Forward", "Go forward through visited threads.", &["history", "next"], G::GoTo, &["Alt+Right", "Mouse forward"]),
    step(def(Cmd::History, "history", "History…", "Pick a place from the back and forward list.", &["visited", "recent"], G::GoTo, &[]), StepKind::History),
    def(Cmd::Parent, "parent", "Go to the parent thread", "Open the thread that spawned this one.", &["spawner", "orchestrator", "up"], G::GoTo, &["Alt+Up"]),
    step(def(Cmd::Related, "related", "Go to a related agent…", "Pick one of this thread's related agents and open it.", &["worker", "child", "contact", "mesh", "agents"], G::GoTo, &["Alt+Down"]), StepKind::Related),
    def(Cmd::Settings, "settings", "Settings", "Open Settings.", &["preferences", "options"], G::GoTo, &["Ctrl+Comma"]),
    step(def(Cmd::SettingsPage, "settings-page", "Settings: a page…", "Open one Settings page (Targets, Providers, Keyboard, Window…).", &["preferences", "keyboard", "shortcuts", "bindings"], G::GoTo, &[]), StepKind::Page),
    // ── Threads
    def(Cmd::NewThread, "new-thread", "New thread", "Start a draft (choose target, project, model).", &["create", "start", "chat"], G::Threads, &["Ctrl+N"]),
    def(Cmd::NewInProject, "new-in-project", "New thread in this project", "Draft in this thread's project and target, same harness and model.", &["create", "start", "sibling"], G::Threads, &["Ctrl+Shift+N"]),
    def(Cmd::Rename, "rename", "Rename", "Rename this thread.", &["title"], G::Threads, &["F2"]),
    def(Cmd::CopyId, "copy-id", "Copy thread ID", "Copy the harness's conversation id.", &["session", "clipboard"], G::Threads, &["Ctrl+Shift+C"]),
    def(Cmd::Pin, "pin", "Pin / Unpin", "Keep this thread in Pinned.", &["favourite", "favorite", "star"], G::Threads, &["Ctrl+Shift+P"]),
    step(def(Cmd::Mute, "mute", "Mute… / Unmute", "Mute for a duration or until a date.", &["silence", "snooze", "quiet"], G::Threads, &["Ctrl+Shift+M"]), StepKind::Mute),
    def(Cmd::Archive, "archive", "Archive… / Unarchive", "Move this thread to Archived, with confirmation.", &["hide"], G::Threads, &["Ctrl+Shift+E"]),
    def(Cmd::Delete, "delete", "Delete…", "Delete this thread, with the \"also in the harness\" choice.", &["remove", "trash"], G::Threads, &["Ctrl+Delete"]),
    def(Cmd::Dismiss, "dismiss", "Dismiss / Show again", "Hide a discovered thread in the window.", &["hide"], G::Threads, &[]),
    def(Cmd::DismissAllTemp, "dismiss-all-temp", "Dismiss all in this temporary project", "Hide every thread of this folder in the window.", &["hide"], G::Threads, &[]),
    def(Cmd::Fork, "fork", "Fork…", "Open the fork dialog (native or handoff).", &["branch", "copy", "handoff", "switch provider"], G::Threads, &["Ctrl+Shift+F"]),
    step(def(Cmd::MoveThread, "move", "Move to project…", "List this thread under another project or worktree of its machine; its harness may move its folder too.", &["relocate", "project", "worktree", "regroup"], G::Threads, &[]), StepKind::MoveTo),
    def(Cmd::MoveSelected, "move-selected", "Move selected…", "Move the rows selected in the sidebar to another project or worktree of their machine.", &["bulk", "many", "multiple", "relocate", "project"], G::Threads, &[]),
    def(Cmd::CheckLatest, "check-latest", "Check for the latest", "Read this thread again from its target.", &["refresh", "reload", "status"], G::Threads, &[]),
    def(Cmd::ContinueBudget, "continue-budget", "Continue (budget)", "Resume an orchestration paused on its budget.", &["resume", "allowance"], G::Threads, &[]),
    def(Cmd::ArchiveSelected, "archive-selected", "Archive selected…", "Archive the rows selected in the sidebar, with one confirmation.", &["bulk", "many", "multiple", "hide"], G::Threads, &[]),
    def(Cmd::MuteSelected, "mute-selected", "Mute selected…", "Mute the rows selected in the sidebar, with one choice of how long.", &["bulk", "many", "multiple", "silence", "snooze"], G::Threads, &[]),
    local(def(Cmd::ClearSelection, "clear-selection", "Clear selection", "Unselect the rows selected in the sidebar.", &["deselect", "unselect", "bulk"], G::Threads, &["Escape"]), "in the sidebar list"),
    // ── Composer and turns
    def(Cmd::FocusComposer, "focus-composer", "Focus the composer", "Put the caret in the message box.", &["type", "message", "input"], G::Composer, &["Ctrl+L"]),
    local(def(Cmd::Send, "send", "Send / queue", "Send; while a turn runs, queue.", &["submit"], G::Composer, &["Enter"]), "in the message box"),
    local(def(Cmd::SendNow, "send-now", "Send now", "Steer or interrupt the running turn.", &["steer", "interrupt"], G::Composer, &["Ctrl+Enter"]), "in the message box"),
    local(def(Cmd::NewLine, "new-line", "New line", "Start a new line in the message.", &["newline", "break"], G::Composer, &["Shift+Enter"]), "in the message box"),
    local(def(Cmd::Stop, "stop", "Stop the turn", "Stop the running turn.", &["interrupt", "cancel", "abort"], G::Composer, &["Escape"]), "in the message box"),
    def(Cmd::Attach, "attach", "Attach files…", "Open the file picker.", &["upload", "file", "image"], G::Composer, &["Ctrl+Shift+A"]),
    local(def(Cmd::Paste, "paste", "Paste", "Paste text, files or images.", &["clipboard"], G::Composer, &["Ctrl+V"]), "in any field"),
    step(def(Cmd::Model, "model", "Change model…", "Pick a model of this harness.", &["switch model", "llm"], G::Composer, &["Ctrl+M"]), StepKind::Model),
    step(def(Cmd::Effort, "effort", "Change effort…", "Pick an effort level.", &["reasoning", "thinking"], G::Composer, &["Ctrl+E"]), StepKind::Effort),
    step(def(Cmd::Mode, "mode", "Change mode…", "Pick the approval mode (ask, auto, …).", &["approvals", "permissions", "sandbox"], G::Composer, &[]), StepKind::Mode),
    step(def(Cmd::Agents, "agents", "Agents: ask / allow…", "Set whether agents may spawn and message.", &["mesh", "spawn", "permission"], G::Composer, &[]), StepKind::Agents),
    def(Cmd::Fast, "fast", "Fast mode on / off", "Toggle fast mode where the model offers it.", &["speed", "lightning", "quick"], G::Composer, &["Ctrl+Shift+L"]),
    // ── Requests
    local(def(Cmd::AllowOnce, "allow-once", "Allow once", "Allow the focused approval request this time.", &["approve", "yes", "accept"], G::Requests, &["Enter"]), "on the focused card"),
    local(def(Cmd::AllowSession, "allow-session", "Allow for this thread", "Allow the focused request for the rest of the thread.", &["approve", "always"], G::Requests, &[]), "on the focused card"),
    local(def(Cmd::Deny, "deny", "Deny", "Deny the focused approval request.", &["reject", "no", "decline"], G::Requests, &["Escape"]), "on the focused card"),
    def(Cmd::AnswerQuestion, "answer-question", "Answer the question", "Focus the open question's input.", &["reply", "request"], G::Requests, &[]),
    // ── Pull requests
    def(Cmd::OpenPr, "open-pr", "Open this branch's PR", "Open it in the browser.", &["pull request", "github", "merge request"], G::PullRequests, &["Ctrl+Shift+O"]),
    def(Cmd::CreatePr, "create-pr", "Create a pull request…", "Open the create modal.", &["pull request", "new pr"], G::PullRequests, &[]),
    def(Cmd::CopyPrLink, "copy-pr-link", "Copy the PR link", "Copy the pull request's address.", &["pull request", "url", "clipboard"], G::PullRequests, &[]),
    // ── View
    def(Cmd::ToggleSidebar, "toggle-sidebar", "Toggle the sidebar", "Collapse the sidebar to the strip and back.", &["collapse", "expand", "strip"], G::View, &["Ctrl+B"]),
    def(Cmd::Dock, "dock", "Dock / undock the agents panel", "Dock the agents panel beside the thread, or float it.", &["panel", "pill"], G::View, &["Ctrl+Shift+B"]),
    step(def(Cmd::Grouping, "grouping", "Grouping: machine / project / worktree…", "Toggle each grouping of the sidebar.", &["group by", "sections"], G::View, &[]), StepKind::Grouping),
    step(def(Cmd::Order, "order", "Order: last message / A → Z / Z → A…", "How threads are ordered in each group.", &["sort", "ordering"], G::View, &[]), StepKind::Order),
    def(Cmd::UnseenFirst, "unseen-first", "Keep unseen at the top", "Unseen finished threads stay at the top of their group.", &["ordering", "sort"], G::View, &[]),
    def(Cmd::NeedsFirst, "needs-first", "Keep needs-you at the top", "Threads that need you stay at the top of their group.", &["ordering", "sort"], G::View, &[]),
    def(Cmd::RunningOnly, "running-only", "Show only running", "List only running threads (and those kept at the top).", &["filter", "active", "busy"], G::View, &["Ctrl+Shift+R"]),
    def(Cmd::ShowDismissed, "show-dismissed", "Show dismissed", "List dismissed threads too.", &["hidden"], G::View, &[]),
    def(Cmd::ShowArchived, "show-archived", "Show archived", "Open the Archived section.", &["hidden"], G::View, &[]),
    step(def(Cmd::Theme, "theme", "Theme: system / light / dark…", "Choose the colours.", &["dark mode", "light mode", "appearance", "colors", "colours"], G::View, &[]), StepKind::Theme),
    def(Cmd::ZoomIn, "zoom-in", "Zoom in", "Make the interface larger.", &["bigger", "scale", "font size"], G::View, &["Ctrl+Equals", "Ctrl+Plus"]),
    def(Cmd::ZoomOut, "zoom-out", "Zoom out", "Make the interface smaller.", &["smaller", "scale", "font size"], G::View, &["Ctrl+Minus"]),
    def(Cmd::ZoomReset, "zoom-reset", "Reset zoom", "Return the interface to its normal size.", &["actual size", "scale"], G::View, &["Ctrl+0"]),
    def(Cmd::ExpandAll, "expand-all", "Expand all tool lines", "Open every activity line in this thread.", &["unfold", "show"], G::View, &[]),
    def(Cmd::CollapseAll, "collapse-all", "Collapse all tool lines", "Close every activity line in this thread.", &["fold", "hide"], G::View, &[]),
    local(def(Cmd::CopySelection, "copy-selection", "Copy the selection", "Copy the selected text.", &["clipboard"], G::View, &["Ctrl+C"]), "where text is selected"),
    def(Cmd::CopyMessage, "copy-message", "Copy message", "Copy the agent's last message in this thread.", &["clipboard", "reply"], G::View, &[]),
    // ── Targets and providers
    step(def(Cmd::Retry, "retry", "Retry connecting…", "Retry an offline target now.", &["reconnect", "offline"], G::Targets, &[]), StepKind::Retry),
    step(def(Cmd::StartWsl, "start-wsl", "Start a WSL distribution…", "Start a stopped distribution and connect to it.", &["linux", "boot"], G::Targets, &[]), StepKind::StartWsl),
    step(def(Cmd::TargetEnable, "target-enable", "Enable / disable a target…", "Turn a target's connection on or off.", &["machine", "connection"], G::Targets, &[]), StepKind::TargetEnable),
    def(Cmd::AddSsh, "add-ssh", "Add an SSH target…", "Open the add form.", &["server", "remote", "machine"], G::Targets, &[]),
    def(Cmd::AddWsl, "add-wsl", "Add WSL…", "Open the WSL form.", &["linux", "distribution"], G::Targets, &[]),
    step(def(Cmd::RefreshProviders, "refresh-providers", "Refresh providers and models…", "Read a target's harnesses again.", &["reload", "harness", "models"], G::Targets, &[]), StepKind::RefreshProviders),
    // ── Window
    def(Cmd::HideToTray, "hide-to-tray", "Hide to the tray", "Hide the window; the tray icon brings it back.", &["minimize", "minimise", "close"], G::Window, &["Ctrl+W"]),
    def(Cmd::Quit, "quit", "Quit YAAC", "Close the window (the hub keeps running).", &["exit", "close"], G::Window, &["Ctrl+Q"]),
    def(Cmd::Renderer, "renderer", "Renderer…", "Choose the renderer in Settings → Window.", &["gpu", "graphics", "display", "software"], G::Window, &[]),
];

/// The row of a command.
pub fn def_of(cmd: Cmd) -> &'static CommandDef {
    COMMANDS.iter().find(|d| d.cmd == cmd).unwrap_or(&COMMANDS[0])
}

/// The row with this id.
pub fn by_id(id: &str) -> Option<&'static CommandDef> {
    COMMANDS.iter().find(|d| d.id == id)
}

/// What the window knows that the state does not.
#[derive(Clone, Copy, Debug)]
pub struct Ctx<'a> {
    /// The state.
    pub state: &'a AppState,
    /// Now (seconds).
    pub now: i64,
    /// The open thread, while the thread view shows (not Settings).
    pub thread: Option<&'a ThreadKey>,
    /// The open new-thread draft, while it shows.
    pub draft: Option<u64>,
    /// Back is possible.
    pub can_back: bool,
    /// Forward is possible.
    pub can_forward: bool,
    /// A tray icon exists.
    pub has_tray: bool,
    /// The sidebar grouping.
    pub grouping: Grouping,
    /// The sidebar ordering.
    pub ordering: Ordering,
    /// The Archived section is open.
    pub archived_open: bool,
    /// The agents panel is docked.
    pub docked: bool,
    /// The theme.
    pub theme: ThemePref,
    /// The interface size (points), and its bounds.
    pub zoom: (f32, f32, f32),
    /// The host is Windows (WSL commands apply).
    pub host_windows: bool,
    /// The request card that had focus when the Commander opened (or has it now).
    pub focused_card: Option<&'a ItemId>,
    /// How many rows are selected in the sidebar (D138).
    pub list_selected: usize,
}

/// Whether a command can be used now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Avail {
    /// Yes.
    Enabled,
    /// No, and why (shown on the row).
    Disabled(String),
    /// Not listed (it would only be noise).
    Hidden,
}

impl Avail {
    /// It can run.
    pub fn enabled(&self) -> bool {
        *self == Avail::Enabled
    }
}

fn off(why: &str) -> Avail {
    Avail::Disabled(why.to_owned())
}

fn when(ok: bool, why: &str) -> Avail {
    if ok { Avail::Enabled } else { off(why) }
}

/// From a thread menu entry: enabled, or its reason; hidden when the menu does not offer it.
fn menu_avail(ctx: &Ctx<'_>, key: &ThreadKey, actions: &[MenuAction]) -> Avail {
    let entries = thread_menu(ctx.state, key, ctx.now);
    match entries.iter().filter_map(|e| e.item()).find(|i| actions.contains(&i.action)) {
        Some(i) if i.enabled => Avail::Enabled,
        Some(i) => off(i.reason.unwrap_or("Not available for this thread")),
        None => Avail::Hidden,
    }
}

/// The open thread's or draft's control row.
pub fn control_row(ctx: &Ctx<'_>) -> Option<ControlRow> {
    if let Some(k) = ctx.thread {
        return match thread_row(ctx.state, k) {
            ControlRowView::Row(r) => Some(r),
            ControlRowView::Hidden => None,
        };
    }
    ctx.draft.and_then(|id| draft_row_of(ctx.state, id))
}

/// The pending request items of the open thread.
fn pending<'a>(ctx: &Ctx<'a>) -> Vec<&'a yaac_protocol::Item> {
    let Some(k) = ctx.thread else { return Vec::new() };
    ctx.state.transcripts.get(k).map_or_else(Vec::new, |t| {
        t.items().iter().filter(|i| matches!(&i.body, ItemBody::Request(r) if r.state == RequestState::Pending)).collect()
    })
}

/// The decision of the focused card a request command picks: its id, label and whether one is
/// there.
pub fn card_decision(ctx: &Ctx<'_>, cmd: Cmd) -> Option<(ThreadKey, yaac_protocol::RequestId, yaac_protocol::ApprovalDecision)> {
    use yaac_protocol::{DecisionKind, RequestBody};
    let k = ctx.thread?;
    let id = ctx.focused_card?;
    let item = pending(ctx).into_iter().find(|i| &i.id == id)?;
    let ItemBody::Request(r) = &item.body else { return None };
    let RequestBody::Approval(a) = &r.body else { return None };
    let want: &[DecisionKind] = match cmd {
        Cmd::AllowOnce => &[DecisionKind::Allow],
        Cmd::AllowSession => &[DecisionKind::AllowSession, DecisionKind::AllowAlways],
        Cmd::Deny => &[DecisionKind::Deny, DecisionKind::Abort],
        _ => return None,
    };
    let d = want.iter().find_map(|w| a.decisions.iter().find(|d| d.kind == *w))?;
    Some((k.clone(), r.request_id.clone(), d.clone()))
}

/// The open thread's last agent message.
pub fn last_agent_message(state: &AppState, key: &ThreadKey) -> Option<String> {
    let t = state.transcripts.get(key)?;
    t.items().iter().rev().find_map(|i| match &i.body {
        ItemBody::AgentMessage(m) if !m.text.trim().is_empty() => Some(m.text.clone()),
        _ => None,
    })
}

/// The thread that spawned this one, when it is known here.
pub fn parent_of(state: &AppState, key: &ThreadKey) -> Option<ThreadKey> {
    state.summary(key)?.spawned_by.as_ref().and_then(|a| state.key_of(a))
}

/// Whether a command can be used now.
pub fn availability(cmd: Cmd, ctx: &Ctx<'_>) -> Avail {
    let st = ctx.state;
    let thread = ctx.thread;
    let composer = thread.is_some() || ctx.draft.is_some();
    let may_act = thread.is_some_and(|k| st.may_act(k));
    let in_use = "This thread is in use elsewhere";
    // Thread commands are hidden while no thread is open.
    let need_thread = matches!(
        cmd,
        Cmd::NewInProject
            | Cmd::Rename
            | Cmd::CopyId
            | Cmd::Pin
            | Cmd::Mute
            | Cmd::Archive
            | Cmd::Delete
            | Cmd::Dismiss
            | Cmd::DismissAllTemp
            | Cmd::Fork
            | Cmd::MoveThread
            | Cmd::CheckLatest
            | Cmd::ContinueBudget
            | Cmd::Parent
            | Cmd::Related
            | Cmd::Stop
            | Cmd::SendNow
            | Cmd::AllowOnce
            | Cmd::AllowSession
            | Cmd::Deny
            | Cmd::AnswerQuestion
            | Cmd::OpenPr
            | Cmd::CreatePr
            | Cmd::CopyPrLink
            | Cmd::ExpandAll
            | Cmd::CollapseAll
            | Cmd::CopyMessage
    );
    let need_composer = matches!(cmd, Cmd::FocusComposer | Cmd::Send | Cmd::NewLine | Cmd::Attach | Cmd::Model | Cmd::Effort | Cmd::Mode | Cmd::Agents | Cmd::Fast);
    if (need_thread && thread.is_none()) || (need_composer && !composer) {
        return Avail::Hidden;
    }
    let key = thread.cloned();
    let summary = key.as_ref().and_then(|k| st.summary(k));
    let row = || control_row(ctx);
    match cmd {
        Cmd::NeedsNext => {
            let map = AttentionMap::compute(st);
            when(!crate::sidebar::needs_you(st, &map, ctx.now).is_empty(), "Nothing needs you")
        }
        Cmd::NextUnseen => when(st.threads.keys().any(|k| st.is_unseen(k) && Some(k) != thread), "No unseen finished thread"),
        Cmd::NextThread | Cmd::PrevThread => when(!st.threads.is_empty(), "No threads yet"),
        Cmd::Back => when(ctx.can_back, "Nowhere to go back to"),
        Cmd::Forward => when(ctx.can_forward, "Nowhere to go forward to"),
        Cmd::History => when(ctx.can_back || ctx.can_forward, "No other place visited yet"),
        Cmd::Parent => match summary.and_then(|s| s.spawned_by.as_ref()) {
            None => off("A person started this thread"),
            Some(_) => when(key.as_ref().and_then(|k| parent_of(st, k)).is_some(), "The parent thread is not known here"),
        },
        Cmd::Related => when(!related(ctx).is_empty(), "This thread has no related agents"),
        Cmd::FindThread => when(!st.threads.is_empty(), "No threads yet"),
        Cmd::NewThread => when(st.targets.iter().any(|t| t.connected()), "No target is connected"),
        Cmd::NewInProject => when(key.as_ref().is_some_and(|k| st.target(&k.target).is_some_and(|t| t.connected())), "This thread's target is offline"),
        Cmd::Rename => key.map_or(Avail::Hidden, |k| match menu_avail(ctx, &k, &[MenuAction::Rename]) {
            Avail::Enabled if !may_act => off(in_use),
            a => a,
        }),
        Cmd::CopyId => Avail::Enabled,
        Cmd::Pin => key.map_or(Avail::Hidden, |k| match menu_avail(ctx, &k, &[MenuAction::Pin, MenuAction::Unpin]) {
            Avail::Hidden => off("An archived thread is not pinned"),
            a => a,
        }),
        Cmd::Mute => key.map_or(Avail::Hidden, |k| match menu_avail(ctx, &k, &[MenuAction::Mute, MenuAction::Unmute]) {
            Avail::Hidden => off("An archived thread is not muted"),
            a => a,
        }),
        Cmd::Archive => key.map_or(Avail::Hidden, |k| menu_avail(ctx, &k, &[MenuAction::Archive, MenuAction::Unarchive])),
        Cmd::Delete => key.map_or(Avail::Hidden, |k| menu_avail(ctx, &k, &[MenuAction::Delete])),
        Cmd::Dismiss => key.map_or(Avail::Hidden, |k| match menu_avail(ctx, &k, &[MenuAction::Dismiss, MenuAction::ShowAgain]) {
            Avail::Hidden => off("Only a thread outside any project can be dismissed"),
            a => a,
        }),
        Cmd::DismissAllTemp => when(key.as_ref().is_some_and(|k| st.project_of(k).is_none()), "This thread is in a project"),
        Cmd::MoveThread => key.map_or(Avail::Hidden, |k| menu_avail(ctx, &k, &[MenuAction::Move])),
        Cmd::Fork => when(key.as_ref().is_some_and(|k| crate::fork::default_choice(st, k).is_some()), "No harness here can fork this thread"),
        Cmd::CheckLatest => {
            let unknown = key.as_ref().and_then(|k| st.transcripts.get(k)).is_some_and(|t| {
                t.items().iter().any(|i| matches!(&i.body, ItemBody::Request(r) if r.state == RequestState::Unknown))
            });
            when(!may_act || unknown, "Only for a thread in use elsewhere, or an answer whose outcome is unknown")
        }
        Cmd::ContinueBudget => {
            let exhausted = key.as_ref().and_then(|k| crate::agents::root_of(st, k)).and_then(|r| st.budgets.get(&r)).is_some_and(|b| b.exhausted);
            when(exhausted, "No orchestration here is paused on its budget")
        }
        Cmd::FocusComposer | Cmd::Attach | Cmd::Send | Cmd::NewLine | Cmd::Paste => {
            if thread.is_some() && !may_act { off(in_use) } else { Avail::Enabled }
        }
        Cmd::Stop | Cmd::SendNow => {
            if !may_act {
                off(in_use)
            } else {
                when(summary.is_some_and(|s| s.state == ThreadState::Running), "No turn is running")
            }
        }
        Cmd::Model => match row() {
            None => off(in_use),
            Some(r) => when(r.model.as_ref().is_some_and(|m| m.changeable), "This harness cannot switch models in a thread"),
        },
        Cmd::Effort => match row() {
            None => off(in_use),
            Some(r) => when(r.effort.is_some(), "This model has no effort levels to choose"),
        },
        Cmd::Mode => match row() {
            None => off(in_use),
            Some(r) => when(r.mode.is_some(), "This harness has no approval modes"),
        },
        Cmd::Agents => match row() {
            None => off(in_use),
            Some(r) => when(r.agents.is_some(), "Only for a thread a person started, on a harness with agent tools"),
        },
        Cmd::Fast => match row() {
            None => off(in_use),
            Some(r) => when(r.fast.is_some(), "This model has no fast mode"),
        },
        Cmd::AllowOnce | Cmd::AllowSession | Cmd::Deny => {
            if pending(ctx).is_empty() {
                off("No request is waiting here")
            } else {
                when(card_decision(ctx, cmd).is_some(), "Focus the request card first (Ctrl+J)")
            }
        }
        Cmd::AnswerQuestion => when(
            pending(ctx).iter().any(|i| matches!(&i.body, ItemBody::Request(r) if matches!(r.body, yaac_protocol::RequestBody::Question(_)))),
            "No question is waiting here",
        ),
        Cmd::OpenPr | Cmd::CopyPrLink => match key.as_ref().and_then(|k| crate::pr::badge_of(st, k)) {
            None => off("This thread's branch has no pull request"),
            Some(b) if !b.enabled => Avail::Disabled(b.reason.unwrap_or_default()),
            Some(b) => when(b.primary.is_some_and(|p| !p.url.is_empty()), "This branch has no pull request yet"),
        },
        Cmd::CreatePr => match key.as_ref().and_then(|k| crate::pr::badge_of(st, k)) {
            None => off("Pull requests are not available for this thread"),
            Some(b) if !b.enabled => Avail::Disabled(b.reason.unwrap_or_default()),
            Some(b) => when(b.can_create, "Pull requests cannot be created from this target"),
        },
        Cmd::Dock => {
            let map = AttentionMap::compute(st);
            when(key.as_ref().and_then(|k| agents_panel(st, &map, k)).is_some_and(|p| p.all().next().is_some()), "This thread has no agents panel")
        }
        Cmd::ZoomIn => when(ctx.zoom.0 < ctx.zoom.2, "Already at the largest size"),
        Cmd::ZoomOut => when(ctx.zoom.0 > ctx.zoom.1, "Already at the smallest size"),
        Cmd::ZoomReset => Avail::Enabled,
        Cmd::ExpandAll | Cmd::CollapseAll => when(key.as_ref().is_some_and(|k| st.transcripts.contains_key(k)), "This thread is not loaded yet"),
        Cmd::CopyMessage => when(key.as_ref().and_then(|k| last_agent_message(st, k)).is_some(), "No agent message in this thread yet"),
        Cmd::Retry => when(st.targets.iter().any(|t| t.entry.enabled && !t.connected()), "Every target is connected"),
        Cmd::StartWsl => {
            if ctx.host_windows {
                when(st.targets.iter().any(|t| st.stopped_distro(&t.entry.id).is_some()), "No WSL distribution is stopped")
            } else {
                Avail::Hidden
            }
        }
        Cmd::AddWsl => if ctx.host_windows { Avail::Enabled } else { Avail::Hidden },
        Cmd::TargetEnable => when(!st.targets.is_empty(), "No targets yet"),
        Cmd::RefreshProviders => when(st.targets.iter().any(|t| t.connected()), "No target is connected"),
        Cmd::HideToTray => when(ctx.has_tray, "There is no tray icon on this system"),
        Cmd::ArchiveSelected | Cmd::MuteSelected | Cmd::MoveSelected | Cmd::ClearSelection => when(ctx.list_selected > 0, "Nothing is selected in the sidebar (Ctrl+click rows)"),
        Cmd::Commander
        | Cmd::SearchThreads
        | Cmd::Settings
        | Cmd::SettingsPage
        | Cmd::ToggleSidebar
        | Cmd::Grouping
        | Cmd::Order
        | Cmd::UnseenFirst
        | Cmd::NeedsFirst
        | Cmd::RunningOnly
        | Cmd::ShowDismissed
        | Cmd::ShowArchived
        | Cmd::Theme
        | Cmd::CopySelection
        | Cmd::AddSsh
        | Cmd::Quit
        | Cmd::Renderer => Avail::Enabled,
    }
}

/// The name as it reads now ("Unpin" on a pinned thread).
pub fn title(cmd: Cmd, ctx: &Ctx<'_>) -> String {
    let st = ctx.state;
    let k = ctx.thread;
    let d = def_of(cmd);
    let pick = |on: bool, a: &str, b: &str| if on { a.to_owned() } else { b.to_owned() };
    match cmd {
        Cmd::Pin => k.map_or_else(|| d.name.to_owned(), |k| pick(st.is_pinned(k), "Unpin", "Pin")),
        Cmd::Mute => k.map_or_else(|| d.name.to_owned(), |k| pick(st.is_muted(k, ctx.now), "Unmute", "Mute…")),
        Cmd::Archive => k.map_or_else(|| d.name.to_owned(), |k| pick(st.summary(k).is_some_and(|s| s.archived), "Unarchive", "Archive…")),
        Cmd::Dismiss => k.map_or_else(|| d.name.to_owned(), |k| pick(st.is_dismissed(k), "Show again", "Dismiss")),
        Cmd::ArchiveSelected if ctx.list_selected > 0 => format!("Archive selected ({})…", ctx.list_selected),
        Cmd::MuteSelected if ctx.list_selected > 0 => format!("Mute selected ({})…", ctx.list_selected),
        Cmd::MoveSelected if ctx.list_selected > 0 => format!("Move selected ({})…", ctx.list_selected),
        Cmd::Dock => pick(ctx.docked, "Undock the agents panel", "Dock the agents panel"),
        Cmd::Fast => match control_row(ctx).and_then(|r| r.fast) {
            Some(f) => pick(f.on, "Fast mode off", "Fast mode on"),
            None => d.name.to_owned(),
        },
        _ => d.name.to_owned(),
    }
}

/// A toggle's current state (a tick on the row).
pub fn checked(cmd: Cmd, ctx: &Ctx<'_>) -> Option<bool> {
    match cmd {
        Cmd::UnseenFirst => Some(ctx.ordering.unseen_first),
        Cmd::NeedsFirst => Some(ctx.ordering.needs_first),
        Cmd::RunningOnly => Some(ctx.ordering.running_only),
        Cmd::ShowDismissed => Some(ctx.grouping.show_dismissed),
        Cmd::ShowArchived => Some(ctx.archived_open),
        _ => None,
    }
}

// ── Steps ───────────────────────────────────────────────────────────────────────────────

/// The grouping checkboxes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GroupField {
    /// By machine.
    Machine,
    /// By project.
    Project,
    /// By worktree.
    Worktree,
}

/// What a step's row chooses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StepValue {
    /// Open this thread.
    Thread(ThreadKey),
    /// Move this many places in the history.
    Nav(isize),
    /// Open this settings page.
    Page(Page),
    /// This model of this instance.
    Model(InstanceId, ModelId),
    /// This effort.
    Effort(EffortId),
    /// This mode.
    Mode(ModeId),
    /// This agents policy.
    Agents(AgentsPolicy),
    /// Mute for this many seconds (`None`: until unmuted).
    MuteFor(Option<i64>),
    /// "Until a date…": the Mute dialog.
    MuteDate,
    /// Move the open thread here (D139): the Move dialog opens with it chosen.
    MoveTo(crate::moving::Destination),
    /// This order.
    Order(Order),
    /// Toggle this grouping.
    Grouping(GroupField),
    /// This theme.
    Theme(ThemePref),
    /// Retry (or start) this target.
    Retry(TargetId),
    /// Enable (`true`) or disable this target.
    TargetEnabled(TargetId, bool),
    /// Read this target's providers again.
    Refresh(TargetId),
}

/// One row of a step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StepItem {
    /// The label.
    pub label: String,
    /// A secondary line (machine, range…).
    pub detail: Option<String>,
    /// What it chooses.
    pub value: StepValue,
    /// It is the current choice (ticked).
    pub current: bool,
    /// Why it cannot be chosen.
    pub reason: Option<String>,
}

fn item(label: impl Into<String>, detail: Option<String>, value: StepValue, current: bool) -> StepItem {
    StepItem { label: label.into(), detail, value, current, reason: None }
}

/// The related agents of the open thread that can be opened: parent, workers, contacts.
pub fn related(ctx: &Ctx<'_>) -> Vec<(ThreadKey, String, String)> {
    let Some(k) = ctx.thread else { return Vec::new() };
    let map = AttentionMap::compute(ctx.state);
    let Some(panel) = agents_panel(ctx.state, &map, k) else { return Vec::new() };
    let parent = match &panel.spawned_by {
        SpawnedBy::Agent(p) => p.key.clone(),
        SpawnedBy::You => None,
    };
    let mut seen = BTreeSet::new();
    panel
        .all()
        .filter_map(|r| {
            let key = r.key.clone()?;
            if !seen.insert(key.clone()) {
                return None;
            }
            let role = if Some(&key) == parent.as_ref() {
                "parent"
            } else if panel.workers.iter().any(|w| w.key.as_ref() == Some(&key)) {
                "worker"
            } else {
                "contact"
            };
            Some((key, r.title.clone(), format!("{role} · {} · {}", r.target, r.activity)))
        })
        .collect()
}

/// The mute durations offered in the Commander; "Until a date…" opens the dialog.
pub const MUTE_DURATIONS: &[(&str, Option<i64>)] =
    &[("For 1 hour", Some(3600)), ("For 4 hours", Some(4 * 3600)), ("For 1 day", Some(86_400)), ("For 1 week", Some(7 * 86_400)), ("Until I unmute it", None)];

/// The rows of a step, before the query filters them.
pub fn step_items(kind: StepKind, ctx: &Ctx<'_>, history: &[(isize, String, Option<String>)]) -> Vec<StepItem> {
    let st = ctx.state;
    match kind {
        StepKind::Thread => {
            let mut keys: Vec<&ThreadKey> = st.threads.keys().collect();
            keys.sort_by_key(|k| std::cmp::Reverse(st.summary(k).map(|s| s.updated_at.clone()).unwrap_or_default()));
            keys.into_iter()
                .filter_map(|k| {
                    let s = st.summary(k)?;
                    let title = if s.title.trim().is_empty() { k.thread.to_string() } else { s.title.clone() };
                    Some(item(title, Some(format!("{} · {}", st.target_name(&k.target), s.native_id.clone().unwrap_or_else(|| k.thread.to_string()))), StepValue::Thread(k.clone()), Some(k) == ctx.thread))
                })
                .collect()
        }
        StepKind::History => history.iter().map(|(n, t, m)| item(t.clone(), m.clone(), StepValue::Nav(*n), false)).collect(),
        StepKind::Related => related(ctx).into_iter().map(|(k, t, d)| item(t, Some(d), StepValue::Thread(k), false)).collect(),
        StepKind::Page => Page::ALL.iter().map(|p| item(format!("Settings: {}", p.label()), None, StepValue::Page(*p), false)).collect(),
        StepKind::Model => {
            let Some(m) = control_row(ctx).and_then(|r| r.model) else { return Vec::new() };
            let mut out = Vec::new();
            for g in &m.groups {
                if !g.enabled {
                    out.push(StepItem {
                        label: g.name.clone(),
                        detail: None,
                        value: StepValue::Model(g.instance.clone(), ModelId::new("")),
                        current: false,
                        reason: g.reason.clone(),
                    });
                    continue;
                }
                for pm in &g.models {
                    let mut detail = vec![g.name.clone()];
                    if pm.is_default {
                        detail.push("default".into());
                    }
                    if let Some(r) = &pm.effort_range {
                        detail.push(r.clone());
                    }
                    out.push(StepItem {
                        label: pm.name.clone(),
                        detail: Some(detail.join(" · ")),
                        value: StepValue::Model(g.instance.clone(), pm.id.clone()),
                        current: pm.selected,
                        reason: (!pm.enabled).then(|| "Not available".to_owned()),
                    });
                }
            }
            out
        }
        StepKind::Effort => control_row(ctx)
            .and_then(|r| r.effort)
            .map(|c| c.options.iter().map(|(id, l, d)| item(l.clone(), d.then(|| "default".into()), StepValue::Effort(id.clone()), c.current.as_ref() == Some(id))).collect())
            .unwrap_or_default(),
        StepKind::Mode => control_row(ctx)
            .and_then(|r| r.mode)
            .map(|c| c.options.iter().map(|(id, l, d)| item(l.clone(), d.then(|| "default".into()), StepValue::Mode(id.clone()), c.current.as_ref() == Some(id))).collect())
            .unwrap_or_default(),
        StepKind::Agents => control_row(ctx)
            .and_then(|r| r.agents)
            .map(|c| c.options.iter().map(|(id, l, _)| item(l.clone(), None, StepValue::Agents(*id), c.current.as_ref() == Some(id))).collect())
            .unwrap_or_default(),
        StepKind::MoveTo => {
            let Some(k) = ctx.thread else { return Vec::new() };
            let now = crate::moving::current(st, k);
            let mut out = Vec::new();
            for p in crate::moving::projects_on(st, &k.target) {
                let own = crate::moving::Destination { project: p.wire_id.clone(), worktree: None };
                let here = now.as_ref().is_some_and(|n| n.project == p.wire_id && n.worktree.as_ref().is_none_or(|w| p.repo.as_ref().is_some_and(|r| &r.root == w)));
                out.push(item(p.name.clone(), Some(p.folder.clone()), StepValue::MoveTo(own), here));
                for w in crate::moving::worktrees(st, p).into_iter().filter(|w| !w.own) {
                    let dest = crate::moving::Destination { project: p.wire_id.clone(), worktree: Some(w.root.clone()) };
                    let here = now.as_ref() == Some(&dest);
                    out.push(item(format!("{} · {}", p.name, w.label), Some(w.root), StepValue::MoveTo(dest), here));
                }
            }
            out
        }
        StepKind::Mute => {
            let mut out: Vec<StepItem> = MUTE_DURATIONS.iter().map(|(l, s)| item(*l, None, StepValue::MuteFor(*s), false)).collect();
            out.push(item("Until a date…", Some("Opens the Mute dialog".into()), StepValue::MuteDate, false));
            out
        }
        StepKind::Order => Order::ALL.iter().map(|o| item(o.label(), None, StepValue::Order(*o), ctx.ordering.order == *o)).collect(),
        StepKind::Grouping => [
            ("Machine", GroupField::Machine, ctx.grouping.machine),
            ("Project", GroupField::Project, ctx.grouping.project),
            ("Worktree", GroupField::Worktree, ctx.grouping.worktree),
        ]
        .into_iter()
        .map(|(l, f, on)| item(l, Some(if on { "on · select to turn off".into() } else { "off · select to turn on".into() }), StepValue::Grouping(f), on))
        .collect(),
        StepKind::Theme => [("System", ThemePref::System), ("Light", ThemePref::Light), ("Dark", ThemePref::Dark)]
            .into_iter()
            .map(|(l, t)| item(l, None, StepValue::Theme(t), ctx.theme == t))
            .collect(),
        StepKind::Retry => st
            .targets
            .iter()
            .filter(|t| t.entry.enabled && !t.connected())
            .map(|t| item(t.entry.name.to_string(), t.transport_word().map(str::to_owned), StepValue::Retry(t.entry.id.clone()), false))
            .collect(),
        StepKind::StartWsl => st
            .targets
            .iter()
            .filter_map(|t| st.stopped_distro(&t.entry.id).map(|d| item(d, Some(t.entry.name.to_string()), StepValue::Retry(t.entry.id.clone()), false)))
            .collect(),
        StepKind::TargetEnable => st
            .targets
            .iter()
            .map(|t| {
                let on = t.entry.enabled;
                item(t.entry.name.to_string(), Some(if on { "on · select to disable".into() } else { "off · select to enable".into() }), StepValue::TargetEnabled(t.entry.id.clone(), !on), on)
            })
            .collect(),
        StepKind::RefreshProviders => st
            .targets
            .iter()
            .map(|t| StepItem {
                label: t.entry.name.to_string(),
                detail: t.transport_word().map(str::to_owned),
                value: StepValue::Refresh(t.entry.id.clone()),
                current: false,
                reason: (!t.connected()).then(|| "Offline".to_owned()),
            })
            .collect(),
    }
}

// ── Search ──────────────────────────────────────────────────────────────────────────────

/// How well `query` matches `text`: 0 for no match. Every word of the query must match, as a
/// substring (best, more at a word start) or as letters in order (fuzzy).
pub fn fuzzy_score(query: &str, text: &str) -> u32 {
    let text = text.to_lowercase();
    let mut total = 0;
    for word in query.to_lowercase().split_whitespace() {
        let s = word_score(word, &text);
        if s == 0 {
            return 0;
        }
        total += s;
    }
    total
}

fn word_score(word: &str, text: &str) -> u32 {
    if let Some(pos) = text.find(word) {
        let at_start = pos == 0 || !text[..pos].chars().next_back().is_some_and(char::is_alphanumeric);
        let len = u32::try_from(word.chars().count()).unwrap_or(0);
        return 100 + if at_start { 60 } else { 0 } + len * 4 - u32::try_from(pos.min(40)).unwrap_or(0);
    }
    // Letters in order, more for word starts and runs.
    let mut score = 0;
    let mut chars = text.char_indices().peekable();
    let mut prev_matched = false;
    let mut prev_char: Option<char> = None;
    for w in word.chars() {
        let mut found = false;
        for (_, c) in chars.by_ref() {
            let start = prev_char.is_none_or(|p| !p.is_alphanumeric());
            prev_char = Some(c);
            if c == w {
                score += 1 + if start { 3 } else { 0 } + if prev_matched { 2 } else { 0 };
                prev_matched = true;
                found = true;
                break;
            }
            prev_matched = false;
        }
        if !found {
            return 0;
        }
    }
    // Letters scattered through unrelated words are noise: most letters must start a word or
    // follow the one before.
    let len = u32::try_from(word.chars().count()).unwrap_or(0);
    if score * 2 < len * 5 { 0 } else { score }
}

/// The score of a command for a query: its name counts most, then aliases, then the
/// explanation.
pub fn command_score(d: &CommandDef, name: &str, query: &str) -> u32 {
    let n = fuzzy_score(query, name).max(fuzzy_score(query, d.name)) * 10;
    let a = d.aliases.iter().map(|a| fuzzy_score(query, a)).max().unwrap_or(0) * 8;
    let e = fuzzy_score(query, d.explain) * 5;
    let joined = format!("{name} {} {}", d.aliases.join(" "), d.explain);
    // Words spread over the name, aliases and explanation: each must appear as typed.
    let lower = joined.to_lowercase();
    let all = if query.to_lowercase().split_whitespace().all(|w| lower.contains(w)) { fuzzy_score(query, &joined) * 4 } else { 0 };
    n.max(a).max(e).max(all)
}

/// The most recent commands kept.
pub const RECENT_MAX: usize = 8;

/// Records a use: most recent first, no repeats.
pub fn note_recent(recent: &mut Vec<String>, id: &str) {
    recent.retain(|r| r != id);
    recent.insert(0, id.to_owned());
    recent.truncate(RECENT_MAX);
}

/// One row of the Commander's list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListRow {
    /// The command.
    pub cmd: Cmd,
    /// Its name now.
    pub name: String,
    /// The group heading to draw above it (only when the query is empty).
    pub heading: Option<&'static str>,
    /// Enabled or why not.
    pub avail: Avail,
    /// The toggle state.
    pub checked: Option<bool>,
}

/// The Commander's list: with no query, recently used commands under "Recent", then every
/// group; with a query, matches ranked by score, with a bonus for recent use. Hidden commands
/// are left out.
pub fn list(ctx: &Ctx<'_>, query: &str, recent: &[String]) -> Vec<ListRow> {
    let shown = |d: &CommandDef| {
        let avail = availability(d.cmd, ctx);
        (avail != Avail::Hidden).then(|| (avail, title(d.cmd, ctx)))
    };
    let q = query.trim();
    let mut out = Vec::new();
    if q.is_empty() {
        let mut first = true;
        for id in recent {
            if let Some(d) = by_id(id)
                && let Some((avail, name)) = shown(d)
            {
                out.push(ListRow { cmd: d.cmd, name, heading: first.then_some("Recent"), avail, checked: checked(d.cmd, ctx) });
                first = false;
            }
        }
        for g in Group::ALL {
            let mut first = true;
            for d in COMMANDS.iter().filter(|d| d.group == g && !recent.iter().any(|r| r == d.id)) {
                if let Some((avail, name)) = shown(d) {
                    out.push(ListRow { cmd: d.cmd, name, heading: first.then_some(g.label()), avail, checked: checked(d.cmd, ctx) });
                    first = false;
                }
            }
        }
        return out;
    }
    let mut scored: Vec<(u32, usize, ListRow)> = Vec::new();
    for (i, d) in COMMANDS.iter().enumerate() {
        let Some((avail, name)) = shown(d) else { continue };
        let s = command_score(d, &name, q);
        if s == 0 {
            continue;
        }
        let bonus = recent.iter().position(|r| r == d.id).map_or(0, |p| 4000 - u32::try_from(p).unwrap_or(0) * 300);
        // Disabled commands sort after enabled ones of the same strength.
        let pen = if avail.enabled() { 0 } else { 200 };
        scored.push(((s + bonus).saturating_sub(pen), i, ListRow { cmd: d.cmd, name, heading: None, avail, checked: checked(d.cmd, ctx) }));
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    out.extend(scored.into_iter().map(|(_, _, r)| r));
    out
}

/// A step's rows filtered by the query, best first (the order kept when it is empty).
pub fn filter_step(items: Vec<StepItem>, query: &str) -> Vec<StepItem> {
    let q = query.trim();
    if q.is_empty() {
        return items;
    }
    let mut scored: Vec<(u32, usize, StepItem)> = items
        .into_iter()
        .enumerate()
        .filter_map(|(i, it)| {
            let s = fuzzy_score(q, &it.label) * 2 + it.detail.as_deref().map_or(0, |d| fuzzy_score(q, d));
            (s > 0).then_some((s, i, it))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, _, it)| it).collect()
}

// ── Bindings ────────────────────────────────────────────────────────────────────────────

/// The keys in effect: the defaults with the user's overrides.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Bindings {
    /// What the user changed (stored).
    pub overrides: Overrides,
}

/// The default chords of a command.
pub fn default_chords(d: &CommandDef) -> Vec<Chord> {
    d.keys.iter().filter_map(|k| Chord::parse(k)).collect()
}

impl Bindings {
    /// The chords of a command now.
    pub fn chords(&self, cmd: Cmd) -> Vec<Chord> {
        let d = def_of(cmd);
        match self.overrides.0.get(d.id) {
            Some(cs) if d.scope == Scope::Global => cs.clone(),
            _ => default_chords(d),
        }
    }

    /// The command a pressed chord runs (global commands only).
    pub fn resolve(&self, chord: &Chord) -> Option<Cmd> {
        COMMANDS.iter().filter(|d| d.scope == Scope::Global).find(|d| self.chords(d.cmd).contains(chord)).map(|d| d.cmd)
    }

    /// Another command that has this chord (a local one too: they would compete where it
    /// works).
    pub fn conflict(&self, chord: &Chord, for_cmd: Cmd) -> Option<Cmd> {
        COMMANDS.iter().filter(|d| d.cmd != for_cmd).find(|d| self.chords(d.cmd).contains(chord)).map(|d| d.cmd)
    }

    /// Set a command's chords; only a difference from the defaults is kept.
    pub fn set(&mut self, cmd: Cmd, chords: Vec<Chord>) {
        let d = def_of(cmd);
        if d.scope != Scope::Global {
            return;
        }
        if chords == default_chords(d) {
            self.overrides.0.remove(d.id);
        } else {
            self.overrides.0.insert(d.id.to_owned(), chords);
        }
    }

    /// Give `chord` to `cmd` (replacing its keys). With `replace`, the command that had it
    /// loses it; without, nothing changes when another has it. Returns whether it was set.
    pub fn assign(&mut self, cmd: Cmd, chord: Chord, replace: bool) -> bool {
        if def_of(cmd).scope != Scope::Global {
            return false;
        }
        if let Some(other) = self.conflict(&chord, cmd) {
            if !replace || def_of(other).scope != Scope::Global {
                return false;
            }
            let rest: Vec<Chord> = self.chords(other).into_iter().filter(|c| c != &chord).collect();
            self.set(other, rest);
        }
        self.set(cmd, vec![chord]);
        true
    }

    /// Remove a command's keys.
    pub fn clear(&mut self, cmd: Cmd) {
        self.set(cmd, Vec::new());
    }

    /// Back to the default keys for one command.
    pub fn reset(&mut self, cmd: Cmd) {
        self.overrides.0.remove(def_of(cmd).id);
    }

    /// Back to every default.
    pub fn reset_all(&mut self) {
        self.overrides.0.clear();
    }

    /// The command's keys differ from its defaults.
    pub fn is_changed(&self, cmd: Cmd) -> bool {
        self.overrides.0.contains_key(def_of(cmd).id)
    }

    /// "Ctrl+K" — the text shown beside a command, empty when it has none.
    pub fn label(&self, cmd: Cmd) -> String {
        crate::keys::format_chords(&self.chords(cmd))
    }
}

/// A note about a chord the system may take before the window sees it.
pub fn os_note(chord: &Chord) -> Option<&'static str> {
    (chord == &Chord::key(true, false, false, "Space"))
        .then_some("Some keyboard layouts (Chinese, Japanese, Korean input methods) switch input with Ctrl+Space before YAAC sees it. If it does nothing, choose another key.")
}

// ── Moving through the list ─────────────────────────────────────────────────────────────

/// The thread `delta` rows from `current` in the sidebar's order (wrapping at neither end);
/// the first (or last) when nothing is open.
pub fn step_in_list(order: &[ThreadKey], current: Option<&ThreadKey>, delta: isize) -> Option<ThreadKey> {
    if order.is_empty() {
        return None;
    }
    let Some(i) = current.and_then(|c| order.iter().position(|k| k == c)) else {
        return if delta >= 0 { order.first().cloned() } else { order.last().cloned() };
    };
    let j = i.checked_add_signed(delta)?;
    order.get(j).cloned()
}

/// The next unseen finished thread after `current` in the sidebar's order, wrapping.
pub fn next_unseen(state: &AppState, order: &[ThreadKey], current: Option<&ThreadKey>) -> Option<ThreadKey> {
    let start = current.and_then(|c| order.iter().position(|k| k == c)).map_or(0, |i| i + 1);
    (0..order.len()).map(|n| &order[(start + n) % order.len()]).find(|k| state.is_unseen(k) && Some(*k) != current).cloned()
}

