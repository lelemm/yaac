//! The WPF preview hosts the same window controller as the standard display. The
//! private pipe carries view state and UI input. It is independent of the hub protocol.
//!
//! The controller loop ([`serve`]) does not know its transport: commands arrive on a
//! [`Connection`] (fed by a pipe reader thread, or directly through a channel) and view frames
//! leave through any [`Write`], one JSON object per line. The WPF window uses its process's
//! pipes; the Android library uses channels (`crate::view_protocol`).

use std::io::{self, BufRead, Write};
use std::process::ExitCode;
#[cfg(windows)]
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};
use yaac_core::agents::{PanelInput, agents_panel};
use yaac_core::attachments::DraftRef;
use yaac_core::attention::AttentionMap;
use yaac_core::demo::{Scenario, Scene};
use yaac_core::settings::Page;
use yaac_core::state::Selection;
use yaac_core::{ThreadKey, sidebar};
use yaac_protocol::{Address, ItemId};

use crate::app::{Action, YaacApp};
use crate::theme::ThemeChoice;

mod agents;
mod browser;
mod composer;
mod dialogs;
mod files;
mod header;
mod markdown_view;
mod navigation;
mod pull_requests;
mod settings;
mod sidebar_view;
mod transcript;
mod terminal;
#[cfg(test)]
mod shell_tests;

#[cfg(windows)]
fn executable() -> Option<std::path::PathBuf> {
    let executable = std::env::current_exe().ok()?;
    let base = executable.parent()?;
    [base.join("wpf").join("Yaac.WpfPreview.exe"), base.join("Yaac.WpfPreview.exe")]
        .into_iter().find(|path| path.is_file())
}

pub(super) fn available() -> bool {
    #[cfg(windows)]
    {
        static AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        return *AVAILABLE.get_or_init(|| executable().is_some());
    }
    #[cfg(not(windows))]
    false
}


fn scenario_name(scenario: Scenario) -> &'static str {
    match scenario {
        Scenario::Fleet => "fleet",
        Scenario::Fresh => "fresh",
    }
}

#[cfg(windows)]
pub(super) struct Frontend {
    child: Child,
    commands: Connection,
    views: ChildStdin,
    tray: bool,
}

#[cfg(windows)]
impl Frontend {
    pub(super) fn start(launch: &crate::Launch) -> io::Result<Self> {
        Self::start_with_capture(launch, None)
    }

    fn start_with_capture(launch: &crate::Launch, capture: Option<&std::path::Path>) -> io::Result<Self> {
        let path = executable().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "the WPF window is missing"))?;
        let mut command = Command::new(path);
        command.args(["--pipe", "--theme", launch.theme.id()])
            .stdin(Stdio::piped()).stdout(Stdio::piped());
        if launch.tray { command.arg("--tray"); }
        if let Some(output) = capture { command.arg("--capture-ui").arg(output); }
        if let Some(scenario) = launch.demo {
            command.args(["--demo", scenario_name(scenario)]);
        }
        let mut child = command.spawn()?;
        let views = child.stdin.take().expect("piped WPF input");
        let commands = Connection::new(io::BufReader::new(child.stdout.take().expect("piped WPF output")));
        let ready = commands.receiver.recv_timeout(Duration::from_secs(12));
        if let Ok(Input::Command(frame)) = ready
            && string(&frame, "command") == "shell.ready"
        {
            return Ok(Self { child, commands, views, tray: frame["payload"]["tray"] == true });
        }
        let _ = child.kill();
        let _ = child.wait();
        Err(io::Error::other("the WPF window did not finish starting"))
    }

    pub(super) fn run(self, launch: crate::Launch) -> io::Result<()> {
        self.run_with_theme(launch, None)
    }

    fn run_with_theme(mut self, mut launch: crate::Launch, capture_theme: Option<crate::theme_capture::ThemeInput>) -> io::Result<()> {
        launch.tray = false;
        launch.shell.external_tray = self.tray;
        let result = serve(launch, self.commands, io::BufWriter::new(self.views), capture_theme);
        if result.as_ref().is_err_and(|error| error.kind() != io::ErrorKind::BrokenPipe) {
            let _ = self.child.kill();
        }
        let exit = self.child.wait()?;
        match result {
            Err(error) if error.kind() != io::ErrorKind::BrokenPipe => Err(error),
            _ if !exit.success() => Err(io::Error::other(format!("the WPF window exited with {exit}"))),
            _ => Ok(()),
        }
    }
}

pub(super) fn controller_main(scenario: Option<&str>, theme: Option<&str>) -> ExitCode {
    let scenario = match scenario {
        Some("fleet") => Scenario::Fleet,
        Some("fresh") => Scenario::Fresh,
        _ => {
            eprintln!("YAAC: expected a WPF demo scenario (fleet or fresh)");
            return ExitCode::FAILURE;
        }
    };
    let theme = match theme {
        Some("light") => ThemeChoice::Light,
        Some("system") => ThemeChoice::System,
        _ => ThemeChoice::Dark,
    };
    let launch = demo_launch(scenario, theme);
    let commands = Connection::new(io::BufReader::new(io::stdin()));
    match serve(launch, commands, io::stdout().lock(), None) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("YAAC WPF controller: {error}");
            ExitCode::FAILURE
        }
    }
}

/// The window controller on a scripted demo hub, as `--wpf-controller` starts it.
pub(crate) fn demo_launch(scenario: Scenario, theme: ThemeChoice) -> crate::Launch {
    crate::Launch {
        demo: Some(scenario),
        theme,
        shots: None,
        host_windows: true,
        #[cfg(feature = "hub")]
        attach: None,
        display: crate::display::DisplayState::default(),
        shell: crate::tray::Shell::new(),
        tray: false,
        updater: crate::updater::Updater::off("Offline demo"),
    }
}

pub(crate) enum Input {
    Command(Value),
    Wake,
    BrowserWake,
    Closed,
}

/// Where the controller's input arrives: UI commands from the transport, and its own wakes.
pub(crate) struct Connection {
    sender: mpsc::SyncSender<Input>,
    receiver: mpsc::Receiver<Input>,
}

impl Connection {
    /// Fed directly: the transport sends [`Input::Command`] (and [`Input::Closed`] to end the
    /// loop) through [`Connection::sender`].
    pub(crate) fn channel() -> Self {
        let (sender, receiver) = mpsc::sync_channel(64);
        Self { sender, receiver }
    }

    pub(crate) fn sender(&self) -> mpsc::SyncSender<Input> {
        self.sender.clone()
    }

    /// Fed by a reader thread: one JSON command per line of `input`; its end closes the loop.
    pub(crate) fn new(input: impl BufRead + Send + 'static) -> Self {
        let (sender, receiver) = mpsc::sync_channel(64);
        let stdin_sender = sender.clone();
        std::thread::spawn(move || {
            for line in input.lines() {
                let Ok(line) = line else { break };
                if let Ok(frame) = serde_json::from_str(&line)
                    && stdin_sender.send(Input::Command(frame)).is_err()
                {
                    return;
                }
            }
            let _ = stdin_sender.send(Input::Closed);
        });
        Self { sender, receiver }
    }
}

#[cfg(windows)]
pub(super) fn capture(theme: crate::theme_capture::ThemeInput, output: &std::path::Path) -> io::Result<()> {
    let launch = demo_launch(Scenario::Fleet, ThemeChoice::Dark);
    Frontend::start_with_capture(&launch, Some(output))?.run_with_theme(launch, Some(theme))
}

/// Runs the window controller until the input closes or the window quits. Each view frame is
/// written to `stdout` as one line of JSON and flushed.
pub(crate) fn serve(launch: crate::Launch, commands: Connection, mut stdout: impl Write, capture_theme: Option<crate::theme_capture::ThemeInput>) -> io::Result<()> {
    let Connection { sender, receiver } = commands;
    let browser_dirty = Arc::new(AtomicBool::new(false));
    let wake_browser = {
        let sender = sender.clone();
        let dirty = browser_dirty.clone();
        move || {
            if !dirty.swap(true, Ordering::AcqRel) {
                let _ = sender.try_send(Input::BrowserWake);
            }
        }
    };
    let ctx = egui::Context::default();
    let deadline = Arc::new(Mutex::new(None::<Instant>));
    let repaint = deadline.clone();
    ctx.set_request_repaint_callback(move |request| {
        if let Some(at) = Instant::now().checked_add(request.delay)
            && let Ok(mut next) = repaint.lock()
            && next.is_none_or(|pending| at < pending)
        {
            *next = Some(at);
            let _ = sender.try_send(Input::Wake);
        }
    });
    let mut app = YaacApp::from_ctx(&ctx, launch);
    if let Some(theme) = capture_theme {
        theme.apply(&mut app);
    }
    app.display.first_frame();
    app.images.use_wpf();
    let mut previous = Map::new();
    let mut composer_history = Arc::<[String]>::default();
    let mut selected_settings_target = None;
    let mut once = true;
    let mut paste_for: Option<String> = None;
    let mut insert = None;
    let mut paste_sequence = 0;
    let mut window_shown = false;
    let mut system_theme = None;
    let mut visible_pr_rows = Vec::new();
    let mut agent_rows_visible = false;
    let mut next_pr_check = None;
    let mut selection_epoch = 0;
    let mut markdown = markdown_view::MarkdownView::default();
    let mut preview_markdown = markdown_view::MarkdownView::default();
    let mut browser = browser::BrowserPipe::default();
    let mut refresh_browser = true;
    let mut browser_deadline = None;
    let result = (|| loop {
        let due = deadline
            .lock()
            .expect("repaint deadline")
            .is_some_and(|at| at <= Instant::now());
        if once || due {
            once = false;
            refresh_browser = true;
            let mut next = deadline.lock().expect("repaint deadline");
            if next.is_some_and(|at| at <= Instant::now()) {
                *next = None;
            }
            drop(next);
            // Drive the existing async workers and timers without creating any egui widgets.
            app.client.set_showing(
                (window_shown && !app.ui.settings)
                    .then(|| selected_thread(&app).filter(|key| app.browsers.chat_visible(key)).cloned())
                    .flatten()
                    .into_iter()
                    .collect(),
            );
            let mut output = ctx.run_ui(
                egui::RawInput {
                    system_theme,
                    time: Some(app.started.elapsed().as_secs_f64()),
                    ..Default::default()
                },
                |ui| {
                    app.upkeep(ui.ctx());
                    if window_shown && !app.ui.settings && !visible_pr_rows.is_empty() && app.shots.is_none() {
                        app.client.forge_visible(&visible_pr_rows, app.now());
                        // Preserve the wake time across earlier repaints from unrelated work.
                        let now = Instant::now();
                        let at = *next_pr_check.get_or_insert(now);
                        let at = if at <= now {
                            now + Duration::from_secs(u64::try_from(yaac_core::pr::VISIBLE_REFRESH_SECS).unwrap_or(300))
                        } else {
                            at
                        };
                        next_pr_check = Some(at);
                        ui.ctx().request_repaint_after(at.saturating_duration_since(now));
                    } else {
                        next_pr_check = None;
                    }
                    if window_shown && !app.ui.settings {
                        let now = app.now();
                        app.client.rate_limits_shown(now);
                        if let Some(key) = selected_thread(&app).cloned() {
                            app.tick_running_commands(ui.ctx(), &key);
                        }
                        if let Some(secs) = yaac_core::limits::next_change(&app.client.state.rate_limits.accounts, now) {
                            ui.ctx().request_repaint_after(Duration::from_secs(u64::try_from(secs).unwrap_or(60)));
                        }
                    }
                },
            );
            app.persist_prefs();
            if let Some(image) = app.images.take_wpf() {
                write_frame(&mut stdout, &json!({ "imageViewer": image }))?;
            }
            output.textures_delta.clear();
            let mut window = None;
            for viewport in output.viewport_output.values() {
                for command in &viewport.commands {
                    match command {
                        egui::ViewportCommand::Close => {
                            write_frame(&mut stdout, &json!({ "window": "quit" }))?;
                            return Ok(());
                        }
                        egui::ViewportCommand::Visible(false) => window = Some("hide"),
                        egui::ViewportCommand::Visible(true) | egui::ViewportCommand::Focus => window = Some("show"),
                        _ => {}
                    }
                }
            }
            if let Some(window) = window {
                write_frame(&mut stdout, &json!({ "window": window }))?;
            }
            for command in output.platform_output.commands {
                if let egui::OutputCommand::CopyText(text) = command {
                    write_frame(&mut stdout, &json!({ "copy": text }))?;
                }
            }
            for event in app.files.paste_events.drain(..) {
                if let egui::Event::Paste(text) = event
                    && let Some(key) = paste_for.take()
                {
                    paste_sequence += 1;
                    insert = Some((
                        key,
                        json!({ "id": paste_sequence.to_string(), "text": text }),
                    ));
                }
            }
            let composer_key = app.open_composer().map(|draft| draft.store_key());
            if insert
                .as_ref()
                .is_some_and(|(key, _)| Some(key) != composer_key.as_ref())
            {
                insert = None;
            }
            settings::upkeep(&mut app, selected_settings_target.as_ref());
            dialogs::upkeep(&mut app);
            app.pump_demo();
            app.client.poll_create_pr();
            sidebar_view::upkeep_search(&mut app);
            if window_shown {
                composer::upkeep(&mut app);
            }
            crate::agents_ui::poll_dock_hover(&mut app, &ctx);
            let current = view(
                &app,
                selected_settings_target.as_ref(),
                insert.as_ref().map(|(_, v)| v),
                selection_epoch,
                &mut markdown,
                &mut preview_markdown,
                (window_shown && !app.ui.settings && agent_rows_visible).then_some(&ctx),
            );
            let mut changes: Map<String, Value> = current
                .iter()
                .filter(|(key, value)| previous.get(*key) != Some(*value))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            if !Arc::ptr_eq(&composer_history, &app.ui.composer_history) {
                composer_history = app.ui.composer_history.clone();
                changes.insert("composerMessageHistory".into(), json!(composer_history.as_ref()));
            }
            if let Some(view) = changes.get_mut("transcript")
                && let Some(old) = previous.get("transcript")
            {
                transcript::diff(old, view);
            }
            if let Some(view) = changes.get_mut("transcriptPreview").filter(|view| view.is_object())
                && let Some(old) = previous.get("transcriptPreview")
            {
                transcript::diff(old, view);
            }
            if !changes.is_empty() {
                write_frame(&mut stdout, &json!({ "views": changes }))?;
                previous = current;
            }
        }
        let browser_woke = browser_dirty.swap(false, Ordering::AcqRel);
        if std::mem::take(&mut refresh_browser)
            || browser_woke
            || browser_deadline.is_some_and(|at| at <= Instant::now())
        {
            browser_deadline = browser
                .update(&mut app, window_shown, wake_browser.clone(), &mut stdout)?
                .and_then(|delay| Instant::now().checked_add(delay));
        }
        // A demo hub answers in place and has no waker: when the browser's requests (a folder,
        // a file, a diff) were answered just now, take another pass instead of waiting for input.
        if app.demo.is_some() && app.pump_demo() {
            once = true;
            continue;
        }
        let next = (*deadline.lock().expect("repaint deadline"))
            .into_iter()
            .chain(browser_deadline)
            .min();
        let input = match next {
            Some(at) => receiver.recv_timeout(at.saturating_duration_since(Instant::now())),
            None => receiver
                .recv()
                .map_err(|_| mpsc::RecvTimeoutError::Disconnected),
        };
        match input {
            Ok(Input::Command(frame)) => {
                let command = string(&frame, "command");
                let payload = &frame["payload"];
                if command == "shell.systemTheme" {
                    system_theme = Some(if payload["light"] == true { egui::Theme::Light } else { egui::Theme::Dark });
                    once = true;
                    continue;
                }
                if command.starts_with("browser.") || command.starts_with("files.") || command.starts_with("terminal.") {
                    match browser.command(&mut app, window_shown, command, payload) {
                        Ok(changed) => {
                            refresh_browser |= changed;
                            once |= changed && matches!(command, "files.add" | "files.mode" | "terminal.add" | "terminal.pasteDecision" | "terminal.panelHeight");
                        }
                        Err(message) => write_frame(&mut stdout, &json!({ "error": message }))?,
                    }
                    continue;
                }
                if command == "shell.visible" {
                    window_shown = payload["shown"] == true;
                    if !window_shown {
                        visible_pr_rows.clear();
                        agent_rows_visible = false;
                        crate::agents_ui::dock_hover(&mut app, &ctx, false, None);
                    }
                }
                if command == "shell.visibleRequests" {
                    if let Ok(cards) = serde_json::from_value(payload["cards"].clone()) {
                        app.client.set_visible_requests(cards);
                    }
                    continue;
                }
                if command == "shell.prVisible" {
                    agent_rows_visible = payload["agentsVisible"] == true;
                    visible_pr_rows = payload["ids"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .filter_map(|id| key(&app, id))
                        .collect();
                    once = true;
                    continue;
                }
                if command == "composer.action"
                    && matches!(string(payload, "id"), "paste" | "largeText")
                {
                    paste_for = Some(string(payload, "key").to_owned());
                }
                if command == "composer.edit" {
                    insert = None;
                }
                let had_selection = !app.ui.list_sel.is_empty();
                if let Err(message) = dispatch(
                    &mut app,
                    &ctx,
                    &mut selected_settings_target,
                    command,
                    payload,
                ) {
                    write_frame(&mut stdout, &json!({ "error": message }))?;
                }
                if command != "sidebar.selection" && had_selection && app.ui.list_sel.is_empty() {
                    selection_epoch += 1;
                }
                // Read the new state before the next command, keeping sends and selection
                // changes in their original order.
                once = true;
            }
            Ok(Input::Wake | Input::BrowserWake) | Err(mpsc::RecvTimeoutError::Timeout) => {}
            Ok(Input::Closed) | Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
        }
    })();
    app.client.flush_drafts();
    result
}

fn write_frame(stdout: &mut impl Write, frame: &Value) -> io::Result<()> {
    serde_json::to_writer(&mut *stdout, frame)?;
    writeln!(stdout)?;
    stdout.flush()
}

fn view(
    app: &YaacApp,
    settings_target: Option<&yaac_protocol::TargetId>,
    insert: Option<&Value>,
    selection_epoch: u64,
    markdown: &mut markdown_view::MarkdownView,
    preview_markdown: &mut markdown_view::MarkdownView,
    agent_ages_ctx: Option<&egui::Context>,
) -> Map<String, Value> {
    let state = &app.client.state;
    let attention = AttentionMap::compute(state);
    let footer = sidebar::hub_footer(state);
    let fresh =
        yaac_core::first_run::is_first_run(state) && matches!(state.selection, Selection::None);
    let back = navigation::places(app, false);
    let forward = navigation::places(app, true);
    let needs = sidebar::needs_you(state, &attention, app.now()).len();
    let mut shell = json!({
        "settings": app.ui.settings,
        "focusSearch": app.ui.focus_search,
        "demo": app.demo.is_some(), "trayOnClose": app.ui.tray_on_close,
        "scenes": Scene::ALL.into_iter().enumerate().map(|(id, s)| json!({ "id": id, "label": s.label() })).collect::<Vec<_>>(),
        "firstRun": fresh || matches!(state.hub.outcome, yaac_core::state::HelloOutcome::WindowOutdated { .. }),
        "scenario": scenario_name(app.scenario),
        "hub": { "reachable": footer.reachable, "up": footer.up, "down": footer.down },
        "dialog": dialogs::view(app),
        "canBack": !back.is_empty(), "canForward": !forward.is_empty(),
        "back": back, "forward": forward,
        "needs": needs, "trayTooltip": yaac_core::display::tray_tooltip(needs),
        "status": app.client.errors.iter().rfind(|e| e.key.is_none()).map(|e| json!({ "id": e.id, "text": e.text })),
        "stoppedWsl": if app.ui.settings { vec![] } else {
            crate::shell::stopped_wsl_notifications(app).into_iter()
                .map(|(target, notice)| json!({ "id": target, "label": notice })).collect::<Vec<_>>()
        },
        "theme": app.ui.theme.id(),
        "customPalette": (app.ui.theme == ThemeChoice::Custom).then(|| app.ui.themes.current()).flatten()
            .map(|t| crate::theme::Palette::custom(t).wpf_colors()),
        "fontScale": app.ui.font_size / crate::theme::size::UI,
        "fonts": app.fonts.native,
        "commander": navigation::command_view(app),
    });
    // Only for a client that asked for it (`shell.requestFeed`, the Android client): the
    // WPF window's frames are unchanged.
    if let Some(feed) = &state.hub.request_feed {
        shell["requests"] = requests_view(feed);
    }
    if let (Some(shell), Value::Object(header)) =
        (shell.as_object_mut(), header::fields(app, &attention))
    {
        shell.extend(header);
    }
    Map::from_iter([
        ("shell".into(), shell),
        ("paneSizing".into(), json!({
            "sidebarCollapsed": app.ui.sidebar.collapsed,
            "sidebarWidth": app.ui.sidebar.width,
            "dockWidth": app.ui.agents_width,
        })),
        ("captionUpdate".into(), caption_update(&app.updater.phase)),
        (
            "sidebar".into(),
            sidebar_view::view(app, &attention, selection_epoch),
        ),
        ("shortcuts".into(), json!(navigation::shortcuts(app))),
        ("transcript".into(), transcript::view(app, &attention, markdown)),
        ("transcriptPreview".into(), transcript::preview(app, &attention, preview_markdown)),
        ("composer".into(), composer::view(app, &attention, insert)),
        ("agents".into(), agents::view(app, &attention, agent_ages_ctx)),
        ("settings".into(), settings::view(app, settings_target)),
    ])
}

/// The hub's request reminder as a list (`shell.requests`): exactly the unseen requests the
/// desktop reminder counts, oldest first, with Allow once and Deny only where the toast offers
/// them. `threadId` is the thread's address (`target/thread`), as everywhere in the frames.
fn requests_view(feed: &yaac_protocol::attach::RequestFeed) -> Value {
    let items: Vec<Value> = feed.items.iter().map(|i| {
        let address = ThreadKey::new(i.location.target.clone(), i.location.thread_id.clone()).address();
        let decisions = if i.decisions.is_some() {
            json!([{ "id": "allow", "label": "Allow once" }, { "id": "deny", "label": "Deny" }])
        } else {
            json!([])
        };
        json!({
            "threadId": address.as_str(), "requestId": i.location.request_id, "itemId": i.item_id,
            "workerTitle": i.worker, "kind": i.kind, "title": i.title,
            "detail": format!("{} · {} · {}", i.worker, i.kind, i.title),
            "decisions": decisions, "createdAt": i.created_ms,
        })
    }).collect();
    json!({ "revision": feed.revision, "items": items })
}

/// The feed entry a `shell.requestAction` / `shell.requestOpen` names.
fn feed_location(payload: &Value) -> Option<yaac_protocol::attach::RequestLocation> {
    let key = ThreadKey::from_address(&Address(string(payload, "threadId").to_owned()))?;
    let request = string(payload, "requestId");
    (!request.is_empty()).then(|| yaac_protocol::attach::RequestLocation {
        target: key.target, thread_id: key.thread, request_id: request.into(),
    })
}

fn caption_update(phase: &crate::updater::Phase) -> Value {
    use crate::updater::CaptionAction;
    let Some(caption) = phase.caption() else { return Value::Null };
    let action = match caption.action {
        Some(CaptionAction::Download) => Some("download"),
        Some(CaptionAction::Install) => Some("install"),
        None => None,
    };
    json!({ "tooltip": caption.tooltip, "action": action, "progress": caption.progress })
}

#[cfg(test)]
#[test]
fn caption_update_tracks_the_shared_update_phases() {
    use crate::updater::{Found, Phase};
    use yaac_core::update::{Manifest, Pending};

    let manifest = Manifest::parse(include_str!("../../../scripts/testdata/manifest.sample.json")).unwrap();
    let found = Found { installer_url: manifest.installer().unwrap().url.clone(), manifest };
    assert!(caption_update(&Phase::Idle).is_null());
    assert_eq!(caption_update(&Phase::Available(found.clone()))["action"], "download");

    let progress = caption_update(&Phase::Downloading { found: found.clone(), done: 3, total: 4 });
    assert!(progress["action"].is_null());
    assert_eq!(progress["progress"], 0.75);
    assert_eq!(progress["tooltip"], "Downloading YAAC 0.1.0+1005 · 75%");

    let failed = caption_update(&Phase::Failed { found, message: "connection closed".into() });
    assert_eq!(failed["action"], "download");
    assert!(failed["tooltip"].as_str().unwrap().contains("Try again"));

    let pending = Pending {
        build: 1005, version: "0.1.0+1005".into(), path: "setup.exe".into(), sha256: String::new(), size: 0, attempted: false,
    };
    let ready = caption_update(&Phase::Ready(pending));
    assert_eq!(ready["action"], "install");
    assert!(ready["tooltip"].as_str().unwrap().contains("Restart to update"));
}

#[cfg(test)]
#[test]
fn a_wpf_pill_move_updates_the_window_preference_and_agents_view() {
    let ctx = egui::Context::default();
    let mut app = crate::app::tests::demo_app(&ctx);
    for (input, expected) in [(-1.0, 0.0), (0.25, 0.25), (2.0, 1.0)] {
        dispatch(&mut app, &ctx, &mut None, "agents.movePill", &json!({ "fraction": input })).unwrap();
        assert_eq!(app.ui.prefs().pill_position, expected);
        let map = AttentionMap::compute(&app.client.state);
        assert_eq!(agents::view(&app, &map, None)["pillPosition"], json!(expected));
    }
}

fn dispatch(
    app: &mut YaacApp,
    ctx: &egui::Context,
    settings_target: &mut Option<yaac_protocol::TargetId>,
    command: &str,
    payload: &Value,
) -> Result<(), String> {
    let action = match command {
        "image.close" => {
            app.images.close();
            None
        }
        "sidebar.open" => key(app, string(payload, "id")).map(|k| {
            if AttentionMap::compute(&app.client.state).of(&k).is_some() { Action::OpenRequest(k) } else { Action::Open(k) }
        }),
        "agents.open" => key(app, string(payload, "id")).map(Action::Open),
        "pr.action" => pull_requests::action(app, ctx, payload),
        "header.action" => {
            header::command(app, payload);
            None
        }
        "sidebar.new" | "shell.new" => Some(Action::NewThread),
        "settings.start" => Some(Action::StartFirstThread),
        "shell.settings" => Some(Action::Settings(true)),
        "shell.dismissStatus" => payload["id"].as_u64().map(|id| Action::DismissError(id, None)),
        "shell.startWsl" => {
            let id = yaac_protocol::TargetId::from(string(payload, "id"));
            if app.client.state.stopped_distro(&id).is_some() {
                app.ui.dismissed.insert(id.clone());
                Some(Action::Retry(id))
            } else {
                None
            }
        }
        "settings.close" => Some(Action::Settings(false)),
        "settings.page" => Page::ALL
            .into_iter()
            .find(|page| page.label().eq_ignore_ascii_case(string(payload, "id")))
            .map(Action::SettingsPage),
        "shell.back" => Some(Action::Nav(-1)),
        "shell.forward" => Some(Action::Nav(1)),
        "shell.history" => payload["steps"]
            .as_i64()
            .and_then(|steps| isize::try_from(steps).ok())
            .map(Action::Nav),
        "shell.focusSearch" => {
            app.ui.focus_search = true;
            None
        }
        cmd if cmd.starts_with("commander.") => return navigation::command(app, ctx, cmd, payload),
        "shell.needs" => Some(Action::NeedsNext),
        "shell.stream" => Some(Action::StreamReply),
        "shell.update" => match string(payload, "action") {
            "download" => {
                app.updater.download();
                None
            }
            "install" => {
                if let Err(error) = app.install_update() {
                    app.updater.notice = Some(error);
                }
                None
            }
            _ => None,
        },
        "shell.visible" => {
            app.shell.hidden = payload["hidden"] == true;
            None
        }
        "shell.ready" => {
            app.shell.external_tray = payload["tray"] == true;
            None
        }
        // The Android client says it is a touch screen (and back to pointer when a mouse is
        // attached): only texts that name an input mechanic change.
        // The embedder runs its own browser host (Android WebViews, P380).
        "shell.browserHost" => {
            app.browsers.enable_external_host();
            None
        }
        "shell.platform" => {
            if let Some(platform) = yaac_core::input::Platform::from_id(string(payload, "platform")) {
                app.ui.platform = platform;
            }
            None
        }
        "shell.inputMode" => {
            if let Some(mode) = yaac_core::input::InputMode::from_id(string(payload, "mode")) {
                app.ui.input = mode;
            }
            None
        }
        // The request feed (Android notifications, P355): subscribe; answer as the toast's
        // Allow once / Deny (refused by the hub when `revision` is stale); Open.
        "shell.requestFeed" => {
            app.client.subscribe_request_feed();
            None
        }
        "shell.requestAction" | "shell.requestOpen" => {
            let location = feed_location(payload).ok_or("No such request")?;
            let decision = (command == "shell.requestAction").then(|| string(payload, "decision"));
            if decision.is_some_and(|d| !matches!(d, "allow" | "deny")) {
                return Err("A notification answers only allow or deny".into());
            }
            // An answer names the revision the user saw (a changed list refuses it); Open is
            // harmless and goes with the current one.
            let current = app.client.state.hub.request_feed.as_ref().map(|f| f.revision);
            let revision = if decision.is_some() { payload["revision"].as_u64() } else { current }
                .ok_or("No request list from the hub, or no revision")?;
            app.client.respond_request_feed(&location, revision, decision)?;
            None
        }
        "shell.focusSearchDone" => {
            app.ui.focus_search = false;
            None
        }
        "shell.focusComposerDone" => {
            app.ui.focus_composer = false;
            None
        }
        "shell.shortcut" => {
            navigation::shortcut(app, ctx, payload);
            None
        }
        "shell.sidebar" => {
            app.ui.sidebar.toggle();
            None
        }
        "shell.resizeSidebar" => {
            if let Some(width) = payload["width"].as_f64() {
                #[allow(clippy::cast_possible_truncation, reason = "a width in points")]
                app.ui.sidebar.drag_to(width as f32);
            }
            None
        }
        "shell.scene" => payload["id"]
            .as_u64()
            .and_then(|i| usize::try_from(i).ok())
            .and_then(|i| Scene::ALL.get(i).copied())
            .map(Action::Scene),
        "shell.unreadDemo" => {
            use yaac_core::attention::Mark;
            use yaac_core::state::ViewPatch;
            let map = AttentionMap::compute(&app.client.state);
            let changes = selected_thread(app)
                .and_then(|k| agents_panel(&app.client.state, &map, k))
                .map_or_else(Vec::new, |panel| {
                    panel
                        .all()
                        .filter(|row| row.display.mark == Mark::Done)
                        .filter_map(|row| row.key.clone())
                        .map(|key| {
                            (
                                key,
                                ViewPatch {
                                    unseen: Some(true),
                                    ..Default::default()
                                },
                            )
                        })
                        .collect()
                });
            app.client.set_view_many(&changes);
            None
        }
        "shell.reset" => Some(Action::DemoReset),
        "agents.dock" => Some(Action::Panel(PanelInput::Dock)),
        "agents.undock" => Some(Action::Panel(PanelInput::Undock)),
        "agents.resizeDock" => {
            if let Some(width) = payload["width"].as_f64() {
                #[allow(clippy::cast_possible_truncation, reason = "a width in points")]
                let width = width as f32;
                app.ui.agents_width = yaac_core::prefs::agents_width(width);
            }
            None
        }
        "agents.movePill" => {
            if let Some(fraction) = payload["fraction"].as_f64() {
                app.ui.pill_position = fraction.clamp(0.0, 1.0) as f32;
            }
            None
        }
        "agents.hover" => {
            if let Some(owner) = selected_thread(app)
                && owner.address().as_str() == string(payload, "owner")
            {
                let hovered = key(app, string(payload, "id"));
                crate::agents_ui::dock_hover(app, ctx, payload["inside"] == true, hovered);
            }
            None
        }
        "agents.markRead" => {
            let rows = selected_thread(app).and_then(|key| {
                agents_panel(
                    &app.client.state,
                    &AttentionMap::compute(&app.client.state),
                    key,
                )
            });
            rows.map(|panel| {
                let unseen = |row: &yaac_core::agents::AgentRow| row.display.unseen.then(|| row.key.clone()).flatten();
                let keys = if payload["floating"] == true {
                    yaac_core::agents::card_groups(&panel).into_iter().flat_map(|(_, rows)| rows).filter_map(unseen).collect()
                } else {
                    panel.all().chain(&panel.siblings).filter_map(unseen).collect()
                };
                Action::MarkRead(keys)
            })
        }
        // The budget line's time-limit menu (P352): a listed choice, or any number of hours.
        "agents.timeLimit" => {
            let now = app.now();
            selected_thread(app)
                .and_then(|key| agents_panel(&app.client.state, &AttentionMap::compute(&app.client.state), key))
                .and_then(|panel| panel.budget)
                .filter(|budget| budget.can_set_time_limit && budget.root.as_str() == string(payload, "id"))
                .and_then(|budget| {
                    let secs = match payload.get("hours").and_then(serde_json::Value::as_u64) {
                        Some(hours) => Some(budget.added(now, u32::try_from(hours).ok().filter(|h| (1..=10_000).contains(h))?)),
                        None => {
                            let index = usize::try_from(payload.get("choice")?.as_u64()?).ok()?;
                            budget.time_menu(now).choices.into_iter().nth(index)?.wall_clock_secs
                        }
                    };
                    Some(Action::SetTimeLimit(budget.root, secs))
                })
        }
        "agents.continue" | "agents.action"
            if command == "agents.continue" || string(payload, "action") == "continueBudget" =>
        {
            let budget = selected_thread(app)
                .and_then(|key| {
                    agents_panel(
                        &app.client.state,
                        &AttentionMap::compute(&app.client.state),
                        key,
                    )
                })
                .and_then(|panel| panel.budget);
            budget
                .filter(|budget| budget.exhausted)
                .map(|budget| Action::ContinueBudget(budget.root))
        }
        cmd if cmd.starts_with("sidebar.") || cmd == "agents.action" => {
            return sidebar_view::command(app, ctx, cmd, payload);
        }
        cmd if cmd.starts_with("dialog.") => return dialogs::command(app, cmd, payload),
        cmd if cmd.starts_with("composer.") => return composer::command(app, cmd, payload),
        cmd if cmd.starts_with("transcript.") => return transcript::command(app, cmd, payload),
        cmd if cmd.starts_with("settings.") => {
            return settings::command(app, settings_target, cmd, payload);
        }
        _ => return Err(format!("Unknown preview command {command}")),
    };
    if let Some(action) = action {
        app.apply(vec![action]);
    }
    Ok(())
}

fn string<'a>(value: &'a Value, field: &str) -> &'a str {
    value[field].as_str().unwrap_or_default()
}

fn key(app: &YaacApp, id: &str) -> Option<ThreadKey> {
    ThreadKey::from_address(&Address(id.to_owned()))
        .filter(|key| app.client.state.summary(key).is_some())
}

fn selected_thread(app: &YaacApp) -> Option<&ThreadKey> {
    match &app.client.state.selection {
        Selection::Thread(key) => Some(key),
        _ => None,
    }
}

fn selected_item(app: &YaacApp, id: &str) -> Option<ItemId> {
    let id = ItemId::from(id);
    let thread = selected_thread(app)?;
    app.client.state.transcripts.get(thread)?.item(&id)?;
    Some(id)
}
