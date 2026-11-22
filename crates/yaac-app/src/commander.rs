//! The Commander (D130) and the window's one keyboard dispatcher (D131).
//!
//! Every key the window handles globally is looked up in the command table
//! ([`yaac_core::commands`]) through the user's bindings; the scattered key checks are gone.
//! Keys that belong to one place stay there: Enter, Shift+Enter, Ctrl+Enter and Esc in the
//! composer, Enter and Esc on the focused request card, Ctrl+V and Ctrl+C in fields.
//!
//! A plain chord (F2, Delete) never fires while a text field has focus; a chord with Ctrl or
//! Alt does, except the keys text editing itself uses (Ctrl+Delete, Ctrl+A, Ctrl+←…).
//!
//! The Commander is a centred overlay: a search field over the list, rows in the shared menu
//! style (name, its explanation in the secondary colour, the key on the right). Commands that
//! take an argument continue in it as a sub-list. It draws only when a frame happens anyway and
//! asks for none of its own, so it costs nothing while it sits open.

use eframe::egui::{self, Event, Key, Modifiers, Sense, pos2, vec2};
use yaac_core::commands::{self as cmds, Avail, Cmd, Ctx, StepKind, StepValue};
use yaac_core::history::Place;
use yaac_core::keys::{Chord, MouseButton};
use yaac_core::prefs::ThemePref;
use yaac_core::state::Selection;
use yaac_protocol::ItemId;

use crate::app::{Action, YaacApp};
use crate::theme::{self, ThemeChoice, size};
use crate::widgets;

/// The smallest and largest interface size (points of body text), as in Appearance.
pub(crate) const ZOOM_MIN: f32 = 11.0;
pub(crate) const ZOOM_MAX: f32 = 18.0;
const ZOOM_STEP: f32 = 0.5;

/// The Commander while it is open.
#[derive(Clone, Debug, Default)]
pub(crate) struct CommanderUi {
    /// What is typed.
    pub query: String,
    /// The highlighted row.
    pub sel: usize,
    /// The command whose choice is being made, and which.
    pub step: Option<(Cmd, StepKind)>,
    /// The request card that had focus when it opened (Allow and Deny act on it only).
    pub card: Option<ItemId>,
    /// Put the caret in the field on the next frame.
    pub focus: bool,
    /// Scroll the highlighted row into view on the next frame.
    pub reveal: bool,
    /// The keys it took this frame, before anything else could (Settings and the composer
    /// also listen for Esc and Enter).
    keys: Keys,
}

/// Arrows, Enter, Esc and Backspace-on-empty, taken by the dispatcher.
#[derive(Clone, Copy, Debug, Default)]
struct Keys {
    down: bool,
    up: bool,
    enter: bool,
    esc: bool,
    back: bool,
}

impl CommanderUi {
    pub(crate) fn at(step: Option<(Cmd, StepKind)>, card: Option<ItemId>) -> Self {
        Self { query: String::new(), sel: 0, step, card, focus: true, reveal: true, keys: Keys::default() }
    }
}

/// "Collapse the sidebar (Ctrl+B)": a hover text with the command's key as bound now.
pub(crate) fn tip(app: &YaacApp, text: &str, cmd: Cmd) -> String {
    let k = app.ui.bindings.label(cmd);
    if k.is_empty() { text.to_owned() } else { format!("{text} ({k})") }
}

/// The chord an egui key press is.
pub(crate) fn chord_of(key: Key, m: Modifiers) -> Chord {
    Chord::key(m.command || m.ctrl, m.shift, m.alt, key.name())
}

/// The window's view of things the command table needs.
pub(crate) fn with_ctx<R>(app: &YaacApp, card: Option<&ItemId>, f: impl FnOnce(&Ctx<'_>) -> R) -> R {
    let st = &app.client.state;
    let thread = match &st.selection {
        Selection::Thread(k) if !app.ui.settings => Some(k),
        _ => None,
    };
    let draft = match &st.selection {
        Selection::NewThread(id) if !app.ui.settings => Some(*id),
        _ => None,
    };
    let exists = |p: &Place| app.place_exists(p);
    let ctx = Ctx {
        state: st,
        now: app.now(),
        thread,
        draft,
        can_back: app.ui.history.can_back(&exists),
        can_forward: app.ui.history.can_forward(&exists),
        has_tray: app.shell.has_tray(),
        grouping: app.ui.grouping,
        ordering: app.ui.ordering,
        archived_open: app.ui.archived_open,
        docked: app.ui.panel == yaac_core::agents::PanelMode::Docked,
        theme: match app.ui.theme {
            ThemeChoice::System => ThemePref::System,
            ThemeChoice::Dark => ThemePref::Dark,
            ThemeChoice::Light => ThemePref::Light,
        },
        zoom: (app.ui.font_size, ZOOM_MIN, ZOOM_MAX),
        host_windows: app.host_windows,
        focused_card: card,
        list_selected: app.ui.list_sel.len(),
    };
    f(&ctx)
}

/// The threads in the sidebar's order, as listed now.
pub(crate) fn sidebar_order(app: &YaacApp) -> Vec<yaac_core::ThreadKey> {
    let map = yaac_core::attention::AttentionMap::compute(&app.client.state);
    let open = yaac_core::sidebar::Open { muted: app.ui.muted_open, archived: app.ui.archived_open };
    yaac_core::sidebar::sections(&app.client.state, &map, app.ui.grouping, app.ui.ordering, &app.ui.search, open, app.now(), app.zone)
        .into_iter()
        .flat_map(|s| s.rows.into_iter().map(|r| r.key))
        .collect()
}

/// The keyboard, once per frame, before anything is drawn.
pub(crate) fn dispatch(app: &mut YaacApp, ctx: &egui::Context, actions: &mut Vec<Action>) {
    // The request card that has focus (last frame's), for the Commander's Allow and Deny.
    let card = app.ui.card_focus.take();
    // Settings → Keyboard is recording a chord: the next press is that chord.
    if app.ui.set.keys.recording.is_some() {
        crate::keyboard::capture(app, ctx);
        return;
    }
    // Ctrl+V arrives as a key (`clipboard_shortcuts` is off): see `YaacApp::paste`.
    let paste = ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::V) || i.consume_key(Modifiers::NONE, Key::Paste) || (cfg!(windows) && i.consume_key(Modifiers::SHIFT, Key::Insert)));
    if paste {
        app.paste(ctx);
    }
    let in_text = ctx.text_edit_focused();
    if let Some(c) = &mut app.ui.commander {
        let empty_step = c.query.is_empty() && c.step.is_some();
        c.keys = ctx.input_mut(|i| Keys {
            down: i.consume_key(Modifiers::NONE, Key::ArrowDown),
            up: i.consume_key(Modifiers::NONE, Key::ArrowUp),
            enter: i.consume_key(Modifiers::NONE, Key::Enter),
            esc: i.consume_key(Modifiers::NONE, Key::Escape),
            back: empty_step && i.consume_key(Modifiers::NONE, Key::Backspace),
        });
    }
    let commander_open = app.ui.commander.is_some();
    let bindings = &app.ui.bindings;
    let mut hits: Vec<Cmd> = Vec::new();
    ctx.input_mut(|i| {
        i.events.retain(|e| {
            let (chord, repeat) = match e {
                Event::Key { key, pressed: true, modifiers, repeat, .. } => (chord_of(*key, *modifiers), *repeat),
                Event::PointerButton { button: egui::PointerButton::Extra1, pressed: true, .. } => (Chord::mouse(MouseButton::Back), false),
                Event::PointerButton { button: egui::PointerButton::Extra2, pressed: true, .. } => (Chord::mouse(MouseButton::Forward), false),
                _ => return true,
            };
            let cmd = bindings.resolve(&chord).or_else(|| if chord.shift { bindings.resolve(&chord.without_shift()) } else { None });
            let Some(cmd) = cmd else { return true };
            if in_text && !chord.works_in_text() {
                return true;
            }
            // While the Commander is open its own key closes it; every other key belongs to it.
            if commander_open && cmd != Cmd::Commander {
                return true;
            }
            // Held keys repeat only where repeating means something.
            if repeat && !matches!(cmd, Cmd::NextThread | Cmd::PrevThread | Cmd::ZoomIn | Cmd::ZoomOut) {
                return false;
            }
            hits.push(cmd);
            false
        });
    });
    for cmd in hits {
        if cmd == Cmd::Commander && app.ui.commander.is_some() {
            app.ui.commander = None;
            continue;
        }
        let card = card.clone();
        let avail = with_ctx(app, card.as_ref(), |c| cmds::availability(cmd, c));
        if avail.enabled() {
            run(app, ctx, cmd, card, actions);
        }
    }
    if app.ui.menu.is_some() && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
        app.ui.menu = None;
    }
}

/// Run a command (it is enabled). A command with a choice opens the Commander at it.
pub(crate) fn run(app: &mut YaacApp, ctx: &egui::Context, cmd: Cmd, card: Option<ItemId>, actions: &mut Vec<Action>) {
    let d = cmds::def_of(cmd);
    let thread = app.client.state.selected_thread().cloned().filter(|_| !app.ui.settings);
    // Unmute needs no choice; every other step opens in the Commander.
    let muted = thread.as_ref().is_some_and(|k| app.client.state.is_muted(k, app.now()));
    if let Some(step) = d.step
        && !(cmd == Cmd::Mute && muted)
    {
        app.ui.commander = Some(CommanderUi::at(Some((cmd, step)), card));
        return;
    }
    app.ui.menu = None;
    let st = &app.client.state;
    match cmd {
        Cmd::Commander => app.ui.commander = Some(CommanderUi::at(None, card)),
        Cmd::SearchThreads => {
            if app.ui.sidebar.collapsed {
                app.ui.sidebar.toggle();
            }
            app.ui.focus_search = true;
            if app.ui.settings {
                actions.push(Action::Settings(false));
            }
        }
        Cmd::NeedsNext => actions.push(Action::NeedsNext),
        Cmd::NextUnseen => {
            if let Some(k) = cmds::next_unseen(st, &sidebar_order(app), thread.as_ref()) {
                actions.push(Action::Open(k));
            }
        }
        Cmd::NextThread | Cmd::PrevThread => {
            let delta = if cmd == Cmd::NextThread { 1 } else { -1 };
            if let Some(k) = cmds::step_in_list(&sidebar_order(app), thread.as_ref(), delta) {
                actions.push(Action::Open(k));
            }
        }
        Cmd::Back => actions.push(Action::Nav(-1)),
        Cmd::Forward => actions.push(Action::Nav(1)),
        Cmd::Parent => {
            if let Some(k) = thread.as_ref().and_then(|k| cmds::parent_of(st, k)) {
                actions.push(Action::Open(k));
            }
        }
        Cmd::Settings => actions.push(Action::Settings(true)),
        Cmd::NewThread => actions.push(Action::NewThread),
        Cmd::NewInProject => {
            if let Some(k) = thread {
                actions.push(Action::NewInProject(k));
            }
        }
        Cmd::Rename | Cmd::CopyId => {
            if let Some(k) = thread {
                let a = if cmd == Cmd::Rename { yaac_core::thread_menu::MenuAction::Rename } else { yaac_core::thread_menu::MenuAction::CopyId };
                crate::sidebar::run_menu_action(app, ctx, &k, a, actions);
            }
        }
        Cmd::Pin => {
            if let Some(k) = thread {
                let on = st.is_pinned(&k);
                actions.push(Action::SetPinned(k, !on));
            }
        }
        Cmd::Mute => {
            if let Some(k) = thread {
                actions.push(Action::SetMuted(k, false));
            }
        }
        Cmd::Archive => {
            if let Some(k) = thread {
                let archived = st.summary(&k).is_some_and(|s| s.archived);
                actions.push(if archived { Action::Unarchive(k) } else { Action::OpenArchive(k) });
            }
        }
        Cmd::Delete => {
            if let Some(k) = thread {
                actions.push(Action::OpenDelete(k));
            }
        }
        Cmd::ArchiveSelected => actions.push(Action::OpenBulkArchive(app.ui.list_sel.in_order(&app.ui.list_order))),
        Cmd::MuteSelected => actions.push(Action::OpenBulkMute(app.ui.list_sel.in_order(&app.ui.list_order))),
        Cmd::ClearSelection => actions.push(Action::ClearSelection),
        Cmd::Dismiss => {
            if let Some(k) = thread {
                let d = st.is_dismissed(&k);
                actions.push(Action::Dismiss(k, !d));
            }
        }
        Cmd::DismissAllTemp => {
            if let Some(k) = thread
                && let Some(cwd) = st.summary(&k).map(|s| s.cwd.clone())
            {
                for other in st.threads.keys() {
                    if other.target == k.target && st.project_of(other).is_none() && st.summary(other).is_some_and(|s| s.cwd == cwd) {
                        actions.push(Action::Dismiss(other.clone(), true));
                    }
                }
            }
        }
        Cmd::Fork => {
            if let Some(k) = thread {
                actions.push(Action::OpenFork(k));
            }
        }
        Cmd::TakeOver => {
            if let Some(k) = thread {
                actions.push(Action::TakeOver(k));
            }
        }
        Cmd::CheckLatest => {
            if let Some(k) = thread {
                actions.push(Action::CheckLatest(k));
            }
        }
        Cmd::ContinueBudget => {
            if let Some(r) = thread.as_ref().and_then(|k| yaac_core::agents::root_of(st, k)) {
                actions.push(Action::ContinueBudget(r));
            }
        }
        Cmd::FocusComposer => app.ui.focus_composer = true,
        Cmd::Send | Cmd::SendNow => match (&thread, &st.selection) {
            (Some(k), _) => actions.push(Action::Send(k.clone())),
            (None, Selection::NewThread(id)) => actions.push(Action::StartDraft(*id)),
            _ => {}
        },
        Cmd::NewLine => {
            if let Some(k) = &thread {
                app.client.state.drafts.entry(k.clone()).or_default().text.push('\n');
            } else if let Selection::NewThread(id) = st.selection
                && let Some(d) = app.client.state.new_threads.iter_mut().find(|d| d.id == id)
            {
                d.draft.text.push('\n');
            }
            app.ui.focus_composer = true;
        }
        Cmd::Stop => {
            if let Some(k) = thread {
                actions.push(Action::Stop(k));
            }
        }
        Cmd::Attach => {
            if let Some(d) = app.open_composer() {
                actions.push(Action::AttachPick(d));
            }
        }
        Cmd::Paste => {
            app.ui.focus_composer = true;
            app.paste(ctx);
        }
        Cmd::Fast => {
            let on = with_ctx(app, None, |c| cmds::control_row(c).and_then(|r| r.fast).map(|f| f.on));
            match (on, &thread, &app.client.state.selection) {
                (Some(on), Some(k), _) => actions.push(Action::SetFast(k.clone(), !on)),
                (Some(on), None, Selection::NewThread(id)) => actions.push(Action::DraftFast(*id, !on)),
                _ => {}
            }
        }
        Cmd::AllowOnce | Cmd::AllowSession | Cmd::Deny => {
            let picked = with_ctx(app, card.as_ref(), |c| cmds::card_decision(c, cmd));
            if let Some((key, request, d)) = picked {
                let verb = if cmd == Cmd::Deny { "Denied" } else { "Allowed" };
                actions.push(Action::Respond {
                    key,
                    request,
                    response: yaac_protocol::RequestResponse::Approval(yaac_protocol::ApprovalAnswer { decision: d.id.clone(), reason: None }),
                    summary: format!("{} ({verb})", d.label),
                });
            }
        }
        Cmd::AnswerQuestion => {
            if let Some(k) = &thread
                && let Some(t) = st.transcripts.get(k)
            {
                let q = t.items().iter().rev().find(|i| {
                    matches!(&i.body, yaac_protocol::ItemBody::Request(r) if r.state == yaac_protocol::RequestState::Pending && matches!(r.body, yaac_protocol::RequestBody::Question(_)))
                });
                if let Some(q) = q {
                    app.ui.scroll_to = Some(q.id.clone());
                    app.ui.focus_card = Some(q.id.clone());
                }
            }
        }
        Cmd::OpenPr | Cmd::CopyPrLink => {
            let url = thread.as_ref().and_then(|k| yaac_core::pr::badge_of(st, k)).and_then(|b| b.primary).map(|p| p.url).filter(|u| !u.is_empty());
            if let Some(url) = url {
                if cmd == Cmd::OpenPr {
                    actions.push(Action::OpenUrl(url));
                } else {
                    ctx.copy_text(url);
                }
            }
        }
        Cmd::CreatePr => {
            if let Some(k) = thread {
                actions.push(Action::OpenCreatePr(k));
            }
        }
        Cmd::ToggleSidebar => app.ui.sidebar.toggle(),
        Cmd::Dock => {
            let docked = app.ui.panel == yaac_core::agents::PanelMode::Docked;
            actions.push(Action::Panel(if docked { yaac_core::agents::PanelInput::Undock } else { yaac_core::agents::PanelInput::Dock }));
        }
        Cmd::UnseenFirst => app.ui.ordering.unseen_first = !app.ui.ordering.unseen_first,
        Cmd::NeedsFirst => app.ui.ordering.needs_first = !app.ui.ordering.needs_first,
        Cmd::RunningOnly => app.ui.ordering.running_only = !app.ui.ordering.running_only,
        Cmd::ShowDismissed => app.ui.grouping.show_dismissed = !app.ui.grouping.show_dismissed,
        Cmd::ShowArchived => app.ui.archived_open = !app.ui.archived_open,
        Cmd::ZoomIn => app.ui.font_size = (app.ui.font_size + ZOOM_STEP).min(ZOOM_MAX),
        Cmd::ZoomOut => app.ui.font_size = (app.ui.font_size - ZOOM_STEP).max(ZOOM_MIN),
        Cmd::ZoomReset => app.ui.font_size = size::UI,
        Cmd::ExpandAll => {
            if let Some(k) = thread {
                let map = yaac_core::attention::AttentionMap::compute(&app.client.state);
                yaac_core::activity::expand_all(&app.client.state, &map, &k, &mut app.ui.expansion);
            }
        }
        Cmd::CollapseAll => {
            if let Some(k) = thread {
                yaac_core::activity::collapse_all(&k, &mut app.ui.expansion);
            }
        }
        Cmd::CopySelection => ctx.send_viewport_cmd(egui::ViewportCommand::RequestCopy),
        Cmd::CopyMessage => {
            if let Some(text) = thread.as_ref().and_then(|k| cmds::last_agent_message(st, k)) {
                ctx.copy_text(text);
            }
        }
        Cmd::AddSsh => {
            actions.push(Action::SettingsPage(yaac_core::settings::Page::Targets));
            app.ui.set.ssh = Some(crate::settings::SshPanel::new(app.now(), None));
            app.ui.set.revoke = None;
        }
        Cmd::AddWsl => actions.push(Action::SettingsPage(yaac_core::settings::Page::Targets)),
        Cmd::Renderer => actions.push(Action::SettingsPage(yaac_core::settings::Page::Window)),
        Cmd::HideToTray => app.shell.hide(ctx),
        Cmd::Quit => {
            // With the tray in use, Quit is the tray's Quit. Without it, the window asks first.
            if app.shell.has_tray() && app.ui.tray_on_close {
                app.shell.quit(ctx);
            } else {
                app.ui.quit_confirm = true;
            }
        }
        // The steps were opened above.
        Cmd::FindThread
        | Cmd::History
        | Cmd::Related
        | Cmd::SettingsPage
        | Cmd::Model
        | Cmd::Effort
        | Cmd::Mode
        | Cmd::Agents
        | Cmd::Grouping
        | Cmd::Order
        | Cmd::Theme
        | Cmd::Retry
        | Cmd::StartWsl
        | Cmd::TargetEnable
        | Cmd::RefreshProviders => {}
    }
}

/// What choosing a step's row does.
fn run_step(app: &mut YaacApp, cmd: Cmd, value: StepValue, actions: &mut Vec<Action>) {
    let st = &app.client.state;
    let thread = st.selected_thread().cloned().filter(|_| !app.ui.settings);
    let draft = match st.selection {
        Selection::NewThread(id) if !app.ui.settings => Some(id),
        _ => None,
    };
    match value {
        StepValue::Thread(k) => actions.push(Action::Open(k)),
        StepValue::Nav(n) => actions.push(Action::Nav(n)),
        StepValue::Page(p) => actions.push(Action::SettingsPage(p)),
        StepValue::Model(inst, model) => match (thread, draft) {
            (Some(k), _) => actions.push(Action::SetModel(k, model)),
            (None, Some(id)) => actions.push(Action::DraftModel(id, inst, model)),
            _ => {}
        },
        StepValue::Effort(e) => match (thread, draft) {
            (Some(k), _) => actions.push(Action::SetEffort(k, e)),
            (None, Some(id)) => actions.push(Action::DraftEffort(id, e)),
            _ => {}
        },
        StepValue::Mode(m) => match (thread, draft) {
            (Some(k), _) => actions.push(Action::ChooseMode(yaac_core::mode_confirm::ModeTarget::Thread(k), m)),
            (None, Some(id)) => actions.push(Action::ChooseMode(yaac_core::mode_confirm::ModeTarget::Draft(id), m)),
            _ => {}
        },
        StepValue::Agents(pol) => {
            if let Some(k) = thread {
                actions.push(Action::SetAgents(k, pol));
            }
        }
        StepValue::MuteFor(secs) => {
            if let Some(k) = thread {
                let until = secs.map(|s| app.now() + s);
                actions.push(Action::MuteUntil(k, until));
            }
        }
        StepValue::MuteDate => {
            if let Some(k) = thread {
                actions.push(Action::OpenMute(k, false));
            }
        }
        StepValue::Order(o) => app.ui.ordering.order = o,
        StepValue::Grouping(f) => {
            let g = &mut app.ui.grouping;
            match f {
                cmds::GroupField::Machine => g.machine = !g.machine,
                cmds::GroupField::Project => g.project = !g.project,
                cmds::GroupField::Worktree => g.worktree = !g.worktree,
            }
        }
        StepValue::Theme(t) => actions.push(Action::Theme(match t {
            ThemePref::System => ThemeChoice::System,
            ThemePref::Dark => ThemeChoice::Dark,
            ThemePref::Light => ThemeChoice::Light,
        })),
        StepValue::Retry(t) => actions.push(Action::Retry(t)),
        StepValue::TargetEnabled(t, on) => actions.push(Action::TargetEnabled(t, on)),
        StepValue::Refresh(t) => actions.push(Action::RefreshProviders(t, None)),
    }
    let _ = cmd;
}

/// One row of what the Commander shows.
enum Row {
    Cmd(cmds::ListRow),
    Step(cmds::StepItem),
}

impl Row {
    fn enabled(&self) -> bool {
        match self {
            Row::Cmd(r) => r.avail.enabled(),
            Row::Step(s) => s.reason.is_none(),
        }
    }
}

fn rows(app: &YaacApp, c: &CommanderUi) -> Vec<Row> {
    match c.step {
        None => with_ctx(app, c.card.as_ref(), |x| cmds::list(x, &c.query, &app.ui.recent)).into_iter().map(Row::Cmd).collect(),
        Some((_, kind)) => {
            let exists = |p: &Place| app.place_exists(p);
            let mut history: Vec<(isize, String, Option<String>)> = Vec::new();
            if kind == StepKind::History {
                for forward in [false, true] {
                    for (n, e) in app.ui.history.menu(forward, &exists) {
                        let (t, m) = crate::nav::label(app, e);
                        let dir = if forward { "Forward" } else { "Back" };
                        history.push((n, t, Some(m.map_or_else(|| dir.to_owned(), |m| format!("{dir} · {m}")))));
                    }
                }
            }
            let items = with_ctx(app, c.card.as_ref(), |x| cmds::step_items(kind, x, &history));
            cmds::filter_step(items, &c.query).into_iter().map(Row::Step).collect()
        }
    }
}

/// The overlay, drawn after everything else.
pub(crate) fn show(ctx: &egui::Context, app: &mut YaacApp, actions: &mut Vec<Action>) {
    let Some(mut c) = app.ui.commander.take() else { return };
    let p = app.p;
    let list = rows(app, &c);
    // The keys the dispatcher took for it, so neither the field nor the page saw them.
    let Keys { down, up, enter, esc, back } = std::mem::take(&mut c.keys);
    if down && !list.is_empty() {
        c.sel = (c.sel + 1).min(list.len() - 1);
        c.reveal = true;
    }
    if up {
        c.sel = c.sel.saturating_sub(1);
        c.reveal = true;
    }
    if esc || back {
        // Esc in a step goes back to the list; on the list it closes.
        if c.step.is_some() {
            app.ui.commander = Some(CommanderUi::at(None, c.card));
        }
        return;
    }
    c.sel = c.sel.min(list.len().saturating_sub(1));
    let mut chosen: Option<usize> = if enter && list.get(c.sel).is_some_and(Row::enabled) { Some(c.sel) } else { None };
    let screen = ctx.content_rect();
    let width = 600.0_f32.min(screen.width() - 32.0);
    crate::dialogs::scrim(ctx, &p, "commander");
    let title = c.step.map(|(cmd, _)| cmds::def_of(cmd).name.trim_end_matches('…').to_owned());
    let mut close = false;
    let area = egui::Area::new(egui::Id::unique("commander"))
        .order(egui::Order::Foreground)
        .fixed_pos(pos2(screen.center().x - width / 2.0, screen.top() + (screen.height() * 0.14).max(48.0)))
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).fill(p.raised).stroke(egui::Stroke::new(1.0, p.border)).corner_radius(10).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
                ui.set_width(width - 16.0);
                ui.spacing_mut().item_spacing.y = 0.0;
                // A steady caret: a blinking one would repaint the idle window twice a second.
                ui.visuals_mut().text_cursor.blink = false;
                // The field, with the step's name before it.
                ui.allocate_ui_with_layout(vec2(ui.available_width(), 38.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.add_space(6.0);
                    if let Some(t) = &title {
                        egui::Frame::NONE.fill(p.faint).corner_radius(4).inner_margin(egui::Margin::symmetric(7, 3)).show(ui, |ui| {
                            ui.label(egui::RichText::new(t).font(theme::ui(size::SMALL)).color(p.text));
                        });
                    }
                    let hint = if c.step.is_some() { "Choose…" } else { "Type a command…" };
                    let field = egui::TextEdit::singleline(&mut c.query)
                        .id(egui::Id::unique("commander-field"))
                        .hint_text(widgets::secondary(hint, &p))
                        .font(theme::ui(size::SECTION))
                        .frame(egui::Frame::NONE)
                        .margin(egui::Margin::symmetric(4, 8))
                        .desired_width(f32::INFINITY);
                    let r = ui.add(field);
                    if c.focus {
                        r.request_focus();
                        c.focus = false;
                    }
                    if r.changed() {
                        c.sel = 0;
                        c.reveal = true;
                    }
                });
                ui.add_space(4.0);
                ui.painter().hline(ui.max_rect().x_range(), ui.cursor().top(), egui::Stroke::new(1.0, p.border));
                ui.add_space(5.0);
                if list.is_empty() {
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        ui.add_space(10.0);
                        ui.label(widgets::secondary(if c.step.is_some() { "Nothing to choose here." } else { "No command matches." }, &p));
                    });
                    ui.add_space(12.0);
                    return;
                }
                let max_h = (screen.height() * 0.6).clamp(200.0, 520.0);
                egui::ScrollArea::vertical().max_height(max_h).auto_shrink([false, true]).show(ui, |ui| {
                    for (i, row) in list.iter().enumerate() {
                        if let Row::Cmd(r) = row
                            && let Some(h) = r.heading
                        {
                            ui.add_space(if i == 0 { 2.0 } else { 8.0 });
                            ui.horizontal(|ui| {
                                ui.add_space(10.0);
                                ui.label(egui::RichText::new(h).font(theme::bold(size::SMALL)).color(p.text2));
                            });
                            ui.add_space(3.0);
                        }
                        let selected = i == c.sel;
                        let resp = draw_row(ui, app, row, selected);
                        if selected && c.reveal {
                            resp.scroll_to_me(None);
                        }
                        if resp.hovered() && ui.input(|i| i.pointer.delta() != egui::Vec2::ZERO) {
                            c.sel = i;
                        }
                        if resp.clicked() && row.enabled() {
                            chosen = Some(i);
                        }
                    }
                });
                c.reveal = false;
            });
        });
    // A click outside closes it.
    if ctx.input(|i| i.pointer.any_pressed()) && ctx.pointer_interact_pos().is_some_and(|pos| !area.response.rect.contains(pos)) {
        close = true;
    }
    if let Some(i) = chosen {
        match &list[i] {
            Row::Cmd(r) => {
                let cmd = r.cmd;
                cmds::note_recent(&mut app.ui.recent, cmds::def_of(cmd).id);
                let card = c.card.clone();
                // Runs with the Commander closed; a command with a step opens it again there.
                run(app, ctx, cmd, card, actions);
                return;
            }
            Row::Step(s) => {
                let (cmd, kind) = c.step.unwrap_or((Cmd::Commander, StepKind::Page));
                let value = s.value.clone();
                run_step(app, cmd, value, actions);
                // Grouping toggles several at once: the list stays open on it.
                if kind == StepKind::Grouping {
                    app.ui.commander = Some(c);
                }
                return;
            }
        }
    }
    if !close {
        app.ui.commander = Some(c);
    }
}

/// One row: the name (and a tick for a toggle or the current choice), the explanation — or
/// why it cannot run — in the secondary colour, the key on the right.
fn draw_row(ui: &mut egui::Ui, app: &YaacApp, row: &Row, selected: bool) -> egui::Response {
    let p = app.p;
    let (name, detail, key, tick, enabled) = match row {
        Row::Cmd(r) => {
            let d = cmds::def_of(r.cmd);
            let detail = match &r.avail {
                Avail::Disabled(why) => why.clone(),
                _ => d.explain.to_owned(),
            };
            (r.name.clone(), Some(detail), app.ui.bindings.label(r.cmd), r.checked, r.avail.enabled())
        }
        Row::Step(s) => (s.label.clone(), s.reason.clone().or_else(|| s.detail.clone()), String::new(), s.current.then_some(true), s.reason.is_none()),
    };
    let h = if detail.is_some() { 42.0 } else { 30.0 };
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::click());
    if selected {
        ui.painter().rect_filled(rect, 5.0, if enabled { p.selected } else { p.faint });
    } else if resp.hovered() && enabled {
        ui.painter().rect_filled(rect, 5.0, p.faint);
    }
    let left = rect.left() + 30.0;
    if tick == Some(true) {
        crate::icons::check(ui.painter(), pos2(rect.left() + 15.0, rect.top() + if detail.is_some() { 13.0 } else { h / 2.0 }), 12.0, if enabled { p.text } else { p.unavailable });
    }
    let key_w = if key.is_empty() {
        0.0
    } else {
        let g = ui.fonts_mut(|f| f.layout_no_wrap(key.clone(), theme::ui(size::SMALL), p.text2));
        let w = g.size().x;
        let y = if detail.is_some() { rect.top() + 13.0 } else { rect.center().y };
        let kr = egui::Rect::from_center_size(pos2(rect.right() - 12.0 - w / 2.0, y), vec2(w + 10.0, 18.0));
        ui.painter().rect_stroke(kr, 4.0, egui::Stroke::new(1.0, p.border), egui::StrokeKind::Inside);
        ui.painter().galley(pos2(kr.left() + 5.0, y - g.size().y / 2.0), g, p.text2);
        w + 30.0
    };
    let text_w = rect.right() - left - key_w - 8.0;
    let color = if enabled { p.text } else { p.unavailable };
    let g = widgets::elided(ui, &name, &theme::ui(size::UI), color, text_w);
    let top = if detail.is_some() { rect.top() + 5.0 } else { rect.center().y - g.size().y / 2.0 };
    ui.painter().galley(pos2(left, top), g, color);
    if let Some(d) = detail {
        let g = widgets::elided(ui, &d, &theme::ui(size::SMALL), p.text2, text_w);
        ui.painter().galley(pos2(left, rect.top() + 23.0), g, p.text2);
    }
    resp
}
